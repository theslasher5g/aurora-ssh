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
    Help,
    Proxmox,
    /// Docker/Kubernetes overview.
    Containers,
    /// Server inventory table.
    Inventory,
}

#[derive(Clone)]
pub enum ConnectSpec {
    Ssh(SshParams),
    /// A command in a terminal on the SSH host (container shell).
    SshExec(SshParams, String),
    Serial(SerialSettings),
    Telnet(crate::backend::telnet::TelnetParams),
    /// A program on this PC in a pseudo terminal (container shell).
    Local(Vec<String>),
    /// Plays a recorded session (`.cast` file).
    Replay(std::path::PathBuf),
    /// Shell on a Proxmox node through the API (termproxy), no SSH.
    PveShell(crate::backend::proxmox::cluster::term::ShellParams),
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
    /// Colors and font of this host's terminal.
    pub profile: crate::model::TermProfile,
    /// Shell command for a persistent (tmux) session; reused on reconnect.
    pub persist: Option<String>,
    /// Log viewer docked at the right.
    pub side_log: Option<SideLog>,
    /// Running recording of this session.
    pub recorder: Option<backend::record::Recorder>,
    /// Automatic reconnect in progress (after an unexpected drop).
    pub auto: Option<Box<crate::ui::reconnect_auto::Auto>>,
    /// The backend reported a lost connection for the session that just ended.
    pub dropped: bool,
    /// Automatic shell integration after login (per-host switch).
    pub si: crate::ui::blocks_setup::AutoSetup,
}

/// State of the log panel at the right of a terminal.
#[derive(Default)]
pub struct SideLog {
    /// The followed source; `None` shows the source list.
    pub log: Option<Box<LogTab>>,
    pub input: String,
    pub save_name: String,
    /// Copy the shown lines this frame.
    pub copy: bool,
    /// Panel width; 0 = default.
    pub width: f32,
}

/// Log viewer tab: follows a file or command and shows it filtered and highlighted.
pub struct LogTab {
    pub source: backend::logs::LogSource,
    /// SSH host that runs the source; `None` = this PC.
    pub target_host: Option<Uuid>,
    pub lines: VecDeque<String>,
    /// Lines dropped at the top because of the limit.
    pub dropped: usize,
    pub state: ConnState,
    pub rx: Option<UnboundedReceiver<backend::logs::LogEvent>>,
    pub stop: Option<UnboundedSender<()>>,
    pub filter: String,
    /// Show only matching lines (otherwise matches are highlighted).
    pub only_matches: bool,
    /// 0 all, 1 warnings and errors, 2 errors.
    pub level: u8,
    pub paused: bool,
    /// Lines that arrived while paused.
    pub paused_at: usize,
    pub follow: bool,
    pub wrap: bool,
}

/// The log of a log tab or of a terminal's side panel.
fn log_in(tabs: &mut [Tab], tab_id: u64) -> Option<&mut LogTab> {
    match &mut tabs.iter_mut().find(|t| t.id == tab_id)?.kind {
        TabKind::Log(t) => Some(t),
        TabKind::Term(t) => t.side_log.as_mut()?.log.as_deref_mut(),
        _ => None,
    }
}

impl LogTab {
    pub fn new(source: backend::logs::LogSource, target_host: Option<Uuid>) -> Self {
        Self {
            source,
            target_host,
            lines: VecDeque::new(),
            dropped: 0,
            state: ConnState::Connecting(t!("Verbinde…").into()),
            rx: None,
            stop: None,
            filter: String::new(),
            only_matches: true,
            level: 0,
            paused: false,
            paused_at: 0,
            follow: true,
            wrap: false,
        }
    }

    /// Takes the events that arrived from the source.
    pub fn pump(&mut self, title: &str, toasts: &mut Vec<(String, Color32)>) {
        use backend::logs::LogEvent;
        while let Some(ev) = self.rx.as_mut().and_then(|rx| rx.try_recv().ok()) {
            match ev {
                LogEvent::Status(s) => self.state = ConnState::Connecting(s),
                LogEvent::Connected => self.state = ConnState::Connected,
                LogEvent::Lines(lines) => {
                    if self.paused {
                        self.paused_at += lines.len();
                    }
                    self.lines.extend(lines);
                    let over = self
                        .lines
                        .len()
                        .saturating_sub(crate::ui::logview::MAX_LINES);
                    if over > 0 {
                        self.lines.drain(..over);
                        self.dropped += over;
                    }
                }
                LogEvent::Closed(err) => {
                    if let Some(e) = &err {
                        toasts.push((format!("{title}: {e}"), theme::red()));
                    }
                    self.state = ConnState::Closed(err);
                    self.stop = None;
                }
            }
        }
    }
}

pub struct VncTab {
    pub view: crate::ui::vnc::VncView,
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
    /// Folder a pane opens in once its server is connected.
    pub start_dir: [Option<String>; 2],
}

/// The terminal's folder as an SFTP path: `~` is the home folder, which SFTP paths
/// are relative to.
fn sftp_start_dir(dir: &str) -> String {
    match dir.strip_prefix('~') {
        Some("") => ".".into(),
        Some(rest) => match rest.strip_prefix('/') {
            Some(r) if !r.is_empty() => r.to_string(),
            Some(_) => ".".into(),
            // `~user/…` – let the server resolve it like a normal path.
            None => dir.to_string(),
        },
        None => dir.to_string(),
    }
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
                self.status = t!("Läuft in {}", p.client);
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
    Vnc(Box<VncTab>),
    Log(Box<LogTab>),
    Edit(Box<crate::ui::edit::EditTab>),
    /// Web interface in the `aurora-web` helper window.
    Web(Box<crate::ui::web::WebTab>),
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
    /// Attach the log source of this (already open) log tab.
    Log(u64),
    /// Open the container/pod waiting in `pending_containers` under this key.
    Container(u64),
    /// List containers/pods for the container browser.
    ListContainers,
    /// List the listening ports for the port scan dialog.
    ListPorts,
    /// Collect a diagnostics bundle (separate connection).
    Diag,
    /// Connection of the transfer queue.
    XferPool,
}

/// "Container durchsuchen" dialog.
pub struct ContainerBrowser {
    pub proto: Protocol,
    /// SSH host running docker/kubectl; `None` = this PC.
    pub via: Option<Uuid>,
    pub podman: bool,
    pub context: String,
    pub loading: bool,
    pub items: Vec<backend::container::Item>,
    pub error: Option<String>,
    pub filter: String,
    pub rx: Option<UnboundedReceiver<backend::container::ListEvent>>,
    /// Opened from the host editor: the pick fills in this host instead of connecting.
    pub for_editor: bool,
}

/// "Log ansehen" dialog for a host.
pub struct LogPicker {
    pub host: Option<Uuid>,
    pub source: String,
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
    /// "Aus Keychain wählen" list in the dialog.
    pub pick: crate::ui::pwmanager_pick::PromptPick,
}

pub struct SnippetRun {
    pub snippet: Snippet,
    pub values: Vec<(String, String)>,
    pub all: bool,
    /// Run as PowerShell in this RDP tab instead of pasting into terminals.
    pub rdp_tab: Option<u64>,
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
    /// Filter on the help page.
    pub help_filter: String,
    pub tour_step: usize,
    /// "What's new" sheet after an update.
    pub whatsnew: crate::ui::whatsnew_sheet::State,
    /// Theme import card in the settings.
    pub themes_ui: crate::ui::themeimport_ui::State,
    /// Terminal that gets the keyboard focus on the next frame.
    pub focus_term: Option<u64>,
    /// Split pane clicked; its terminal gets the focus unless the click focused a widget.
    pub pane_press: Option<u64>,
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
    pub containers: Option<ContainerBrowser>,
    /// Container hosts waiting for their SSH host's credentials (`Purpose::Container`).
    pub pending_containers: std::collections::HashMap<u64, Host>,
    /// Command line for a `Purpose::Container` session instead of the container shell.
    pub pending_exec: std::collections::HashMap<u64, String>,
    pub log_picker: Option<LogPicker>,
    /// Values for the `{{placeholders}}` of a log command about to start.
    pub log_vars: Option<crate::ui::logvars::LogVarPrompt>,
    /// "Ports scannen" dialog.
    pub port_scan: Option<crate::ui::portscan::PortScan>,
    pub tunnel_sink: Sink<backend::tunnel::TunnelEvent>,
    pub tunnel_rx: UnboundedReceiver<backend::tunnel::TunnelEvent>,
    pub next_id: u64,

    pub search: String,
    /// Hosts view filter: 0 all, 1 SSH, 2 RDP, 3 Serial, 4 favorites.
    pub host_filter: usize,
    pub snippet_search: String,
    pub keychain_search: String,
    /// Keys page: group editor sheet.
    pub keys_ui: crate::ui::keychain::KeysUi,
    pub broadcast: bool,
    /// Typing goes to every terminal shown in the split.
    pub split_broadcast: bool,
    /// Split pane temporarily shown alone.
    pub maximized: Option<u64>,
    /// Full-screen mode: no sidebar, the active session fills the window.
    pub focus: bool,
    /// Open tab on the settings page.
    pub settings_tab: usize,
    /// Pending window full-screen change for the viewport.
    pub ctx_fullscreen: Option<bool>,
    /// Sessions shown in their own window.
    pub popped: Vec<u64>,
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
    /// Password-manager integration (sessions, lookups, dialogs).
    pub pwm: crate::ui::pwmanager_ui::PwmState,
    pub snippet_run: Option<SnippetRun>,
    pub toasts: Vec<Toast>,
    pub serial_ports: Vec<String>,
    pub update: UpdateState,
    pub reach: crate::backend::reach::Reachability,
    pub update_sink: Sink<crate::update::UpdateEvent>,
    pub update_rx: UnboundedReceiver<crate::update::UpdateEvent>,
    /// Proxmox servers: live data, editor, certificate prompt.
    pub pve: crate::ui::proxmox::PveState,
    /// Command history, paste guard and time travel of the terminals.
    pub termx: crate::ui::history::TermExtras,
    /// Route finder, certificate radar, Wake-on-LAN.
    pub net: crate::ui::net::NetState,
    /// Runbooks, maintenance windows, diagnostics bundles.
    pub ops: crate::ui::ops::OpsState,
    /// Importer for other connection managers (mRemoteNG, Royal TS, RDM, CSV).
    pub import: crate::ui::import_ui::State,
    /// Large-inventory navigation (cache, cursor, scope sheets).
    pub nav: crate::ui::nav_state::NavState,
    /// Docker/Kubernetes overview page.
    pub ctr: crate::ui::containers_page::CtrState,
    /// Server inventory page.
    pub inv: crate::ui::inventory_page::InvState,
    /// Transfer queue.
    pub xfer: crate::ui::xfer::XferUi,
    /// Built-in editor: dialogs that belong to no single tab.
    pub editx: crate::ui::edit::EditGlobal,
}

