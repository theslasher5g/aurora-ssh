//! Persistent data model: hosts, groups, snippets, keychain identities, settings.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Hash, Default)]
pub enum Protocol {
    #[default]
    Ssh,
    Rdp,
    Serial,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [Protocol::Ssh, Protocol::Rdp, Protocol::Serial];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ssh => "SSH",
            Protocol::Rdp => "RDP",
            Protocol::Serial => "Serial",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Protocol::Ssh => "🖧",
            Protocol::Rdp => "🖥",
            Protocol::Serial => "🔌",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ssh => 22,
            Protocol::Rdp => 3389,
            Protocol::Serial => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum FlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum LineEnding {
    #[default]
    Cr,
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            LineEnding::Cr => b"\r",
            LineEnding::Lf => b"\n",
            LineEnding::CrLf => b"\r\n",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct SerialSettings {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: Parity,
    pub flow_control: FlowControl,
    pub line_ending: LineEnding,
    pub local_echo: bool,
}

impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: 8,
            stop_bits: 1,
            parity: Parity::None,
            flow_control: FlowControl::None,
            line_ending: LineEnding::Cr,
            local_echo: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct RdpSettings {
    /// Show the session inside Aurora (IronRDP) instead of the system client.
    pub embedded: bool,
    pub domain: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub clipboard: bool,
    pub multimon: bool,
    /// OpenSSH port on the Windows host, used for file access (SFTP).
    pub sftp_port: u16,
}

impl Default for RdpSettings {
    fn default() -> Self {
        Self {
            embedded: true,
            domain: String::new(),
            width: 1600,
            height: 900,
            fullscreen: false,
            clipboard: true,
            multimon: false,
            sftp_port: 22,
        }
    }
}

/// Local port forwarding: `localhost:local_port` → (as seen from the server)
/// `remote_host:remote_port`.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Tunnel {
    pub id: Uuid,
    pub name: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    /// Open `http://localhost:<local_port>` once the tunnel is up.
    pub open_browser: bool,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            local_port: 8080,
            remote_host: "localhost".into(),
            remote_port: 80,
            open_browser: false,
        }
    }
}

impl Tunnel {
    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            format!(
                "{}:{} → :{}",
                self.remote_host, self.remote_port, self.local_port
            )
        } else {
            self.name.clone()
        }
    }
}

/// One session of the last run, for "restore sessions".
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedSession {
    pub host: Uuid,
    /// "term", "files" or "rdp"
    pub kind: String,
    pub in_split: bool,
}

/// Environment label of a host or group; production sessions get a red frame and
/// confirm dangerous commands.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub enum Env {
    #[default]
    None,
    Prod,
    Staging,
    Test,
    Dev,
}

impl Env {
    pub const ALL: [Env; 5] = [Env::None, Env::Prod, Env::Staging, Env::Test, Env::Dev];

    pub fn label(self) -> &'static str {
        match self {
            Env::None => "—",
            Env::Prod => "PROD",
            Env::Staging => "STAGING",
            Env::Test => "TEST",
            Env::Dev => "DEV",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Host {
    pub id: Uuid,
    pub label: String,
    pub group_id: Option<Uuid>,
    pub protocol: Protocol,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub identity_id: Option<Uuid>,
    pub color: Option<[u8; 3]>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub notes: String,
    /// `Env::None` inherits from the group.
    pub env: Env,
    /// Connect through this SSH host (ProxyJump).
    pub jump_host: Option<Uuid>,
    /// Saved port forwardings.
    pub tunnels: Vec<Tunnel>,
    pub startup_snippet: Option<Uuid>,
    pub last_connected: Option<DateTime<Utc>>,
    pub serial: SerialSettings,
    pub rdp: RdpSettings,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            label: String::new(),
            group_id: None,
            protocol: Protocol::Ssh,
            hostname: String::new(),
            port: 22,
            username: String::new(),
            identity_id: None,
            color: None,
            tags: Vec::new(),
            favorite: false,
            notes: String::new(),
            env: Env::None,
            jump_host: None,
            tunnels: Vec::new(),
            startup_snippet: None,
            last_connected: None,
            serial: SerialSettings::default(),
            rdp: RdpSettings::default(),
        }
    }
}

