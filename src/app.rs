//! Application state, session management and the top-level layout.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Vec2};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::backend::rdp::{RdpLaunch, RdpProcess};
use crate::backend::sftp::{Entry, SftpCmd, SftpEvent};
use crate::backend::ssh::SshParams;
use crate::backend::{self, HostKeyPrompt, Sink, TermCmd, TermEvent};
use crate::model::{
    Env, Group, Host, Identity, Keychain, Library, Protocol, SerialSettings, Snippet,
};
use crate::store::{Store, Vault};
use crate::terminal::{TermView, Terminal};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Hosts,
    Files,
    Keychain,
    Snippets,
    Settings,
}

#[derive(Clone)]
pub enum ConnectSpec {
    Ssh(SshParams),
    Serial(SerialSettings),
}

pub enum ConnState {
    Connecting(String),
    Connected,
    Closed(Option<String>),
}

pub struct TermTab {
    pub spec: ConnectSpec,
    pub term: Terminal,
    pub tx: UnboundedSender<TermCmd>,
    pub rx: UnboundedReceiver<TermEvent>,
    pub state: ConnState,
    pub startup: Option<String>,
    pub show_snippets: bool,
    pub snippet_filter: String,
    /// Enter was pressed at this time; cleared once the prompt is back.
    pub cmd_started: Option<Instant>,
    pub last_output: Instant,
    /// File upload from a drop onto the terminal: (file, done, total).
    pub upload: Option<(String, u64, u64)>,
    /// Command typed once after the first login (not on reconnect).
    pub run_once: Option<String>,
}

pub struct Transfer {
    pub id: u64,
    pub name: String,
    /// File currently being copied (inside a folder).
    pub current: String,
    /// "Lokal → web-01"
    pub route: String,
    /// Pane index the data goes to; refreshed when done.
    pub dst: usize,
    pub upload: bool,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

/// A live SFTP connection behind one file pane.
pub struct RemoteConn {
    pub host_id: Option<Uuid>,
    pub label: String,
    pub params: SshParams,
    pub tx: UnboundedSender<SftpCmd>,
    pub rx: UnboundedReceiver<SftpEvent>,
    pub sink: Sink<SftpEvent>,
    pub state: ConnState,
    pub session: Option<crate::backend::sftp::Session>,
}

/// One side of the file browser: the local disk (`remote == None`) or a server.
#[derive(Default)]
pub struct FilePane {
    pub remote: Option<RemoteConn>,
    /// Local: OS path ("" = drive list on Windows). Remote: SFTP path ("" until connected).
    pub path: String,
    pub edit: String,
    pub entries: Vec<Entry>,
    pub selected: Option<String>,
    pub error: Option<String>,
}

pub enum FsDialog {
    Mkdir { name: String },
    Rename { from: String, to: String },
    Delete { name: String, is_dir: bool },
}

pub struct SftpTab {
    pub panes: [FilePane; 2],
    pub transfers: Vec<Transfer>,
    pub dialog: Option<(usize, FsDialog)>,
    /// Remote files opened in a local editor; saved changes are uploaded back.
    pub edits: Vec<EditFile>,
}

pub struct EditFile {
    pub pane: usize,
    pub remote_path: String,
    pub local: PathBuf,
    /// Download transfer; the editor opens once it is done.
    pub transfer: u64,
    pub ready: bool,
    pub opened: bool,
    pub mtime: Option<std::time::SystemTime>,
    pub last_check: Instant,
    pub uploads: u32,
}

impl SftpTab {
    /// Worst state of the connected panes (local-only counts as connected).
    pub fn state(&self) -> &ConnState {
        static OK: ConnState = ConnState::Connected;
        let states = || {
            self.panes
                .iter()
                .filter_map(|p| p.remote.as_ref())
                .map(|r| &r.state)
        };
        states()
            .find(|s| matches!(s, ConnState::Closed(_)))
            .or_else(|| states().find(|s| matches!(s, ConnState::Connecting(_))))
            .unwrap_or(&OK)
    }
}

pub struct RdpTab {
    pub launch: RdpLaunch,
    /// Session rendered inside Aurora; `None` = handed off to the system client.
    pub embedded: Option<Box<crate::ui::rdp::RdpView>>,
    pub process: Option<RdpProcess>,
    pub started: Instant,
    pub status: String,
    pub running: bool,
}

impl RdpTab {
    pub fn is_live(&self) -> bool {
        match &self.embedded {
            Some(v) => matches!(v.state, ConnState::Connected),
            None => self.running,
        }
    }

    pub fn status_color(&self) -> Color32 {
        match &self.embedded {
            Some(v) => state_color(&v.state),
            None if self.running => theme::green(),
            None => theme::text_dim(),
        }
    }

    fn launch_external(&mut self) {
        self.embedded = None;
        match backend::rdp::launch(&self.launch) {
            Ok(p) => {
                self.status = format!("Läuft in {}", p.client);
                self.running = p.child.is_some();
                self.process = Some(p);
                self.started = Instant::now();
            }
            Err(e) => {
                self.status = format!("{e:#}");
                self.running = false;
            }
        }
    }
}

pub enum TabKind {
    Term(Box<TermTab>),
    Sftp(Box<SftpTab>),
    Rdp(Box<RdpTab>),
}

pub struct Tab {
    pub id: u64,
    pub title: String,
    /// `user@host:port` or serial port, shown under the title.
    pub subtitle: String,
    pub color: Color32,
    pub kind: TabKind,
    /// Something happened while the session was out of sight (command done, bell).
    pub attention: bool,
    pub env: Env,
    /// Host notes, shown as a banner when the session opens.
    pub notes: String,
    pub notes_open: bool,
    /// Quick-connect target that isn't saved as a host yet.
    pub unsaved: Option<Host>,
    /// Saved host behind this session.
    pub host_id: Option<Uuid>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    Terminal,
    /// Connect the given pane of a file tab.
    Pane(u64, usize),
    /// Start the port forwarding with this id.
    Tunnel(Uuid),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TunnelState {
    Connecting,
    Up,
    Failed(String),
}

/// A running (or failed) port forwarding.
pub struct TunnelRun {
    pub host: Uuid,
    pub state: TunnelState,
    pub conns: i32,
    pub stop: Option<UnboundedSender<()>>,
}

pub struct PasswordPrompt {
    pub host: Host,
    pub purpose: Purpose,
    pub password: String,
    pub save: bool,
}

pub struct SnippetRun {
    pub snippet: Snippet,
    pub values: Vec<(String, String)>,
    pub all: bool,
}

/// "SSH-Key einrichten" dialog.
pub struct KeySetup {
    pub host: Uuid,
    /// Existing identity with a private key, or `None` = generate a new Ed25519 key.
    pub identity: Option<Uuid>,
    pub password: String,
    pub running: bool,
    pub error: Option<String>,
    /// Key generated for this run: (private, public).
    pub generated: Option<(String, String)>,
}

/// "Netz durchsuchen" dialog.
pub struct ScanState {
    pub range: String,
    pub ports: String,
    pub running: bool,
    pub progress: (usize, usize),
    /// Found machines and whether they are ticked for import.
    pub results: Vec<(crate::backend::scan::Found, bool)>,
    pub rx: Option<UnboundedReceiver<crate::backend::scan::ScanEvent>>,
    pub error: Option<String>,
    pub new_group: bool,
}

pub struct HostEditor {
    pub host: Host,
    pub is_new: bool,
    pub tags_text: String,
}

impl HostEditor {
    pub fn new(host: Host, is_new: bool) -> Self {
        Self {
            tags_text: host.tags.join(", "),
            host,
            is_new,
        }
    }
}

#[derive(Default)]
pub struct LockScreen {
    pub password: String,
    pub confirm: String,
    pub error: Option<String>,
}

pub struct Toast {
    pub text: String,
    pub color: Color32,
    pub at: Instant,
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

#[derive(Default)]
pub struct PwChange {
    pub new: String,
    pub confirm: String,
}

#[derive(Default)]
pub struct UpdateState {
    pub latest: Option<crate::update::Release>,
    pub checking: bool,
    /// Last check was started by the user (report "up to date" / errors).
    pub manual: bool,
    pub installing: bool,
    pub progress: f32,
    /// Installed in place; a restart activates it.
    pub installed: bool,
    pub message: Option<(String, Color32)>,
}

pub struct App {
    pub rt: tokio::runtime::Runtime,
    pub store: Store,
    pub data_dir: PathBuf,
    pub lib: Library,
    pub vault: Option<Vault>,
    pub keychain: Keychain,
    pub lock: LockScreen,

