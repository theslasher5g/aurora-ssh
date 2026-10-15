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

    pub fn kind(&self) -> &'static str {
        match (self.private_key.is_empty(), self.password.is_empty()) {
            (false, false) => "Key + Passwort",
            (false, true) => "SSH-Key",
            (true, false) => "Passwort",
            (true, true) => "Nur Benutzer",
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            scrollback: 5000,
            theme: "aurora".into(),
            copy_on_select: true,
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

    pub fn seed_examples(&mut self) {
        let prod = Group {
            name: "Production".into(),
            color: [248, 113, 113],
            icon: "🔥".into(),
            ..Default::default()
        };
        let lab = Group {
            name: "Homelab".into(),
            color: [45, 226, 195],
            icon: "🏠".into(),
            ..Default::default()
        };
        self.hosts.push(Host {
            label: "web-01".into(),
            hostname: "web-01.example.com".into(),
            username: "deploy".into(),
            group_id: Some(prod.id),
            tags: vec!["nginx".into()],
            ..Default::default()
        });
        self.hosts.push(Host {
            label: "Raspberry Pi".into(),
            hostname: "192.168.1.50".into(),
            username: "pi".into(),
            group_id: Some(lab.id),
            favorite: true,
            ..Default::default()
        });
        self.hosts.push(Host {
            label: "Windows Desktop".into(),
            protocol: Protocol::Rdp,
            hostname: "192.168.1.20".into(),
            port: 3389,
            group_id: Some(lab.id),
            ..Default::default()
        });
        self.hosts.push(Host {
            label: "Router Console".into(),
            protocol: Protocol::Serial,
            port: 0,
            group_id: Some(lab.id),
            serial: SerialSettings {
                port_name: if cfg!(windows) {
                    "COM3".into()
                } else {
                    "/dev/ttyUSB0".into()
                },
                ..Default::default()
            },
            ..Default::default()
        });
        self.groups.push(prod);
        self.groups.push(lab);

        let snippets = [
            (
                "System-Update",
                "Pakete aktualisieren",
                "sudo apt update && sudo apt upgrade -y",
            ),
            ("Disk Usage", "Belegung aller Mounts", "df -h"),
            (
                "Top Prozesse",
                "CPU-hungrigste Prozesse",
                "ps aux --sort=-%cpu | head -n 15",
            ),
            (
                "Logs folgen",
                "journalctl eines Dienstes",
                "sudo journalctl -fu {{service}}",
            ),
            (
                "Docker PS",
                "Laufende Container",
                "docker ps --format 'table {{.Names}}\\t{{.Status}}\\t{{.Ports}}'",
            ),
        ];
        for (name, desc, script) in snippets {
            self.snippets.push(Snippet {
                name: name.into(),
                description: desc.into(),
                script: script.into(),
                ..Default::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
