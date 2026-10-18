//! Diagnostics bundle: read-only commands over their own SSH connection (optionally with
//! `sudo -S`, the password goes to stdin and is never stored), optional redaction of
//! addresses and names, written as a `.tar.gz` with a `summary.txt`.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use russh::ChannelMsg;

use super::ssh::{SshHandle, SshParams, connect};
use super::{HostKeyPrompt, Sink};

/// One thing to collect: file name in the bundle and the command.
#[derive(Clone, Debug, PartialEq)]
pub struct DiagItem {
    /// Group caption ("System", "Logs" …).
    pub group: &'static str,
    pub file: String,
    pub command: String,
}

/// Built-in items: (group, file, command).
pub const BUILTIN: &[(&str, &str, &str)] = &[
    ("System", "uname.txt", "uname -a"),
    ("System", "os-release.txt", "cat /etc/os-release"),
    ("System", "uptime.txt", "uptime"),
    ("System", "df.txt", "df -h"),
    ("System", "free.txt", "free -m"),
    ("System", "ip-addr.txt", "ip addr"),
    ("System", "ip-route.txt", "ip route"),
    ("System", "ss-listening.txt", "ss -tulpn"),
    (
        "Logs",
        "journal-warnings.txt",
        "journalctl -p warning --since '24h ago' -n 2000 --no-pager",
    ),
    ("Logs", "dmesg.txt", "dmesg -T | tail -n 500"),
    (
        "Logs",
        "syslog-tail.txt",
        "if [ -r /var/log/syslog ]; then tail -n 1000 /var/log/syslog; elif [ -r /var/log/messages ]; then tail -n 1000 /var/log/messages; else echo 'no /var/log/syslog or /var/log/messages (or not readable)'; fi",
    ),
    (
        "Dienste",
        "systemctl-failed.txt",
        "systemctl --failed --no-pager",
    ),
    (
        "Dienste",
        "services-running.txt",
        "systemctl list-units --type=service --state=running --no-pager",
    ),
    (
        "Container",
        "docker-ps.txt",
        "if command -v docker >/dev/null 2>&1; then docker ps -a; else echo 'docker not installed'; fi",
    ),
];

pub fn builtin_items() -> Vec<DiagItem> {
    BUILTIN
        .iter()
        .map(|(g, f, c)| DiagItem {
            group: g,
            file: f.to_string(),
            command: c.to_string(),
        })
        .collect()
}

/// Extra commands, one per line, as `custom-N.txt` items.
pub fn custom_items(text: &str) -> Vec<DiagItem> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .enumerate()
        .map(|(i, l)| DiagItem {
            group: "Eigene",
            file: format!("custom-{}.txt", i + 1),
            command: l.to_string(),
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct ItemResult {
    pub status: u32,
    /// stdout followed by stderr.
    pub output: Vec<u8>,
    pub took: Duration,
    pub error: Option<String>,
}

#[derive(Debug)]
pub enum DiagEvent {
    Connected { hostname: String },
    Started(usize),
    Finished(usize, ItemResult),
    Failed(String),
    Done,
}

/// Output beyond this size (per item) is cut off.
pub const MAX_ITEM: usize = 8 * 1024 * 1024;
const ITEM_TIMEOUT: Duration = Duration::from_secs(120);

/// POSIX single-quoting.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The line sent to the server for one command.
pub fn wrap(cmd: &str, sudo: bool) -> String {
    let inner = format!("env LC_ALL=C sh -c {}", sh_quote(cmd));
    if sudo {
        format!("sudo -S -p '' {inner}")
    } else {
        inner
    }
}

/// Collects every item on a fresh connection; `sudo` holds the password for `sudo -S`.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    params: SshParams,
    items: Vec<DiagItem>,
    sudo: Option<String>,
    sink: Sink<DiagEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        if let Err(e) = run(&params, &items, sudo, &sink, prompts).await {
            sink.send(DiagEvent::Failed(format!("{e:#}")));
        }
    });
}

