//! Live host statistics in the app: the sampler manager (one SSH connection per watched
//! host, only while the host is visible in the list or its panel is open), the detail
//! sheet with area charts and the tiny sparklines in the host list rows. Everything is
//! painted with egui in theme colours.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Stroke, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;
use uuid::Uuid;

use crate::app::App;
use crate::backend::Sink;
use crate::backend::hoststats_core::{self as core, Event, History, Kind, Task};
use crate::model::Protocol;
use crate::theme;
use crate::ui::containers_page::background_params;
use crate::ui::human_size;

/// Most hosts sampled at the same time.
pub const MAX_WATCH: usize = 10;
/// A watcher keeps running this long after nobody looks at the host any more.
const LINGER: Duration = Duration::from_secs(15);
/// A row counts as visible this long after it was last drawn.
const VISIBLE_FOR: Duration = Duration::from_secs(3);

enum WState {
    Connecting,
    Live,
    Unsupported(String),
    NoAccess(String),
    Error(String),
}

struct Watcher {
    gen_id: u64,
    stop: Option<watch::Sender<bool>>,
    state: WState,
    hist: History,
    last_wanted: Instant,
    retry_at: Option<Instant>,
    failures: u32,
}

#[derive(Default)]
pub struct StatsState {
    /// The detail sheet shows this host.
    pub panel: Option<Uuid>,
    watchers: HashMap<Uuid, Watcher>,
    /// Rows drawn recently (written while drawing, hence the cell).
    visible: RefCell<HashMap<Uuid, Instant>>,
    io: Option<(Sink<Event>, UnboundedReceiver<Event>)>,
    next_gen: u64,
}

fn interval() -> u32 {
    core::DEFAULT_INTERVAL
}

impl Watcher {
    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(true);
        }
    }
}

/// Once per frame: takes in samples, starts and stops the samplers, draws the sheet.
pub fn show(app: &mut App, ctx: &egui::Context) {
    tick(app, ctx);
    panel(app, ctx);
}

fn tick(app: &mut App, ctx: &egui::Context) {
    let now = Instant::now();
    // --- events
    let mut events = Vec::new();
    if let Some((_, rx)) = app.srv.stats.io.as_mut() {
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    for e in events {
        let Some(w) = app.srv.stats.watchers.get_mut(&e.host) else {
            continue;
        };
        if w.gen_id != e.gen_id {
            continue;
        }
        match e.kind {
            Kind::Connected => {}
            Kind::Sample(raw) => {
                w.hist.push(*raw);
                w.state = WState::Live;
                w.failures = 0;
            }
            Kind::Unsupported(why) => {
                w.state = WState::Unsupported(why);
                w.stop = None;
            }
            Kind::Ended(why) => {
                w.failures += 1;
                w.state = WState::Error(why);
                w.stop = None;
                let back = 5u64 * 2u64.pow(w.failures.min(4));
                w.retry_at = Some(now + Duration::from_secs(back));
            }
        }
    }
    // --- who is wanted
    app.srv
        .stats
        .visible
        .borrow_mut()
        .retain(|_, t| now.duration_since(*t) < VISIBLE_FOR);
    let mut wanted: Vec<Uuid> = Vec::new();
    if let Some(p) = app.srv.stats.panel {
        wanted.push(p);
    }
    if app.lib.settings.host_sparklines {
        let mut vis: Vec<(Uuid, Instant)> = app
            .srv
            .stats
            .visible
            .borrow()
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        vis.sort_by_key(|(id, _)| *id);
        for (id, _) in vis {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
    }
    wanted.truncate(MAX_WATCH);
    for id in &wanted {
        if let Some(w) = app.srv.stats.watchers.get_mut(id) {
            w.last_wanted = now;
        }
    }
    // --- stop the ones nobody looks at
    app.srv.stats.watchers.retain(|id, w| {
        let keep = wanted.contains(id) || now.duration_since(w.last_wanted) < LINGER;
        if !keep {
            w.stop();
        }
        keep
    });
    // --- start / restart
    for id in wanted {
        let due = match app.srv.stats.watchers.get(&id) {
            None => true,
            Some(w) => {
                w.stop.is_none()
                    && matches!(w.state, WState::Error(_))
                    && w.retry_at.is_some_and(|t| now >= t)
            }
        };
        if !due {
            continue;
        }
        let Some(host) = app.lib.host(id).cloned() else {
            continue;
        };
        let failures = app.srv.stats.watchers.get(&id).map_or(0, |w| w.failures);
        let hist = app
            .srv
            .stats
            .watchers
            .remove(&id)
            .map_or_else(|| History::new(interval()), |w| w.hist);
        let mut w = Watcher {
            gen_id: 0,
            stop: None,
            state: WState::Connecting,
            hist,
            last_wanted: now,
            retry_at: None,
            failures,
        };
        if host.protocol != Protocol::Ssh {
            w.state = WState::Unsupported(t!("Nur für SSH-Hosts").to_string());
        } else {
            match background_params(app, &host) {
                Err(skip) => w.state = WState::NoAccess(skip.text().to_string()),
                Ok(params) => {
                    let st = &mut app.srv.stats;
                    st.next_gen += 1;
                    w.gen_id = st.next_gen;
                    let (tx, rx) = watch::channel(false);
                    w.stop = Some(tx);
                    let sink = st
                        .io
                        .get_or_insert_with(|| Sink::new(ctx.clone()))
                        .0
                        .clone();
                    core::spawn(
                        &app.rt,
                        Task {
                            host: id,
                            gen_id: w.gen_id,
                            params,
                            interval: interval(),
                        },
                        sink,
                        app.prompts_sink.clone(),
                        rx,
                    );
                }
            }
        }
        app.srv.stats.watchers.insert(id, w);
    }
    // Come back for retries and for lingering watchers.
    if !app.srv.stats.watchers.is_empty() {
        ctx.request_repaint_after(Duration::from_secs(2));
    }
}

// ------------------------------------------------------------------ list rows

/// Width a row gives to its sparkline (0 = none); also marks the host as visible.
pub fn row_spark_width(app: &App, h: &crate::model::Host, row_w: f32) -> f32 {
    if !app.lib.settings.host_sparklines || h.protocol != Protocol::Ssh || row_w < 640.0 {
        return 0.0;
    }
    app.srv
        .stats
        .visible
        .borrow_mut()
        .insert(h.id, Instant::now());
    // Hosts without /proc or access show nothing.
    match app.srv.stats.watchers.get(&h.id).map(|w| &w.state) {
        Some(WState::Unsupported(_) | WState::NoAccess(_)) => 0.0,
        _ => 78.0,
    }
}

/// Tiny CPU (accent) and memory (second colour) lines.
pub fn row_spark(app: &App, p: &egui::Painter, h: &crate::model::Host, rect: Rect) {
    let Some(w) = app.srv.stats.watchers.get(&h.id) else {
        return;
    };
    if w.hist.points.len() < 2 {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "…",
            theme::medium(12.0),
            theme::text_dim(),
        );
        return;
    }
    let half = (rect.height() - 2.0) / 2.0;
    let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), half));
    let bottom = Rect::from_min_size(
        Pos2::new(rect.left(), rect.top() + half + 2.0),
        Vec2::new(rect.width(), half),
    );
    spark(p, top, &w.hist.series(|x| x.cpu), 100.0, theme::accent());
    spark(
        p,
        bottom,
        &w.hist.series(|x| x.mem),
        100.0,
        theme::accent2(),
    );
}

