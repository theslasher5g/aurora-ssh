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
    Telnet,
    Vnc,
    /// Shell in a Docker/Podman container (on this PC or an SSH host).
    Docker,
    /// Shell in a Kubernetes pod (kubectl on this PC or an SSH host).
    Kubernetes,
    /// Web interface of a firewall, switch, NAS … (opened by the `aurora-web` helper).
    Web,
}

impl Protocol {
    pub const ALL: [Protocol; 8] = [
        Protocol::Ssh,
        Protocol::Rdp,
        Protocol::Vnc,
        Protocol::Telnet,
        Protocol::Serial,
        Protocol::Docker,
        Protocol::Kubernetes,
        Protocol::Web,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ssh => "SSH",
            Protocol::Rdp => "RDP",
            Protocol::Serial => "Serial",
            Protocol::Telnet => "Telnet",
            Protocol::Vnc => "VNC",
            Protocol::Docker => "Docker",
            Protocol::Kubernetes => "Kubernetes",
            Protocol::Web => "Web",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Protocol::Ssh => "🖧",
            Protocol::Rdp => "🖥",
            Protocol::Serial => "🔌",
            Protocol::Telnet => "☎",
            Protocol::Vnc => "🖵",
            Protocol::Docker => "📦",
            Protocol::Kubernetes => "☸",
            Protocol::Web => "🌐",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ssh => 22,
            Protocol::Rdp => 3389,
            Protocol::Serial => 0,
            Protocol::Telnet => 23,
            Protocol::Vnc => 5900,
            Protocol::Web => 443,
            Protocol::Docker | Protocol::Kubernetes => 0,
        }
    }

    /// Reached by TCP at `hostname:port` (reachability LED, scan, address field).
    pub fn is_network(self) -> bool {
        matches!(
            self,
            Protocol::Ssh | Protocol::Rdp | Protocol::Telnet | Protocol::Vnc | Protocol::Web
        )
    }

    pub fn is_container(self) -> bool {
        matches!(self, Protocol::Docker | Protocol::Kubernetes)
    }

    /// Opens a text terminal (and can use a terminal profile).
    pub fn is_terminal(self) -> bool {
        !matches!(self, Protocol::Rdp | Protocol::Vnc | Protocol::Web)
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
    /// Show a local folder as a drive in the session (drive redirection, off by default).
    pub drive_enabled: bool,
    /// The local folder to share.
    pub drive_path: String,
    /// Name of the drive in Explorer ("" = folder name).
    pub drive_name: String,
    /// Let the session change files in the folder (default: read only).
    pub drive_write: bool,
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
            drive_enabled: false,
            drive_path: String::new(),
            drive_name: String::new(),
            drive_write: false,
        }
    }
}

/// Docker/Podman container or Kubernetes pod reached through its CLI.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ContainerSettings {
    /// SSH host that runs docker/kubectl; `None` = this PC.
    pub via: Option<Uuid>,
    /// Docker: `podman` instead of `docker`.
    pub podman: bool,
    /// Container name/ID or pod name.
    pub name: String,
    /// Kubernetes namespace ("" = default).
    pub namespace: String,
    /// Kubernetes container inside the pod ("" = default container).
    pub container: String,
    /// kubectl context ("" = current).
    pub context: String,
    /// Shell to start ("" = bash, else sh).
    pub shell: String,
    /// Run as this user (docker exec -u).
    pub user: String,
}

/// Terminal look per host; empty/zero values follow the app settings.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(default)]
pub struct TermProfile {
    /// Theme id for the terminal colors.
    pub theme: String,
    /// Font name from the font list, or a path to a .ttf/.otf file.
    pub font: String,
    pub font_size: f32,
}