async fn run(
    p: &SshParams,
    items: &[DiagItem],
    sudo: Option<String>,
    sink: &Sink<DiagEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> Result<()> {
    let handle = connect(p, prompts).await?;
    let hostname = exec_capture(&handle, "uname -n", None, 4096, Duration::from_secs(20))
        .await
        .map(|(_, out, _)| String::from_utf8_lossy(&out).trim().to_string())
        .unwrap_or_default();
    let stdin = sudo.map(|pw| format!("{pw}\n"));
    if let Some(pw) = &stdin {
        // Check the password once instead of failing (and maybe locking the account) on
        // every item.
        let (status, out, err) = exec_capture(
            &handle,
            "sudo -S -p '' true",
            Some(pw),
            64 * 1024,
            Duration::from_secs(30),
        )
        .await?;
        if status != 0 {
            let msg = String::from_utf8_lossy(&err).trim().to_string();
            let msg = if msg.is_empty() {
                String::from_utf8_lossy(&out).trim().to_string()
            } else {
                msg
            };
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
            bail!("{}", t!("sudo abgelehnt: {}", msg));
        }
    }
    sink.send(DiagEvent::Connected { hostname });
    for (i, item) in items.iter().enumerate() {
        if sink.is_closed() {
            break;
        }
        sink.send(DiagEvent::Started(i));
        let start = Instant::now();
        let line = wrap(&item.command, stdin.is_some());
        let res = exec_capture(&handle, &line, stdin.as_deref(), MAX_ITEM, ITEM_TIMEOUT).await;
        let result = match res {
            Ok((status, mut out, err)) => {
                if !err.is_empty() {
                    if !out.is_empty() && !out.ends_with(b"\n") {
                        out.push(b'\n');
                    }
                    out.extend_from_slice(&err);
                }
                out.truncate(MAX_ITEM);
                ItemResult {
                    status,
                    output: out,
                    took: start.elapsed(),
                    error: None,
                }
            }
            Err(e) => ItemResult {
                status: 255,
                output: Vec::new(),
                took: start.elapsed(),
                error: Some(format!("{e:#}")),
            },
        };
        sink.send(DiagEvent::Finished(i, result));
    }
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await;
    sink.send(DiagEvent::Done);
    Ok(())
}

/// Runs `cmd` on its own channel, optionally feeding `stdin`; returns (status, stdout, stderr).
async fn exec_capture(
    handle: &SshHandle,
    cmd: &str,
    stdin: Option<&str>,
    cap: usize,
    timeout: Duration,
) -> Result<(u32, Vec<u8>, Vec<u8>)> {
    let run = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        if let Some(input) = stdin {
            ch.data(input.as_bytes()).await?;
        }
        ch.eof().await?;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut status = 0;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() < cap {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    if err.len() < cap {
                        err.extend_from_slice(&data);
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => status = exit_status,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok((status, out, err))
    };
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
}

// ------------------------------------------------------------------ redaction

/// Replaces IPv4/IPv6 addresses, MAC addresses, e-mail addresses and host names by stable
/// placeholders (`<ip-1>`, `<host-2>` …) – the same value gets the same placeholder in
/// every file of a bundle. Loopback/unspecified addresses stay.
#[derive(Default)]
pub struct Redactor {
    /// Lower-case names that are always masked (the server's own names).
    names: Vec<String>,
    map: HashMap<String, String>,
    counts: HashMap<&'static str, usize>,
}

const TLDS: &[&str] = &[
    "com",
    "net",
    "org",
    "de",
    "at",
    "ch",
    "eu",
    "io",
    "dev",
    "local",
    "lan",
    "internal",
    "intra",
    "intranet",
    "corp",
    "home",
    "arpa",
    "uk",
    "fr",
    "nl",
    "it",
    "info",
    "biz",
    "cloud",
    "app",
    "me",
    "us",
    "co",
    "edu",
    "gov",
    "localdomain",
    "test",
    "example",
];

impl Redactor {
    pub fn new(names: &[String]) -> Self {
        let mut names: Vec<String> = names
            .iter()
            .map(|n| n.trim().to_lowercase())
            .filter(|n| n.len() >= 2 && n != "localhost" && n.parse::<std::net::IpAddr>().is_err())
            .collect();
        // Short names of FQDNs too.
        let shorts: Vec<String> = names
            .iter()
            .filter_map(|n| n.split('.').next().filter(|s| s.len() >= 2 && *s != n))
            .map(str::to_string)
            .collect();
        names.extend(shorts);
        names.sort();
        names.dedup();
        Self {
            names,
            ..Default::default()
        }
    }

    pub fn redact(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut run_start: Option<usize> = None;
        for (i, c) in text.char_indices() {
            if is_token_char(c) {
                run_start.get_or_insert(i);
            } else {
                if let Some(s) = run_start.take() {
                    out.push_str(&self.token(&text[s..i], true));
                }
                out.push(c);
            }
        }
        if let Some(s) = run_start {
            out.push_str(&self.token(&text[s..], true));
        }
        out
    }

    fn placeholder(&mut self, kind: &'static str, value: &str) -> String {
        let key = format!("{kind}:{}", value.to_lowercase());
        if let Some(p) = self.map.get(&key) {
            return p.clone();
        }
        let n = self.counts.entry(kind).or_insert(0);
        *n += 1;
        let p = format!("<{kind}-{n}>");
        self.map.insert(key, p.clone());
        p
    }

    /// One run of token characters; `split` allows one more pass on its parts.
    fn token(&mut self, run: &str, split: bool) -> String {
        let core = run.trim_end_matches(['.', ':', '-', '_']);
        let tail = &run[core.len()..];
        if core.is_empty() {
            return run.to_string();
        }
        if let Some(kind) = classify(core, &self.names) {
            let mut s = self.placeholder(kind.0, kind.1);
            s.push_str(&core[kind.1.len()..]);
            s.push_str(tail);
            return s;
        }
        if split && core.contains([':', '=', '@']) {
            let mut out = String::new();
            let mut last = 0;
            for (i, c) in core.char_indices() {
                if matches!(c, ':' | '=' | '@') {
                    out.push_str(&self.token(&core[last..i], false));
                    out.push(c);
                    last = i + 1;
                }
            }
            out.push_str(&self.token(&core[last..], false));
            out.push_str(tail);
            return out;
        }
        run.to_string()
    }
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_' | '@' | '%' | '+')
}

/// (kind, matched prefix of `s`) if `s` is (or starts with, for `ip:port`) something to mask.
fn classify<'a>(s: &'a str, names: &[String]) -> Option<(&'static str, &'a str)> {
    if is_email(s) {
        return Some(("mail", s));
    }
    if is_mac(s) {
        let l = s.to_lowercase();
        if l != "00:00:00:00:00:00" && l != "ff:ff:ff:ff:ff:ff" {
            return Some(("mac", s));
        }
        return None;
    }
    if s.matches(':').count() >= 2 {
        let addr = s.split('%').next().unwrap_or(s);
        if let Ok(ip) = addr.parse::<std::net::Ipv6Addr>() {
            if ip.is_loopback() || ip.is_unspecified() {
                return None;
            }
            if let Some(v4) = ip.to_ipv4_mapped() {
                return keep_v4(v4).then_some(("ip", s));
            }
            return Some(("ip6", s));
        }
    }
    // IPv4, maybe with `:port`.
    let (addr, port) = match s.rsplit_once(':') {
        Some((a, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (a, true),
        _ => (s, false),
    };
    if let Ok(v4) = addr.parse::<std::net::Ipv4Addr>()
        && addr.split('.').count() == 4
    {
        return keep_v4(v4).then_some(("ip", if port { addr } else { s }));
    }
    let lower = s.to_lowercase();
    if names.contains(&lower) || is_fqdn(&lower) {
        return Some(("host", s));
    }
    None
}

/// Worth masking (not loopback, unspecified or a netmask).
fn keep_v4(ip: std::net::Ipv4Addr) -> bool {
    !(ip.is_loopback() || ip.is_unspecified() || ip.octets()[0] == 255)
}

fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
        && domain.contains('.')
        && domain
            .split('.')
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && domain
            .rsplit('.')
            .next()
            .is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()))
}

