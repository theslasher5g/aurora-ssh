//! "Zeitreise": every terminal keeps its recent output (last 30 minutes, at most 16 MB)
//! with timestamps. The rewind overlay replays it into a scratch terminal up to the time
//! picked on a timeline. Checkpoints (the screen state as escape sequences) every 128 KB
//! keep scrubbing fast: a position never needs more than one segment replayed.

use std::collections::VecDeque;
use std::time::Instant;

use egui::{CornerRadius, Pos2, Rect, RichText, Stroke, Vec2};

use crate::terminal::{TermOutput, TermView, Terminal};
use crate::theme::{self, Btn};
use crate::ui::icons::Icon;

pub const MAX_BYTES: usize = 16 << 20;
pub const MAX_AGE: f64 = 30.0 * 60.0;
const SEGMENT: usize = 128 << 10;
/// Output arriving closer together than this is stored as one chunk.
const MERGE: f64 = 0.01;

/// Output since a checkpoint.
struct Segment {
    start: f64,
    cols: u16,
    rows: u16,
    /// Screen state at `start` (`Screen::state_formatted`).
    checkpoint: Vec<u8>,
    chunks: Vec<(f64, Vec<u8>)>,
    bytes: usize,
}

/// Output history of one terminal.
pub struct Rewind {
    epoch: Instant,
    /// Wall clock at `epoch`; `None` for a recording (times from its start).
    wall: Option<chrono::DateTime<chrono::Local>>,
    segments: VecDeque<Segment>,
    total: usize,
    max_bytes: usize,
    max_age: f64,
}

impl Rewind {
    pub fn live() -> Self {
        Self::with_limits(MAX_BYTES, MAX_AGE)
    }

    pub fn with_limits(max_bytes: usize, max_age: f64) -> Self {
        Self {
            epoch: Instant::now(),
            wall: Some(chrono::Local::now()),
            segments: VecDeque::new(),
            total: 0,
            max_bytes,
            max_age,
        }
    }

    /// A whole `.cast` recording (no limits), for scrubbing in replay tabs.
    pub fn from_cast(cols: u16, rows: u16, frames: &[crate::backend::record::Frame]) -> Self {
        let mut rw = Self::with_limits(usize::MAX, f64::INFINITY);
        rw.wall = None;
        let mut scratch = Terminal::with_size(cols, rows, 0);
        for f in frames {
            rw.push_at(
                f.time,
                (cols, rows),
                || scratch.state_formatted(),
                f.data.as_bytes(),
            );
            scratch.feed(f.data.as_bytes());
        }
        rw
    }