/// UI intents collected while drawing and applied afterwards.
#[derive(Clone, Debug)]
pub enum Action {
    /// Show where a queued transfer went.
    XferShow(u64),
    /// "Datei öffnen…" for the active SSH terminal.
    EditPrompt,
    Connect(Uuid),
    /// Open the port scan dialog for a host.
    PortScan(Uuid),
    /// Replay a `.cast` recording in a terminal tab.
    PlayRecording(PathBuf),
    /// Marker, search, history and export of recordings.
    SessionLog(crate::ui::sessionlog_ui::Act),
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
    /// Groups on the keys page (see `ui::keychain::apply`).
    KeyGroup(crate::ui::keychain::KeyGroupOp),
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
    /// Open the "What's new" sheet.
    WhatsNew,
    /// Terminal with the user's shell on this PC.
    LocalTerminal,
    /// Open every host of a group, side by side.
    OpenGroup(Uuid),
    /// Snippet editor prefilled with this text.
    SnippetFromText(String),
    /// Type a command from the history into a session of this host (without Enter).
    TypeCommand(Uuid, String),
    /// Host editor for a quick-connect session.
    SaveQuickHost(u64),
    /// New host with this hostname (from a search without results).
    NewHostNamed(String),
    ImportSshConfig,
    /// Opens the importer for other connection managers.
    OpenImport,
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
    /// Container browser: (Docker/Kubernetes, SSH host or this PC, pick for the host editor).
    BrowseContainers(Protocol, Option<Uuid>, bool),
    /// Log dialog for a host (`None` = this PC).
    PickLog(Option<Uuid>),
    /// Open a log tab: (host or this PC, file or command).
    OpenLog(Option<Uuid>, String),
    /// Start a log right away (placeholders of a log command are already filled in).
    OpenLogNow(Option<Uuid>, String),
    /// Open a container/pod (not necessarily saved): shell or logs.
    OpenContainer(Box<Host>, bool),
    /// Proxmox page: servers, power actions, consoles.
    Pve(crate::ui::proxmox::PveAction),
    /// Route finder, certificate radar, Wake-on-LAN.
    Net(crate::ui::net::NetAction),
    /// Runbooks, maintenance windows, diagnostics bundles.
    Ops(crate::ui::ops::OpsAction),
    /// Container overview page.
    Ctr(crate::ui::containers_page::CtrAction),
    /// Server inventory page.
    Inv(crate::ui::inventory_page::InvAction),
    /// Scope, saved views, expansion, table settings of the host list.
    Nav(crate::ui::nav_state::NavAction),
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
        crate::themeimport_convert::register(&lib.settings.custom_themes);
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
            help_filter: String::new(),
            tour_step: 0,
            whatsnew: Default::default(),
            themes_ui: Default::default(),
            focus_term: None,
            pane_press: None,
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
            containers: None,
            pending_containers: Default::default(),
            pending_exec: Default::default(),
            log_picker: None,
            log_vars: None,
            port_scan: None,
            tunnel_sink,
            tunnel_rx,
            next_id: 1,
            search: String::new(),
            host_filter: 0,
            snippet_search: String::new(),
            keychain_search: String::new(),
            keys_ui: Default::default(),
            broadcast: false,
            split_broadcast: false,
            maximized: None,
            focus: false,
            settings_tab: 0,
            ctx_fullscreen: None,
            popped: Vec::new(),
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
            pwm: Default::default(),
            snippet_run: None,
            toasts: Vec::new(),
            serial_ports: backend::serial::available_ports(),
            update,
            reach: Default::default(),
            update_sink,
            update_rx,
            pve: crate::ui::proxmox::PveState::new(&cc.egui_ctx),
            termx: Default::default(),
            net: Default::default(),
            ops: Default::default(),
            import: Default::default(),
            nav: Default::default(),
            ctr: Default::default(),
            inv: Default::default(),
            xfer: crate::ui::xfer::XferUi::new(&cc.egui_ctx),
            editx: Default::default(),
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
        self.nav.bump();
        if let Err(e) = self.store.save_library(&self.lib) {
            self.toast(
                t!("Speichern fehlgeschlagen: {}", format!("{e:#}")),
                theme::red(),
            );
        }
    }

    pub fn save_keychain(&mut self) {
        if let Some(v) = &self.vault
            && let Err(e) = v.save(&self.keychain)
        {
            self.toast(
                t!("Keychain-Speichern fehlgeschlagen: {}", format!("{e:#}")),
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
        // Secrets fetched from a password manager for this connection.
        let (ident, item_user) = self.pwm.apply(host.id, ident);
        let user = if !host.username.is_empty() {
            host.username.clone()
        } else if let Some(u) = item_user.filter(|u| !u.is_empty()) {
            u
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

    pub fn host_color(&self, host: &Host) -> Color32 {
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
        // Containers on an SSH host: log in there (with its credentials) first.
        if host.protocol.is_container() && purpose == Purpose::Terminal {
            match host.container.via.and_then(|v| self.lib.host(v).cloned()) {
                Some(via) => {
                    let key = self.alloc_id();
                    self.pending_containers.insert(key, host);
                    self.connect_host(via, Purpose::Container(key), ctx);
                }
                None if host.container.via.is_some() => self.toast(
                    t!("Der SSH-Host dieses Containers existiert nicht mehr"),
                    theme::red(),
                ),
                None => self.open_session(&host, purpose, String::new(), None, None, ctx),
            }
            return;
        }
        if host.protocol == Protocol::Web && purpose == Purpose::Terminal {
            // One window per entry: its profile directory cannot be shared.
            if self.focus_web_tab(host.id) {
                return;
            }
        }
        if self.pwm_intercept(&host, purpose, ctx) {
            return;
        }
        let (user, ident) = self.credentials(&host);
        let has_secret = ident.as_ref().is_some_and(|i| {
            !i.password.is_empty() || (host.protocol == Protocol::Ssh && !i.private_key.is_empty())
        }) || (host.protocol == Protocol::Ssh
            && crate::backend::sshca::login_for(&host, &user, &self.keychain).is_some());
        let needs_secret = match host.protocol {
            Protocol::Ssh => true,
            Protocol::Rdp => host.rdp.embedded,
            // VNC asks once; an empty answer is fine for servers without a password.
            Protocol::Vnc => ident.is_none(),
            Protocol::Serial | Protocol::Telnet | Protocol::Docker | Protocol::Kubernetes => false,
            // Only needed to fill the login form.
            Protocol::Web => host.web.auto_login,
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
                pick: Default::default(),
            });
            return;
        }
        let password = ident
            .as_ref()
            .map(|i| i.password.clone())
            .filter(|p| !p.is_empty());
        self.open_session(&host, purpose, user, password, ident, ctx);
    }

    /// Terminal tab titled `title` that logs into the SSH host `via` (with its stored
    /// credentials or the password prompt) and runs `line` there – the container-session
    /// path with a custom command (Proxmox consoles).
    pub fn open_exec(&mut self, via: Host, title: String, line: String, ctx: &egui::Context) {
        let key = self.alloc_id();
        let tab_host = Host {
            id: Uuid::new_v4(),
            label: title,
            notes: String::new(),
            ..via.clone()
        };
        self.pending_containers.insert(key, tab_host);
        self.pending_exec.insert(key, line);
        self.connect_host(via, Purpose::Container(key), ctx);
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
        if let Purpose::Log(tab_id) = purpose {
            let params = ssh_params();
            self.start_log(tab_id, backend::container::Target::Ssh(params), ctx);
            return;
        }
        if purpose == Purpose::XferPool {
            crate::ui::xfer::pool_attach(self, host, ssh_params(), ctx);
            return;
        }
        if purpose == Purpose::Diag {
            crate::ui::diag::start(self, ssh_params(), ctx);
            return;
        }
        if purpose == Purpose::ListPorts {
            let params = ssh_params();
            crate::ui::portscan::start_listening(self, params, ctx);
            return;
        }
        if purpose == Purpose::ListContainers {
            let params = ssh_params();
            self.list_containers(backend::container::Target::Ssh(params), ctx);
            return;
        }
        if let Purpose::Container(key) = purpose {
            let params = ssh_params();
            let Some(c) = self.pending_containers.remove(&key) else {
                return;
            };
            let custom = self.pending_exec.remove(&key);
            let line = custom.clone().unwrap_or_else(|| {
                backend::container::shell_line(&backend::container::exec_argv(
                    c.protocol,
                    &c.container,
                ))
            });
            let spec = ConnectSpec::SshExec(params, line);
            let kind = if custom.is_some() {
                "Proxmox".to_string()
            } else {
                container_kind(&c)
            };
            let subtitle = format!("{kind} · {}", host.display_name());
            self.push_term_tab(&c, spec, subtitle, ctx);
            if custom.is_some()
                && let Some(t) = self.tabs.last_mut()
            {
                // A command on the SSH host, nothing to save as a host.
                t.unsaved = None;
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
        if host.protocol.is_container() {
            // On this PC (containers on SSH hosts come through `Purpose::Container`).
            let argv = backend::container::exec_argv(host.protocol, &host.container);
            let subtitle = format!("{} · {}", container_kind(host), t!("dieser PC"));
            self.push_term_tab(host, ConnectSpec::Local(argv), subtitle, ctx);
            return;
        }
        let kind = match (host.protocol, purpose) {
            (Protocol::Docker | Protocol::Kubernetes, _) => unreachable!(),
            (Protocol::Web, _) => {
                let tab = crate::ui::web::WebTab::open(host, &user, password.as_deref(), ctx);
                TabKind::Web(Box::new(tab))
            }
            (Protocol::Telnet, _) => {
                let spec = ConnectSpec::Telnet(backend::telnet::TelnetParams {
                    host: host.hostname.trim().to_string(),
                    port: host.port,
                    username: user.clone(),
                    password: password.clone(),
                });
                let mut term = self.spawn_term(spec, None, ctx);
                term.profile = host.term.clone();
                TabKind::Term(Box::new(term))
            }
            (Protocol::Vnc, _) => {
                let mut view = crate::ui::vnc::VncView::new(backend::vnc::VncParams {
                    host: host.hostname.trim().to_string(),
                    port: host.port,
                    password: password.clone(),
                    view_only: host.vnc.view_only,
                });
                view.start(&self.rt, ctx);
                TabKind::Vnc(Box::new(VncTab { view }))
            }
            (Protocol::Ssh, _) => {
                let spec = ConnectSpec::Ssh(ssh_params());
                // Each open tab to the host gets its own tmux session (lowest free slot).
                let persist = host.persistent.then(|| {
                    let used: Vec<String> = self
                        .tabs
                        .iter()
                        .filter_map(|t| match &t.kind {
                            TabKind::Term(term) if t.host_id == Some(host.id) => {
                                term.persist.clone()
                            }
                            _ => None,
                        })
                        .collect();
                    (1..)
                        .map(|slot| crate::model::persistent_shell(host.id, slot))
                        .find(|cmd| !used.contains(cmd))
                        .unwrap_or_default()
                });
                let startup = host
                    .startup_snippet
                    .and_then(|s| self.lib.snippet(s))
                    .filter(|s| s.variables().is_empty())
                    .map(|s| s.script.clone());
                let mut term = self.spawn_term_with(spec, startup, persist, ctx);
                term.profile = host.term.clone();
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
                let mut term = self.spawn_term(spec, None, ctx);
                term.profile = host.term.clone();
                TabKind::Term(Box::new(term))
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
            Protocol::Serial | Protocol::Web => host.subtitle(),
            _ => {
                let user = if user.is_empty() {
                    String::new()
                } else {
                    format!("{user}@")
                };
                format!("{user}{}:{}", host.hostname.trim(), host.port)
            }
        };
        let subtitle = crate::ui::route::with_route(&self.lib, host, subtitle);
        self.push_tab(id, host, subtitle, color, kind);
    }

    /// Adds a session tab for `host` and makes it active (or puts it into the split
    /// when its group is being opened side by side).
    fn push_tab(&mut self, id: u64, host: &Host, subtitle: String, color: Color32, kind: TabKind) {
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
            unsaved: (self.lib.host(host.id).is_none()
                && matches!(
                    host.protocol,
                    Protocol::Ssh | Protocol::Docker | Protocol::Kubernetes
                ))
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
        self.focus_term = Some(id);
    }

    /// Terminal tab with the user's shell on this PC.
    pub fn open_local_terminal(&mut self, ctx: &egui::Context) {
        let argv = backend::local::default_shell();
        let shell = std::path::Path::new(&argv[0])
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Not a saved host and nothing to save (same trick as the node shell).
        let host = Host {
            label: t!("Lokales Terminal").into(),
            hostname: "localhost".into(),
            protocol: Protocol::Telnet,
            ..Default::default()
        };
        let subtitle = format!("{shell} · {}", t!("dieser PC"));
        self.push_term_tab(&host, ConnectSpec::Local(argv), subtitle, ctx);
    }

    /// Terminal tab with a shell on a Proxmox node (termproxy over the API).
    pub fn open_pve_shell(
        &mut self,
        title: String,
        subtitle: String,
        params: crate::backend::proxmox::cluster::term::ShellParams,
        ctx: &egui::Context,
    ) {
        let host = Host {
            label: title,
            hostname: params.conn.host.clone(),
            protocol: Protocol::Telnet,
            ..Default::default()
        };
        self.push_term_tab(&host, ConnectSpec::PveShell(params), subtitle, ctx);
    }

    /// Terminal tab for a container shell (the host may be unsaved).
    fn push_term_tab(
        &mut self,
        host: &Host,
        spec: ConnectSpec,
        subtitle: String,
        ctx: &egui::Context,
    ) {
        if let Some(h) = self.lib.hosts.iter_mut().find(|h| h.id == host.id) {
            h.last_connected = Some(chrono::Utc::now());
            self.save_library();
        }
        let mut term = self.spawn_term(spec, None, ctx);
        term.profile = host.term.clone();
        let id = self.alloc_id();
        let color = self.host_color(host);
        self.push_tab(id, host, subtitle, color, TabKind::Term(Box::new(term)));
    }

    pub(crate) fn ssh_params(
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
            cert: crate::backend::sshca::login_for(host, user, &self.keychain),
        }
    }

    /// ProxyJump: the jump hosts' own connection parameters (stored credentials only),
    /// nested so each hop is reached through the one before it (up to 4, loops are cut).
    fn jump_params(&self, host: &Host) -> Option<Box<SshParams>> {
        let mut via: Option<Box<SshParams>> = None;
        for id in backend::route::jump_chain_lossy(&self.lib, host) {
            let jump = self.lib.host(id)?.clone();
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
            p.jump = via.take();
            via = Some(Box::new(p));
        }
        via
    }

    /// New file tab with two panes; `None` means the local disk.
    pub fn open_files(&mut self, left: Option<Uuid>, right: Option<Uuid>, ctx: &egui::Context) {
        let id = self.alloc_id();
        let home = crate::ui::sftp::home_dir();
        self.tabs.push(Tab {
            id,
            title: t!("Lokal").into(),
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
                start_dir: [None, None],
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
        let start = t.start_dir[idx].take().unwrap_or_default();
        t.panes[idx] = match conn {
            Some(c) => FilePane {
                remote: Some(c),
                path: start,
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
            t!("Lokal").into()
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
        self.spawn_term_with(spec, startup, None, ctx)
    }

    /// `persist`: run this command instead of the login shell (tmux session).
    fn spawn_term_with(
        &self,
        spec: ConnectSpec,
        startup: Option<String>,
        persist: Option<String>,
        ctx: &egui::Context,
    ) -> TermTab {
        let (sink, rx) = Sink::new(ctx.clone());
        let tx = match &spec {
            ConnectSpec::Ssh(p) => backend::ssh::spawn_shell(
                &self.rt,
                p.clone(),
                match &persist {
                    Some(cmd) => backend::ssh::ShellStart::Persistent(cmd.clone()),
                    None => backend::ssh::ShellStart::Login,
                },
                80,
                24,
                sink,
                self.prompts_sink.clone(),
            ),
            ConnectSpec::SshExec(p, cmd) => backend::ssh::spawn_shell(
                &self.rt,
                p.clone(),
                backend::ssh::ShellStart::Exec(cmd.clone()),
                80,
                24,
                sink,
                self.prompts_sink.clone(),
            ),
            ConnectSpec::Serial(s) => backend::serial::spawn(s.clone(), sink),
            ConnectSpec::Telnet(p) => backend::telnet::spawn(&self.rt, p.clone(), 80, 24, sink),
            ConnectSpec::Local(argv) => backend::local::spawn(argv.clone(), 80, 24, sink),
            ConnectSpec::Replay(path) => backend::record::spawn_replay(path.clone(), sink),
            ConnectSpec::PveShell(p) => {
                backend::proxmox::cluster::term::spawn(p.clone(), 80, 24, sink)
            }
        };
        TermTab {
            spec,
            term: Terminal::new(self.lib.settings.scrollback),
            tx,
            rx,
            state: ConnState::Connecting(t!("Verbinde…").into()),
            startup,
            show_snippets: false,
            snippet_filter: String::new(),
            cmd_started: None,
            last_output: Instant::now(),
            upload: None,
            run_once: None,
            profile: Default::default(),
            persist,
            side_log: None,
            recorder: None,
            auto: None,
            dropped: false,
            si: crate::ui::blocks_setup::AutoSetup::new(false),
        }
    }

    pub fn reconnect(&mut self, tab_id: u64, ctx: &egui::Context) {
        self.reconnect_inner(tab_id, ctx, false);
    }

    /// `auto`: an attempt of the automatic reconnect (keeps its state, no startup snippet on
    /// PROD hosts or in persistent sessions, no separator line per attempt).
    pub(crate) fn reconnect_inner(&mut self, tab_id: u64, ctx: &egui::Context, auto: bool) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return;
        };
        let env = self.tabs[idx].env;
        if let TabKind::Term(t) = &self.tabs[idx].kind {
            let mut spec = t.spec.clone();
            if let ConnectSpec::Ssh(p) = &mut spec {
                // A key accepted in this run is in the library, not in the old params.
                crate::ui::reconnect_auto::refresh_known(p, &self.lib.known_hosts);
            }
            let quiet = auto && (env == Env::Prod || t.persist.is_some());
            let startup = if quiet { None } else { t.startup.clone() };
            let mut fresh = self.spawn_term_with(spec, startup, t.persist.clone(), ctx);
            fresh.show_snippets = t.show_snippets;
            fresh.profile = t.profile.clone();
            if let TabKind::Term(old) = &mut self.tabs[idx].kind {
                fresh.side_log = old.side_log.take();
                fresh.recorder = old.recorder.take();
                if !matches!(old.spec, ConnectSpec::Replay(_)) {
                    // Keep the scrollback; leave full-screen programs of the dead session.
                    fresh.term = std::mem::replace(&mut old.term, Terminal::new(0));
                    if !auto {
                        fresh.term.reset_modes();
                        fresh.term.feed(b"\r\n\x1b[2m--- reconnect ---\x1b[0m\r\n");
                    }
                }
                if auto {
                    fresh.auto = old.auto.take();
                }
            }
            self.tabs[idx].kind = TabKind::Term(Box::new(fresh));
        } else if let TabKind::Vnc(t) = &mut self.tabs[idx].kind {
            t.view.start(&self.rt, ctx);
        } else if matches!(self.tabs[idx].kind, TabKind::Log(_)) {
            self.reopen_log(tab_id, ctx);
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

    // ------------------------------------------------------------------ logs & containers

    /// New log tab for a file/command on an SSH host (`None` = this PC).
    pub fn open_log(&mut self, host: Option<Uuid>, source: &str, ctx: &egui::Context) {
        let source = backend::logs::LogSource::parse(source);
        if source.text().is_empty() {
            return;
        }
        let id = self.alloc_id();
        let h = host.and_then(|h| self.lib.host(h)).cloned();
        let (name, color, env) = match &h {
            Some(h) => (
                h.display_name().to_string(),
                self.host_color(h),
                self.lib.env_of(h),
            ),
            None => (t!("Dieser PC").to_string(), theme::accent(), Env::None),
        };
        self.tabs.push(Tab {
            id,
            title: t!("Log · {}", name),
            subtitle: source.text().to_string(),
            color,
            attention: false,
            env,
            notes: String::new(),
            notes_open: false,
            unsaved: None,
            host_id: h.as_ref().map(|h| h.id),
            kind: TabKind::Log(Box::new(LogTab::new(source, h.map(|h| h.id)))),
        });
        self.active = Some(id);
        self.reopen_log(id, ctx);
    }

    /// (Re)starts the source of a log tab.
    fn reopen_log(&mut self, tab_id: u64, ctx: &egui::Context) {
        // A quick-connect host that is not saved lives only in its terminal tab.
        let unsaved: Vec<Host> = self.tabs.iter().filter_map(|t| t.unsaved.clone()).collect();
        // The log panel of an SSH terminal uses the session's login (no second prompt).
        let session = self
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .and_then(|t| match &t.kind {
                TabKind::Term(term) => match &term.spec {
                    ConnectSpec::Ssh(p) => Some(p.clone()),
                    _ => None,
                },
                _ => None,
            });
        let Some(t) = log_in(&mut self.tabs, tab_id) else {
            return;
        };
        t.stop = None;
        t.rx = None;
        if !t.lines.is_empty() {
            t.lines
                .push_back(format!("──── {} ────", t!("neu verbunden")));
        }
        t.state = ConnState::Connecting(t!("Verbinde…").into());
        if let Some(p) = session {
            self.start_log(tab_id, backend::container::Target::Ssh(p), ctx);
            return;
        }
        let host = t.target_host.and_then(|h| {
            self.lib
                .host(h)
                .cloned()
                .or_else(|| unsaved.into_iter().find(|u| u.id == h))
        });
        match host {
            Some(h) => self.connect_host(h, Purpose::Log(tab_id), ctx),
            None if t.target_host.is_some() => {
                t.state = ConnState::Closed(Some(t!("Host nicht mehr vorhanden").into()));
            }
            None => self.start_log(tab_id, backend::container::Target::Local, ctx),
        }
    }

    /// Starts following `source` in the log panel of terminal `tab_id` (on its host).
    pub fn open_side_log(&mut self, tab_id: u64, source: &str, ctx: &egui::Context) {
        let source = backend::logs::LogSource::parse(source);
        if source.text().is_empty() {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let host = tab.host_id;
        let TabKind::Term(t) = &mut tab.kind else {
            return;
        };
        let side = t.side_log.get_or_insert_with(SideLog::default);
        side.input = source.text().to_string();
        side.log = Some(Box::new(LogTab::new(source, host)));
        self.reopen_log(tab_id, ctx);
    }

    fn start_log(&mut self, tab_id: u64, target: backend::container::Target, ctx: &egui::Context) {
        let (sink, rx) = Sink::new(ctx.clone());
        let prompts = self.prompts_sink.clone();
        let Some(t) = log_in(&mut self.tabs, tab_id) else {
            return;
        };
        t.rx = Some(rx);
        t.stop = Some(backend::logs::spawn(
            &self.rt,
            target,
            t.source.clone(),
            sink,
            prompts,
        ));
    }

    /// Fetches the container/pod list for the browser dialog.
    pub fn refresh_containers(&mut self, ctx: &egui::Context) {
        let Some(b) = self.containers.as_mut() else {
            return;
        };
        b.loading = true;
        b.error = None;
        match b.via {
            None => self.list_containers(backend::container::Target::Local, ctx),
            Some(v) => match self.lib.host(v).cloned() {
                Some(h) => self.connect_host(h, Purpose::ListContainers, ctx),
                None => {
                    b.loading = false;
                    b.error = Some(t!("Host nicht mehr vorhanden").into());
                }
            },
        }
    }

    fn list_containers(&mut self, target: backend::container::Target, ctx: &egui::Context) {
        let Some(b) = self.containers.as_mut() else {
            return;
        };
        let (sink, rx) = Sink::new(ctx.clone());
        b.rx = Some(rx);
        let settings = crate::model::ContainerSettings {
            podman: b.podman,
            context: b.context.clone(),
            ..Default::default()
        };
        backend::container::spawn_list(
            &self.rt,
            b.proto,
            settings,
            target,
            sink,
            self.prompts_sink.clone(),
        );
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
            SplitOp::Layout(l) => {
                self.lib.settings.split_layout = l;
                self.save_library();
            }
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
            SplitOp::Focus(id) => {
                self.set_focus(id);
                return;
            }
            SplitOp::PopOut(id) => {
                if self.popped.contains(&id) {
                    self.popped.retain(|p| *p != id);
                    self.active = Some(id);
                    return;
                }
                self.popped.push(id);
                self.split.retain(|s| *s != id);
                if self.active == Some(id) {
                    self.active = self.split.first().copied().or_else(|| {
                        self.tabs
                            .iter()
                            .map(|t| t.id)
                            .find(|t| !self.popped.contains(t))
                    });
                }
            }
        }
        self.maximized = None;
        if self.split.len() < 2 {
            self.split.clear();
        }
    }

    /// Enters full screen with `id` (or leaves it with `None`).
    pub fn set_focus(&mut self, id: Option<u64>) {
        self.focus = id.is_some();
        if let Some(id) = id {
            self.active = Some(id);
            self.focus_term = Some(id);
        }
        self.ctx_fullscreen = Some(self.focus);
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
                    ks.error = Some(t!("Schlüssel nicht lesbar: {}", format!("{e:#}")));
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
            t!("{}: Anmeldung jetzt per SSH-Key", host.display_name()),
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
                t!("1 Host hinzugefügt – der Name kommt beim ersten Login").to_string()
            } else {
                t!("{} Hosts hinzugefügt – Namen kommen beim ersten Login", n)
            },
            theme::green(),
        );
    }

    /// Open sessions as (host, kind) for "restore sessions".
    fn session_snapshot(&self) -> Vec<crate::model::SavedSession> {
        self.tabs
            .iter()
            .filter_map(|t| {
                let mut source = String::new();
                let (host, kind) = match &t.kind {
                    TabKind::Term(term) => {
                        // The followed source of the log panel, reopened with the session.
                        if let Some(l) = term.side_log.as_ref().and_then(|s| s.log.as_ref()) {
                            source = l.source.text().to_string();
                        }
                        (t.host_id?, "term")
                    }
                    TabKind::Rdp(_) => (t.host_id?, "rdp"),
                    TabKind::Vnc(_) => (t.host_id?, "vnc"),
                    // A window pops up on start: not restored automatically.
                    TabKind::Edit(_) | TabKind::Web(_) => return None,
                    TabKind::Log(l) => {
                        source = l.source.text().to_string();
                        (l.target_host.unwrap_or(Uuid::nil()), "log")
                    }
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
                    source,
                    title: if kind == "log" {
                        t.title.clone()
                    } else {
                        String::new()
                    },
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
            .filter(|s| s.in_split && s.kind != "files" && s.kind != "log")
            .map(|s| s.host)
            .collect();
        let mut n = 0;
        for s in saved {
            if s.kind == "log" {
                let host = (!s.host.is_nil()).then_some(s.host);
                if host.is_none_or(|h| self.lib.host(h).is_some()) {
                    n += 1;
                    self.open_log(host, &s.source, ctx);
                    if let Some(tab) = self.tabs.last_mut()
                        && !s.title.is_empty()
                    {
                        tab.title = s.title.clone();
                    }
                }
                continue;
            }
            let Some(h) = self.lib.host(s.host).cloned() else {
                continue;
            };
            n += 1;
            if s.kind == "files" {
                self.open_files(None, Some(h.id), ctx);
            } else {
                let before = self.tabs.len();
                self.connect_host(h.clone(), Purpose::Terminal, ctx);
                if s.kind == "term"
                    && !s.source.is_empty()
                    && self.tabs.len() > before
                    && let Some(tab) = self.tabs.last()
                    && tab.host_id == Some(h.id)
                {
                    let id = tab.id;
                    self.open_side_log(id, &s.source, ctx);
                }
            }
        }
        if n > 0 {
            self.toast(
                t!("{} Sessions der letzten Sitzung wiederhergestellt", n),
                theme::accent(),
            );
        }
    }

    /// Writes hosts, groups, snippets, keys and known hosts into an encrypted file.
    pub fn export_data(&mut self) {
        if self.export_pw.chars().count() < 8 {
            self.toast(t!("Export-Passwort: mindestens 8 Zeichen"), theme::amber());
            return;
        }
        let bundle = crate::store::ExportBundle {
            version: 1,
            hosts: self.lib.hosts.clone(),
            groups: self.lib.groups.clone(),
            snippets: self.lib.snippets.clone(),
            identities: self.keychain.identities.clone(),
            known_hosts: self.lib.known_hosts.clone(),
            key_groups: self.keychain.groups.clone(),
            pve_servers: self.lib.pve_servers.clone(),
            pve_secrets: self.keychain.pve_secrets.clone(),
            pbs_servers: self.lib.pbs_servers.clone(),
            pbs_secrets: self.keychain.pbs_secrets.clone(),
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
                    t!("Exportiert nach {} – verschlüsselt", path.display()),
                    theme::green(),
                );
            }
            Err(e) => self.toast(
                t!("Export fehlgeschlagen: {}", format!("{e:#}")),
                theme::red(),
            ),
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
        let mut bundle = match r {
            Ok(b) => b,
            Err(e) => {
                self.toast(
                    t!("Import fehlgeschlagen: {}", format!("{e:#}")),
                    theme::red(),
                );
                return;
            }
        };
        let mut n = bundle.merge_proxmox(&mut self.lib, &mut self.keychain);
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
        for g in bundle.key_groups {
            if self.keychain.key_group(g.id).is_none() {
                self.keychain.groups.push(g);
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
        self.toast(t!("{} Einträge importiert", n), theme::green());
    }

    /// Coloured frame around PROD / STAGING sessions.
    /// Keeps the highlighted pane and the keyboard focus together: the pane holding the
    /// focused widget is the active one, and a click into a pane moves the focus there.
    fn sync_split_focus(&mut self, ctx: &egui::Context, ids: &[u64], rects: &[egui::Rect]) {
        let pane_of = |p: egui::Pos2| {
            ids.iter()
                .zip(rects)
                .find(|(_, r)| r.contains(p))
                .map(|(id, _)| *id)
        };
        if let Some(f) = self.focus_term
            && !self
                .tabs
                .iter()
                .any(|t| t.id == f && matches!(t.kind, TabKind::Term(_)))
        {
            self.focus_term = None;
        }
        let focused_pane = ctx
            .memory(|m| m.focused())
            .and_then(|f| ctx.read_response(f))
            .and_then(|r| pane_of(r.rect.center()));
        if ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        if let Some(pane) = self.pane_press.take()
            && focused_pane != Some(pane)
        {
            // Clicked on something that takes no focus (header, empty space).
            if self
                .tabs
                .iter()
                .any(|t| t.id == pane && matches!(t.kind, TabKind::Term(_)))
            {
                self.focus_term = Some(pane);
            }
            return;
        }
        if self.focus_term.is_none()
            && let Some(p) = focused_pane
            && self.active != Some(p)
        {
            self.active = Some(p);
        }
    }

    /// Production sessions keep a red frame (staging a faint amber one), so they stand
    /// out even when not focused. The active pane's accent frame is drawn over it.
    fn env_frame(&self, painter: &egui::Painter, rect: egui::Rect, id: u64) {
        let Some(env) = self.tabs.iter().find(|t| t.id == id).map(|t| t.env) else {
            return;
        };
        let stroke = match env {
            Env::Prod => Stroke::new(1.5, theme::env_color(env)),
            Env::Staging => Stroke::new(1.0, theme::env_color(env).gamma_multiply(0.6)),
            _ => return,
        };
        painter.rect_stroke(
            rect,
            CornerRadius::same(PANE_RADIUS as u8),
            stroke,
            egui::StrokeKind::Inside,
        );
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
        let layout = self.lib.settings.split_layout;
        let saved = self.lib.settings.split_ratios.clone();
        let order =
            pos.and_then(|p| chrome::drop_order(central, &shown, dragged, p, layout, &saved));

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
            let rects = chrome::split_rects_for(central, order.len(), layout, &saved);
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
                        t!("hier ablegen"),
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

    /// Raises the window of the open web tab of `host` and returns true; a closed tab of
    /// the host is removed (the caller opens a fresh one).
    fn focus_web_tab(&mut self, host: Uuid) -> bool {
        let found = self.tabs.iter().find_map(|t| match &t.kind {
            TabKind::Web(w) if t.host_id == Some(host) => Some((t.id, !w.is_closed())),
            _ => None,
        });
        match found {
            Some((id, true)) => {
                if let Some(Tab {
                    kind: TabKind::Web(w),
                    ..
                }) = self.tabs.iter().find(|t| t.id == id)
                {
                    w.send(crate::backend::web::WebCmd::Focus);
                }
                self.active = Some(id);
                true
            }
            Some((id, false)) => {
                self.close_tab(id);
                false
            }
            None => false,
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        if let Some(Tab {
            kind: TabKind::Edit(e),
            ..
        }) = self.tabs.iter().find(|t| t.id == id)
            && e.dirty
            && !e.force_close
        {
            self.editx.close_confirm = Some(id);
            self.active = Some(id);
            return;
        }
        self.popped.retain(|p| *p != id);
        let was_split = self.split.contains(&id);
        self.split.retain(|s| *s != id);
        // Closing a pane keeps the focus inside the split.
        let partner = self.split.first().copied().filter(|_| was_split);
        if self.split.len() < 2 {
            self.split.clear();
        }
        self.termx.slog.tab_closed(id);
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
                TabKind::Vnc(mut t) => t.view.close(),
                TabKind::Web(mut t) => t.close(),
                TabKind::Log(_) | TabKind::Rdp(_) | TabKind::Edit(_) => {}
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
        if self.focus && self.active.is_none() {
            self.set_focus(None);
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
            self.toast(
                t!("Keine verbundene Terminal-Session aktiv"),
                theme::amber(),
            );
        } else if sent > 1 {
            self.toast(t!("Snippet an {} Sessions gesendet", sent), theme::accent());
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
                                    t!("Aurora {} ist verfügbar", rel.version_text()),
                                    theme::accent(),
                                );
                            }
                            self.update.latest = Some(rel);
                        }
                        Ok(None) if self.update.manual => {
                            self.update.message =
                                Some((t!("Aurora ist aktuell.").into(), theme::green()));
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
                                t!("Installiert – Neustart aktiviert die neue Version.").into(),
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
        crate::ui::proxmox::pump(self);
        crate::ui::containers_page::pump(self, ctx);
        crate::ui::inventory_page::pump(self, ctx);
        while let Ok(p) = self.prompts_rx.try_recv() {
            self.hostkey_queue.push_back(p);
        }
        let mut toasts = Vec::new();
        let mut prompts = Vec::new();
        let visible = self.visible_tabs();
        let focused = ctx.input(|i| i.focused);
        let mut notify = false;
        let mut edit_close: Vec<u64> = Vec::new();
        let mut learned: Vec<(Uuid, crate::model::HostFacts)> = Vec::new();
        let mut outcome: Vec<(Uuid, bool)> = Vec::new();
        let (auto_global, auto_minutes) = (
            self.lib.settings.auto_reconnect,
            self.lib.settings.auto_reconnect_minutes,
        );
        let record_all = self.lib.settings.record_sessions;
        let time_travel = self.lib.settings.time_travel;
        // Hosts in a maintenance window: no toasts, attention or "failed" LED.
        let muted = crate::ui::maintenance::muted_hosts(&self.lib);
        for tab in &mut self.tabs {
            let seen = focused && visible.contains(&tab.id);
            let quiet = tab.host_id.is_some_and(|h| muted.contains(&h));
            let before = (toasts.len(), notify, tab.attention);
            match &mut tab.kind {
                TabKind::Term(t) => {
                    if let Some(log) = t.side_log.as_mut().and_then(|s| s.log.as_mut()) {
                        log.pump(&tab.title, &mut toasts);
                    }
                    while let Ok(ev) = t.rx.try_recv() {
                        match ev {
                            TermEvent::Status(s) => {
                                let a = theme::accent();
                                // Automatic attempts keep the scrollback free of status lines.
                                if t.auto.is_none() {
                                    t.term.feed(
                                        format!(
                                            "\x1b[38;2;{};{};{}m›\x1b[0m {s}\r\n",
                                            a.r(),
                                            a.g(),
                                            a.b()
                                        )
                                        .as_bytes(),
                                    );
                                }
                                t.state = ConnState::Connecting(s);
                            }
                            TermEvent::Dropped => t.dropped = true,
                            TermEvent::Facts(f) => {
                                if let Some(h) = tab.host_id {
                                    learned.push((h, f));
                                }
                            }
                            TermEvent::Connected => {
                                t.state = ConnState::Connected;
                                crate::ui::reconnect_auto::on_connected(t);
                                if let ConnectSpec::Replay(p) = &t.spec {
                                    self.termx.slog.replay_start(tab.id, p);
                                } else {
                                    self.termx
                                        .slog
                                        .session_start(tab.id, tab.host_id, &tab.title);
                                }
                                let want_si = tab
                                    .host_id
                                    .and_then(|h| self.lib.host(h))
                                    .is_some_and(|h| h.shell_integration);
                                if want_si
                                    && matches!(t.spec, ConnectSpec::Ssh(_))
                                    && t.persist.is_none()
                                    && t.startup.is_none()
                                    && t.run_once.is_none()
                                {
                                    t.si = crate::ui::blocks_setup::AutoSetup::new(true);
                                }
                                if record_all
                                    && t.recorder.is_none()
                                    && matches!(t.spec, ConnectSpec::Ssh(_))
                                {
                                    let (cols, rows) = t.term.size();
                                    match backend::record::Recorder::start_for(
                                        &tab.title,
                                        cols,
                                        rows,
                                        tab.host_id,
                                    ) {
                                        Ok(r) => {
                                            self.termx.slog.set_rec(tab.id, Some(r.path.clone()));
                                            t.recorder = Some(r)
                                        }
                                        Err(e) => toasts.push((
                                            t!("Aufnahme fehlgeschlagen: {}", e),
                                            theme::red(),
                                        )),
                                    }
                                }
                                if let Some(h) = tab.host_id {
                                    outcome.push((h, true));
                                }
                                let (cols, rows) = t.term.size();
                                let _ = t.tx.send(TermCmd::Resize { cols, rows });
                                if let Some(cmd) = t.run_once.take() {
                                    // Pre-typed commands wait for the user's Enter.
                                    let line = match cmd.strip_prefix(crate::ui::sshca::PRETYPE) {
                                        Some(c) => c.to_string(),
                                        None => format!("{cmd}\r"),
                                    };
                                    let _ = t.tx.send(TermCmd::Input(line.into_bytes()));
                                }
                                if let Some(s) = &t.startup {
                                    let mut s = s.replace('\n', "\r");
                                    s.push('\r');
                                    let _ = t.tx.send(TermCmd::Input(s.into_bytes()));
                                }
                            }
                            TermEvent::Output(b) => {
                                // The automatic shell setup hides its own exchange.
                                let b = if t.si.swallowing() { t.si.absorb(b) } else { b };
                                if b.is_empty() {
                                    t.last_output = Instant::now();
                                    continue;
                                }
                                self.termx.record_output(time_travel, tab.id, &t.term, &b);
                                if let Some(r) = t.recorder.as_mut() {
                                    r.output(&b);
                                }
                                crate::ui::runbooks::capture(&mut self.ops, tab.id, &b);
                                t.term.feed(&b);
                                let reply = t.term.take_replies();
                                if !reply.is_empty() {
                                    let _ = t.tx.send(TermCmd::Input(reply));
                                }
                                t.last_output = Instant::now();
                            }
                            TermEvent::UploadProgress { name, done, total } => {
                                t.upload = Some((name, done, total));
                            }
                            TermEvent::UploadDone(r) => {
                                t.upload = None;
                                toasts.push(match r {
                                    Ok(dir) => (t!("Hochgeladen nach {}", dir), theme::green()),
                                    Err(e) => (t!("Upload fehlgeschlagen: {}", e), theme::red()),
                                });
                            }
                            TermEvent::Closed(err) => {
                                let mode = tab
                                    .host_id
                                    .and_then(|h| self.lib.host(h))
                                    .map(|h| h.auto_reconnect)
                                    .unwrap_or_default();
                                let on = backend::reconnect_policy::enabled(auto_global, mode);
                                if crate::ui::reconnect_auto::handle_closed(
                                    t,
                                    &err,
                                    on,
                                    auto_minutes,
                                ) {
                                    if !seen {
                                        toasts.push((
                                            t!(
                                                "{}: Verbindung verloren – verbinde automatisch neu",
                                                tab.title
                                            ),
                                            theme::amber(),
                                        ));
                                    }
                                    continue;
                                }
                                let replay = matches!(t.spec, ConnectSpec::Replay(_));
                                let msg = match &err {
                                    Some(e) => format!("\r\n\x1b[31m✖ {e}\x1b[0m\r\n"),
                                    None if replay => format!(
                                        "\r\n\x1b[2m{}\x1b[0m\r\n",
                                        t!("Ende der Aufnahme.")
                                    ),
                                    None => format!(
                                        "\r\n\x1b[2m{}\x1b[0m\r\n",
                                        t!("Verbindung beendet.")
                                    ),
                                };
                                t.term.feed(msg.as_bytes());
                                if let Some(e) = &err {
                                    toasts.push((format!("{}: {e}", tab.title), theme::red()));
                                } else if !seen && !replay {
                                    toasts.push((
                                        t!(
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
                                self.termx.slog.session_end(tab.id);
                                t.state = ConnState::Closed(err);
                                t.cmd_started = None;
                                t.upload = None;
                            }
                        }
                    }
                    if t.si.waiting() || t.si.swallowing() {
                        if t.si.waiting() && t.term.blocks().osc {
                            // The server's shell already sends the marks.
                            let _ = t.si.cancel();
                        }
                        let ready = matches!(t.state, ConnState::Connected)
                            && t.last_output.elapsed() > Duration::from_millis(450)
                            && t.term.at_prompt();
                        let flush = t.si.tick(ready);
                        if !flush.is_empty() {
                            t.term.feed(&flush);
                        }
                        for m in t.si.outbox.drain(..) {
                            let _ = t.tx.send(TermCmd::Input(m));
                        }
                        ctx.request_repaint_after(Duration::from_millis(200));
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
                                    t!(
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
                TabKind::Vnc(t) => {
                    let phase = |s: &ConnState| match s {
                        ConnState::Connecting(_) => 0,
                        ConnState::Connected => 1,
                        ConnState::Closed(_) => 2,
                    };
                    let before = phase(&t.view.state);
                    for text in t.view.pump() {
                        toasts.push((text, theme::accent()));
                    }
                    match (before, &t.view.state) {
                        (0, ConnState::Connected) => {
                            if let Some(h) = tab.host_id {
                                outcome.push((h, true));
                            }
                        }
                        (0 | 1, ConnState::Closed(Some(e))) => {
                            toasts.push((format!("{}: {e}", tab.title), theme::red()));
                            if before == 0
                                && let Some(h) = tab.host_id
                            {
                                outcome.push((h, false));
                            }
                        }
                        _ => {}
                    }
                }
                TabKind::Log(t) => t.pump(&tab.title, &mut toasts),
                TabKind::Web(t) => t.pump(&tab.title, &mut toasts),
                TabKind::Edit(_) => {
                    let r = crate::ui::edit::pump(tab, &self.rt, &self.prompts_sink, ctx);
                    toasts.extend(r.toasts);
                    if r.close {
                        edit_close.push(tab.id);
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
                            t!("RDP-Sitzung beendet").into()
                        } else {
                            t!("RDP-Client beendet ({})", status)
                        };
                    }
                }
            }
            if quiet {
                toasts.truncate(before.0);
                (notify, tab.attention) = (before.1, before.2);
            }
        }
        self.hostkey_queue.extend(prompts);
        outcome.retain(|(h, ok)| *ok || !muted.contains(h));
        for (h, ok) in outcome {
            if ok {
                self.failed.remove(&h);
            } else {
                self.failed.insert(h);
                crate::ui::net::on_connect_failed(self, h, ctx);
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
                        t!("{} heißt jetzt „{}“", host.hostname, f.hostname),
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
                            t!("Tunnel „{}“ offen: localhost:{}", t.label(), t.local_port),
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
        if let Some(b) = self.containers.as_mut()
            && let Some(Ok(backend::container::ListEvent::Done(r))) =
                b.rx.as_mut().map(|rx| rx.try_recv())
        {
            b.loading = false;
            b.rx = None;
            match r {
                Ok(items) => {
                    b.items = items;
                    b.error = None;
                }
                Err(e) => b.error = Some(e),
            }
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
        for id in edit_close {
            if let Some(Tab {
                kind: TabKind::Edit(e),
                ..
            }) = self.tabs.iter_mut().find(|t| t.id == id)
            {
                e.force_close = true;
            }
            self.close_tab(id);
        }
        crate::ui::xfer::pump(self, ctx);
    }

    // ------------------------------------------------------------------ actions

    pub fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::BrowseContainers(proto, via, for_editor) => {
                let (podman, context) = match self.host_editor.as_ref().filter(|_| for_editor) {
                    Some(ed) => (ed.host.container.podman, ed.host.container.context.clone()),
                    None => (false, String::new()),
                };
                self.containers = Some(ContainerBrowser {
                    proto,
                    via,
                    podman,
                    context,
                    loading: false,
                    items: Vec::new(),
                    error: None,
                    filter: String::new(),
                    rx: None,
                    for_editor,
                });
                self.refresh_containers(ctx);
            }
            Action::PickLog(host) => {
                let container = host
                    .and_then(|h| self.lib.host(h))
                    .filter(|h| h.protocol.is_container())
                    .cloned();
                if let Some(h) = container {
                    self.apply(Action::OpenContainer(Box::new(h), true), ctx);
                } else {
                    let source = if host.is_some() { "journalctl -f" } else { "" };
                    self.log_picker = Some(LogPicker {
                        host,
                        source: source.into(),
                    });
                }
            }
            Action::PlayRecording(path) => self.play_recording(path, ctx),
            Action::SessionLog(a) => crate::ui::sessionlog_ui::handle(self, a, ctx),
            Action::PortScan(id) => {
                self.port_scan = crate::ui::portscan::PortScan::new(self, id);
            }
            Action::OpenLog(host, source) => {
                self.log_picker = None;
                match crate::ui::logvars::LogVarPrompt::new(
                    &source,
                    crate::ui::logvars::Target::Tab(host),
                    &self.lib.log_var_history,
                ) {
                    Some(p) => self.log_vars = Some(p),
                    None => self.open_log(host, &source, ctx),
                }
            }
            Action::OpenLogNow(host, source) => self.open_log(host, &source, ctx),
            Action::OpenContainer(host, logs) => {
                if logs {
                    let argv = backend::container::logs_argv(host.protocol, &host.container);
                    self.open_log(
                        host.container.via,
                        &backend::container::shell_line(&argv),
                        ctx,
                    );
                    if let Some(tab) = self.tabs.last_mut() {
                        tab.title = t!("Log · {}", host.display_name());
                    }
                } else {
                    self.connect_host(*host, Purpose::Terminal, ctx);
                }
            }
            Action::Connect(id) => {
                if let Some(h) = self.lib.host(id).cloned() {
                    self.connect_host(h, Purpose::Terminal, ctx);
                }
            }
            Action::XferShow(id) => crate::ui::xfer::show_in_folder(self, id, ctx),
            Action::EditPrompt => self.edit_prompt(),
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
                    copy.label = t!("{} (Kopie)", h.display_name());
                    copy.last_connected = None;
                    self.lib.hosts.push(copy);
                    self.save_library();
                }
            }
            Action::DeleteHost(id) => {
                crate::backend::web::forget_host_data(&self.lib.hosts, &[id]);
                self.lib.hosts.retain(|h| h.id != id);
                self.lib.detail.remove(&id);
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
                let alive: std::collections::HashSet<Uuid> =
                    self.lib.groups.iter().map(|g| g.id).collect();
                self.lib.nav.prune(|g| alive.contains(&g));
                self.save_library();
            }
            Action::ToggleGroup(id) => crate::ui::nav_state::toggle_group(self, id),
            Action::Nav(a) => crate::ui::nav_state::apply(self, a, ctx),
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
            Action::KeyGroup(op) => crate::ui::keychain::apply(self, op),
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
                            rdp_tab: None,
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
            Action::WhatsNew => crate::ui::whatsnew_sheet::open(self),
            Action::LocalTerminal => self.open_local_terminal(ctx),
            Action::SetTheme(id) => {
                theme::apply(ctx, &id);
                self.lib.settings.theme = id;
                self.save_library();
            }
            Action::Confirm(text, inner) => self.confirm_delete = Some((text, *inner)),
            Action::Pve(a) => crate::ui::proxmox::apply(self, a, ctx),
            Action::Net(a) => crate::ui::net::apply(self, a, ctx),
            Action::Ops(a) => crate::ui::ops::apply(self, a, ctx),
            Action::Ctr(a) => crate::ui::containers_page::apply(self, a, ctx),
            Action::Inv(a) => crate::ui::inventory_page::apply(self, a, ctx),
            Action::OpenGroup(gid) => {
                let hosts: Vec<Host> = self
                    .lib
                    .hosts_in_group(gid)
                    .into_iter()
                    .filter(|h| h.protocol != Protocol::Serial)
                    .take(crate::ui::chrome::MAX_SPLIT)
                    .collect();
                if hosts.is_empty() {
                    self.toast(t!("Die Gruppe enthält keine Hosts"), theme::amber());
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
            Action::OpenImport => self.import.open(),
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
                        0 => t!("Keine neuen Hosts in ~/.ssh/config").to_string(),
                        1 => t!("1 Host importiert").to_string(),
                        n => t!("{} Hosts importiert", n),
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
                self.toast(t!("{} Hosts in „LAN {}“", n, prefix), theme::green());
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
                crate::backend::web::forget_host_data(&self.lib.hosts, &ids);
                self.lib.hosts.retain(|h| !ids.contains(&h.id));
                for id in &ids {
                    self.lib.facts.remove(id);
                    self.lib.detail.remove(id);
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
            Action::TypeCommand(h, cmd) => crate::ui::history::type_command(self, h, cmd, ctx),
            Action::Lock => {
                self.termx.flush(self.vault.as_ref(), true);
                self.vault = None;
                self.keychain = Keychain::default();
                self.lock = LockScreen::default();
                self.host_editor = None;
                self.identity_editor = None;
                self.password_prompt = None;
                self.pwm.lock();
                self.pve.lock();
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
        let host_id = tab.host_id;
        let host_name = tab
            .title
            .split(" · ")
            .next()
            .unwrap_or_default()
            .to_string();
        let log_commands: Vec<(Uuid, String, String)> = self
            .lib
            .log_commands
            .iter()
            .filter(|c| c.host.is_none() || c.host == host_id)
            .map(|c| (c.id, c.name.clone(), c.command.clone()))
            .collect();
        let mut toggle_log = false;
        let mut toggle_record = false;
        let mut toggle_markers = false;
        let mut add_marker = false;
        let markers_open = self.termx.slog.sess.get(&tab_id).is_some_and(|s| s.panel);
        let mut block_notice: Option<String> = None;
        let mut open_files = false;
        let mut side_action = None;
        let maint = host_id.and_then(|h| {
            crate::ui::maintenance::active_for(&self.lib, h, chrono::Utc::now()).cloned()
        });
        let runbooks: Vec<(Uuid, String)> = (self.lib.runbooks.iter())
            .map(|r| (r.id, r.title.clone()))
            .collect();
        let TabKind::Term(t) = &mut tab.kind else {
            return;
        };
        let ssh_session = matches!(t.spec, ConnectSpec::Ssh(_)) && host_id.is_some();
        let log_open = t.side_log.is_some();
        let recording = t.recorder.is_some();
        let rewinding = self.termx.rewind_views.contains_key(&tab_id);
        let mut toggle_rewind = false;
        let mut command = None;

        let state = crate::ui::chrome::state_pair(&t.state);
        let closed = matches!(t.state, ConnState::Closed(_));
        let mut toggle_snippets = false;
        let snippets_open = t.show_snippets;
        crate::ui::chrome::session_header(
            ui,
            egui::Id::new(("term-bar", tab_id)),
            index,
            &title,
            &subtitle,
            state,
            |ui| {
                if theme::icon_toggle(
                    ui,
                    crate::ui::icons::Icon::Bolt,
                    t!("Snippets ein-/ausblenden"),
                    snippets_open,
                )
                .clicked()
                {
                    toggle_snippets = true;
                }
                crate::ui::runbooks::header_button(ui, &mut self.ops, &runbooks, tab_id, actions);
                if theme::icon_button(ui, "🔍", t!("Im Terminal suchen (Ctrl+Shift+F)")).clicked()
                {
                    search = true;
                }
                if has_notes && theme::icon_button(ui, "📝", t!("Notizen zum Host")).clicked() {
                    toggle_notes = true;
                }
                if ssh_session {
                    if theme::icon_toggle(
                        ui,
                        crate::ui::icons::Icon::Logs,
                        t!("Logs rechts neben dem Terminal (Ctrl+Shift+L)"),
                        log_open,
                    )
                    .clicked()
                    {
                        toggle_log = true;
                    }
                    if theme::icon_btn(
                        ui,
                        crate::ui::icons::Icon::Edit,
                        t!("Datei im Aurora-Editor öffnen…"),
                    )
                    .clicked()
                    {
                        actions.push(Action::EditPrompt);
                    }
                    if theme::icon_btn(
                        ui,
                        crate::ui::icons::Icon::Folder,
                        t!("Dateien im aktuellen Ordner des Terminals öffnen"),
                    )
                    .clicked()
                    {
                        open_files = true;
                    }
                }
                if theme::icon_toggle(
                    ui,
                    crate::ui::icons::Icon::Flag,
                    t!("Marker und Notizen zur Sitzung (Ctrl+Shift+M)"),
                    markers_open,
                )
                .clicked()
                {
                    toggle_markers = true;
                }
                if settings.time_travel
                    && theme::icon_toggle(
                        ui,
                        crate::ui::icons::Icon::Clock,
                        t!("Zurückspulen (Zeitreise)"),
                        rewinding,
                    )
                    .clicked()
                {
                    toggle_rewind = true;
                }
                if !matches!(t.spec, ConnectSpec::Replay(_)) {
                    let tip = if recording {
                        t!("Aufnahme beenden")
                    } else {
                        t!("Session aufzeichnen (asciinema)")
                    };
                    let r = theme::icon_toggle(ui, crate::ui::icons::Icon::Record, tip, recording);
                    if recording {
                        // Red while recording, like a camera.
                        crate::ui::icons::paint(
                            ui.painter(),
                            egui::Rect::from_center_size(r.rect.center(), Vec2::splat(17.0)),
                            crate::ui::icons::Icon::Record,
                            theme::red(),
                        );
                    }
                    if r.clicked() {
                        toggle_record = true;
                    }
                }
                if unsaved
                    && ui
                        .button(t!("Als Host speichern"))
                        .on_hover_text(t!("Diese Schnellverbindung in die Hostliste übernehmen"))
                        .clicked()
                {
                    save_host = true;
                }
                if closed && theme::icon_button(ui, "⟳", t!("Neu verbinden (Enter)")).clicked() {
                    reconnect = true;
                }
            },
        );
        if toggle_snippets {
            t.show_snippets = !t.show_snippets;
        }
        if toggle_rewind {
            self.termx.toggle_rewind(tab_id, &t.spec);
        }
        if toggle_markers && let Some(s) = self.termx.slog.sess.get_mut(&tab_id) {
            s.panel = !s.panel;
        }
        let mut marker_panel = None;
        if let Some(s) = self.termx.slog.sess.get_mut(&tab_id)
            && s.panel
        {
            let replay = matches!(t.spec, ConnectSpec::Replay(_));
            let can_add = !replay || self.termx.rewind_views.contains_key(&tab_id);
            egui::Panel::right(egui::Id::new(("term-markers", tab_id)))
                .resizable(false)
                .exact_size(300.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::bg_panel())
                        .inner_margin(Margin::symmetric(14, 14)),
                )
                .show(ui, |ui| {
                    marker_panel = Some(crate::ui::sessionlog_ui::panel(ui, tab_id, s, can_add));
                });
            if let Some(po) = &marker_panel {
                if po.close {
                    s.panel = false;
                }
                if let Some(kind) = po.export
                    && let Some(path) = s.file().cloned()
                {
                    actions.push(Action::SessionLog(crate::ui::sessionlog_ui::Act::Export(
                        path, kind,
                    )));
                }
            }
        }
        if let Some(po) = marker_panel {
            add_marker |= po.add;
            if let Some((mt, at)) = po.seek {
                let target = match at {
                    Some(at) => self.termx.rewinds.get(&tab_id).map(|rw| rw.time_at(at)),
                    None => Some(mt),
                };
                if let Some(target) = target {
                    if !self.termx.rewind_views.contains_key(&tab_id) {
                        self.termx.toggle_rewind(tab_id, &t.spec);
                    }
                    if let Some(v) = self.termx.rewind_views.get_mut(&tab_id) {
                        v.t = target;
                        v.playing = false;
                    }
                }
            }
        }
        if let Some(m) = &maint
            && let Some(a) =
                crate::ui::maintenance::session_banner(ui, egui::Id::new(("term-maint", tab_id)), m)
        {
            actions.push(Action::Ops(a));
        }
        if ssh_session
            && drop_target
            && ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::L)
            })
        {
            toggle_log = true;
        }
        if toggle_log {
            t.side_log = match t.side_log.take() {
                Some(_) => None,
                None => Some(SideLog::default()),
            };
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
                        .fill(theme::lerp(theme::surface(), theme::amber(), 0.10))
                        .inner_margin(Margin::symmetric(14, 4)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_btn(ui, crate::ui::icons::Icon::Close, t!("Ausblenden"))
                                .clicked()
                            {
                                notes_open = false;
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    crate::ui::icons::show(
                                        ui,
                                        crate::ui::icons::Icon::File,
                                        15.0,
                                        theme::amber(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&notes).size(13.0).color(theme::text()),
                                        )
                                        .wrap(),
                                    );
                                },
                            );
                        });
                    });
                });
        }
        if save_host {
            actions.push(Action::SaveQuickHost(tab_id));
        }
        if let Some(a) = t.auto.as_mut() {
            crate::ui::reconnect_auto::banner(ui, tab_id, a);
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
                            .hint_text(t!("Filtern"))
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let f = t.snippet_filter.to_lowercase();
                        if snippets.is_empty() {
                            ui.label(
                                RichText::new(t!("Noch keine Snippets")).color(theme::text_dim()),
                            );
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
                                .on_hover_text(t!("Klicken zum Ausführen"))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                actions.push(Action::RunSnippet(*id, false));
                            }
                        }
                    });
                });
        }

        if let Some(side) = t.side_log.as_mut() {
            // Fixed width with an own drag handle: the log content must not widen the panel.
            let room = ui.available_width();
            let max = (room - 260.0).max(280.0);
            if side.width <= 0.0 {
                side.width = (room * 0.42).clamp(320.0, 620.0);
            }
            side.width = side.width.clamp(280.0, max);
            let panel = egui::Panel::right(egui::Id::new(("term-log", tab_id)))
                .resizable(false)
                .exact_size(side.width)
                .frame(egui::Frame::NONE.fill(theme::bg_panel()))
                .show(ui, |ui| {
                    side_action = crate::ui::logview::side_panel(
                        ui,
                        egui::Id::new(("side-log", tab_id)),
                        side,
                        &host_name,
                        &log_commands,
                    );
                });
            let r = panel.response.rect;
            let grip = egui::Rect::from_x_y_ranges(r.left() - 4.0..=r.left() + 4.0, r.y_range());
            let drag = ui.interact(
                grip,
                egui::Id::new(("term-log-grip", tab_id)),
                egui::Sense::drag(),
            );
            if drag.dragged() {
                side.width -= drag.drag_delta().x;
            }
            if drag.hovered() || drag.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let line = if drag.hovered() || drag.dragged() {
                theme::accent()
            } else {
                theme::line()
            };
            ui.painter()
                .vline(r.left(), r.y_range(), egui::Stroke::new(1.0, line));
        }
        if let Some(d) = crate::ui::runbooks::terminal_panel(&mut self.ops, ui, tab_id, t, guarded)
        {
            danger = Some(d);
        }

        // Host terminal profile over the app settings.
        let font_name = if t.profile.font.trim().is_empty() {
            settings.term_font.clone()
        } else {
            t.profile.font.clone()
        };
        let family = theme::term_font(&ctx, &font_name);
        let palette = (!t.profile.theme.is_empty()).then(|| theme::find(&t.profile.theme));
        let font_size = if t.profile.font_size >= 6.0 {
            t.profile.font_size
        } else {
            settings.font_size
        };
        let term_bg = palette.unwrap_or_else(theme::palette).term_bg;
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
                        t!("Hochladen geht nur bei SSH-Sessions").to_string()
                    } else if !live {
                        t!("Nicht verbunden").to_string()
                    } else {
                        t!("Loslassen zum Hochladen nach {}", shown_dir)
                    };
                    Some(drop_text.as_str())
                } else {
                    match &t.state {
                        ConnState::Connecting(_) if t.term.contents().trim().is_empty() => {
                            Some(t!("Verbinde…"))
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
                let view = TermView {
                    font_size,
                    family,
                    palette,
                    accent,
                    copy_on_select: settings.copy_on_select,
                    glow: None,
                    overlay,
                    ctrl_r: ssh_session && settings.command_history,
                    blocks: settings.command_blocks,
                };
                let marks = self.termx.slog.timeline_marks(
                    tab_id,
                    self.termx.rewinds.get(&tab_id),
                    matches!(t.spec, ConnectSpec::Replay(_)),
                );
                let term_rect = ui.available_rect_before_wrap();
                let mut out = match self.termx.rewind_views.get_mut(&tab_id) {
                    Some(v) => {
                        let id = egui::Id::new(("rewind", tab_id));
                        let shown = crate::ui::timetravel::show(
                            ui,
                            id,
                            self.termx.rewinds.get(&tab_id),
                            v,
                            view,
                            &marks,
                        );
                        if shown.live {
                            self.termx.rewind_views.remove(&tab_id);
                            ui.memory_mut(|m| m.request_focus(egui::Id::new(("term", tab_id))));
                        }
                        // The past screen keeps the grid of its time.
                        crate::terminal::TermOutput {
                            resize: None,
                            ..shown.out
                        }
                    }
                    None => t.term.show(ui, egui::Id::new(("term", tab_id)), view),
                };
                if out.history_search {
                    let typed = t.term.prompt_command().unwrap_or_default();
                    crate::ui::history::open_popup(&mut self.termx, tab_id, &typed);
                }
                if let Some(st) = self.termx.popups.get_mut(&tab_id) {
                    let entries = host_id
                        .map(|h| self.termx.history.entries(h))
                        .unwrap_or_default();
                    match crate::ui::history::popup(&ctx, tab_id, term_rect, st, entries) {
                        crate::ui::history::PopupOut::Keep => {}
                        crate::ui::history::PopupOut::Close => {
                            self.termx.popups.remove(&tab_id);
                            ui.memory_mut(|m| m.request_focus(egui::Id::new(("term", tab_id))));
                        }
                        crate::ui::history::PopupOut::Insert(cmd, run) => {
                            if st.clear_line {
                                // Ctrl+U: drop what was typed before Ctrl+R.
                                out.input.push(0x15);
                            }
                            out.input.extend_from_slice(cmd.as_bytes());
                            // A dangerous command on PROD waits for a real Enter (and the
                            // confirmation that comes with it).
                            let risky = guarded && crate::model::dangerous_command(&cmd).is_some();
                            if run && !risky {
                                out.input.push(b'\r');
                                command = Some((cmd, t.term.cwd()));
                            }
                            self.termx.popups.remove(&tab_id);
                            ui.memory_mut(|m| m.request_focus(egui::Id::new(("term", tab_id))));
                        }
                    }
                }
                if let Some(cmd) = out.rerun.take()
                    && t.term.at_shell_prompt()
                {
                    // Type the command again. On PROD (and with "warn everywhere") Enter
                    // stays with the user, who then gets the usual danger dialog.
                    out.input.push(0x15);
                    out.input.extend_from_slice(cmd.as_bytes());
                    if !guarded {
                        out.input.push(b'\r');
                        command = Some((cmd, t.term.cwd()));
                    }
                }
                if let Some(n) = out.notice.take() {
                    block_notice = Some(n);
                }
                add_marker |= out.marker;
                if let Some(text) = out.paste.take() {
                    let check = settings.paste_guard && !matches!(t.state, ConnState::Closed(_));
                    match check
                        .then(|| {
                            crate::ui::paste_guard::check(
                                &text,
                                tab_id,
                                guarded,
                                t.term.bracketed_paste(),
                            )
                        })
                        .flatten()
                    {
                        Some(p) => self.termx.paste = Some(p),
                        None => out.input.extend(t.term.paste_bytes(&text)),
                    }
                }
                focused_now = ui.memory(|m| m.has_focus(egui::Id::new(("term", tab_id))));
                if let Some((cols, rows)) = out.resize {
                    let _ = t.tx.send(TermCmd::Resize { cols, rows });
                    if let Some(r) = t.recorder.as_mut() {
                        r.resize(cols, rows);
                    }
                }
                if let Some((name, done, total)) = &t.upload {
                    upload_badge(ui, name, *done, *total);
                }
                if matches!(t.state, ConnState::Closed(_)) {
                    closed_badge(ui, matches!(t.spec, ConnectSpec::Replay(_)));
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
                        // Remember the command line Enter runs (history, snippet hints).
                        if ssh_session
                            && command.is_none()
                            && let Some(pos) = out.input.iter().position(|&b| b == b'\r')
                            && out.input[..pos].iter().all(|&b| b >= 0x20 && b != 0x7f)
                            && let Some(mut line) = t.term.prompt_command()
                        {
                            line.push_str(&String::from_utf8_lossy(&out.input[..pos]));
                            command = Some((line, t.term.cwd()));
                        }
                        if out.input.contains(&b'\r') {
                            // Command blocks without shell integration: the prompt heuristic
                            // needs to know which line Enter runs.
                            let typed = command.as_ref().map(|c| c.0.clone()).or_else(|| {
                                let pos = out.input.iter().position(|&b| b == b'\r')?;
                                let head = &out.input[..pos];
                                head.iter()
                                    .all(|&b| b >= 0x20 && b != 0x7f)
                                    .then(|| {
                                        let mut line = t.term.prompt_command()?;
                                        line.push_str(&String::from_utf8_lossy(head));
                                        Some(line)
                                    })
                                    .flatten()
                            });
                            t.term.note_enter(typed.as_deref());
                        }
                        input = Some(out.input);
                    }
                }
                if let Some(text) = out.save_snippet {
                    actions.push(Action::SnippetFromText(text));
                }
            });
        let cwd = t.term.cwd();
        let session_params = match &t.spec {
            ConnectSpec::Ssh(p) => Some(p.clone()),
            _ => None,
        };
        let mut record_note = None;
        if toggle_record {
            if let Some(r) = t.recorder.take() {
                let path = r.path.clone();
                drop(r);
                record_note = Some((
                    t!("Aufnahme gespeichert: {}", path.display()),
                    theme::green(),
                ));
            } else {
                let (cols, rows) = t.term.size();
                match backend::record::Recorder::start_for(&title, cols, rows, host_id) {
                    Ok(mut r) => {
                        r.output(&t.term.snapshot());
                        self.termx.slog.set_rec(tab_id, Some(r.path.clone()));
                        t.recorder = Some(r);
                    }
                    Err(e) => {
                        record_note = Some((t!("Aufnahme fehlgeschlagen: {}", e), theme::red()))
                    }
                }
            }
        }
        let mut marker_failed = false;
        if add_marker {
            let replay_t = self.termx.rewind_views.get(&tab_id).map(|v| v.t);
            marker_failed = !self
                .termx
                .slog
                .add_marker(tab_id, t.recorder.as_mut(), replay_t);
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.notes_open = notes_open;
        }
        if let Some((text, color)) = record_note {
            self.toast(text, color);
        }
        if let Some(n) = block_notice {
            self.toast(n, theme::accent());
        }
        if marker_failed {
            self.toast(
                t!("Marker in einer Aufnahme: erst die Zeitreise öffnen und eine Stelle wählen"),
                theme::amber(),
            );
        }
        if let Some((cmd, why)) = danger {
            self.danger = Some((tab_id, cmd, why));
        }
        if let (Some(h), Some((cmd, cwd))) = (host_id, command) {
            crate::ui::history::on_command(self, h, &cmd, cwd);
        }
        if refocus && focused_now {
            self.focus_term = None;
        }
        if open_files && let (Some(h), Some(p)) = (host_id, session_params) {
            self.open_files_at(h, p, cwd, &ctx);
        }
        if let Some(a) = side_action {
            self.side_log_action(tab_id, a, host_id, &ctx);
        }

        if let Some(bytes) = input {
            self.send_input(tab_id, bytes);
        }
        if reconnect {
            self.reconnect(tab_id, &ctx);
        }
    }

    fn side_log_action(
        &mut self,
        tab_id: u64,
        a: crate::ui::logview::SideAction,
        host: Option<Uuid>,
        ctx: &egui::Context,
    ) {
        use crate::ui::logview::SideAction;
        fn side(app: &mut App, tab_id: u64) -> Option<&mut SideLog> {
            match &mut app.tabs.iter_mut().find(|t| t.id == tab_id)?.kind {
                TabKind::Term(t) => t.side_log.as_mut(),
                _ => None,
            }
        }
        match a {
            SideAction::Close => {
                if let Some(TabKind::Term(t)) = self
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == tab_id)
                    .map(|t| &mut t.kind)
                {
                    t.side_log = None;
                }
            }
            SideAction::Back => {
                if let Some(s) = side(self, tab_id) {
                    s.log = None;
                }
            }
            SideAction::Reconnect => self.reopen_log(tab_id, ctx),
            SideAction::Start(cmd) => match crate::ui::logvars::LogVarPrompt::new(
                &cmd,
                crate::ui::logvars::Target::Side(tab_id),
                &self.lib.log_var_history,
            ) {
                Some(p) => self.log_vars = Some(p),
                None => self.open_side_log(tab_id, &cmd, ctx),
            },
            SideAction::Save(name, command) => {
                // Saved for this host only when it is in the host list.
                let host = host.filter(|h| self.lib.host(*h).is_some());
                self.lib.log_commands.push(crate::model::LogCommand {
                    name,
                    command,
                    host,
                    ..Default::default()
                });
                self.save_library();
                self.toast(t!("Log-Befehl gespeichert"), theme::green());
            }
            SideAction::Delete(id) => {
                self.lib.log_commands.retain(|c| c.id != id);
                self.save_library();
            }
        }
    }

    /// Replays a recording in a new terminal tab (Space pauses, reconnect starts over).
    pub fn play_recording(&mut self, path: PathBuf, ctx: &egui::Context) {
        let name = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let term = self.spawn_term(ConnectSpec::Replay(path), None, ctx);
        let id = self.alloc_id();
        self.tabs.push(Tab {
            id,
            title: t!("Aufnahme · {}", name),
            subtitle: t!("Leertaste pausiert").to_string(),
            color: theme::accent(),
            attention: false,
            env: Env::None,
            notes: String::new(),
            notes_open: false,
            unsaved: None,
            host_id: None,
            kind: TabKind::Term(Box::new(term)),
        });
        self.active = Some(id);
    }

    /// Asks for a file path and opens it in the editor, on the host of the active SSH terminal.
    pub fn edit_prompt(&mut self) {
        let Some(tab) = self
            .active
            .and_then(|a| self.tabs.iter().find(|t| t.id == a))
        else {
            return;
        };
        let TabKind::Term(t) = &tab.kind else {
            self.toast(t!("Dafür ein SSH-Terminal auswählen"), theme::amber());
            return;
        };
        let ConnectSpec::Ssh(p) = &t.spec else {
            self.toast(t!("Dafür ein SSH-Terminal auswählen"), theme::amber());
            return;
        };
        self.editx.open_prompt = Some(crate::ui::edit::OpenPrompt {
            host: tab.host_id,
            label: tab.title.clone(),
            params: p.clone(),
            cwd: t.term.cwd(),
            color: tab.color,
            env: tab.env,
            input: String::new(),
            error: None,
            focus: true,
        });
    }

    /// Opens the file browser for `host` in `dir` (the terminal's folder, may start with `~`).
    fn open_files_at(
        &mut self,
        host: Uuid,
        params: SshParams,
        dir: Option<String>,
        ctx: &egui::Context,
    ) {
        if dir.is_none() {
            self.toast(
                t!("Ordner unbekannt – die Shell meldet ihn nicht, öffne den Home-Ordner"),
                theme::amber(),
            );
        }
        self.open_files(None, None, ctx);
        let Some(tab) = self.tabs.last_mut() else {
            return;
        };
        let tab_id = tab.id;
        if let TabKind::Sftp(t) = &mut tab.kind {
            t.start_dir[1] = dir.map(|d| sftp_start_dir(&d));
        }
        // Same login as the terminal: no second password prompt.
        let (label, color) = match self.lib.host(host) {
            Some(h) => (h.display_name().to_string(), self.host_color(h)),
            None => (params.host.clone(), theme::accent()),
        };
        let conn = crate::ui::sftp::spawn_conn(
            &self.rt,
            params,
            label,
            Some(host),
            ctx,
            self.prompts_sink.clone(),
        );
        self.set_pane(tab_id, 1, Some(conn), color);
    }

    fn vnc_tab(&mut self, ui: &mut egui::Ui, tab_id: u64) {
        let index = self.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let (title, subtitle) = (tab.title.clone(), tab.subtitle.clone());
        let TabKind::Vnc(t) = &mut tab.kind else {
            return;
        };
        let state = crate::ui::chrome::state_pair(&t.view.state);
        let closed = matches!(t.view.state, ConnState::Closed(_));
        let mut reconnect = false;
        crate::ui::chrome::session_header(
            ui,
            egui::Id::new(("vnc-bar", tab_id)),
            index,
            &title,
            &subtitle,
            state,
            |ui| {
                t.view.toolbar(ui);
                if closed && theme::icon_button(ui, "⟳", t!("Neu verbinden")).clicked() {
                    reconnect = true;
                }
            },
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::bg_deep()))
            .show(ui, |ui| {
                t.view.show(ui, egui::Id::new(("vnc", tab_id)));
            });
        if reconnect {
            let ctx = ui.ctx().clone();
            self.reconnect(tab_id, &ctx);
        }
    }

    /// Starts a PowerShell script on the Windows host of RDP tab `tab_id`.
    pub fn run_powershell(&mut self, tab_id: u64, script: &str) {
        let result = match self
            .tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .map(|t| &mut t.kind)
        {
            Some(TabKind::Rdp(t)) => match t.embedded.as_mut() {
                Some(v) => v.run_powershell(script),
                None => Err(t!("Nicht verbunden").into()),
            },
            _ => Err(t!("Nicht verbunden").into()),
        };
        if let Err(e) = result {
            self.toast(e, theme::amber());
        }
    }

    fn rdp_tab(&mut self, ui: &mut egui::Ui, tab_id: u64) {
        let ps_snippets: Vec<(Uuid, String)> = self
            .lib
            .snippets
            .iter()
            .filter(|s| s.shell == crate::model::ShellKind::PowerShell)
            .map(|s| (s.id, s.name.clone()))
            .collect();
        let index = self.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
        // Takes OS file drops when the pointer position is unknown (a popped-out window
        // gets its own drops).
        let drop_target = self.active == Some(tab_id) || self.popped.contains(&tab_id);
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
                &ps_snippets,
                drop_target,
                &mut toasts,
            );
            match action {
                crate::ui::rdp::RdpAction::OpenExternal => t.launch_external(),
                crate::ui::rdp::RdpAction::RunSnippet(id) => {
                    if let Some(s) = self.lib.snippet(id).cloned() {
                        let vars = s.variables();
                        if vars.is_empty() {
                            if let Err(e) = view.run_powershell(&s.script) {
                                toasts.push((e, theme::amber()));
                            }
                        } else {
                            self.snippet_run = Some(SnippetRun {
                                values: vars.into_iter().map(|v| (v, String::new())).collect(),
                                snippet: s,
                                all: false,
                                rdp_tab: Some(tab_id),
                            });
                        }
                    }
                }
                crate::ui::rdp::RdpAction::None => {}
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
                    if theme::icon_button(ui, "⏹", t!("Beenden")).clicked() {
                        stop = true;
                    }
                } else if theme::icon_button(ui, "⟳", t!("Erneut starten")).clicked() {
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
                        t!("Läuft im System-Client")
                    } else {
                        t!("Fenster geschlossen")
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

    /// Sessions popped out into their own native windows.
    fn popout_windows(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        self.popped
            .retain(|id| self.tabs.iter().any(|t| t.id == *id));
        for id in self.popped.clone() {
            let Some(title) = self
                .tabs
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.title.clone())
            else {
                continue;
            };
            let vid = egui::ViewportId::from_hash_of(("aurora-popout", id));
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{title} – Aurora"))
                .with_inner_size([960.0, 640.0])
                .with_min_inner_size([480.0, 320.0]);
            let mut back = false;
            ctx.show_viewport_immediate(vid, builder, |ui, _class| {
                if ui.input(|i| i.viewport().close_requested()) {
                    back = true;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::bg_deep()))
                    .show(ui, |ui| self.render_tab(ui, id, actions));
            });
            if back {
                self.popped.retain(|p| *p != id);
                self.active = Some(id);
            }
        }
    }

    /// Stand-in for a session that lives in its own window.
    fn popped_placeholder(&mut self, ui: &mut egui::Ui, id: u64) {
        let r = ui.max_rect();
        ui.painter()
            .rect_filled(r, CornerRadius::ZERO, theme::surface());
        let c = r.center();
        crate::ui::icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(c - Vec2::new(0.0, 46.0), Vec2::splat(36.0)),
            crate::ui::icons::Icon::External,
            theme::text_dim(),
        );
        ui.painter().text(
            c - Vec2::new(0.0, 6.0),
            egui::Align2::CENTER_CENTER,
            t!("Läuft in eigenem Fenster"),
            theme::semibold(15.0),
            theme::text(),
        );
        let br = egui::Rect::from_center_size(c + Vec2::new(0.0, 36.0), Vec2::new(160.0, 34.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(br)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        if theme::btn(&mut child, theme::Btn::Normal, None, t!("Zurückholen")).clicked() {
            self.popped.retain(|p| *p != id);
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
                        .fill(theme::lerp(theme::surface(), theme::text(), 0.08))
                        .stroke(Stroke::new(1.0, theme::line()))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(16, 12))
                        .shadow(egui::Shadow {
                            offset: [0, 10],
                            blur: 30,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(90),
                        })
                        .show(ui, |ui| {
                            ui.set_max_width(400.0);
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
            TabKind::Vnc(_) => 3,
            TabKind::Log(_) => 4,
            TabKind::Edit(_) => 5,
            TabKind::Web(_) => 6,
        });
        match kind {
            Some(0) => self.terminal_tab(ui, id, actions),
            Some(1) => crate::ui::sftp::show(self, ui, id),
            Some(2) => self.rdp_tab(ui, id),
            Some(3) => self.vnc_tab(ui, id),
            Some(4) => crate::ui::logview::show(self, ui, id),
            Some(5) => crate::ui::edit::show(self, ui, id),
            Some(6) => crate::ui::web::show(self, ui, id, actions),
            _ => {}
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let before = self.active;
        let (palette, close_tab, next_tab, local) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::K),
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::W,
                ),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::T,
                ),
            )
        });
        if palette {
            actions.push(Action::OpenPalette);
        }
        if local {
            actions.push(Action::LocalTerminal);
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
            if self.active.is_none() && self.view == View::Help {
                actions.push(Action::Navigate(View::Hosts));
            } else {
                actions.push(Action::Navigate(View::Help));
            }
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
        if self.active != before {
            // The keyboard follows the session picked with a shortcut.
            self.focus_term = self.active;
        }
    }
}