    pub view: View,
    pub tabs: Vec<Tab>,
    pub active: Option<u64>,
    /// Sessions shown side by side (2–4) while one of them is active.
    pub split: Vec<u64>,
    /// File pages: sources picked for a new file tab.
    pub files_left: Option<Uuid>,
    pub files_right: Option<Uuid>,
    /// Pane connections waiting for the password dialog to be free.
    /// Connections waiting for the password dialog to be free.
    pub connect_queue: VecDeque<(Purpose, Host)>,
    /// Hosts of a group being opened together; their sessions go into the split.
    pub group_split: Vec<Uuid>,
    /// Enter on a dangerous command waits here for confirmation: (tab, command, why).
    pub danger: Option<(u64, String, &'static str)>,
    pub last_activity: Instant,
    pub show_shortcuts: bool,
    pub tour_step: usize,
    /// Terminal that gets the keyboard focus on the next frame.
    pub focus_term: Option<u64>,
    /// Hosts whose last connection attempt failed (red LED).
    pub failed: std::collections::HashSet<Uuid>,
    /// Command for the next session of this host (e.g. "install updates").
    pub pending_command: Option<(Uuid, String)>,
    pub key_setup: Option<KeySetup>,
    pub key_sink: Sink<Result<(), String>>,
    pub key_rx: UnboundedReceiver<Result<(), String>>,
    pub scan: Option<ScanState>,
    /// Sessions of the last run were reopened (or skipped) already.
    pub restored: bool,
    /// Multi-selection in the host list.
    pub selected_hosts: std::collections::HashSet<Uuid>,
    /// Settings → Export/Import form.
    pub export_path: String,
    pub export_pw: String,
    pub select_anchor: Option<Uuid>,
    pub tunnels: std::collections::HashMap<Uuid, TunnelRun>,
    pub tunnel_sink: Sink<backend::tunnel::TunnelEvent>,
    pub tunnel_rx: UnboundedReceiver<backend::tunnel::TunnelEvent>,
    pub next_id: u64,

    pub search: String,
    /// Hosts view filter: 0 all, 1 SSH, 2 RDP, 3 Serial, 4 favorites.
    pub host_filter: usize,
    pub snippet_search: String,
    pub keychain_search: String,
    pub broadcast: bool,
    /// Typing goes to every terminal shown in the split.
    pub split_broadcast: bool,
    /// Split pane temporarily shown alone.
    pub maximized: Option<u64>,
    pub palette: Palette,
    pub pw_change: PwChange,

    pub host_editor: Option<HostEditor>,
    pub group_editor: Option<(Group, bool)>,
    pub identity_editor: Option<(Identity, bool, String)>,
    pub snippet_editor: Option<(Snippet, bool, String)>,
    pub confirm_delete: Option<(String, Action)>,

    pub prompts_sink: Sink<HostKeyPrompt>,
    pub prompts_rx: UnboundedReceiver<HostKeyPrompt>,
    pub hostkey_queue: VecDeque<HostKeyPrompt>,
    pub password_prompt: Option<PasswordPrompt>,
    pub snippet_run: Option<SnippetRun>,
    pub toasts: Vec<Toast>,
    pub serial_ports: Vec<String>,
    pub update: UpdateState,
    pub reach: crate::backend::reach::Reachability,
    pub update_sink: Sink<crate::update::UpdateEvent>,
    pub update_rx: UnboundedReceiver<crate::update::UpdateEvent>,
}

/// UI intents collected while drawing and applied afterwards.
#[derive(Clone, Debug)]
pub enum Action {
    Connect(Uuid),
    OpenSftp(Uuid),
    /// New file tab; `None` = local disk.
    OpenFiles(Option<Uuid>, Option<Uuid>),
    /// Switch a pane of a file tab to another source.
    SetPaneSource(u64, usize, Option<Uuid>),
    QuickConnect(String),
    NewHost(Option<Uuid>),
    NewHostOf(Protocol, Option<Uuid>),
    EditHost(Uuid),
    DuplicateHost(Uuid),
    DeleteHost(Uuid),
    ToggleFavorite(Uuid),
    MoveHost(Uuid, Option<Uuid>),
    NewGroup(Option<Uuid>),
    EditGroup(Uuid),
    DeleteGroup(Uuid),
    ToggleGroup(Uuid),
    NewIdentity,
    EditIdentity(Uuid),
    DeleteIdentity(Uuid),
    NewSnippet,
    EditSnippet(Uuid),
    DeleteSnippet(Uuid),
    RunSnippet(Uuid, bool),
    ForgetKnownHost(String),
    Navigate(View),
    OpenPalette,
    Lock,
    SetTheme(String),
    Reconnect(u64),
    /// Open every host of a group, side by side.
    OpenGroup(Uuid),
    /// Snippet editor prefilled with this text.
    SnippetFromText(String),
    /// Host editor for a quick-connect session.
    SaveQuickHost(u64),
    /// New host with this hostname (from a search without results).
    NewHostNamed(String),
    ImportSshConfig,
    RenameHost(Uuid, String),
    /// Put all loose hosts of `a.b.c.x` into a new group.
    GroupSubnet(String),
    DismissSubnet(String),
    OpenScan,
    SetupKey(Uuid),
    /// Connect and run the package manager's upgrade.
    RunUpdates(Uuid),
    /// Start or stop a port forwarding: (host, tunnel).
    ToggleTunnel(Uuid, Uuid),
    /// Host list selection: 0 = only this, 1 = toggle (Ctrl), 2 = range (Shift).
    SelectHost(Uuid, u8),
    ClearSelection,
    MoveHosts(Vec<Uuid>, Option<Uuid>),
    /// Connect several hosts; up to four end up side by side.
    ConnectMany(Vec<Uuid>),
    SetEnvMany(Vec<Uuid>, Env),
    DeleteHosts(Vec<Uuid>),
    /// Ask before running the inner (destructive) action.
    Confirm(String, Box<Action>),
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let data_dir = crate::store::data_dir();
        let store = Store::new(data_dir.clone());
        let lib = match store.load_library() {
            Ok(Some(lib)) => lib,
            Ok(None) => Library::default(),
            Err(e) => {
                log::error!("library konnte nicht geladen werden: {e:#}");
                Library::default()
            }
        };
        crate::i18n::set_language(&lib.settings.language);
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &lib.settings.theme);
        let (prompts_sink, prompts_rx) = Sink::new(cc.egui_ctx.clone());
        let (update_sink, update_rx) = Sink::new(cc.egui_ctx.clone());
        let (key_sink, key_rx) = Sink::new(cc.egui_ctx.clone());
        let (tunnel_sink, tunnel_rx) = Sink::new(cc.egui_ctx.clone());
        let mut update = UpdateState::default();
        if lib.settings.check_updates {
            update.checking = true;
            crate::update::check(update_sink.clone());
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self {
            rt,
            store,
            data_dir,
            lib,
            vault: None,
            keychain: Keychain::default(),
            lock: LockScreen::default(),
            view: View::Hosts,
            tabs: Vec::new(),
            active: None,
            split: Vec::new(),
            files_left: None,
            files_right: None,
            connect_queue: VecDeque::new(),
            group_split: Vec::new(),
            danger: None,
            last_activity: Instant::now(),
            show_shortcuts: false,
            tour_step: 0,
            focus_term: None,
            failed: Default::default(),
            pending_command: None,
            key_setup: None,
            key_sink,
            key_rx,
            scan: None,
            restored: false,
            selected_hosts: Default::default(),
            export_path: dirs_home()
                .join("aurora-export.aurora")
                .to_string_lossy()
                .into_owned(),
            export_pw: String::new(),
            select_anchor: None,
            tunnels: Default::default(),
            tunnel_sink,
            tunnel_rx,
            next_id: 1,
            search: String::new(),
            host_filter: 0,
            snippet_search: String::new(),
            keychain_search: String::new(),
            broadcast: false,
            split_broadcast: false,
            maximized: None,
            palette: Palette::default(),
            pw_change: PwChange::default(),
            host_editor: None,
            group_editor: None,
            identity_editor: None,
            snippet_editor: None,
            confirm_delete: None,
            prompts_sink,
            prompts_rx,
            hostkey_queue: VecDeque::new(),
            password_prompt: None,
            snippet_run: None,
            toasts: Vec::new(),
            serial_ports: backend::serial::available_ports(),
            update,
            reach: Default::default(),
            update_sink,
            update_rx,
        }
    }

    pub fn accent(&self) -> Color32 {
        theme::accent()
    }

    pub fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast {
            text: text.into(),
            color,
            at: Instant::now(),
        });
    }

    pub fn save_library(&mut self) {
        if let Err(e) = self.store.save_library(&self.lib) {
            self.toast(format!("Speichern fehlgeschlagen: {e:#}"), theme::red());
        }
    }

    pub fn save_keychain(&mut self) {
        if let Some(v) = &self.vault
            && let Err(e) = v.save(&self.keychain)
        {
            self.toast(
                format!("Keychain-Speichern fehlgeschlagen: {e:#}"),
                theme::red(),
            );
        }
    }

    pub fn identity(&self, id: Uuid) -> Option<&Identity> {
        self.keychain.identities.iter().find(|i| i.id == id)
    }

    /// Effective username and identity for a host (host > identity > group defaults).
    pub fn credentials(&self, host: &Host) -> (String, Option<Identity>) {
        let (group_user, group_ident) = self.lib.inherited_defaults(host);
        let ident = host
            .identity_id
            .or(group_ident)
            .and_then(|id| self.identity(id))
            .cloned();
        let user = if !host.username.is_empty() {
            host.username.clone()
        } else if let Some(i) = ident.as_ref().filter(|i| !i.username.is_empty()) {
            i.username.clone()
        } else if !group_user.is_empty() {
            group_user
        } else if host.protocol == Protocol::Ssh {
            "root".into()
        } else {
            String::new()
        };
        (user, ident)
    }

    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn host_color(&self, host: &Host) -> Color32 {
        if let Some(c) = host.color {
            return theme::rgb(c);
        }
        if let Some(g) = host.group_id.and_then(|g| self.lib.group(g)) {
            return theme::rgb(g.color);
        }
        theme::rgb(theme::hash_color(&host.hostname))
    }

    // ------------------------------------------------------------------ connecting

    pub fn connect_host(&mut self, host: Host, purpose: Purpose, ctx: &egui::Context) {
        let (user, ident) = self.credentials(&host);
        let has_secret = ident.as_ref().is_some_and(|i| {
            !i.password.is_empty() || (host.protocol == Protocol::Ssh && !i.private_key.is_empty())
        });
        let needs_secret = match host.protocol {
            Protocol::Ssh => true,
            Protocol::Rdp => host.rdp.embedded,
            Protocol::Serial => false,
        };
        if needs_secret && !has_secret {
            if self.password_prompt.is_some() {
                self.connect_queue.push_back((purpose, host));
                return;
            }
            self.password_prompt = Some(PasswordPrompt {
                host,
                purpose,
                password: String::new(),
                save: false,
            });
            return;
        }
        let password = ident
            .as_ref()
            .map(|i| i.password.clone())
            .filter(|p| !p.is_empty());
        self.open_session(&host, purpose, user, password, ident, ctx);
    }

    pub fn open_session(
        &mut self,
        host: &Host,
        purpose: Purpose,
        user: String,
        password: Option<String>,
        ident: Option<Identity>,
        ctx: &egui::Context,
    ) {
        if let Some(h) = self.lib.hosts.iter_mut().find(|h| h.id == host.id) {
            h.last_connected = Some(chrono::Utc::now());
        }
        self.save_library();
        let color = self.host_color(host);
        let id = self.alloc_id();
        let ssh_params = || self.ssh_params(host, &user, &password, &ident);
        if let Purpose::Tunnel(tid) = purpose {
            let tunnel = host.tunnels.iter().find(|t| t.id == tid).cloned();
            if let Some(t) = tunnel {
                let stop = backend::tunnel::spawn(
                    &self.rt,
                    ssh_params(),
                    t,
                    self.tunnel_sink.clone(),
                    self.prompts_sink.clone(),
                );
                if let Some(run) = self.tunnels.get_mut(&tid) {
                    run.stop = Some(stop);
                }
            }
            return;
        }
        if let Purpose::Pane(tab_id, idx) = purpose {
            let conn = crate::ui::sftp::spawn_conn(
                &self.rt,
                ssh_params(),
                host.display_name().to_string(),
                Some(host.id),
                ctx,
                self.prompts_sink.clone(),
            );
            self.set_pane(tab_id, idx, Some(conn), color);
            return;
        }
        let kind = match (host.protocol, purpose) {
            (Protocol::Ssh, _) => {
                let spec = ConnectSpec::Ssh(ssh_params());
                let startup = host
                    .startup_snippet
                    .and_then(|s| self.lib.snippet(s))
                    .filter(|s| s.variables().is_empty())
                    .map(|s| s.script.clone());
                let mut term = self.spawn_term(spec, startup, ctx);
                if let Some((h, cmd)) = self.pending_command.take() {
                    if h == host.id {
                        term.run_once = Some(cmd);
                    } else {
                        self.pending_command = Some((h, cmd));
                    }
                }
                TabKind::Term(Box::new(term))
            }
            (Protocol::Serial, _) => {
                let spec = ConnectSpec::Serial(host.serial.clone());
                TabKind::Term(Box::new(self.spawn_term(spec, None, ctx)))
            }
            (Protocol::Rdp, _) => {
                let launch = RdpLaunch {
                    host: host.hostname.trim().to_string(),
                    port: host.port,
                    username: user.clone(),
                    password: password.clone(),
                    settings: host.rdp.clone(),
                };
                let mut tab = RdpTab {
                    launch,
                    embedded: None,
                    process: None,
                    started: Instant::now(),
                    status: String::new(),
                    running: false,
                };
                if host.rdp.embedded && backend::rdp_embed::helper_path().is_some() {
                    let known = self
                        .lib
                        .known_hosts
                        .get(&format!("rdp://{}:{}", host.hostname.trim(), host.port))
                        .cloned();
                    tab.embedded = Some(Box::new(crate::ui::rdp::RdpView::new(
                        known,
                        host.rdp.clipboard,
                    )));
                } else {
                    tab.launch_external();
                }
                TabKind::Rdp(Box::new(tab))
            }
        };
        let subtitle = match host.protocol {
            Protocol::Serial => host.subtitle(),
            _ => {
                let user = if user.is_empty() {
                    String::new()
                } else {
                    format!("{user}@")
                };
                format!("{user}{}:{}", host.hostname.trim(), host.port)
            }
        };
        self.tabs.push(Tab {
            id,
            title: host.display_name().to_string(),
            subtitle,
            color,
            kind,
            attention: false,
            env: self.lib.env_of(host),
            notes: host.notes.trim().to_string(),
            notes_open: !host.notes.trim().is_empty(),
            unsaved: (self.lib.host(host.id).is_none() && host.protocol == Protocol::Ssh)
                .then(|| host.clone()),
            host_id: self.lib.host(host.id).map(|h| h.id),
        });
        if self.group_split.contains(&host.id) {
            self.group_split.retain(|h| *h != host.id);
            if self.split.len() < crate::ui::chrome::MAX_SPLIT {
                self.split.push(id);
            }
            self.active = self.split.first().copied().or(Some(id));
            return;
        }
        self.active = Some(id);
    }