impl Host {
    pub fn display_name(&self) -> &str {
        if !self.label.trim().is_empty() {
            &self.label
        } else if self.protocol == Protocol::Serial {
            &self.serial.port_name
        } else {
            &self.hostname
        }
    }

    pub fn subtitle(&self) -> String {
        match self.protocol {
            Protocol::Serial => {
                format!("{} · {} baud", self.serial.port_name, self.serial.baud_rate)
            }
            _ => {
                let user = if self.username.is_empty() {
                    String::new()
                } else {
                    format!("{}@", self.username)
                };
                if self.port == self.protocol.default_port() {
                    format!("{user}{}", self.hostname)
                } else {
                    format!("{user}{}:{}", self.hostname, self.port)
                }
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub color: [u8; 3],
    pub icon: String,
    /// Defaults inherited by hosts in this group that leave the field empty.
    pub default_username: String,
    pub default_identity: Option<Uuid>,
    pub env: Env,
    pub collapsed: bool,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            parent_id: None,
            color: [139, 92, 246],
            icon: "🗀".into(),
            default_username: String::new(),
            default_identity: None,
            env: Env::None,
            collapsed: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Snippet {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub script: String,
    pub tags: Vec<String>,
    /// Press enter after pasting the snippet.
    pub auto_run: bool,
}

impl Default for Snippet {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            description: String::new(),
            script: String::new(),
            tags: Vec::new(),
            auto_run: true,
        }
    }
}

impl Snippet {
    /// Returns the distinct `{{variable}}` placeholders in order of appearance.
    pub fn variables(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut rest = self.script.as_str();
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            let name = after[..end].trim().to_string();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
            rest = &after[end + 2..];
        }
        out
    }

    pub fn render(&self, values: &HashMap<String, String>) -> String {
        let mut out = String::with_capacity(self.script.len());
        let mut rest = self.script.as_str();
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            out.push_str(&rest[..start]);
            let name = after[..end].trim();
            match values.get(name) {
                Some(v) => out.push_str(v),
                None => out.push_str(&rest[start..start + 2 + end + 2]),
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        out
    }
}

/// A credential stored in the encrypted keychain.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct Identity {
    pub id: Uuid,
    pub label: String,
    pub username: String,
    pub password: String,
    /// OpenSSH / PEM private key text.
    pub private_key: String,
    pub passphrase: String,
}

impl Identity {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct Keychain {
    pub identities: Vec<Identity>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    pub scrollback: usize,
    pub theme: String,
    pub copy_on_select: bool,
    /// Look for a newer release on start.
    pub check_updates: bool,
    /// Probe hosts in the list and show whether they are reachable.
    pub reachability: bool,
    /// Lock the vault after this many minutes without input (0 = never).
    pub auto_lock_minutes: u32,
    /// Ask before dangerous commands on every host, not only on PROD.
    pub warn_everywhere: bool,
    /// The first-start tour was shown.
    pub onboarded: bool,
    /// Subnets for which "group these hosts" was dismissed.
    pub dismissed_subnets: Vec<String>,
    /// Smaller rows in the host list.
    pub compact: bool,
    /// Host list order inside a group: 0 name, 1 last used, 2 online first.
    pub sort: u8,
    /// Reopen the sessions of the last run after unlocking.
    pub restore_sessions: bool,
    pub last_session: Vec<SavedSession>,
    /// Command used to open files for editing (empty = system default).
    pub editor: String,
    /// "de" or "en".
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            scrollback: 5000,
            theme: "phosphor".into(),
            copy_on_select: true,
            check_updates: true,
            reachability: true,
            auto_lock_minutes: 15,
            warn_everywhere: false,
            onboarded: false,
            dismissed_subnets: Vec::new(),
            compact: false,
            sort: 0,
            restore_sessions: true,
            last_session: Vec::new(),
            editor: String::new(),
            language: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct Library {
    pub hosts: Vec<Host>,
    pub groups: Vec<Group>,
    pub snippets: Vec<Snippet>,
    pub settings: Settings,
    /// `host:port` -> `SHA256:...` fingerprint of the accepted server key.
    pub known_hosts: HashMap<String, String>,
    /// What Aurora learned about each host on the last SSH login.
    pub facts: HashMap<Uuid, HostFacts>,
}

/// System facts collected after an SSH login (read-only commands).
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HostFacts {
    /// os-release `ID` (`ubuntu`, `debian` …) or `windows`.
    pub os_id: String,
    pub os_name: String,
    pub hostname: String,
    pub uptime_secs: u64,
    pub disk_used_pct: Option<u8>,
    /// Pending package updates, if the package manager was recognised.
    pub updates: Option<u32>,
    pub proxmox: bool,
    pub collected: Option<DateTime<Utc>>,
}

impl HostFacts {
    /// Parses the `key=value` lines printed by the facts script.
    pub fn parse(text: &str) -> Option<Self> {
        let mut f = HostFacts::default();
        let clean = |v: &str| -> String {
            v.trim()
                .trim_matches('"')
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect()
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "id" => f.os_id = clean(v).to_lowercase(),
                "name" => f.os_name = clean(v),
                "host" => f.hostname = clean(v),
                "up" => f.uptime_secs = v.trim().parse().unwrap_or(0),
                "disk" => f.disk_used_pct = v.trim().trim_end_matches('%').parse().ok(),
                "upd" => f.updates = v.trim().parse().ok(),
                "pve" => f.proxmox = v.trim() == "1",
                _ => {}
            }
        }
        (!f.os_id.is_empty() || !f.hostname.is_empty()).then_some(f)
    }

