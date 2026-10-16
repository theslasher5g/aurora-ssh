//! Embedded VNC viewer: renders the session's framebuffer and forwards input.

use std::sync::{Arc, Mutex};

use egui::{Color32, CornerRadius, Event, Pos2, Rect, Sense, Vec2};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::app::ConnState;
use crate::backend::Sink;
use crate::backend::vnc::{self, FrameBuffer, VncCmd, VncEvent, VncParams};
use crate::theme;

const XK_CONTROL_L: u32 = 0xffe3;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_ALT_L: u32 = 0xffe9;
const XK_SUPER_L: u32 = 0xffeb;
const XK_DELETE: u32 = 0xffff;
const XK_RETURN: u32 = 0xff0d;
const XK_TAB: u32 = 0xff09;

/// Modifier keysyms in the order of `VncView::mods`.
const MOD_KEYSYMS: [u32; 4] = [XK_CONTROL_L, XK_SHIFT_L, XK_ALT_L, XK_SUPER_L];

// Not wired into the app yet.
pub struct VncView {
    pub state: ConnState,
    params: VncParams,
    tx: Option<UnboundedSender<VncCmd>>,
    rx: Option<UnboundedReceiver<VncEvent>>,
    fb: Arc<Mutex<FrameBuffer>>,
    texture: Option<egui::TextureHandle>,
    /// Region to upload as (x0, y0, x1, y1).
    damage: Option<(usize, usize, usize, usize)>,
    full_upload: bool,
    ctx: Option<egui::Context>,
    /// Desktop name reported by the server.
    pub name: String,
    /// Remote desktop size in pixels.
    pub size: (u16, u16),
    /// Scale to fit (true) or show 1:1 with scroll bars.
    fit: bool,
    buttons: u8,
    last_pos: Option<(u16, u16)>,
    /// Ctrl, Shift, Alt, Super as currently pressed on the remote side.
    mods: [bool; 4],
    wheel: Vec2,
    had_focus: bool,
    clipboard: Option<arboard::Clipboard>,
    last_server_cut: String,
    toasts: Vec<String>,
}

impl VncView {
    /// Creates the view; [`Self::start`] connects.
    pub fn new(params: VncParams) -> Self {
        Self {
            state: ConnState::Connecting(t!("Verbinde…").into()),
            params,
            tx: None,
            rx: None,
            fb: Arc::new(Mutex::new(FrameBuffer::default())),
            texture: None,
            damage: None,
            full_upload: true,
            ctx: None,
            name: String::new(),
            size: (0, 0),
            fit: true,
            buttons: 0,
            last_pos: None,
            mods: [false; 4],
            wheel: Vec2::ZERO,
            had_focus: false,
            clipboard: None,
            last_server_cut: String::new(),
            toasts: Vec::new(),
        }
    }

    /// (Re)connects; an existing session is closed first.
    pub fn start(&mut self, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        self.close();
        let (sink, rx) = Sink::new(ctx.clone());
        // A fresh framebuffer, so a late write of the old session cannot leak in.
        self.fb = Arc::new(Mutex::new(FrameBuffer::default()));
        self.texture = None;
        self.damage = None;
        self.full_upload = true;
        self.buttons = 0;
        self.last_pos = None;
        self.mods = [false; 4];
        self.ctx = Some(ctx.clone());
        self.tx = Some(vnc::spawn(rt, self.params.clone(), self.fb.clone(), sink));
        self.rx = Some(rx);
        self.state = ConnState::Connecting(t!("Verbinde…").into());
    }

