//! Built-in text editor tab for files on a server (over SFTP) or on this PC: line numbers,
//! syntax highlighting, search/replace, safe saving with conflict detection.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, CornerRadius, Margin, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::app::{App, ConnState, RemoteConn, Tab, TabKind};
use crate::backend::sftp::{SftpCmd, SftpEvent};
use crate::backend::sftp_edit::{self as core, EditCmd, EditEvent, Eol, Refuse, Version};
use crate::backend::ssh::SshParams;
use crate::model::Env;
use crate::theme;
use crate::ui::edit_syntax::{self as syntax, Lang, Tok};
use crate::ui::edit_text as text;

// ---------------------------------------------------------------------------- state

pub enum Origin {
    Local(PathBuf),
    Remote {
        host: Option<Uuid>,
        label: String,
        params: SshParams,
        /// Kept alive for the tab; replaced when the connection dropped.
        conn: Box<RemoteConn>,
    },
}

/// A file to open in a new editor tab.
#[allow(clippy::large_enum_variant)]
pub enum NewEdit {
    Local(PathBuf),
    Remote {
        host: Option<Uuid>,
        label: String,
        params: SshParams,
        path: String,
        color: Color32,
        env: Env,
    },
}

pub enum Phase {
    Connecting,
    Loading,
    Ready,
    /// The file could not be read; `missing` = does not exist (can be created).
    Failed {
        error: String,
        missing: bool,
    },
    /// Not editable as text; `bytes` allow a read-only look.
    Refused {
        reason: String,
        bytes: Vec<u8>,
    },
}

enum Pending {
    Load,
    Save {
        data: Vec<u8>,
        force: bool,
        /// Fingerprint of the saved text, to compare after the answer.
        fp: u64,
    },
}

pub enum Dialog {
    Conflict {
        remote: Option<String>,
        summary: String,
        deleted: bool,
        current: Option<Version>,
    },
    SaveError {
        error: String,
        denied: bool,
    },
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub replace: bool,
    pub query: String,
    pub with: String,
    pub case: bool,
    pub matches: Vec<std::ops::Range<usize>>,
    pub cur: Option<usize>,
    /// (text hash, query, case) the matches were computed for.
    key: (u64, String, bool),
    pub focus: bool,
}

struct Cache {
    key: Option<(u64, u32, u32, u32, Lang, u32)>,
    galley: Option<Arc<egui::Galley>>,
}

pub struct EditTab {
    pub origin: Origin,
    /// Path as shown (and used for SFTP).
    pub path: String,
    pub name: String,
    pub phase: Phase,
    pub text: String,
    pub eol: Eol,
    pub mixed: bool,
    pub final_newline: bool,
    pub bom: bool,
    pub indent: text::Indent,
    pub version: Option<Version>,
    /// Fingerprint of the saved state (text and layout options).
    clean: u64,
    pub dirty: bool,
    pub lang: Lang,
    pub wrap: bool,
    /// Font size change in points (Ctrl+wheel).
    pub zoom: f32,
    pub find: Find,
    pub goto: Option<String>,
    pub dialog: Option<Dialog>,
    pub read_only: bool,
    pending: Option<Pending>,
    sent: bool,
    pub saving: bool,
    /// A save was started by "Speichern und schließen".
    pub close_after_save: bool,
    pub force_close: bool,
    cache: Cache,
    pub cursor: (usize, usize),
    scroll_to: Option<usize>,
    set_cursor: Option<(usize, usize)>,
    focus_text: bool,
    status: Option<(String, Color32, Instant)>,
    reconnects: u8,
    seq: u64,
    /// Asked to open the file in an external program.
    pub want_external: bool,
    /// The read-only preview shows only the start of the file.
    preview_cut: bool,
}

impl EditTab {
    fn new(origin: Origin, path: String) -> Self {
        let name = path
            .replace('\\', "/")
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .to_string();
        let lang = syntax::detect(&path, "");
        Self {
            origin,
            path,
            name,
            phase: Phase::Connecting,
            text: String::new(),
            eol: Eol::Lf,
            mixed: false,
            final_newline: true,
            bom: false,
            indent: text::Indent::Spaces(4),
            version: None,
            clean: 0,
            dirty: false,
            lang,
            wrap: false,
            zoom: 0.0,
            find: Find::default(),
            goto: None,
            dialog: None,
            read_only: false,
            pending: None,
            sent: false,
            saving: false,
            close_after_save: false,
            force_close: false,
            cache: Cache {
                key: None,
                galley: None,
            },
            cursor: (1, 1),
            scroll_to: None,
            set_cursor: None,
            focus_text: true,
            status: None,
            reconnects: 0,
            seq: 0,
            want_external: false,
            preview_cut: false,
        }
    }

    fn fingerprint(&self) -> u64 {
        text::fast_hash(&self.text)
            ^ ((self.eol as u64) << 1)
            ^ ((self.final_newline as u64) << 2)
            ^ ((self.bom as u64) << 3)
    }

    fn say(&mut self, msg: impl Into<String>, color: Color32) {
        self.status = Some((msg.into(), color, Instant::now()));
    }

    pub fn conn(&self) -> Option<&RemoteConn> {
        match &self.origin {
            Origin::Remote { conn, .. } => Some(conn),
            Origin::Local(_) => None,
        }
    }

    fn is_remote(&self) -> bool {
        matches!(self.origin, Origin::Remote { .. })
    }

    pub fn state(&self) -> &ConnState {
        static OK: ConnState = ConnState::Connected;
        match &self.origin {
            Origin::Local(_) => &OK,
            Origin::Remote { conn, .. } => &conn.state,
        }
    }

    /// The server file as the editor shows it after a successful load.
    fn apply_doc(&mut self, doc: core::Doc) {
        self.indent = text::detect_indent(&doc.text, self.lang, &self.name);
        self.text = doc.text;
        self.eol = doc.eol;
        self.mixed = doc.mixed;
        self.final_newline = doc.final_newline;
        self.bom = doc.bom;
        self.clean = self.fingerprint();
        self.dirty = false;
        self.read_only = false;
        self.phase = Phase::Ready;
        self.focus_text = true;
        self.cursor = (1, 1);
        self.set_cursor = Some((0, 0));
        if let Some(l) = self.text.lines().next() {
            self.lang = syntax::detect(&self.path, l);
        }
    }

    fn got_bytes(&mut self, bytes: Vec<u8>, version: Version, truncated: bool) {
        self.version = Some(version);
        if truncated {
            self.phase = Phase::Refused {
                reason: Refuse::TooLarge(version.size as usize).message(),
                bytes,
            };
            return;
        }
        match core::decode(&bytes) {
            Ok(doc) => self.apply_doc(doc),
            Err(r) => {
                self.phase = Phase::Refused {
                    reason: r.message(),
                    bytes,
                }
            }
        }
    }

    /// The text to write.
    fn encoded(&self) -> Vec<u8> {
        core::encode(&self.text, self.eol, self.final_newline, self.bom)
    }

    /// Starts a save (the answer arrives through `pump`).
    pub fn save(&mut self, force: bool) {
        if !matches!(self.phase, Phase::Ready) || self.read_only || self.saving {
            return;
        }
        self.saving = true;
        self.dialog = None;
        let data = self.encoded();
        let fp = self.fingerprint();
        match &self.origin {
            Origin::Local(p) => {
                let r = core::local_save(p, &data, self.version, force);
                self.saving = false;
                match r {
                    Ok(core::SaveOutcome::Saved(v)) => self.saved(v, fp),
                    Ok(core::SaveOutcome::Conflict(cur, content)) => {
                        self.conflict(cur, content);
                    }
                    Err(e) => self.save_failed(
                        e.to_string(),
                        e.kind() == std::io::ErrorKind::PermissionDenied,
                    ),
                }
            }
            Origin::Remote { .. } => {
                self.pending = Some(Pending::Save { data, force, fp });
                self.sent = false;
            }
        }
    }

