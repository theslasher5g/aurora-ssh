//! Facts, services, firewall rules and listening ports of a Windows host over WinRM,
//! stored in the same inventory model as the Linux collector. The PowerShell scripts are
//! constants (nothing from the host list is ever put into them); the parsers are pure
//! functions that are tested with sample output.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::inventory_collect::Collected;
use super::inventory_detail::{
    self as detail, Avail, DiskInfo, Firewall, FwRule, HostDetail, NetAddr, PortInfo, ServiceInfo,
    clip, sections,
};
use super::inventory_info::{self as info, InfoResult, InfoSnippet};
use super::winrm_client::{RunError, Session};
use crate::model::HostFacts;

const CAP: usize = 1024 * 1024;

pub const FACTS_SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
$os=Get-CimInstance Win32_OperatingSystem
$cs=Get-CimInstance Win32_ComputerSystem
$cpu=@(Get-CimInstance Win32_Processor)
'@@os'
"caption=$($os.Caption)"
"version=$($os.Version)"
"build=$($os.BuildNumber)"
"arch=$($os.OSArchitecture)"
"uptime=$([int]((Get-Date)-$os.LastBootUpTime).TotalSeconds)"
"host=$($env:COMPUTERNAME)"
"domain=$($cs.Domain)"
"partofdomain=$($cs.PartOfDomain)"
"model=$($cs.Manufacturer) $($cs.Model)"
"ramkb=$($os.TotalVisibleMemorySize)"
"freekb=$($os.FreePhysicalMemory)"
"cpu=$($cpu[0].Name)"
"sockets=$($cpu.Count)"
"logical=$(($cpu|Measure-Object NumberOfLogicalProcessors -Sum).Sum)"
"user=$($cs.UserName)"
'@@disks'
Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object { "$($_.DeviceID)`t$($_.VolumeName)`t$($_.FileSystem)`t$($_.Size)`t$($_.FreeSpace)" }
'@@addr'
Get-NetIPAddress | Where-Object { $_.IPAddress -notlike '127.*' -and $_.IPAddress -ne '::1' -and $_.IPAddress -notlike 'fe80*' -and $_.IPAddress -notlike '169.254*' } | ForEach-Object { "$($_.InterfaceAlias)`t$($_.IPAddress)/$($_.PrefixLength)" }"#;

/// Pending Windows updates (can take a minute; optional).
pub const UPDATES_SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
$s=New-Object -ComObject Microsoft.Update.Session
$r=$s.CreateUpdateSearcher().Search('IsInstalled=0 and IsHidden=0')
$sec=0
foreach($u in $r.Updates){ foreach($c in $u.Categories){ if($c.Name -eq 'Security Updates'){ $sec++; break } } }
"updates=$($r.Updates.Count)"
"security=$sec""#;

pub const SERVICES_SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
$ex=@{}
Get-CimInstance Win32_Service | ForEach-Object { $ex[$_.Name]=$_.ExitCode }
Get-Service | ForEach-Object { "$($_.Name)`t$($_.DisplayName)`t$($_.Status)`t$($_.StartType)`t$($ex[$_.Name])" }"#;

pub const PORTS_SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
$pn=@{}
Get-Process | ForEach-Object { $pn[[int]$_.Id]=$_.ProcessName }
Get-NetTCPConnection -State Listen | ForEach-Object { "tcp`t$($_.LocalAddress)`t$($_.LocalPort)`t$($pn[[int]$_.OwningProcess])" }
Get-NetUDPEndpoint | ForEach-Object { "udp`t$($_.LocalAddress)`t$($_.LocalPort)`t$($pn[[int]$_.OwningProcess])" }"#;