    /// Position on this buffer's timeline of a moment.
    pub fn time_at(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.epoch).as_secs_f64()
    }

    /// Seconds since this buffer was created.
    pub fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Stores output that is about to be fed to `term` (call before `feed`).
    pub fn push(&mut self, term: &Terminal, data: &[u8]) {
        let t = self.now();
        self.push_at(t, term.size(), || term.state_formatted(), data);
    }

    /// Stores output at time `t`. `checkpoint` gives the screen state before `data`; it is
    /// only asked for when a new segment starts (size change or segment full).
    pub fn push_at(
        &mut self,
        t: f64,
        size: (u16, u16),
        checkpoint: impl FnOnce() -> Vec<u8>,
        data: &[u8],
    ) {
        if data.is_empty() {
            return;
        }
        let fresh = self
            .segments
            .back()
            .is_none_or(|s| s.bytes >= SEGMENT || (s.cols, s.rows) != size);
        if fresh {
            let cp = checkpoint();
            self.total += cp.len();
            self.segments.push_back(Segment {
                start: t,
                cols: size.0,
                rows: size.1,
                checkpoint: cp,
                chunks: Vec::new(),
                bytes: 0,
            });
        }
        let Some(seg) = self.segments.back_mut() else {
            return;
        };
        match seg.chunks.last_mut() {
            Some(last) if t - last.0 < MERGE => last.1.extend_from_slice(data),
            _ => seg.chunks.push((t, data.to_vec())),
        }
        seg.bytes += data.len();
        self.total += data.len();
        self.evict(t);
    }

    /// Drops whole segments from the front: over the size limit, or older than the limit.
    fn evict(&mut self, now: f64) {
        while self.segments.len() > 1
            && (self.total > self.max_bytes || self.segments[1].start < now - self.max_age)
        {
            if let Some(s) = self.segments.pop_front() {
                self.total -= s.bytes + s.checkpoint.len();
            }
        }
    }

    /// Bytes held (output plus checkpoints).
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.total
    }

    /// First and last moment that can be shown.
    pub fn span(&self) -> Option<(f64, f64)> {
        let first = self.segments.front()?;
        let last = self.segments.back()?;
        let end = last.chunks.last().map(|c| c.0).unwrap_or(last.start);
        Some((first.start, end))
    }

    fn segment_at(&self, t: f64) -> Option<usize> {
        if self.segments.is_empty() {
            return None;
        }
        let i = self.segments.partition_point(|s| s.start <= t);
        Some(i.saturating_sub(1))
    }

    /// The screen as it was at `t`.
    #[cfg(test)]
    pub fn render_at(&self, t: f64) -> Option<Terminal> {
        let seg = &self.segments[self.segment_at(t)?];
        let mut term = Terminal::with_size(seg.cols, seg.rows, 1000);
        term.feed(&seg.checkpoint);
        for (_, data) in seg.chunks.iter().take_while(|c| c.0 <= t) {
            term.feed(data);
        }
        Some(term)
    }

    /// Time of the first output after `t`.
    fn next_output(&self, t: f64) -> Option<f64> {
        self.segments
            .iter()
            .flat_map(|s| s.chunks.iter())
            .map(|c| c.0)
            .find(|&c| c > t)
    }

    /// Output volume over the span in `n` buckets, scaled to 0..=1.
    fn activity(&self, n: usize) -> Vec<f32> {
        let mut out = vec![0f32; n.max(1)];
        let Some((a, b)) = self.span() else {
            return out;
        };
        let len = (b - a).max(1e-6);
        for s in &self.segments {
            for (t, d) in &s.chunks {
                let i = (((t - a) / len) * n as f64) as usize;
                out[i.min(n - 1)] += d.len() as f32;
            }
        }
        let max = out.iter().cloned().fold(1.0, f32::max);
        for v in &mut out {
            *v = (*v / max).sqrt();
        }
        out
    }

    /// Clock time of `t` (`14:03:22`), or the position in a recording (`2:31`).
    pub fn label(&self, t: f64) -> String {
        match self.wall {
            Some(w) => (w + chrono::Duration::milliseconds((t * 1000.0) as i64))
                .format("%H:%M:%S")
                .to_string(),
            None => clock(t),
        }
    }
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// State of the open rewind overlay of one tab.
pub struct RewindView {
    /// Shown moment.
    pub t: f64,
    pub playing: bool,
    /// The whole recording of a replay tab (instead of the live buffer).
    pub own: Option<Rewind>,
    scratch: Option<Terminal>,
    /// Segment the scratch terminal was built from, and the chunks fed so far.
    seg_start: f64,
    fed: usize,
    shown: f64,
    focus: bool,
}

impl RewindView {
    /// Opens at the newest moment of `rw` (or of `own`).
    pub fn new(rw: Option<&Rewind>, own: Option<Rewind>) -> Self {
        let end = own
            .as_ref()
            .or(rw)
            .and_then(Rewind::span)
            .map(|s| s.1)
            .unwrap_or(0.0);
        Self {
            t: end,
            playing: false,
            own,
            scratch: None,
            seg_start: f64::NAN,
            fed: 0,
            shown: f64::NAN,
            focus: true,
        }
    }

    /// Brings the scratch terminal to `self.t`: feeds forward within the segment while
    /// playing, rebuilds from the checkpoint otherwise.
    fn sync(&mut self, rw: &Rewind) {
        let Some(i) = rw.segment_at(self.t) else {
            self.scratch = None;
            return;
        };
        let seg = &rw.segments[i];
        let forward = self.scratch.is_some() && seg.start == self.seg_start && self.t >= self.shown;
        if !forward {
            let mut term = Terminal::with_size(seg.cols, seg.rows, 1000);
            term.feed(&seg.checkpoint);
            self.scratch = Some(term);
            self.seg_start = seg.start;
            self.fed = 0;
        }
        if let Some(term) = self.scratch.as_mut() {
            while let Some((t, data)) = seg.chunks.get(self.fed) {
                if *t > self.t {
                    break;
                }
                term.feed(data);
                self.fed += 1;
            }
        }
        self.shown = self.t;
    }
}

