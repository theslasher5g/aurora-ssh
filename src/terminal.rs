//! Terminal widget: vt100 state machine + egui renderer + keyboard translation.

use egui::{Color32, CornerRadius, Event, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};

use crate::theme;

fn idx_color(i: u8, ansi: &[Color32; 16]) -> Color32 {
    match i {
        0..=15 => ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
    }
}

fn resolve(c: vt100::Color, default: Color32, bold: bool, ansi: &[Color32; 16]) -> Color32 {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) if bold && i < 8 => ansi[i as usize + 8],
        vt100::Color::Idx(i) => idx_color(i, ansi),
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CellPos {
    row: u16,
    col: u16,
}

/// Side channel of the escape stream: window title, OSC 7 working directory, bell.
#[derive(Default)]
pub struct Hooks {
    title: String,
    cwd: Option<String>,
    bell: bool,
}

impl vt100::Callbacks for Hooks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // OSC 7 ; file://host/path
        if let [b"7", url, ..] = params {
            let url = String::from_utf8_lossy(url);
            if let Some(rest) = url.strip_prefix("file://") {
                let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("/");
                self.cwd = Some(percent_decode(path));
            }
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Working directory from a title like `user@host: ~/src` (the Debian/Ubuntu default).
fn dir_from_title(title: &str) -> Option<String> {
    let (_, dir) = title.rsplit_once(": ")?;
    let dir = dir.trim();
    (dir.starts_with('/') || dir.starts_with('~')).then(|| dir.to_string())
}

pub struct Terminal {
    parser: vt100::Parser<Hooks>,
    cols: u16,
    rows: u16,
    scrollback_rows: usize,
    selection: Option<(CellPos, CellPos)>,
    selecting: bool,
}

#[derive(Default)]
pub struct TermOutput {
    /// Bytes to send to the remote side.
    pub input: Vec<u8>,
    /// New grid size, if it changed.
    pub resize: Option<(u16, u16)>,
}

pub struct TermView<'a> {
    pub font_size: f32,
    pub accent: Color32,
    pub copy_on_select: bool,
    pub glow: Option<Color32>,
    pub overlay: Option<&'a str>,
}

impl Terminal {
    pub fn new(scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(24, 80, scrollback, Hooks::default()),
            cols: 80,
            rows: 24,
            scrollback_rows: 0,
            selection: None,
            selecting: false,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Remote working directory, if the shell reports it (OSC 7 or the window title).
    /// May start with `~`.
    pub fn cwd(&self) -> Option<String> {
        let h = self.parser.callbacks();
        h.cwd.clone().or_else(|| dir_from_title(&h.title))
    }

    /// True once after the remote rang the bell.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.parser.callbacks_mut().bell)
    }