impl App {
    fn ui_frame(&mut self, ui: &mut egui::Ui) {
        sync_window_icon(ui.ctx());
        let _perf = crate::nav_perf::frame();
        let ctx = ui.ctx().clone();
        self.pump_events(&ctx);
        self.tick_auto_reconnect(&ctx);

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
                    t!("Automatisch gesperrt – Sessions laufen weiter"),
                    theme::accent(),
                );
                ctx.request_repaint();
                return;
            }
            ctx.request_repaint_after(limit.saturating_sub(self.last_activity.elapsed()));
        }
        self.shortcuts(&ctx, &mut actions);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F11)) {
            if self.focus {
                self.set_focus(None);
            } else if let Some(a) = self.active {
                self.set_focus(Some(a));
            }
        }
        if self.focus
            && !self
                .active
                .is_some_and(|a| self.tabs.iter().any(|t| t.id == a))
        {
            self.set_focus(None);
        }
        if let Some(fs) = self.ctx_fullscreen.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fs));
        }
        if !self.focus {
            crate::ui::chrome::rail(self, ui, &mut actions);
        }
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
                    live: self.tabs.iter().map(tab_live).collect(),
                    focus: self.focus,
                    popped: self.popped.clone(),
                    layout: self.lib.settings.split_layout,
                },
            )
        });
        if self.maximized.is_some() && self.maximized != self.active {
            self.maximized = None;
        }
        let split_view = self.split_view() && !self.focus;
        match self.active {
            Some(id) if self.focus && self.tabs.iter().any(|t| t.id == id) => {
                self.render_tab(ui, id, &mut actions);
            }
            Some(id) if split_view || self.tabs.iter().any(|t| t.id == id) => {
                let ids = if split_view {
                    self.split.clone()
                } else {
                    vec![id]
                };
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::bg_deep())
                            .inner_margin(Margin::same(PANE_GAP as i8)),
                    )
                    .show(ui, |ui| {
                        let layout = self.lib.settings.split_layout;
                        let (rects, dividers) = crate::ui::chrome::split_geometry(
                            ui.max_rect(),
                            ids.len(),
                            layout,
                            &self.lib.settings.split_ratios,
                        );
                        for (id, rect) in ids.iter().copied().zip(rects.iter().copied()) {
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .id_salt(("split-pane", id))
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            child.set_clip_rect(rect);
                            if self.popped.contains(&id) {
                                self.popped_placeholder(&mut child, id);
                            } else {
                                self.render_tab(&mut child, id, &mut actions);
                            }
                            let pressed = ui.input(|i| {
                                (i.pointer.primary_pressed() || i.pointer.secondary_pressed())
                                    && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
                            });
                            if pressed {
                                self.active = Some(id);
                                self.pane_press = Some(id);
                            }
                        }
                        // Dividers between the panes: drag to resize, double-click to reset.
                        let mut moved = false;
                        for (k, d) in dividers.iter().enumerate() {
                            let resp = ui.interact(
                                d.hit,
                                egui::Id::new(("split-divider", ids.len(), layout as u8, k)),
                                egui::Sense::click_and_drag(),
                            );
                            if resp.hovered() || resp.dragged() {
                                ctx.set_cursor_icon(if d.vertical {
                                    egui::CursorIcon::ResizeHorizontal
                                } else {
                                    egui::CursorIcon::ResizeVertical
                                });
                                let line = if d.vertical {
                                    egui::vec2(3.0, d.hit.height())
                                } else {
                                    egui::vec2(d.hit.width(), 3.0)
                                };
                                ui.painter().rect_filled(
                                    egui::Rect::from_center_size(d.hit.center(), line),
                                    CornerRadius::same(2),
                                    theme::accent().gamma_multiply(0.8),
                                );
                            }
                            let key = crate::ui::chrome::ratio_key(layout, ids.len());
                            if resp.dragged()
                                && let Some(p) = resp.interact_pointer_pos()
                            {
                                let v = self
                                    .lib
                                    .settings
                                    .split_ratios
                                    .entry(key.clone())
                                    .or_default();
                                if v.len() <= d.idx {
                                    v.resize(d.idx + 1, 0.0);
                                }
                                v[d.idx] = crate::ui::chrome::ratio_at(d, p);
                            } else if resp.double_clicked()
                                && let Some(v) = self.lib.settings.split_ratios.get_mut(&key)
                                && d.idx < v.len()
                            {
                                v[d.idx] = 0.0;
                                moved = true;
                            }
                            if resp.drag_stopped() {
                                moved = true;
                            }
                        }
                        if moved {
                            self.save_library();
                        }
                        if ids.len() >= 2 {
                            self.sync_split_focus(&ctx, &ids, &rects);
                        }
                        // Corners and frames last, on top of the panes.
                        let painter = ui.painter().clone();
                        for (id, rect) in ids.iter().copied().zip(rects) {
                            theme::round_corners(&painter, rect, PANE_RADIUS, theme::bg_deep());
                            let accent_frame = ids.len() >= 2 && self.active == Some(id);
                            if !accent_frame {
                                self.env_frame(&painter, rect, id);
                            }
                            if accent_frame {
                                painter.rect_stroke(
                                    rect,
                                    CornerRadius::same(PANE_RADIUS as u8),
                                    Stroke::new(2.0, theme::accent()),
                                    egui::StrokeKind::Inside,
                                );
                            } else {
                                painter.rect_stroke(
                                    rect,
                                    CornerRadius::same(PANE_RADIUS as u8),
                                    Stroke::new(1.0, theme::card_edge()),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }
                    });
            }
            _ => {
                self.active = None;
                // Small windows: less padding around the pages.
                let screen = ctx.content_rect().size();
                let side = if screen.x < 1000.0 { 24 } else { 40 };
                let top = if screen.y < 640.0 { 20 } else { 32 };
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::bg_deep())
                            .inner_margin(Margin {
                                left: side,
                                right: side,
                                top,
                                bottom: if screen.y < 640.0 { 12 } else { 24 },
                            }),
                    )
                    .show(ui, |ui| match self.view {
                        View::Hosts => crate::ui::hosts::show(self, ui, &mut actions),
                        View::Files => crate::ui::sftp::page(self, ui, &mut actions),
                        View::Keychain => crate::ui::keychain::show(self, ui, &mut actions),
                        View::Snippets => crate::ui::snippets::show(self, ui, &mut actions),
                        View::Settings => crate::ui::settings::show(self, ui, &mut actions),
                        View::Help => crate::ui::help::show(self, ui),
                        View::Proxmox => crate::ui::proxmox::show(self, ui, &mut actions),
                        View::Containers => {
                            crate::ui::containers_page::show(self, ui, &mut actions)
                        }
                        View::Inventory => crate::ui::inventory_page::show(self, ui, &mut actions),
                    });
            }
        }
        self.popout_windows(&ctx, &mut actions);

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
        crate::ui::proxmox::windows(self, &ctx);
        crate::ui::palette::show(self, &ctx, &mut actions);
        crate::ui::nav_rail::sheets(self, &ctx);
        // Deferred view-only changes (expansion, scope) are written even when the host list
        // is not the page on screen.
        crate::ui::nav_state::flush(self, &ctx);
        crate::ui::dialogs::show(self, &ctx);
        crate::ui::logvars::show(self, &ctx, &mut actions);
        crate::ui::pwmanager_ui::show(self, &ctx);
        crate::ui::history::frame(self, &ctx);
        crate::ui::sessionlog_ui::frame(self);
        crate::ui::containers::show(self, &ctx, &mut actions);
        crate::ui::portscan::show(self, &ctx, &mut actions);
        crate::ui::net::show(self, &ctx, &mut actions);
        crate::ui::ops::show(self, &ctx, &mut actions);
        crate::ui::import_ui::show(self, &ctx);
        crate::ui::containers_page::sheets(self, &ctx);
        crate::ui::xfer::windows(self, &ctx);
        crate::ui::edit::windows(self, &ctx);
        if let Some(id) = self.xfer.show_request.take() {
            actions.push(Action::XferShow(id));
        }
        self.toasts_ui(&ctx);

        for a in actions {
            self.apply(a, &ctx);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        crate::ui::web_embed::begin_frame(self.lib.settings.web_own_window, frame);
        self.ui_frame(ui);
        crate::ui::web_embed::end_frame(self, &ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.termx.flush(self.vault.as_ref(), false);
        if self.vault.is_some() {
            self.lib.settings.last_session = self.session_snapshot();
            self.save_library();
        }
        self.xfer.shutdown();
        for tab in &mut self.tabs {
            match &mut tab.kind {
                TabKind::Term(t) => {
                    let _ = t.tx.send(TermCmd::Close);
                }
                TabKind::Sftp(t) => {
                    for r in t.panes.iter().filter_map(|p| p.remote.as_ref()) {
                        let _ = r.tx.send(SftpCmd::Close);
                    }
                }
                TabKind::Vnc(t) => t.view.close(),
                TabKind::Web(t) => t.close(),
                TabKind::Log(_) | TabKind::Rdp(_) | TabKind::Edit(_) => {}
            }
        }
    }
}

