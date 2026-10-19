//! Collects the facts and the detail data (services, ports, firewall …) of one host over
//! an open SSH connection with POSIX shell commands, then runs the user's info snippets.
//! The built-in commands are constants – nothing from the host list or from the server is
//! ever put into a command line. Info snippets run exactly as the user typed them.

use std::time::Duration;

use anyhow::{Result, anyhow};
use russh::ChannelMsg;

use super::inventory_detail::{self as detail, Avail, HostDetail};
use super::inventory_info::{self as info, InfoResult, InfoSnippet};
use super::ssh::{FACTS_SCRIPT, SshHandle, exec};
use crate::model::HostFacts;

/// Output cap per command (service lists of big servers are long).
const OUT_CAP: usize = 1024 * 1024;
/// Per command; the commands of one host run side by side.
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Collected {
    pub facts: HostFacts,
    pub detail: HostDetail,
}

/// Wraps a script for `sh -c '…'` (single quotes escaped) so it works whatever the login
/// shell is.
pub fn sh_wrap(script: &str) -> String {
    format!("sh -c '{}'", script.replace('\'', "'\\''"))
}

async fn run(handle: &SshHandle, cmd: String) -> Result<String> {
    let work = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        // PowerShell waits for input on an open stdin in some configurations.
        let _ = ch.eof().await;
        let mut out = Vec::new();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() < OUT_CAP {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok(String::from_utf8_lossy(&out).into_owned())
    };
    tokio::time::timeout(TIMEOUT, work)
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
}

// --- Linux scripts ---------------------------------------------------------------

const LINUX_EXTRAS: &str = r#"export LC_ALL=C PATH="$PATH:/usr/sbin:/sbin"
echo '@@cpu'; (grep -m1 'model name' /proc/cpuinfo || grep -m1 -i '^model' /proc/cpuinfo) 2>/dev/null | cut -d: -f2-
echo '@@arch'; uname -m 2>/dev/null
echo '@@osver'; (. /etc/os-release 2>/dev/null; echo "$VERSION_ID")
echo '@@fqdn'; hostname -f 2>/dev/null
echo '@@virt'; systemd-detect-virt 2>/dev/null
echo '@@model'; cat /sys/devices/virtual/dmi/id/product_name 2>/dev/null
echo '@@df'; df -PkT 2>/dev/null
echo '@@ip'; ip -o addr show scope global 2>/dev/null
echo '@@hostname'; hostname -I 2>/dev/null
echo '@@users'; who 2>/dev/null"#;

const LINUX_SERVICES: &str = r#"export LC_ALL=C PATH="$PATH:/usr/sbin:/sbin"
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
echo '@@units'; systemctl list-units --type=service --all --no-pager --plain --no-legend 2>/dev/null
echo '@@files'; systemctl list-unit-files --type=service --no-pager --no-legend 2>/dev/null
else echo '@@nosystemd'; fi"#;

const LINUX_PORTS: &str = r#"export LC_ALL=C PATH="$PATH:/usr/sbin:/sbin"
S=; [ "$(id -u)" != 0 ] && command -v sudo >/dev/null 2>&1 && S="sudo -n"
if command -v ss >/dev/null 2>&1; then echo '@@ss'; { [ -n "$S" ] && $S ss -tulpn 2>/dev/null; } || ss -tulpn 2>/dev/null
elif command -v netstat >/dev/null 2>&1; then echo '@@netstat'; { [ -n "$S" ] && $S netstat -tulpn 2>/dev/null; } || netstat -tulpn 2>/dev/null
else echo '@@none'; fi"#;

/// `sudo -n` never asks for a password: without a passwordless rule it fails at once.
const LINUX_FIREWALL: &str = r#"export LC_ALL=C PATH="$PATH:/usr/sbin:/sbin"
S=; [ "$(id -u)" != 0 ] && S="sudo -n"
run() { n=$1; shift; o=$("$@" 2>&1 </dev/null); rc=$?; printf '@@fw %s %s\n%s\n' "$n" "$rc" "$o"; }
command -v ufw >/dev/null 2>&1 && run ufw $S ufw status verbose
command -v firewall-cmd >/dev/null 2>&1 && run firewalld $S firewall-cmd --list-all
command -v nft >/dev/null 2>&1 && run nft $S nft list ruleset
command -v iptables >/dev/null 2>&1 && run iptables $S iptables -S
true"#;