    fn saved(&mut self, v: Version, fp: u64) {
        self.version = Some(v);
        self.clean = fp;
        self.saving = false;
        self.pending = None;
        self.dirty = self.fingerprint() != self.clean;
        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        self.say(t!("Gespeichert {}", now), theme::green());
    }

    fn conflict(&mut self, current: Option<Version>, remote: Option<Vec<u8>>) {
        self.saving = false;
        self.pending = None;
        let remote_text = remote
            .as_deref()
            .and_then(|b| core::decode(b).ok())
            .map(|d| d.text);
        let summary = match (&remote_text, current) {
            (Some(r), _) => core::diff_summary(&self.text, r).text(),
            (None, Some(_)) => t!("Die Server-Version lässt sich nicht als Text lesen.").into(),
            (None, None) => t!("Die Datei wurde auf dem Server gelöscht.").into(),
        };
        self.dialog = Some(Dialog::Conflict {
            remote: remote_text,
            summary,
            deleted: current.is_none(),
            current,
        });
    }

    fn save_failed(&mut self, error: String, denied: bool) {
        self.saving = false;
        self.pending = None;
        self.close_after_save = false;
        self.dialog = Some(Dialog::SaveError { error, denied });
    }

    /// Loads the server's version instead of the editor's (after a conflict).
    fn take_remote(&mut self, remote: String, current: Option<Version>) {
        self.version = current;
        let eol = self.eol;
        self.text = remote;
        self.eol = eol;
        self.clean = self.fingerprint();
        self.dirty = false;
        self.dialog = None;
        self.say(t!("Server-Version geladen"), theme::accent());
    }

    pub fn reload(&mut self) {
        self.dialog = None;
        self.phase = Phase::Loading;
        self.begin_load();
    }

    fn begin_load(&mut self) {
        match &self.origin {
            Origin::Local(p) => match core::local_load(p) {
                Ok((bytes, v)) => {
                    let truncated = bytes.len() > core::MAX_BYTES;
                    self.got_bytes(bytes, v, truncated);
                }
                Err(e) => {
                    self.phase = Phase::Failed {
                        missing: e.kind() == std::io::ErrorKind::NotFound,
                        error: e.to_string(),
                    }
                }
            },
            Origin::Remote { .. } => {
                self.phase = Phase::Loading;
                self.pending = Some(Pending::Load);
                self.sent = false;
            }
        }
    }

