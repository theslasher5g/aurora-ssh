//! Embedded RDP viewer tab: renders the helper's framebuffer and forwards input.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Event, Pos2, Rect, RichText, Sense, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::ConnState;
use crate::backend::rdp::RdpLaunch;
use crate::backend::rdp_embed::{
    self, FileEv, FrameBuffer, RdpCmd, RdpConnect, RdpEvent, RdpHandle, SC_ENTER, SC_LALT,
    SC_LCTRL, SC_LSHIFT, SC_LWIN,
};
use crate::backend::{HostKeyPrompt, Sink};
use crate::theme::{self, Btn};
use crate::ui::icons::Icon;

/// Scancode of V (for Ctrl+V) and R (for Win+R).
const SC_V: u16 = 0x2F;
const SC_R: u16 = 0x13;
/// The Windows Run dialog takes at most this many characters.
const RUN_DIALOG_MAX: usize = 259;
/// Longest command typed into the PowerShell window (fallback for long scripts).
const TYPED_MAX: usize = 9_000;

/// One queued input step, sent once its time has come (never blocks the UI thread).
enum Step {
    Cmds(Vec<RdpCmd>),
    Text(String),
}

/// Running or just finished clipboard file transfer, for the badge.
struct Transfer {
    up: bool,
    files: usize,
    done: u64,
    total: u64,
    finished: Option<Instant>,
    /// Last change, to hide an offer the host never reads.
    touched: Instant,
}

/// The badge goes away when the host has not asked for (or sent) data this long; the
/// helper gives up on a download after the same time.
const TRANSFER_IDLE: Duration = Duration::from_secs(15);
/// An offer the host never reads at all is hidden a bit later (pasting takes a moment).
const OFFER_IDLE: Duration = Duration::from_secs(20);