fn failed_to_avail<T>(r: &Result<T>) -> Avail {
    if r.is_ok() { Avail::Ok } else { Avail::Failed }
}

/// Facts and details of the host behind `handle`; `None` if it answered neither the
/// Linux commands nor `ver` (Windows OpenSSH with cmd.exe).
pub async fn collect(handle: &SshHandle, snippets: &[InfoSnippet]) -> Option<Collected> {
    let mut c = if let Ok((_, out)) = exec(handle, FACTS_SCRIPT).await
        && let Some(f) = HostFacts::parse(&out)
    {
        linux(handle, f).await
    } else {
        // Not a POSIX shell (Windows OpenSSH): name and version only; everything else
        // comes from the user's info snippets.
        let (_, out) = exec(handle, "ver & hostname").await.ok()?;
        let mut facts = HostFacts::parse_windows(&out)?;
        facts.collected = Some(chrono::Utc::now());
        let detail = HostDetail {
            platform: "other".into(),
            os_caption: facts.os_name.clone(),
            hostname: facts.hostname.clone(),
            services_avail: Avail::Missing,
            ports_avail: Avail::Missing,
            firewall: detail::Firewall {
                avail: Avail::Missing,
                ..Default::default()
            },
            collected: facts.collected,
            ..Default::default()
        };
        Collected { facts, detail }
    };
    c.detail.info = run_snippets(handle, snippets).await;
    Some(c)
}

/// Runs the snippets one after the other (a host should not get a burst of commands).
async fn run_snippets(handle: &SshHandle, snippets: &[InfoSnippet]) -> Vec<InfoResult> {
    let mut out = Vec::new();
    for s in snippets
        .iter()
        .filter(|s| s.runnable())
        .take(info::MAX_SNIPPETS)
    {
        let mut r = InfoResult {
            snippet: Some(s.id),
            name: s.name.trim().chars().take(80).collect(),
            at: Some(chrono::Utc::now()),
            ..Default::default()
        };
        match run_raw(handle, s.command.trim()).await {
            Ok((status, raw)) => {
                (r.output, r.truncated) = info::clean_output(&raw);
                r.exit = Some(status);
            }
            Err(e) => r.error = format!("{e:#}"),
        }
        out.push(r);
    }
    out
}

/// stdout and stderr together (a failing command explains itself there), capped.
async fn run_raw(handle: &SshHandle, cmd: &str) -> Result<(u32, Vec<u8>)> {
    let work = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        let _ = ch.eof().await;
        let mut out = Vec::new();
        let mut status = 0;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    if out.len() < info::MAX_OUTPUT * 2 {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => status = exit_status,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok((status, out))
    };
    tokio::time::timeout(TIMEOUT, work)
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
}

async fn linux(handle: &SshHandle, mut facts: HostFacts) -> Collected {
    facts.collected = Some(chrono::Utc::now());
    let (extras, services, ports, firewall) = tokio::join!(
        run(handle, sh_wrap(LINUX_EXTRAS)),
        run(handle, sh_wrap(LINUX_SERVICES)),
        run(handle, sh_wrap(LINUX_PORTS)),
        run(handle, sh_wrap(LINUX_FIREWALL)),
    );
    let mut d = HostDetail {
        platform: "linux".into(),
        os_caption: facts.os_name.clone(),
        hostname: facts.hostname.clone(),
        collected: facts.collected,
        ..Default::default()
    };
    if let Ok(t) = &extras {
        detail::parse_linux_extras(t, &mut d);
    }
    match &services {
        Ok(t) => (d.services, d.services_total, d.services_avail) = detail::parse_linux_services(t),
        Err(_) => d.services_avail = Avail::Failed,
    }
    match &ports {
        Ok(t) => (d.ports, d.ports_avail) = detail::parse_linux_ports(t),
        Err(_) => d.ports_avail = Avail::Failed,
    }
    d.firewall = match &firewall {
        Ok(t) => detail::parse_linux_firewall(t),
        Err(_) => detail::Firewall {
            avail: failed_to_avail(&firewall),
            ..Default::default()
        },
    };
    Collected { facts, detail: d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(sh_wrap("echo 'a'"), "sh -c 'echo '\\''a'\\'''");
    }
}