    fn ssh_params(
        &self,
        host: &Host,
        user: &str,
        password: &Option<String>,
        ident: &Option<Identity>,
    ) -> SshParams {
        SshParams {
            host: host.hostname.trim().to_string(),
            port: host.port,
            username: user.to_string(),
            password: password.clone(),
            private_key: ident
                .as_ref()
                .map(|i| i.private_key.clone())
                .filter(|k| !k.is_empty()),
            passphrase: ident
                .as_ref()
                .map(|i| i.passphrase.clone())
                .filter(|k| !k.is_empty()),
            known_fingerprint: self
                .lib
                .known_hosts
                .get(&format!("{}:{}", host.hostname.trim(), host.port))
                .cloned(),
            jump: self.jump_params(host),
        }
    }

    /// ProxyJump: the jump host's own connection parameters (stored credentials only;
    /// one level, no chains).
    fn jump_params(&self, host: &Host) -> Option<Box<SshParams>> {
        let jump = host
            .jump_host
            .filter(|j| *j != host.id)
            .and_then(|j| self.lib.host(j))?
            .clone();
        let (user, ident) = self.credentials(&jump);
        let password = ident
            .as_ref()
            .map(|i| i.password.clone())
            .filter(|p| !p.is_empty());
        let mut p = self.ssh_params(
            &Host {
                jump_host: None,
                ..jump
            },
            &user,
            &password,
            &ident,
        );
        p.jump = None;
        Some(Box::new(p))
    }

    /// New file tab with two panes; `None` means the local disk.
    pub fn open_files(&mut self, left: Option<Uuid>, right: Option<Uuid>, ctx: &egui::Context) {
        let id = self.alloc_id();
        let home = crate::ui::sftp::home_dir();
        self.tabs.push(Tab {
            id,
            title: "Lokal".into(),
            subtitle: "sftp".into(),
            color: theme::accent(),
            attention: false,
            env: Env::None,
            notes: String::new(),
            notes_open: false,
            unsaved: None,
            host_id: None,
            kind: TabKind::Sftp(Box::new(SftpTab {
                edits: Vec::new(),
                panes: [
                    crate::ui::sftp::local_pane(home.clone()),
                    crate::ui::sftp::local_pane(home),
                ],
                transfers: Vec::new(),
                dialog: None,
            })),
        });
        self.active = Some(id);
        for (idx, host) in [(0, left), (1, right)] {
            if let Some(h) = host.and_then(|h| self.lib.host(h).cloned()) {
                self.connect_host(crate::ui::sftp::sftp_target(h), Purpose::Pane(id, idx), ctx);
            }
        }
    }