/// Web interface entry (protocol `Web`). `Host::hostname`/`port` mirror the URL so the
/// reachability LED, certificate radar and search work like for any network host.
/// Passwords are never stored here: they come from the keychain/password manager.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct WebSettings {
    /// `https://host[:port]/path`; only http and https are accepted.
    pub url: String,
    pub tls: crate::backend::web::TlsMode,
    /// SHA-256 fingerprint of the pinned (self-signed) certificate, `AA:BB:…`.
    pub pinned_fp: String,
    /// `host:port` the pin was taken for.
    pub pinned_for: String,
    pub pinned_at: Option<DateTime<Utc>>,
    /// Fill the login form with the entry's credentials.
    pub auto_login: bool,
    /// Press the login button after filling.
    pub submit: bool,
    /// CSS selectors of the login fields; empty = detect automatically.
    pub user_selector: String,
    pub pass_selector: String,
    /// Further hosts the page may show inside the window (SSO …), comma separated.
    pub extra_hosts: String,
    /// Vendor template the entry was created from.
    pub template: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            url: String::new(),
            tls: Default::default(),
            pinned_fp: String::new(),
            pinned_for: String::new(),
            pinned_at: None,
            auto_login: false,
            submit: true,
            user_selector: String::new(),
            pass_selector: String::new(),
            extra_hosts: String::new(),
            template: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(default)]
pub struct VncSettings {
    /// Only watch, never send keyboard/mouse.
    pub view_only: bool,
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
    /// "term", "files", "rdp", "vnc" or "log"
    pub kind: String,
    pub in_split: bool,
    /// Log tabs: the followed file or command.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub title: String,
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
    pub container: ContainerSettings,
    pub vnc: VncSettings,
    /// Terminal colors and font for this host.
    pub term: TermProfile,
    /// SSH: open the shell inside `tmux` (or `screen`) so a reconnect lands in the same
    /// session with its running jobs.
    pub persistent: bool,
    /// SSH: log in with a short-lived certificate signed by the keychain CA.
    #[serde(default)]
    pub cert: CertSettings,
    /// Wake-on-LAN: MAC address and where to send the magic packet.
    #[serde(default)]
    pub wol: WolSettings,
    /// Reconnect this host's session automatically after an unexpected drop.
    #[serde(default)]
    pub auto_reconnect: ReconnectMode,
    /// Password (and optionally user name) from an external password manager; only the
    /// reference is stored here.
    #[serde(default)]
    pub credential_source: Option<crate::backend::pwmanager_types::CredentialSource>,
    /// SSH: set up shell integration (OSC 133/7 marks) in the login shell after login.
    #[serde(default)]
    pub shell_integration: bool,
    /// Web interface entries (`Protocol::Web`).
    #[serde(default)]
    pub web: WebSettings,
    /// Windows hosts: facts and commands through WinRM (no SSH needed).
    #[serde(default)]
    pub winrm: crate::backend::winrm_types::WinrmSettings,
    /// Set on entries a dynamic folder generated (Proxmox, list); see `nav2_dyn`.
    #[serde(default)]
    pub dynamic: Option<crate::nav2_dyn::DynMark>,
}

/// Per-host override of the global "reconnect automatically" switch.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub enum ReconnectMode {
    /// Follow the global setting.
    #[default]
    Default,
    On,
    Off,
}

/// Wake-on-LAN settings of a host.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct WolSettings {
    /// `aa:bb:cc:dd:ee:ff`; empty = no Wake-on-LAN.
    pub mac: String,
    /// Broadcast address; empty = 255.255.255.255.
    pub broadcast: String,
    /// UDP port; 0 = 9.
    pub port: u16,
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
            container: ContainerSettings::default(),
            vnc: VncSettings::default(),
            term: TermProfile::default(),
            persistent: false,
            cert: CertSettings::default(),
            wol: WolSettings::default(),
            auto_reconnect: ReconnectMode::Default,
            credential_source: None,
            shell_integration: false,
            web: WebSettings::default(),
            winrm: Default::default(),
            dynamic: None,
        }
    }
}

impl Host {
    pub fn display_name(&self) -> &str {
        if !self.label.trim().is_empty() {
            &self.label
        } else if self.protocol == Protocol::Serial {
            &self.serial.port_name
        } else if self.protocol.is_container() {
            &self.container.name
        } else {
            &self.hostname
        }
    }

