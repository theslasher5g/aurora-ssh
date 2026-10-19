//! Per-host detail data of the server inventory: hardware, disks, network, services,
//! listening ports and firewall rules, plus the parsers for the output of the Linux
//! commands and the Windows PowerShell scripts. Pure functions, no I/O; every list is
//! bounded because it is stored in `library.json`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_SERVICES: usize = 500;
pub const MAX_RULES: usize = 200;
pub const MAX_PORTS: usize = 500;
const MAX_DISKS: usize = 40;
const MAX_ADDRS: usize = 40;
const MAX_USERS: usize = 30;

/// Whether a part of the detail data could be collected.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Avail {
    /// Never tried (old library files).
    #[default]
    Unknown,
    Ok,
    /// The command exists but needs root (and `sudo -n` did not work).
    NeedsRoot,
    /// No tool for it on the host (no systemd, no firewall program …).
    Missing,
    /// Timeout, broken output …
    Failed,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DiskInfo {
    /// `/var` or `C:`
    pub mount: String,
    pub label: String,
    pub fs: String,
    pub total_mb: u64,
    pub used_mb: u64,
    pub used_pct: u8,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct NetAddr {
    pub iface: String,
    /// `10.0.0.5/24`
    pub addr: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ServiceInfo {
    pub name: String,
    pub description: String,
    /// systemd: `loaded` / `not-found` …; Windows: empty.
    pub load: String,
    /// `active`, `inactive`, `failed` …
    pub active: String,
    /// `running`, `exited`, `dead` …
    pub sub: String,
    /// Linux: `enabled` / `disabled` / `static` / `masked`; Windows: `automatic` …
    pub start: String,
    pub running: bool,
    pub failed: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct PortInfo {
    /// `tcp` or `udp`
    pub proto: String,
    pub addr: String,
    pub port: u16,
    pub process: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct FwRule {
    pub name: String,
    /// `in`, `out`, `fwd` (or the Windows words)
    pub direction: String,
    pub action: String,
    pub protocol: String,
    pub port: String,
    pub program: String,
    pub profile: String,
    /// Remote side (`Anywhere`, a network …).
    pub source: String,
    /// Unstructured line (nft, iptables, firewalld rich rules).
    pub raw: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Firewall {
    pub avail: Avail,
    /// `ufw`, `firewalld`, `nft`, `iptables`, `windows`
    pub backend: String,
    pub active: Option<bool>,
    /// Default policies, profiles, zone … one line each.
    pub summary: Vec<String>,
    pub rules: Vec<FwRule>,
    /// Number of rules before the list was cut at [`MAX_RULES`].
    pub total_rules: usize,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HostDetail {
    /// `linux` or `other`
    pub platform: String,
    pub os_caption: String,
    pub os_version: String,
    pub arch: String,
    pub hostname: String,
    /// FQDN (Linux) or domain / workgroup (Windows).
    pub domain: String,
    /// Hardware model or virtualisation.
    pub model: String,
    pub cpu_model: String,
    pub sockets: Option<u32>,
    pub disks: Vec<DiskInfo>,
    pub addresses: Vec<NetAddr>,
    pub users: Vec<String>,
    pub services: Vec<ServiceInfo>,
    pub services_avail: Avail,
    pub services_total: usize,
    pub ports: Vec<PortInfo>,
    pub ports_avail: Avail,
    pub firewall: Firewall,
    pub collected: Option<DateTime<Utc>>,
    /// Output of the user's info snippets.
    pub info: Vec<super::inventory_info::InfoResult>,
}

impl HostDetail {
    pub fn failed_count(&self) -> u32 {
        self.services.iter().filter(|s| s.failed).count() as u32
    }
    pub fn running_count(&self) -> usize {
        self.services.iter().filter(|s| s.running).count()
    }
}

/// Printable text of at most `n` characters (control characters dropped).
pub fn clip(s: &str, n: usize) -> String {
    s.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(n)
        .collect()
}

// ---------------------------------------------------------------------------
// Linux

/// Splits `@@name rest` marked output into `(header line without @@, body)`.
pub fn sections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("@@") {
            out.push((h.trim().to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

fn section<'a>(all: &'a [(String, String)], name: &str) -> &'a str {
    all.iter()
        .find(|(h, _)| h.split_whitespace().next() == Some(name))
        .map(|(_, b)| b.as_str())
        .unwrap_or("")
}

/// `systemctl list-units` (+ `list-unit-files` for the enabled state).
pub fn parse_systemctl(units: &str, files: &str) -> Vec<ServiceInfo> {
    let mut enabled: HashMap<String, String> = HashMap::new();
    for line in files.lines() {
        let mut it = line.split_whitespace();
        if let (Some(n), Some(s)) = (it.next(), it.next())
            && n.ends_with(".service")
        {
            enabled.insert(n.to_string(), s.to_string());
        }
    }
    let mut out = Vec::new();
    for line in units.lines() {
        let line = line.trim().trim_start_matches(['●', '*', '○', '×']).trim();
        let mut it = line.split_whitespace();
        let (Some(unit), Some(load), Some(active), Some(sub)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        if !unit.ends_with(".service")
            || !matches!(
                load,
                "loaded" | "not-found" | "masked" | "error" | "bad-setting" | "stub" | "merged"
            )
        {
            continue;
        }
        let desc: Vec<&str> = it.collect();
        // `getty@tty1.service` takes its state from the template `getty@.service`.
        let template = unit.split_once('@').map(|(a, _)| format!("{a}@.service"));
        let start = enabled
            .get(unit)
            .or_else(|| template.as_ref().and_then(|t| enabled.get(t)))
            .cloned()
            .unwrap_or_default();
        out.push(ServiceInfo {
            name: clip(unit.trim_end_matches(".service"), 120),
            description: clip(&desc.join(" "), 120),
            load: load.to_string(),
            active: active.to_string(),
            sub: sub.to_string(),
            start: clip(&start, 30),
            running: active == "active" && sub == "running",
            failed: active == "failed",
        });
    }
    out
}

/// Failed first, then running, then by name; cut at [`MAX_SERVICES`]. Returns the
/// number before cutting.
pub fn limit_services(list: &mut Vec<ServiceInfo>) -> usize {
    let total = list.len();
    list.sort_by(|a, b| {
        b.failed
            .cmp(&a.failed)
            .then(b.running.cmp(&a.running))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list.truncate(MAX_SERVICES);
    total
}

/// `0.0.0.0:22`, `[::]:22`, `127.0.0.53%lo:53`, `*:80` → (address, port).
pub fn split_hostport(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.rsplit_once(':')?;
    let port = p.parse::<u16>().ok()?;
    let h = h.trim_matches(['[', ']']);
    let h = h.split('%').next().unwrap_or(h);
    Some((clip(h, 60), port))
}

fn process_from_users(s: &str) -> String {
    // users:(("sshd",pid=812,fd=3),("sshd",pid=900,fd=3))
    s.split("((\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .map(|n| clip(n, 60))
        .unwrap_or_default()
}

/// `ss -tulpn`
pub fn parse_ss(text: &str) -> Vec<PortInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 6 || !(t[0].starts_with("tcp") || t[0].starts_with("udp")) {
            continue;
        }
        if t[0].starts_with("tcp") && t[1] != "LISTEN" {
            continue;
        }
        let Some((addr, port)) = split_hostport(t[4]) else {
            continue;
        };
        out.push(PortInfo {
            proto: t[0][..3].to_string(),
            addr,
            port,
            process: t
                .get(6..)
                .map(|r| process_from_users(&r.join(" ")))
                .unwrap_or_default(),
        });
    }
    out
}

/// `netstat -tulpn`
pub fn parse_netstat(text: &str) -> Vec<PortInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 5 || !(t[0].starts_with("tcp") || t[0].starts_with("udp")) {
            continue;
        }
        if t[0].starts_with("tcp") && t.get(5) != Some(&"LISTEN") {
            continue;
        }
        let Some((addr, port)) = split_hostport(t[3]) else {
            continue;
        };
        let process = t
            .last()
            .and_then(|l| l.split_once('/'))
            .map(|(_, n)| clip(n, 60))
            .unwrap_or_default();
        out.push(PortInfo {
            proto: t[0][..3].to_string(),
            addr,
            port,
            process,
        });
    }
    out
}

/// Sorted by port, duplicates removed, cut at [`MAX_PORTS`].
pub fn tidy_ports(list: &mut Vec<PortInfo>) {
    list.sort_by(|a, b| {
        (a.port, &a.proto, &a.addr, &a.process).cmp(&(b.port, &b.proto, &b.addr, &b.process))
    });
    list.dedup();
    list.truncate(MAX_PORTS);
}

/// `df -PkT`
pub fn parse_df(text: &str) -> Vec<DiskInfo> {
    const SKIP_FS: &[&str] = &[
        "tmpfs",
        "devtmpfs",
        "squashfs",
        "overlay",
        "efivarfs",
        "devpts",
        "proc",
        "sysfs",
        "cgroup",
        "cgroup2",
        "ramfs",
        "fuse.lxcfs",
        "iso9660",
        "autofs",
        "tracefs",
        "debugfs",
    ];
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 7 {
            continue;
        }
        let (Ok(total), Ok(used)) = (t[2].parse::<u64>(), t[3].parse::<u64>()) else {
            continue;
        };
        let mount = t[6..].join(" ");
        if SKIP_FS.contains(&t[1])
            || total == 0
            || ["/snap", "/run", "/sys", "/proc", "/dev"]
                .iter()
                .any(|p| mount == *p || mount.starts_with(&format!("{p}/")))
            || mount.starts_with("/var/lib/docker")
        {
            continue;
        }
        let pct = t[5]
            .trim_end_matches('%')
            .parse::<u8>()
            .unwrap_or((used * 100 / total).min(100) as u8);
        if out.iter().any(|d: &DiskInfo| d.label == t[0]) {
            continue; // bind mounts of the same device
        }
        out.push(DiskInfo {
            mount: clip(&mount, 120),
            label: clip(t[0], 80),
            fs: clip(t[1], 20),
            total_mb: total / 1024,
            used_mb: used / 1024,
            used_pct: pct.min(100),
        });
        if out.len() >= MAX_DISKS {
            break;
        }
    }
    out
}

/// `ip -o addr show scope global`
pub fn parse_ip_addr(text: &str) -> Vec<NetAddr> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = t.iter().position(|w| *w == "inet" || *w == "inet6") else {
            continue;
        };
        let (Some(addr), Some(iface)) = (t.get(i + 1), t.get(1)) else {
            continue;
        };
        if addr.starts_with("fe80:") || addr.starts_with("127.") {
            continue;
        }
        out.push(NetAddr {
            iface: clip(iface.trim_end_matches(':'), 30),
            addr: clip(addr, 60),
        });
        if out.len() >= MAX_ADDRS {
            break;
        }
    }
    out
}

/// `who`: the distinct user names in the first column.
pub fn parse_who(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(u) = line.split_whitespace().next() {
            let u = clip(u, 40);
            if !out.contains(&u) {
                out.push(u);
            }
        }
        if out.len() >= MAX_USERS {
            break;
        }
    }
    out
}

/// The extra Linux facts script (`@@cpu` … sections).
pub fn parse_linux_extras(text: &str, d: &mut HostDetail) {
    let s = sections(text);
    let first = |name: &str| clip(section(&s, name).lines().next().unwrap_or(""), 120);
    d.cpu_model = first("cpu");
    d.arch = first("arch");
    d.os_version = first("osver");
    let fqdn = first("fqdn");
    if fqdn.contains('.') {
        d.domain = fqdn;
    }
    let virt = first("virt");
    let model = first("model");
    d.model = match (model.is_empty(), virt.is_empty() || virt == "none") {
        (false, false) => format!("{model} ({virt})"),
        (false, true) => model,
        (true, false) => virt,
        _ => String::new(),
    };
    d.disks = parse_df(section(&s, "df"));
    d.addresses = parse_ip_addr(section(&s, "ip"));
    if d.addresses.is_empty() {
        d.addresses = section(&s, "hostname")
            .split_whitespace()
            .filter(|a| !a.starts_with("127.") && !a.starts_with("fe80:"))
            .take(MAX_ADDRS)
            .map(|a| NetAddr {
                iface: String::new(),
                addr: clip(a, 60),
            })
            .collect();
    }
    d.users = parse_who(section(&s, "users"));
}

/// Output of the services script: `@@units` / `@@files` (or `@@nosystemd`).
pub fn parse_linux_services(text: &str) -> (Vec<ServiceInfo>, usize, Avail) {
    let s = sections(text);
    if s.iter().any(|(h, _)| h == "nosystemd") {
        return (Vec::new(), 0, Avail::Missing);
    }
    let mut list = parse_systemctl(section(&s, "units"), section(&s, "files"));
    if list.is_empty() {
        return (list, 0, Avail::Failed);
    }
    let total = limit_services(&mut list);
    (list, total, Avail::Ok)
}

/// Output of the ports script: `@@ss` or `@@netstat` (or `@@none`).
pub fn parse_linux_ports(text: &str) -> (Vec<PortInfo>, Avail) {
    let s = sections(text);
    let mut list = if s.iter().any(|(h, _)| h == "ss") {
        parse_ss(section(&s, "ss"))
    } else if s.iter().any(|(h, _)| h == "netstat") {
        parse_netstat(section(&s, "netstat"))
    } else {
        return (Vec::new(), Avail::Missing);
    };
    tidy_ports(&mut list);
    (list, Avail::Ok)
}

fn denied(text: &str) -> bool {
    let l = text.to_lowercase();
    [
        "root",
        "permission",
        "not permitted",
        "sudo",
        "password",
        "denied",
        "superuser",
    ]
    .iter()
    .any(|w| l.contains(w))
}

fn rule_raw(line: &str) -> FwRule {
    FwRule {
        raw: line
            .trim_end()
            .replace('\t', "  ")
            .chars()
            .filter(|c| !c.is_control())
            .take(300)
            .collect(),
        ..Default::default()
    }
}

/// `ufw status verbose`
pub fn parse_ufw(out: &str) -> Firewall {
    let mut fw = Firewall {
        backend: "ufw".into(),
        avail: Avail::Ok,
        ..Default::default()
    };
    let mut in_rules = false;
    for line in out.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if let Some(st) = l.strip_prefix("Status:") {
            fw.active = Some(st.trim() == "active");
        } else if l.starts_with("--") {
            in_rules = true;
        } else if in_rules {
            // "22/tcp                     ALLOW IN    Anywhere"
            let cols: Vec<&str> = l
                .split("  ")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect();
            if cols.len() >= 3 {
                let (mut action, mut dir) = (cols[1].to_string(), String::new());
                if let Some((a, d)) = cols[1].split_once(' ') {
                    action = a.to_string();
                    dir = d.to_lowercase();
                }
                let (port, proto) = match cols[0].split_once('/') {
                    Some((p, pr)) => (p.to_string(), pr.to_string()),
                    None => (cols[0].to_string(), String::new()),
                };
                fw.rules.push(FwRule {
                    direction: dir,
                    action: clip(&action, 20),
                    protocol: clip(&proto, 20),
                    port: clip(&port, 80),
                    source: clip(&cols[2..].join(" "), 80),
                    ..Default::default()
                });
            } else {
                fw.rules.push(rule_raw(l));
            }
        } else if l.starts_with("Logging:")
            || l.starts_with("Default:")
            || l.starts_with("New profiles:")
        {
            fw.summary.push(clip(l, 200));
        }
    }
    fw
}

/// `firewall-cmd --list-all`
pub fn parse_firewalld(out: &str) -> Firewall {
    let mut fw = Firewall {
        backend: "firewalld".into(),
        avail: Avail::Ok,
        active: Some(true),
        ..Default::default()
    };
    let mut in_rich = false;
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            // "public (active)"
            fw.summary.push(clip(&format!("zone: {line}"), 200));
            continue;
        }
        let l = line.trim();
        if in_rich && !l.contains(": ") && !l.ends_with(':') {
            fw.rules.push(rule_raw(l));
            continue;
        }
        in_rich = false;
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k {
            "services" => {
                for s in v.split_whitespace() {
                    fw.rules.push(FwRule {
                        name: clip(s, 60),
                        direction: "in".into(),
                        action: "ALLOW".into(),
                        ..Default::default()
                    });
                }
            }
            "ports" => {
                for p in v.split_whitespace() {
                    let (port, proto) = p.split_once('/').unwrap_or((p, ""));
                    fw.rules.push(FwRule {
                        direction: "in".into(),
                        action: "ALLOW".into(),
                        port: clip(port, 40),
                        protocol: clip(proto, 10),
                        ..Default::default()
                    });
                }
            }
            "rich rules" => {
                in_rich = true;
                if !v.is_empty() {
                    fw.rules.push(rule_raw(v));
                }
            }
            "target" | "interfaces" | "sources" | "masquerade" | "forward-ports"
            | "icmp-blocks"
                if !v.is_empty() =>
            {
                fw.summary.push(clip(&format!("{k}: {v}"), 200));
            }
            _ => {}
        }
    }
    fw
}