    /// Replaces the source of one pane (closing its old connection) and retitles the tab.
    pub fn set_pane(&mut self, tab_id: u64, idx: usize, conn: Option<RemoteConn>, color: Color32) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            if let Some(c) = conn {
                let _ = c.tx.send(SftpCmd::Close);
            }
            return;
        };
        let TabKind::Sftp(t) = &mut tab.kind else {
            return;
        };
        if let Some(old) = t.panes[idx].remote.take() {
            let _ = old.tx.send(SftpCmd::Close);
        }
        t.panes[idx] = match conn {
            Some(c) => FilePane {
                remote: Some(c),
                ..Default::default()
            },
            None => crate::ui::sftp::local_pane(crate::ui::sftp::home_dir()),
        };
        if t.panes[idx].remote.is_some() {
            tab.color = color;
        }
        let labels: Vec<String> = t
            .panes
            .iter()
            .filter_map(|p| p.remote.as_ref().map(|r| r.label.clone()))
            .collect();
        tab.title = if labels.is_empty() {
            "Lokal".into()
        } else {
            labels.join(" ⇄ ")
        };
    }

    fn spawn_term(
        &self,
        spec: ConnectSpec,
        startup: Option<String>,
        ctx: &egui::Context,
    ) -> TermTab {
        let (sink, rx) = Sink::new(ctx.clone());
        let tx = match &spec {
            ConnectSpec::Ssh(p) => backend::ssh::spawn_shell(
                &self.rt,
                p.clone(),
                80,
                24,
                sink,
                self.prompts_sink.clone(),
            ),
            ConnectSpec::Serial(s) => backend::serial::spawn(s.clone(), sink),
        };
        TermTab {
            spec,
            term: Terminal::new(self.lib.settings.scrollback),
            tx,
            rx,
            state: ConnState::Connecting("Verbinde…".into()),
            startup,
            show_snippets: false,
            snippet_filter: String::new(),
            cmd_started: None,
            last_output: Instant::now(),
            upload: None,
            run_once: None,
        }
    }

    pub fn reconnect(&mut self, tab_id: u64, ctx: &egui::Context) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return;
        };
        if let TabKind::Term(t) = &self.tabs[idx].kind {
            let mut fresh = self.spawn_term(t.spec.clone(), t.startup.clone(), ctx);
            fresh.show_snippets = t.show_snippets;
            fresh.term.feed(b"\r\n\x1b[2m--- reconnect ---\x1b[0m\r\n");
            self.tabs[idx].kind = TabKind::Term(Box::new(fresh));
        } else if let TabKind::Rdp(t) = &mut self.tabs[idx].kind {
            if let Some(v) = t.embedded.as_mut() {
                v.reconnect();
            }
        } else if let TabKind::Sftp(t) = &mut self.tabs[idx].kind {
            for pane in t.panes.iter_mut() {
                if let Some(r) = pane.remote.as_mut()
                    && matches!(r.state, ConnState::Closed(_))
                {
                    *r = crate::ui::sftp::spawn_conn(
                        &self.rt,
                        r.params.clone(),
                        r.label.clone(),
                        r.host_id,
                        ctx,
                        self.prompts_sink.clone(),
                    );
                }
            }
        }
    }

    fn apply_split(&mut self, op: crate::ui::chrome::SplitOp) {
        use crate::ui::chrome::{MAX_SPLIT, SplitOp};
        match op {
            SplitOp::Add(base, other) => {
                if !(self.split.len() >= 2 && self.split.contains(&base)) {
                    self.split = vec![base];
                }
                if !self.split.contains(&other) && self.split.len() < MAX_SPLIT {
                    self.split.push(other);
                }
                self.active = Some(base);
            }
            SplitOp::Remove(id) => {
                self.split.retain(|s| *s != id);
                if self.active == Some(id) {
                    self.active = self.split.first().copied().or(Some(id));
                }
            }
            SplitOp::Clear => self.split.clear(),
            SplitOp::Close(id) => self.close_tab(id),
            SplitOp::Maximize(id) => {
                self.maximized = if self.maximized == Some(id) {
                    None
                } else {
                    Some(id)
                };
                self.active = Some(id);
                return;
            }
            SplitOp::Broadcast => {
                self.split_broadcast = !self.split_broadcast;
                return;
            }
        }
        self.maximized = None;
        if self.split.len() < 2 {
            self.split.clear();
        }
    }

    /// Starts the key installation for the open "SSH-Key einrichten" dialog.
    pub fn start_key_setup(&mut self) {
        let Some(ks) = self.key_setup.as_mut() else {
            return;
        };
        let Some(host) = self.lib.host(ks.host).cloned() else {
            return;
        };
        let (private, public) = match ks
            .identity
            .and_then(|id| self.keychain.identities.iter().find(|i| i.id == id))
        {
            Some(i) => match crate::keygen::public_info(&i.private_key, &i.passphrase) {
                Ok((public, _)) => (i.private_key.clone(), public),
                Err(e) => {
                    ks.error = Some(format!("Schlüssel nicht lesbar: {e:#}"));
                    return;
                }
            },
            None => {
                let comment = format!("aurora@{}", host.display_name().replace(' ', "-"));
                let comment: String = comment
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-'))
                    .collect();
                match crate::keygen::generate_ed25519(&comment) {
                    Ok(pair) => {
                        ks.generated = Some(pair.clone());
                        pair
                    }
                    Err(e) => {
                        ks.error = Some(format!("{e:#}"));
                        return;
                    }
                }
            }
        };
        ks.running = true;
        ks.error = None;
        let password = (!ks.password.is_empty()).then(|| ks.password.clone());
        let (user, ident) = self.credentials(&host);
        // Log in with the password only for the installation.
        let mut params = self.ssh_params(&host, &user, &password, &ident);
        params.private_key = None;
        params.passphrase = None;
        let (sink, prompts) = (self.key_sink.clone(), self.prompts_sink.clone());
        self.rt.spawn(async move {
            let r = crate::backend::ssh::install_key(params, public, private, prompts).await;
            sink.send(r.map_err(|e| format!("{e:#}")));
        });
    }

    fn finish_key_setup(&mut self, r: Result<(), String>) {
        let Some(ks) = self.key_setup.as_mut() else {
            return;
        };
        ks.running = false;
        if let Err(e) = r {
            ks.error = Some(e);
            return;
        }
        let ks = self.key_setup.take().expect("checked above");
        let Some(host) = self.lib.host(ks.host).cloned() else {
            return;
        };
        let (user, _) = self.credentials(&host);
        let ident_id = match (ks.identity, ks.generated) {
            (Some(id), _) => id,
            (None, Some((private, _))) => {
                let mut ident = Identity::new();
                ident.label = format!("Key {}", host.display_name());
                ident.username = user;
                ident.private_key = private;
                let id = ident.id;
                self.keychain.identities.push(ident);
                self.save_keychain();
                id
            }
            (None, None) => return,
        };
        if let Some(h) = self.lib.hosts.iter_mut().find(|h| h.id == host.id) {
            h.identity_id = Some(ident_id);
        }
        self.save_library();
        self.toast(
            format!("{}: Anmeldung jetzt per SSH-Key", host.display_name()),
            theme::green(),
        );
    }

    pub fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(scan) = self.scan.as_mut() else {
            return;
        };
        let parsed = crate::backend::scan::parse_range(&scan.range)
            .and_then(|ips| crate::backend::scan::parse_ports(&scan.ports).map(|p| (ips, p)));
        match parsed {
            Ok((ips, ports)) => {
                let (sink, rx) = Sink::new(ctx.clone());
                scan.progress = (0, ips.len());
                scan.results.clear();
                scan.error = None;
                scan.running = true;
                scan.rx = Some(rx);
                crate::backend::scan::spawn(&self.rt, ips, ports, sink);
            }
            Err(e) => scan.error = Some(format!("{e:#}")),
        }
    }

    /// Adds the ticked scan results as hosts.
    pub fn import_scan(&mut self) {
        let Some(scan) = self.scan.take() else {
            return;
        };
        let picked: Vec<_> = scan
            .results
            .into_iter()
            .filter(|(_, t)| *t)
            .map(|(f, _)| f)
            .collect();
        if picked.is_empty() {
            return;
        }
        let group = if scan.new_group {
            let prefix = crate::model::subnet24(&picked[0].ip.to_string()).unwrap_or_default();
            let g = Group {
                name: format!("LAN {prefix}"),
                color: [96, 165, 250],
                ..Default::default()
            };
            let id = g.id;
            if let Some(existing) = self.lib.groups.iter().find(|x| x.name == g.name) {
                Some(existing.id)
            } else {
                self.lib.groups.push(g);
                Some(id)
            }
        } else {
            None
        };
        let n = picked.len();
        for f in picked {
            let ssh_port = f
                .ports
                .iter()
                .copied()
                .find(|p| *p != 3389 && (f.banner.is_some() || *p == 22));
            let (protocol, port) = match ssh_port {
                Some(p) => (Protocol::Ssh, p),
                None => (Protocol::Rdp, 3389),
            };
            self.lib.hosts.push(Host {
                hostname: f.ip.to_string(),
                port,
                protocol,
                group_id: group,
                ..Default::default()
            });
        }
        self.save_library();
        self.toast(
            if n == 1 {
                "1 Host hinzugefügt – der Name kommt beim ersten Login".to_string()
            } else {
                format!("{n} Hosts hinzugefügt – Namen kommen beim ersten Login")
            },
            theme::green(),
        );
    }

    /// Open sessions as (host, kind) for "restore sessions".
    fn session_snapshot(&self) -> Vec<crate::model::SavedSession> {
        self.tabs
            .iter()
            .filter_map(|t| {
                let (host, kind) = match &t.kind {
                    TabKind::Term(_) => (t.host_id?, "term"),
                    TabKind::Rdp(_) => (t.host_id?, "rdp"),
                    TabKind::Sftp(f) => (
                        f.panes
                            .iter()
                            .rev()
                            .find_map(|p| p.remote.as_ref()?.host_id)?,
                        "files",
                    ),
                };
                Some(crate::model::SavedSession {
                    host,
                    kind: kind.into(),
                    in_split: self.split.contains(&t.id),
                })
            })
            .collect()
    }

    /// Reopens the sessions of the last run (once, right after unlocking).
    fn restore_sessions(&mut self, ctx: &egui::Context) {
        let saved = std::mem::take(&mut self.lib.settings.last_session);
        if !self.lib.settings.restore_sessions || saved.is_empty() {
            return;
        }
        self.group_split = saved
            .iter()
            .filter(|s| s.in_split && s.kind != "files")
            .map(|s| s.host)
            .collect();
        let mut n = 0;
        for s in saved {
            let Some(h) = self.lib.host(s.host).cloned() else {
                continue;
            };
            n += 1;
            if s.kind == "files" {
                self.open_files(None, Some(h.id), ctx);
            } else {
                self.connect_host(h, Purpose::Terminal, ctx);
            }
        }
        if n > 0 {
            self.toast(
                format!("{n} Sessions der letzten Sitzung wiederhergestellt"),
                theme::accent(),
            );
        }
    }

    /// Writes hosts, groups, snippets, keys and known hosts into an encrypted file.
    pub fn export_data(&mut self) {
        if self.export_pw.chars().count() < 8 {
            self.toast("Export-Passwort: mindestens 8 Zeichen", theme::amber());
            return;
        }
        let bundle = crate::store::ExportBundle {
            version: 1,
            hosts: self.lib.hosts.clone(),
            groups: self.lib.groups.clone(),
            snippets: self.lib.snippets.clone(),
            identities: self.keychain.identities.clone(),
            known_hosts: self.lib.known_hosts.clone(),
        };
        let path = PathBuf::from(self.export_path.trim());
        let r = serde_json::to_vec(&bundle)
            .map_err(anyhow::Error::from)
            .and_then(|plain| crate::store::seal(&self.export_pw, &plain))
            .and_then(|text| crate::store::write_export(&path, &text));
        match r {
            Ok(()) => {
                self.export_pw.clear();
                self.toast(
                    format!("Exportiert nach {} – verschlüsselt", path.display()),
                    theme::green(),
                );
            }
            Err(e) => self.toast(format!("Export fehlgeschlagen: {e:#}"), theme::red()),
        }
    }

    /// Adds everything from an export file that isn't here yet (matched by id).
    pub fn import_data(&mut self) {
        let path = PathBuf::from(self.export_path.trim());
        let r = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| crate::store::unseal(&self.export_pw, &text))
            .and_then(|plain| {
                serde_json::from_slice::<crate::store::ExportBundle>(&plain).map_err(Into::into)
            });
        let bundle = match r {
            Ok(b) => b,
            Err(e) => {
                self.toast(format!("Import fehlgeschlagen: {e:#}"), theme::red());
                return;
            }
        };
        let mut n = 0;
        for g in bundle.groups {
            if self.lib.group(g.id).is_none() {
                self.lib.groups.push(g);
                n += 1;
            }
        }
        for h in bundle.hosts {
            if self.lib.host(h.id).is_none() {
                self.lib.hosts.push(h);
                n += 1;
            }
        }
        for sn in bundle.snippets {
            if self.lib.snippet(sn.id).is_none() {
                self.lib.snippets.push(sn);
                n += 1;
            }
        }
        for i in bundle.identities {
            if !self.keychain.identities.iter().any(|x| x.id == i.id) {
                self.keychain.identities.push(i);
                n += 1;
            }
        }
        for (k, v) in bundle.known_hosts {
            self.lib.known_hosts.entry(k).or_insert(v);
        }
        self.export_pw.clear();
        self.save_library();
        self.save_keychain();
        self.toast(format!("{n} Einträge importiert"), theme::green());
    }

    /// Coloured frame around PROD / STAGING sessions.
    fn env_frame(&self, painter: &egui::Painter, rect: egui::Rect, id: u64) {
        let Some(env) = self.tabs.iter().find(|t| t.id == id).map(|t| t.env) else {
            return;
        };
        if matches!(env, Env::Prod | Env::Staging) {
            let width = if env == Env::Prod { 3.0 } else { 2.0 };
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(width, theme::env_color(env)),
                egui::StrokeKind::Inside,
            );
        }
    }

    /// A session tile dragged out of the rail: preview where it lands, place it on release.
    fn drag_drop(&mut self, ctx: &egui::Context, central: egui::Rect, split_view: bool) {
        use crate::ui::chrome;
        let Some(dragged) = chrome::dragged_tab(ctx) else {
            return;
        };
        let Some(tab) = self.tabs.iter().find(|t| t.id == dragged) else {
            chrome::end_drag(ctx);
            return;
        };
        let (pos, released, cancel) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                !i.pointer.primary_down(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        let shown: Vec<u64> = if split_view {
            self.split.clone()
        } else {
            self.active.into_iter().collect()
        };
        let order = pos.and_then(|p| chrome::drop_order(central, &shown, dragged, p));

        if released || cancel {
            chrome::end_drag(ctx);
            if let (Some(order), false) = (order, cancel) {
                if order.len() >= 2 {
                    self.split = order;
                    self.maximized = None;
                }
                self.active = Some(dragged);
            }
            return;
        }

        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("aurora-drop-preview"),
        ));
        if let Some(order) = &order {
            // Show the resulting layout: the new slot highlighted, the others outlined.
            let rects = chrome::split_rects(central, order.len());
            painter.rect_filled(
                central,
                CornerRadius::ZERO,
                theme::bg_deep().gamma_multiply(0.55),
            );
            for (id, r) in order.iter().zip(&rects) {
                let r = r.shrink(6.0);
                if *id == dragged {
                    painter.rect(
                        r,
                        CornerRadius::same(10),
                        theme::accent().gamma_multiply(0.16),
                        Stroke::new(2.0, theme::accent()),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        r.center() - egui::vec2(0.0, 10.0),
                        egui::Align2::CENTER_CENTER,
                        &tab.title,
                        theme::mono_medium(15.0),
                        theme::accent(),
                    );
                    painter.text(
                        r.center() + egui::vec2(0.0, 12.0),
                        egui::Align2::CENTER_CENTER,
                        "hier ablegen",
                        egui::FontId::proportional(12.5),
                        theme::text_dim(),
                    );
                } else {
                    let title = self
                        .tabs
                        .iter()
                        .find(|t| t.id == *id)
                        .map(|t| t.title.as_str())
                        .unwrap_or_default();
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(10),
                        Stroke::new(1.0, theme::border()),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        title,
                        egui::FontId::monospace(13.0),
                        theme::text_dim(),
                    );
                }
            }
        }
        if let Some(p) = pos {
            // Ghost tile under the pointer.
            let r =
                egui::Rect::from_center_size(p + egui::vec2(18.0, 18.0), egui::vec2(36.0, 36.0));
            painter.rect(
                r,
                CornerRadius::same(8),
                theme::bg_card(),
                Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                crate::ui::chrome::initials(&tab.title),
                theme::mono_medium(12.0),
                theme::text(),
            );
        }
        ctx.request_repaint();
    }

    pub fn close_tab(&mut self, id: u64) {
        let was_split = self.split.contains(&id);
        self.split.retain(|s| *s != id);
        // Closing a pane keeps the focus inside the split.
        let partner = self.split.first().copied().filter(|_| was_split);
        if self.split.len() < 2 {
            self.split.clear();
        }
        if let Some(idx) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(idx);
            match tab.kind {
                TabKind::Term(t) => {
                    let _ = t.tx.send(TermCmd::Close);
                }
                TabKind::Sftp(t) => {
                    for r in t.panes.iter().filter_map(|p| p.remote.as_ref()) {
                        let _ = r.tx.send(SftpCmd::Close);
                    }
                }
                TabKind::Rdp(_) => {}
            }
            if self.active == Some(id) {
                self.active = self
                    .tabs
                    .get(idx.saturating_sub(1).min(self.tabs.len().saturating_sub(1)))
                    .map(|t| t.id);
                if self.tabs.is_empty() {
                    self.active = None;
                }
                if partner.is_some() {
                    self.active = partner;
                }
            }
        }
    }

    /// Bytes typed into a terminal; mirrored to all terminals in broadcast mode.
    pub fn send_input(&mut self, from: u64, bytes: Vec<u8>) {
        let split = self.split_broadcast_targets();
        let enter = bytes.contains(&b'\r');
        for tab in &mut self.tabs {
            if let TabKind::Term(t) = &mut tab.kind {
                let live = matches!(t.state, ConnState::Connected);
                let target = tab.id == from
                    || (live && self.broadcast)
                    || (live && split.contains(&from) && split.contains(&tab.id));
                if target {
                    let _ = t.tx.send(TermCmd::Input(bytes.clone()));
                    if enter {
                        t.cmd_started = Some(Instant::now());
                    }
                }
            }
        }
    }

    /// Panes on screen side by side right now (not counting a maximized pane).
    pub fn split_view(&self) -> bool {
        self.split.len() >= 2
            && self.active.is_some_and(|a| self.split.contains(&a))
            && self.maximized != self.active
    }

    /// Tabs currently visible on screen.
    pub fn visible_tabs(&self) -> Vec<u64> {
        if self.split_view() {
            self.split.clone()
        } else {
            self.active.into_iter().collect()
        }
    }

    fn split_broadcast_targets(&self) -> Vec<u64> {
        if self.split_broadcast && self.split_view() {
            self.split.clone()
        } else {
            Vec::new()
        }
    }

    pub fn run_snippet_text(&mut self, text: &str, auto_run: bool, all: bool) {
        let mut payload = text.replace("\r\n", "\n").replace('\n', "\r");
        if auto_run && !payload.ends_with('\r') {
            payload.push('\r');
        }
        let mut sent = 0;
        let active = self.active;
        let everywhere = all || self.broadcast;
        let split = self.split_broadcast_targets();
        for tab in &mut self.tabs {
            if let TabKind::Term(t) = &mut tab.kind
                && matches!(t.state, ConnState::Connected)
                && (everywhere || Some(tab.id) == active || split.contains(&tab.id))
            {
                let _ = t.tx.send(TermCmd::Input(payload.clone().into_bytes()));
                sent += 1;
            }
        }
        if sent == 0 {
            self.toast("Keine verbundene Terminal-Session aktiv", theme::amber());
        } else if sent > 1 {
            self.toast(
                format!("Snippet an {sent} Sessions gesendet"),
                theme::accent(),
            );
        }
    }

    // ------------------------------------------------------------------ events

    pub fn check_updates(&mut self) {
        self.update.checking = true;
        self.update.manual = true;
        self.update.message = None;
        crate::update::check(self.update_sink.clone());
    }

    pub fn install_update(&mut self) {
        if let Some(r) = self.update.latest.clone() {
            self.update.installing = true;
            self.update.progress = 0.0;
            self.update.message = None;
            crate::update::install(r, self.update_sink.clone());
        }
    }

    fn pump_updates(&mut self, ctx: &egui::Context) {
        use crate::update::UpdateEvent;
        while let Ok(ev) = self.update_rx.try_recv() {
            match ev {
                UpdateEvent::Checked(r) => {
                    self.update.checking = false;
                    match r {
                        Ok(Some(rel)) => {
                            if self.update.latest.is_none() {
                                self.toast(
                                    format!("Aurora {} ist verfügbar", rel.version_text()),
                                    theme::accent(),
                                );
                            }
                            self.update.latest = Some(rel);
                        }
                        Ok(None) if self.update.manual => {
                            self.update.message =
                                Some(("Aurora ist aktuell.".into(), theme::green()));
                        }
                        Err(e) if self.update.manual => {
                            self.update.message = Some((e, theme::red()));
                        }
                        _ => {}
                    }
                }
                UpdateEvent::Progress(p) => self.update.progress = p,
                UpdateEvent::Installed(r) => {
                    self.update.installing = false;
                    match r {
                        Ok(true) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Ok(false) => {
                            self.update.installed = true;
                            self.update.message = Some((
                                "Installiert – Neustart aktiviert die neue Version.".into(),
                                theme::green(),
                            ));
                        }
                        Err(e) => self.update.message = Some((e, theme::red())),
                    }
                }
            }
        }
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        self.pump_updates(ctx);
        while let Ok(p) = self.prompts_rx.try_recv() {
            self.hostkey_queue.push_back(p);
        }
        let mut toasts = Vec::new();
        let mut prompts = Vec::new();
        let visible = self.visible_tabs();
        let focused = ctx.input(|i| i.focused);
        let mut notify = false;
        let mut learned: Vec<(Uuid, crate::model::HostFacts)> = Vec::new();
        let mut outcome: Vec<(Uuid, bool)> = Vec::new();
        for tab in &mut self.tabs {
            let seen = focused && visible.contains(&tab.id);
            match &mut tab.kind {
                TabKind::Term(t) => {
                    while let Ok(ev) = t.rx.try_recv() {
                        match ev {
                            TermEvent::Status(s) => {
                                let a = theme::accent();
                                t.term.feed(
                                    format!(
                                        "\x1b[38;2;{};{};{}m›\x1b[0m {s}\r\n",
                                        a.r(),
                                        a.g(),
                                        a.b()
                                    )
                                    .as_bytes(),
                                );
                                t.state = ConnState::Connecting(s);
                            }
                            TermEvent::Facts(f) => {
                                if let Some(h) = tab.host_id {
                                    learned.push((h, f));
                                }
                            }
                            TermEvent::Connected => {
                                t.state = ConnState::Connected;
                                if let Some(h) = tab.host_id {
                                    outcome.push((h, true));
                                }
                                let (cols, rows) = t.term.size();
                                let _ = t.tx.send(TermCmd::Resize { cols, rows });
                                if let Some(cmd) = t.run_once.take() {
                                    let _ =
                                        t.tx.send(TermCmd::Input(format!("{cmd}\r").into_bytes()));
                                }
                                if let Some(s) = &t.startup {
                                    let mut s = s.replace('\n', "\r");
                                    s.push('\r');
                                    let _ = t.tx.send(TermCmd::Input(s.into_bytes()));
                                }
                            }
                            TermEvent::Output(b) => {
                                t.term.feed(&b);
                                t.last_output = Instant::now();
                            }
                            TermEvent::UploadProgress { name, done, total } => {
                                t.upload = Some((name, done, total));
                            }
                            TermEvent::UploadDone(r) => {
                                t.upload = None;
                                toasts.push(match r {
                                    Ok(dir) => (format!("Hochgeladen nach {dir}"), theme::green()),
                                    Err(e) => (format!("Upload fehlgeschlagen: {e}"), theme::red()),
                                });
                            }
                            TermEvent::Closed(err) => {
                                let msg = match &err {
                                    Some(e) => format!("\r\n\x1b[31m✖ {e}\x1b[0m\r\n"),
                                    None => "\r\n\x1b[2mVerbindung beendet.\x1b[0m\r\n".into(),
                                };
                                t.term.feed(msg.as_bytes());
                                if let Some(e) = &err {
                                    toasts.push((format!("{}: {e}", tab.title), theme::red()));
                                } else if !seen {
                                    toasts.push((
                                        format!(
                                            "{} getrennt – Klick auf die Kachel verbindet neu",
                                            tab.title
                                        ),
                                        theme::amber(),
                                    ));
                                }
                                if err.is_some()
                                    && !matches!(t.state, ConnState::Connected)
                                    && let Some(h) = tab.host_id
                                {
                                    outcome.push((h, false));
                                }
                                t.state = ConnState::Closed(err);
                                t.cmd_started = None;
                                t.upload = None;
                            }
                        }
                    }
                    // Background notifications: a long command finished, or the bell rang.
                    if t.term.take_bell() && !seen {
                        tab.attention = true;
                        notify = true;
                    }
                    if let Some(start) = t.cmd_started {
                        let quiet = t.last_output.elapsed();
                        if t.last_output > start
                            && quiet >= Duration::from_millis(1200)
                            && t.term.at_prompt()
                        {
                            t.cmd_started = None;
                            let took = t.last_output - start;
                            if took >= Duration::from_secs(5) && !seen {
                                tab.attention = true;
                                notify = true;
                                toasts.push((
                                    format!(
                                        "{}: Befehl fertig nach {}",
                                        tab.title,
                                        crate::ui::short_duration(took)
                                    ),
                                    theme::accent(),
                                ));
                            }
                        } else {
                            ctx.request_repaint_after(Duration::from_millis(400));
                        }
                    }
                }
                TabKind::Sftp(t) => {
                    for idx in 0..2 {
                        while let Some(ev) = t.panes[idx]
                            .remote
                            .as_mut()
                            .and_then(|r| r.rx.try_recv().ok())
                        {
                            crate::ui::sftp::handle_event(t, idx, ev, &mut toasts);
                        }
                    }
                }
                TabKind::Rdp(t) => {
                    if let Some(v) = t.embedded.as_mut() {
                        let host_id = format!("rdp://{}:{}", t.launch.host, t.launch.port);
                        v.pump(ctx, &host_id, &mut toasts, &mut prompts);
                    }
                    if let Some(child) = t.process.as_mut().and_then(|p| p.child.as_mut())
                        && t.running
                        && let Ok(Some(status)) = child.try_wait()
                    {
                        t.running = false;
                        t.status = if status.success() {
                            "RDP-Sitzung beendet".into()
                        } else {
                            format!("RDP-Client beendet ({status})")
                        };
                    }
                }
            }
        }
        self.hostkey_queue.extend(prompts);
        for (h, ok) in outcome {
            if ok {
                self.failed.remove(&h);
            } else {
                self.failed.insert(h);
            }
        }
        if !learned.is_empty() {
            for (h, f) in learned {
                // Hosts saved under their IP get the server's own name.
                if let Some(host) = self.lib.hosts.iter_mut().find(|x| x.id == h)
                    && host.label.trim().is_empty()
                    && !f.hostname.is_empty()
                    && f.hostname != host.hostname
                    && f.hostname != "localhost"
                {
                    toasts.push((
                        format!("{} heißt jetzt „{}“", host.hostname, f.hostname),
                        theme::accent(),
                    ));
                    host.label = f.hostname.clone();
                }
                self.lib.facts.insert(h, f);
            }
            self.save_library();
        }
        while let Ok(ev) = self.tunnel_rx.try_recv() {
            use backend::tunnel::TunnelEvent;
            match ev {
                TunnelEvent::Up(tid) => {
                    let Some(run) = self.tunnels.get_mut(&tid) else {
                        continue;
                    };
                    run.state = TunnelState::Up;
                    let tunnel = self
                        .lib
                        .host(run.host)
                        .and_then(|h| h.tunnels.iter().find(|t| t.id == tid))
                        .cloned();
                    if let Some(t) = tunnel {
                        self.toast(
                            format!("Tunnel „{}“ offen: localhost:{}", t.label(), t.local_port),
                            theme::green(),
                        );
                        if t.open_browser {
                            crate::update::open_url(&format!("http://localhost:{}", t.local_port));
                        }
                    }
                }
                TunnelEvent::Conns(tid, d) => {
                    if let Some(run) = self.tunnels.get_mut(&tid) {
                        run.conns = (run.conns + d).max(0);
                    }
                }
                TunnelEvent::Down(tid, err) => match err {
                    Some(e) => {
                        if let Some(run) = self.tunnels.get_mut(&tid) {
                            run.state = TunnelState::Failed(e.clone());
                            run.stop = None;
                        }
                        self.toast(format!("Tunnel: {e}"), theme::red());
                    }
                    None => {
                        self.tunnels.remove(&tid);
                    }
                },
            }
        }
        while let Ok(r) = self.key_rx.try_recv() {
            self.finish_key_setup(r);
        }
        if let Some(scan) = self.scan.as_mut()
            && let Some(rx) = scan.rx.as_mut()
        {
            use crate::backend::scan::ScanEvent;
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    ScanEvent::Progress(d, t) => scan.progress = (d, t),
                    ScanEvent::Found(f) => {
                        let known = self
                            .lib
                            .hosts
                            .iter()
                            .any(|h| h.hostname.trim() == f.ip.to_string());
                        scan.results.push((f, !known));
                        scan.results.sort_by_key(|(f, _)| f.ip);
                    }
                    ScanEvent::Done => {
                        scan.running = false;
                        scan.rx = None;
                        break;
                    }
                }
            }
        }
        if notify && !focused {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
        for tab in self.tabs.iter_mut().filter(|t| visible.contains(&t.id)) {
            tab.attention = false;
        }
        for (text, color) in toasts {
            self.toast(text, color);
        }
    }

    // ------------------------------------------------------------------ actions

    pub fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Connect(id) => {
                if let Some(h) = self.lib.host(id).cloned() {
                    self.connect_host(h, Purpose::Terminal, ctx);
                }
            }
            Action::OpenSftp(id) => self.open_files(None, Some(id), ctx),
            Action::OpenFiles(l, r) => self.open_files(l, r, ctx),
            Action::SetPaneSource(tab, idx, host) => {
                match host.and_then(|h| self.lib.host(h).cloned()) {
                    Some(h) => self.connect_host(
                        crate::ui::sftp::sftp_target(h),
                        Purpose::Pane(tab, idx),
                        ctx,
                    ),
                    None => self.set_pane(tab, idx, None, theme::accent()),
                }
            }
            Action::QuickConnect(s) => match parse_quick(&s) {
                Some(host) => self.connect_host(host, Purpose::Terminal, ctx),
                None => self.toast("Format: user@host[:port]", theme::amber()),
            },
            Action::NewHostOf(protocol, group) => {
                let host = Host {
                    group_id: group,
                    protocol,
                    port: protocol.default_port(),
                    ..Default::default()
                };
                self.host_editor = Some(HostEditor {
                    host,
                    is_new: true,
                    tags_text: String::new(),
                });
            }
            Action::NewHost(group) => {
                let host = Host {
                    group_id: group,
                    ..Default::default()
                };
                self.host_editor = Some(HostEditor {
                    host,
                    is_new: true,
                    tags_text: String::new(),
                });
            }
            Action::EditHost(id) => {
                if let Some(h) = self.lib.host(id) {
                    self.host_editor = Some(HostEditor {
                        tags_text: h.tags.join(", "),
                        host: h.clone(),
                        is_new: false,
                    });
                }
            }
            Action::DuplicateHost(id) => {
                if let Some(h) = self.lib.host(id) {
                    let mut copy = h.clone();
                    copy.id = Uuid::new_v4();
                    copy.label = format!("{} (Kopie)", h.display_name());
                    copy.last_connected = None;
                    self.lib.hosts.push(copy);
                    self.save_library();
                }
            }
            Action::DeleteHost(id) => {
                self.lib.hosts.retain(|h| h.id != id);
                self.save_library();
            }
            Action::ToggleFavorite(id) => {
                if let Some(h) = self.lib.hosts.iter_mut().find(|h| h.id == id) {
                    h.favorite = !h.favorite;
                }
                self.save_library();
            }
            Action::MoveHost(id, group) => {
                if let Some(h) = self.lib.hosts.iter_mut().find(|h| h.id == id) {
                    h.group_id = group;
                }
                self.save_library();
            }
            Action::NewGroup(parent) => {
                let g = Group {
                    parent_id: parent,
                    color: theme::TAG_COLORS[self.lib.groups.len() % theme::TAG_COLORS.len()],
                    ..Default::default()
                };
                self.group_editor = Some((g, true));
            }
            Action::EditGroup(id) => {
                if let Some(g) = self.lib.group(id) {
                    self.group_editor = Some((g.clone(), false));
                }
            }
            Action::DeleteGroup(id) => {
                let parent = self.lib.group(id).and_then(|g| g.parent_id);
                for h in &mut self.lib.hosts {
                    if h.group_id == Some(id) {
                        h.group_id = parent;
                    }
                }
                for g in &mut self.lib.groups {
                    if g.parent_id == Some(id) {
                        g.parent_id = parent;
                    }
                }
                self.lib.groups.retain(|g| g.id != id);
                self.save_library();
            }
            Action::ToggleGroup(id) => {
                if let Some(g) = self.lib.groups.iter_mut().find(|g| g.id == id) {
                    g.collapsed = !g.collapsed;
                }
                self.save_library();
            }
            Action::NewIdentity => {
                self.identity_editor = Some((Identity::new(), true, String::new()))
            }
            Action::EditIdentity(id) => {
                if let Some(i) = self.identity(id) {
                    self.identity_editor = Some((i.clone(), false, String::new()));
                }
            }
            Action::DeleteIdentity(id) => {
                self.keychain.identities.retain(|i| i.id != id);
                for h in &mut self.lib.hosts {
                    if h.identity_id == Some(id) {
                        h.identity_id = None;
                    }
                }
                for g in &mut self.lib.groups {
                    if g.default_identity == Some(id) {
                        g.default_identity = None;
                    }
                }
                self.save_keychain();
                self.save_library();
            }
            Action::NewSnippet => {
                self.snippet_editor = Some((Snippet::default(), true, String::new()))
            }
            Action::EditSnippet(id) => {
                if let Some(s) = self.lib.snippet(id) {
                    self.snippet_editor = Some((s.clone(), false, s.tags.join(", ")));
                }
            }
            Action::DeleteSnippet(id) => {
                self.lib.snippets.retain(|s| s.id != id);
                for h in &mut self.lib.hosts {
                    if h.startup_snippet == Some(id) {
                        h.startup_snippet = None;
                    }
                }
                self.save_library();
            }
            Action::RunSnippet(id, all) => {
                if let Some(s) = self.lib.snippet(id).cloned() {
                    let vars = s.variables();
                    if vars.is_empty() {
                        self.run_snippet_text(&s.script, s.auto_run, all);
                    } else {
                        self.snippet_run = Some(SnippetRun {
                            values: vars.into_iter().map(|v| (v, String::new())).collect(),
                            snippet: s,
                            all,
                        });
                    }
                }
            }
            Action::ForgetKnownHost(h) => {
                self.lib.known_hosts.remove(&h);
                self.save_library();
            }
            Action::Navigate(v) => {
                self.view = v;
                self.active = None;
            }
            Action::OpenPalette => {
                self.palette.open = true;
                self.palette.query.clear();
                self.palette.selected = 0;
            }
            Action::Reconnect(id) => self.reconnect(id, ctx),
            Action::SetTheme(id) => {
                theme::apply(ctx, &id);
                self.lib.settings.theme = id;
                self.save_library();
            }
            Action::Confirm(text, inner) => self.confirm_delete = Some((text, *inner)),
            Action::OpenGroup(gid) => {
                let hosts: Vec<Host> = self
                    .lib
                    .hosts_in_group(gid)
                    .into_iter()
                    .filter(|h| h.protocol != Protocol::Serial)
                    .take(crate::ui::chrome::MAX_SPLIT)
                    .collect();
                if hosts.is_empty() {
                    self.toast("Die Gruppe enthält keine Hosts", theme::amber());
                } else {
                    self.split.clear();
                    self.maximized = None;
                    self.group_split = hosts.iter().map(|h| h.id).collect();
                    for h in hosts {
                        self.connect_host(h, Purpose::Terminal, ctx);
                    }
                }
            }
            Action::SnippetFromText(text) => {
                let name: String = text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(40)
                    .collect();
                self.snippet_editor = Some((
                    Snippet {
                        name: name.trim().to_string(),
                        script: text.trim_end().to_string(),
                        ..Default::default()
                    },
                    true,
                    String::new(),
                ));
            }
            Action::SaveQuickHost(tab_id) => {
                if let Some(h) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .and_then(|t| t.unsaved.clone())
                {
                    let mut h = h;
                    if h.label.is_empty() {
                        h.label = h.hostname.clone();
                    }
                    self.host_editor = Some(HostEditor::new(h, true));
                }
            }
            Action::NewHostNamed(name) => {
                let host = parse_quick(&name).unwrap_or_else(|| Host {
                    hostname: name.trim().to_string(),
                    ..Default::default()
                });
                self.host_editor = Some(HostEditor::new(host, true));
            }
            Action::ImportSshConfig => {
                let found = ssh_config_hosts();
                let mut added = 0;
                for h in found {
                    let dup = self.lib.hosts.iter().any(|x| {
                        x.hostname.eq_ignore_ascii_case(&h.hostname)
                            && x.port == h.port
                            && x.username == h.username
                    });
                    if !dup {
                        self.lib.hosts.push(h);
                        added += 1;
                    }
                }
                self.save_library();
                self.toast(
                    match added {
                        0 => "Keine neuen Hosts in ~/.ssh/config".to_string(),
                        1 => "1 Host importiert".to_string(),
                        n => format!("{n} Hosts importiert"),
                    },
                    theme::green(),
                );
            }
            Action::RenameHost(id, name) => {
                if let Some(h) = self.lib.hosts.iter_mut().find(|h| h.id == id) {
                    h.label = name.trim().to_string();
                }
                self.save_library();
            }
            Action::GroupSubnet(prefix) => {
                let group = Group {
                    name: format!("LAN {prefix}"),
                    color: [96, 165, 250],
                    ..Default::default()
                };
                let gid = group.id;
                self.lib.groups.push(group);
                let mut n = 0;
                for h in self.lib.hosts.iter_mut() {
                    let loose = h.group_id.is_none();
                    if loose && crate::model::subnet24(&h.hostname).as_deref() == Some(&prefix) {
                        h.group_id = Some(gid);
                        n += 1;
                    }
                }
                self.save_library();
                self.toast(format!("{n} Hosts in „LAN {prefix}“"), theme::green());
            }
            Action::DismissSubnet(prefix) => {
                self.lib.settings.dismissed_subnets.push(prefix);
                self.save_library();
            }
            Action::OpenScan => {
                self.scan = Some(ScanState {
                    range: crate::backend::scan::local_subnet()
                        .unwrap_or_else(|| "192.168.0.0/24".into()),
                    ports: "22, 3389".into(),
                    running: false,
                    progress: (0, 0),
                    results: Vec::new(),
                    rx: None,
                    error: None,
                    new_group: true,
                });
                self.start_scan(ctx);
            }
            Action::SetupKey(id) => {
                let host = self.lib.host(id).cloned();
                if let Some(host) = host {
                    let (_, ident) = self.credentials(&host);
                    self.key_setup = Some(KeySetup {
                        host: id,
                        identity: None,
                        password: ident.map(|i| i.password).unwrap_or_default(),
                        running: false,
                        error: None,
                        generated: None,
                    });
                }
            }
            Action::SelectHost(id, mode) => match mode {
                1 => {
                    if !self.selected_hosts.remove(&id) {
                        self.selected_hosts.insert(id);
                    }
                    self.select_anchor = Some(id);
                }
                2 => {
                    let order: Vec<Uuid> = ctx
                        .data(|d| d.get_temp(crate::ui::hosts::order_id()))
                        .unwrap_or_default();
                    let anchor = self.select_anchor.unwrap_or(id);
                    let a = order.iter().position(|x| *x == anchor);
                    let b = order.iter().position(|x| *x == id);
                    if let (Some(a), Some(b)) = (a, b) {
                        let (lo, hi) = (a.min(b), a.max(b));
                        self.selected_hosts.extend(order[lo..=hi].iter().copied());
                    } else {
                        self.selected_hosts.insert(id);
                    }
                }
                _ => {
                    self.selected_hosts.clear();
                    self.select_anchor = Some(id);
                }
            },
            Action::ClearSelection => {
                self.selected_hosts.clear();
                self.select_anchor = None;
            }
            Action::MoveHosts(ids, group) => {
                for h in self.lib.hosts.iter_mut().filter(|h| ids.contains(&h.id)) {
                    h.group_id = group;
                }
                self.save_library();
            }
            Action::ConnectMany(ids) => {
                let hosts: Vec<Host> = ids
                    .iter()
                    .filter_map(|id| self.lib.host(*id).cloned())
                    .filter(|h| h.protocol != Protocol::Serial)
                    .collect();
                if hosts.len() > 1 {
                    self.split.clear();
                    self.maximized = None;
                    self.group_split = hosts
                        .iter()
                        .take(crate::ui::chrome::MAX_SPLIT)
                        .map(|h| h.id)
                        .collect();
                }
                for h in hosts {
                    self.connect_host(h, Purpose::Terminal, ctx);
                }
                self.selected_hosts.clear();
            }
            Action::SetEnvMany(ids, env) => {
                for h in self.lib.hosts.iter_mut().filter(|h| ids.contains(&h.id)) {
                    h.env = env;
                }
                self.save_library();
            }
            Action::DeleteHosts(ids) => {
                self.lib.hosts.retain(|h| !ids.contains(&h.id));
                for id in &ids {
                    self.lib.facts.remove(id);
                }
                self.selected_hosts.clear();
                self.save_library();
            }
            Action::ToggleTunnel(host_id, tid) => {
                if let Some(run) = self.tunnels.remove(&tid) {
                    drop(run.stop);
                } else if let Some(h) = self.lib.host(host_id).cloned() {
                    self.tunnels.insert(
                        tid,
                        TunnelRun {
                            host: host_id,
                            state: TunnelState::Connecting,
                            conns: 0,
                            stop: None,
                        },
                    );
                    self.connect_host(h, Purpose::Tunnel(tid), ctx);
                }
            }
            Action::RunUpdates(id) => {
                let os = self
                    .lib
                    .facts
                    .get(&id)
                    .map(|f| f.os_id.clone())
                    .unwrap_or_default();
                let cmd = match os.as_str() {
                    "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => "sudo dnf upgrade",
                    _ => "sudo apt-get update && sudo apt-get upgrade",
                };
                if let Some(h) = self.lib.host(id).cloned() {
                    self.pending_command = Some((id, cmd.to_string()));
                    self.connect_host(h, Purpose::Terminal, ctx);
                }
            }
            Action::Lock => {
                self.vault = None;
                self.keychain = Keychain::default();
                self.lock = LockScreen::default();
                self.host_editor = None;
                self.identity_editor = None;
                self.password_prompt = None;
            }
        }
    }

    // ------------------------------------------------------------------ layout

    fn terminal_tab(&mut self, ui: &mut egui::Ui, tab_id: u64, actions: &mut Vec<Action>) {
        let ctx = ui.ctx().clone();
        let band = if self.broadcast {
            Some(theme::pink())
        } else if self.split_broadcast_targets().contains(&tab_id) {
            Some(theme::accent())
        } else {
            None
        };
        // Files dragged in from the OS land in the active pane (the OS doesn't tell us
        // where the pointer is while dragging).
        let drop_target = self.active == Some(tab_id);
        let settings = self.lib.settings.clone();
        let accent = self.accent();
        let snippets: Vec<(Uuid, String, String)> = self
            .lib
            .snippets
            .iter()
            .map(|s| (s.id, s.name.clone(), s.script.clone()))
            .collect();
        let mut reconnect = false;
        let mut input = None;
        let mut danger = None;
        // Kept until the terminal really has the focus: a modal that just closed still
        // blocks focus changes for a frame.
        let refocus = self.focus_term == Some(tab_id);
        let mut focused_now = false;
        let saved_ids: Vec<Uuid> = self.lib.hosts.iter().map(|h| h.id).collect();
        let index = self.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let (title, subtitle) = (tab.title.clone(), tab.subtitle.clone());
        let guarded = tab.env == Env::Prod || settings.warn_everywhere;
        let unsaved = tab
            .unsaved
            .as_ref()
            .is_some_and(|h| saved_ids.iter().all(|id| *id != h.id));
        let has_notes = !tab.notes.is_empty();
        let notes = tab.notes.clone();
        let mut notes_open = tab.notes_open;
        let mut toggle_notes = false;
        let mut save_host = false;
        let mut search = false;
        let TabKind::Term(t) = &mut tab.kind else {
            return;
        };

        let state = crate::ui::chrome::state_pair(&t.state);
        let closed = matches!(t.state, ConnState::Closed(_));
        let mut toggle_snippets = false;
        crate::ui::chrome::session_header(
            ui,
            egui::Id::new(("term-bar", tab_id)),
            index,
            &title,
            &subtitle,
            state,
            |ui| {
                if theme::icon_button(ui, "⚡", "Snippets ein-/ausblenden").clicked() {
                    toggle_snippets = true;
                }
                if theme::icon_button(ui, "🔍", "Im Terminal suchen (Ctrl+Shift+F)").clicked() {
                    search = true;
                }
                if has_notes && theme::icon_button(ui, "📝", "Notizen zum Host").clicked() {
                    toggle_notes = true;
                }
                if unsaved
                    && ui
                        .button("Als Host speichern")
                        .on_hover_text("Diese Schnellverbindung in die Hostliste übernehmen")
                        .clicked()
                {
                    save_host = true;
                }
                if closed && theme::icon_button(ui, "⟳", "Neu verbinden (Enter)").clicked() {
                    reconnect = true;
                }
            },
        );
        if toggle_snippets {
            t.show_snippets = !t.show_snippets;
        }
        if search {
            t.term.open_search();
        }
        if toggle_notes {
            notes_open = !notes_open;
        }
        if notes_open && has_notes {
            egui::Panel::top(egui::Id::new(("term-notes", tab_id)))
                .frame(
                    egui::Frame::NONE
                        .fill(theme::amber().gamma_multiply(0.10))
                        .inner_margin(Margin::symmetric(18, 8)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("📝").color(theme::amber()));
                        ui.add(
                            egui::Label::new(RichText::new(&notes).size(13.0).color(theme::text()))
                                .wrap(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Ausblenden").clicked() {
                                notes_open = false;
                            }
                        });
                    });
                });
        }
        if save_host {
            actions.push(Action::SaveQuickHost(tab_id));
        }

        if t.show_snippets {
            egui::Panel::right(egui::Id::new(("term-snips", tab_id)))
                .default_size(280.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::bg_panel())
                        .inner_margin(Margin::symmetric(14, 14)),
                )
                .show(ui, |ui| {
                    theme::section_title(ui, "Snippets");
                    ui.add(
                        egui::TextEdit::singleline(&mut t.snippet_filter)
                            .hint_text("Filtern")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let f = t.snippet_filter.to_lowercase();
                        if snippets.is_empty() {
                            ui.label(RichText::new("Noch keine Snippets").color(theme::text_dim()));
                        }
                        for (id, name, script) in snippets.iter().filter(|(_, n, s)| {
                            f.is_empty()
                                || n.to_lowercase().contains(&f)
                                || s.to_lowercase().contains(&f)
                        }) {
                            let (rect, resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 46.0),
                                egui::Sense::click(),
                            );
                            if resp.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(6),
                                    theme::bg_hover(),
                                );
                            }
                            let p = ui.painter();
                            p.text(
                                rect.left_top() + Vec2::new(10.0, 8.0),
                                egui::Align2::LEFT_TOP,
                                name,
                                theme::semibold(13.0),
                                theme::text(),
                            );
                            let line = script.lines().next().unwrap_or_default();
                            p.with_clip_rect(rect.shrink(4.0)).text(
                                rect.left_top() + Vec2::new(10.0, 26.0),
                                egui::Align2::LEFT_TOP,
                                line,
                                egui::FontId::monospace(11.5),
                                theme::text_dim(),
                            );
                            if resp
                                .on_hover_text("Klicken zum Ausführen")
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                actions.push(Action::RunSnippet(*id, false));
                            }
                        }
                    });
                });
        }

        let term_bg = theme::palette().term_bg;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(term_bg)
                    .inner_margin(Margin::symmetric(10, 6)),
            )
            .show(ui, |ui| {
                if let Some(color) = band {
                    crate::ui::chrome::band(
                        ui.painter(),
                        ui.max_rect().expand2(Vec2::new(10.0, 6.0)),
                        color,
                    );
                }
                let ssh = matches!(t.spec, ConnectSpec::Ssh(_));
                let live = matches!(t.state, ConnState::Connected);
                let (hovering, dropped) = ctx.input(|i| {
                    (
                        !i.raw.hovered_files.is_empty(),
                        i.raw
                            .dropped_files
                            .iter()
                            .map(|f| f.path().to_path_buf())
                            .collect::<Vec<_>>(),
                    )
                });
                let dir = t.term.cwd();
                let shown_dir = dir.clone().unwrap_or_else(|| "~".into());
                let drop_text;
                let overlay = if hovering && drop_target {
                    drop_text = if !ssh {
                        "Hochladen geht nur bei SSH-Sessions".to_string()
                    } else if !live {
                        "Nicht verbunden".to_string()
                    } else {
                        format!("Loslassen zum Hochladen nach {shown_dir}")
                    };
                    Some(drop_text.as_str())
                } else {
                    match &t.state {
                        ConnState::Connecting(_) if t.term.contents().trim().is_empty() => {
                            Some("Verbinde…")
                        }
                        _ => None,
                    }
                };
                if !dropped.is_empty() && drop_target && ssh && live {
                    let _ = t.tx.send(TermCmd::Upload {
                        files: dropped,
                        dir,
                    });
                    t.upload = Some((String::new(), 0, 0));
                }
                if refocus {
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(("term", tab_id))));
                }
                let out = t.term.show(
                    ui,
                    egui::Id::new(("term", tab_id)),
                    TermView {
                        font_size: settings.font_size,
                        accent,
                        copy_on_select: settings.copy_on_select,
                        glow: None,
                        overlay,
                    },
                );
                focused_now = ui.memory(|m| m.has_focus(egui::Id::new(("term", tab_id))));
                if let Some((cols, rows)) = out.resize {
                    let _ = t.tx.send(TermCmd::Resize { cols, rows });
                }
                if let Some((name, done, total)) = &t.upload {
                    upload_badge(ui, name, *done, *total);
                }
                if matches!(t.state, ConnState::Closed(_)) {
                    closed_badge(ui);
                }
                if !out.input.is_empty() {
                    if matches!(t.state, ConnState::Closed(_)) {
                        // Enter on a dead session reconnects, like Termius.
                        if out.input.contains(&b'\r') {
                            reconnect = true;
                        }
                    } else if guarded
                        && out.input == b"\r"
                        && let Some(why) =
                            crate::model::dangerous_command(&t.term.current_command())
                    {
                        danger = Some((t.term.current_command(), why));
                    } else {
                        input = Some(out.input);
                    }
                }
                if let Some(text) = out.save_snippet {
                    actions.push(Action::SnippetFromText(text));
                }
            });
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.notes_open = notes_open;
        }
        if let Some((cmd, why)) = danger {
            self.danger = Some((tab_id, cmd, why));
        }
        if refocus && focused_now {
            self.focus_term = None;
        }

        if let Some(bytes) = input {
            self.send_input(tab_id, bytes);
        }
        if reconnect {
            self.reconnect(tab_id, &ctx);
        }
    }

    fn rdp_tab(&mut self, ui: &mut egui::Ui, tab_id: u64) {
        let index = self.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let (title, subtitle) = (tab.title.clone(), tab.subtitle.clone());
        let TabKind::Rdp(t) = &mut tab.kind else {
            return;
        };
        if let Some(view) = t.embedded.as_mut() {
            let mut toasts = Vec::new();
            let action = view.show(
                ui,
                egui::Id::new(("rdp", tab_id)),
                index,
                &title,
                &subtitle,
                &t.launch,
                &mut toasts,
            );
            if let crate::ui::rdp::RdpAction::OpenExternal = action {
                t.launch_external();
            }
            for (text, c) in toasts {
                self.toast(text, c);
            }
            return;
        }
        let mut relaunch = false;
        let mut stop = false;
        let state = if t.running {
            (t.status.clone(), theme::green())
        } else {
            (t.status.clone(), theme::text_dim())
        };
        let running = t.running;
        crate::ui::chrome::session_header(
            ui,
            egui::Id::new(("rdp-ext-bar", tab_id)),
            index,
            &title,
            &subtitle,
            state,
            |ui| {
                if running {
                    if theme::icon_button(ui, "⏹", "Beenden").clicked() {
                        stop = true;
                    }
                } else if theme::icon_button(ui, "⟳", "Erneut starten").clicked() {
                    relaunch = true;
                }
            },
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::bg_deep()))
            .show(ui, |ui| {
                let r = ui.max_rect();
                let p = ui.painter();
                // A single big jack: the session lives in an external window.
                let c = r.center() - Vec2::new(0.0, 40.0);
                let ring = if running {
                    theme::accent()
                } else {
                    theme::border()
                };
                p.circle_stroke(c, 34.0, Stroke::new(2.0, ring));
                p.circle_filled(c, 9.0, ring);
                theme::led(
                    p,
                    c + Vec2::new(40.0, -40.0),
                    if running {
                        theme::green()
                    } else {
                        theme::text_dim()
                    },
                    running,
                );
                p.text(
                    c + Vec2::new(0.0, 70.0),
                    egui::Align2::CENTER_CENTER,
                    if running {
                        "Läuft im System-Client"
                    } else {
                        "Fenster geschlossen"
                    },
                    theme::semibold(18.0),
                    theme::text(),
                );
                let detail = if running {
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                    let s = t.started.elapsed().as_secs();
                    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
                } else {
                    format!("{}:{}", t.launch.host, t.launch.port)
                };
                p.text(
                    c + Vec2::new(0.0, 96.0),
                    egui::Align2::CENTER_CENTER,
                    detail,
                    egui::FontId::monospace(13.0),
                    theme::text_dim(),
                );
            });
        if stop && let Some(child) = t.process.as_mut().and_then(|p| p.child.as_mut()) {
            let _ = child.kill();
        }
        if relaunch {
            t.launch_external();
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|t| t.at.elapsed() < Duration::from_secs(5));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for t in &self.toasts {
                    egui::Frame::NONE
                        .fill(theme::bg_panel())
                        .stroke(Stroke::new(1.0, theme::border()))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                                theme::led(ui.painter(), r.center(), t.color, true);
                                ui.add_space(4.0);
                                ui.label(&t.text);
                            });
                        });
                    ui.add_space(6.0);
                }
            });
    }

    fn render_tab(&mut self, ui: &mut egui::Ui, id: u64, actions: &mut Vec<Action>) {
        let kind = self.tabs.iter().find(|t| t.id == id).map(|t| match t.kind {
            TabKind::Term(_) => 0,
            TabKind::Sftp(_) => 1,
            TabKind::Rdp(_) => 2,
        });
        match kind {
            Some(0) => self.terminal_tab(ui, id, actions),
            Some(1) => crate::ui::sftp::show(self, ui, id),
            Some(_) => self.rdp_tab(ui, id),
            None => {}
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let (palette, close_tab, next_tab) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::K),
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::W,
                ),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
            )
        });
        if palette {
            actions.push(Action::OpenPalette);
        }
        // F1 anywhere, "?" when no text field or terminal has the keyboard.
        let nothing_focused = ctx.memory(|m| m.focused().is_none());
        let help = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::F1)
                || (nothing_focused
                    && i.events
                        .iter()
                        .any(|e| matches!(e, egui::Event::Text(t) if t == "?")))
        });
        if help {
            self.show_shortcuts = !self.show_shortcuts;
        }
        if close_tab && let Some(id) = self.active {
            self.close_tab(id);
        }
        // Ctrl+1…9 jumps to a port, Ctrl+0 back to the overview.
        let digits = [
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        for (n, key) in digits.into_iter().enumerate() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key)) {
                self.active = if n == 0 {
                    None
                } else {
                    self.tabs.get(n - 1).map(|t| t.id).or(self.active)
                };
            }
        }
        if next_tab && !self.tabs.is_empty() {
            let idx = self
                .active
                .and_then(|a| self.tabs.iter().position(|t| t.id == a));
            self.active = match idx {
                None => Some(self.tabs[0].id),
                Some(i) if i + 1 < self.tabs.len() => Some(self.tabs[i + 1].id),
                Some(_) => None,
            };
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_events(&ctx);

        if self.vault.is_none() {
            crate::ui::lock::show(self, ui);
            if self.vault.is_some() && !self.restored {
                self.restored = true;
                self.restore_sessions(&ctx);
            }
            // The idle timer starts again after unlocking.
            self.last_activity = Instant::now();
            self.toasts_ui(&ctx);
            return;
        }

        let mut actions = Vec::new();
        // Auto-lock after a period without any input; sessions keep running.
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_activity = Instant::now();
        }
        let minutes = self.lib.settings.auto_lock_minutes;
        if minutes > 0 {
            let limit = Duration::from_secs(minutes as u64 * 60);
            if self.last_activity.elapsed() >= limit {
                self.apply(Action::Lock, &ctx);
                self.toast(
                    "Automatisch gesperrt – Sessions laufen weiter",
                    theme::accent(),
                );
                ctx.request_repaint();
                return;
            }
            ctx.request_repaint_after(limit.saturating_sub(self.last_activity.elapsed()));
        }
        self.shortcuts(&ctx, &mut actions);
        crate::ui::chrome::rail(self, ui, &mut actions);
        let central = ui.available_rect_before_wrap();

        ctx.data_mut(|d| {
            d.insert_temp(
                crate::ui::chrome::split_info_id(),
                crate::ui::chrome::SplitInfo {
                    split: self.split.clone(),
                    maximized: self.maximized,
                    split_broadcast: self.split_broadcast,
                    envs: self.tabs.iter().map(|t| t.env).collect(),
                    tabs: self.tabs.iter().map(|t| (t.id, t.title.clone())).collect(),
                },
            )
        });
        if self.maximized.is_some() && self.maximized != self.active {
            self.maximized = None;
        }
        let split_view = self.split_view();
        match self.active {
            Some(_) if split_view => {
                let ids = self.split.clone();
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::border()))
                    .show(ui, |ui| {
                        let rects = crate::ui::chrome::split_rects(ui.max_rect(), ids.len());
                        for (id, rect) in ids.iter().copied().zip(rects) {
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .id_salt(("split-pane", id))
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            child.set_clip_rect(rect);
                            self.render_tab(&mut child, id, &mut actions);
                            let pressed = ui.input(|i| {
                                (i.pointer.primary_pressed() || i.pointer.secondary_pressed())
                                    && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
                            });
                            if pressed {
                                self.active = Some(id);
                            }
                            self.env_frame(ui.painter(), rect, id);
                            if self.active == Some(id) {
                                ui.painter().rect_stroke(
                                    rect,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.5, theme::accent().linear_multiply(0.8)),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }
                    });
            }
            Some(id) if self.tabs.iter().any(|t| t.id == id) => {
                self.render_tab(ui, id, &mut actions);
                self.env_frame(&ui.painter().clone(), central, id);
            }
            _ => {
                self.active = None;
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::bg_deep())
                            .inner_margin(Margin::symmetric(40, 30)),
                    )
                    .show(ui, |ui| match self.view {
                        View::Hosts => crate::ui::hosts::show(self, ui, &mut actions),
                        View::Files => crate::ui::sftp::page(self, ui, &mut actions),
                        View::Keychain => crate::ui::keychain::show(self, ui, &mut actions),
                        View::Snippets => crate::ui::snippets::show(self, ui, &mut actions),
                        View::Settings => crate::ui::settings::show(self, ui, &mut actions),
                    });
            }
        }

        self.drag_drop(&ctx, central, split_view);

        if let Some(op) = ctx.data_mut(|d| {
            let id = crate::ui::chrome::split_op_id();
            let op = d.get_temp::<crate::ui::chrome::SplitOp>(id);
            d.remove::<crate::ui::chrome::SplitOp>(id);
            op
        }) {
            self.apply_split(op);
        }

        if self.password_prompt.is_none()
            && let Some((purpose, host)) = self.connect_queue.pop_front()
        {
            self.connect_host(host, purpose, &ctx);
        }
        if self.password_prompt.is_none() && self.connect_queue.is_empty() {
            // Cancelled prompts must not pull later sessions into the split.
            self.group_split.clear();
        }

        crate::ui::editors::show_all(self, &ctx, &mut actions);
        crate::ui::palette::show(self, &ctx, &mut actions);
        crate::ui::dialogs::show(self, &ctx);
        self.toasts_ui(&ctx);

        for a in actions {
            self.apply(a, &ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.vault.is_some() {
            self.lib.settings.last_session = self.session_snapshot();
            self.save_library();
        }
        for tab in &self.tabs {
            match &tab.kind {
                TabKind::Term(t) => {
                    let _ = t.tx.send(TermCmd::Close);
                }
                TabKind::Sftp(t) => {
                    for r in t.panes.iter().filter_map(|p| p.remote.as_ref()) {
                        let _ = r.tx.send(SftpCmd::Close);
                    }
                }
                TabKind::Rdp(_) => {}
            }
        }
    }
}