    pub fn close(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(VncCmd::Close);
        }
        self.rx = None;
        if !matches!(self.state, ConnState::Closed(_)) {
            self.state = ConnState::Closed(None);
        }
    }

    fn connected(&self) -> bool {
        matches!(self.state, ConnState::Connected)
    }

    fn send(&self, cmd: VncCmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    fn input_allowed(&self) -> bool {
        self.connected() && !self.params.view_only
    }

    /// Processes session events (also while the view is hidden); returns toast texts.
    pub fn pump(&mut self) -> Vec<String> {
        let mut toasts = std::mem::take(&mut self.toasts);
        let Some(rx) = self.rx.as_mut() else {
            return toasts;
        };
        let mut closed = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                VncEvent::Status(s) => self.state = ConnState::Connecting(s),
                VncEvent::Connected {
                    name,
                    width,
                    height,
                } => {
                    self.name = name;
                    self.size = (width, height);
                    self.state = ConnState::Connected;
                    self.full_upload = true;
                }
                VncEvent::Resized { width, height } => {
                    self.size = (width, height);
                    self.full_upload = true;
                }
                VncEvent::Damage { x, y, w, h } => {
                    let (x, y) = (x as usize, y as usize);
                    let (x1, y1) = (x + w as usize, y + h as usize);
                    self.damage = Some(match self.damage {
                        None => (x, y, x1, y1),
                        Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x1), d.max(y1)),
                    });
                }
                VncEvent::ServerCut(text) => {
                    if !text.is_empty() && text != self.last_server_cut {
                        if let Some(ctx) = &self.ctx {
                            ctx.copy_text(text.clone());
                        }
                        self.last_server_cut = text;
                        toasts.push(t!("Zwischenablage vom Host übernommen").into());
                    }
                }
                VncEvent::Closed(err) => closed = Some(err),
            }
        }
        if let Some(err) = closed {
            self.state = ConnState::Closed(err);
            self.tx = None;
            self.rx = None;
        }
        toasts
    }

    /// Buttons for the session header.
    pub fn toolbar(&mut self, ui: &mut egui::Ui) {
        let (glyph, tip) = if self.fit {
            ("1:1", t!("Originalgröße (1:1)"))
        } else {
            ("⤢", t!("Ansicht anpassen"))
        };
        if theme::icon_button(ui, glyph, tip).clicked() {
            self.fit = !self.fit;
        }
        let connected = self.connected();
        ui.add_enabled_ui(connected, |ui| {
            if theme::icon_button(ui, "⟳", t!("Bild neu anfordern")).clicked() {
                self.send(VncCmd::Refresh);
            }
        });
        ui.add_enabled_ui(self.input_allowed(), |ui| {
            if theme::icon_button(ui, "⌨", t!("Zwischenablage als Tastatureingabe tippen"))
                .clicked()
                && let Some(text) = self.local_clipboard()
            {
                self.type_text(&text);
            }
            if theme::icon_button(ui, "📤", t!("Zwischenablage senden")).clicked() {
                match self.local_clipboard() {
                    Some(text) => {
                        self.send(VncCmd::ClientCut(text));
                        self.toasts
                            .push(t!("Zwischenablage an Host gesendet").into());
                    }
                    None => self
                        .toasts
                        .push(t!("Lokale Zwischenablage ist leer").into()),
                }
            }
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("Ctrl+Alt+Del").size(12.5),
                ))
                .on_hover_text(t!("Strg+Alt+Entf an den Host senden"))
                .clicked()
            {
                self.release_all();
                for k in [XK_CONTROL_L, XK_ALT_L, XK_DELETE] {
                    self.send(VncCmd::Key {
                        keysym: k,
                        down: true,
                    });
                }
                for k in [XK_DELETE, XK_ALT_L, XK_CONTROL_L] {
                    self.send(VncCmd::Key {
                        keysym: k,
                        down: false,
                    });
                }
            }
        });
    }

    fn local_clipboard(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard
            .as_mut()?
            .get_text()
            .ok()
            .filter(|t| !t.is_empty())
    }

    fn tap(&self, keysym: u32) {
        self.send(VncCmd::Key { keysym, down: true });
        self.send(VncCmd::Key {
            keysym,
            down: false,
        });
    }

    fn type_text(&mut self, text: &str) {
        self.sync_mods(egui::Modifiers::NONE);
        for ch in text.chars().take(10_000) {
            match ch {
                '\r' => {}
                '\n' => self.tap(XK_RETURN),
                '\t' => self.tap(XK_TAB),
                c => self.tap(char_keysym(c)),
            }
        }
    }

    /// Presses / releases Ctrl, Shift, Alt and Super on the remote side to match `m`.
    fn sync_mods(&mut self, m: egui::Modifiers) {
        let want = [m.ctrl, m.shift, m.alt, m.mac_cmd];
        for i in 0..4 {
            if self.mods[i] != want[i] {
                self.mods[i] = want[i];
                self.send(VncCmd::Key {
                    keysym: MOD_KEYSYMS[i],
                    down: want[i],
                });
            }
        }
    }

    /// Lifts held modifiers and mouse buttons (focus lost, Ctrl+Alt+Del).
    fn release_all(&mut self) {
        self.sync_mods(egui::Modifiers::NONE);
        if self.buttons != 0 {
            self.buttons = 0;
            if let Some((x, y)) = self.last_pos {
                self.send(VncCmd::Pointer { x, y, buttons: 0 });
            }
        }
    }

    /// Uploads changed framebuffer regions into the texture.
    fn upload(&mut self, ctx: &egui::Context) {
        let fb = self.fb.lock().unwrap_or_else(|p| p.into_inner());
        if fb.width == 0 || fb.height == 0 || fb.rgba.len() != fb.width * fb.height * 4 {
            return;
        }
        let size = [fb.width, fb.height];
        let tex_ok = self.texture.as_ref().is_some_and(|t| t.size() == size);
        let opts = egui::TextureOptions::LINEAR;
        if !tex_ok || self.full_upload {
            let img = egui::ColorImage::from_rgba_unmultiplied(size, &fb.rgba);
            match &mut self.texture {
                Some(t) if tex_ok => t.set(img, opts),
                _ => self.texture = Some(ctx.load_texture("vnc-frame", img, opts)),
            }
            self.full_upload = false;
            self.damage = None;
            return;
        }
        let Some((x0, y0, x1, y1)) = self.damage.take() else {
            return;
        };
        let (x1, y1) = (x1.min(fb.width), y1.min(fb.height));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut sub = Vec::with_capacity(w * h * 4);
        for row in y0..y1 {
            let start = (row * fb.width + x0) * 4;
            sub.extend_from_slice(&fb.rgba[start..start + w * 4]);
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &sub);
        if let Some(t) = &mut self.texture {
            t.set_partial([x0, y0], img, opts);
        }
    }

    /// Draws the remote screen into the available space and handles input.
    pub fn show(&mut self, ui: &mut egui::Ui, id: egui::Id) {
        self.upload(ui.ctx());
        let area = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(area, CornerRadius::ZERO, theme::bg_deep());
        let ppp = ui.ctx().pixels_per_point();

        let Some(tex) = self.texture.as_ref().map(|t| (t.id(), t.size())) else {
            ui.allocate_rect(area, Sense::hover());
            self.overlay(ui, area);
            return;
        };
        let (tex_id, [tw, th]) = tex;
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

        if self.fit {
            let resp = ui.interact(area, id.with("screen"), Sense::click_and_drag());
            ui.allocate_rect(area, Sense::hover());
            let scale = (area.width() / tw as f32).min(area.height() / th as f32);
            // Snap to exact pixels when the fit is (almost) 1:1.
            let scale = if (scale * ppp - 1.0).abs() < 0.02 {
                1.0 / ppp
            } else {
                scale
            };
            let rect =
                Rect::from_center_size(area.center(), Vec2::new(tw as f32, th as f32) * scale);
            ui.painter().image(tex_id, rect, uv, Color32::WHITE);
            self.overlay(ui, area);
            self.handle_input(ui, &resp, rect, (tw, th));
        } else {
            let size = Vec2::new(tw as f32, th as f32) / ppp;
            let mut out = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                egui::ScrollArea::both()
                    .id_salt(id.with("scroll"))
                    .auto_shrink(false)
                    .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR)
                    .show(ui, |ui| {
                        let inner = ui.available_size();
                        let (outer, _) = ui.allocate_exact_size(size.max(inner), Sense::hover());
                        let rect = Rect::from_center_size(outer.center(), size);
                        let rect = if size.x > inner.x || size.y > inner.y {
                            Rect::from_min_size(
                                Pos2::new(
                                    if size.x > inner.x {
                                        outer.left()
                                    } else {
                                        rect.left()
                                    },
                                    if size.y > inner.y {
                                        outer.top()
                                    } else {
                                        rect.top()
                                    },
                                ),
                                size,
                            )
                        } else {
                            rect
                        };
                        let resp = ui.interact(outer, id.with("screen"), Sense::click_and_drag());
                        ui.painter().image(tex_id, rect, uv, Color32::WHITE);
                        out = Some((resp, rect));
                    });
            });
            self.overlay(ui, area);
            if let Some((resp, rect)) = out {
                self.handle_input(ui, &resp, rect, (tw, th));
            }
        }
    }

    /// Veil + message while connecting or after the session ended.
    fn overlay(&self, ui: &egui::Ui, area: Rect) {
        let msg = match &self.state {
            ConnState::Connected => return,
            ConnState::Connecting(s) => s.clone(),
            ConnState::Closed(Some(e)) => e.clone(),
            ConnState::Closed(None) => t!("Getrennt").into(),
        };
        let painter = ui.painter_at(area);
        if self.texture.is_some() {
            let veil = Color32::from_rgba_unmultiplied(10, 12, 18, 200);
            painter.rect_filled(area, CornerRadius::ZERO, veil);
        }
        let galley = painter.layout(
            msg,
            egui::FontId::proportional(15.0),
            theme::text(),
            (area.width() - 48.0).clamp(120.0, 560.0),
        );
        let pos = area.center() - galley.size() / 2.0;
        painter.galley(pos, galley, theme::text());
    }

    fn handle_input(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        size: (usize, usize),
    ) {
        if !self.input_allowed() {
            return;
        }
        if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if self.had_focus && !focused {
            self.release_all();
        }
        self.had_focus = focused;

        let to_remote = |p: Pos2| -> (u16, u16) {
            let x = ((p.x - rect.left()) / rect.width() * size.0 as f32)
                .clamp(0.0, size.0 as f32 - 1.0);
            let y = ((p.y - rect.top()) / rect.height() * size.1 as f32)
                .clamp(0.0, size.1 as f32 - 1.0);
            (x as u16, y as u16)
        };
        let clip = ui.clip_rect().intersect(rect);
        let hovered = resp.hovered();

        if hovered && let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| clip.contains(*p)) {
            let pos = to_remote(p);
            if self.last_pos != Some(pos) {
                self.last_pos = Some(pos);
                self.send(VncCmd::Pointer {
                    x: pos.0,
                    y: pos.1,
                    buttons: self.buttons,
                });
            }
        }

        let events = ui.input(|i| i.events.clone());
        for ev in &events {
            match ev {
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } if (hovered && clip.contains(*pos)) || !pressed => {
                    let bit = match button {
                        egui::PointerButton::Primary => 1,
                        egui::PointerButton::Middle => 2,
                        egui::PointerButton::Secondary => 4,
                        _ => continue,
                    };
                    let before = self.buttons;
                    if *pressed {
                        self.buttons |= bit;
                    } else {
                        self.buttons &= !bit;
                    }
                    if before == self.buttons {
                        continue;
                    }
                    let (x, y) = to_remote(*pos);
                    self.last_pos = Some((x, y));
                    self.send(VncCmd::Pointer {
                        x,
                        y,
                        buttons: self.buttons,
                    });
                }
                Event::MouseWheel { unit, delta, .. } if hovered => {
                    let lines = match unit {
                        egui::MouseWheelUnit::Line => *delta,
                        egui::MouseWheelUnit::Page => *delta * 3.0,
                        egui::MouseWheelUnit::Point => *delta / 40.0,
                    };
                    self.wheel += lines;
                    self.flush_wheel();
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
        let ctrl_combo = |this: &mut Self, letter: char| {
            this.sync_mods(egui::Modifiers {
                ctrl: true,
                ..frame_mods
            });
            this.tap(letter as u32);
        };
        for ev in events {
            match ev {
                Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    self.sync_mods(modifiers);
                    if let Some(keysym) = special_keysym(key) {
                        self.send(VncCmd::Key {
                            keysym,
                            down: pressed,
                        });
                    } else if modifiers.ctrl || modifiers.alt || modifiers.mac_cmd {
                        // No Text event arrives for these combos: send the key itself.
                        if let Some(keysym) = combo_keysym(key) {
                            self.send(VncCmd::Key {
                                keysym,
                                down: pressed,
                            });
                        }
                    }
                }
                Event::Text(t) => {
                    for ch in t.chars() {
                        self.tap(char_keysym(ch));
                    }
                }
                Event::Ime(egui::ImeEvent::Commit(t)) => {
                    for ch in t.chars() {
                        self.tap(char_keysym(ch));
                    }
                }
                // egui turns Ctrl/Cmd+C/X/V into these events; forward them as Ctrl combos.
                Event::Copy => ctrl_combo(self, 'c'),
                Event::Cut => ctrl_combo(self, 'x'),
                Event::Paste(text) => {
                    // Hand the text to the host first so its Ctrl+V pastes it.
                    self.send(VncCmd::ClientCut(text));
                    ctrl_combo(self, 'v');
                }
                _ => {}
            }
        }
        self.sync_mods(frame_mods);
    }

    /// Sends whole wheel notches as press + release of buttons 4–7.
    fn flush_wheel(&mut self) {
        let Some((x, y)) = self.last_pos else {
            self.wheel = Vec2::ZERO;
            return;
        };
        for (axis, neg_bit, pos_bit) in [(1usize, 16u8, 8u8), (0, 64, 32)] {
            let mut v = self.wheel[axis];
            let mut steps = 0;
            while v.abs() >= 1.0 && steps < 10 {
                // A positive egui delta scrolls up (button 4) or left (button 6).
                let bit = if v > 0.0 { pos_bit } else { neg_bit };
                self.send(VncCmd::Pointer {
                    x,
                    y,
                    buttons: self.buttons | bit,
                });
                self.send(VncCmd::Pointer {
                    x,
                    y,
                    buttons: self.buttons,
                });
                v -= v.signum();
                steps += 1;
            }
            self.wheel[axis] = if steps == 10 { 0.0 } else { v };
        }
    }
}