/// Space around and between session panes.
const PANE_GAP: f32 = 10.0;
const PANE_RADIUS: f32 = 12.0;

/// Whether a session is connected right now.
fn tab_live(t: &Tab) -> bool {
    match &t.kind {
        TabKind::Term(t) => matches!(t.state, ConnState::Connected),
        TabKind::Sftp(t) => matches!(t.state(), ConnState::Connected),
        TabKind::Rdp(t) => t.is_live(),
        TabKind::Vnc(t) => matches!(t.view.state, ConnState::Connected),
        TabKind::Log(t) => matches!(t.state, ConnState::Connected),
        TabKind::Edit(t) => matches!(t.state(), ConnState::Connected),
        TabKind::Web(t) => t.is_live(),
    }
}

/// Small pill at the bottom of a terminal while dropped files upload.
fn upload_badge(ui: &egui::Ui, name: &str, done: u64, total: u64) {
    let r = ui.max_rect();
    let text = if name.is_empty() {
        t!("Upload startet…").to_string()
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
    painter.rect_filled(
        pill,
        CornerRadius::same(12),
        theme::lerp(theme::surface(), theme::accent(), 0.16),
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
fn closed_badge(ui: &egui::Ui, replay: bool) {
    let r = ui.max_rect();
    let painter = ui.painter();
    let text = if replay {
        t!("Aufnahme zu Ende · Enter spielt erneut ab")
    } else {
        t!("Getrennt · Enter verbindet neu")
    };
    let g = painter.layout_no_wrap(text.into(), egui::FontId::proportional(12.5), theme::text());
    let size = g.size() + Vec2::new(32.0, 16.0);
    let pill = egui::Rect::from_min_size(
        egui::pos2(r.center().x - size.x / 2.0, r.bottom() - size.y - 16.0),
        size,
    );
    painter.rect_filled(
        pill,
        CornerRadius::same(12),
        theme::lerp(theme::surface(), theme::amber(), 0.18),
    );
    painter.galley(pill.min + Vec2::new(16.0, 8.0), g, theme::text());
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

/// "docker", "podman" or the pod path, shown under a container session's title.
fn container_kind(h: &Host) -> String {
    match h.protocol {
        Protocol::Kubernetes => h.subtitle(),
        _ if h.container.podman => "podman".into(),
        _ => "docker".into(),
    }
}

/// Taskbar / window icon that matches the theme: the dark logo for dark themes, the light one
/// for light themes (changed only when the theme flips).
fn sync_window_icon(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static SHOWN: AtomicU8 = AtomicU8::new(0);
    let dark = theme::palette().dark;
    let want = if dark { 1 } else { 2 };
    if SHOWN.swap(want, Ordering::Relaxed) == want {
        return;
    }
    let bytes: &[u8] = if dark {
        include_bytes!("../assets/icon.png")
    } else {
        include_bytes!("../assets/icon-light.png")
    };
    if let Ok(icon) = eframe::icon_data::from_png_bytes(bytes) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(std::sync::Arc::new(icon))));
    }
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
