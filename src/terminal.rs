//! Terminal widget: vt100 state machine + egui renderer + keyboard translation.

use std::time::Instant;

use egui::{Color32, CornerRadius, Event, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};

use crate::theme;
use crate::ui::blocks_core::{Blocks, join_command, join_output};

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
    /// Absolute line (stays put while the view scrolls).
    line: usize,
    col: u16,
}

/// Side channel of the escape stream: window title, OSC 7 working directory, bell.
#[derive(Default)]
pub struct Hooks {
    title: String,
    cwd: Option<String>,
    bell: bool,
    /// Answers to terminal queries (device attributes, cursor position …) for the session.
    replies: Vec<u8>,
    /// Command blocks (OSC 133 or the prompt heuristic).
    blocks: Blocks,
    scroll: Scroll,
    epoch: Option<Instant>,
}

/// Counts the lines that scrolled off the screen top, so marks can be stored with absolute
/// line numbers. vt100 does not tell, but while the view is scrolled back by one row it
/// moves with every scroll (even when the scrollback is full and drops its oldest line):
/// `Terminal::feed` parks the view at offset 1 and reads the count back from it.
#[derive(Default)]
struct Scroll {
    /// Lines scrolled off before the chunk being processed.
    total: usize,
    /// `Some(1)`: counted through the view offset; `Some(0)`: scrollback was empty, the
    /// count is its length; `None`: not counting (alternate screen).
    tracking: Option<usize>,
    /// Scrollback length of the main screen when the alternate screen was entered.
    main_len: Option<usize>,
}

impl Hooks {
    fn now(&mut self) -> f64 {
        self.epoch
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_secs_f64()
    }

    /// Lines scrolled off since the chunk began.
    fn scrolled_now(&self, screen: &mut vt100::Screen) -> usize {
        match self.scroll.tracking {
            Some(1) if !screen.alternate_screen() => screen.scrollback().saturating_sub(1),
            Some(0) if !screen.alternate_screen() => scrollback_len(screen),
            _ => 0,
        }
    }

    fn osc133(&mut self, screen: &mut vt100::Screen, kind: u8, rest: &[&[u8]]) {
        let (row, col) = screen.cursor_position();
        let top = self.scroll.total + self.scrolled_now(screen);
        let line = top + row as usize;
        let now = self.now();
        let arg = rest
            .first()
            .map(|a| String::from_utf8_lossy(a).into_owned());
        let cols = screen.size().1;
        self.blocks
            .osc_mark(kind, (line, col), arg.as_deref(), now, |cmd_pos| {
                let Some((bl, bc)) = cmd_pos else {
                    return String::new();
                };
                let last = if col == 0 && line > bl {
                    line - 1
                } else {
                    line
                };
                join_command(&read_lines(screen, top, bl, last, cols), bc)
            });
    }
}

/// Length of the scrollback of the shown screen.
fn scrollback_len(screen: &mut vt100::Screen) -> usize {
    let keep = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let n = screen.scrollback();
    screen.set_scrollback(keep);
    n
}

/// Text of the absolute lines `from..=to` as `(text, wrapped into the next row)`.
/// `top` is the absolute number of screen row 0. Lines that are gone are skipped.
fn read_lines(
    screen: &mut vt100::Screen,
    top: usize,
    from: usize,
    to: usize,
    cols: u16,
) -> Vec<(String, bool)> {
    let keep = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let len = screen.scrollback();
    let rows = screen.size().0 as usize;
    let oldest = top.saturating_sub(len);
    let mut out = Vec::new();
    let mut n = from.max(oldest);
    while n <= to && out.len() < 100_000 {
        let idx = n - oldest;
        let head = idx.min(len);
        screen.set_scrollback(len - head);
        let k = idx - head;
        if k >= rows {
            break;
        }
        let take = (rows - k).min(to - n + 1);
        let texts: Vec<String> = screen.rows(0, cols).skip(k).take(take).collect();
        for (i, text) in texts.into_iter().enumerate() {
            out.push((text, screen.row_wrapped((k + i) as u16)));
        }
        n += take;
    }
    screen.set_scrollback(keep);
    out
}

impl vt100::Callbacks for Hooks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        // Programs such as fish, vim or tmux ask the terminal about itself and wait for
        // the answer (fish for up to 10 seconds). Never let the buffer grow unbounded.
        if self.replies.len() > 4096 {
            return;
        }
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, i2, c) {
            // Primary device attributes: a VT220-class terminal with colour.
            (None, None, 'c') if first == 0 => self.replies.extend_from_slice(b"\x1b[?62;22c"),
            // Secondary device attributes.
            (Some(b'>'), None, 'c') if first == 0 => {
                self.replies.extend_from_slice(b"\x1b[>1;10;0c")
            }
            // Device status: "all fine".
            (None, None, 'n') if first == 5 => self.replies.extend_from_slice(b"\x1b[0n"),
            // Cursor position report.
            (None, None, 'n') if first == 6 => {
                let (row, col) = screen.cursor_position();
                self.replies
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // Terminal name and version (XTVERSION).
            (Some(b'>'), None, 'q') if first == 0 => self.replies.extend_from_slice(
                format!("\x1bP>|Aurora {}\x1b\\", env!("CARGO_PKG_VERSION")).as_bytes(),
            ),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, screen: &mut vt100::Screen, params: &[&[u8]]) {
        // OSC 133 ; A|B|C|D[;code]: shell integration marks (prompt, command, output, end).
        if let [b"133", kind, rest @ ..] = params
            && let Some(&k) = kind.first()
            && matches!(k, b'A' | b'B' | b'C' | b'D')
            && !screen.alternate_screen()
        {
            self.osc133(screen, k, rest);
        }
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

/// Find-in-terminal state (Ctrl+Shift+F).
#[derive(Default)]
struct Search {
    query: String,
    /// (absolute line, first char, length) – absolute line 0 is the oldest scrollback row.
    matches: Vec<(usize, usize, usize)>,
    current: usize,
    /// Scrollback length when the matches were computed.
    max: usize,
    dirty: bool,
    /// Scroll to the current match after the next search (query edited).
    jump: bool,
    focus: bool,
}

/// Buttons of the hover toolbar of a command block.
#[derive(Clone, Copy, PartialEq)]
enum ToolItem {
    Cmd,
    Output,
    Both,
    Rerun,
    Fold,
}

impl ToolItem {
    fn label(self, b: Option<&crate::ui::blocks_core::Block>) -> &'static str {
        match self {
            ToolItem::Cmd => t!("Befehl"),
            ToolItem::Output => t!("Ausgabe"),
            ToolItem::Both => t!("Beides"),
            ToolItem::Rerun => t!("Erneut"),
            ToolItem::Fold if b.is_some_and(|b| b.collapsed) => t!("Ausklappen"),
            ToolItem::Fold => t!("Einklappen"),
        }
    }

    fn tip(self) -> &'static str {
        match self {
            ToolItem::Cmd => t!("Befehl kopieren"),
            ToolItem::Output => t!("Ausgabe kopieren"),
            ToolItem::Both => t!("Befehl und Ausgabe kopieren"),
            ToolItem::Rerun => {
                t!("Befehl am Prompt eintippen (PROD: nur eintippen, Enter bleibt bei dir)")
            }
            ToolItem::Fold => t!("Ausgabe ein- oder ausklappen"),
        }
    }

    fn visible(self, b: Option<&crate::ui::blocks_core::Block>) -> bool {
        let Some(b) = b else { return false };
        match self {
            ToolItem::Cmd | ToolItem::Both | ToolItem::Rerun => !b.command.is_empty(),
            ToolItem::Output => b.ran(),
            ToolItem::Fold => b.out_end.zip(b.out_start).is_some_and(|(e, s)| e > s),
        }
    }
}