/// Result of a frame of the overlay.
pub struct Shown {
    /// Typing and pasting in the past screen: goes to the live session.
    pub out: TermOutput,
    /// Back to the live terminal.
    pub live: bool,
}

/// Draws the past screen with the timeline bar over the terminal area.
pub fn show(
    ui: &mut egui::Ui,
    id: egui::Id,
    live_rw: Option<&Rewind>,
    v: &mut RewindView,
    view: TermView<'_>,
    marks: &[(f64, String)],
) -> Shown {
    let own = v.own.take();
    let shown = show_inner(ui, id, own.as_ref().or(live_rw), v, view, marks);
    v.own = own;
    shown
}

fn show_inner(
    ui: &mut egui::Ui,
    id: egui::Id,
    rw: Option<&Rewind>,
    v: &mut RewindView,
    view: TermView<'_>,
    marks: &[(f64, String)],
) -> Shown {
    let full = ui.available_rect_before_wrap();
    let mut live = false;
    let span = rw.and_then(Rewind::span);
    let (Some(rw), Some((a, b))) = (rw, span) else {
        ui.allocate_rect(full, egui::Sense::hover());
        ui.painter().text(
            full.center(),
            egui::Align2::CENTER_CENTER,
            t!("Noch keine Ausgabe aufgezeichnet"),
            egui::FontId::proportional(15.0),
            theme::text_dim(),
        );
        let bar =
            Rect::from_center_size(full.center() + Vec2::new(0.0, 40.0), Vec2::new(120.0, 34.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar));
        if theme::btn(&mut child, Btn::Primary, None, "Live").clicked() {
            live = true;
        }
        return Shown {
            out: TermOutput::default(),
            live,
        };
    };

    // Playback.
    if v.playing {
        let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
        let mut t = v.t + dt;
        // Idle stretches are skipped.
        if let Some(next) = rw.next_output(v.t)
            && next > t + 1.5
        {
            t = next;
        }
        if t >= b {
            t = b;
            v.playing = false;
        }
        v.t = t;
        ui.ctx().request_repaint();
    }
    v.t = v.t.clamp(a, b);
    if v.shown != v.t || v.scratch.is_none() {
        v.sync(rw);
    }

    let term_id = id.with("past");
    if std::mem::take(&mut v.focus) {
        ui.memory_mut(|m| m.request_focus(term_id));
    }
    let accent = view.accent;
    let out = match v.scratch.as_mut() {
        Some(term) => term.show(ui, term_id, view),
        None => TermOutput::default(),
    };
    if !out.input.is_empty() || out.paste.is_some() {
        live = true;
    }

    // Amber frame and a badge: this is not the live screen.
    let p = ui.painter();
    p.rect_stroke(
        full.shrink(1.0),
        CornerRadius::same(10),
        Stroke::new(2.0, theme::amber()),
        egui::StrokeKind::Inside,
    );
    let badge = format!("{} · {}", t!("ZEITREISE"), rw.label(v.t));
    let g = p.layout_no_wrap(badge, theme::bold(11.0), theme::amber());
    let br = Rect::from_min_size(
        full.left_top() + Vec2::new(12.0, 10.0),
        g.size() + Vec2::new(16.0, 8.0),
    );
    p.rect_filled(br, CornerRadius::same(255), theme::surface());
    p.rect_stroke(
        br,
        CornerRadius::same(255),
        Stroke::new(1.0, theme::amber()),
        egui::StrokeKind::Inside,
    );
    p.galley(br.min + Vec2::new(8.0, 4.0), g, theme::amber());

    // Timeline bar.
    let bar = Rect::from_min_max(
        Pos2::new(full.left() + 12.0, full.bottom() - 62.0),
        Pos2::new(full.right() - 12.0, full.bottom() - 12.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    egui::Frame::NONE
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(80),
        })
        .show(&mut child, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (icon, tip) = if v.playing {
                    (Icon::Pause, t!("Pause"))
                } else {
                    (Icon::Play, t!("Abspielen"))
                };
                if theme::icon_btn(ui, icon, tip).clicked() {
                    if !v.playing && v.t >= b {
                        v.t = a;
                    }
                    v.playing = !v.playing;
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        RichText::new(rw.label(v.t))
                            .font(theme::mono_medium(13.0))
                            .color(theme::text()),
                    );
                    let behind = b - v.t;
                    let sub = if behind < 0.5 {
                        t!("jetzt").to_string()
                    } else {
                        format!("−{}", clock(behind))
                    };
                    ui.label(RichText::new(sub).size(11.0).color(theme::text_dim()));
                });
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::btn(ui, Btn::SmallPrimary, None, "Live")
                        .on_hover_text(t!("Zurück zur laufenden Session (oder einfach tippen)"))
                        .clicked()
                    {
                        live = true;
                    }
                    if theme::icon_btn(ui, Icon::Copy, t!("Text dieses Bildschirms kopieren"))
                        .clicked()
                        && let Some(term) = &v.scratch
                    {
                        ui.ctx().copy_text(term.contents());
                    }
                    ui.add_space(6.0);
                    let w = ui.available_width().max(60.0);
                    timeline(ui, rw, v, (a, b), w, accent, marks);
                });
            });
        });
    Shown { out, live }
}