/// A line with a soft fill under it.
pub fn spark(p: &egui::Painter, rect: Rect, values: &[f32], max: f32, color: Color32) {
    if values.len() < 2 || max <= 0.0 {
        return;
    }
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            Pos2::new(
                rect.left() + rect.width() * i as f32 / (values.len() - 1) as f32,
                rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0),
            )
        })
        .collect();
    fill_under(p, &pts, rect.bottom(), theme::soft(color));
    p.add(egui::Shape::line(pts, Stroke::new(1.3, color)));
}

fn fill_under(p: &egui::Painter, pts: &[Pos2], base: f32, color: Color32) {
    for w in pts.windows(2) {
        p.add(egui::Shape::convex_polygon(
            vec![w[0], w[1], Pos2::new(w[1].x, base), Pos2::new(w[0].x, base)],
            color,
            Stroke::NONE,
        ));
    }
}

// ------------------------------------------------------------------ sheet

fn level_color(pct: f32) -> Color32 {
    if pct >= 90.0 {
        theme::red()
    } else if pct >= 75.0 {
        theme::amber()
    } else {
        theme::green()
    }
}

struct Series<'a> {
    name: &'static str,
    color: Color32,
    values: Vec<f32>,
    _p: std::marker::PhantomData<&'a ()>,
}

/// Rounds a maximum up to a "nice" axis end (1, 2, 5 × 10ⁿ).
fn nice_max(v: f32) -> f32 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f32.powf(v.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if v <= m * mag {
            return m * mag;
        }
    }
    10.0 * mag
}