/// Small pill at the bottom of a terminal while dropped files upload.
fn upload_badge(ui: &egui::Ui, name: &str, done: u64, total: u64) {
    let r = ui.max_rect();
    let text = if name.is_empty() {
        "Upload startet…".to_string()
    } else {
        format!(
            "↑ {name}  {} / {}",
            crate::ui::human_size(done),
            crate::ui::human_size(total)
        )
    };
    let painter = ui.painter();
    let g = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), theme::text());
    let size = g.size() + Vec2::new(24.0, 14.0);
    let pill = egui::Rect::from_min_size(
        egui::pos2(r.right() - size.x - 12.0, r.bottom() - size.y - 10.0),
        size,
    );
    painter.rect(
        pill,
        CornerRadius::same(255),
        theme::bg_card(),
        Stroke::new(1.0, theme::accent()),
        egui::StrokeKind::Inside,
    );
    let frac = if total > 0 {
        done as f32 / total as f32
    } else {
        0.0
    };
    let bar = egui::Rect::from_min_size(
        pill.left_bottom() + Vec2::new(12.0, -4.0),
        Vec2::new((pill.width() - 24.0) * frac, 2.0),
    );
    painter.rect_filled(bar, CornerRadius::same(1), theme::accent());
    painter.galley(pill.min + Vec2::new(12.0, 6.0), g, theme::text());
    ui.ctx().request_repaint();
}