    /// The cursor sits after something that looks like a shell prompt (`$ `, `# `, `> `…).
    pub fn at_prompt(&self) -> bool {
        let screen = self.parser.screen();
        if screen.alternate_screen() {
            return false;
        }
        let (row, col) = screen.cursor_position();
        let line = screen.contents_between(row, 0, row, col);
        line.trim_end()
            .chars()
            .last()
            .is_some_and(|c| matches!(c, '$' | '#' | '>' | '%' | '❯' | '➜' | 'λ' | ':'))
    }

    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// Encodes pasted text, honoring bracketed paste mode.
    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.bracketed_paste() {
            // Pasted text must not be able to end bracketed paste early and have the rest
            // executed as typed input (the "paste-jacking" trick).
            let text = text.replace("\x1b[201~", "").replace("\x1b[200~", "");
            let mut v = b"\x1b[200~".to_vec();
            v.extend_from_slice(text.as_bytes());
            v.extend_from_slice(b"\x1b[201~");
            v
        } else {
            text.into_bytes()
        }
    }

    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (s, e) = if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        let text =
            self.parser
                .screen()
                .contents_between(s.row, s.col, e.row, e.col.saturating_add(1));
        (!text.is_empty()).then_some(text)
    }

    fn in_selection(&self, row: u16, col: u16) -> bool {
        let Some((a, b)) = self.selection else {
            return false;
        };
        let (s, e) = if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        (row, col) >= (s.row, s.col) && (row, col) <= (e.row, e.col)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, id: egui::Id, view: TermView<'_>) -> TermOutput {
        let mut out = TermOutput::default();
        let pal = theme::palette();
        let (term_bg, term_fg) = (pal.term_bg, pal.term_fg);
        let font = FontId::monospace(view.font_size);
        let (cw, ch) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let cw = cw.max(1.0);
        let ch = ch.ceil().max(1.0);

        let full = ui.available_rect_before_wrap();
        let resp = ui.interact(full, id, Sense::click_and_drag());
        ui.allocate_rect(full, Sense::hover());
        let painter = ui.painter_at(full);
        painter.rect_filled(full, CornerRadius::same(10), term_bg);
        if let Some(glow) = view.glow {
            painter.rect_stroke(
                full.shrink(1.0),
                CornerRadius::same(10),
                Stroke::new(2.0, glow),
                egui::StrokeKind::Inside,
            );
        }

        let inner = full.shrink2(Vec2::new(12.0, 10.0));
        let cols = ((inner.width() / cw).floor() as u16).max(10);
        let rows = ((inner.height() / ch).floor() as u16).max(3);
        if (cols, rows) != (self.cols, self.rows) {
            self.cols = cols;
            self.rows = rows;
            self.parser.screen_mut().set_size(rows, cols);
            self.selection = None;
            out.resize = Some((cols, rows));
        }

        let to_cell = |p: Pos2| CellPos {
            row: (((p.y - inner.top()) / ch).floor().max(0.0) as u16).min(rows - 1),
            col: (((p.x - inner.left()) / cw).floor().max(0.0) as u16).min(cols - 1),
        };

        // --- mouse: focus, selection, scrollback, context menu
        if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        if resp.clicked() {
            self.selection = None;
        }
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let c = to_cell(p);
            self.selection = Some((c, c));
            self.selecting = true;
        }
        if self.selecting
            && resp.dragged()
            && let (Some(p), Some(sel)) = (resp.interact_pointer_pos(), self.selection.as_mut())
        {
            sel.1 = to_cell(p);
        }
        if self.selecting && resp.drag_stopped() {
            self.selecting = false;
            if view.copy_on_select
                && let Some(t) = self.selected_text()
            {
                ui.ctx().copy_text(t);
            }
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let lines = (scroll / ch).round() as isize;
                let target = (self.scrollback_rows as isize + lines).max(0) as usize;
                self.parser.screen_mut().set_scrollback(target);
                self.scrollback_rows = self.parser.screen().scrollback();
            }
        }
        let selected = self.selected_text();
        resp.context_menu(|ui| {
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("📋 Kopieren"))
                .clicked()
            {
                if let Some(t) = &selected {
                    ui.ctx().copy_text(t.clone());
                }
                ui.close();
            }
            if ui.button("📥 Alles kopieren").clicked() {
                ui.ctx().copy_text(self.contents());
                ui.close();
            }
            if ui.button("🗑 Bildschirm leeren").clicked() {
                out.input.extend_from_slice(b"\x0c");
                ui.close();
            }
        });

        // --- keyboard
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
            let app_cursor = self.parser.screen().application_cursor();
            for ev in events {
                match ev {
                    Event::Text(t) => out.input.extend_from_slice(t.as_bytes()),
                    Event::Ime(egui::ImeEvent::Commit(t)) => {
                        out.input.extend_from_slice(t.as_bytes())
                    }
                    Event::Paste(t) => out.input.extend(self.paste_bytes(&t)),
                    Event::Copy => {
                        if cfg!(target_os = "macos") || mods.shift {
                            if let Some(t) = self.selected_text() {
                                ui.ctx().copy_text(t);
                            }
                        } else {
                            out.input.push(0x03);
                        }
                    }
                    Event::Cut => {
                        if !cfg!(target_os = "macos") {
                            out.input.push(0x18);
                        }
                    }
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if let Some(bytes) = key_bytes(key, modifiers, app_cursor) {
                            out.input.extend(bytes);
                        }
                    }
                    _ => {}
                }
            }
            if !out.input.is_empty() && self.scrollback_rows > 0 {
                self.parser.screen_mut().set_scrollback(0);
                self.scrollback_rows = 0;
            }
        }

        // --- paint grid
        let screen = self.parser.screen();
        let (cur_row, cur_col) = screen.cursor_position();
        for row in 0..rows {
            let y = inner.top() + row as f32 * ch;
            let mut col = 0u16;
            while col < cols {
                let Some(cell) = screen.cell(row, col) else {
                    break;
                };
                let start = col;
                let style = CellStyle::of(cell);
                let mut text = String::new();
                let sel = self.in_selection(row, col);
                // Extend the run while the style stays the same.
                while col < cols {
                    let Some(c) = screen.cell(row, col) else {
                        break;
                    };
                    if c.is_wide_continuation() {
                        col += 1;
                        continue;
                    }
                    if CellStyle::of(c) != style || self.in_selection(row, col) != sel {
                        break;
                    }
                    if c.has_contents() {
                        text.push_str(c.contents());
                    } else {
                        text.push(' ');
                    }
                    col += 1;
                    if c.is_wide() {
                        col += 1;
                        break;
                    }
                }
                if text.is_empty() {
                    continue;
                }
                let x = inner.left() + start as f32 * cw;
                let run_rect =
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new((col - start) as f32 * cw, ch));
                let (mut fg, mut bg) = (
                    resolve(style.fg, term_fg, style.bold, &pal.ansi),
                    resolve(style.bg, term_bg, false, &pal.ansi),
                );
                if style.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if style.dim {
                    fg = fg.linear_multiply(0.6);
                }
                if sel {
                    bg = view.accent.linear_multiply(0.35);
                }
                if bg != term_bg {
                    painter.rect_filled(run_rect, CornerRadius::ZERO, bg);
                }
                if text.trim().is_empty() {
                    continue;
                }
                painter.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    &text,
                    font.clone(),
                    fg,
                );
                if style.underline {
                    painter.line_segment(
                        [
                            Pos2::new(run_rect.left(), run_rect.bottom() - 1.5),
                            Pos2::new(run_rect.right(), run_rect.bottom() - 1.5),
                        ],
                        Stroke::new(1.0, fg),
                    );
                }
            }
        }

        // --- cursor
        if self.scrollback_rows == 0 && !screen.hide_cursor() && cur_row < rows && cur_col < cols {
            let r = Rect::from_min_size(
                Pos2::new(
                    inner.left() + cur_col as f32 * cw,
                    inner.top() + cur_row as f32 * ch,
                ),
                Vec2::new(cw, ch),
            );
            if focused {
                painter.rect_filled(r, CornerRadius::same(2), view.accent.linear_multiply(0.75));
                if let Some(c) = screen.cell(cur_row, cur_col).filter(|c| c.has_contents()) {
                    painter.text(
                        r.min,
                        egui::Align2::LEFT_TOP,
                        c.contents(),
                        font.clone(),
                        term_bg,
                    );
                }
            } else {
                painter.rect_stroke(
                    r,
                    CornerRadius::same(2),
                    Stroke::new(1.0, view.accent),
                    egui::StrokeKind::Inside,
                );
            }
        }

        if self.scrollback_rows > 0 {
            let label = format!("⬆ {} Zeilen zurück", self.scrollback_rows);
            let pos = full.right_top() + Vec2::new(-14.0, 10.0);
            let g = painter.layout_no_wrap(label, FontId::proportional(11.0), theme::text());
            let bg = Rect::from_min_size(
                pos - Vec2::new(g.size().x + 16.0, 0.0),
                g.size() + Vec2::new(16.0, 8.0),
            );
            painter.rect_filled(bg, CornerRadius::same(255), theme::bg_hover());
            painter.galley(bg.min + Vec2::new(8.0, 4.0), g, theme::text());
        }

        if let Some(msg) = view.overlay {
            let veil = Color32::from_rgba_unmultiplied(10, 12, 18, 200);
            painter.rect_filled(full, CornerRadius::same(10), veil);
            painter.text(
                full.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                FontId::proportional(16.0),
                theme::text(),
            );
        }

        out
    }
}