#[allow(clippy::too_many_arguments)]
fn chart(
    ui: &mut egui::Ui,
    title: &str,
    hist: &History,
    series: &[Series],
    fixed_max: Option<f32>,
    fmt: &dyn Fn(f32) -> String,
    height: f32,
) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(12), theme::bg_deep());
    p.text(
        rect.left_top() + Vec2::new(14.0, 10.0),
        egui::Align2::LEFT_TOP,
        title,
        theme::semibold(13.0),
        theme::text(),
    );
    // Current values on the right.
    let mut x = rect.right() - 14.0;
    for s in series.iter().rev() {
        if let Some(v) = s.values.last() {
            let text = format!("{} {}", s.name, fmt(*v));
            let g = p.layout_no_wrap(text, theme::medium(12.5), s.color);
            x -= g.size().x;
            p.galley(Pos2::new(x, rect.top() + 10.0), g, s.color);
            x -= 14.0;
        }
    }
    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + 14.0, rect.top() + 34.0),
        Pos2::new(rect.right() - 14.0, rect.bottom() - 18.0),
    );
    let max = fixed_max.unwrap_or_else(|| {
        nice_max(
            series
                .iter()
                .flat_map(|s| s.values.iter().copied())
                .fold(0.0, f32::max)
                * 1.1,
        )
    });
    for k in 0..=2 {
        let y = plot.bottom() - plot.height() * k as f32 / 2.0;
        p.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0, theme::line()),
        );
        if k > 0 {
            let (anchor, dy) = if k == 2 {
                (egui::Align2::LEFT_TOP, 2.0)
            } else {
                (egui::Align2::LEFT_BOTTOM, -2.0)
            };
            p.text(
                Pos2::new(plot.left() + 2.0, y + dy),
                anchor,
                fmt(max * k as f32 / 2.0),
                theme::medium(10.5),
                theme::text_dim(),
            );
        }
    }
    p.text(
        Pos2::new(plot.left(), rect.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        t!("vor {} min", core::WINDOW_SECS / 60),
        theme::medium(10.5),
        theme::text_dim(),
    );
    p.text(
        Pos2::new(plot.right(), rect.bottom() - 4.0),
        egui::Align2::RIGHT_BOTTOM,
        t!("jetzt"),
        theme::medium(10.5),
        theme::text_dim(),
    );
    let Some(last) = hist.points.back() else {
        p.text(
            plot.center(),
            egui::Align2::CENTER_CENTER,
            t!("sammle Messwerte…"),
            theme::medium(13.0),
            theme::text_dim(),
        );
        return;
    };
    let xs: Vec<f32> = hist
        .points
        .iter()
        .map(|pt| plot.right() - plot.width() * ((last.t - pt.t) / core::WINDOW_SECS as f64) as f32)
        .collect();
    let clipped = p.with_clip_rect(plot.expand2(Vec2::new(0.0, 3.0)));
    for (si, s) in series.iter().enumerate() {
        let pts: Vec<Pos2> = xs
            .iter()
            .zip(&s.values)
            .map(|(x, v)| {
                Pos2::new(
                    *x,
                    plot.bottom() - plot.height() * (v / max).clamp(0.0, 1.0),
                )
            })
            .collect();
        if si == 0 {
            fill_under(&clipped, &pts, plot.bottom(), theme::soft(s.color));
        }
        clipped.add(egui::Shape::line(pts, Stroke::new(1.6, s.color)));
    }
    // Crosshair and values at the pointer.
    if let Some(pos) = resp.hover_pos().filter(|q| plot.contains(*q)) {
        let idx = xs
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - pos.x)
                    .abs()
                    .partial_cmp(&(b.1 - pos.x).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(i, _)| i);
        p.line_segment(
            [
                Pos2::new(xs[idx], plot.top()),
                Pos2::new(xs[idx], plot.bottom()),
            ],
            Stroke::new(1.0, theme::text_dim()),
        );
        let ago = (last.t - hist.points[idx].t).round() as u32;
        let mut tip = if ago == 0 {
            t!("jetzt").to_string()
        } else {
            t!("vor {} s", ago)
        };
        for s in series {
            tip.push_str(&format!("\n{}: {}", s.name, fmt(s.values[idx])));
        }
        resp.on_hover_text(tip);
    }
}

fn series<'a>(name: &'static str, color: Color32, values: Vec<f32>) -> Series<'a> {
    Series {
        name,
        color,
        values,
        _p: std::marker::PhantomData,
    }
}