/// Keysym for a character typed as text (Latin-1 directly, Unicode via 0x0100_0000).
pub fn char_keysym(c: char) -> u32 {
    let cp = c as u32;
    match cp {
        0x20..=0x7e | 0xa0..=0xff => cp,
        0x0a | 0x0d => XK_RETURN,
        0x09 => XK_TAB,
        0x08 => 0xff08,
        0x1b => 0xff1b,
        _ => 0x0100_0000 + cp,
    }
}

/// X11 keysym for keys that do not produce text.
pub fn special_keysym(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        Enter => XK_RETURN,
        Backspace => 0xff08,
        Tab => XK_TAB,
        Escape => 0xff1b,
        Insert => 0xff63,
        Delete => XK_DELETE,
        Home => 0xff50,
        End => 0xff57,
        PageUp => 0xff55,
        PageDown => 0xff56,
        ArrowLeft => 0xff51,
        ArrowUp => 0xff52,
        ArrowRight => 0xff53,
        ArrowDown => 0xff54,
        F1 => 0xffbe,
        F2 => 0xffbf,
        F3 => 0xffc0,
        F4 => 0xffc1,
        F5 => 0xffc2,
        F6 => 0xffc3,
        F7 => 0xffc4,
        F8 => 0xffc5,
        F9 => 0xffc6,
        F10 => 0xffc7,
        F11 => 0xffc8,
        F12 => 0xffc9,
        _ => return None,
    })
}