#[cfg(test)]
mod hook_tests {
    use super::Terminal;

    #[test]
    fn cwd_and_prompt() {
        let mut t = Terminal::new(100);
        t.feed(b"\x1b]0;aurora@vm: ~/src/app\x07aurora@vm:~/src/app$ ");
        assert_eq!(t.cwd().as_deref(), Some("~/src/app"));
        assert!(t.at_prompt());
        t.feed(b"\x1b]7;file://vm/home/a%20b\x07");
        assert_eq!(t.cwd().as_deref(), Some("/home/a b"));
        t.feed(b"make\r\nbuilding...");
        assert!(!t.at_prompt());
        t.feed(b"\x07");
        assert!(t.take_bell());
        assert!(!t.take_bell());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    dim: bool,
    underline: bool,
    inverse: bool,
}

impl CellStyle {
    fn of(c: &vt100::Cell) -> Self {
        Self {
            fg: c.fgcolor(),
            bg: c.bgcolor(),
            bold: c.bold(),
            dim: c.dim(),
            underline: c.underline(),
            inverse: c.inverse(),
        }
    }
}

/// Translates a key press into the byte sequence an xterm would send.
pub fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let csi_or_ss3 = |c: u8| -> Vec<u8> {
        let mods = 1 + (m.shift as u8) + 2 * (m.alt as u8) + 4 * (m.ctrl as u8);
        if mods > 1 {
            format!("\x1b[1;{mods}{}", c as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    let tilde = |n: u8| format!("\x1b[{n}~").into_bytes();
    let bytes = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace if m.ctrl || m.alt => b"\x17".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => csi_or_ss3(b'A'),
        Key::ArrowDown => csi_or_ss3(b'B'),
        Key::ArrowRight => csi_or_ss3(b'C'),
        Key::ArrowLeft => csi_or_ss3(b'D'),
        Key::Home => csi_or_ss3(b'H'),
        Key::End => csi_or_ss3(b'F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ if m.ctrl && !m.alt => {
            let name = key.name();
            let c = name.chars().next()?;
            if name.len() == 1 && c.is_ascii_alphabetic() {
                vec![(c.to_ascii_uppercase() as u8) - b'A' + 1]
            } else {
                match key {
                    Key::Space => vec![0],
                    Key::OpenBracket => vec![0x1b],
                    Key::Backslash => vec![0x1c],
                    Key::CloseBracket => vec![0x1d],
                    _ => return None,
                }
            }
        }
        _ => return None,
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_keys() {
        assert_eq!(key_bytes(Key::C, Modifiers::CTRL, false), Some(vec![3]));
        assert_eq!(key_bytes(Key::D, Modifiers::CTRL, false), Some(vec![4]));
        assert_eq!(
            key_bytes(Key::ArrowUp, Modifiers::NONE, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowRight, Modifiers::CTRL, false),
            Some(b"\x1b[1;5C".to_vec())
        );
        assert_eq!(key_bytes(Key::A, Modifiers::NONE, false), None);
    }

    #[test]
    fn bracketed_paste() {
        let mut t = Terminal::new(100);
        assert_eq!(t.paste_bytes("a\nb"), b"a\rb");
        t.feed(b"\x1b[?2004h");
        assert_eq!(t.paste_bytes("x"), b"\x1b[200~x\x1b[201~");
        assert_eq!(
            t.paste_bytes("a\x1b[201~rm -rf ~\n"),
            b"\x1b[200~arm -rf ~\r\x1b[201~"
        );
    }

    #[test]
    fn color_cube() {
        let ansi = theme::THEMES[0].ansi;
        assert_eq!(idx_color(16, &ansi), Color32::from_rgb(0, 0, 0));
        assert_eq!(idx_color(231, &ansi), Color32::from_rgb(255, 255, 255));
        assert_eq!(idx_color(232, &ansi), Color32::from_rgb(8, 8, 8));
    }
}