    /// Creates an empty file for a path that does not exist yet.
    fn create_new(&mut self) {
        self.version = None;
        self.text.clear();
        self.eol = Eol::Lf;
        self.final_newline = true;
        self.bom = false;
        self.clean = self.fingerprint();
        self.dirty = false;
        self.phase = Phase::Ready;
        self.focus_text = true;
        // A new file counts as modified so that closing asks.
        self.clean ^= 1;
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------- global state

#[derive(Default)]
pub struct EditGlobal {
    /// Tab whose closing waits for the "unsaved changes" answer.
    pub close_confirm: Option<u64>,
    pub quit_confirm: bool,
    pub quit_ok: bool,
    pub open_prompt: Option<OpenPrompt>,
}

/// "Datei öffnen…" for a terminal session.
pub struct OpenPrompt {
    pub host: Option<Uuid>,
    pub label: String,
    pub params: SshParams,
    pub cwd: Option<String>,
    pub color: Color32,
    pub env: Env,
    pub input: String,
    pub error: Option<String>,
    pub focus: bool,
}

pub fn dirty_tabs(app: &App) -> Vec<u64> {
    app.tabs
        .iter()
        .filter(|t| matches!(&t.kind, TabKind::Edit(e) if e.dirty))
        .map(|t| t.id)
        .collect()
}

// ---------------------------------------------------------------------------- opening

/// Opens `file` in an editor tab (or focuses the tab that already has it open).
pub fn open(app: &mut App, file: NewEdit, ctx: &egui::Context) {
    let (path, host, color, env, subtitle) = match &file {
        NewEdit::Local(p) => (
            p.display().to_string(),
            None,
            theme::accent(),
            Env::None,
            t!("Dieser PC").to_string(),
        ),
        NewEdit::Remote {
            host,
            path,
            color,
            env,
            label,
            ..
        } => (path.clone(), *host, *color, *env, label.clone()),
    };
    // Already open?
    let same = |t: &Tab| match (&t.kind, &file) {
        (TabKind::Edit(e), NewEdit::Local(p)) => {
            matches!(&e.origin, Origin::Local(q) if q == p)
        }
        (TabKind::Edit(e), NewEdit::Remote { host, path, .. }) => {
            matches!(&e.origin, Origin::Remote { host: h, .. } if h == host) && &e.path == path
        }
        _ => false,
    };
    if let Some(t) = app.tabs.iter().find(|t| same(t)) {
        app.active = Some(t.id);
        app.toast(t!("Datei ist schon im Editor offen"), theme::accent());
        return;
    }
    app.next_id += 1;
    let id = app.next_id;
    let origin = match file {
        NewEdit::Local(p) => Origin::Local(p),
        NewEdit::Remote {
            host,
            label,
            params,
            ..
        } => {
            let conn = crate::ui::sftp::spawn_conn(
                &app.rt,
                params.clone(),
                label.clone(),
                host,
                ctx,
                app.prompts_sink.clone(),
            );
            Origin::Remote {
                host,
                label,
                params,
                conn: Box::new(conn),
            }
        }
    };
    let mut e = EditTab::new(origin, path);
    if matches!(e.origin, Origin::Local(_)) {
        e.phase = Phase::Loading;
        e.begin_load();
    }
    let title = e.name.clone();
    app.tabs.push(Tab {
        id,
        title,
        subtitle,
        color,
        attention: false,
        env,
        notes: String::new(),
        notes_open: false,
        unsaved: None,
        host_id: host,
        kind: TabKind::Edit(Box::new(e)),
    });
    app.active = Some(id);
}

// ---------------------------------------------------------------------------- pump

pub struct PumpOut {
    pub toasts: Vec<(String, Color32)>,
    pub close: bool,
}

/// Handles connection events and sends queued commands. Called every frame.
pub fn pump(
    tab: &mut Tab,
    rt: &tokio::runtime::Runtime,
    prompts: &crate::backend::Sink<crate::backend::HostKeyPrompt>,
    ctx: &egui::Context,
) -> PumpOut {
    let mut out = PumpOut {
        toasts: Vec::new(),
        close: false,
    };
    let TabKind::Edit(e) = &mut tab.kind else {
        return out;
    };
    // Title with the unsaved marker.
    e.dirty = matches!(e.phase, Phase::Ready) && !e.read_only && e.fingerprint() != e.clean;
    let want = if e.dirty {
        format!("● {}", e.name)
    } else {
        e.name.clone()
    };
    if tab.title != want {
        tab.title = want;
    }
    let Origin::Remote {
        params,
        conn,
        label,
        host,
    } = &mut e.origin
    else {
        return out;
    };

    // Events of the connection.
    let mut events = Vec::new();
    while let Ok(ev) = conn.rx.try_recv() {
        events.push(ev);
    }
    let mut connected_now = false;
    let mut results: Vec<EditEvent> = Vec::new();
    for ev in events {
        match ev {
            SftpEvent::Status(s) => conn.state = ConnState::Connecting(s),
            SftpEvent::Connected { session, .. } => {
                conn.state = ConnState::Connected;
                conn.session = Some(session);
                connected_now = true;
            }
            SftpEvent::Closed(err) => {
                conn.state = ConnState::Closed(err);
                conn.session = None;
                e.sent = false;
            }
            SftpEvent::Edit(r) => results.push(r),
            _ => {}
        }
    }
    let _ = connected_now;

    // Reconnect when work is waiting but the connection is gone.
    let waiting = e.pending.is_some();
    if matches!(conn.state, ConnState::Closed(_)) && waiting && e.reconnects < 3 {
        e.reconnects += 1;
        **conn = crate::ui::sftp::spawn_conn(
            rt,
            params.clone(),
            label.clone(),
            *host,
            ctx,
            prompts.clone(),
        );
        e.sent = false;
        e.status = Some((
            t!("Verbindung verloren – verbinde neu…").to_string(),
            theme::amber(),
            Instant::now(),
        ));
    } else if matches!(conn.state, ConnState::Closed(_)) && waiting && e.reconnects >= 3 {
        let msg = match &conn.state {
            ConnState::Closed(Some(m)) => m.clone(),
            _ => t!("Verbindung getrennt").to_string(),
        };
        e.reconnects = 0;
        e.pending = None;
        e.saving = false;
        e.close_after_save = false;
        if matches!(e.phase, Phase::Connecting | Phase::Loading) {
            e.phase = Phase::Failed {
                error: msg,
                missing: false,
            };
        } else {
            e.dialog = Some(Dialog::SaveError {
                error: msg,
                denied: false,
            });
        }
    }

    // Send what is waiting.
    if matches!(conn.state, ConnState::Connected) && !e.sent {
        match &e.pending {
            Some(Pending::Load) => {
                e.seq += 1;
                let _ = conn.tx.send(SftpCmd::Edit(EditCmd::Load {
                    id: e.seq,
                    path: e.path.clone(),
                }));
                e.sent = true;
            }
            Some(Pending::Save { data, force, .. }) => {
                e.seq += 1;
                let _ = conn.tx.send(SftpCmd::Edit(EditCmd::Save {
                    id: e.seq,
                    path: e.path.clone(),
                    data: data.clone(),
                    expect: e.version,
                    force: *force,
                }));
                e.sent = true;
            }
            None => {}
        }
        if matches!(e.phase, Phase::Connecting) && e.pending.is_none() {
            e.phase = Phase::Loading;
            e.pending = Some(Pending::Load);
            e.sent = false;
        }
    }
    if matches!(conn.state, ConnState::Connected) {
        // Connected before anything was requested: start loading.
        if matches!(e.phase, Phase::Connecting) {
            e.phase = Phase::Loading;
            e.pending = Some(Pending::Load);
            e.sent = false;
            ctx.request_repaint();
        }
        e.reconnects = 0;
    }

    for r in results {
        match r {
            EditEvent::Loaded {
                bytes,
                version,
                truncated,
                ..
            } => {
                e.pending = None;
                e.got_bytes(bytes, version, truncated);
            }
            EditEvent::LoadFailed { error, .. } => {
                e.pending = None;
                let missing = {
                    let l = error.to_lowercase();
                    l.contains("no such file") || l.contains("not found")
                };
                e.phase = Phase::Failed { error, missing };
            }
            EditEvent::Saved { version, .. } => {
                let fp = match &e.pending {
                    Some(Pending::Save { fp, .. }) => *fp,
                    _ => e.fingerprint(),
                };
                e.saved(version, fp);
                if e.close_after_save {
                    out.close = true;
                }
            }
            EditEvent::Conflict {
                current, remote, ..
            } => {
                e.close_after_save = false;
                e.conflict(current, remote);
            }
            EditEvent::SaveFailed { error, denied, .. } => {
                e.save_failed(error, denied);
            }
        }
    }
    if e.pending.is_some() || e.saving {
        ctx.request_repaint_after(Duration::from_millis(300));
    }
    out
}

// ---------------------------------------------------------------------------- view

enum Act {
    Save,
    Reload,
    External,
    CloseTab,
}

pub fn show(app: &mut App, ui: &mut egui::Ui, tab_id: u64) {
    let index = app.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
    let font_size = app.lib.settings.font_size;
    let font_name = app.lib.settings.term_font.clone();
    let family = theme::term_font(ui.ctx(), &font_name);
    let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab_id) else {
        return;
    };
    let title = tab.title.clone();
    let subtitle = tab.subtitle.clone();
    let TabKind::Edit(e) = &mut tab.kind else {
        return;
    };
    let mut act: Option<Act> = None;
    let state = crate::ui::chrome::state_pair(e.state());
    let path_line = format!("{}  ·  {}", subtitle, e.path);
    let ready = matches!(e.phase, Phase::Ready);
    let can_save = ready && !e.read_only && !e.saving;
    let dirty = e.dirty;
    let remote = e.is_remote();
    let _ = remote;
    crate::ui::chrome::session_header(
        ui,
        egui::Id::new(("edit-bar", tab_id)),
        index,
        &title,
        &path_line,
        state,
        |ui| {
            if theme::icon_btn(
                ui,
                crate::ui::icons::Icon::External,
                t!("Mit externem Programm öffnen"),
            )
            .clicked()
            {
                act = Some(Act::External);
            }
            if (ready || matches!(e.phase, Phase::Failed { .. } | Phase::Refused { .. }))
                && theme::icon_btn(ui, crate::ui::icons::Icon::Refresh, t!("Neu laden")).clicked()
            {
                act = Some(Act::Reload);
            }
            if ready {
                if theme::icon_toggle(
                    ui,
                    crate::ui::icons::Icon::Wrap,
                    t!("Zeilenumbruch"),
                    e.wrap,
                )
                .clicked()
                {
                    e.wrap = !e.wrap;
                }
                if theme::icon_btn(ui, crate::ui::icons::Icon::Search, t!("Suchen (Strg+F)"))
                    .clicked()
                {
                    e.find.open = true;
                    e.find.focus = true;
                }
            }
            ui.add_enabled_ui(can_save, |ui| {
                let kind = if dirty {
                    theme::Btn::SmallPrimary
                } else {
                    theme::Btn::Small
                };
                if theme::btn(ui, kind, None, t!("Speichern")).clicked() {
                    act = Some(Act::Save);
                }
            });
        },
    );

    let bg = theme::palette().term_bg;
    match &e.phase {
        Phase::Connecting | Phase::Loading => {
            let msg = match (&e.phase, e.state()) {
                (_, ConnState::Connecting(m)) => m.clone(),
                (Phase::Connecting, _) => t!("Verbinde…").to_string(),
                _ => t!("Lade Datei…").to_string(),
            };
            centered(ui, bg, |ui| {
                ui.label(RichText::new(msg).size(14.0).color(theme::text_dim()));
            });
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }
        Phase::Failed { error, missing } => {
            let (error, missing) = (error.clone(), *missing);
            let mut create = false;
            let mut retry = false;
            centered(ui, bg, |ui| {
                ui.label(
                    RichText::new(if missing {
                        t!("Die Datei gibt es noch nicht.")
                    } else {
                        t!("Die Datei konnte nicht geöffnet werden.")
                    })
                    .font(theme::semibold(16.0)),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(RichText::new(&error).color(theme::red()).size(13.0)).wrap(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if missing
                        && theme::btn(ui, theme::Btn::Primary, None, t!("Neue Datei anlegen"))
                            .clicked()
                    {
                        create = true;
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Erneut versuchen")).clicked() {
                        retry = true;
                    }
                });
            });
            if create {
                e.create_new();
            }
            if retry {
                act = Some(Act::Reload);
            }
        }
        Phase::Refused { .. } => {
            refused_view(ui, e, &family, font_size, bg, &mut act);
        }
        Phase::Ready => {
            editor_view(ui, e, tab_id, &family, font_size, bg, &mut act);
        }
    }

    if e.read_only && matches!(e.phase, Phase::Ready) {
        // handled in editor_view
    }
    dialogs(ui.ctx(), e);
    let (do_save, do_reload, do_external, close) = (
        matches!(act, Some(Act::Save)),
        matches!(act, Some(Act::Reload)),
        matches!(act, Some(Act::External)),
        matches!(act, Some(Act::CloseTab)),
    );
    if do_save {
        e.save(false);
    }
    if do_reload {
        if e.dirty {
            // Reloading throws away edits: the conflict dialog's "server version" does that
            // deliberately; here we keep it simple and just reload.
        }
        e.reload();
    }
    if do_external {
        e.want_external = true;
    }
    let _ = bg;
    let want_external = std::mem::take(&mut e.want_external);
    let ext = if want_external {
        Some((
            e.path.clone(),
            e.conn()
                .map(|c| (c.params.clone(), c.label.clone(), c.host_id)),
            match &e.origin {
                Origin::Local(p) => Some(p.clone()),
                _ => None,
            },
        ))
    } else {
        None
    };
    if close {
        app.close_tab(tab_id);
    }
    if let Some((path, remote, local)) = ext {
        let ctx = ui.ctx().clone();
        match (remote, local) {
            (Some((params, label, host)), _) => {
                crate::ui::sftp::external_edit_remote(app, params, label, host, path, &ctx)
            }
            (None, Some(p)) => {
                let editor = app.lib.settings.editor.clone();
                if let Err(err) = crate::ui::sftp::open_in_editor(&editor, &p) {
                    app.toast(format!("Editor: {err}"), theme::red());
                }
            }
            _ => {}
        }
    }
}

fn centered(ui: &mut egui::Ui, bg: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(bg).inner_margin(Margin::same(24)))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.3).max(10.0));
                ui.set_max_width(520.0);
                add(ui);
            });
        });
}