pub const FIREWALL_SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
'@@profiles'
Get-NetFirewallProfile | ForEach-Object { "$($_.Name)`t$($_.Enabled)`t$($_.DefaultInboundAction)`t$($_.DefaultOutboundAction)" }
'@@rules'
$pf=@{}
Get-NetFirewallPortFilter -All | ForEach-Object { $pf[$_.InstanceID]=$_ }
$af=@{}
Get-NetFirewallApplicationFilter -All | ForEach-Object { $af[$_.InstanceID]=$_.Program }
Get-NetFirewallRule -Enabled True | ForEach-Object { $p=$pf[$_.InstanceID]; "$($_.DisplayName)`t$($_.Direction)`t$($_.Action)`t$($p.Protocol)`t$($p.LocalPort)`t$($af[$_.InstanceID])`t$($_.Profile)" }"#;

// ---------------------------------------------------------------------------
// Parsers

fn kv(body: &str) -> std::collections::HashMap<String, String> {
    body.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_string(),
                v.trim().trim_start_matches('\u{feff}').to_string(),
            )
        })
        .collect()
}

fn section_of<'a>(all: &'a [(String, String)], name: &str) -> &'a str {
    all.iter()
        .find(|(h, _)| h == name)
        .map(|(_, b)| b.as_str())
        .unwrap_or("")
}

fn fields(line: &str) -> Vec<&str> {
    line.trim_end_matches(['\r', '\n']).split('\t').collect()
}

pub fn parse_disks(text: &str) -> Vec<DiskInfo> {
    let mut out = Vec::new();
    for l in text.lines() {
        let f = fields(l);
        if f.len() < 5 || f[0].trim().is_empty() {
            continue;
        }
        let total: u64 = f[3].trim().parse().unwrap_or(0);
        let free: u64 = f[4].trim().parse().unwrap_or(0);
        if total == 0 {
            continue;
        }
        let used = total.saturating_sub(free);
        out.push(DiskInfo {
            mount: clip(f[0], 20),
            label: clip(f[1], 60),
            fs: clip(f[2], 20),
            total_mb: total / 1_048_576,
            used_mb: used / 1_048_576,
            used_pct: (used * 100 / total).min(100) as u8,
        });
    }
    out.truncate(40);
    out
}

pub fn parse_addresses(text: &str) -> Vec<NetAddr> {
    text.lines()
        .filter_map(|l| {
            let f = fields(l);
            (f.len() >= 2 && !f[1].trim().is_empty()).then(|| NetAddr {
                iface: clip(f[0], 40),
                addr: clip(f[1], 60),
            })
        })
        .take(40)
        .collect()
}

/// Facts and the Windows part of the detail from the output of [`FACTS_SCRIPT`].
pub fn parse_facts(text: &str, facts: &mut HostFacts, d: &mut HostDetail) -> bool {
    let s = sections(text);
    let os = kv(section_of(&s, "os"));
    let Some(caption) = os.get("caption").filter(|c| !c.is_empty()) else {
        return false;
    };
    let get = |k: &str| os.get(k).cloned().unwrap_or_default();
    let ram_kb: u64 = get("ramkb").parse().unwrap_or(0);
    let free_kb: u64 = get("freekb").parse().unwrap_or(0);

    facts.os_id = "windows".into();
    facts.os_name = clip(caption.trim_start_matches("Microsoft ").trim(), 80);
    facts.hostname = clip(&get("host"), 80);
    facts.uptime_secs = get("uptime").parse().unwrap_or(0);
    facts.kernel = clip(
        &format!("{} ({})", get("version"), get("build")).replace(" ()", ""),
        80,
    );
    facts.cpus = get("logical").parse().ok().filter(|n| *n > 0);
    facts.mem_total_mb = (ram_kb > 0).then_some(ram_kb / 1024);
    facts.mem_used_pct =
        (ram_kb > 0).then(|| (ram_kb.saturating_sub(free_kb) * 100 / ram_kb).min(100) as u8);

    d.platform = "windows".into();
    d.os_caption = facts.os_name.clone();
    d.os_version = clip(&get("version"), 40);
    d.arch = clip(&get("arch"), 20);
    d.hostname = facts.hostname.clone();
    d.domain = if get("partofdomain").eq_ignore_ascii_case("true") {
        clip(&get("domain"), 80)
    } else {
        String::new()
    };
    d.model = clip(&get("model"), 80);
    d.cpu_model = clip(&get("cpu"), 120);
    d.sockets = get("sockets").parse().ok().filter(|n| *n > 0);
    d.disks = parse_disks(section_of(&s, "disks"));
    d.addresses = parse_addresses(section_of(&s, "addr"));
    let user = get("user");
    d.users = if user.is_empty() {
        Vec::new()
    } else {
        vec![clip(&user, 80)]
    };
    facts.disk_used_pct = d
        .disks
        .iter()
        .find(|x| x.mount.eq_ignore_ascii_case("C:"))
        .or(d.disks.first())
        .map(|x| x.used_pct);
    true
}