    /// Output of `ver & hostname` on Windows.
    pub fn parse_windows(text: &str) -> Option<Self> {
        let ver = text.lines().find(|l| l.contains("Windows"))?;
        let version = ver
            .split("Version")
            .nth(1)
            .map(|v| v.trim().trim_end_matches(']').trim().to_string())
            .unwrap_or_default();
        let hostname = text
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty() && !l.contains("Windows"))
            .unwrap_or_default()
            .to_string();
        Some(HostFacts {
            os_id: "windows".into(),
            os_name: if version.is_empty() {
                "Windows".into()
            } else {
                format!("Windows {version}")
            },
            hostname,
            ..Default::default()
        })
    }

    /// Short label for the host list, e.g. `Ubuntu 24.04`.
    pub fn short_os(&self) -> String {
        if self.proxmox {
            return "Proxmox VE".into();
        }
        let name = self
            .os_name
            .replace("GNU/Linux", "")
            .replace(" LTS", "")
            .replace("(", "")
            .replace(")", "");
        let words: Vec<&str> = name.split_whitespace().collect();
        // "Ubuntu 24.04.1" -> "Ubuntu 24.04", "Debian 12 bookworm" -> "Debian 12"
        let mut out: Vec<String> = Vec::new();
        for w in words.iter().take(3) {
            if out.len() == 1 {
                let v: Vec<&str> = w.split('.').collect();
                out.push(v.iter().take(2).cloned().collect::<Vec<_>>().join("."));
                break;
            }
            out.push(w.to_string());
        }
        out.join(" ")
    }
}

impl Library {
    pub fn host(&self, id: Uuid) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    pub fn group(&self, id: Uuid) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn snippet(&self, id: Uuid) -> Option<&Snippet> {
        self.snippets.iter().find(|s| s.id == id)
    }

    /// Walks up the group chain and returns the first non-empty default username / identity.
    pub fn inherited_defaults(&self, host: &Host) -> (String, Option<Uuid>) {
        let mut user = String::new();
        let mut identity = None;
        let mut next = host.group_id;
        let mut guard = 0;
        while let Some(gid) = next {
            guard += 1;
            if guard > 32 {
                break;
            }
            let Some(g) = self.group(gid) else { break };
            if user.is_empty() && !g.default_username.is_empty() {
                user = g.default_username.clone();
            }
            if identity.is_none() {
                identity = g.default_identity;
            }
            next = g.parent_id;
        }
        (user, identity)
    }