/// Hint on a dead session.
fn closed_badge(ui: &egui::Ui) {
    let r = ui.max_rect();
    let painter = ui.painter();
    let g = painter.layout_no_wrap(
        "Getrennt · Enter verbindet neu".into(),
        egui::FontId::proportional(12.5),
        theme::text(),
    );
    let size = g.size() + Vec2::new(28.0, 14.0);
    let pill = egui::Rect::from_min_size(
        egui::pos2(r.center().x - size.x / 2.0, r.bottom() - size.y - 14.0),
        size,
    );
    painter.rect(
        pill,
        CornerRadius::same(255),
        theme::bg_card(),
        Stroke::new(1.0, theme::amber()),
        egui::StrokeKind::Inside,
    );
    painter.galley(pill.min + Vec2::new(14.0, 7.0), g, theme::text());
}

pub fn state_color(s: &ConnState) -> Color32 {
    match s {
        ConnState::Connecting(_) => theme::amber(),
        ConnState::Connected => theme::green(),
        ConnState::Closed(Some(_)) => theme::red(),
        ConnState::Closed(None) => theme::text_dim(),
    }
}

/// Parses `user@host:port` (user and port optional) into an ad-hoc SSH host.
fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Hosts from `~/.ssh/config`, if there is one.
pub fn ssh_config_hosts() -> Vec<Host> {
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return Vec::new();
    };
    let path = std::path::Path::new(&home).join(".ssh").join("config");
    std::fs::read_to_string(path)
        .map(|t| crate::model::parse_ssh_config(&t))
        .unwrap_or_default()
}