    pub fn subtitle(&self) -> String {
        match self.protocol {
            Protocol::Serial => {
                format!("{} · {} baud", self.serial.port_name, self.serial.baud_rate)
            }
            Protocol::Web => {
                let url = self.web.url.trim();
                url.split_once("://")
                    .map_or(url, |(_, rest)| rest)
                    .to_string()
            }
            Protocol::Docker | Protocol::Kubernetes => {
                let c = &self.container;
                let mut s = if self.protocol == Protocol::Kubernetes && !c.namespace.is_empty() {
                    format!("{}/{}", c.namespace, c.name)
                } else {
                    c.name.clone()
                };
                if !c.container.is_empty() {
                    s = format!("{s}:{}", c.container);
                }
                s
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
    /// Where the script runs; PowerShell snippets can also run on RDP hosts.
    pub shell: ShellKind,
}

/// Script language of a snippet.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShellKind {
    /// Pasted into a terminal session (bash, zsh, cmd …).
    #[default]
    Shell,
    /// Windows PowerShell: in a terminal, or started on an RDP host via Win+R.
    PowerShell,
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
            shell: ShellKind::Shell,
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
    /// Keychain group (`Keychain::groups`); `None` = "Ohne Gruppe".
    pub group_id: Option<Uuid>,
    /// Password or private key taken from a password manager at connect time (only the
    /// reference is stored; the vault does not hold the secret).
    pub secret_source: Option<crate::backend::pwmanager_types::CredentialSource>,
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
    /// Proxmox server id → API token secret or password.
    pub pve_secrets: HashMap<Uuid, String>,
    /// Proxmox Backup Server id → API token secret or password.
    pub pbs_secrets: HashMap<Uuid, String>,
    /// Groups on the keys page, in display order.
    pub groups: Vec<KeyGroup>,
    /// Aurora's own SSH certificate authority (signs short-lived user certificates).
    #[serde(default)]
    pub ca: Option<SshCa>,
}

impl Keychain {
    pub fn key_group(&self, id: Uuid) -> Option<&KeyGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// Group of an identity; dangling ids count as "no group".
    pub fn group_of(&self, ident: &Identity) -> Option<&KeyGroup> {
        ident.group_id.and_then(|g| self.key_group(g))
    }

    /// Identities in display order: by group (in group order), ungrouped last, each
    /// list sorted by name.
    pub fn grouped(&self) -> Vec<(Option<&KeyGroup>, Vec<&Identity>)> {
        fn sorted(mut v: Vec<&Identity>) -> Vec<&Identity> {
            v.sort_by_key(|i| i.label.to_lowercase());
            v
        }
        let mut out: Vec<(Option<&KeyGroup>, Vec<&Identity>)> = self
            .groups
            .iter()
            .map(|g| {
                let members = self
                    .identities
                    .iter()
                    .filter(|i| i.group_id == Some(g.id))
                    .collect();
                (Some(g), sorted(members))
            })
            .collect();
        let loose = self
            .identities
            .iter()
            .filter(|i| self.group_of(i).is_none())
            .collect();
        out.push((None, sorted(loose)));
        out
    }
}

/// A group of keys on the keychain page (stored in the encrypted keychain).
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct KeyGroup {
    pub id: Uuid,
    pub name: String,
    pub color: [u8; 3],
    pub collapsed: bool,
    /// Customer (host group) this key group belongs to: the customer focus shows only
    /// the key groups of that customer.
    pub customer: Option<Uuid>,
}

impl Default for KeyGroup {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            color: [139, 92, 246],
            collapsed: false,
            customer: None,
        }
    }
}