    /// Environment of a host: its own, else the nearest group's.
    pub fn env_of(&self, host: &Host) -> Env {
        if host.env != Env::None {
            return host.env;
        }
        let mut next = host.group_id;
        for _ in 0..32 {
            let Some(g) = next.and_then(|id| self.group(id)) else {
                break;
            };
            if g.env != Env::None {
                return g.env;
            }
            next = g.parent_id;
        }
        Env::None
    }

    /// All hosts in `group` and its subgroups, in list order.
    pub fn hosts_in_group(&self, group: Uuid) -> Vec<Host> {
        self.hosts
            .iter()
            .filter(|h| h.group_id.is_some_and(|g| self.is_descendant(g, group)))
            .cloned()
            .collect()
    }

    /// True if `candidate` is `group` or one of its descendants (prevents parent cycles).
    pub fn is_descendant(&self, candidate: Uuid, group: Uuid) -> bool {
        let mut next = Some(candidate);
        let mut guard = 0;
        while let Some(id) = next {
            if id == group {
                return true;
            }
            guard += 1;
            if guard > 64 {
                return true;
            }
            next = self.group(id).and_then(|g| g.parent_id);
        }
        false
    }
}

/// IPv4 network prefix `a.b.c` of a host address, for "same subnet" grouping.
pub fn subnet24(addr: &str) -> Option<String> {
    let ip: std::net::Ipv4Addr = addr.trim().parse().ok()?;
    let o = ip.octets();
    Some(format!("{}.{}.{}", o[0], o[1], o[2]))
}

/// Commands that deserve a second look before they run (on PROD).
pub fn dangerous_command(line: &str) -> Option<&'static str> {
    let l = line.trim().to_lowercase();
    let l = l.strip_prefix("sudo ").unwrap_or(&l).trim();
    let words: Vec<&str> = l.split_whitespace().collect();
    let first = *words.first()?;
    let has = |w: &str| words.contains(&w);
    let rm_recursive = first == "rm"
        && words.iter().any(|w| {
            w.starts_with('-') && !w.starts_with("--") && (w.contains('r') || w.contains('R'))
        })
        || first == "rm" && has("--recursive");
    if rm_recursive {
        return Some("löscht rekursiv");
    }
    match first {
        "shutdown" | "poweroff" | "halt" => return Some("fährt den Server herunter"),
        "reboot" => return Some("startet den Server neu"),
        "dd" => return Some("schreibt direkt auf ein Gerät"),
        "init" if has("0") || has("6") => return Some("ändert den Runlevel"),
        "kill" | "killall" | "pkill" if has("-9") || has("-kill") => {
            return Some("beendet Prozesse hart");
        }
        "truncate" => return Some("leert Dateien"),
        "chmod" | "chown" if has("-r") || has("--recursive") => {
            return Some("ändert Rechte rekursiv");
        }
        _ => {}
    }
    if first.starts_with("mkfs") {
        return Some("formatiert ein Dateisystem");
    }
    if first == "systemctl" && (has("stop") || has("restart") || has("poweroff") || has("reboot")) {
        return Some("stoppt oder startet Dienste");
    }
    if l.contains("drop database") || l.contains("drop table") || l.contains("truncate table") {
        return Some("löscht Datenbankinhalte");
    }
    if l.contains(":(){") || l.contains("> /dev/sd") || l.contains(">/dev/sd") {
        return Some("überschreibt Systemdaten");
    }
    None
}