/// `nft list ruleset`: every line is kept as it is.
pub fn parse_nft(out: &str) -> Firewall {
    let rules: Vec<FwRule> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(rule_raw)
        .collect();
    Firewall {
        backend: "nft".into(),
        avail: Avail::Ok,
        active: Some(!rules.is_empty()),
        rules,
        ..Default::default()
    }
}

/// `iptables -S`
pub fn parse_iptables(out: &str) -> Firewall {
    let mut fw = Firewall {
        backend: "iptables".into(),
        avail: Avail::Ok,
        ..Default::default()
    };
    for line in out.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.as_slice() {
            ["-P", chain, policy] => fw.summary.push(format!("{chain}: {policy}")),
            ["-A", chain, rest @ ..] => {
                let opt = |flag: &str| {
                    rest.iter()
                        .position(|w| *w == flag)
                        .and_then(|i| rest.get(i + 1))
                        .map(|v| clip(v, 60))
                        .unwrap_or_default()
                };
                let direction = match *chain {
                    "INPUT" => "in",
                    "OUTPUT" => "out",
                    "FORWARD" => "fwd",
                    _ => "",
                };
                fw.rules.push(FwRule {
                    direction: direction.into(),
                    action: opt("-j"),
                    protocol: opt("-p"),
                    port: opt("--dport"),
                    source: opt("-s"),
                    raw: clip(line, 300),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    fw.active = Some(!fw.rules.is_empty() || fw.summary.iter().any(|s| !s.ends_with("ACCEPT")));
    fw
}

/// The firewall script prints `@@fw <tool> <exit code>` followed by the tool's output.
pub fn parse_linux_firewall(text: &str) -> Firewall {
    let s = sections(text);
    let tools: Vec<(&str, i32, &str)> = s
        .iter()
        .filter_map(|(h, b)| {
            let mut it = h.split_whitespace();
            (it.next() == Some("fw")).then_some(())?;
            Some((it.next()?, it.next()?.parse().unwrap_or(1), b.as_str()))
        })
        .collect();
    if tools.is_empty() {
        return Firewall {
            avail: Avail::Missing,
            ..Default::default()
        };
    }
    let mut inactive: Option<Firewall> = None;
    let mut saw_denied = false;
    for (tool, rc, body) in &tools {
        if *rc != 0 {
            saw_denied |= denied(body) || *rc == 127;
            continue;
        }
        let mut fw = match *tool {
            "ufw" => parse_ufw(body),
            "firewalld" => parse_firewalld(body),
            "nft" => parse_nft(body),
            _ => parse_iptables(body),
        };
        fw.total_rules = fw.rules.len();
        fw.rules.truncate(MAX_RULES);
        // An inactive front end: look at the next tool, keep this as fallback.
        if fw.active == Some(false) {
            inactive.get_or_insert(fw);
            continue;
        }
        return fw;
    }
    if let Some(fw) = inactive {
        return fw;
    }
    Firewall {
        avail: if saw_denied {
            Avail::NeedsRoot
        } else {
            Avail::Failed
        },
        backend: tools[0].0.to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNITS: &str = "\
accounts-daemon.service loaded active running Accounts Service
apparmor.service        loaded active exited  Load AppArmor profiles
cloud-init.service      loaded failed failed  Initial cloud-init job (metadata service crawler)
getty@tty1.service      loaded active running Getty on tty1
nginx.service           loaded active running A high performance web server and a reverse proxy server
foo.service             not-found inactive dead foo.service
ssh.service             loaded active running OpenBSD Secure Shell server
";
    const FILES: &str = "\
accounts-daemon.service enabled enabled
apparmor.service        enabled enabled
cloud-init.service      enabled enabled
getty@.service          enabled enabled
nginx.service           disabled enabled
ssh.service             enabled enabled
systemd-fsck@.service   static   -
";

    #[test]
    fn systemctl_units() {
        let l = parse_systemctl(UNITS, FILES);
        assert_eq!(l.len(), 7);
        let ci = l.iter().find(|s| s.name == "cloud-init").unwrap();
        assert!(ci.failed && !ci.running);
        assert_eq!(
            ci.description,
            "Initial cloud-init job (metadata service crawler)"
        );
        let g = l.iter().find(|s| s.name == "getty@tty1").unwrap();
        assert_eq!(g.start, "enabled"); // from the template
        let n = l.iter().find(|s| s.name == "nginx").unwrap();
        assert_eq!(n.start, "disabled");
        assert!(n.running);
        let a = l.iter().find(|s| s.name == "apparmor").unwrap();
        assert!(!a.running && !a.failed && a.sub == "exited");
        assert!(l.iter().find(|s| s.name == "foo").unwrap().start.is_empty());
        // With the bullet that old systemd versions print.
        let b = parse_systemctl("● x.service loaded failed failed X\n", "");
        assert_eq!(b[0].name, "x");
        assert!(b[0].failed);
        assert!(parse_systemctl("garbage\n\nUNIT LOAD\n", "").is_empty());
    }

    #[test]
    fn services_are_limited_and_failed_first() {
        let mut l: Vec<ServiceInfo> = (0..600)
            .map(|i| ServiceInfo {
                name: format!("svc{i:03}"),
                running: i % 2 == 0,
                failed: i == 599,
                ..Default::default()
            })
            .collect();
        let total = limit_services(&mut l);
        assert_eq!((total, l.len()), (600, MAX_SERVICES));
        assert_eq!(l[0].name, "svc599");
        assert!(l[1].running);
    }

    #[test]
    fn linux_services_script_output() {
        let text = format!("@@units\n{UNITS}@@files\n{FILES}");
        let (l, total, avail) = parse_linux_services(&text);
        assert_eq!((l.len(), total, avail), (7, 7, Avail::Ok));
        assert_eq!(l[0].name, "cloud-init");
        assert_eq!(parse_linux_services("@@nosystemd\n").2, Avail::Missing);
        assert_eq!(parse_linux_services("@@units\n@@files\n").2, Avail::Failed);
    }

    const SS: &str = "\
Netid State  Recv-Q Send-Q Local Address:Port  Peer Address:PortProcess
udp   UNCONN 0      0      127.0.0.53%lo:53         0.0.0.0:*    users:((\"systemd-resolve\",pid=612,fd=13))
udp   UNCONN 0      0            0.0.0.0:68         0.0.0.0:*    users:((\"dhclient\",pid=700,fd=9))
tcp   LISTEN 0      128          0.0.0.0:22         0.0.0.0:*    users:((\"sshd\",pid=812,fd=3))
tcp   LISTEN 0      511                *:80               *:*    users:((\"nginx\",pid=900,fd=6),(\"nginx\",pid=901,fd=6))
tcp   LISTEN 0      128             [::]:22            [::]:*    users:((\"sshd\",pid=812,fd=4))
tcp   ESTAB  0      0         10.0.0.5:22       10.0.0.9:50122  users:((\"sshd\",pid=1500,fd=4))
tcp   LISTEN 0      4096       127.0.0.1:5432       0.0.0.0:*
";

    #[test]
    fn ss_ports() {
        let mut p = parse_ss(SS);
        assert_eq!(p.len(), 6);
        tidy_ports(&mut p);
        assert_eq!(p[0].port, 22);
        assert_eq!(p[0].process, "sshd");
        assert!(p.iter().any(|x| x.addr == "::" && x.port == 22));
        let web = p.iter().find(|x| x.port == 80).unwrap();
        assert_eq!((web.addr.as_str(), web.process.as_str()), ("*", "nginx"));
        let dns = p.iter().find(|x| x.port == 53).unwrap();
        assert_eq!(
            (dns.proto.as_str(), dns.addr.as_str()),
            ("udp", "127.0.0.53")
        );
        // Without root there is no process column.
        assert_eq!(p.iter().find(|x| x.port == 5432).unwrap().process, "");
        assert!(!p.iter().any(|x| x.port == 50122));
    }

    #[test]
    fn netstat_ports() {
        let n = "\
Active Internet connections (only servers)
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name
tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      812/sshd
tcp        0      0 10.0.0.5:22             10.0.0.9:50122          ESTABLISHED 1500/sshd
tcp6       0      0 :::443                  :::*                    LISTEN      -
udp        0      0 0.0.0.0:68              0.0.0.0:*                           700/dhclient
";
        let p = parse_netstat(n);
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].port, p[0].process.as_str()), (22, "sshd"));
        assert_eq!(
            (p[1].port, p[1].proto.as_str(), p[1].addr.as_str()),
            (443, "tcp", "::")
        );
        assert_eq!(p[2].proto, "udp");
        let (l, a) = parse_linux_ports(&format!("@@netstat\n{n}"));
        assert_eq!((l.len(), a), (3, Avail::Ok));
        assert_eq!(parse_linux_ports("@@none\n").1, Avail::Missing);
    }

    #[test]
    fn df_ip_who_extras() {
        let df = "\
Filesystem     Type     1024-blocks     Used Available Capacity Mounted on
udev           devtmpfs     1983000        0   1983000       0% /dev
tmpfs          tmpfs         401000     1200    399800       1% /run
/dev/sda1      ext4        41152812 29606000  9420000      76% /
/dev/sdb1      xfs        209612800 10480640 199132160       5% /data disk
/dev/loop0     squashfs       64000    64000         0     100% /snap/core/1
/dev/sda15     vfat          106858     6186    100672       6% /boot/efi
";
        let d = parse_df(df);
        assert_eq!(d.len(), 3);
        assert_eq!(
            (d[0].mount.as_str(), d[0].used_pct, d[0].total_mb),
            ("/", 76, 40188)
        );
        assert_eq!(d[1].mount, "/data disk");
        let ip = "\
1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever
2: eth0    inet 10.0.0.5/24 brd 10.0.0.255 scope global eth0\\       valid_lft forever
2: eth0    inet6 2001:db8::5/64 scope global \\       valid_lft forever
2: eth0    inet6 fe80::1/64 scope link \\       valid_lft forever
";
        let a = parse_ip_addr(ip);
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a[0].iface.as_str(), a[0].addr.as_str()),
            ("eth0", "10.0.0.5/24")
        );
        assert_eq!(
            parse_who("root pts/0 2026-10-19 09:00 (10.0.0.9)\nroot pts/1 x\nanna tty1 y\n"),
            vec!["root", "anna"]
        );
        let mut det = HostDetail::default();
        parse_linux_extras(
            &format!(
                "@@cpu\n Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz\n@@arch\nx86_64\n@@osver\n12\n@@fqdn\nweb1.example.org\n@@virt\nkvm\n@@model\nQEMU Standard PC\n@@df\n{df}@@ip\n{ip}@@users\nroot pts/0\n"
            ),
            &mut det,
        );
        assert_eq!(det.cpu_model, "Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz");
        assert_eq!(det.domain, "web1.example.org");
        assert_eq!(det.model, "QEMU Standard PC (kvm)");
        assert_eq!(
            (det.disks.len(), det.addresses.len(), det.users.len()),
            (3, 2, 1)
        );
    }

    #[test]
    fn ufw_rules() {
        let out = "\
Status: active
Logging: on (low)
Default: deny (incoming), allow (outgoing), disabled (routed)
New profiles: skip

To                         Action      From
--                         ------      ----
22/tcp                     ALLOW IN    Anywhere
80,443/tcp                 ALLOW IN    10.0.0.0/24
Anywhere                   DENY IN     203.0.113.7
22/tcp (v6)                ALLOW IN    Anywhere (v6)
";
        let fw = parse_ufw(out);
        assert_eq!(fw.active, Some(true));
        assert_eq!(fw.summary.len(), 3);
        assert_eq!(fw.rules.len(), 4);
        let r = &fw.rules[1];
        assert_eq!(
            (r.port.as_str(), r.protocol.as_str(), r.action.as_str()),
            ("80,443", "tcp", "ALLOW")
        );
        assert_eq!(
            (r.direction.as_str(), r.source.as_str()),
            ("in", "10.0.0.0/24")
        );
        assert_eq!(fw.rules[2].action, "DENY");
        assert_eq!(parse_ufw("Status: inactive\n").active, Some(false));
    }

    #[test]
    fn firewalld_and_iptables_and_nft() {
        let out = "\
public (active)
  target: default
  icmp-block-inversion: no
  interfaces: eth0
  sources:
  services: cockpit dhcpv6-client ssh
  ports: 8080/tcp 53/udp
  protocols:
  forward: yes
  masquerade: no
  forward-ports:
  source-ports:
  icmp-blocks:
  rich rules:
	rule family=\"ipv4\" source address=\"10.1.1.1\" reject
";
        let fw = parse_firewalld(out);
        assert_eq!(fw.rules.len(), 3 + 2 + 1);
        assert!(fw.summary[0].starts_with("zone: public"));
        assert!(
            fw.rules
                .iter()
                .any(|r| r.port == "8080" && r.protocol == "tcp")
        );
        assert!(fw.rules.last().unwrap().raw.contains("reject"));
        let ipt = parse_iptables(
            "-P INPUT DROP\n-P FORWARD DROP\n-P OUTPUT ACCEPT\n-A INPUT -i lo -j ACCEPT\n-A INPUT -p tcp -m tcp --dport 22 -j ACCEPT\n-A OUTPUT -s 10.0.0.1/32 -j DROP\n",
        );
        assert_eq!(
            ipt.summary,
            vec!["INPUT: DROP", "FORWARD: DROP", "OUTPUT: ACCEPT"]
        );
        assert_eq!(ipt.rules.len(), 3);
        assert_eq!(
            (
                ipt.rules[1].port.as_str(),
                ipt.rules[1].protocol.as_str(),
                ipt.rules[1].action.as_str()
            ),
            ("22", "tcp", "ACCEPT")
        );
        assert_eq!(ipt.rules[2].direction, "out");
        let nft = parse_nft(
            "table inet filter {\n\tchain input {\n\t\ttype filter hook input priority filter; policy drop;\n\t}\n}\n",
        );
        assert_eq!(nft.rules.len(), 5);
        assert_eq!(nft.active, Some(true));
    }

    #[test]
    fn firewall_script_choice() {
        // No root: every tool refuses.
        let denied_all = "@@fw ufw 1\nERROR: You need to be root to run this script\n@@fw nft 1\nOperation not permitted\n@@fw iptables 1\niptables v1.8.9: can't initialize iptables table `filter': Permission denied (you must be root)\n";
        let fw = parse_linux_firewall(denied_all);
        assert_eq!(fw.avail, Avail::NeedsRoot);
        assert_eq!(parse_linux_firewall("").avail, Avail::Missing);
        // ufw is installed but inactive, nft has the real rules.
        let both = "@@fw ufw 0\nStatus: inactive\n@@fw nft 0\ntable inet f {\n}\n";
        let fw = parse_linux_firewall(both);
        assert_eq!(fw.backend, "nft");
        // Only an inactive ufw: reported as such.
        let fw = parse_linux_firewall("@@fw ufw 0\nStatus: inactive\n");
        assert_eq!(
            (fw.backend.as_str(), fw.active, fw.avail),
            ("ufw", Some(false), Avail::Ok)
        );
        // A failing tool that is not about permissions.
        assert_eq!(
            parse_linux_firewall("@@fw nft 3\nsomething broke\n").avail,
            Avail::Failed
        );
        // Rules are cut at the limit but counted.
        let mut big = String::from("@@fw iptables 0\n");
        for i in 0..300 {
            big.push_str(&format!("-A INPUT -p tcp --dport {} -j ACCEPT\n", 1000 + i));
        }
        let fw = parse_linux_firewall(&big);
        assert_eq!((fw.rules.len(), fw.total_rules), (MAX_RULES, 300));
    }
}