/// A Proxmox VE server (or cluster) shown on the Proxmox page. The secret lives in the
/// keychain (`Keychain::pve_secrets`).
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct PveServer {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// API token (`user@realm!tokenid`) instead of user + password.
    pub token: bool,
    /// `user@realm` or, with a token, `user@realm!tokenid`.
    pub user: String,
    /// SHA-256 fingerprint of the trusted certificate (`AB:CD:…`), empty = not yet.
    pub tls_pin: String,
    /// SSH host that reaches the nodes (consoles via `pct enter` / `qm terminal`).
    pub ssh_host: Option<Uuid>,
    /// Alerts (guest down, node offline, failed task, full storage) while Aurora runs.
    pub notify: bool,
    /// Customer: the group this server belongs to (its top-level group is the customer).
    pub group: Option<Uuid>,
}

impl Default for PveServer {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            host: String::new(),
            port: 8006,
            token: true,
            user: String::new(),
            tls_pin: String::new(),
            ssh_host: None,
            notify: true,
            group: None,
        }
    }
}

/// A Proxmox Backup Server (own list on the Proxmox page). The secret lives in the
/// keychain (`Keychain::pbs_secrets`).
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct PbsServer {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// API token (`user@realm!name`) instead of user + password.
    pub token: bool,
    pub user: String,
    /// SHA-256 fingerprint of the trusted certificate, empty = not yet.
    pub tls_pin: String,
    /// Customer: the group this server belongs to.
    pub group: Option<Uuid>,
}

impl Default for PbsServer {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            host: String::new(),
            port: 8007,
            token: true,
            user: String::new(),
            tls_pin: String::new(),
            group: None,
        }
    }
}

impl PbsServer {
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            self.host.trim()
        } else {
            self.name.trim()
        }
    }
}

impl PveServer {
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            self.host.trim()
        } else {
            self.name.trim()
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    /// Terminal font ("" = IBM Plex Mono), see `theme::term_fonts`.
    pub term_font: String,
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
    /// Record every SSH terminal session (asciinema files in the recordings folder).
    pub record_sessions: bool,
    /// Remember the commands run in SSH sessions per host (encrypted, `history.enc`).
    pub command_history: bool,
    /// Offer to save often repeated long commands as snippets.
    pub snippet_hints: bool,
    /// Hashes of the command patterns whose snippet suggestion was dismissed.
    pub snippet_hints_dismissed: Vec<String>,
    /// Check pasted text for secrets and multi-line pastes on PROD.
    pub paste_guard: bool,
    /// Keep the last 30 minutes of each terminal's output for rewinding.
    pub time_travel: bool,
    /// Reconnect SSH sessions automatically after an unexpected drop.
    pub auto_reconnect: bool,
    /// Give up after this many minutes without success.
    pub auto_reconnect_minutes: u32,
    /// Version whose "What's new" was last shown (empty = never).
    pub last_seen_version: String,
    /// Imported terminal colour schemes (shown as themes).
    pub custom_themes: Vec<crate::themeimport_parse::Scheme>,
    /// Warning thresholds of the server inventory page.
    pub inventory: crate::backend::inventory_rules::InventorySettings,
    /// Password-manager integration (Bitwarden, 1Password, KeePassXC, Proton Pass).
    pub pwm: crate::backend::pwmanager_types::PwmSettings,
    /// Command blocks in the terminal (gutter marks, toolbar, jump keys).
    pub command_blocks: bool,
    /// Double-click opens files in Aurora's editor (else the external program).
    pub files_editor_aurora: bool,
    /// Files copied at the same time by the transfer queue.
    pub transfer_slots: u32,
    /// Bandwidth limit of all transfers in KB/s (0 = unlimited).
    pub transfer_limit_kbps: u32,
    /// Compare checksums after each transferred file.
    pub transfer_verify: bool,
    /// Saved searches/filters/scopes of the connections list (sidebar).
    pub saved_views: Vec<crate::nav_model::SavedView>,
    /// How split panes are arranged (side by side, stacked, main + rest, grid).
    pub split_layout: crate::ui::chrome::SplitLayout,
    /// Dragged divider positions of the split view per layout and pane count.
    pub split_ratios: crate::ui::chrome::SplitRatios,
    /// Only install updates whose checksum file carries a valid release signature.
    pub signed_updates_only: bool,
    /// Open web interfaces in their own window instead of inside the tab.
    pub web_own_window: bool,
    /// Tiny CPU/RAM graphs in the host list rows (needs a background SSH connection per
    /// visible host, therefore off by default).
    #[serde(default)]
    pub host_sparklines: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            term_font: String::new(),
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
            record_sessions: false,
            command_history: true,
            snippet_hints: true,
            snippet_hints_dismissed: Vec::new(),
            paste_guard: true,
            time_travel: true,
            auto_reconnect: true,
            auto_reconnect_minutes: 10,
            last_seen_version: String::new(),
            custom_themes: Vec::new(),
            inventory: Default::default(),
            pwm: Default::default(),
            command_blocks: true,
            files_editor_aurora: true,
            transfer_slots: 2,
            transfer_limit_kbps: 0,
            split_layout: Default::default(),
            split_ratios: Default::default(),
            host_sparklines: false,
            transfer_verify: false,
            saved_views: Vec::new(),
            signed_updates_only: false,
            web_own_window: false,
        }
    }
}