/// The scrubbable track: output activity as bars, the played part filled, a knob.
fn timeline(
    ui: &mut egui::Ui,
    rw: &Rewind,
    v: &mut RewindView,
    (a, b): (f64, f64),
    w: f32,
    accent: egui::Color32,
    marks: &[(f64, String)],
) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), egui::Sense::click_and_drag());
    let len = (b - a).max(1e-6);
    let x_of = |t: f64| rect.left() + ((t - a) / len) as f32 * rect.width();
    let t_of = |x: f32| a + ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * len;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        v.t = t_of(p.x);
        v.playing = false;
    }
    if let Some(p) = resp.hover_pos() {
        resp.clone().on_hover_text_at_pointer(rw.label(t_of(p.x)));
    }
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 22.0));
    p.rect_filled(track, CornerRadius::same(6), theme::fill());
    let n = ((rect.width() / 4.0) as usize).clamp(8, 400);
    let bw = rect.width() / n as f32;
    for (i, h) in rw.activity(n).into_iter().enumerate() {
        if h <= 0.0 {
            continue;
        }
        let x = rect.left() + i as f32 * bw;
        let bh = (h * 18.0).max(2.0);
        let r = Rect::from_min_max(
            Pos2::new(x + 0.5, track.center().y - bh / 2.0),
            Pos2::new(x + bw - 0.5, track.center().y + bh / 2.0),
        );
        let played = x + bw / 2.0 <= x_of(v.t);
        p.rect_filled(
            r,
            CornerRadius::same(1),
            if played {
                accent.gamma_multiply(0.85)
            } else {
                theme::text_dim().gamma_multiply(0.45)
            },
        );
    }
    let x = x_of(v.t);
    p.line_segment(
        [
            Pos2::new(x, track.top() - 3.0),
            Pos2::new(x, track.bottom() + 3.0),
        ],
        Stroke::new(2.0, accent),
    );
    p.circle_filled(Pos2::new(x, track.top() - 3.0), 4.5, accent);
    // Session markers: small flags with the note as tooltip; a click jumps there.
    for (i, (mt, label)) in marks.iter().enumerate() {
        if *mt < a || *mt > b {
            continue;
        }
        let mx = x_of(*mt);
        let flag =
            Rect::from_min_size(Pos2::new(mx - 1.0, track.top() - 8.0), Vec2::new(11.0, 9.0));
        let p = ui.painter();
        p.line_segment(
            [
                Pos2::new(mx, track.top() - 8.0),
                Pos2::new(mx, track.bottom()),
            ],
            Stroke::new(1.0, theme::amber()),
        );
        p.rect_filled(flag, CornerRadius::same(2), theme::amber());
        let hit = ui.interact(
            flag.expand2(Vec2::new(4.0, 4.0)),
            resp.id.with(("mark", i)),
            egui::Sense::click(),
        );
        if hit
            .on_hover_text(format!("{} · {}", rw.label(*mt), label))
            .clicked()
        {
            v.t = *mt;
            v.playing = false;
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_at(rw: &Rewind, t: f64) -> String {
        rw.render_at(t).unwrap().contents()
    }

    #[test]
    fn replays_screen_at_time() {
        let mut rw = Rewind::live();
        let blank = Terminal::with_size(40, 5, 0);
        rw.push_at(1.0, (40, 5), || blank.state_formatted(), b"one\r\n");
        rw.push_at(2.0, (40, 5), Vec::new, b"two\r\n");
        rw.push_at(3.0, (40, 5), Vec::new, b"\x1b[2J\x1b[Hthree");
        assert_eq!(rw.span(), Some((1.0, 3.0)));
        assert_eq!(text_at(&rw, 1.5).trim(), "one");
        assert_eq!(text_at(&rw, 2.0).trim(), "one\ntwo");
        assert_eq!(text_at(&rw, 9.0).trim(), "three");
        // Before the first output: the first checkpoint.
        assert_eq!(text_at(&rw, 0.0).trim(), "");
    }

    #[test]
    fn merges_close_chunks_and_checkpoints_segments() {
        let mut rw = Rewind::live();
        let mut live = Terminal::with_size(80, 24, 0);
        let line = vec![b'x'; 1000];
        let mut t = 0.0;
        for i in 0..400 {
            // Pairs 1 ms apart are merged into one chunk.
            t += if i % 2 == 0 { 1.0 } else { 0.001 };
            rw.push_at(t, live.size(), || live.state_formatted(), &line);
            live.feed(&line);
        }
        assert!(rw.segments.len() >= 3);
        assert_eq!(rw.segments[0].chunks.len() * 2, rw.segments[0].bytes / 1000);
        // Each segment replays from its own checkpoint to the same screen as the live one.
        let end = rw.span().unwrap().1;
        assert_eq!(text_at(&rw, end), live.contents());
        // A resize starts a new segment.
        let n = rw.segments.len();
        rw.push_at(t + 1.0, (100, 30), || live.state_formatted(), b"y");
        assert_eq!(rw.segments.len(), n + 1);
        assert_eq!(rw.segments.back().unwrap().cols, 100);
    }

    #[test]
    fn limits_drop_old_segments() {
        let mut rw = Rewind::with_limits(600 << 10, 100.0);
        let chunk = vec![b'a'; 64 << 10];
        for i in 0..40 {
            rw.push_at(i as f64, (80, 24), Vec::new, &chunk);
        }
        assert!(rw.bytes() <= (600 << 10) + SEGMENT);
        let (a, b) = rw.span().unwrap();
        assert!(a > 0.0 && b == 39.0);
        // By age: a later burst pushes everything older than 100 s out.
        let mut rw = Rewind::with_limits(usize::MAX, 100.0);
        for i in 0..10 {
            rw.push_at(i as f64 * 30.0, (80, 24), Vec::new, &vec![b'b'; SEGMENT]);
        }
        let (a, _) = rw.span().unwrap();
        assert!(a >= 270.0 - 100.0 - 30.0, "{a}");
        // The newest segment always stays.
        let mut rw = Rewind::with_limits(10, 1.0);
        rw.push_at(0.0, (80, 24), Vec::new, &[b'z'; 100]);
        assert_eq!(rw.segments.len(), 1);
    }

    #[test]
    fn scrubbing_feeds_forward_and_rebuilds_backwards() {
        let mut rw = Rewind::live();
        rw.push_at(1.0, (20, 3), Vec::new, b"a");
        rw.push_at(2.0, (20, 3), Vec::new, b"b");
        rw.push_at(3.0, (20, 3), Vec::new, b"c");
        let mut v = RewindView::new(Some(&rw), None);
        assert_eq!(v.t, 3.0);
        v.t = 1.0;
        v.sync(&rw);
        assert_eq!(v.scratch.as_ref().unwrap().contents(), "a");
        v.t = 2.5;
        v.sync(&rw);
        assert_eq!(
            (v.fed, v.scratch.as_ref().unwrap().contents()),
            (2, "ab".into())
        );
        v.t = 1.5;
        v.sync(&rw);
        assert_eq!(v.scratch.as_ref().unwrap().contents(), "a");
        assert_eq!(rw.next_output(1.0), Some(2.0));
        assert_eq!(clock(75.0), "1:15");
        assert_eq!(clock(3725.0), "1:02:05");
    }

    #[test]
    fn recording_buffer() {
        use crate::backend::record::Frame;
        let frames = vec![
            Frame {
                time: 0.5,
                data: "hello ".into(),
            },
            Frame {
                time: 1.5,
                data: "world".into(),
            },
        ];
        let rw = Rewind::from_cast(30, 4, &frames);
        assert_eq!(rw.span(), Some((0.5, 1.5)));
        assert_eq!(text_at(&rw, 1.0), "hello ");
        assert_eq!(text_at(&rw, 2.0), "hello world");
        assert_eq!(rw.label(65.0), "1:05");
    }
}
