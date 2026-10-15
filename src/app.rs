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
    pub upload: bool,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct LocalPane {
    pub path: PathBuf,
    pub edit: String,
    pub entries: Vec<Entry>,
    pub selected: Option<String>,
    pub error: Option<String>,
}

pub enum FsDialog {
    Mkdir {
        remote: bool,
        name: String,
    },
    Rename {
        remote: bool,
        from: String,
        to: String,
    },
    Delete {
        remote: bool,
        name: String,
        is_dir: bool,
    },
}

pub struct SftpTab {
    pub params: SshParams,
    pub tx: UnboundedSender<SftpCmd>,
    pub rx: UnboundedReceiver<SftpEvent>,
    pub state: ConnState,
    pub remote_path: String,
    pub remote_edit: String,
    pub remote_entries: Vec<Entry>,
    pub remote_selected: Option<String>,
    pub local: LocalPane,
    pub transfers: Vec<Transfer>,
    pub dialog: Option<FsDialog>,
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
    pub color: Color32,
    pub kind: TabKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Terminal,
    Sftp,
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
    pub next_id: u64,

    pub search: String,
    pub quick_connect: String,
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
}

/// UI intents collected while drawing and applied afterwards.
#[derive(Clone, Debug)]
pub enum Action {
    Connect(Uuid),
    OpenSftp(Uuid),
    QuickConnect(String),
    NewHost(Option<Uuid>),
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
    /// Ask before running the inner (destructive) action.
    Confirm(String, Box<Action>),
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let data_dir = crate::store::data_dir();
        let store = Store::new(data_dir.clone());
        let lib = match store.load_library() {
            Ok(Some(lib)) => lib,
            Ok(None) => {
                let mut lib = Library::default();
                lib.seed_examples();
                lib
            }
            Err(e) => {
                log::error!("library konnte nicht geladen werden: {e:#}");
                Library::default()
            }
        };
        theme::apply(&cc.egui_ctx, &lib.settings.theme);
        let (prompts_sink, prompts_rx) = Sink::new(cc.egui_ctx.clone());
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
            next_id: 1,
            search: String::new(),
            quick_connect: String::new(),
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
        }
    }

    pub fn accent(&self) -> Color32 {
        theme::teal()
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
        let ssh_params = || SshParams {
            host: host.hostname.trim().to_string(),
            port: host.port,
            username: user.clone(),
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
        };
        let kind = match (host.protocol, purpose) {
            (Protocol::Ssh, Purpose::Sftp) => {
                let params = ssh_params();
                let (sink, rx) = Sink::new(ctx.clone());
                let tx =
                    backend::sftp::spawn(&self.rt, params.clone(), sink, self.prompts_sink.clone());
                let home = directories::UserDirs::new()
                    .map(|d| d.home_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                let mut local = LocalPane {
                    path: home.clone(),
                    ..Default::default()
                };
                crate::ui::sftp::refresh_local(&mut local);
                TabKind::Sftp(Box::new(SftpTab {
                    params,
                    tx,
                    rx,
                    state: ConnState::Connecting("Verbinde…".into()),
                    remote_path: String::new(),
                    remote_edit: String::new(),
                    remote_entries: Vec::new(),
                    remote_selected: None,
                    local,
                    transfers: Vec::new(),
                    dialog: None,
                }))
            }
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
        let prefix = match &kind {
            TabKind::Sftp(_) => "🗁 ",
            _ => "",
        };
        self.tabs.push(Tab {
            id,
            title: format!("{prefix}{}", host.display_name()),
            color,
            kind,
        });
        self.active = Some(id);
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

    fn reconnect(&mut self, tab_id: u64, ctx: &egui::Context) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return;
        };
        if let TabKind::Term(t) = &self.tabs[idx].kind {
            let mut fresh = self.spawn_term(t.spec.clone(), t.startup.clone(), ctx);
            fresh.show_snippets = t.show_snippets;
            fresh.term.feed(b"\r\n\x1b[2m--- reconnect ---\x1b[0m\r\n");
            self.tabs[idx].kind = TabKind::Term(Box::new(fresh));
        } else if let TabKind::Sftp(t) = &self.tabs[idx].kind {
            let (sink, rx) = Sink::new(ctx.clone());
            let tx =
                backend::sftp::spawn(&self.rt, t.params.clone(), sink, self.prompts_sink.clone());
            if let TabKind::Sftp(t) = &mut self.tabs[idx].kind {
                t.tx = tx;
                t.rx = rx;
                t.state = ConnState::Connecting("Verbinde…".into());
            }
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        if let Some(idx) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(idx);
            match tab.kind {
                TabKind::Term(t) => {
                    let _ = t.tx.send(TermCmd::Close);
                }
                TabKind::Sftp(t) => {
                    let _ = t.tx.send(SftpCmd::Close);
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
                theme::teal(),
            );
        }
    }

    // ------------------------------------------------------------------ events

    fn pump_events(&mut self, ctx: &egui::Context) {
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
                                t.term.feed(
                                    format!("\x1b[38;2;45;226;195m●\x1b[0m {s}\r\n").as_bytes(),
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
                    while let Ok(ev) = t.rx.try_recv() {
                        crate::ui::sftp::handle_event(t, ev, &mut toasts);
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
            Action::OpenSftp(id) => {
                if let Some(h) = self.lib.host(id).cloned() {
                    self.connect_host(h, Purpose::Sftp, ctx);
                }
            }
            Action::QuickConnect(s) => match parse_quick(&s) {
                Some(host) => self.connect_host(host, Purpose::Terminal, ctx),
                None => self.toast("Format: user@host[:port]", theme::amber()),
            },
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

    fn top_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("top")
            .exact_size(46.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_deep())
                    .inner_margin(Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // Brand
                    theme::logo(ui, 24.0);
                    ui.label(
                        RichText::new("aurora")
                            .size(17.0)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.add_space(14.0);

                    // Home tab
                    let home_sel = self.active.is_none();
                    if tab_button(ui, "🏠 Vault", home_sel, theme::teal(), None)
                        .0
                        .clicked()
                    {
                        self.active = None;
                    }

                    let mut close = None;
                    let mut activate = None;
                    egui::ScrollArea::horizontal()
                        .max_width(ui.available_width() - 190.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for tab in &self.tabs {
                                    let status = match &tab.kind {
                                        TabKind::Term(t) => Some(state_color(&t.state)),
                                        TabKind::Sftp(t) => Some(state_color(&t.state)),
                                        TabKind::Rdp(t) => Some(t.status_color()),
                                    };
                                    let (resp, closed) = tab_button(
                                        ui,
                                        &tab.title,
                                        self.active == Some(tab.id),
                                        tab.color,
                                        status,
                                    );
                                    if resp.clicked() {
                                        activate = Some(tab.id);
                                    }
                                    if resp.middle_clicked() || closed {
                                        close = Some(tab.id);
                                    }
                                }
                            });
                        });
                    if let Some(id) = activate {
                        self.active = Some(id);
                    }
                    if let Some(id) = close {
                        self.close_tab(id);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(
                                RichText::new("⌘K  Suche").color(theme::text_dim()),
                            ))
                            .on_hover_text("Befehlspalette (Ctrl+K)")
                            .clicked()
                        {
                            actions.push(Action::OpenPalette);
                        }
                        let bc = if self.broadcast {
                            theme::pink()
                        } else {
                            theme::text_dim()
                        };
                        let btn = egui::Button::new(RichText::new("📡 Broadcast").color(bc)).fill(
                            if self.broadcast {
                                theme::pink().linear_multiply(0.15)
                            } else {
                                theme::bg_card()
                            },
                        );
                        if ui
                            .add(btn)
                            .on_hover_text("Eingaben an alle verbundenen Terminals senden")
                            .clicked()
                        {
                            self.broadcast = !self.broadcast;
                        }
                    });
                });
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("nav")
            .exact_size(214.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_panel())
                    .inner_margin(Margin::symmetric(12, 16)),
            )
            .show(ui, |ui| {
                let items = [
                    (View::Hosts, "🖧", "Hosts", self.lib.hosts.len()),
                    (
                        View::Keychain,
                        "🔑",
                        "Keychain",
                        self.keychain.identities.len(),
                    ),
                    (View::Snippets, "⚡", "Snippets", self.lib.snippets.len()),
                    (View::Settings, "⚙", "Einstellungen", 0),
                ];
                for (view, icon, label, count) in items {
                    let selected = self.active.is_none() && self.view == view;
                    if nav_item(ui, icon, label, count, selected, self.accent()).clicked() {
                        actions.push(Action::Navigate(view));
                    }
                }

                ui.add_space(18.0);
                theme::section_title(ui, "Gruppen");
                ui.add_space(4.0);
                let groups: Vec<(Uuid, String, [u8; 3], String, usize)> = self
                    .lib
                    .groups
                    .iter()
                    .filter(|g| g.parent_id.is_none())
                    .map(|g| {
                        let n = self
                            .lib
                            .hosts
                            .iter()
                            .filter(|h| h.group_id == Some(g.id))
                            .count();
                        (g.id, g.name.clone(), g.color, g.icon.clone(), n)
                    })
                    .collect();
                for (_id, name, color, icon, n) in groups {
                    let r = ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(dot.center(), 4.0, theme::rgb(color));
                        ui.add(
                            egui::Label::new(format!("{icon} {name}"))
                                .sense(egui::Sense::click())
                                .truncate(),
                        )
                    });
                    if r.inner.on_hover_text(format!("{n} Hosts")).clicked() {
                        self.search = format!("group:{name}");
                        actions.push(Action::Navigate(View::Hosts));
                    }
                }
                if ui.small_button("➕ Neue Gruppe").clicked() {
                    actions.push(Action::NewGroup(None));
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if ui
                        .add(
                            egui::Button::new("🔒 Sperren")
                                .min_size(Vec2::new(ui.available_width(), 30.0)),
                        )
                        .clicked()
                    {
                        actions.push(Action::Lock);
                    }
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_deep())
                    .inner_margin(Margin::symmetric(14, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let connected = self
                        .tabs
                        .iter()
                        .filter(|t| match &t.kind {
                            TabKind::Term(t) => matches!(t.state, ConnState::Connected),
                            TabKind::Sftp(t) => matches!(t.state, ConnState::Connected),
                            TabKind::Rdp(t) => t.is_live(),
                        })
                        .count();
                    let small = |t: String| RichText::new(t).size(11.5).color(theme::text_dim());
                    ui.label(RichText::new("⏺").size(10.0).color(if connected > 0 {
                        theme::green()
                    } else {
                        theme::text_dim()
                    }));
                    ui.label(small(format!("{connected} aktive Sessions")));
                    ui.label(small("·".into()));
                    ui.label(small(format!(
                        "{} Hosts · {} Snippets",
                        self.lib.hosts.len(),
                        self.lib.snippets.len()
                    )));
                    if self.broadcast {
                        ui.label(
                            RichText::new("· BROADCAST AKTIV")
                                .size(11.5)
                                .color(theme::pink())
                                .strong(),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(small("🔐 Vault entsperrt".into()));
                    });
                });
            });
    }

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
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let TabKind::Term(t) = &mut tab.kind else {
            return;
        };
        let mut reconnect = false;
        let mut input = None;

        // Toolbar
        egui::Panel::top(egui::Id::new(("term-bar", tab_id)))
            .exact_size(38.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_panel())
                    .inner_margin(Margin::symmetric(12, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let (label, color) = match &t.state {
                        ConnState::Connecting(s) => (s.clone(), theme::amber()),
                        ConnState::Connected => ("Verbunden".into(), theme::green()),
                        ConnState::Closed(Some(e)) => (format!("Getrennt: {e}"), theme::red()),
                        ConnState::Closed(None) => ("Getrennt".into(), theme::text_dim()),
                    };
                    ui.label(RichText::new("⏺").color(color));
                    ui.add(
                        egui::Label::new(RichText::new(label).color(theme::text_dim()).size(12.5))
                            .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .selectable_label(t.show_snippets, "⚡ Snippets")
                            .on_hover_text("Snippet-Leiste ein/aus")
                            .clicked()
                        {
                            t.show_snippets = !t.show_snippets;
                        }
                        if matches!(t.state, ConnState::Closed(_))
                            && ui.button("⟳ Neu verbinden").clicked()
                        {
                            reconnect = true;
                        }
                    });
                });
            });

        if t.show_snippets {
            egui::Panel::right(egui::Id::new(("term-snips", tab_id)))
                .default_size(260.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::bg_panel())
                        .inner_margin(Margin::same(10)),
                )
                .show(ui, |ui| {
                    theme::section_title(ui, "Snippets");
                    ui.add(
                        egui::TextEdit::singleline(&mut t.snippet_filter).hint_text("🔍 Filtern…"),
                    );
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let f = t.snippet_filter.to_lowercase();
                        for (id, name, script) in snippets.iter().filter(|(_, n, s)| {
                            f.is_empty()
                                || n.to_lowercase().contains(&f)
                                || s.to_lowercase().contains(&f)
                        }) {
                            theme::card_frame()
                                .inner_margin(Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(name).strong());
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button("▶")
                                                    .on_hover_text("Ausführen")
                                                    .clicked()
                                                {
                                                    actions.push(Action::RunSnippet(*id, false));
                                                }
                                            },
                                        );
                                    });
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(script)
                                                .monospace()
                                                .size(11.5)
                                                .color(theme::text_dim()),
                                        )
                                        .truncate(),
                                    );
                                });
                        }
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_deep())
                    .inner_margin(Margin::same(8)),
            )
            .show(ui, |ui| {
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
                        glow: broadcast.then_some(theme::pink().linear_multiply(0.7)),
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
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let color = tab.color;
        let title = tab.title.clone();
        let TabKind::Rdp(t) = &mut tab.kind else {
            return;
        };
        if let Some(view) = t.embedded.as_mut() {
            let mut toasts = Vec::new();
            let action = view.show(
                ui,
                egui::Id::new(("rdp", tab_id)),
                &title,
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
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_deep())
                    .inner_margin(Margin::same(40)),
            )
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    theme::avatar(ui, 84.0, color, "🖥");
                    ui.add_space(12.0);
                    ui.label(RichText::new(&title).size(24.0).strong());
                    let mut target = format!("{}:{}", t.launch.host, t.launch.port);
                    if !t.launch.username.is_empty() {
                        target = format!("{}@{target}", t.launch.username);
                    }
                    ui.label(RichText::new(target).color(theme::text_dim()));
                    ui.add_space(16.0);
                    let c = if t.running {
                        theme::green()
                    } else {
                        theme::amber()
                    };
                    theme::chip(ui, &t.status, c);
                    if t.running {
                        ui.label(
                            RichText::new(format!("seit {}s", t.started.elapsed().as_secs()))
                                .color(theme::text_dim())
                                .size(12.0),
                        );
                        ui.ctx().request_repaint_after(Duration::from_secs(1));
                    }
                    ui.add_space(20.0);
                    if t.running {
                        if ui.button("⏹ Beenden").clicked()
                            && let Some(child) = t.process.as_mut().and_then(|p| p.child.as_mut())
                        {
                            let _ = child.kill();
                        }
                    } else if theme::primary_button(ui, "⟳ Erneut starten").clicked() {
                        relaunch = true;
                    }
                });
            });
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
                        .fill(theme::bg_card())
                        .stroke(Stroke::new(1.0, t.color.linear_multiply(0.6)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("⏺").color(t.color));
                                ui.label(&t.text);
                            });
                        });
                }
            });
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
        self.top_bar(ui, &mut actions);
        self.status_bar(ui);

        match self.active {
            Some(id) if self.tabs.iter().any(|t| t.id == id) => {
                let kind = self.tabs.iter().find(|t| t.id == id).map(|t| match t.kind {
                    TabKind::Term(_) => 0,
                    TabKind::Sftp(_) => 1,
                    TabKind::Rdp(_) => 2,
                });
                match kind {
                    Some(0) => self.terminal_tab(ui, id, &mut actions),
                    Some(1) => crate::ui::sftp::show(self, ui, id),
                    _ => self.rdp_tab(ui, id),
                }
            }
            _ => {
                self.active = None;
                self.sidebar(ui, &mut actions);
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::bg_deep())
                            .inner_margin(Margin::symmetric(28, 22)),
                    )
                    .show(ui, |ui| match self.view {
                        View::Hosts => crate::ui::hosts::show(self, ui, &mut actions),
                        View::Keychain => crate::ui::keychain::show(self, ui, &mut actions),
                        View::Snippets => crate::ui::snippets::show(self, ui, &mut actions),
                        View::Settings => crate::ui::settings::show(self, ui, &mut actions),
                    });
            }
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
                    let _ = t.tx.send(SftpCmd::Close);
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