/// A log source saved by the user for the log viewer.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct LogCommand {
    pub id: Uuid,
    pub name: String,
    /// A file (`/var/log/app.log`, followed with `tail -F`) or a command that keeps printing.
    pub command: String,
    /// Only offered for this host; `None` = every SSH host.
    pub host: Option<Uuid>,
}

impl Default for LogCommand {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            command: String::new(),
            host: None,
        }
    }
}

/// Built-in log sources offered next to the user's own.
pub const LOG_PRESETS: &[(&str, &str)] = &[
    ("Journal", "journalctl -f"),
    ("Journal (Warnungen)", "journalctl -f -p warning"),
    ("Syslog", "/var/log/syslog"),
    ("Messages", "/var/log/messages"),
    ("Auth", "/var/log/auth.log"),
    ("nginx error", "/var/log/nginx/error.log"),
    ("nginx access", "/var/log/nginx/access.log"),
    ("Apache error", "/var/log/apache2/error.log"),
];

/// Shell command for a persistent session: attach to (or create) a tmux session, fall back
/// to screen and then to a plain login shell. `slot` keeps parallel tabs to the same host in
/// separate sessions (slot 1 = `aurora-<id>`, then `aurora-<id>-2` …). Runs under `sh` so a
/// non-POSIX login shell (fish) works too.
pub fn persistent_shell(host: Uuid, slot: usize) -> String {
    let mut name = format!("aurora-{}", &host.simple().to_string()[..8]);
    if slot > 1 {
        name.push_str(&format!("-{slot}"));
    }
    format!(
        "sh -c 'if command -v tmux >/dev/null 2>&1; then exec tmux new-session -A -s {name}; \
         elif command -v screen >/dev/null 2>&1; then exec screen -D -RR {name}; \
         else exec \"${{SHELL:-/bin/sh}}\" -l; fi'"
    )
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
    /// The user's own log viewer sources.
    pub log_commands: Vec<LogCommand>,
    /// Proxmox VE servers (own page, not part of the host list).
    pub pve_servers: Vec<PveServer>,
    /// Proxmox Backup Servers (listed on the Proxmox page).
    pub pbs_servers: Vec<PbsServer>,
    /// `host:port` -> every server key accepted so far, oldest first.
    #[serde(default)]
    pub host_key_history: HashMap<String, Vec<HostKeyRecord>>,
    /// Port scan results and the certificate radar.
    #[serde(default)]
    pub net: NetData,
    /// Step-by-step procedures (Snippets page → Runbooks).
    #[serde(default)]
    pub runbooks: Vec<crate::backend::runbook::Runbook>,
    /// Active maintenance windows of hosts and groups.
    #[serde(default)]
    pub maintenance: Vec<crate::ui::maintenance::Maintenance>,
    /// Recent values of `{{variables}}` in log commands, newest first.
    #[serde(default)]
    pub log_var_history: HashMap<String, Vec<String>>,
    /// Container page: marked/hidden container hosts and remembered namespaces.
    #[serde(default)]
    pub ctr: crate::backend::containers_parse::CtrPrefs,
    /// Inventory detail data (services, ports, firewall, disks …) of each host.
    #[serde(default)]
    pub detail: HashMap<Uuid, crate::backend::inventory_detail::HostDetail>,
    /// Large-inventory navigation: group badges/pins, scope, view mode, table columns.
    #[serde(default)]
    pub nav: crate::nav_model::NavData,
}