/// `powershell -NoExit -EncodedCommand …` for `script` (Base64 of UTF-16LE).
pub fn encoded_command(script: &str) -> String {
    use base64::Engine;
    let script = script.replace("\r\n", "\n");
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell -NoExit -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Line for an open PowerShell window that runs the Base64-encoded `script`.
fn typed_command(script: &str) -> String {
    let b64 = encoded_command(script)
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .to_string();
    format!("iex ([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{b64}')))")
}

/// `Downloads/Aurora` (home directory if there is no download folder).
pub fn download_target() -> std::path::PathBuf {
    directories::UserDirs::new()
        .map(|d| {
            d.download_dir()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| d.home_dir().to_path_buf())
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Aurora")
}

fn file_error_text(code: &str, detail: &str) -> String {
    match code {
        "unsupported" => t!("Der Host erlaubt keine Dateiübertragung über die Zwischenablage"),
        "empty" => t!("Keine übertragbaren Dateien"),
        "rejected" => t!("Der Host hat die Dateiliste abgelehnt"),
        "busy" => t!("Es läuft bereits eine Übertragung"),
        "no_files" => t!("Die Zwischenablage des Hosts enthält keine Dateien"),
        "list" => t!("Dateiliste des Hosts konnte nicht gelesen werden"),
        "read" => return t!("Datei konnte nicht gelesen werden: {}", detail),
        "write" => return t!("Datei konnte nicht gespeichert werden: {}", detail),
        "too_big" => t!("Datei zu groß für diesen Host (über 2 GB)"),
        "timeout" => t!("Der Host antwortet nicht – Übertragung abgebrochen"),
        _ => t!("Übertragung vom Host abgebrochen"),
    }
    .to_string()
}

pub struct RdpView {
    pub handle: Option<RdpHandle>,
    rx: Option<UnboundedReceiver<RdpEvent>>,
    fb: Arc<Mutex<FrameBuffer>>,
    texture: Option<egui::TextureHandle>,
    pub state: ConnState,
    /// Remote desktop size we asked for.
    requested: (u16, u16),
    pending_resize: Option<(Instant, (u16, u16))>,
    mods: [bool; 3],
    last_pos: Option<(u16, u16)>,
    clipboard: Option<arboard::Clipboard>,
    clip_local_seen: String,
    clip_remote: String,
    clip_poll: Instant,
    /// Show a toast for the next clipboard text that arrives from the host.
    announce_clip: bool,
    pending_paste: Option<Instant>,
    had_focus: bool,
    start_requested: bool,
    /// Pinned TLS key fingerprint for this server (`rdp://host:port` in known hosts).
    pub known_fingerprint: Option<String>,
    /// Sync the clipboard automatically (host setting); the buttons always work.
    /// Also the switch for file transfer through the clipboard.
    pub auto_clipboard: bool,
    /// Remote position to click before pasting dropped files.
    pending_drop: Option<Option<(u16, u16)>>,
    /// Files are on offer: "Dateien einfügen" pastes them again.
    files_offered: bool,
    /// The host's clipboard holds files.
    remote_files: bool,
    transfer: Option<Transfer>,
    steps: VecDeque<(Instant, Step)>,
}

/// What the tab wants the app to do.
pub enum RdpAction {
    None,
    OpenExternal,
    /// Run this PowerShell snippet on the host (after asking for its variables).
    RunSnippet(uuid::Uuid),
}

impl RdpView {
    pub fn new(known_fingerprint: Option<String>, auto_clipboard: bool) -> Self {
        Self {
            known_fingerprint,
            auto_clipboard,
            handle: None,
            rx: None,
            fb: Arc::new(Mutex::new(FrameBuffer::default())),
            texture: None,
            state: ConnState::Connecting(t!("Verbinde…").into()),
            requested: (0, 0),
            pending_resize: None,
            mods: [false; 3],
            last_pos: None,
            clipboard: None,
            clip_local_seen: String::new(),
            clip_remote: String::new(),
            clip_poll: Instant::now(),
            announce_clip: false,
            pending_paste: None,
            had_focus: false,
            start_requested: true,
            pending_drop: None,
            files_offered: false,
            remote_files: false,
            transfer: None,
            steps: VecDeque::new(),
        }
    }

    /// Queues `step` `delay` after the previously queued one.
    fn queue(&mut self, delay: Duration, step: Step) {
        let base = self
            .steps
            .back()
            .map(|(t, _)| *t)
            .unwrap_or_else(Instant::now)
            .max(Instant::now());
        self.steps.push_back((base + delay, step));
    }

    /// Sends the queued steps that are due; returns when the next one is.
    fn run_steps(&mut self) -> Option<Duration> {
        if !self.connected() {
            self.steps.clear();
            return None;
        }
        while let Some((due, _)) = self.steps.front() {
            if *due > Instant::now() {
                return Some(due.saturating_duration_since(Instant::now()));
            }
            let Some((_, step)) = self.steps.pop_front() else {
                break;
            };
            match step {
                Step::Cmds(cmds) => {
                    for c in cmds {
                        self.send(c);
                    }
                }
                Step::Text(t) => self.type_text(&t),
            }
        }
        None
    }

    fn ctrl_v() -> Vec<RdpCmd> {
        vec![
            RdpCmd::Key {
                code: SC_LCTRL,
                down: true,
            },
            RdpCmd::Key {
                code: SC_V,
                down: true,
            },
            RdpCmd::Key {
                code: SC_V,
                down: false,
            },
            RdpCmd::Key {
                code: SC_LCTRL,
                down: false,
            },
        ]
    }

    /// Pastes the offered files: click at the drop position (if known), then Ctrl+V.
    fn paste_files(&mut self, at: Option<(u16, u16)>) {
        // Release whatever the local keyboard holds, so Ctrl+V is clean.
        self.send(RdpCmd::ReleaseAll);
        self.mods = [false; 3];
        if let Some((x, y)) = at {
            self.queue(
                Duration::ZERO,
                Step::Cmds(vec![
                    RdpCmd::Move { x, y },
                    RdpCmd::Button { b: 0, down: true },
                    RdpCmd::Button { b: 0, down: false },
                ]),
            );
        }
        self.queue(Duration::from_millis(250), Step::Cmds(Self::ctrl_v()));
    }

    /// Runs `script` in PowerShell on the Windows host: Win+R, then the encoded command.
    /// Long scripts open PowerShell first and are typed into its window.
    pub fn run_powershell(&mut self, script: &str) -> Result<(), String> {
        if !self.connected() {
            return Err(t!("Nicht verbunden").into());
        }
        let direct = encoded_command(script);
        let typed = typed_command(script);
        if direct.chars().count() > RUN_DIALOG_MAX && typed.chars().count() > TYPED_MAX {
            return Err(t!("Snippet ist zu lang zum Eintippen").into());
        }
        self.send(RdpCmd::ReleaseAll);
        self.mods = [false; 3];
        let win_r = vec![
            RdpCmd::Key {
                code: SC_LWIN,
                down: true,
            },
            RdpCmd::Key {
                code: SC_R,
                down: true,
            },
            RdpCmd::Key {
                code: SC_R,
                down: false,
            },
            RdpCmd::Key {
                code: SC_LWIN,
                down: false,
            },
        ];
        self.queue(Duration::ZERO, Step::Cmds(win_r));
        if direct.chars().count() <= RUN_DIALOG_MAX {
            self.queue(Duration::from_millis(700), Step::Text(direct + "\n"));
        } else {
            self.queue(
                Duration::from_millis(700),
                Step::Text("powershell -NoExit\n".into()),
            );
            self.queue(Duration::from_millis(2500), Step::Text(typed + "\n"));
        }
        Ok(())
    }

    fn send(&self, cmd: RdpCmd) {
        if let Some(h) = &self.handle {
            h.send(&cmd);
        }
    }

    fn tap(&self, code: u16) {
        self.send(RdpCmd::Key { code, down: true });
        self.send(RdpCmd::Key { code, down: false });
    }

    /// Presses / releases Ctrl, Shift and Alt on the remote side to match `m`.
    fn sync_mods(&mut self, m: egui::Modifiers) {
        for (idx, (now, code)) in [(m.ctrl, SC_LCTRL), (m.shift, SC_LSHIFT), (m.alt, SC_LALT)]
            .into_iter()
            .enumerate()
        {
            if self.mods[idx] != now {
                self.mods[idx] = now;
                self.send(RdpCmd::Key { code, down: now });
            }
        }
    }

    fn connected(&self) -> bool {
        matches!(self.state, ConnState::Connected)
    }

    fn start(&mut self, launch: &RdpLaunch, size: (u16, u16), ctx: &egui::Context) {
        self.start_requested = false;
        let (sink, rx) = Sink::new(ctx.clone());
        *self.fb.lock().unwrap() = FrameBuffer::default();
        self.texture = None;
        self.requested = size;
        let connect = RdpConnect {
            host: launch.host.clone(),
            port: launch.port,
            username: launch.username.clone(),
            password: launch.password.clone().unwrap_or_default(),
            domain: launch.settings.domain.clone(),
            width: size.0,
            height: size.1,
            known_fingerprint: self.known_fingerprint.clone(),
        };
        match rdp_embed::spawn(connect, sink, self.fb.clone()) {
            Ok(h) => {
                self.handle = Some(h);
                self.rx = Some(rx);
                self.state = ConnState::Connecting(t!("Verbinde…").into());
            }
            Err(e) => self.state = ConnState::Closed(Some(format!("{e:#}"))),
        }
    }

    pub fn reconnect(&mut self) {
        self.handle = None;
        self.rx = None;
        self.steps.clear();
        self.files_offered = false;
        self.remote_files = false;
        self.transfer = None;
        self.pending_drop = None;
        self.start_requested = true;
        self.state = ConnState::Connecting(t!("Verbinde…").into());
    }

    /// Processes helper events (also while the tab is in the background).
    pub fn pump(
        &mut self,
        ctx: &egui::Context,
        host_id: &str,
        toasts: &mut Vec<(String, Color32)>,
        prompts: &mut Vec<HostKeyPrompt>,
    ) {
        if let Some(next) = self.run_steps() {
            ctx.request_repaint_after(next);
        }
        let Some(rx) = self.rx.as_mut() else { return };
        let mut file_events = Vec::new();
        let mut closed = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                RdpEvent::Status(s) => self.state = ConnState::Connecting(s),
                RdpEvent::Connected => self.state = ConnState::Connected,
                RdpEvent::Frame => {}
                RdpEvent::Cert {
                    fingerprint,
                    previous,
                } => {
                    let Some(sender) = self.handle.as_ref().map(RdpHandle::sender) else {
                        continue;
                    };
                    let (reply, answer) = tokio::sync::oneshot::channel();
                    prompts.push(HostKeyPrompt {
                        host: host_id.to_string(),
                        algorithm: t!("RDP · TLS-Zertifikat").into(),
                        fingerprint: fingerprint.clone(),
                        previous,
                        reply,
                    });
                    std::thread::spawn(move || {
                        let accept = answer.blocking_recv().unwrap_or(false);
                        sender.send(&RdpCmd::Trust { accept });
                    });
                    self.known_fingerprint = Some(fingerprint);
                }
                RdpEvent::ClipFromHost(text) => {
                    let wanted = self.auto_clipboard || self.announce_clip;
                    if !text.is_empty() && wanted {
                        ctx.copy_text(text.clone());
                        self.clip_local_seen = text.clone();
                        self.clip_remote = text;
                        if std::mem::take(&mut self.announce_clip) {
                            toasts.push((
                                t!("Zwischenablage vom Host übernommen").into(),
                                theme::accent(),
                            ));
                        }
                    }
                }
                RdpEvent::Files(ev) => file_events.push(ev),
                RdpEvent::Closed(err) => closed = Some(err),
            }
        }
        for ev in file_events {
            self.file_event(ev, ctx, toasts);
        }
        if let Some(err) = closed {
            if let Some(e) = &err {
                toasts.push((format!("RDP: {e}"), theme::red()));
            }
            self.state = ConnState::Closed(err);
            self.handle = None;
            self.rx = None;
        }
    }

    fn file_event(&mut self, ev: FileEv, ctx: &egui::Context, toasts: &mut Vec<(String, Color32)>) {
        match ev {
            FileEv::Offered {
                files,
                bytes,
                skipped,
            } => {
                self.files_offered = true;
                self.transfer = Some(Transfer {
                    up: true,
                    files,
                    done: 0,
                    total: bytes,
                    finished: None,
                    touched: Instant::now(),
                });
                if !skipped.is_empty() {
                    toasts.push((
                        t!(
                            "{} übersprungen (Links nach außen oder ungültige Namen)",
                            skipped.len()
                        ),
                        theme::amber(),
                    ));
                }
            }
            FileEv::Accepted => {
                if let Some(at) = self.pending_drop.take() {
                    self.paste_files(at);
                    ctx.request_repaint();
                }
            }
            FileEv::Progress { up, done, total } => {
                let t = self.transfer.get_or_insert(Transfer {
                    up,
                    files: 0,
                    done: 0,
                    total,
                    finished: None,
                    touched: Instant::now(),
                });
                t.up = up;
                t.touched = Instant::now();
                t.done = done;
                t.total = total;
                t.finished = (up && total > 0 && done >= total).then(Instant::now);
            }
            FileEv::Remote { files } => self.remote_files = files,
            FileEv::Saved { dir, files } => {
                if let Some(t) = &mut self.transfer {
                    t.finished = Some(Instant::now());
                }
                toasts.push((
                    t!("{} Dateien gespeichert in {}", files, dir),
                    theme::accent(),
                ));
            }
            FileEv::Error { code, detail } => {
                self.pending_drop = None;
                if code == "rejected" || code == "unsupported" {
                    self.files_offered = false;
                }
                self.transfer = None;
                toasts.push((file_error_text(&code, &detail), theme::red()));
            }
        }
    }

    /// Whether this view takes an OS file drop. The pane under the pointer does; when
    /// the position is unknown (Wayland reports none while dragging) only the active
    /// pane does, so exactly one view handles a drop.
    fn takes_drop(pointer: Option<Pos2>, area: Rect, active: bool) -> bool {
        match pointer {
            Some(p) => area.contains(p),
            None => active,
        }
    }

    /// Small pill at the bottom right while files go over the clipboard.
    fn transfer_badge(&mut self, ui: &egui::Ui, area: Rect) {
        let Some(t) = &self.transfer else { return };
        let idle = t.touched.elapsed()
            > if t.done == 0 {
                OFFER_IDLE
            } else {
                TRANSFER_IDLE
            };
        if idle
            || t.finished
                .is_some_and(|f| f.elapsed() > Duration::from_millis(2500))
        {
            self.transfer = None;
            return;
        }
        let arrow = if t.up { "↑" } else { "↓" };
        let what = if t.files > 0 {
            t!("{} Dateien", t.files)
        } else {
            t!("Dateien").to_string()
        };
        let text = format!(
            "{arrow} {what}  {} / {}",
            crate::ui::human_size(t.done),
            crate::ui::human_size(t.total)
        );
        let painter = ui.painter();
        let g = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), theme::text());
        let size = g.size() + Vec2::new(24.0, 14.0);
        let pill = Rect::from_min_size(
            egui::pos2(area.right() - size.x - 12.0, area.bottom() - size.y - 10.0),
            size,
        );
        painter.rect_filled(
            pill,
            CornerRadius::same(12),
            theme::lerp(theme::surface(), theme::accent(), 0.16),
        );
        let frac = if t.total > 0 {
            (t.done as f32 / t.total as f32).min(1.0)
        } else {
            0.0
        };
        let bar = Rect::from_min_size(
            pill.left_bottom() + Vec2::new(12.0, -4.0),
            Vec2::new((pill.width() - 24.0) * frac, 2.0),
        );
        painter.rect_filled(bar, CornerRadius::same(1), theme::accent());
        painter.galley(pill.min + Vec2::new(12.0, 6.0), g, theme::text());
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }

    /// Veil with a hint while OS files hover over the session.
    fn drop_overlay(ui: &egui::Ui, area: Rect, msg: &str, hint: Option<&str>) {
        let painter = ui.painter();
        painter.rect_filled(
            area.shrink(6.0),
            CornerRadius::same(10),
            Color32::from_rgba_unmultiplied(10, 12, 18, 200),
        );
        painter.rect_stroke(
            area.shrink(6.0),
            CornerRadius::same(10),
            egui::Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            msg,
            egui::FontId::proportional(16.0),
            theme::text(),
        );
        if let Some(h) = hint {
            painter.text(
                area.center() + Vec2::new(0.0, 24.0),
                egui::Align2::CENTER_CENTER,
                h,
                egui::FontId::proportional(12.5),
                theme::text_dim(),
            );
        }
    }

    /// Offers dropped files to the host and remembers where to paste them.
    fn drop_files(&mut self, paths: Vec<std::path::PathBuf>, at: Option<(u16, u16)>) {
        let paths = paths
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.pending_drop = Some(at);
        self.transfer = None;
        self.send(RdpCmd::OfferFiles { paths });
    }

    /// Offers new local clipboard text to the host (polled, since there is no change event).
    fn sync_local_clipboard(&mut self, force: bool) -> Option<String> {
        if !force && (!self.auto_clipboard || self.clip_poll.elapsed() < Duration::from_millis(800))
        {
            return None;
        }
        self.clip_poll = Instant::now();
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        let text = self.clipboard.as_mut()?.get_text().ok()?;
        if force || (text != self.clip_local_seen && text != self.clip_remote) {
            self.clip_local_seen = text.clone();
            self.send(RdpCmd::ClipSet { text: text.clone() });
            return Some(text);
        }
        None
    }

    fn type_text(&mut self, text: &str) {
        self.sync_mods(egui::Modifiers::NONE);
        for ch in text.chars().take(10_000) {
            match ch {
                '\r' => {}
                '\n' => self.tap(SC_ENTER),
                '\t' => self.tap(0x0F),
                c => {
                    self.send(RdpCmd::Unicode { ch: c, down: true });
                    self.send(RdpCmd::Unicode { ch: c, down: false });
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        index: usize,
        title: &str,
        subtitle: &str,
        launch: &RdpLaunch,
        ps_snippets: &[(uuid::Uuid, String)],
        active: bool,
        toasts: &mut Vec<(String, Color32)>,
    ) -> RdpAction {
        let mut action = RdpAction::None;
        let connected = self.connected();

        let state = crate::ui::chrome::state_pair(&self.state);
        crate::ui::chrome::session_header(
            ui,
            id.with("bar"),
            index,
            title,
            subtitle,
            state,
            |ui| {
                if theme::icon_btn(
                    ui,
                    crate::ui::icons::Icon::Monitor,
                    t!("Im System-RDP-Client öffnen"),
                )
                .clicked()
                {
                    action = RdpAction::OpenExternal;
                }
                if matches!(self.state, ConnState::Closed(_)) {
                    if theme::icon_button(ui, "⟳", t!("Neu verbinden")).clicked() {
                        self.reconnect();
                    }
                    return;
                }
                ui.add_enabled_ui(connected, |ui| {
                    if theme::icon_button(ui, "⊞", t!("Windows-Taste")).clicked() {
                        self.tap(SC_LWIN);
                    }
                    if theme::icon_button(ui, "⌨", t!("Zwischenablage als Tastatureingabe tippen"))
                        .clicked()
                    {
                        let text = self
                            .clipboard
                            .get_or_insert_with(|| arboard::Clipboard::new().expect("clipboard"))
                            .get_text()
                            .unwrap_or_default();
                        self.type_text(&text);
                    }
                    if theme::icon_button(ui, "📤", t!("Lokale Zwischenablage an den Host senden"))
                        .clicked()
                    {
                        match self.sync_local_clipboard(true) {
                            Some(_) => toasts.push((
                                t!("Zwischenablage an Host gesendet").into(),
                                theme::accent(),
                            )),
                            None => toasts.push((
                                t!("Lokale Zwischenablage ist leer").into(),
                                theme::amber(),
                            )),
                        }
                    }
                    if theme::icon_button(ui, "📥", t!("Zwischenablage vom Host kopieren"))
                        .clicked()
                    {
                        self.announce_clip = true;
                        self.send(RdpCmd::ClipGet);
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Ctrl+Alt+Del").size(12.5)))
                        .on_hover_text(t!("Sicherheitsbildschirm / Anmeldung"))
                        .clicked()
                    {
                        self.send(RdpCmd::CtrlAltDel);
                    }
                    let ps = theme::icon_btn(ui, Icon::Bolt, t!("PowerShell-Snippet ausführen"));
                    egui::Popup::menu(&ps).show(|ui| {
                        ui.set_min_width(220.0);
                        if ps_snippets.is_empty() {
                            ui.label(
                                RichText::new(t!(
                                    "Keine PowerShell-Snippets – im Snippet-Editor „PowerShell“ wählen"
                                ))
                                .color(theme::text_dim()),
                            );
                        }
                        for (sid, name) in ps_snippets {
                            if ui.button(name).clicked() {
                                action = RdpAction::RunSnippet(*sid);
                                ui.close();
                            }
                        }
                    });
                    if self.auto_clipboard && self.remote_files {
                        let r = theme::btn(
                            ui,
                            Btn::Small,
                            Some(Icon::Download),
                            t!("Dateien vom Host speichern"),
                        )
                        .on_hover_text(t!("Kopierte Dateien des Hosts in Downloads/Aurora speichern"));
                        if r.clicked() {
                            self.transfer = None;
                            self.send(RdpCmd::FetchFiles {
                                dir: download_target().display().to_string(),
                            });
                        }
                    }
                    if self.auto_clipboard && self.files_offered {
                        let r = theme::btn(ui, Btn::Small, Some(Icon::File), t!("Dateien einfügen"))
                            .on_hover_text(t!("Abgelegte Dateien erneut einfügen (Strg+V auf dem Host)"));
                        if r.clicked() {
                            self.paste_files(None);
                        }
                    }
                });
            },
        );

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| {
                let area = ui.max_rect();
                let ppp = ui.ctx().pixels_per_point();
                let want = (
                    ((area.width() * ppp) as u16 & !1).clamp(640, 8192),
                    ((area.height() * ppp) as u16 & !1).clamp(480, 8192),
                );
                if self.start_requested {
                    self.start(launch, want, ui.ctx());
                }

                // Upload changed pixels.
                {
                    let mut fb = self.fb.lock().unwrap();
                    if let Some((x, y, w, h)) = fb.dirty.take() {
                        let full_size = [fb.width, fb.height];
                        let tex_matches =
                            self.texture.as_ref().is_some_and(|t| t.size() == full_size);
                        if !tex_matches || (w, h) == (fb.width, fb.height) {
                            let img = egui::ColorImage::from_rgba_unmultiplied(full_size, &fb.rgba);
                            match &mut self.texture {
                                Some(t) if tex_matches => t.set(img, egui::TextureOptions::LINEAR),
                                _ => {
                                    self.texture = Some(ui.ctx().load_texture(
                                        "rdp-frame",
                                        img,
                                        egui::TextureOptions::LINEAR,
                                    ))
                                }
                            }
                        } else if let Some(t) = &mut self.texture {
                            let mut sub = Vec::with_capacity(w * h * 4);
                            for row in y..y + h {
                                let start = (row * fb.width + x) * 4;
                                sub.extend_from_slice(&fb.rgba[start..start + w * 4]);
                            }
                            let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &sub);
                            t.set_partial([x, y], img, egui::TextureOptions::LINEAR);
                        }
                    }
                }

                let resp = ui.interact(area, id.with("screen"), Sense::click_and_drag());
                let Some(tex) = &self.texture else {
                    let msg = match &self.state {
                        ConnState::Closed(Some(e)) => e.clone(),
                        ConnState::Closed(None) => t!("Getrennt").into(),
                        _ => t!("Verbinde…").into(),
                    };
                    ui.painter().text(
                        area.center(),
                        egui::Align2::CENTER_CENTER,
                        msg,
                        egui::FontId::proportional(15.0),
                        theme::text_dim(),
                    );
                    return;
                };

                // Fit the remote screen into the area, keeping the aspect ratio.
                let [tw, th] = tex.size();
                let scale = (area.width() / tw as f32).min(area.height() / th as f32);
                let scale = if (scale * ppp - 1.0).abs() < 0.02 {
                    1.0 / ppp
                } else {
                    scale
                };
                let size = Vec2::new(tw as f32, th as f32) * scale;
                let rect = Rect::from_center_size(area.center(), size);
                ui.painter().image(
                    tex.id(),
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                if !connected {
                    ui.painter().rect_filled(
                        area,
                        CornerRadius::ZERO,
                        Color32::from_black_alpha(150),
                    );
                }

                // Follow the tab size (debounced; the server may reconnect for it).
                if connected && want != self.requested {
                    match self.pending_resize {
                        Some((t, size))
                            if size == want && t.elapsed() > Duration::from_millis(600) =>
                        {
                            self.send(RdpCmd::Resize {
                                w: want.0,
                                h: want.1,
                            });
                            self.requested = want;
                            self.pending_resize = None;
                        }
                        Some((_, size)) if size == want => {}
                        _ => self.pending_resize = Some((Instant::now(), want)),
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(650));
                }

                // Files from the local desktop: overlay while hovering, offer on drop.
                let (hovering, dropped, pointer) = ui.input(|i| {
                    (
                        !i.raw.hovered_files.is_empty(),
                        i.raw
                            .dropped_files
                            .iter()
                            .map(|f| f.path().to_path_buf())
                            .filter(|p| !p.as_os_str().is_empty())
                            .collect::<Vec<_>>(),
                        i.pointer.latest_pos(),
                    )
                });
                let over_me = Self::takes_drop(pointer, area, active);
                if hovering && over_me {
                    let (msg, hint) = if !connected {
                        (t!("Nicht verbunden").to_string(), None)
                    } else if !self.auto_clipboard {
                        (
                            t!("Zwischenablage ist für diesen Host ausgeschaltet").to_string(),
                            None,
                        )
                    } else {
                        (
                            t!("Loslassen zum Kopieren auf {}", title),
                            Some(t!("Große Dateien gehen per Dateibrowser (SFTP) schneller")),
                        )
                    };
                    Self::drop_overlay(ui, area, &msg, hint);
                }
                if !dropped.is_empty() && over_me && connected {
                    if self.auto_clipboard {
                        let at = pointer.filter(|p| rect.contains(*p)).map(|p| {
                            let x = ((p.x - rect.left()) / rect.width() * tw as f32)
                                .clamp(0.0, tw as f32 - 1.0);
                            let y = ((p.y - rect.top()) / rect.height() * th as f32)
                                .clamp(0.0, th as f32 - 1.0);
                            (x as u16, y as u16)
                        });
                        self.drop_files(dropped, at);
                    } else {
                        toasts.push((
                            t!("Zwischenablage ist für diesen Host ausgeschaltet").into(),
                            theme::amber(),
                        ));
                    }
                }
                self.transfer_badge(ui, area);

                if !connected {
                    return;
                }
                if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
                    resp.request_focus();
                }
                let focused = resp.has_focus();
                self.handle_input(ui, &resp, rect, (tw, th), focused);
            });

        action
    }

    fn handle_input(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        size: (usize, usize),
        focused: bool,
    ) {
        let to_remote = |p: Pos2| -> (u16, u16) {
            let x = ((p.x - rect.left()) / rect.width() * size.0 as f32)
                .clamp(0.0, size.0 as f32 - 1.0);
            let y = ((p.y - rect.top()) / rect.height() * size.1 as f32)
                .clamp(0.0, size.1 as f32 - 1.0);
            (x as u16, y as u16)
        };

        if resp.hovered() {
            // The remote cursor is rendered into the image.
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            if let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| rect.contains(*p)) {
                let pos = to_remote(p);
                if self.last_pos != Some(pos) {
                    self.last_pos = Some(pos);
                    self.send(RdpCmd::Move { x: pos.0, y: pos.1 });
                }
            }
        }

        if self.had_focus && !focused {
            self.send(RdpCmd::ReleaseAll);
            self.mods = [false; 3];
        }
        self.had_focus = focused;

        // Mouse buttons and wheel work on hover; the keyboard needs focus.
        let events = ui.input(|i| i.events.clone());
        for ev in &events {
            match ev {
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } if rect.contains(*pos) || !pressed => {
                    let b = match button {
                        egui::PointerButton::Primary => 0,
                        egui::PointerButton::Middle => 1,
                        egui::PointerButton::Secondary => 2,
                        egui::PointerButton::Extra1 => 3,
                        egui::PointerButton::Extra2 => 4,
                    };
                    let (x, y) = to_remote(*pos);
                    self.send(RdpCmd::Move { x, y });
                    self.send(RdpCmd::Button { b, down: *pressed });
                }
                Event::MouseWheel { unit, delta, .. } if resp.hovered() => {
                    let per = match unit {
                        egui::MouseWheelUnit::Line => 120.0,
                        egui::MouseWheelUnit::Page => 360.0,
                        egui::MouseWheelUnit::Point => 2.4,
                    };
                    for (vertical, d) in [(true, delta.y), (false, -delta.x)] {
                        let units = (d * per).clamp(-1200.0, 1200.0) as i16;
                        if units != 0 {
                            self.send(RdpCmd::Wheel { vertical, units });
                        }
                    }
                }
                _ => {}
            }
        }

        if !focused {
            return;
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });

        let frame_mods = ui.input(|i| i.modifiers);
        if let Some(t) = self.pending_paste
            && t.elapsed() > Duration::from_millis(150)
        {
            self.pending_paste = None;
            self.sync_mods(egui::Modifiers {
                ctrl: true,
                ..frame_mods
            });
            self.tap(0x2F);
        }

        let ctrl_combo = |this: &mut Self, code: u16, base: egui::Modifiers| {
            this.sync_mods(egui::Modifiers { ctrl: true, ..base });
            this.tap(code);
        };
        for ev in events {
            match ev {
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    self.sync_mods(modifiers);
                    let k = physical_key.unwrap_or(key);
                    if rdp_embed::is_text_key(k)
                        && !modifiers.ctrl
                        && !modifiers.alt
                        && !modifiers.command
                    {
                        continue; // arrives as Event::Text
                    }
                    if let Some(code) = rdp_embed::scancode(k) {
                        self.send(RdpCmd::Key {
                            code,
                            down: pressed,
                        });
                    }
                }
                Event::Text(t) => {
                    // Unicode events carry the final character; a held Shift confuses some
                    // servers (xrdp drops e.g. '+'), so lift it for the duration.
                    let shift = self.mods[1];
                    if shift {
                        self.send(RdpCmd::Key {
                            code: SC_LSHIFT,
                            down: false,
                        });
                    }
                    for ch in t.chars() {
                        self.send(RdpCmd::Unicode { ch, down: true });
                        self.send(RdpCmd::Unicode { ch, down: false });
                    }
                    if shift {
                        self.send(RdpCmd::Key {
                            code: SC_LSHIFT,
                            down: true,
                        });
                    }
                }
                // egui turns Ctrl/Cmd+C/X/V into these events; forward them as Ctrl combos.
                Event::Copy => ctrl_combo(self, 0x2E, frame_mods),
                Event::Cut => ctrl_combo(self, 0x2D, frame_mods),
                Event::Paste(text) => {
                    self.sync_mods(egui::Modifiers {
                        ctrl: true,
                        ..frame_mods
                    });
                    // Make sure the host has our clipboard before it handles Ctrl+V.
                    if text != self.clip_local_seen && text != self.clip_remote {
                        self.clip_local_seen = text.clone();
                        self.send(RdpCmd::ClipSet { text });
                        self.pending_paste = Some(Instant::now());
                        ui.ctx().request_repaint_after(Duration::from_millis(160));
                    } else {
                        self.tap(0x2F);
                    }
                }
                _ => {}
            }
        }
        if self.pending_paste.is_none() {
            self.sync_mods(frame_mods);
        }

        self.sync_local_clipboard(false);
        ui.ctx().request_repaint_after(Duration::from_millis(900));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64_of(s: &str) -> String {
        encoded_command(s).rsplit(' ').next().unwrap().to_string()
    }

    #[test]
    fn powershell_command_is_utf16le_base64() {
        use base64::Engine;
        let b64 = b64_of("Get-Date\r\nWrite-Host 'Grüße'");
        assert!(encoded_command("x").starts_with("powershell -NoExit -EncodedCommand "));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            "Get-Date\nWrite-Host 'Grüße'"
        );
        assert!(typed_command("x").contains(&b64_of("x")));
        assert!(download_target().ends_with("Aurora"));
    }

    #[test]
    fn exactly_one_pane_takes_a_drop() {
        let left = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let right = Rect::from_min_max(Pos2::new(110.0, 0.0), Pos2::new(210.0, 100.0));
        let panes = [(left, false), (right, true)];
        let takers = |pointer: Option<Pos2>| {
            panes
                .iter()
                .filter(|(area, active)| RdpView::takes_drop(pointer, *area, *active))
                .count()
        };
        // Position known: the pane under the pointer, even if it is not the active one.
        assert!(RdpView::takes_drop(
            Some(Pos2::new(50.0, 50.0)),
            left,
            false
        ));
        assert_eq!(takers(Some(Pos2::new(50.0, 50.0))), 1);
        assert_eq!(takers(Some(Pos2::new(105.0, 50.0))), 0);
        // Unknown (Wayland while dragging): only the active pane.
        assert!(!RdpView::takes_drop(None, left, false));
        assert!(RdpView::takes_drop(None, right, true));
        assert_eq!(takers(None), 1);
    }
}