// ----- refused (binary / not UTF-8 / too large)

fn refused_view(
    ui: &mut egui::Ui,
    e: &mut EditTab,
    family: &egui::FontFamily,
    font_size: f32,
    bg: Color32,
    act: &mut Option<Act>,
) {
    let Phase::Refused { reason, .. } = &e.phase else {
        return;
    };
    let reason = reason.clone();
    let mut view = false;
    egui::Panel::top(egui::Id::new(("edit-refused", e.seq, e.name.clone())))
        .frame(
            egui::Frame::NONE
                .fill(theme::soft(theme::amber()))
                .inner_margin(Margin::symmetric(16, 10)),
        )
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(&reason).size(13.0).color(theme::text())).wrap());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::btn(
                    ui,
                    theme::Btn::Small,
                    None,
                    t!("Mit externem Programm öffnen"),
                )
                .clicked()
                {
                    *act = Some(Act::External);
                }
                if !e.read_only
                    && theme::btn(ui, theme::Btn::Small, None, t!("Schreibgeschützt ansehen"))
                        .clicked()
                {
                    view = true;
                }
                if theme::btn(ui, theme::Btn::Small, None, t!("Schließen")).clicked() {
                    *act = Some(Act::CloseTab);
                }
            });
        });
    if view {
        // Built once: binary garbage is cut short, it is only there for a quick look.
        let Phase::Refused { bytes, .. } = &e.phase else {
            return;
        };
        let binary = bytes.contains(&0) || std::str::from_utf8(bytes).is_err();
        let limit = if binary { 64 * 1024 } else { 512 * 1024 };
        let cut = &bytes[..bytes.len().min(limit)];
        let s: String = String::from_utf8_lossy(cut)
            .chars()
            .map(|c| {
                if c == '\0' || (c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
                    '·'
                } else {
                    c
                }
            })
            .collect::<String>()
            .replace('\r', "");
        e.preview_cut = bytes.len() > limit;
        e.text = s;
        e.wrap = true;
        e.read_only = true;
        ui.ctx().request_repaint();
    }
    if e.read_only {
        let note = e.preview_cut;
        let lossy = std::mem::take(&mut e.text);
        let mut s: &str = &lossy;
        let font = egui::FontId::new(font_size + e.zoom, family.clone());
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg))
            .show(ui, |ui| {
                if note {
                    ui.label(
                        RichText::new(t!("Nur der Anfang der Datei wird angezeigt."))
                            .size(12.0)
                            .color(theme::text_dim()),
                    );
                }
                egui::ScrollArea::both()
                    .id_salt("edit-readonly")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut s)
                                .font(font)
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(10, 6)),
                        );
                    });
            });
        e.text = lossy;
    } else {
        centered(ui, bg, |_| {});
    }
}

// ----- the editor itself