/// Pending updates `(all, security)`.
pub fn parse_updates(text: &str) -> Option<(u32, u32)> {
    let m = kv(text);
    Some((
        m.get("updates")?.parse().ok()?,
        m.get("security")?.parse().unwrap_or(0),
    ))
}

/// Output of [`SERVICES_SCRIPT`]: name, display name, status, start type, exit code.
pub fn parse_services(text: &str) -> (Vec<ServiceInfo>, usize, Avail) {
    let mut list = Vec::new();
    for l in text.lines() {
        let f = fields(l);
        if f.len() < 4 || f[0].trim().is_empty() {
            continue;
        }
        let status = f[2].trim();
        let start = f[3].trim();
        let exit: i64 = f.get(4).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        let running = status.eq_ignore_ascii_case("Running");
        // A stopped automatic service that ended with an error code.
        let failed = !running
            && start.eq_ignore_ascii_case("Automatic")
            && exit != 0
            && exit != 1077
            && exit != 1056;
        list.push(ServiceInfo {
            name: clip(f[0], 120),
            description: clip(f[1], 120),
            load: String::new(),
            active: if running {
                "active"
            } else if failed {
                "failed"
            } else {
                "inactive"
            }
            .into(),
            sub: status.to_lowercase(),
            start: start.to_lowercase(),
            running,
            failed,
        });
    }
    if list.is_empty() {
        return (list, 0, Avail::Failed);
    }
    let total = detail::limit_services(&mut list);
    (list, total, Avail::Ok)
}

/// Output of [`PORTS_SCRIPT`]: protocol, address, port, process.
pub fn parse_ports(text: &str) -> (Vec<PortInfo>, Avail) {
    let mut list = Vec::new();
    for l in text.lines() {
        let f = fields(l);
        if f.len() < 3 {
            continue;
        }
        let Ok(port) = f[2].trim().parse::<u16>() else {
            continue;
        };
        if f[0] != "tcp" && f[0] != "udp" {
            continue;
        }
        list.push(PortInfo {
            proto: f[0].into(),
            addr: clip(f[1], 60),
            port,
            process: clip(f.get(3).copied().unwrap_or(""), 60),
        });
    }
    if list.is_empty() {
        return (list, Avail::Failed);
    }
    detail::tidy_ports(&mut list);
    (list, Avail::Ok)
}