fn nav_item(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    count: usize,
    selected: bool,
    accent: Color32,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(9), accent.linear_multiply(0.14));
        let bar =
            egui::Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 8.0), Vec2::new(3.0, 20.0));
        p.rect_filled(bar, CornerRadius::same(2), accent);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(9), theme::bg_hover());
    }
    let fg = if selected {
        theme::text()
    } else {
        theme::text_dim()
    };
    p.text(
        rect.left_center() + Vec2::new(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(15.0),
        if selected { accent } else { fg },
    );
    p.text(
        rect.left_center() + Vec2::new(40.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        fg,
    );
    if count > 0 {
        p.text(
            rect.right_center() - Vec2::new(12.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(11.5),
            theme::text_dim(),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pill-shaped tab; returns (response, close_clicked).
fn tab_button(
    ui: &mut egui::Ui,
    title: &str,
    selected: bool,
    color: Color32,
    status: Option<Color32>,
) -> (egui::Response, bool) {
    let font = egui::FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(title.to_string(), font, theme::text());
    let closable = status.is_some();
    let w = galley.size().x + 30.0 + if closable { 22.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w.min(240.0), 30.0), egui::Sense::click());
    let p = ui.painter_at(rect);
    let bg = if selected {
        theme::bg_card()
    } else if resp.hovered() {
        theme::bg_panel()
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, CornerRadius::same(9), bg);
    if selected {
        let line = egui::Rect::from_min_size(
            rect.left_bottom() - Vec2::new(-10.0, 2.0),
            Vec2::new(rect.width() - 20.0, 2.0),
        );
        theme::gradient_rect(&p, line, color, theme::violet(), 0.0);
    }
    p.circle_filled(
        rect.left_center() + Vec2::new(12.0, 0.0),
        3.5,
        status.unwrap_or(color),
    );
    p.galley(
        rect.left_center() + Vec2::new(22.0, -galley.size().y / 2.0),
        galley,
        if selected {
            theme::text()
        } else {
            theme::text_dim()
        },
    );
    let mut closed = false;
    if closable {
        let x_rect = egui::Rect::from_center_size(
            rect.right_center() - Vec2::new(14.0, 0.0),
            Vec2::splat(18.0),
        );
        let x_resp = ui.interact(x_rect, resp.id.with("x"), egui::Sense::click());
        if x_resp.hovered() {
            ui.painter()
                .rect_filled(x_rect, CornerRadius::same(5), theme::bg_hover());
        }
        ui.painter().text(
            x_rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(15.0),
            theme::text_dim(),
        );
        closed = x_resp.clicked();
    }
    (resp, closed)
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
