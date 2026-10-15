//! Embedded RDP viewer tab: renders the helper's framebuffer and forwards input.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Event, Margin, Pos2, Rect, RichText, Sense, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::ConnState;
use crate::backend::rdp::RdpLaunch;
use crate::backend::rdp_embed::{
    self, FrameBuffer, RdpCmd, RdpConnect, RdpEvent, RdpHandle, SC_ENTER, SC_LALT, SC_LCTRL,
    SC_LSHIFT, SC_LWIN,
};
use crate::backend::{HostKeyPrompt, Sink};
use crate::theme;

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
    pub auto_clipboard: bool,
}

/// What the tab wants the app to do.
pub enum RdpAction {
    None,
    OpenExternal,
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
            state: ConnState::Connecting("Verbinde…".into()),
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
        }
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
                self.state = ConnState::Connecting("Verbinde…".into());
            }
            Err(e) => self.state = ConnState::Closed(Some(format!("{e:#}"))),
        }
    }

    pub fn reconnect(&mut self) {
        self.handle = None;
        self.rx = None;
        self.start_requested = true;
        self.state = ConnState::Connecting("Verbinde…".into());
    }

    /// Processes helper events (also while the tab is in the background).
    pub fn pump(
        &mut self,
        ctx: &egui::Context,
        host_id: &str,
        toasts: &mut Vec<(String, Color32)>,
        prompts: &mut Vec<HostKeyPrompt>,
    ) {
        let Some(rx) = self.rx.as_mut() else { return };
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
                        algorithm: "RDP · TLS-Zertifikat".into(),
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
                            toasts
                                .push(("Zwischenablage vom Host übernommen".into(), theme::teal()));
                        }
                    }
                }
                RdpEvent::Closed(err) => closed = Some(err),
            }
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

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        title: &str,
        launch: &RdpLaunch,
        toasts: &mut Vec<(String, Color32)>,
    ) -> RdpAction {
        let mut action = RdpAction::None;
        let connected = self.connected();

        egui::Panel::top(id.with("bar"))
            .exact_size(40.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_panel())
                    .inner_margin(Margin::symmetric(12, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("⏺")
                            .color(crate::app::state_color(&self.state))
                            .size(10.0),
                    );
                    ui.label(RichText::new(title).strong());
                    if let ConnState::Connecting(s) | ConnState::Closed(Some(s)) = &self.state {
                        ui.add(
                            egui::Label::new(RichText::new(s).color(theme::text_dim()).size(12.0))
                                .truncate(),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, "↗", "Im System-RDP-Client öffnen").clicked() {
                            action = RdpAction::OpenExternal;
                        }
                        if matches!(self.state, ConnState::Closed(_)) {
                            if theme::icon_button(ui, "⟳", "Neu verbinden").clicked() {
                                self.reconnect();
                            }
                            return;
                        }
                        ui.add_enabled_ui(connected, |ui| {
                            if theme::icon_button(ui, "⊞", "Windows-Taste").clicked() {
                                self.tap(SC_LWIN);
                            }
                            if theme::icon_button(
                                ui,
                                "⌨",
                                "Zwischenablage als Tastatureingabe tippen",
                            )
                            .clicked()
                            {
                                let text = self
                                    .clipboard
                                    .get_or_insert_with(|| {
                                        arboard::Clipboard::new().expect("clipboard")
                                    })
                                    .get_text()
                                    .unwrap_or_default();
                                self.type_text(&text);
                            }
                            if theme::icon_button(
                                ui,
                                "📤",
                                "Lokale Zwischenablage an den Host senden",
                            )
                            .clicked()
                            {
                                match self.sync_local_clipboard(true) {
                                    Some(_) => toasts.push((
                                        "Zwischenablage an Host gesendet".into(),
                                        theme::teal(),
                                    )),
                                    None => toasts.push((
                                        "Lokale Zwischenablage ist leer".into(),
                                        theme::amber(),
                                    )),
                                }
                            }
                            if theme::icon_button(ui, "📥", "Zwischenablage vom Host kopieren")
                                .clicked()
                            {
                                self.announce_clip = true;
                                self.send(RdpCmd::ClipGet);
                            }
                            if ui
                                .add(egui::Button::new(RichText::new("Ctrl+Alt+Del").size(12.5)))
                                .on_hover_text("Sicherheitsbildschirm / Anmeldung")
                                .clicked()
                            {
                                self.send(RdpCmd::CtrlAltDel);
                            }
                        });
                    });
                });
            });

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
                        ConnState::Closed(None) => "Getrennt".into(),
                        _ => "Verbinde…".into(),
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