/// Output of [`FIREWALL_SCRIPT`].
pub fn parse_firewall(text: &str) -> Firewall {
    let s = sections(text);
    let profiles = section_of(&s, "profiles");
    let mut fw = Firewall {
        backend: "windows".into(),
        avail: Avail::Ok,
        ..Default::default()
    };
    let mut any_on = false;
    let mut any = false;
    for l in profiles.lines() {
        let f = fields(l);
        if f.len() < 4 || f[0].trim().is_empty() {
            continue;
        }
        any = true;
        let on = matches!(f[1].trim().to_lowercase().as_str(), "true" | "1");
        any_on |= on;
        fw.summary.push(clip(
            &format!(
                "{}: {} · {} {} / {} {}",
                f[0],
                if on { t!("aktiv") } else { t!("aus") },
                t!("eingehend"),
                f[2],
                t!("ausgehend"),
                f[3]
            ),
            160,
        ));
    }
    if !any {
        return Firewall {
            avail: Avail::Failed,
            ..Default::default()
        };
    }
    fw.active = Some(any_on);
    let mut rules = Vec::new();
    for l in section_of(&s, "rules").lines() {
        let f = fields(l);
        if f.len() < 5 || f[0].trim().is_empty() {
            continue;
        }
        let word = |x: &str| match x.trim() {
            "1" => "Inbound".to_string(),
            "2" => "Outbound".to_string(),
            o => o.to_string(),
        };
        let action = |x: &str| match x.trim() {
            "2" => "Allow".to_string(),
            "4" => "Block".to_string(),
            o => o.to_string(),
        };
        rules.push(FwRule {
            name: clip(f[0], 120),
            direction: match word(f[1]).as_str() {
                "Inbound" => "in".into(),
                "Outbound" => "out".into(),
                o => o.to_lowercase(),
            },
            action: action(f[2]).to_lowercase(),
            protocol: clip(f[3], 20),
            port: clip(f[4], 80),
            program: clip(f.get(5).copied().unwrap_or(""), 160),
            profile: clip(f.get(6).copied().unwrap_or(""), 60),
            ..Default::default()
        });
    }
    // Blocks first, then inbound rules, then by name; cut at the model's limit.
    rules.sort_by(|a, b| {
        (
            a.action != "block",
            a.direction != "in",
            a.name.to_lowercase(),
        )
            .cmp(&(
                b.action != "block",
                b.direction != "in",
                b.name.to_lowercase(),
            ))
    });
    fw.total_rules = rules.len();
    rules.truncate(detail::MAX_RULES);
    fw.rules = rules;
    fw
}

// ---------------------------------------------------------------------------
// Collecting

fn run(
    sess: &mut Session,
    script: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String, RunError> {
    let out = sess.run_script(script, timeout, CAP, cancel)?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_start_matches('\u{feff}')
        .to_string())
}

/// Collects everything over `sess`; `with_updates` also asks the Windows Update agent
/// (slow). `None` if not even the basic facts came back.
pub fn collect(
    sess: &mut Session,
    snippets: &[InfoSnippet],
    with_updates: bool,
    cancel: &AtomicBool,
) -> Result<Collected, RunError> {
    let mut facts = HostFacts::default();
    let mut d = HostDetail::default();
    let t = Duration::from_secs(90);
    let text = run(sess, FACTS_SCRIPT, t, cancel)?;
    if !parse_facts(&text, &mut facts, &mut d) {
        return Err(RunError::Other(anyhow::anyhow!(
            "{}",
            t!("Der Server hat keine Windows-Fakten geliefert")
        )));
    }
    facts.collected = Some(chrono::Utc::now());
    d.collected = facts.collected;

    match run(sess, SERVICES_SCRIPT, t, cancel) {
        Ok(text) => (d.services, d.services_total, d.services_avail) = parse_services(&text),
        Err(RunError::Cancelled) => return Err(RunError::Cancelled),
        Err(_) => d.services_avail = Avail::Failed,
    }
    match run(sess, PORTS_SCRIPT, t, cancel) {
        Ok(text) => (d.ports, d.ports_avail) = parse_ports(&text),
        Err(RunError::Cancelled) => return Err(RunError::Cancelled),
        Err(_) => d.ports_avail = Avail::Failed,
    }
    d.firewall = match run(sess, FIREWALL_SCRIPT, t, cancel) {
        Ok(text) => parse_firewall(&text),
        Err(RunError::Cancelled) => return Err(RunError::Cancelled),
        Err(_) => Firewall {
            avail: Avail::Failed,
            ..Default::default()
        },
    };
    if with_updates
        && let Ok(text) = run(sess, UPDATES_SCRIPT, Duration::from_secs(150), cancel)
        && let Some((all, sec)) = parse_updates(&text)
    {
        facts.updates = Some(all);
        facts.security_updates = Some(sec);
    }
    d.info = run_snippets(sess, snippets, cancel);
    Ok(Collected { facts, detail: d })
}