/// Network data Aurora remembers between runs.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct NetData {
    /// Open ports of each host from its last port scan.
    pub open_ports: HashMap<Uuid, Vec<u16>>,
    /// Certificates found by the last radar run.
    pub certs: Vec<crate::backend::certradar::CertInfo>,
    pub certs_checked: Option<DateTime<Utc>>,
    /// No daily background check.
    pub radar_paused: bool,
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
    /// Kernel release (`uname -r`).
    pub kernel: String,
    /// Pending updates that come from a security repository, if known.
    pub security_updates: Option<u32>,
    pub cpus: Option<u32>,
    pub mem_total_mb: Option<u64>,
    pub mem_used_pct: Option<u8>,
    /// Load average over one minute.
    pub load1: Option<f32>,
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
                "kernel" => f.kernel = clean(v),
                "supd" => f.security_updates = v.trim().parse().ok(),
                "cpus" => f.cpus = v.trim().parse().ok().filter(|n| *n > 0),
                "mem" => f.mem_total_mb = v.trim().parse().ok().filter(|n| *n > 0),
                "memused" => f.mem_used_pct = v.trim().parse::<u8>().ok().filter(|n| *n <= 100),
                "load" => f.load1 = v.trim().parse().ok(),
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
        return Some(t!("löscht rekursiv"));
    }
    match first {
        "shutdown" | "poweroff" | "halt" => return Some(t!("fährt den Server herunter")),
        "reboot" => return Some(t!("startet den Server neu")),
        "dd" => return Some(t!("schreibt direkt auf ein Gerät")),
        "init" if has("0") || has("6") => return Some(t!("ändert den Runlevel")),
        "kill" | "killall" | "pkill" if has("-9") || has("-kill") => {
            return Some(t!("beendet Prozesse hart"));
        }
        "truncate" => return Some(t!("leert Dateien")),
        "chmod" | "chown" if has("-r") || has("--recursive") => {
            return Some(t!("ändert Rechte rekursiv"));
        }
        _ => {}
    }
    if first.starts_with("mkfs") {
        return Some(t!("formatiert ein Dateisystem"));
    }
    if first == "systemctl" && (has("stop") || has("restart") || has("poweroff") || has("reboot")) {
        return Some(t!("stoppt oder startet Dienste"));
    }
    if l.contains("drop database") || l.contains("drop table") || l.contains("truncate table") {
        return Some(t!("löscht Datenbankinhalte"));
    }
    if l.contains(":(){") || l.contains("> /dev/sd") || l.contains(">/dev/sd") {
        return Some(t!("überschreibt Systemdaten"));
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

/// Per-host settings for the login with a short-lived SSH certificate.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct CertSettings {
    pub enabled: bool,
    /// Validity in hours (1, 8 or 24).
    pub hours: u32,
    /// Comma-separated principals; empty = the login user.
    pub principals: String,
}

impl Default for CertSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hours: 8,
            principals: String::new(),
        }
    }
}

/// The certificate authority key pair (lives in the encrypted keychain).
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SshCa {
    /// OpenSSH private key (Ed25519).
    pub private_key: String,
    /// `ssh-ed25519 AAAA… comment`
    pub public_key: String,
    pub created: Option<DateTime<Utc>>,
}

/// A server key that was accepted for a host.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HostKeyRecord {
    /// `SHA256:…`
    pub fingerprint: String,
    pub algorithm: String,
    /// When it was trusted (unknown for keys from before the history existed).
    pub trusted: Option<DateTime<Utc>>,
    /// When another key replaced it; `None` = current key.
    pub replaced: Option<DateTime<Utc>>,
}