/// Keysym for a text key pressed together with Ctrl / Alt (egui sends no Text then).
pub fn combo_keysym(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let c = match key {
        Space => ' ',
        Minus => '-',
        Plus => '+',
        Equals => '=',
        OpenBracket => '[',
        CloseBracket => ']',
        Semicolon => ';',
        Colon => ':',
        Quote => '\'',
        Backtick => '`',
        Backslash => '\\',
        Pipe => '|',
        Comma => ',',
        Period => '.',
        Slash => '/',
        Questionmark => '?',
        Exclamationmark => '!',
        _ => {
            let name = key.name();
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_alphanumeric() {
                return None;
            }
            c.to_ascii_lowercase()
        }
    };
    Some(char_keysym(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms() {
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym('ä'), 0xe4);
        assert_eq!(char_keysym('€'), 0x0100_20ac);
        assert_eq!(char_keysym('\n'), 0xff0d);
        assert_eq!(special_keysym(egui::Key::Enter), Some(0xff0d));
        assert_eq!(special_keysym(egui::Key::F12), Some(0xffc9));
        assert_eq!(special_keysym(egui::Key::A), None);
        assert_eq!(combo_keysym(egui::Key::C), Some(b'c' as u32));
        assert_eq!(combo_keysym(egui::Key::Num1), Some(b'1' as u32));
        assert_eq!(combo_keysym(egui::Key::Minus), Some(b'-' as u32));
        assert_eq!(combo_keysym(egui::Key::Space), Some(0x20));
    }
}