/// Hosts from an OpenSSH config file (`Host` blocks without wildcards).
pub fn parse_ssh_config(text: &str) -> Vec<Host> {
    let mut out: Vec<Host> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for name in value.split_whitespace() {
                    if name.contains(['*', '?', '!']) {
                        continue;
                    }
                    out.push(Host {
                        label: name.to_string(),
                        hostname: name.to_string(),
                        ..Default::default()
                    });
                    current.push(out.len() - 1);
                }
            }
            "match" => current.clear(),
            "hostname" => current
                .iter()
                .for_each(|&i| out[i].hostname = value.to_string()),
            "user" => current
                .iter()
                .for_each(|&i| out[i].username = value.to_string()),
            "port" => {
                if let Ok(p) = value.parse() {
                    current.iter().for_each(|&i| out[i].port = p);
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_facts() {
        let f = HostFacts::parse(
            "id=ubuntu\nname=\"Ubuntu 24.04.1 LTS\"\nhost=pihole\nup=86400\ndisk=71%\nupd=12\n",
        )
        .unwrap();
        assert_eq!(f.short_os(), "Ubuntu 24.04");
        assert_eq!(
            (f.uptime_secs, f.disk_used_pct, f.updates),
            (86400, Some(71), Some(12))
        );
        assert_eq!(f.hostname, "pihole");
        let d = HostFacts::parse("id=debian\nname=Debian GNU/Linux 12 (bookworm)\n").unwrap();
        assert_eq!(d.short_os(), "Debian 12");
        let w = HostFacts::parse_windows(
            "\r\nMicrosoft Windows [Version 10.0.20348.2340]\r\nSRV-DC01\r\n",
        )
        .unwrap();
        assert_eq!(
            (w.os_id.as_str(), w.hostname.as_str()),
            ("windows", "SRV-DC01")
        );
        assert!(HostFacts::parse("garbage").is_none());
        assert_eq!(subnet24("192.168.0.44").as_deref(), Some("192.168.0"));
        assert_eq!(subnet24("web-01.example.com"), None);
    }

    #[test]
    fn dangerous_commands() {
        assert!(dangerous_command("rm -rf /var/www").is_some());
        assert!(dangerous_command("sudo rm -Rf x").is_some());
        assert!(dangerous_command("rm file.txt").is_none());
        assert!(dangerous_command("sudo reboot").is_some());
        assert!(dangerous_command("systemctl restart nginx").is_some());
        assert!(dangerous_command("systemctl status nginx").is_none());
        assert!(dangerous_command("mysql -e 'DROP TABLE users'").is_some());
        assert!(dangerous_command("ls -la").is_none());
        assert!(dangerous_command("echo shutdown").is_none());
    }

    #[test]
    fn ssh_config_import() {
        let cfg = "Host *\n  ServerAliveInterval 30\nHost web db\n  HostName 10.0.0.5\n  User deploy\n  Port 2222\nHost pi\n  HostName=raspberrypi.local\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 3);
        assert_eq!(
            (
                hosts[0].label.as_str(),
                hosts[0].hostname.as_str(),
                hosts[0].username.as_str(),
                hosts[0].port
            ),
            ("web", "10.0.0.5", "deploy", 2222)
        );
        assert_eq!(hosts[2].hostname, "raspberrypi.local");
        assert_eq!(hosts[2].port, 22);
    }

    #[test]
    fn snippet_variables_and_render() {
        let s = Snippet {
            script: "ssh {{ user }}@{{host}} -p {{port}} # {{user}}".into(),
            ..Default::default()
        };
        assert_eq!(s.variables(), vec!["user", "host", "port"]);
        let mut v = HashMap::new();
        v.insert("user".to_string(), "root".to_string());
        v.insert("host".to_string(), "box".to_string());
        assert_eq!(s.render(&v), "ssh root@box -p {{port}} # root");
    }

    #[test]
    fn group_defaults_are_inherited() {
        let mut lib = Library::default();
        let parent = Group {
            default_username: "admin".into(),
            ..Default::default()
        };
        let child = Group {
            parent_id: Some(parent.id),
            ..Default::default()
        };
        let host = Host {
            group_id: Some(child.id),
            ..Default::default()
        };
        assert!(lib.is_descendant(child.id, child.id));
        lib.groups.push(parent.clone());
        lib.groups.push(child.clone());
        assert_eq!(lib.inherited_defaults(&host).0, "admin");
        assert!(lib.is_descendant(child.id, parent.id));
        assert!(!lib.is_descendant(parent.id, child.id));
    }
}