struct ViewOut {
    cursor_char: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn editor_view(
    ui: &mut egui::Ui,
    e: &mut EditTab,
    tab_id: u64,
    family: &egui::FontFamily,
    font_size: f32,
    bg: Color32,
    act: &mut Option<Act>,
) {
    let ctx = ui.ctx().clone();
    let base = egui::Id::new(("edit", tab_id));
    let text_id = base.with("text");
    let find_id = base.with("find");
    let repl_id = base.with("repl");
    let goto_id = base.with("goto");
    let font = egui::FontId::new((font_size + e.zoom).clamp(7.0, 40.0), family.clone());
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));

    // ---- shortcuts (only when this editor has the keyboard or the pointer)
    let focused = ctx.memory(|m| {
        [text_id, find_id, repl_id, goto_id]
            .iter()
            .any(|i| m.has_focus(*i))
    });
    let hovered = ui.rect_contains_pointer(ui.max_rect());
    let mine = focused || (hovered && ctx.memory(|m| m.focused().is_none()));
    if mine {
        let (save, find, replace, goto, next, prev, esc_zoom) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::H),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::G),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F3),
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::F3),
                0,
            )
        });
        let _ = esc_zoom;
        if save {
            *act = Some(Act::Save);
        }
        if find || replace {
            e.find.open = true;
            e.find.replace = replace || e.find.replace;
            e.find.focus = true;
            // Selected text becomes the query.
            if let Some(sel) = selected_text(&ctx, text_id, &e.text)
                && !sel.contains('\n')
                && !sel.is_empty()
            {
                e.find.query = sel;
            }
        }
        if goto {
            e.goto = Some(String::new());
        }
        if next {
            find_step(e, true);
        }
        if prev {
            find_step(e, false);
        }
        // Ctrl + wheel zooms the font.
        let zoom = ctx.input(|i| i.zoom_delta());
        if hovered && (zoom - 1.0).abs() > 0.001 {
            let step = if zoom > 1.0 { 1.0 } else { -1.0 };
            e.zoom = (e.zoom + step).clamp(-6.0, 24.0);
        }
    }

    // ---- find / replace / goto bars
    find_bars(ui, e, find_id, repl_id, goto_id, text_id);

    // ---- status bar
    status_bar(ui, e);

    // ---- text area
    let lines = text::line_count(&e.text);
    let digits = lines.to_string().len().max(3);
    let gutter_w = digits as f32 * char_w + 22.0;
    let avail = ui.available_rect_before_wrap();
    let gutter = egui::Rect::from_min_size(avail.min, Vec2::new(gutter_w, avail.height()));
    let body =
        egui::Rect::from_min_max(egui::pos2(avail.left() + gutter_w, avail.top()), avail.max);
    ui.painter().rect_filled(avail, CornerRadius::ZERO, bg);
    ui.painter().rect_filled(
        gutter,
        CornerRadius::ZERO,
        theme::lerp(bg, theme::text(), 0.04),
    );

    // Keys that change the text must be handled before the TextEdit sees them.
    let text_focused = ctx.memory(|m| m.has_focus(text_id));
    if text_focused
        && e.find.open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        e.find.open = false;
        e.find.cur = None;
    }
    if text_focused && !e.read_only {
        key_edits(&ctx, e, text_id);
    }
    if let Some((a, b)) = e.set_cursor.take() {
        set_selection(&ctx, text_id, a, b);
    }
    if e.focus_text {
        ctx.memory_mut(|m| m.request_focus(text_id));
        e.focus_text = false;
    }

    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    body_ui.set_clip_rect(body.intersect(ui.clip_rect()));
    let wrap = e.wrap;
    let lang = e.lang;
    let read_only = e.read_only;
    let mut hits: Vec<(usize, usize)> = Vec::new();
    let find_active = e.find.open && !e.find.query.is_empty();
    if find_active {
        recompute_matches(e);
        hits = e.find.matches.iter().map(|m| (m.start, m.end)).collect();
    }
    let cur_hit = e.find.cur;
    let scroll_to = e.scroll_to.take();
    let (plain, term_bg) = (theme::palette().term_fg, bg);
    let _ = term_bg;
    let mut view = ViewOut { cursor_char: None };
    let mut galley_info: Option<(Arc<egui::Galley>, egui::Pos2)> = None;
    let EditTab {
        text: buf, cache, ..
    } = &mut *e;
    let area = egui::ScrollArea::both()
        .id_salt(base.with("scroll"))
        .auto_shrink([false, false])
        .show(&mut body_ui, |ui| {
            let highlight_idx = ui.painter().add(egui::Shape::Noop);
            let match_idx = ui.painter().add(egui::Shape::Noop);
            let min_h = ui.available_height();
            let mut layouter = |ui: &egui::Ui, t: &dyn egui::TextBuffer, wrap_width: f32| {
                let s = t.as_str();
                let w = if wrap { wrap_width } else { f32::INFINITY };
                let key = (
                    text::fast_hash(s),
                    if w.is_finite() { w.to_bits() } else { 0 },
                    font.size.to_bits(),
                    plain
                        .to_array()
                        .iter()
                        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(*b as u32))
                        ^ theme::accent().r() as u32,
                    lang,
                    s.len() as u32,
                );
                if cache.key == Some(key)
                    && let Some(g) = &cache.galley
                {
                    return g.clone();
                }
                let job = build_job(s, lang, &font, w, plain);
                let g = ui.painter().layout_job(job);
                cache.key = Some(key);
                cache.galley = Some(g.clone());
                g
            };
            let output = if read_only {
                let mut s: &str = buf;
                egui::TextEdit::multiline(&mut s)
                    .id(text_id)
                    .font(font.clone())
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 6))
                    .min_size(Vec2::new(0.0, min_h))
                    .layouter(&mut layouter)
                    .show(ui)
            } else {
                egui::TextEdit::multiline(buf)
                    .id(text_id)
                    .font(font.clone())
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 6))
                    .min_size(Vec2::new(0.0, min_h))
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui)
            };
            let galley = output.galley.clone();
            let pos = output.galley_pos;
            let clip = ui.clip_rect();
            // Current line.
            if let Some(range) = output.cursor_range {
                let c = range.primary;
                view.cursor_char = Some(c.index.into());
                let lc = galley.layout_from_cursor(c);
                let (r0, r1) = logical_rows(&galley, lc.row);
                if range.is_empty() && output.response.has_focus() || range.is_empty() {
                    let top = pos.y + galley.rows[r0].pos.y;
                    let bottom = pos.y + galley.rows[r1].pos.y + galley.rows[r1].row.size.y;
                    let r = egui::Rect::from_min_max(
                        egui::pos2(clip.left(), top),
                        egui::pos2(clip.right().max(pos.x + galley.size().x), bottom),
                    );
                    ui.painter().set(
                        highlight_idx,
                        egui::Shape::rect_filled(
                            r,
                            CornerRadius::ZERO,
                            theme::lerp(bg, theme::text(), 0.06),
                        ),
                    );
                }
            }
            // Search hits in the visible part.
            if !hits.is_empty() {
                let mut shapes = Vec::new();
                let first_vis = galley
                    .rows
                    .iter()
                    .position(|r| pos.y + r.pos.y + r.row.size.y >= clip.top())
                    .unwrap_or(0);
                let last_vis = galley
                    .rows
                    .iter()
                    .rposition(|r| pos.y + r.pos.y <= clip.bottom())
                    .unwrap_or(0);
                let mut char_at_row = vec![0usize; galley.rows.len() + 1];
                let mut acc = 0usize;
                for (i, r) in galley.rows.iter().enumerate() {
                    char_at_row[i] = acc;
                    acc += r.row.glyphs.len() + usize::from(r.ends_with_newline);
                }
                char_at_row[galley.rows.len()] = acc;
                let (c0, c1) = (
                    char_at_row[first_vis],
                    char_at_row[(last_vis + 1).min(galley.rows.len())],
                );
                let src: &str = buf;
                let mut painted = 0;
                for (k, (bs, be)) in hits.iter().enumerate() {
                    let (cs, ce) = (text::byte_to_char(src, *bs), text::byte_to_char(src, *be));
                    if ce < c0 || cs > c1 {
                        continue;
                    }
                    let a = galley.pos_from_cursor(CCursor::new(cs));
                    let b = galley.pos_from_cursor(CCursor::new(ce));
                    let r = if (a.top() - b.top()).abs() < 1.0 {
                        egui::Rect::from_min_max(
                            egui::pos2(pos.x + a.left(), pos.y + a.top()),
                            egui::pos2(pos.x + b.left(), pos.y + a.bottom()),
                        )
                    } else {
                        egui::Rect::from_min_max(
                            egui::pos2(pos.x + a.left(), pos.y + a.top()),
                            egui::pos2(
                                pos.x + a.left() + char_w * (ce - cs) as f32,
                                pos.y + a.bottom(),
                            ),
                        )
                    };
                    let color = if Some(k) == cur_hit {
                        theme::amber().gamma_multiply(0.55)
                    } else {
                        theme::amber().gamma_multiply(0.22)
                    };
                    shapes.push(egui::Shape::rect_filled(r, CornerRadius::same(2), color));
                    painted += 1;
                    if painted > 600 {
                        break;
                    }
                }
                ui.painter().set(match_idx, egui::Shape::Vec(shapes));
            }
            // Scroll a jump target into view.
            if let Some(ci) = scroll_to {
                let r = galley.pos_from_cursor(CCursor::new(ci));
                let rect = r
                    .translate(pos.to_vec2())
                    .expand2(Vec2::new(60.0, row_h * 2.0));
                ui.scroll_to_rect(rect, Some(egui::Align::Center));
            }
            galley_info = Some((galley, pos));
            output.response
        });
    let _ = area.inner;

    // Line numbers next to the visible rows.
    if let Some((galley, pos)) = galley_info {
        let gp = ui
            .painter()
            .with_clip_rect(gutter.intersect(ui.clip_rect()));
        let cur_line = view
            .cursor_char
            .map(|c| galley.layout_from_cursor(CCursor::new(c)).row)
            .map(|r| logical_rows(&galley, r).0);
        let mut n = 1usize;
        let mut new_line = true;
        for (i, r) in galley.rows.iter().enumerate() {
            let y = pos.y + r.pos.y;
            if new_line {
                if y + r.row.size.y >= gutter.top() && y <= gutter.bottom() {
                    let is_cur = cur_line == Some(i);
                    gp.text(
                        egui::pos2(gutter.right() - 10.0, y + r.row.size.y / 2.0),
                        egui::Align2::RIGHT_CENTER,
                        n.to_string(),
                        egui::FontId::new(font.size, font.family.clone()),
                        if is_cur {
                            theme::text()
                        } else {
                            theme::text_dim().gamma_multiply(0.7)
                        },
                    );
                }
                n += 1;
            }
            new_line = r.ends_with_newline;
            if y > gutter.bottom() {
                break;
            }
        }
        // Cursor position for the status bar.
        if let Some(c) = view.cursor_char {
            let line_col = {
                let b = text::char_to_byte(&e.text, c);
                text::line_col(&e.text, b)
            };
            e.cursor = line_col;
        }
    }
    ui.allocate_rect(avail, Sense::hover());
}