/// The user's info snippets, run as PowerShell on this host.
fn run_snippets(
    sess: &mut Session,
    snippets: &[InfoSnippet],
    cancel: &AtomicBool,
) -> Vec<InfoResult> {
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
        match sess.run_script(
            s.command.trim(),
            Duration::from_secs(60),
            info::MAX_OUTPUT * 2,
            cancel,
        ) {
            Ok(o) => {
                let mut raw = o.stdout;
                if !o.stderr.is_empty() {
                    raw.extend_from_slice(b"\n");
                    raw.extend_from_slice(o.stderr.as_bytes());
                }
                (r.output, r.truncated) = info::clean_output(&raw);
                r.exit = Some(o.exit.max(0) as u32);
            }
            Err(e) => r.error = e.to_string(),
        }
        out.push(r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACTS: &str = "@@os\ncaption=Microsoft Windows Server 2022 Standard\nversion=10.0.20348\nbuild=20348\narch=64-bit\nuptime=93784\nhost=SRV-DC01\ndomain=corp.example.com\npartofdomain=True\nmodel=VMware, Inc. VMware7,1\nramkb=16776692\nfreekb=8388346\ncpu=Intel(R) Xeon(R) Gold 6248 CPU @ 2.50GHz\nsockets=2\nlogical=8\nuser=CORP\\alice\n@@disks\nC:\tSystem\tNTFS\t107374182400\t32212254720\nD:\t\tNTFS\t0\t0\nE:\tData\tReFS\t1099511627776\t549755813888\n@@addr\nEthernet\t10.0.0.5/24\nEthernet\t2001:db8::5/64\nLoopback\t\n";

    #[test]
    fn facts() {
        let mut f = HostFacts::default();
        let mut d = HostDetail::default();
        assert!(parse_facts(FACTS, &mut f, &mut d));
        assert_eq!(f.os_id, "windows");
        assert_eq!(f.os_name, "Windows Server 2022 Standard");
        assert_eq!(f.hostname, "SRV-DC01");
        assert_eq!(f.uptime_secs, 93784);
        assert_eq!(f.cpus, Some(8));
        assert_eq!(f.mem_total_mb, Some(16383));
        assert_eq!(f.mem_used_pct, Some(50));
        assert_eq!(f.disk_used_pct, Some(70));
        assert_eq!(f.kernel, "10.0.20348 (20348)");
        assert_eq!(d.platform, "windows");
        assert_eq!(d.domain, "corp.example.com");
        assert_eq!(d.sockets, Some(2));
        assert_eq!(d.disks.len(), 2, "the empty D: drive is skipped");
        assert_eq!(d.disks[0].mount, "C:");
        assert_eq!(d.disks[0].total_mb, 102400);
        assert_eq!(d.disks[0].used_mb, 71680);
        assert_eq!(d.disks[1].used_pct, 50);
        assert_eq!(d.addresses.len(), 2);
        assert_eq!(d.addresses[0].addr, "10.0.0.5/24");
        assert_eq!(d.users, vec!["CORP\\alice".to_string()]);
        assert!(!parse_facts("garbage", &mut f, &mut d));
        assert!(!parse_facts("@@os\ncaption=\n", &mut f, &mut d));
    }

    #[test]
    fn workgroup_machines_have_no_domain() {
        let text = FACTS.replace("partofdomain=True", "partofdomain=False");
        let (mut f, mut d) = (HostFacts::default(), HostDetail::default());
        assert!(parse_facts(&text, &mut f, &mut d));
        assert!(d.domain.is_empty());
    }

    #[test]
    fn updates() {
        assert_eq!(parse_updates("updates=7\nsecurity=2\n"), Some((7, 2)));
        assert_eq!(parse_updates("updates=0\nsecurity=0"), Some((0, 0)));
        assert_eq!(parse_updates("nothing"), None);
    }

    #[test]
    fn services() {
        let text = "Spooler\tPrint Spooler\tRunning\tAutomatic\t0\nWinDefend\tMicrosoft Defender\tStopped\tAutomatic\t1\nMyApp\tMy App\tStopped\tAutomatic\t1053\nBITS\tBackground Intelligent Transfer\tStopped\tManual\t0\nsppsvc\tSoftware Protection\tStopped\tAutomatic\t0\n\t\t\t\t\n";
        let (list, total, avail) = parse_services(text);
        assert_eq!((total, avail), (5, Avail::Ok));
        // Failed first (error exit code on an automatic service), then running.
        assert_eq!(list[0].name, "MyApp");
        assert!(list[0].failed && list[0].active == "failed");
        assert!(list.iter().find(|s| s.name == "WinDefend").unwrap().failed);
        let sp = list.iter().find(|s| s.name == "Spooler").unwrap();
        assert!(sp.running && !sp.failed);
        assert_eq!(sp.start, "automatic");
        let bits = list.iter().find(|s| s.name == "BITS").unwrap();
        assert!(!bits.running && !bits.failed);
        assert!(!list.iter().find(|s| s.name == "sppsvc").unwrap().failed);
        assert_eq!(parse_services("").2, Avail::Failed);
    }

    #[test]
    fn ports() {
        let text = "tcp\t0.0.0.0\t135\tsvchost\ntcp\t::\t445\tSystem\nudp\t0.0.0.0\t123\tsvchost\ntcp\t0.0.0.0\t135\tsvchost\nbad\tline\ntcp\t0.0.0.0\tx\t\n";
        let (list, avail) = parse_ports(text);
        assert_eq!(avail, Avail::Ok);
        assert_eq!(list.len(), 3, "duplicates and bad lines are dropped");
        assert_eq!(list[0].port, 123);
        assert_eq!(list[0].proto, "udp");
        assert_eq!(list[1].process, "svchost");
        assert_eq!(parse_ports("").1, Avail::Failed);
    }

    #[test]
    fn firewall() {
        let text = "@@profiles\nDomain\tTrue\tBlock\tAllow\nPrivate\tTrue\tBlock\tAllow\nPublic\tFalse\tBlock\tAllow\n@@rules\nRemote Desktop (TCP-In)\tInbound\tAllow\tTCP\t3389\t\tAny\nBlock telnet\tInbound\tBlock\tTCP\t23\t\tAny\nCore Networking - DNS (UDP-Out)\tOutbound\tAllow\tUDP\tAny\tC:\\Windows\\system32\\svchost.exe\tAny\nWinRM\t1\t2\t6\t5985\t\tPrivate, Domain\n";
        let fw = parse_firewall(text);
        assert_eq!(fw.avail, Avail::Ok);
        assert_eq!(fw.backend, "windows");
        assert_eq!(fw.active, Some(true));
        assert_eq!(fw.summary.len(), 3);
        assert_eq!(fw.total_rules, 4);
        // Block first, then inbound.
        assert_eq!(fw.rules[0].name, "Block telnet");
        assert_eq!(fw.rules[0].action, "block");
        assert_eq!(fw.rules[0].direction, "in");
        let rdp = fw.rules.iter().find(|r| r.port == "3389").unwrap();
        assert_eq!(
            (rdp.action.as_str(), rdp.protocol.as_str()),
            ("allow", "TCP")
        );
        // Numeric enum values (older PowerShell) are understood.
        let winrm = fw.rules.iter().find(|r| r.name == "WinRM").unwrap();
        assert_eq!(
            (winrm.direction.as_str(), winrm.action.as_str()),
            ("in", "allow")
        );
        let dns = fw.rules.iter().find(|r| r.name.contains("DNS")).unwrap();
        assert_eq!(dns.direction, "out");
        assert!(dns.program.ends_with("svchost.exe"));
        assert_eq!(parse_firewall("").avail, Avail::Failed);
        assert_eq!(
            parse_firewall("@@profiles\nPublic\tFalse\tBlock\tAllow\n").active,
            Some(false)
        );
    }

    #[test]
    fn scripts_fit_a_command_line() {
        for s in [
            FACTS_SCRIPT,
            UPDATES_SCRIPT,
            SERVICES_SCRIPT,
            PORTS_SCRIPT,
            FIREWALL_SCRIPT,
        ] {
            // Small enough to fit a command line once encoded.
            assert!(s.len() < 3500, "{}", s.len());
        }
    }
}