fn is_mac(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 17 {
        return false;
    }
    let sep = b[2];
    if sep != b':' && sep != b'-' {
        return false;
    }
    (0..17).all(|i| {
        if i % 3 == 2 {
            b[i] == sep
        } else {
            b[i].is_ascii_hexdigit()
        }
    })
}

/// `name.domain.tld` with a known top-level domain.
fn is_fqdn(s: &str) -> bool {
    let labels: Vec<&str> = s.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
        })
        && labels.last().is_some_and(|t| TLDS.contains(t))
        // `10.0.0.de`? Real names have at least one letter in front of the TLD.
        && labels[..labels.len() - 1]
            .iter()
            .any(|l| l.chars().any(|c| c.is_ascii_alphabetic()))
}

// ------------------------------------------------------------------ tar.gz

/// Writes `entries` (path inside the archive, content) as a gzip-compressed ustar archive.
pub fn write_tar_gz(path: &Path, entries: &[(String, Vec<u8>)], mtime: u64) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::File::create(path)?;
    let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    gz.write_all(&tar_bytes(entries, mtime)?)?;
    gz.finish()?.sync_all()?;
    Ok(())
}

/// Uncompressed ustar archive.
pub fn tar_bytes(entries: &[(String, Vec<u8>)], mtime: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (name, data) in entries {
        out.extend_from_slice(&tar_header(name, data.len() as u64, mtime)?);
        out.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    // Two zero blocks end the archive.
    out.extend(std::iter::repeat_n(0u8, 1024));
    Ok(out)
}

fn tar_header(name: &str, size: u64, mtime: u64) -> Result<[u8; 512]> {
    let mut h = [0u8; 512];
    let bytes = name.as_bytes();
    // Long names go into the ustar prefix field (split at a '/').
    let (prefix, base) = if bytes.len() <= 100 {
        (&[][..], bytes)
    } else {
        let cut = name[..name.len().min(155)]
            .rfind('/')
            .filter(|&i| name.len() - i - 1 <= 100)
            .ok_or_else(|| anyhow!("name too long for tar: {name}"))?;
        (&bytes[..cut], &bytes[cut + 1..])
    };
    h[..base.len()].copy_from_slice(base);
    let octal = |field: &mut [u8], v: u64| {
        let s = format!("{:0width$o}", v, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
    };
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime);
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[265..271].copy_from_slice(b"aurora");
    h[297..303].copy_from_slice(b"aurora");
    h[345..345 + prefix.len()].copy_from_slice(prefix);
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn redaction() {
        let mut r = Redactor::new(&["web-01.example.com".into(), "lab".into()]);
        let text = "inet 192.168.1.20/24 brd 192.168.1.255 scope global eth0\n\
                    link/ether 52:54:00:12:34:56 brd ff:ff:ff:ff:ff:ff\n\
                    inet6 fe80::5054:ff:fe12:3456/64 scope link\n\
                    LISTEN 0 128 127.0.0.1:631 0.0.0.0:* users:((\"cupsd\"))\n\
                    tcp 10.0.0.5:22 [::1]:22\n\
                    Mail an admin@firma.de von web-01: ok. Host web-01.example.com, db.internal.\n\
                    aurora@lab:~$ systemctl status nginx.service; uname 6.8.0-45-generic; 255.255.255.0\n\
                    again 192.168.1.20 at 12:34:56";
        let out = r.redact(text);
        assert!(out.contains("inet <ip-1>/24 brd <ip-2> scope"), "{out}");
        assert!(
            out.contains("link/ether <mac-1> brd ff:ff:ff:ff:ff:ff"),
            "{out}"
        );
        assert!(out.contains("inet6 <ip6-1>/64"), "{out}");
        assert!(out.contains("127.0.0.1:631 0.0.0.0:*"), "{out}");
        assert!(out.contains("tcp <ip-3>:22 [::1]:22"), "{out}");
        assert!(out.contains("Mail an <mail-1> von <host-1>: ok."), "{out}");
        assert!(out.contains("Host <host-2>, <host-3>."), "{out}");
        assert!(out.contains("aurora@<host-4>:~$"), "{out}");
        assert!(out.contains("nginx.service"), "{out}");
        assert!(out.contains("6.8.0-45-generic"), "{out}");
        assert!(out.contains("255.255.255.0"), "{out}");
        // Same address, same placeholder; times are no addresses.
        assert!(out.contains("again <ip-1> at 12:34:56"), "{out}");
        for leak in [
            "192.168", "52:54:00", "fe80", "10.0.0.5", "firma.de", "web-01",
        ] {
            assert!(!out.contains(leak), "{leak} in {out}");
        }
    }

    #[test]
    fn quoting_and_wrapping() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(wrap("df -h", false), "env LC_ALL=C sh -c 'df -h'");
        assert_eq!(
            wrap("dmesg -T | tail", true),
            "sudo -S -p '' env LC_ALL=C sh -c 'dmesg -T | tail'"
        );
        let c = custom_items("lsblk\n\n# note\n cat /proc/mdstat ");
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].file, "custom-2.txt");
        assert_eq!(c[1].command, "cat /proc/mdstat");
        assert_eq!(builtin_items().len(), BUILTIN.len());
    }

    /// Reads back what `tar_bytes` wrote: (name, content) of each member.
    fn untar(data: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos + 512 <= data.len() {
            let h = &data[pos..pos + 512];
            if h.iter().all(|b| *b == 0) {
                break;
            }
            let sum: u32 = h
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        *b as u32
                    }
                })
                .sum();
            let stored = std::str::from_utf8(&h[148..154]).unwrap();
            assert_eq!(u32::from_str_radix(stored, 8).unwrap(), sum);
            assert_eq!(&h[257..262], b"ustar");
            let field = |r: std::ops::Range<usize>| {
                String::from_utf8(h[r].iter().take_while(|b| **b != 0).copied().collect()).unwrap()
            };
            let mut name = field(0..100);
            let prefix = field(345..500);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
            let size = u64::from_str_radix(field(124..136).trim(), 8).unwrap() as usize;
            pos += 512;
            out.push((name, data[pos..pos + size].to_vec()));
            pos += size.div_ceil(512) * 512;
        }
        out
    }

    #[test]
    fn tar_round_trip() {
        let long = format!("{}/{}", "d".repeat(80), "f".repeat(60));
        let entries = vec![
            ("bundle/summary.txt".to_string(), b"hello\n".to_vec()),
            ("bundle/empty.txt".to_string(), Vec::new()),
            ("bundle/big.txt".to_string(), vec![b'x'; 1500]),
            (long.clone(), b"long".to_vec()),
        ];
        let tar = tar_bytes(&entries, 1_700_000_000).unwrap();
        assert_eq!(tar.len() % 512, 0);
        assert_eq!(untar(&tar), entries);
        let dir = std::env::temp_dir().join(format!("aurora-tar-{}", uuid::Uuid::new_v4()));
        let path = dir.join("x.tar.gz");
        write_tar_gz(&path, &entries, 0).unwrap();
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(&path).unwrap())
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(untar(&raw), entries);
        let _ = std::fs::remove_dir_all(dir);
        assert!(tar_bytes(&[("x".repeat(300), Vec::new())], 0).is_err());
    }
}