/// Rows of the logical line (between line breaks) that contains `row`.
fn logical_rows(galley: &egui::Galley, row: usize) -> (usize, usize) {
    let rows = &galley.rows;
    let row = row.min(rows.len().saturating_sub(1));
    let mut start = row;
    while start > 0 && !rows[start - 1].ends_with_newline {
        start -= 1;
    }
    let mut end = row;
    while end + 1 < rows.len() && !rows[end].ends_with_newline {
        end += 1;
    }
    (start, end)
}

fn build_job(
    s: &str,
    lang: Lang,
    font: &egui::FontId,
    wrap_width: f32,
    plain: Color32,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutSection, TextFormat};
    let mut job = egui::text::LayoutJob {
        text: s.to_string(),
        ..Default::default()
    };
    job.wrap.max_width = wrap_width;
    let fmt = |c: Color32| TextFormat {
        font_id: font.clone(),
        color: c,
        ..Default::default()
    };
    let spans = syntax::tokenize(lang, s);
    let mut pos = 0;
    for sp in spans {
        if sp.start > pos {
            job.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: pos.into()..sp.start.into(),
                format: fmt(plain),
            });
        }
        let c = if sp.tok == Tok::Plain {
            plain
        } else {
            syntax::color(sp.tok)
        };
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: sp.start.into()..sp.end.into(),
            format: fmt(c),
        });
        pos = sp.end;
    }
    if pos < s.len() || job.sections.is_empty() {
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: pos.into()..s.len().into(),
            format: fmt(plain),
        });
    }
    job
}

fn selection_chars(ctx: &egui::Context, id: egui::Id) -> Option<(usize, usize)> {
    let st = egui::TextEdit::load_state(ctx, id)?;
    let r = st.cursor.char_range()?;
    Some((r.primary.index.into(), r.secondary.index.into()))
}

fn selected_text(ctx: &egui::Context, id: egui::Id, buf: &str) -> Option<String> {
    let (a, b) = selection_chars(ctx, id)?;
    if a == b {
        return None;
    }
    let (a, b) = (a.min(b), a.max(b));
    let (ba, bb) = (text::char_to_byte(buf, a), text::char_to_byte(buf, b));
    Some(buf[ba..bb].to_string())
}

fn set_selection(ctx: &egui::Context, id: egui::Id, a: usize, b: usize) {
    let mut st = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    st.cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
    egui::TextEdit::store_state(ctx, id, st);
}

/// Tab, Shift+Tab and Enter (auto indent).
fn key_edits(ctx: &egui::Context, e: &mut EditTab, id: egui::Id) {
    let (backtab, tab, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    if !(backtab || tab || enter) {
        return;
    }
    let Some((p, s)) = selection_chars(ctx, id) else {
        return;
    };
    let sel = (s.min(p), s.max(p));
    let edit = if backtab {
        text::backtab_key(&e.text, sel, e.indent)
    } else if tab {
        text::tab_key(&e.text, sel, e.indent)
    } else {
        // Enter replaces the selection by a line break with the current indentation.
        let (ba, bb) = (
            text::char_to_byte(&e.text, sel.0),
            text::char_to_byte(&e.text, sel.1),
        );
        let line_start = e.text[..ba].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let ins = text::newline_with_indent(&e.text[line_start..ba], e.lang, e.indent);
        let mut out = String::with_capacity(e.text.len() + ins.len());
        out.push_str(&e.text[..ba]);
        out.push_str(&ins);
        out.push_str(&e.text[bb..]);
        let c = sel.0 + ins.chars().count();
        text::Edit {
            text: out,
            sel: (c, c),
        }
    };
    e.text = edit.text;
    set_selection(ctx, id, edit.sel.0, edit.sel.1);
    ctx.request_repaint();
}

fn recompute_matches(e: &mut EditTab) {
    let key = (text::fast_hash(&e.text), e.find.query.clone(), e.find.case);
    if e.find.key == key {
        return;
    }
    e.find.matches = text::find_all(&e.text, &e.find.query, e.find.case, 20_000);
    if e.find.cur.is_some_and(|c| c >= e.find.matches.len()) {
        e.find.cur = None;
    }
    e.find.key = key;
}

/// Jumps to the next/previous match from the current cursor.
fn find_step(e: &mut EditTab, forward: bool) {
    if e.find.query.is_empty() {
        e.find.open = true;
        e.find.focus = true;
        return;
    }
    recompute_matches(e);
    let from = match e.find.cur {
        Some(c) if c < e.find.matches.len() => {
            let m = &e.find.matches[c];
            if forward { m.end } else { m.start }
        }
        _ => text::char_to_byte(&e.text, e.set_cursor.map(|c| c.1).unwrap_or(0)),
    };
    let from = if e.find.cur.is_none() {
        e.cursor_byte()
    } else {
        from
    };
    if let Some(i) = text::pick_match(&e.find.matches, from, forward) {
        select_match(e, i);
    }
}

impl EditTab {
    fn cursor_byte(&self) -> usize {
        text::goto_offset(&self.text, self.cursor.0, self.cursor.1)
    }
}

fn select_match(e: &mut EditTab, i: usize) {
    let m = e.find.matches[i].clone();
    let (a, b) = (
        text::byte_to_char(&e.text, m.start),
        text::byte_to_char(&e.text, m.end),
    );
    e.find.cur = Some(i);
    e.set_cursor = Some((a, b));
    e.scroll_to = Some(a);
}

fn find_bars(
    ui: &mut egui::Ui,
    e: &mut EditTab,
    find_id: egui::Id,
    repl_id: egui::Id,
    goto_id: egui::Id,
    text_id: egui::Id,
) {
    if !e.find.open && e.goto.is_none() {
        return;
    }
    let frame = egui::Frame::NONE
        .fill(theme::surface())
        .inner_margin(Margin::symmetric(12, 8));
    egui::Panel::top(find_id.with("panel"))
        .frame(frame)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            if e.find.open {
                let mut close = false;
                let mut step: Option<bool> = None;
                let mut replace_one = false;
                let mut replace_all = false;
                ui.horizontal_wrapped(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut e.find.query)
                            .id(find_id)
                            .hint_text(t!("Suchen"))
                            .desired_width(220.0)
                            .margin(Margin::symmetric(10, 6)),
                    );
                    if e.find.focus {
                        r.request_focus();
                        e.find.focus = false;
                    }
                    if r.changed() {
                        e.find.cur = None;
                        recompute_matches(e);
                        // First hit at or after the cursor.
                        let from = e.cursor_byte();
                        if let Some(i) = text::pick_match(&e.find.matches, from, true) {
                            select_match(e, i);
                        }
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        step = Some(!ui.input(|i| i.modifiers.shift));
                        r.request_focus();
                    }
                    if theme::btn(
                        ui,
                        if e.find.case {
                            theme::Btn::SmallPrimary
                        } else {
                            theme::Btn::Small
                        },
                        None,
                        "Aa",
                    )
                    .on_hover_text(t!("Groß-/Kleinschreibung beachten"))
                    .clicked()
                    {
                        e.find.case = !e.find.case;
                        e.find.cur = None;
                    }
                    if theme::icon_btn(
                        ui,
                        crate::ui::icons::Icon::Up,
                        t!("Vorheriger Treffer (Umschalt+F3)"),
                    )
                    .clicked()
                    {
                        step = Some(false);
                    }
                    if theme::icon_btn(
                        ui,
                        crate::ui::icons::Icon::Down,
                        t!("Nächster Treffer (F3)"),
                    )
                    .clicked()
                    {
                        step = Some(true);
                    }
                    recompute_matches(e);
                    let n = e.find.matches.len();
                    let label = if e.find.query.is_empty() {
                        String::new()
                    } else if n == 0 {
                        t!("Kein Treffer").to_string()
                    } else {
                        match e.find.cur {
                            Some(c) => {
                                format!("{} / {}{}", c + 1, n, if n >= 20_000 { "+" } else { "" })
                            }
                            None => format!("{n}"),
                        }
                    };
                    ui.label(RichText::new(label).size(12.5).color(
                        if n == 0 && !e.find.query.is_empty() {
                            theme::red()
                        } else {
                            theme::text_dim()
                        },
                    ));
                    if !e.find.replace
                        && theme::btn(ui, theme::Btn::Link, None, t!("Ersetzen…")).clicked()
                    {
                        e.find.replace = true;
                    }
                    if theme::icon_btn(ui, crate::ui::icons::Icon::Close, t!("Schließen (Esc)"))
                        .clicked()
                    {
                        close = true;
                    }
                });
                if e.find.replace && !e.read_only {
                    ui.horizontal_wrapped(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut e.find.with)
                                .id(repl_id)
                                .hint_text(t!("Ersetzen durch"))
                                .desired_width(220.0)
                                .margin(Margin::symmetric(10, 6)),
                        );
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            replace_one = true;
                            r.request_focus();
                        }
                        if theme::btn(ui, theme::Btn::Small, None, t!("Ersetzen")).clicked() {
                            replace_one = true;
                        }
                        if theme::btn(ui, theme::Btn::Small, None, t!("Alle ersetzen")).clicked() {
                            replace_all = true;
                        }
                    });
                }
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape))
                    && (ui.memory(|m| m.has_focus(find_id)) || ui.memory(|m| m.has_focus(repl_id)));
                if close || esc {
                    e.find.open = false;
                    e.find.cur = None;
                    e.focus_text = true;
                }
                if let Some(fwd) = step {
                    find_step(e, fwd);
                }
                if replace_one {
                    do_replace_one(e);
                }
                if replace_all {
                    let (t, n) =
                        text::replace_all(&e.text, &e.find.query, &e.find.with, e.find.case);
                    if n > 0 {
                        e.text = t;
                        e.find.cur = None;
                        let msg = t!("{} Ersetzungen", n);
                        e.say(msg, theme::accent());
                    }
                }
            }
            if let Some(input) = e.goto.as_mut() {
                let mut done = false;
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t!("Gehe zu Zeile"))
                            .size(13.0)
                            .color(theme::text_dim()),
                    );
                    let r = ui.add(
                        egui::TextEdit::singleline(input)
                            .id(goto_id)
                            .hint_text("42  /  42:7")
                            .desired_width(110.0)
                            .margin(Margin::symmetric(10, 6)),
                    );
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = true;
                    } else if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                    if theme::btn(ui, theme::Btn::Small, None, "OK").clicked() {
                        done = true;
                    }
                    if theme::icon_btn(ui, crate::ui::icons::Icon::Close, t!("Schließen (Esc)"))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
                if done {
                    let target = text::parse_goto(input);
                    e.goto = None;
                    e.focus_text = true;
                    if let Some((l, c)) = target {
                        let b = text::goto_offset(&e.text, l, c);
                        let ci = text::byte_to_char(&e.text, b);
                        e.set_cursor = Some((ci, ci));
                        e.scroll_to = Some(ci);
                    }
                } else if cancel {
                    e.goto = None;
                    e.focus_text = true;
                }
            }
        });
    let _ = text_id;
}