impl Library {
    /// Records that `fingerprint` is now the trusted key of `host_id` (`host:port`);
    /// the key it replaces (if any) is kept with today's date.
    pub fn remember_host_key(&mut self, host_id: &str, fingerprint: &str, algorithm: &str) {
        let now = Utc::now();
        let previous = self.known_hosts.get(host_id).cloned();
        let list = self
            .host_key_history
            .entry(host_id.to_string())
            .or_default();
        if let Some(prev) = previous.filter(|p| p != fingerprint) {
            match list.iter_mut().rev().find(|r| r.fingerprint == prev) {
                Some(r) => {
                    r.replaced.get_or_insert(now);
                }
                None => list.push(HostKeyRecord {
                    fingerprint: prev,
                    algorithm: String::new(),
                    trusted: None,
                    replaced: Some(now),
                }),
            }
        }
        if !list
            .iter()
            .any(|r| r.fingerprint == fingerprint && r.replaced.is_none())
        {
            list.push(HostKeyRecord {
                fingerprint: fingerprint.to_string(),
                algorithm: algorithm.to_string(),
                trusted: Some(now),
                replaced: None,
            });
        }
        self.known_hosts
            .insert(host_id.to_string(), fingerprint.to_string());
    }
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
    fn old_files_load_without_proxmox() {
        let lib: Library = serde_json::from_str(r#"{"hosts":[],"groups":[]}"#).unwrap();
        assert!(lib.pve_servers.is_empty());
        let kc: Keychain = serde_json::from_str(r#"{"identities":[]}"#).unwrap();
        assert!(kc.pve_secrets.is_empty());
        let s: PveServer = serde_json::from_str(r#"{"host":"pve"}"#).unwrap();
        assert_eq!((s.port, s.token, s.display_name()), (8006, true, "pve"));
        // Servers saved before customers existed load without one.
        assert_eq!(s.group, None);
        let b: PbsServer = serde_json::from_str(r#"{"host":"pbs"}"#).unwrap();
        assert_eq!(b.group, None);
        let g = Uuid::new_v4();
        let s2: PveServer =
            serde_json::from_str(&format!(r#"{{"host":"pve","group":"{g}"}}"#)).unwrap();
        assert_eq!(s2.group, Some(g));
        assert!(
            s.notify,
            "alerts are on for servers saved before the switch existed"
        );
        assert!(lib.pbs_servers.is_empty() && kc.pbs_secrets.is_empty());
        let b: PbsServer = serde_json::from_str(r#"{"host":"pbs"}"#).unwrap();
        assert_eq!((b.port, b.display_name()), (8007, "pbs"));
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
    fn snippet_shell_kind_defaults_for_old_data() {
        let old: Snippet = serde_json::from_str(r#"{"name":"x","script":"ls"}"#).unwrap();
        assert_eq!(old.shell, ShellKind::Shell);
        let ps = Snippet {
            shell: ShellKind::PowerShell,
            ..Default::default()
        };
        let j = serde_json::to_string(&ps).unwrap();
        assert!(j.contains(r#""shell":"powershell""#));
        let back: Snippet = serde_json::from_str(&j).unwrap();
        assert_eq!(back.shell, ShellKind::PowerShell);
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

    #[test]
    fn host_key_history_keeps_old_keys() {
        let mut lib = Library::default();
        lib.known_hosts.insert("a:22".into(), "SHA256:old".into());
        lib.remember_host_key("a:22", "SHA256:new", "ssh-ed25519");
        let h = &lib.host_key_history["a:22"];
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].fingerprint, "SHA256:old");
        assert!(h[0].replaced.is_some());
        assert_eq!(h[1].fingerprint, "SHA256:new");
        assert!(h[1].replaced.is_none());
        assert_eq!(lib.known_hosts["a:22"], "SHA256:new");
        // Accepting the same key again changes nothing.
        lib.remember_host_key("a:22", "SHA256:new", "ssh-ed25519");
        assert_eq!(lib.host_key_history["a:22"].len(), 2);
    }
}