fn panel(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.srv.stats.panel else {
        return;
    };
    let Some(host) = app.lib.host(id).cloned() else {
        app.srv.stats.panel = None;
        return;
    };
    let w = app.srv.stats.watchers.get(&id);
    let dismissed = {
        let (_, dismissed) = theme::sheet(
            ctx,
            egui::Id::new("hoststats-sheet"),
            760.0,
            &t!("Live-Statistik: {}", host.display_name()),
            |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                let Some(w) = w else {
                    ui.add(egui::Spinner::new());
                    return;
                };
                match &w.state {
                    WState::Connecting if w.hist.points.is_empty() => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(RichText::new(t!("verbinde…")).color(theme::text_dim()));
                        });
                    }
                    WState::Unsupported(why) => {
                        status_card(
                            ui,
                            theme::amber(),
                            t!("Nicht unterstützt"),
                            &format!(
                                "{why}\n{}",
                                t!(
                                    "Die Live-Statistik liest /proc und braucht einen Linux-Host mit POSIX-Shell (BSD und Windows-OpenSSH liefern keine Werte)."
                                )
                            ),
                        );
                    }
                    WState::NoAccess(why) => {
                        status_card(ui, theme::amber(), t!("Kein Zugang im Hintergrund"), why);
                    }
                    WState::Error(why) => {
                        status_card(
                            ui,
                            theme::red(),
                            t!("Verbindung unterbrochen – neuer Versuch folgt"),
                            why,
                        );
                    }
                    _ => {}
                }
                if w.hist.points.is_empty() {
                    return;
                }
                let h = &w.hist;
                let pct = |v: f32| format!("{v:.0} %");
                chart(
                    ui,
                    "CPU",
                    h,
                    &[series("CPU", theme::accent(), h.series(|p| p.cpu))],
                    Some(100.0),
                    &pct,
                    130.0,
                );
                let mem_title = format!(
                    "{} ({})",
                    t!("Arbeitsspeicher"),
                    human_size(h.mem_total_kb * 1024)
                );
                let mut mem = vec![series("RAM", theme::accent2(), h.series(|p| p.mem))];
                if h.swap_total_kb > 0 {
                    mem.push(series("Swap", theme::amber(), h.series(|p| p.swap)));
                }
                chart(ui, &mem_title, h, &mem, Some(100.0), &pct, 130.0);
                chart(
                    ui,
                    t!("Last (1 min)"),
                    h,
                    &[series("load", theme::pink(), h.series(|p| p.load1))],
                    None,
                    &|v| format!("{v:.2}"),
                    110.0,
                );
                if h.has_net {
                    let rate = |v: f32| format!("{}/s", human_size(v as u64));
                    chart(
                        ui,
                        t!("Netzwerk"),
                        h,
                        &[
                            series("↓", theme::green(), h.series(|p| p.rx as f32)),
                            series("↑", theme::accent(), h.series(|p| p.tx as f32)),
                        ],
                        None,
                        &rate,
                        120.0,
                    );
                }
                if !h.disks.is_empty() {
                    theme::section_title(ui, t!("Datenträger"));
                    for d in &h.disks {
                        let (r, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 22.0),
                            egui::Sense::hover(),
                        );
                        let p = ui.painter_at(r);
                        let bar = Rect::from_min_max(
                            Pos2::new(r.left() + 150.0, r.center().y - 5.0),
                            Pos2::new(r.right() - 190.0, r.center().y + 5.0),
                        );
                        p.text(
                            Pos2::new(r.left(), r.center().y),
                            egui::Align2::LEFT_CENTER,
                            d.mount.chars().take(18).collect::<String>(),
                            egui::FontId::monospace(12.5),
                            theme::text(),
                        );
                        p.rect_filled(bar, CornerRadius::same(5), theme::fill());
                        let mut f = bar;
                        f.set_width(bar.width() * (d.pct() / 100.0).clamp(0.0, 1.0));
                        p.rect_filled(f, CornerRadius::same(5), level_color(d.pct()));
                        p.text(
                            Pos2::new(r.right(), r.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!(
                                "{:.0} %  ·  {} / {}",
                                d.pct(),
                                human_size(d.used_kb * 1024),
                                human_size(d.total_kb * 1024)
                            ),
                            theme::medium(12.5),
                            theme::text_dim(),
                        );
                    }
                }
            },
            |ui, ()| {
                ui.label(
                    RichText::new(t!("Messung alle {} s über eine SSH-Verbindung, die schließt, wenn du das Fenster schließt", interval()))
                        .size(12.0)
                        .color(theme::text_dim()),
                );
            },
        );
        dismissed
    };
    if dismissed {
        app.srv.stats.panel = None;
    }
}

fn status_card(ui: &mut egui::Ui, color: Color32, title: &str, body: &str) {
    egui::Frame::NONE
        .fill(theme::soft(color))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(title)
                    .font(theme::semibold(14.0))
                    .color(color),
            );
            ui.add(egui::Label::new(RichText::new(body).size(12.5).color(theme::text())).wrap());
        });
}

/// Opens the sheet for a host.
pub fn open(app: &mut App, host: Uuid) {
    app.srv.stats.panel = Some(host);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_ends_are_round() {
        assert_eq!(nice_max(0.0), 1.0);
        assert_eq!(nice_max(0.3), 0.5);
        assert_eq!(nice_max(1.2), 2.0);
        assert_eq!(nice_max(7.0), 10.0);
        assert_eq!(nice_max(130.0), 200.0);
        assert_eq!(nice_max(1_500_000.0), 2_000_000.0);
    }

    #[test]
    fn levels() {
        assert_ne!(level_color(10.0), level_color(95.0));
    }
}