fn do_replace_one(e: &mut EditTab) {
    if e.find.query.is_empty() {
        return;
    }
    recompute_matches(e);
    // The current hit, or the first one after the cursor.
    let idx = match e.find.cur {
        Some(c) if c < e.find.matches.len() => Some(c),
        _ => text::pick_match(&e.find.matches, e.cursor_byte(), true),
    };
    let Some(i) = idx else { return };
    let m = e.find.matches[i].clone();
    e.text.replace_range(m.clone(), &e.find.with.clone());
    let after = m.start + e.find.with.len();
    e.find.key.0 = 0;
    recompute_matches(e);
    e.find.cur = None;
    if let Some(n) = text::pick_match(&e.find.matches, after, true) {
        select_match(e, n);
    } else {
        let ci = text::byte_to_char(&e.text, after);
        e.set_cursor = Some((ci, ci));
    }
}

fn status_bar(ui: &mut egui::Ui, e: &mut EditTab) {
    egui::Panel::bottom(egui::Id::new(("edit-status", e.name.clone(), e.seq)).with(e.path.as_str()))
        .exact_size(30.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::surface())
                .inner_margin(Margin::symmetric(12, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let dim = |s: String| RichText::new(s).size(12.0).color(theme::text_dim());
                if let Some((msg, color, at)) = &e.status
                    && at.elapsed() < Duration::from_secs(8)
                {
                    ui.label(RichText::new(msg).size(12.0).color(*color));
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                }
                if e.dirty {
                    ui.label(RichText::new(t!("● Ungespeichert")).size(12.0).color(theme::amber()));
                }
                if e.saving {
                    ui.label(dim(t!("Speichere…").to_string()));
                }
                if e.read_only {
                    ui.label(dim(t!("Schreibgeschützt").to_string()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    // Syntax picker.
                    egui::ComboBox::from_id_salt("edit-lang")
                        .selected_text(RichText::new(e.lang.label()).size(12.0))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for l in syntax::ALL {
                                if ui.selectable_label(e.lang == *l, l.label()).clicked() {
                                    e.lang = *l;
                                }
                            }
                        });
                    if ui
                        .add(
                            egui::Label::new(dim(format!(
                                "{}{}",
                                e.eol.label(),
                                if e.mixed { " *" } else { "" }
                            )))
                            .sense(Sense::click()),
                        )
                        .on_hover_text(if e.mixed {
                            t!("Gemischte Zeilenenden – werden beim Speichern vereinheitlicht. Klick: LF/CRLF umschalten")
                        } else {
                            t!("Zeilenende – Klick schaltet zwischen LF und CRLF um")
                        })
                        .clicked()
                    {
                        e.eol = if e.eol == Eol::Lf { Eol::Crlf } else { Eol::Lf };
                        e.mixed = false;
                    }
                    ui.label(dim(format!("UTF-8{}", if e.bom { " BOM" } else { "" })));
                    let nl = if e.final_newline {
                        t!("Endet mit Zeilenumbruch")
                    } else {
                        t!("Ohne Zeilenumbruch am Ende")
                    };
                    if ui
                        .add(egui::Label::new(dim(e.indent.label())).sense(Sense::click()))
                        .on_hover_text(t!("Einrückung – Klick wechselt zwischen Tabs und Leerzeichen"))
                        .clicked()
                    {
                        e.indent = match e.indent {
                            text::Indent::Tabs => text::Indent::Spaces(4),
                            text::Indent::Spaces(_) => text::Indent::Tabs,
                        };
                    }
                    if ui
                        .add(egui::Label::new(dim(nl.to_string())).sense(Sense::click()))
                        .on_hover_text(t!("Klick schaltet den Zeilenumbruch am Dateiende um"))
                        .clicked()
                    {
                        e.final_newline = !e.final_newline;
                    }
                    ui.label(dim(format!(
                        "{} · {}",
                        t!("{} Zeilen", text::line_count(&e.text)),
                        crate::ui::human_size(e.text.len() as u64)
                    )));
                    ui.label(dim(t!("Zeile {}, Spalte {}", e.cursor.0, e.cursor.1)));
                });
            });
        });
}

// ----- dialogs