/// `user@host`, `host:port`, `ssh -p 2222 user@host` → an unsaved SSH host.
pub fn parse_quick(s: &str) -> Option<Host> {
    let s = s.trim();
    let s = s.strip_prefix("ssh ").unwrap_or(s).trim();
    let mut port_flag = None;
    let mut target = None;
    let mut words = s.split_whitespace();
    while let Some(w) = words.next() {
        if w == "-p" {
            port_flag = Some(words.next()?.parse::<u16>().ok()?);
        } else if target.is_none() && !w.starts_with('-') {
            target = Some(w);
        } else {
            return None;
        }
    }
    let s = target?;
    let (user, rest) = match s.rsplit_once('@') {
        Some((u, r)) => (u.to_string(), r),
        None => (String::new(), s),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().ok()?),
        _ => (rest.to_string(), 22),
    };
    if host.is_empty() {
        return None;
    }
    Some(Host {
        id: Uuid::new_v4(),
        hostname: host,
        port: port_flag.unwrap_or(port),
        username: user,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::parse_quick;

    #[test]
    fn quick_connect_parsing() {
        let h = parse_quick("root@10.0.0.1:2222").unwrap();
        assert_eq!(
            (h.username.as_str(), h.hostname.as_str(), h.port),
            ("root", "10.0.0.1", 2222)
        );
        let h = parse_quick("ssh example.org").unwrap();
        assert_eq!(
            (h.username.as_str(), h.hostname.as_str(), h.port),
            ("", "example.org", 22)
        );
        assert!(parse_quick("bad host").is_none());
        let h = parse_quick("ssh -p 2200 admin@db.local").unwrap();
        assert_eq!((h.username.as_str(), h.port), ("admin", 2200));
        assert!(parse_quick("u@h:notaport").is_none());
    }
}