/// What a display row shows: a screen row, the placeholder of a folded block, or nothing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum RowSrc {
    Src(u16),
    Fold { id: u64, lines: usize, src: u16 },
    Blank,
}

pub struct Terminal {
    parser: vt100::Parser<Hooks>,
    /// Per-pane zoom (Ctrl+mouse wheel).
    pub font_scale: f32,
    zoom_hint_until: f64,
    search: Option<Search>,
    cols: u16,
    rows: u16,
    scrollback_rows: usize,
    selection: Option<(CellPos, CellPos)>,
    selecting: bool,
    last_feed: Instant,
    /// Block under the pointer in the previous frame (its toolbar stays while the pointer
    /// moves onto it).
    hover_block: Option<u64>,
    /// Everything fed so far (bounded): a resize replays it at the new width, because the
    /// grid itself does not reflow and would cut off what no longer fits.
    raw: Vec<u8>,
    raw_cut: bool,
    replaying: bool,
    scrollback: usize,
    /// A new size waits until the pane has stopped changing (drag of a divider).
    pending_size: Option<((u16, u16), Instant)>,
}

/// Replay buffer limit.
const RAW_MAX: usize = 8 << 20;

#[derive(Default)]
pub struct TermOutput {
    /// Bytes to send to the remote side.
    pub input: Vec<u8>,
    /// New grid size, if it changed.
    pub resize: Option<(u16, u16)>,
    /// "Save as snippet" was picked for this text.
    pub save_snippet: Option<String>,
    /// Text to paste (Ctrl+Shift+V, middle click, context menu). The caller checks it
    /// (`ui::paste_guard`) and encodes it with `paste_bytes`.
    pub paste: Option<String>,
    /// Ctrl+R at a shell prompt: open Aurora's command history for this host.
    pub history_search: bool,
    /// "Rerun" of a command block: type this command at the prompt.
    pub rerun: Option<String>,
    /// Short message for the user (copied …).
    pub notice: Option<String>,
    /// Ctrl+Shift+M: set a marker at the current time.
    pub marker: bool,
}