fn dialogs(ctx: &egui::Context, e: &mut EditTab) {
    let Some(d) = &e.dialog else { return };
    enum Choice {
        Overwrite,
        TakeRemote,
        Cancel,
        RetrySave,
    }
    let mut choice: Option<Choice> = None;
    egui::Modal::new(egui::Id::new(("edit-dialog", e.path.clone())))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(440.0);
            match d {
                Dialog::Conflict {
                    summary,
                    remote,
                    deleted,
                    ..
                } => {
                    ui.label(theme::heading(
                        if *deleted {
                            t!("Datei wurde gelöscht")
                        } else {
                            t!("Datei hat sich geändert")
                        },
                        18.0,
                    ));
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(if *deleted {
                                t!("Die Datei existiert auf dem Server nicht mehr. Beim Überschreiben wird sie neu angelegt.").to_string()
                            } else {
                                t!("Seit dem Öffnen wurde „{}“ auf dem Server verändert.", e.name)
                            })
                            .size(13.5),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new(summary).size(12.5).color(theme::text_dim())).wrap());
                    ui.add_space(14.0);
                    ui.horizontal_wrapped(|ui| {
                        if theme::btn(ui, theme::Btn::Primary, None, t!("Überschreiben")).clicked() {
                            choice = Some(Choice::Overwrite);
                        }
                        if remote.is_some()
                            && theme::btn(ui, theme::Btn::Normal, None, t!("Server-Version laden")).clicked()
                        {
                            choice = Some(Choice::TakeRemote);
                        }
                        if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            choice = Some(Choice::Cancel);
                        }
                    });
                }
                Dialog::SaveError { error, denied } => {
                    ui.label(theme::heading(t!("Speichern fehlgeschlagen"), 18.0));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(RichText::new(error).color(theme::red()).size(13.0)).wrap());
                    if *denied {
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(t!("Keine Schreibrechte für diese Datei. Speichere sie unter anderem Namen in einem Ordner, in den du schreiben darfst, oder ändere die Rechte auf dem Server."))
                                    .size(12.5)
                                    .color(theme::text_dim()),
                            )
                            .wrap(),
                        );
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if theme::btn(ui, theme::Btn::Primary, None, t!("Erneut versuchen")).clicked() {
                            choice = Some(Choice::RetrySave);
                        }
                        if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            choice = Some(Choice::Cancel);
                        }
                    });
                }
            }
        });
    match choice {
        Some(Choice::Overwrite) => e.save(true),
        Some(Choice::TakeRemote) => {
            if let Some(Dialog::Conflict {
                remote: Some(r),
                current,
                ..
            }) = e.dialog.take()
            {
                e.take_remote(r, current);
            }
        }
        Some(Choice::RetrySave) => {
            e.dialog = None;
            e.save(false);
        }
        Some(Choice::Cancel) => e.dialog = None,
        None => {}
    }
}

// ---------------------------------------------------------------------------- app-level dialogs

/// Closing confirmation, quit confirmation and the "open file" prompt.
pub fn windows(app: &mut App, ctx: &egui::Context) {
    // Window close with unsaved editors.
    if ctx.input(|i| i.viewport().close_requested()) && !app.editx.quit_ok {
        let dirty = dirty_tabs(app);
        if !dirty.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            app.editx.quit_confirm = true;
        }
    }
    close_confirm(app, ctx);
    quit_confirm(app, ctx);
    open_prompt(app, ctx);
}

fn tab_name(app: &App, id: u64) -> String {
    app.tabs
        .iter()
        .find(|t| t.id == id)
        .and_then(|t| match &t.kind {
            TabKind::Edit(e) => Some(e.name.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn close_confirm(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.editx.close_confirm else {
        return;
    };
    if !matches!(
        app.tabs.iter().find(|t| t.id == id).map(|t| &t.kind),
        Some(TabKind::Edit(e)) if e.dirty
    ) {
        app.editx.close_confirm = None;
        return;
    }
    let name = tab_name(app, id);
    enum C {
        Save,
        Discard,
        Cancel,
    }
    let mut c = None;
    egui::Modal::new(egui::Id::new("edit-close-confirm"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(theme::heading(t!("Änderungen speichern?"), 18.0));
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t!("„{}“ hat ungespeicherte Änderungen.", name)).size(13.5),
                )
                .wrap(),
            );
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Speichern und schließen"))
                    .clicked()
                {
                    c = Some(C::Save);
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Verwerfen")).clicked() {
                    c = Some(C::Discard);
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    c = Some(C::Cancel);
                }
            });
        });
    match c {
        Some(C::Save) => {
            app.editx.close_confirm = None;
            if let Some(Tab {
                kind: TabKind::Edit(e),
                ..
            }) = app.tabs.iter_mut().find(|t| t.id == id)
            {
                e.close_after_save = true;
                e.save(false);
                // Local saves finish at once.
                if !e.saving && !e.dirty && e.dialog.is_none() {
                    e.force_close = true;
                    app.close_tab(id);
                }
            }
        }
        Some(C::Discard) => {
            app.editx.close_confirm = None;
            if let Some(Tab {
                kind: TabKind::Edit(e),
                ..
            }) = app.tabs.iter_mut().find(|t| t.id == id)
            {
                e.force_close = true;
            }
            app.close_tab(id);
        }
        Some(C::Cancel) => app.editx.close_confirm = None,
        None => {}
    }
}

fn quit_confirm(app: &mut App, ctx: &egui::Context) {
    if !app.editx.quit_confirm {
        return;
    }
    let names: Vec<String> = dirty_tabs(app)
        .into_iter()
        .map(|id| tab_name(app, id))
        .collect();
    if names.is_empty() {
        app.editx.quit_confirm = false;
        return;
    }
    let mut quit = false;
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("edit-quit-confirm"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(theme::heading(t!("Ungespeicherte Änderungen"), 18.0));
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "Diese Dateien haben ungespeicherte Änderungen: {}",
                        names.join(", ")
                    ))
                    .size(13.5),
                )
                .wrap(),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Zurück zum Editor")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    cancel = true;
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Trotzdem beenden")).clicked() {
                    quit = true;
                }
            });
        });
    if cancel {
        app.editx.quit_confirm = false;
        if let Some(id) = dirty_tabs(app).first() {
            app.active = Some(*id);
        }
    }
    if quit {
        app.editx.quit_confirm = false;
        app.editx.quit_ok = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn open_prompt(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.editx.open_prompt.as_mut() else {
        return;
    };
    let mut go = false;
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("edit-open-prompt"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(theme::heading(t!("Datei öffnen"), 18.0));
            ui.add_space(4.0);
            ui.label(
                RichText::new(t!(
                    "Auf {} – Pfad absolut, mit ~ oder relativ zum Ordner des Terminals.",
                    p.label
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            if let Some(c) = &p.cwd {
                ui.label(theme::mono(format!("cwd: {c}"), 11.5).color(theme::text_dim()));
            }
            ui.add_space(8.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut p.input)
                    .hint_text("/etc/nginx/nginx.conf  ·  ~/.bashrc  ·  app.log")
                    .font(egui::FontId::monospace(13.0))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(12, 8)),
            );
            if p.focus {
                r.request_focus();
                p.focus = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            if let Some(err) = &p.error {
                ui.add_space(4.0);
                ui.label(RichText::new(err).color(theme::red()).size(12.5));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Öffnen")).clicked() {
                    go = true;
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    cancel = true;
                }
            });
        });
    if cancel {
        app.editx.open_prompt = None;
        return;
    }
    if go {
        let Some(p) = app.editx.open_prompt.as_mut() else {
            return;
        };
        match text::resolve_remote_path(&p.input, p.cwd.as_deref()) {
            Some(path) => {
                let p = app.editx.open_prompt.take().unwrap();
                open(
                    app,
                    NewEdit::Remote {
                        host: p.host,
                        label: p.label,
                        params: p.params,
                        path,
                        color: p.color,
                        env: p.env,
                    },
                    ctx,
                );
            }
            None => p.error = Some(t!("Bitte einen Dateipfad eingeben").to_string()),
        }
    }
}
