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
use crate::model::{Group, Host, Identity, Keychain, Library, Protocol, SerialSettings, Snippet};
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    Terminal,
    /// Connect the given pane of a file tab.
    Pane(u64, usize),
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

pub struct HostEditor {
    pub host: Host,
    pub is_new: bool,
    pub tags_text: String,
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
    pub pane_queue: VecDeque<(u64, usize, Host)>,
    pub next_id: u64,

    pub search: String,
    /// Hosts view filter: 0 all, 1 SSH, 2 RDP, 3 Serial, 4 favorites.
    pub host_filter: usize,
    pub snippet_search: String,
    pub keychain_search: String,
    pub broadcast: bool,
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
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &lib.settings.theme);
        let (prompts_sink, prompts_rx) = Sink::new(cc.egui_ctx.clone());
        let (update_sink, update_rx) = Sink::new(cc.egui_ctx.clone());
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
            pane_queue: VecDeque::new(),
            next_id: 1,
            search: String::new(),
            host_filter: 0,
            snippet_search: String::new(),
            keychain_search: String::new(),
            broadcast: false,
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
                if let Purpose::Pane(tab, idx) = purpose {
                    self.pane_queue.push_back((tab, idx, host));
                }
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
                TabKind::Term(Box::new(self.spawn_term(spec, startup, ctx)))
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
        });
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
        }
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
            kind: TabKind::Sftp(Box::new(SftpTab {
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
        }
        if self.split.len() < 2 {
            self.split.clear();
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        self.split.retain(|s| *s != id);
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
            }
        }
    }

    /// Bytes typed into a terminal; mirrored to all terminals in broadcast mode.
    fn send_input(&mut self, from: u64, bytes: Vec<u8>) {
        for tab in &mut self.tabs {
            if let TabKind::Term(t) = &mut tab.kind {
                let target =
                    tab.id == from || (self.broadcast && matches!(t.state, ConnState::Connected));
                if target {
                    let _ = t.tx.send(TermCmd::Input(bytes.clone()));
                }
            }
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
        for tab in &mut self.tabs {
            if let TabKind::Term(t) = &mut tab.kind
                && matches!(t.state, ConnState::Connected)
                && (everywhere || Some(tab.id) == active)
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
        for tab in &mut self.tabs {
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
                            TermEvent::Connected => {
                                t.state = ConnState::Connected;
                                let (cols, rows) = t.term.size();
                                let _ = t.tx.send(TermCmd::Resize { cols, rows });
                                if let Some(s) = &t.startup {
                                    let mut s = s.replace('\n', "\r");
                                    s.push('\r');
                                    let _ = t.tx.send(TermCmd::Input(s.into_bytes()));
                                }
                            }
                            TermEvent::Output(b) => t.term.feed(&b),
                            TermEvent::Closed(err) => {
                                let msg = match &err {
                                    Some(e) => format!("\r\n\x1b[31m✖ {e}\x1b[0m\r\n"),
                                    None => "\r\n\x1b[2mVerbindung beendet.\x1b[0m\r\n".into(),
                                };
                                t.term.feed(msg.as_bytes());
                                if let Some(e) = &err {
                                    toasts.push((format!("{}: {e}", tab.title), theme::red()));
                                }
                                t.state = ConnState::Closed(err);
                            }
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
        let broadcast = self.broadcast;
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
        let index = self.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let (title, subtitle) = (tab.title.clone(), tab.subtitle.clone());
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
                if closed && theme::icon_button(ui, "⟳", "Neu verbinden (Enter)").clicked() {
                    reconnect = true;
                }
            },
        );
        if toggle_snippets {
            t.show_snippets = !t.show_snippets;
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
                if broadcast {
                    crate::ui::chrome::band(
                        ui.painter(),
                        ui.max_rect().expand2(Vec2::new(10.0, 6.0)),
                        theme::pink(),
                    );
                }
                let overlay = match &t.state {
                    ConnState::Connecting(_) if t.term.contents().trim().is_empty() => {
                        Some("Verbinde…")
                    }
                    _ => None,
                };
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
                if let Some((cols, rows)) = out.resize {
                    let _ = t.tx.send(TermCmd::Resize { cols, rows });
                }
                if !out.input.is_empty() {
                    if matches!(t.state, ConnState::Closed(_)) {
                        // Enter on a dead session reconnects, like Termius.
                        if out.input.contains(&b'\r') {
                            reconnect = true;
                        }
                    } else {
                        input = Some(out.input);
                    }
                }
            });

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
            self.toasts_ui(&ctx);
            return;
        }

        let mut actions = Vec::new();
        self.shortcuts(&ctx, &mut actions);
        crate::ui::chrome::rail(self, ui, &mut actions);

        ctx.data_mut(|d| {
            d.insert_temp(
                crate::ui::chrome::split_info_id(),
                crate::ui::chrome::SplitInfo {
                    split: self.split.clone(),
                    tabs: self.tabs.iter().map(|t| (t.id, t.title.clone())).collect(),
                },
            )
        });
        let split_view =
            self.split.len() >= 2 && self.active.is_some_and(|a| self.split.contains(&a));
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

        if let Some(op) = ctx.data_mut(|d| {
            let id = crate::ui::chrome::split_op_id();
            let op = d.get_temp::<crate::ui::chrome::SplitOp>(id);
            d.remove::<crate::ui::chrome::SplitOp>(id);
            op
        }) {
            self.apply_split(op);
        }

        if self.password_prompt.is_none()
            && let Some((tab, idx, host)) = self.pane_queue.pop_front()
        {
            self.connect_host(host, Purpose::Pane(tab, idx), &ctx);
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

pub fn state_color(s: &ConnState) -> Color32 {
    match s {
        ConnState::Connecting(_) => theme::amber(),
        ConnState::Connected => theme::green(),
        ConnState::Closed(Some(_)) => theme::red(),
        ConnState::Closed(None) => theme::text_dim(),
    }
}

/// Parses `user@host:port` (user and port optional) into an ad-hoc SSH host.
pub fn parse_quick(s: &str) -> Option<Host> {
    let s = s.trim().trim_start_matches("ssh ").trim();
    if s.is_empty() || s.contains(' ') {
        return None;
    }
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
        port,
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
        assert!(parse_quick("u@h:notaport").is_none());
    }
}