pub struct TermView<'a> {
    pub font_size: f32,
    /// Terminal font family (`theme::term_font`).
    pub family: egui::FontFamily,
    /// Colors of another theme (host terminal profile); `None` = current theme.
    pub palette: Option<theme::Palette>,
    pub accent: Color32,
    pub copy_on_select: bool,
    pub glow: Option<Color32>,
    pub overlay: Option<&'a str>,
    /// Ctrl+R at a prompt opens the history popup instead of going to the shell.
    pub ctrl_r: bool,
    /// Command blocks: gutter, hover toolbar, Ctrl+↑/↓ navigation.
    pub blocks: bool,
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
            font_scale: 1.0,
            zoom_hint_until: 0.0,
            search: None,
            last_feed: Instant::now(),
            hover_block: None,
            raw: Vec::new(),
            raw_cut: false,
            replaying: false,
            scrollback,
            pending_size: None,
        }
    }

    /// Resizes the grid. Plain screens replay the received bytes at the new size so text
    /// that was cut off by a narrower pane comes back (and wraps again); full-screen
    /// programs, or a history that was trimmed, just resize.
    fn reflow(&mut self, cols: u16, rows: u16) {
        if self.parser.screen().alternate_screen() || self.raw_cut || self.raw.is_empty() {
            self.parser.screen_mut().set_size(rows, cols);
        } else {
            let raw = std::mem::take(&mut self.raw);
            let epoch = self.parser.callbacks().epoch;
            let hooks = Hooks {
                epoch,
                ..Hooks::default()
            };
            self.parser = vt100::Parser::new_with_callbacks(rows, cols, self.scrollback, hooks);
            self.replaying = true;
            for chunk in raw.chunks(4096) {
                self.feed(chunk);
            }
            self.replaying = false;
            self.raw = raw;
            // Questions asked long ago must not be answered again.
            self.parser.callbacks_mut().replies.clear();
        }
        self.cols = cols;
        self.rows = rows;
        self.scrollback_rows = 0;
        self.selection = None;
        self.hover_block = None;
        if let Some(s) = self.search.as_mut() {
            s.dirty = true;
        }
    }

    /// A terminal of a fixed grid size (time travel replays into one of these).
    pub fn with_size(cols: u16, rows: u16, scrollback: usize) -> Self {
        let mut t = Self::new(scrollback);
        t.cols = cols.max(1);
        t.rows = rows.max(1);
        t.parser.screen_mut().set_size(t.rows, t.cols);
        t
    }

    /// Escape sequences that restore the screen, cursor and input modes.
    pub fn state_formatted(&self) -> Vec<u8> {
        self.parser.screen().state_formatted()
    }

    /// Text after the shell prompt on the cursor line, only trimmed at the end (a leading
    /// space matters: `HISTCONTROL=ignorespace`). `None` in full-screen programs or when the
    /// line has no prompt.
    pub fn prompt_command(&self) -> Option<String> {
        let screen = self.parser.screen();
        if screen.alternate_screen() {
            return None;
        }
        let (row, _) = screen.cursor_position();
        let line = screen.contents_between(row, 0, row, self.cols);
        let cut = ["$ ", "# ", "> ", "% ", "❯ "]
            .iter()
            .filter_map(|p| line.find(p).map(|i| i + p.len()))
            .min();
        match cut {
            Some(cut) => Some(line[cut..].trim_end().to_string()),
            // Empty prompt: the row's trailing space is not part of the contents.
            None => self.at_prompt().then(String::new),
        }
    }

    pub fn open_search(&mut self) {
        let s = self.search.get_or_insert_with(Search::default);
        s.focus = true;
        s.dirty = true;
    }

    /// Command typed after the prompt on the cursor line.
    pub fn current_command(&self) -> String {
        let screen = self.parser.screen();
        let (row, _) = screen.cursor_position();
        let line = screen.contents_between(row, 0, row, self.cols);
        let cut = ["$ ", "# ", "> ", "% ", "❯ "]
            .iter()
            .filter_map(|p| line.rfind(p).map(|i| i + p.len()))
            .max()
            .unwrap_or(0);
        line[cut..].trim().to_string()
    }

    /// Every line of scrollback + screen, oldest first; returns them and the scrollback length.
    fn all_lines(&mut self) -> (Vec<String>, usize) {
        let keep = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max = self.parser.screen().scrollback();
        let rows = self.rows as usize;
        let mut lines = vec![String::new(); max + rows];
        let mut off = max;
        loop {
            self.parser.screen_mut().set_scrollback(off);
            for (i, l) in self.parser.screen().rows(0, self.cols).enumerate() {
                if let Some(slot) = lines.get_mut(max - off + i) {
                    *slot = l;
                }
            }
            if off == 0 {
                break;
            }
            off = off.saturating_sub(rows.max(1));
        }
        self.parser.screen_mut().set_scrollback(keep);
        (lines, max)
    }

    fn run_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|s| s.query.to_lowercase()) else {
            return;
        };
        let needle: Vec<char> = query.chars().collect();
        let mut matches = Vec::new();
        let (lines, max) = if needle.is_empty() {
            (Vec::new(), 0)
        } else {
            self.all_lines()
        };
        for (n, line) in lines.iter().enumerate() {
            let hay: Vec<char> = line.to_lowercase().chars().collect();
            let mut i = 0;
            while i + needle.len() <= hay.len() {
                if hay[i..i + needle.len()] == needle[..] {
                    matches.push((n, i, needle.len()));
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
        if let Some(s) = self.search.as_mut() {
            let keep_last =
                s.current == usize::MAX || s.matches.is_empty() || s.current + 1 >= s.matches.len();
            s.matches = matches;
            s.max = max;
            s.dirty = false;
            if keep_last || s.current >= s.matches.len() {
                // Start at the newest match.
                s.current = s.matches.len().saturating_sub(1);
            }
        }
    }

    /// Scrolls so that the current match is visible.
    fn reveal_match(&mut self) {
        let Some(s) = &self.search else { return };
        let Some(&(line, _, _)) = s.matches.get(s.current) else {
            return;
        };
        let rows = self.rows as usize;
        let max = s.max;
        let top = line.saturating_sub(rows / 2);
        let off = max.saturating_sub(top).min(max);
        self.parser.screen_mut().set_scrollback(off);
        self.scrollback_rows = self.parser.screen().scrollback();
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// After the connection died: leave the alternate screen, show the cursor and switch the
    /// mouse, bracketed paste and application cursor modes off.
    pub fn reset_modes(&mut self) {
        if self.parser.screen().alternate_screen() {
            self.feed(b"\x1b[?1049l");
        }
        self.feed(
            b"\x1b[?1l\x1b>\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[0m",
        );
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if !self.replaying {
            self.raw.extend_from_slice(bytes);
            if self.raw.len() > RAW_MAX {
                let cut = self.raw.len() - RAW_MAX;
                self.raw.drain(..cut);
                self.raw_cut = true;
            }
        }
        self.last_feed = Instant::now();
        let alt0 = self.parser.screen().alternate_screen();
        let user_off = self.parser.screen().scrollback();
        let len0 = if alt0 {
            0
        } else {
            scrollback_len(self.parser.screen_mut())
        };
        let mode = (!alt0).then_some(usize::from(len0 >= 1));
        self.parser.callbacks_mut().scroll.tracking = mode;
        if mode == Some(1) {
            self.parser.screen_mut().set_scrollback(1);
        }
        self.parser.process(bytes);
        let alt1 = self.parser.screen().alternate_screen();
        let h = self.parser.callbacks_mut();
        h.scroll.tracking = None;
        if alt1 && !alt0 {
            h.scroll.main_len = Some(len0);
        }
        if !alt1 {
            let screen = self.parser.screen_mut();
            let len1 = scrollback_len(screen);
            let off = screen.scrollback();
            let h = self.parser.callbacks_mut();
            let k = match mode {
                // The view offset moved along with every scroll.
                Some(1) if off >= 1 => off - 1,
                // Offset reset by a visit to the alternate screen: best effort.
                Some(_) => len1.saturating_sub(len0),
                // Back from the alternate screen: the main scrollback grew meanwhile.
                None => len1.saturating_sub(h.scroll.main_len.take().unwrap_or(len1)),
            };
            h.scroll.total += k;
            let total = h.scroll.total;
            h.blocks.trim(total.saturating_sub(len1));
            let keep = if user_off == 0 {
                0
            } else {
                (user_off + k).min(len1)
            };
            self.parser.screen_mut().set_scrollback(keep);
            self.scrollback_rows = keep;
        }
        if let Some(s) = self.search.as_mut() {
            s.dirty = true;
        }
    }

    // ---- command blocks

    pub fn blocks(&self) -> &Blocks {
        &self.parser.callbacks().blocks
    }

    pub fn blocks_mut(&mut self) -> &mut Blocks {
        &mut self.parser.callbacks_mut().blocks
    }

    /// Absolute number of screen row 0 of the live screen.
    fn live_top(&self) -> usize {
        self.parser.callbacks().scroll.total
    }

    /// Absolute number of the first row shown (the view may be scrolled back).
    fn view_top(&self) -> usize {
        self.live_top().saturating_sub(self.scrollback_rows)
    }

    /// Line after the cursor line (end of a command that is still running).
    fn live_end(&self) -> usize {
        self.live_top() + self.parser.screen().cursor_position().0 as usize + 1
    }

    /// The shell sits at its prompt (no program runs, no full-screen mode).
    pub fn at_shell_prompt(&self) -> bool {
        !self.parser.screen().alternate_screen()
            && if self.blocks().osc {
                self.blocks().at_prompt()
            } else {
                self.at_prompt()
            }
    }

    /// Text of the absolute lines `from..to` (end exclusive).
    fn lines_text(&mut self, from: usize, to: usize) -> Vec<(String, bool)> {
        if to <= from {
            return Vec::new();
        }
        let top = self.live_top();
        let cols = self.cols;
        read_lines(self.parser.screen_mut(), top, from, to - 1, cols)
    }

    /// Command and output text of a block.
    pub fn block_text(&mut self, id: u64) -> Option<(String, String)> {
        let live_end = self.live_end();
        let b = self.blocks().get(id)?.clone();
        let output = match b.output_lines(live_end) {
            Some((s, e)) => join_output(&self.lines_text(s, e)),
            None => String::new(),
        };
        Some((b.command, output))
    }

    /// Scrolls so that absolute line `line` is at the top of the view.
    pub fn scroll_to_line(&mut self, line: usize) {
        let total = self.live_top();
        let want = total.saturating_sub(line);
        self.parser.screen_mut().set_scrollback(want);
        self.scrollback_rows = self.parser.screen().scrollback();
    }

    /// Back to the live screen.
    pub fn scroll_to_end(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
        self.scrollback_rows = 0;
    }

    /// Jumps to the previous (`-1`) or next (`1`) block start. Returns whether it moved.
    pub fn jump_block(&mut self, dir: i32) -> bool {
        let top = self.view_top();
        if dir < 0 {
            match self.blocks().prev_start(top.max(1)) {
                Some(l) => {
                    self.scroll_to_line(l);
                    true
                }
                None => false,
            }
        } else if let Some(l) = self.blocks().next_start(top) {
            self.scroll_to_line(l);
            true
        } else if self.scrollback_rows > 0 {
            self.scroll_to_end();
            true
        } else {
            false
        }
    }

    /// Jumps to the newest block that failed.
    pub fn jump_last_error(&mut self) -> bool {
        match self.blocks().last_failed().map(|b| b.prompt) {
            Some(l) => {
                self.scroll_to_line(l);
                true
            }
            None => false,
        }
    }

    /// Heuristic blocks: call once per frame. Notices the prompt when the output has settled.
    pub fn tick_blocks(&mut self) {
        if self.blocks().osc
            || self.parser.screen().alternate_screen()
            || self.last_feed.elapsed().as_millis() < 250
            || !self.at_prompt()
        {
            return;
        }
        let line = self.live_top() + self.parser.screen().cursor_position().0 as usize;
        let now = self.parser.callbacks_mut().now();
        self.blocks_mut().heur_prompt(line, now);
    }

    /// Heuristic blocks: Enter is about to run `command` (or the line at the prompt).
    pub fn note_enter(&mut self, command: Option<&str>) {
        if self.blocks().osc || self.parser.screen().alternate_screen() {
            return;
        }
        let cmd = match command {
            Some(c) => c.to_string(),
            None => match self.prompt_command() {
                Some(c) => c,
                None => return,
            },
        };
        let line = self.live_top() + self.parser.screen().cursor_position().0 as usize;
        let now = self.parser.callbacks_mut().now();
        self.blocks_mut().heur_prompt(line, now);
        self.blocks_mut().heur_enter(line, line + 1, &cmd, now);
    }

    /// New complete lines since `*mark` (absolute numbers), for text indexing of recordings.
    /// The cursor line is left out until the cursor has moved on.
    pub fn take_new_lines(&mut self, mark: &mut usize) -> Vec<(String, bool)> {
        if self.parser.screen().alternate_screen() {
            return Vec::new();
        }
        let top = self.live_top();
        let end = top + self.parser.screen().cursor_position().0 as usize;
        let from = (*mark).max(top.saturating_sub(scrollback_len(self.parser.screen_mut())));
        *mark = end.max(*mark);
        self.lines_text(from, end)
    }

    /// All remaining lines up to and including the cursor line (end of a recording).
    pub fn take_last_lines(&mut self, mark: &mut usize) -> Vec<(String, bool)> {
        let end = self.live_end();
        let from = (*mark).max(
            self.live_top()
                .saturating_sub(scrollback_len(self.parser.screen_mut())),
        );
        *mark = end;
        self.lines_text(from, end)
    }

    /// Remote working directory, if the shell reports it (OSC 7 or the window title).
    /// May start with `~`.
    pub fn cwd(&self) -> Option<String> {
        let h = self.parser.callbacks();
        h.cwd.clone().or_else(|| dir_from_title(&h.title))
    }

    /// Answers the terminal owes the session (to device-attribute and similar queries).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().replies)
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

    /// Escape sequences that redraw the current screen (start of a recording).
    pub fn snapshot(&self) -> Vec<u8> {
        self.parser.screen().contents_formatted()
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

    fn selected_text(&mut self) -> Option<String> {
        let (a, b) = self.selection?;
        let (s, e) = if (a.line, a.col) <= (b.line, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        let lines = self.lines_text(s.line, e.line + 1);
        let n = lines.len();
        let mut text = String::new();
        for (i, (line, wrapped)) in lines.into_iter().enumerate() {
            let mut part: String = line.chars().collect();
            if i + 1 == n {
                part = part.chars().take(e.col as usize + 1).collect();
            }
            if i == 0 {
                part = part.chars().skip(s.col as usize).collect();
            }
            if i + 1 < n && !wrapped {
                part.truncate(part.trim_end().len());
                text.push_str(&part);
                text.push('\n');
            } else {
                text.push_str(&part);
            }
        }
        (!text.trim().is_empty()).then_some(text)
    }

    /// Whether the cell of absolute `line` is inside the selection.
    fn in_selection(&self, line: usize, col: u16) -> bool {
        let Some((a, b)) = self.selection else {
            return false;
        };
        let (s, e) = if (a.line, a.col) <= (b.line, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        (line, col) >= (s.line, s.col) && (line, col) <= (e.line, e.col)
    }

    /// Floating find bar in the top-right corner. Returns its rect while open.
    fn search_bar(&mut self, ui: &mut egui::Ui, full: Rect, term: &egui::Response) -> Option<Rect> {
        let s = self.search.as_mut()?;
        let w = 330f32.min(full.width() - 20.0);
        let bar = Rect::from_min_size(
            full.right_top() + Vec2::new(-w - 10.0, 8.0),
            Vec2::new(w, 34.0),
        );
        ui.painter().rect(
            bar,
            CornerRadius::same(8),
            theme::bg_card(),
            Stroke::new(1.0, theme::border()),
            egui::StrokeKind::Inside,
        );
        let mut step: i32 = 0;
        let mut close = false;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(Vec2::new(8.0, 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let count = if s.matches.is_empty() {
            if s.query.is_empty() {
                String::new()
            } else {
                "0".into()
            }
        } else {
            format!("{}/{}", s.current + 1, s.matches.len())
        };
        let edit = child.add(
            egui::TextEdit::singleline(&mut s.query)
                .hint_text(t!("Im Terminal suchen"))
                .frame(egui::Frame::NONE)
                .desired_width(w - 160.0),
        );
        if s.focus {
            edit.request_focus();
            s.focus = false;
        }
        if edit.changed() {
            s.dirty = true;
            s.jump = true;
            s.current = usize::MAX;
        }
        let (enter, shift, esc) = child.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.modifiers.shift,
                i.key_pressed(Key::Escape),
            )
        });
        // Search runs upwards from the newest output, like reading back in a terminal.
        if edit.lost_focus() && enter {
            step = if shift { 1 } else { -1 };
            edit.request_focus();
        }
        if esc && (edit.has_focus() || edit.lost_focus()) {
            close = true;
        }
        child.label(
            egui::RichText::new(count)
                .font(FontId::monospace(11.5))
                .color(theme::text_dim()),
        );
        if child
            .small_button("↑")
            .on_hover_text(t!("Älterer Treffer (Enter)"))
            .clicked()
        {
            step = -1;
        }
        if child
            .small_button("↓")
            .on_hover_text(t!("Neuerer Treffer (Shift+Enter)"))
            .clicked()
        {
            step = 1;
        }
        if child
            .small_button("×")
            .on_hover_text(t!("Schließen (Esc)"))
            .clicked()
        {
            close = true;
        }
        if close {
            self.search = None;
            term.request_focus();
            return Some(bar);
        }
        if s.dirty {
            let jump = std::mem::take(&mut s.jump);
            self.run_search();
            if jump {
                self.reveal_match();
            }
        }
        if step != 0
            && let Some(s) = self.search.as_mut()
            && !s.matches.is_empty()
        {
            let n = s.matches.len() as i32;
            s.current = ((s.current as i32 + step).rem_euclid(n)) as usize;
            self.reveal_match();
        }
        Some(bar)
    }

    /// Display rows: the screen rows, with the output of folded blocks replaced by one
    /// placeholder row each. At the live screen the content stays anchored at the bottom.
    fn row_map(&self, enabled: bool) -> Vec<RowSrc> {
        let rows = self.rows;
        let plain = || (0..rows).map(RowSrc::Src).collect::<Vec<_>>();
        if !enabled || self.parser.screen().alternate_screen() {
            return plain();
        }
        let folds: Vec<(u64, usize, usize)> = self
            .blocks()
            .list
            .iter()
            .filter(|b| b.collapsed && b.out_end.is_some())
            .filter_map(|b| b.output_lines(usize::MAX).map(|(s, e)| (b.id, s, e)))
            .filter(|(_, s, e)| e > s)
            .collect();
        if folds.is_empty() {
            return plain();
        }
        let top = self.view_top();
        let mut out = Vec::with_capacity(rows as usize);
        let mut last = None;
        for r in 0..rows {
            let abs = top + r as usize;
            match folds.iter().find(|(_, s, e)| abs >= *s && abs < *e) {
                Some(&(id, s, e)) => {
                    if last != Some(id) {
                        out.push(RowSrc::Fold {
                            id,
                            lines: e - s,
                            src: r,
                        });
                        last = Some(id);
                    }
                }
                None => {
                    last = None;
                    out.push(RowSrc::Src(r));
                }
            }
        }
        let pad = rows as usize - out.len();
        if pad > 0 {
            if self.scrollback_rows == 0 {
                out.splice(0..0, std::iter::repeat_n(RowSrc::Blank, pad));
            } else {
                out.extend(std::iter::repeat_n(RowSrc::Blank, pad));
            }
        }
        out
    }

    /// Gutter marks, hover highlight and the mini toolbar of the command blocks.
    #[allow(clippy::too_many_arguments)]
    fn blocks_overlay(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        full: Rect,
        inner: Rect,
        ch: f32,
        map: &[RowSrc],
        accent: Color32,
        out: &mut TermOutput,
    ) {
        if self.parser.screen().alternate_screen() {
            self.hover_block = None;
            return;
        }
        let top = self.view_top();
        let live_end = self.live_end();
        let btns = self.toolbar_items();
        let hover_prev = self.hover_block;
        let blocks_ref = &self.parser.callbacks().blocks;
        let abs_of = |m: &RowSrc| match m {
            RowSrc::Src(r) => Some(top + *r as usize),
            RowSrc::Fold { src, .. } => Some(top + *src as usize),
            RowSrc::Blank => None,
        };
        let row_block: Vec<Option<u64>> = map
            .iter()
            .map(|m| {
                abs_of(m)
                    .and_then(|a| blocks_ref.at_line(a, live_end))
                    .map(|b| b.id)
            })
            .collect();
        let row_rect = |d: usize| {
            Rect::from_min_size(
                Pos2::new(full.left() + 2.0, inner.top() + d as f32 * ch),
                Vec2::new(full.width() - 4.0, ch),
            )
        };
        // The toolbar sits at the right edge of the first shown row of the block.
        let first_row = |id: u64| row_block.iter().position(|b| *b == Some(id));
        let tb_width = |ui: &egui::Ui, id: u64| -> f32 {
            let b = blocks_ref.get(id);
            let mut w = 12.0;
            let g = |s: &str| {
                ui.painter()
                    .layout_no_wrap(s.to_string(), FontId::proportional(11.5), theme::text())
                    .size()
                    .x
            };
            if let Some(d) = b.and_then(|b| b.duration()) {
                w += g(&t!("Dauer {}", crate::ui::blocks_core::duration_label(d))) + 10.0;
            }
            for item in &btns {
                if item.visible(b) {
                    w += g(item.label(b)) + 14.0;
                }
            }
            w
        };
        let toolbar_rect = |ui: &egui::Ui, id: u64| -> Option<Rect> {
            let d = first_row(id)?;
            let w = tb_width(ui, id).min(full.width() - 40.0);
            Some(Rect::from_min_size(
                Pos2::new(full.right() - 14.0 - w, inner.top() + d as f32 * ch - 1.0),
                Vec2::new(w, ch + 2.0),
            ))
        };
        let ptr = ui.input(|i| i.pointer.hover_pos());
        let mut hovered = None;
        if let Some(p) = ptr.filter(|p| full.contains(*p)) {
            let d = ((p.y - inner.top()) / ch).floor();
            if d >= 0.0 {
                hovered = row_block.get(d as usize).copied().flatten();
            }
            if let Some(prev) = hover_prev
                && toolbar_rect(ui, prev).is_some_and(|r| r.expand(2.0).contains(p))
            {
                hovered = Some(prev);
            }
        }
        // Gutter marks (and a faint tint over the hovered block).
        let painter = ui.painter_at(full);
        for (d, id) in row_block.iter().enumerate() {
            let Some(id) = id else { continue };
            let Some(b) = blocks_ref.get(*id) else {
                continue;
            };
            let color = if b.running() {
                accent
            } else if b.failed() {
                theme::red()
            } else if b.exit == Some(0) {
                theme::green()
            } else {
                theme::text_dim().gamma_multiply(0.6)
            };
            let r = row_rect(d);
            let hot = hovered == Some(*id);
            if hot {
                painter.rect_filled(r, CornerRadius::ZERO, accent.gamma_multiply(0.07));
            }
            let bar = Rect::from_min_size(
                Pos2::new(full.left() + 8.0, r.top() + 1.0),
                Vec2::new(if hot { 4.0 } else { 3.0 }, ch - 2.0),
            );
            painter.rect_filled(bar, CornerRadius::same(2), color);
        }

        let rect = hovered.and_then(|id| toolbar_rect(ui, id));
        self.hover_block = hovered;

        // A click on the placeholder of a folded block unfolds it.
        if resp.clicked()
            && let Some(p) = ptr
        {
            let d = ((p.y - inner.top()) / ch).floor();
            if d >= 0.0
                && let Some(RowSrc::Fold { id, .. }) = map.get(d as usize)
                && let Some(b) = self.blocks_mut().get_mut(*id)
            {
                b.collapsed = false;
            }
        }

        let (Some(id), Some(rect)) = (hovered, rect) else {
            return;
        };
        let Some(block) = self.blocks().get(id).cloned() else {
            return;
        };
        painter.rect(
            rect,
            CornerRadius::same(6),
            theme::bg_card(),
            Stroke::new(1.0, theme::border()),
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + 8.0;
        let mid = rect.center().y;
        if let Some(dur) = block.duration() {
            let g = painter.layout_no_wrap(
                t!("Dauer {}", crate::ui::blocks_core::duration_label(dur)),
                FontId::proportional(11.5),
                theme::text_dim(),
            );
            painter.galley(
                Pos2::new(x, mid - g.size().y / 2.0),
                g.clone(),
                theme::text_dim(),
            );
            x += g.size().x + 10.0;
        }
        let mut action = None;
        for (i, item) in btns.iter().enumerate() {
            if !item.visible(Some(&block)) {
                continue;
            }
            let label = item.label(Some(&block));
            let g = painter.layout_no_wrap(
                label.to_string(),
                FontId::proportional(11.5),
                theme::text(),
            );
            let r = Rect::from_min_size(
                Pos2::new(x - 4.0, rect.top() + 2.0),
                Vec2::new(g.size().x + 10.0, rect.height() - 4.0),
            );
            let hit = ui.interact(r, egui::Id::new(("block-btn", id, i)), Sense::click());
            if hit.hovered() {
                painter.rect_filled(r, CornerRadius::same(4), theme::bg_hover());
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            painter.galley(
                Pos2::new(x + 1.0, mid - g.size().y / 2.0),
                g.clone(),
                theme::text(),
            );
            x += g.size().x + 14.0;
            let hit = hit.on_hover_text(item.tip());
            if hit.clicked() {
                action = Some(*item);
            }
        }
        let Some(action) = action else { return };
        match action {
            ToolItem::Rerun => {
                out.rerun = crate::ui::blocks_core::rerunnable(&block.command);
                if out.rerun.is_none() {
                    out.notice = Some(t!("Dieser Befehl lässt sich nicht erneut eintippen").into());
                }
            }
            ToolItem::Fold => {
                if let Some(b) = self.blocks_mut().get_mut(id) {
                    b.collapsed = !b.collapsed;
                }
            }
            ToolItem::Cmd | ToolItem::Output | ToolItem::Both => {
                let (cmd, output) = self.block_text(id).unwrap_or_default();
                let (text, note) = match action {
                    ToolItem::Cmd => (cmd, t!("Befehl kopiert")),
                    ToolItem::Output => (output, t!("Ausgabe kopiert")),
                    _ => (format!("{cmd}\n{output}"), t!("Befehl und Ausgabe kopiert")),
                };
                if text.trim().is_empty() {
                    out.notice = Some(t!("Nichts zu kopieren").into());
                } else {
                    ui.ctx().copy_text(text);
                    out.notice = Some(note.into());
                }
            }
        }
    }

    /// Buttons of the block toolbar (`Rerun` only while the shell is at its prompt).
    fn toolbar_items(&self) -> Vec<ToolItem> {
        let mut v = vec![ToolItem::Cmd, ToolItem::Output, ToolItem::Both];
        if self.at_shell_prompt() {
            v.push(ToolItem::Rerun);
        }
        v.push(ToolItem::Fold);
        v
    }

    pub fn show(&mut self, ui: &mut egui::Ui, id: egui::Id, view: TermView<'_>) -> TermOutput {
        let mut out = TermOutput::default();
        let pal = view.palette.unwrap_or_else(theme::palette);
        let extras = theme::term_extras(&pal);
        let (term_bg, term_fg) = (pal.term_bg, pal.term_fg);
        let font = FontId::new(view.font_size * self.font_scale, view.family.clone());
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

        // Slim gutter for the command block marks.
        let gutter = if view.blocks { 8.0 } else { 0.0 };
        let inner = Rect::from_min_max(
            full.min + Vec2::new(12.0 + gutter, 10.0),
            full.max - Vec2::new(12.0, 10.0),
        );
        let want = (
            ((inner.width() / cw).floor() as u16).max(10),
            ((inner.height() / ch).floor() as u16).max(3),
        );
        if want != (self.cols, self.rows) {
            let now = Instant::now();
            let settled = match self.pending_size {
                Some((size, since)) if size == want => now.duration_since(since).as_millis() >= 120,
                _ => {
                    self.pending_size = Some((want, now));
                    false
                }
            };
            // Without history to replay there is nothing to lose: resize at once.
            if settled || self.raw.is_empty() || self.parser.screen().alternate_screen() {
                self.pending_size = None;
                self.reflow(want.0, want.1);
                out.resize = Some(want);
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(60));
            }
        } else {
            self.pending_size = None;
        }
        let (cols, rows) = (self.cols, self.rows);

        let map0 = self.row_map(view.blocks);
        let first_src = map0
            .iter()
            .find_map(|m| {
                if let RowSrc::Src(r) = m {
                    Some(*r)
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let to_cell = |p: Pos2| {
            let d = (((p.y - inner.top()) / ch).floor().max(0.0) as usize)
                .min(map0.len().saturating_sub(1));
            let row = match map0.get(d) {
                Some(RowSrc::Src(r)) => *r,
                Some(RowSrc::Fold { src, .. }) => *src,
                _ => first_src,
            };
            CellPos {
                line: row.min(rows - 1) as usize,
                col: (((p.x - inner.left()) / cw).floor().max(0.0) as u16).min(cols - 1),
            }
        };

        // --- mouse: focus, selection, scrollback, context menu
        if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        if resp.clicked() {
            self.selection = None;
        }
        // Start the selection where the button went down, not where the drag was detected.
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
        {
            let mut c = to_cell(p);
            c.line += self.view_top();
            self.selection = Some((c, c));
            self.selecting = true;
        }
        if self.selecting
            && resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Dragging above or below the text scrolls on, so a selection can span more
            // than one screen (the farther out, the faster).
            let over = if p.y < inner.top() {
                inner.top() - p.y
            } else {
                (p.y - inner.bottom()).max(0.0)
            };
            if over > 0.0 {
                let n = (1.0 + over / ch).min(6.0) as usize;
                let off = self.scrollback_rows;
                let target = if p.y < inner.top() {
                    off + n
                } else {
                    off.saturating_sub(n)
                };
                self.parser.screen_mut().set_scrollback(target);
                self.scrollback_rows = self.parser.screen().scrollback();
                ui.ctx().request_repaint();
            }
            let mut c = to_cell(p);
            c.line += self.view_top();
            if let Some(sel) = self.selection.as_mut() {
                sel.1 = c;
            }
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
            // Ctrl + wheel zooms this pane only, 5 % per wheel event.
            let notches: i32 = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        Event::MouseWheel {
                            delta, modifiers, ..
                        } if modifiers.command && delta.y != 0.0 => {
                            Some(if delta.y > 0.0 { 1 } else { -1 })
                        }
                        _ => None,
                    })
                    .sum()
            });
            let zoom = if ui.input(|i| i.modifiers.command) {
                0.0
            } else {
                1.0
            };
            if notches != 0 {
                self.font_scale = (self.font_scale * 1.05f32.powi(notches)).clamp(0.6, 3.0);
                self.zoom_hint_until = ui.input(|i| i.time) + 1.2;
            }
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if zoom == 1.0 && scroll.abs() > 0.0 {
                let lines = (scroll / ch).round() as isize;
                let target = (self.scrollback_rows as isize + lines).max(0) as usize;
                self.parser.screen_mut().set_scrollback(target);
                self.scrollback_rows = self.parser.screen().scrollback();
            }
        }
        if resp.middle_clicked()
            && let Some(text) = clipboard_text()
        {
            resp.request_focus();
            out.paste = Some(text);
        }
        let selected = self.selected_text();
        let mut open_search = false;
        let mut copy_last = false;
        let mut jump_error = false;
        let has_failed = view.blocks && self.blocks().last_failed().is_some();
        let has_done = view.blocks && self.blocks().last_done().is_some();
        resp.context_menu(|ui| {
            if ui
                .add_enabled(selected.is_some(), egui::Button::new(t!("📋 Kopieren")))
                .clicked()
            {
                if let Some(t) = &selected {
                    ui.ctx().copy_text(t.clone());
                }
                ui.close();
            }
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new(t!("⚡ Als Snippet speichern")),
                )
                .clicked()
            {
                out.save_snippet = selected.clone();
                ui.close();
            }
            if ui.button(t!("📋 Einfügen")).clicked() {
                out.paste = clipboard_text();
                ui.close();
            }
            if ui.button(t!("🔍 Suchen  (Ctrl+Shift+F)")).clicked() {
                open_search = true;
                ui.close();
            }
            if ui.button(t!("📥 Alles kopieren")).clicked() {
                ui.ctx().copy_text(self.contents());
                ui.close();
            }
            if view.blocks {
                ui.separator();
                if ui
                    .add_enabled(
                        has_done,
                        egui::Button::new(t!("Letzte Ausgabe kopieren  (Ctrl+Shift+O)")),
                    )
                    .clicked()
                {
                    copy_last = true;
                    ui.close();
                }
                if ui
                    .add_enabled(
                        has_failed,
                        egui::Button::new(t!("Zum letzten Fehler springen")),
                    )
                    .clicked()
                {
                    jump_error = true;
                    ui.close();
                }
            }
            if ui.button(t!("Marker setzen  (Ctrl+Shift+M)")).clicked() {
                out.marker = true;
                ui.close();
            }
            if ui.button(t!("🗑 Bildschirm leeren")).clicked() {
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
            let shell_idle =
                view.blocks && self.at_shell_prompt() && self.blocks().list.iter().any(|b| b.ran());
            let mut nav = 0;
            for ev in events {
                match ev {
                    Event::Text(t) => out.input.extend_from_slice(t.as_bytes()),
                    Event::Ime(egui::ImeEvent::Commit(t)) => {
                        out.input.extend_from_slice(t.as_bytes())
                    }
                    Event::Paste(t) => out.paste.get_or_insert_with(String::new).push_str(&t),
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
                        key: Key::F,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command && modifiers.shift => open_search = true,
                    Event::Key {
                        key: Key::ArrowUp,
                        pressed: true,
                        modifiers,
                        ..
                    } if shell_idle && modifiers.ctrl && !modifiers.shift && !modifiers.alt => {
                        nav = -1
                    }
                    Event::Key {
                        key: Key::ArrowDown,
                        pressed: true,
                        modifiers,
                        ..
                    } if shell_idle && modifiers.ctrl && !modifiers.shift && !modifiers.alt => {
                        nav = 1
                    }
                    Event::Key {
                        key: Key::O,
                        pressed: true,
                        modifiers,
                        ..
                    } if view.blocks && modifiers.command && modifiers.shift && !modifiers.alt => {
                        copy_last = true
                    }
                    Event::Key {
                        key: Key::M,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command && modifiers.shift && !modifiers.alt => {
                        out.marker = true
                    }
                    Event::Key {
                        key: Key::R,
                        pressed: true,
                        modifiers,
                        ..
                    } if view.ctrl_r
                        && modifiers.ctrl
                        && !modifiers.shift
                        && !modifiers.alt
                        && self.prompt_command().is_some() =>
                    {
                        out.history_search = true
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
            if (!out.input.is_empty() || out.paste.is_some()) && self.scrollback_rows > 0 {
                self.parser.screen_mut().set_scrollback(0);
                self.scrollback_rows = 0;
            }
            if nav != 0 {
                self.jump_block(nav);
            }
        }
        if view.blocks {
            self.tick_blocks();
            if !self.blocks().osc && !self.parser.screen().alternate_screen() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
        if jump_error && !self.jump_last_error() {
            out.notice = Some(t!("Kein fehlgeschlagener Befehl bekannt").into());
        }
        if copy_last {
            let id = self.blocks().last_done().map(|b| b.id);
            match id.and_then(|id| self.block_text(id)) {
                Some((_, output)) if !output.is_empty() => {
                    ui.ctx().copy_text(output);
                    out.notice = Some(t!("Ausgabe des letzten Befehls kopiert").into());
                }
                _ => out.notice = Some(t!("Keine Ausgabe zum Kopieren").into()),
            }
        }

        if open_search {
            self.open_search();
        }
        let search_rect = self.search_bar(ui, full, &resp);
        if self.search.as_ref().is_some_and(|s| s.dirty) {
            self.run_search();
        }

        // --- paint grid
        let map = self.row_map(view.blocks);
        let view_top = self.view_top();
        let screen = self.parser.screen();
        let (cur_row, cur_col) = screen.cursor_position();
        for (d, src) in map.iter().enumerate() {
            let y = inner.top() + d as f32 * ch;
            let row = match src {
                RowSrc::Src(r) => *r,
                RowSrc::Fold { lines, .. } => {
                    let r = Rect::from_min_size(
                        Pos2::new(inner.left(), y + 1.0),
                        Vec2::new(inner.width(), ch - 2.0),
                    );
                    painter.rect_filled(
                        r,
                        CornerRadius::same(4),
                        theme::lerp(term_bg, term_fg, 0.10),
                    );
                    painter.text(
                        Pos2::new(inner.left() + 8.0, y + ch / 2.0),
                        egui::Align2::LEFT_CENTER,
                        t!("+ {} Zeilen eingeklappt", lines),
                        FontId::proportional((font.size * 0.85).max(10.0)),
                        term_fg.gamma_multiply(0.75),
                    );
                    continue;
                }
                RowSrc::Blank => continue,
            };
            let mut col = 0u16;
            while col < cols {
                let Some(cell) = screen.cell(row, col) else {
                    break;
                };
                let start = col;
                let style = CellStyle::of(cell);
                let mut text = String::new();
                let sel = self.in_selection(view_top + row as usize, col);
                // Extend the run while the style stays the same.
                while col < cols {
                    let Some(c) = screen.cell(row, col) else {
                        break;
                    };
                    if c.is_wide_continuation() {
                        col += 1;
                        continue;
                    }
                    if CellStyle::of(c) != style
                        || self.in_selection(view_top + row as usize, col) != sel
                    {
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
                    match extras.1 {
                        // The theme's own selection color; keep the text readable on it.
                        Some(c) => {
                            bg = c;
                            if crate::themeimport_convert::contrast(fg, c) < 3.0 {
                                fg = crate::themeimport_convert::ensure(fg, &[c], 4.5);
                            }
                        }
                        None => bg = view.accent.linear_multiply(0.35),
                    }
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
        let cur_d = map.iter().position(|m| *m == RowSrc::Src(cur_row));
        if self.scrollback_rows == 0
            && !screen.hide_cursor()
            && cur_row < rows
            && cur_col < cols
            && let Some(cur_d) = cur_d
        {
            let r = Rect::from_min_size(
                Pos2::new(
                    inner.left() + cur_col as f32 * cw,
                    inner.top() + cur_d as f32 * ch,
                ),
                Vec2::new(cw, ch),
            );
            if focused {
                painter.rect_filled(
                    r,
                    CornerRadius::same(2),
                    extras
                        .0
                        .unwrap_or_else(|| view.accent.linear_multiply(0.75)),
                );
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
                    Stroke::new(1.0, extras.0.unwrap_or(view.accent)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        // --- search hits
        if let Some(s) = &self.search {
            let top = s.max.saturating_sub(self.scrollback_rows);
            for (i, &(line, start, len)) in s.matches.iter().enumerate() {
                if line < top || line >= top + rows as usize {
                    continue;
                }
                let Some(d) = map
                    .iter()
                    .position(|m| *m == RowSrc::Src((line - top) as u16))
                else {
                    continue;
                };
                let r = Rect::from_min_size(
                    Pos2::new(
                        inner.left() + start as f32 * cw,
                        inner.top() + d as f32 * ch,
                    ),
                    Vec2::new(len as f32 * cw, ch),
                );
                if i == s.current {
                    painter.rect(
                        r,
                        CornerRadius::same(2),
                        view.accent.gamma_multiply(0.45),
                        Stroke::new(1.5, view.accent),
                        egui::StrokeKind::Outside,
                    );
                } else {
                    painter.rect_filled(r, CornerRadius::same(2), view.accent.gamma_multiply(0.25));
                }
            }
        }

        if view.blocks {
            self.blocks_overlay(ui, &resp, full, inner, ch, &map, view.accent, &mut out);
        }

        if ui.input(|i| i.time) < self.zoom_hint_until {
            let label = format!("{:.0} %", self.font_scale * 100.0);
            let g = painter.layout_no_wrap(label, FontId::proportional(13.0), theme::text());
            let bg = Rect::from_center_size(full.center(), g.size() + Vec2::new(24.0, 12.0));
            painter.rect_filled(bg, CornerRadius::same(255), theme::bg_hover());
            painter.galley(bg.min + Vec2::new(12.0, 6.0), g, theme::text());
            ui.ctx().request_repaint();
        }

        if self.scrollback_rows > 0 && search_rect.is_none() {
            let label = t!("⬆ {} Zeilen zurück", self.scrollback_rows);
            let pos = full.right_bottom() + Vec2::new(-14.0, -34.0);
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

/// Text on the system clipboard (middle click and the context menu's "Einfügen").
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new()
        .ok()?
        .get_text()
        .ok()
        .filter(|t| !t.is_empty())
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
        assert_eq!(t.prompt_command().as_deref(), Some(""));
        t.feed(b"\x1b]7;file://vm/home/a%20b\x07");
        assert_eq!(t.cwd().as_deref(), Some("/home/a b"));
        t.feed(b"make\r\nbuilding...");
        assert!(!t.at_prompt());
        assert_eq!(t.prompt_command(), None);
        t.feed(b"\r\n$  export X=1");
        assert_eq!(t.prompt_command().as_deref(), Some(" export X=1"));
        t.feed(b"\x1b[?1049h");
        assert_eq!(t.prompt_command(), None);
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

#[cfg(test)]
mod query_tests {
    #[test]
    fn narrowing_and_widening_keeps_the_text() {
        let mut t = Terminal::with_size(60, 5, 100);
        let line = "0123456789".repeat(5);
        t.feed(format!("{line}\r\nsecond\r\n").as_bytes());
        t.reflow(20, 5);
        t.reflow(60, 5);
        let all = t.contents();
        assert!(all.contains(&line), "{all}");
        assert!(all.contains("second"));
    }

    use super::*;

    fn replies(input: &[u8]) -> Vec<u8> {
        let mut t = Terminal::new(100);
        t.feed(input);
        t.take_replies()
    }

    #[test]
    fn answers_device_queries() {
        assert_eq!(replies(b"\x1b[c"), b"\x1b[?62;22c");
        assert_eq!(replies(b"\x1b[0c"), b"\x1b[?62;22c");
        assert_eq!(replies(b"\x1b[>c"), b"\x1b[>1;10;0c");
        assert_eq!(replies(b"\x1b[5n"), b"\x1b[0n");
        assert_eq!(replies(b"ab\x1b[6n"), b"\x1b[1;3R");
        assert!(replies(b"\x1b[>0q").starts_with(b"\x1bP>|Aurora "));
        assert!(replies(b"plain text \x1b[1m").is_empty());
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;

    /// One prompt + command + output cycle of a shell with OSC 133 integration.
    fn cycle(cmd: &str, out: &str, code: i32) -> Vec<u8> {
        format!(
            "\x1b]133;A\x07u@h:~$ \x1b]133;B\x07{cmd}\r\n\x1b]133;C\x07{out}\x1b]133;D;{code}\x07"
        )
        .into_bytes()
    }

    #[test]
    fn osc_133_blocks_and_output() {
        let mut t = Terminal::with_size(40, 6, 100);
        t.feed(&cycle("echo hi", "hi\r\n", 0));
        t.feed(&cycle("false", "", 1));
        t.feed(b"\x1b]133;A\x07u@h:~$ ");
        let blocks = t.blocks();
        assert!(blocks.osc);
        // Two commands and the prompt that waits for the next one.
        assert_eq!(blocks.list.len(), 3);
        let (a, b) = (blocks.list[0].clone(), blocks.list[1].clone());
        assert_eq!(a.command, "echo hi");
        assert_eq!(a.exit, Some(0));
        assert_eq!(a.output_lines(99), Some((1, 2)));
        assert_eq!(b.command, "false");
        assert_eq!(b.exit, Some(1));
        assert_eq!(b.output_lines(99), Some((3, 3)));
        assert_eq!(t.block_text(a.id), Some(("echo hi".into(), "hi".into())));
        assert!(t.at_shell_prompt());
        assert_eq!(t.blocks().last_failed().map(|b| b.id), Some(b.id));
    }

    #[test]
    fn blocks_survive_scrolling_and_trimming() {
        // 6 rows, 20 lines of scrollback: far more output than fits.
        let mut t = Terminal::with_size(40, 6, 20);
        for i in 0..40 {
            t.feed(&cycle(
                &format!("cmd{i}"),
                &format!("out{i}\r\nmore{i}\r\n"),
                i % 3,
            ));
        }
        t.feed(b"\x1b]133;A\x07u@h:~$ ");
        // The oldest blocks are gone, the newest are intact with the right text.
        let list = &t.blocks().list;
        assert!(list.len() < 40 && list.len() >= 5, "{}", list.len());
        let last = list.iter().rev().nth(1).unwrap().clone();
        assert_eq!(last.command, "cmd39");
        let (cmd, out) = t.block_text(last.id).unwrap();
        assert_eq!((cmd.as_str(), out.as_str()), ("cmd39", "out39\nmore39"));
        // Absolute numbers: 40 blocks of 3 lines each, the prompt is on line 120.
        assert_eq!(last.prompt, 39 * 3);
        assert_eq!(
            t.live_top() + t.parser.screen().cursor_position().0 as usize,
            120
        );
        // Every kept block reads back its own output.
        let ids: Vec<u64> = t.blocks().list.iter().map(|b| b.id).collect();
        for id in ids {
            let b = t.blocks().get(id).unwrap().clone();
            if b.truncated || !b.ran() {
                continue;
            }
            let n: usize = b.command.trim_start_matches("cmd").parse().unwrap();
            let (_, out) = t.block_text(id).unwrap();
            assert_eq!(out, format!("out{n}\nmore{n}"), "{}", b.command);
        }
    }

    #[test]
    fn keeps_counting_while_scrolled_back_and_in_alternate_screen() {
        let mut t = Terminal::with_size(30, 4, 10);
        for i in 0..30 {
            t.feed(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(t.live_top(), 27);
        // The user looks back; new output keeps the view where it is.
        t.parser.screen_mut().set_scrollback(5);
        t.scrollback_rows = 5;
        t.feed(b"x\r\ny\r\n");
        assert_eq!(t.live_top(), 29);
        assert_eq!(t.scrollback_rows, 7);
        // A full-screen program: nothing is lost afterwards.
        t.scroll_to_end();
        t.feed(b"\x1b[?1049h\x1b[2Jfull screen\r\n\r\n\r\n\r\n\r\n\x1b[?1049l");
        assert_eq!(t.live_top(), 29);
        t.feed(b"a\r\nb\r\n");
        assert_eq!(t.live_top(), 31);
    }

    #[test]
    fn prompt_heuristic_without_marks() {
        let mut t = Terminal::with_size(40, 6, 100);
        t.feed(b"u@h:~$ ");
        t.last_feed = Instant::now() - std::time::Duration::from_secs(1);
        t.tick_blocks();
        assert_eq!(t.blocks().list.len(), 1);
        t.feed(b"ls -l");
        t.note_enter(None);
        t.feed(b"\r\ntotal 0\r\nfile\r\nu@h:~$ ");
        t.last_feed = Instant::now() - std::time::Duration::from_secs(1);
        t.tick_blocks();
        let list = &t.blocks().list;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].command, "ls -l");
        assert!(list[0].heuristic);
        assert_eq!(list[0].output_lines(99), Some((1, 3)));
        let id = list[0].id;
        assert_eq!(t.block_text(id).unwrap().1, "total 0\nfile");
        // While output is still flowing a prompt-like line end does not end the block.
        t.feed(b"sleep");
        t.note_enter(None);
        t.feed(b"\r\nProgress: 50%");
        t.tick_blocks();
        assert!(t.blocks().running().is_some());
    }
}
