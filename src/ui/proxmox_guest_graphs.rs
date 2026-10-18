//! Live graphs: CPU sparklines in the guest rows, the statistics tab of the guest sheet
//! and the node statistics sheet (click on a node heading). Drawn with the painter:
//! lines with a soft fill, axis labels and the values under the pointer.

use super::*;
use crate::backend::proxmox::{RrdPoint, RrdTarget, Timeframe};

/// Sparklines are fetched again after this long.
const SPARK_TTL: Duration = Duration::from_secs(60);
/// The statistics reload while shown.
const STATS_TTL: Duration = Duration::from_secs(60);
/// A row counts as visible this long after it was drawn.
const SEEN: Duration = Duration::from_secs(3);
/// Guests per sparkline request round.
const SPARK_BATCH: usize = 40;

// ------------------------------------------------------------------ sparklines

#[derive(Default)]
pub struct Sparks {
    data: HashMap<(Uuid, u32), (Vec<f32>, Instant)>,
    seen: HashMap<(Uuid, u32), (GuestId, Instant)>,
    loading: HashSet<Uuid>,
}

impl Sparks {
    pub fn forget(&mut self, server: Uuid) {
        self.data.retain(|(s, _), _| *s != server);
        self.seen.retain(|(s, _), _| *s != server);
        self.loading.remove(&server);
    }

    /// The row of a guest is drawn: remember it as visible, return its CPU history.
    pub fn visible(&mut self, server: Uuid, g: &Guest) -> Option<Vec<f32>> {
        if !g.running() || g.template {
            return None;
        }
        self.seen.insert(
            (server, g.vmid),
            (
                GuestId {
                    node: g.node.clone(),
                    kind: g.kind,
                    vmid: g.vmid,
                },
                Instant::now(),
            ),
        );
        self.data
            .get(&(server, g.vmid))
            .map(|(v, _)| v.clone())
            .filter(|v| v.len() >= 2)
    }
}

/// Fetches the history of visible rows that have none or an old one (one job per
/// server at a time).
pub fn pump_sparks(app: &mut App) {
    let now = Instant::now();
    let mut jobs: HashMap<Uuid, Vec<GuestId>> = HashMap::new();
    {
        let sp = &mut app.pve.ext.sparks;
        sp.seen.retain(|_, (_, at)| now.duration_since(*at) < SEEN);
        for ((server, vmid), (id, _)) in &sp.seen {
            if sp.loading.contains(server) {
                continue;
            }
            let stale = sp
                .data
                .get(&(*server, *vmid))
                .is_none_or(|(_, at)| now.duration_since(*at) >= SPARK_TTL);
            if stale {
                jobs.entry(*server).or_default().push(id.clone());
            }
        }
    }
    for (server, mut guests) in jobs {
        let Some(s) = server_of(app, server) else {
            continue;
        };
        // Untrusted certificate or failed login: the page shows that already.
        if app.pve.live.get(&server).is_none_or(|l| l.error.is_some()) {
            continue;
        }
        guests.sort_by_key(|g| g.vmid);
        guests.truncate(SPARK_BATCH);
        let sp = &mut app.pve.ext.sparks;
        sp.loading.insert(server);
        for g in &guests {
            sp.data
                .entry((server, g.vmid))
                .or_insert_with(|| (Vec::new(), now))
                .1 = now;
        }
        let c = conn(app, &s);
        pve::spawn_sparks(
            &app.rt,
            server,
            app.pve.epoch(server),
            c,
            guests,
            app.pve.sink.clone(),
        );
    }
}

pub fn on_sparks(app: &mut App, server: Uuid, list: Vec<(u32, Vec<f32>)>) {
    let sp = &mut app.pve.ext.sparks;
    sp.loading.remove(&server);
    let now = Instant::now();
    for (vmid, v) in list {
        sp.data.insert((server, vmid), (v, now));
    }
}

/// Soft fill from `pts` down to `base`, fading out.
fn fill_under(p: &egui::Painter, pts: &[Pos2], base: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    for w in pts.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], color);
        mesh.colored_vertex(w[1], color);
        mesh.colored_vertex(Pos2::new(w[1].x, base), Color32::TRANSPARENT);
        mesh.colored_vertex(Pos2::new(w[0].x, base), Color32::TRANSPARENT);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// CPU sparkline (percent values) in `rect`; scaled to at least 5 %.
pub fn paint_spark(p: &egui::Painter, rect: Rect, values: &[f32], color: Color32) {
    if values.len() < 2 {
        return;
    }
    let max = values.iter().copied().fold(0.0f32, f32::max).max(5.0);
    let n = (values.len() - 1) as f32;
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            Pos2::new(
                rect.left() + rect.width() * i as f32 / n,
                rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0),
            )
        })
        .collect();
    fill_under(p, &pts, rect.bottom(), color.gamma_multiply(0.22));
    p.add(egui::Shape::line(pts, Stroke::new(1.4, color)));
}

// ------------------------------------------------------------------ statistics data

/// Loaded `rrddata` per timeframe.
#[derive(Default)]
pub struct RrdView {
    pub tf: Timeframe,
    data: HashMap<Timeframe, (Result<Vec<RrdPoint>, String>, Instant)>,
    loading: HashSet<Timeframe>,
}

impl RrdView {
    pub fn has(&self, tf: Timeframe) -> bool {
        self.data.contains_key(&tf)
    }

    /// The shown timeframe is older than a minute (and not being loaded).
    pub fn stale(&self) -> bool {
        !self.loading.contains(&self.tf)
            && self
                .data
                .get(&self.tf)
                .is_some_and(|(_, at)| at.elapsed() >= STATS_TTL)
    }

    /// Marks the current timeframe as loading; `false` if it already is.
    fn start(&mut self) -> bool {
        self.loading.insert(self.tf)
    }

    fn set(&mut self, tf: Timeframe, r: Result<Vec<RrdPoint>, Error>) {
        self.loading.remove(&tf);
        self.data
            .insert(tf, (r.map_err(|e| e.to_string()), Instant::now()));
    }
}

/// Loads the statistics of the open guest sheet.
pub fn load_guest(app: &mut App) {
    let Some(p) = app.pve.panel.as_mut() else {
        return;
    };
    if !p.stats.start() {
        return;
    }
    let (g, tf) = (p.guest.clone(), p.stats.tf);
    let Some(s) = server_of(app, g.server) else {
        return;
    };
    let c = conn(app, &s);
    pve::spawn_rrd(
        &app.rt,
        s.id,
        app.pve.epoch(s.id),
        c,
        RrdTarget::Guest(g.id()),
        tf,
        app.pve.sink.clone(),
    );
}

pub fn on_rrd(
    app: &mut App,
    server: Uuid,
    target: RrdTarget,
    tf: Timeframe,
    result: Result<Vec<RrdPoint>, Error>,
) {
    match target {
        RrdTarget::Guest(g) => {
            if let Some(p) = app
                .pve
                .panel
                .as_mut()
                .filter(|p| p.guest.server == server && p.guest.vmid == g.vmid)
            {
                p.stats.set(tf, result);
            }
        }
        RrdTarget::Node(n) => {
            if let Some(ns) = app
                .pve
                .ext
                .node_stats
                .as_mut()
                .filter(|ns| ns.server == server && ns.node == n)
            {
                ns.view.set(tf, result);
            }
        }
    }
}

// ------------------------------------------------------------------ node sheet

pub struct NodeStats {
    pub server: Uuid,
    pub node: String,
    view: RrdView,
}

pub fn open_node(app: &mut App, server: Uuid, node: String) {
    app.pve.ext.node_stats = Some(NodeStats {
        server,
        node,
        view: RrdView::default(),
    });
    load_node(app);
}

fn load_node(app: &mut App) {
    let Some(ns) = app.pve.ext.node_stats.as_mut() else {
        return;
    };
    if !ns.view.start() {
        return;
    }
    let (server, node, tf) = (ns.server, ns.node.clone(), ns.view.tf);
    let Some(s) = server_of(app, server) else {
        return;
    };
    let c = conn(app, &s);
    pve::spawn_rrd(
        &app.rt,
        s.id,
        app.pve.epoch(s.id),
        c,
        RrdTarget::Node(node),
        tf,
        app.pve.sink.clone(),
    );
}

pub fn node_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(ns) = app.pve.ext.node_stats.as_mut() else {
        return;
    };
    let stale = ns.view.stale();
    let title = t!("Node {} · Statistik", ns.node);
    let ((close, refresh), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-node-stats"),
        720.0,
        &title,
        |ui| {
            let picked = stats_body(ui, &mut ns.view, true);
            if let Some(tf) = picked {
                ns.view.tf = tf;
            }
            picked.is_some()
        },
        |ui, picked| {
            let close = theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked();
            let refresh =
                theme::icon_btn(ui, Icon::Refresh, t!("Jetzt aktualisieren")).clicked() || picked;
            (close, refresh)
        },
    );
    let reload = stale || refresh;
    if close || dismissed {
        app.pve.ext.node_stats = None;
    } else if reload {
        load_node(app);
    }
}

// ------------------------------------------------------------------ charts

/// Statistics of a guest (CPU, RAM, network, disk) or a node (CPU with IO wait, RAM,
/// network, load). Returns a newly picked timeframe.
pub fn stats_body(ui: &mut egui::Ui, view: &mut RrdView, node: bool) -> Option<Timeframe> {
    let mut picked = None;
    ui.horizontal(|ui| {
        let cur = Timeframe::ALL
            .iter()
            .position(|t| *t == view.tf)
            .unwrap_or(0);
        if let Some(i) = theme::segmented(ui, &[t!("Stunde"), t!("Tag"), t!("Woche")], cur)
            && i != cur
        {
            picked = Some(Timeframe::ALL[i]);
        }
        if view.loading.contains(&view.tf) && view.has(view.tf) {
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    ui.add_space(4.0);
    let tf = view.tf;
    match view.data.get(&tf).map(|(r, _)| r) {
        None => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
            });
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::red()));
        }
        Some(Ok(pts)) if pts.iter().all(|p| p.cpu.is_nan() && p.mem.is_nan()) => {
            ui.label(RichText::new(t!("Noch keine Messwerte.")).color(theme::text_dim()));
        }
        Some(Ok(pts)) => {
            let s = |f: fn(&RrdPoint) -> f64| -> Vec<(i64, f64)> {
                pts.iter().map(|p| (p.time, f(p))).collect()
            };
            chart(
                ui,
                t!("CPU"),
                &[Series {
                    label: t!("Last"),
                    color: theme::accent(),
                    pts: s(|p| p.cpu * 100.0),
                    fill: true,
                }]
                .into_iter()
                .chain(node.then(|| Series {
                    label: t!("IO-Wait"),
                    color: theme::accent2(),
                    pts: s(|p| p.iowait * 100.0),
                    fill: true,
                }))
                .collect::<Vec<_>>(),
                &fmt_pct,
                5.0,
                tf,
            );
            chart(
                ui,
                t!("Arbeitsspeicher"),
                &[
                    Series {
                        label: t!("belegt"),
                        color: theme::accent2(),
                        pts: s(|p| p.mem),
                        fill: true,
                    },
                    Series {
                        label: t!("gesamt"),
                        color: theme::text_dim(),
                        pts: s(|p| p.maxmem),
                        fill: false,
                    },
                ],
                &fmt_bytes,
                1024.0 * 1024.0,
                tf,
            );
            chart(
                ui,
                t!("Netzwerk"),
                &[
                    Series {
                        label: t!("ein"),
                        color: theme::green(),
                        pts: s(|p| p.netin),
                        fill: true,
                    },
                    Series {
                        label: t!("aus"),
                        color: theme::accent(),
                        pts: s(|p| p.netout),
                        fill: true,
                    },
                ],
                &fmt_rate,
                1024.0,
                tf,
            );
            if node {
                chart(
                    ui,
                    t!("Systemlast"),
                    &[Series {
                        label: t!("1 min"),
                        color: theme::pink(),
                        pts: s(|p| p.load),
                        fill: true,
                    }],
                    &|v| format!("{v:.2}"),
                    1.0,
                    tf,
                );
            } else {
                chart(
                    ui,
                    t!("Datenträger"),
                    &[
                        Series {
                            label: t!("lesen"),
                            color: theme::green(),
                            pts: s(|p| p.diskread),
                            fill: true,
                        },
                        Series {
                            label: t!("schreiben"),
                            color: theme::amber(),
                            pts: s(|p| p.diskwrite),
                            fill: true,
                        },
                    ],
                    &fmt_rate,
                    1024.0,
                    tf,
                );
            }
        }
    }
    picked
}

pub struct Series<'a> {
    pub label: &'a str,
    pub color: Color32,
    /// Unix time and value; NaN = gap.
    pub pts: Vec<(i64, f64)>,
    /// Soft fill under the line (not for limits like the total RAM).
    pub fill: bool,
}

pub fn fmt_pct(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.1} %")
    } else {
        format!("{v:.0} %")
    }
}

pub fn fmt_bytes(v: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = v.max(0.0);
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 || v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn fmt_rate(v: f64) -> String {
    format!("{}/s", fmt_bytes(v))
}

/// 1, 2, 2.5 or 5 times a power of ten, at least `v`.
pub fn nice_ceil(v: f64) -> f64 {
    if !v.is_finite() || v <= 0.0 {
        return 1.0;
    }
    let exp = 10f64.powf(v.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if m * exp >= v * 0.999_999 {
            return m * exp;
        }
    }
    10.0 * exp
}

/// Byte values get binary steps (1, 2, 4 … × 1024ⁿ) so the labels stay round.
fn nice_ceil_bytes(v: f64) -> f64 {
    if !v.is_finite() || v <= 0.0 {
        return 1.0;
    }
    let mut unit = 1.0;
    while v >= unit * 1024.0 {
        unit *= 1024.0;
    }
    let mut m = 1.0;
    while m * unit < v * 0.999_999 {
        m *= 2.0;
    }
    m * unit
}

fn tick_label(ts: i64, tf: Timeframe) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            let d = d.with_timezone(&chrono::Local);
            match tf {
                Timeframe::Week => d.format("%d.%m. %H:%M").to_string(),
                _ => d.format("%H:%M").to_string(),
            }
        })
        .unwrap_or_default()
}

fn chart(
    ui: &mut egui::Ui,
    title: &str,
    series: &[Series],
    fmt: &dyn Fn(f64) -> String,
    floor: f64,
    tf: Timeframe,
) {
    let bytes = (floor - 1024.0).abs() < 1.0 || floor >= 1024.0 * 1024.0;
    let raw_max = series
        .iter()
        .flat_map(|s| s.pts.iter().map(|p| p.1))
        .filter(|v| v.is_finite())
        .fold(0.0f64, f64::max)
        .max(floor);
    let ymax = if bytes {
        nice_ceil_bytes(raw_max)
    } else {
        nice_ceil(raw_max)
    };
    let (t0, t1) = series
        .iter()
        .flat_map(|s| s.pts.iter().map(|p| p.0))
        .fold((i64::MAX, i64::MIN), |(a, b), t| (a.min(t), b.max(t)));
    group(ui, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 10,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).font(theme::semibold(13.5)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        for s in series.iter().rev() {
                            let last = s
                                .pts
                                .iter()
                                .rev()
                                .find(|p| p.1.is_finite())
                                .map(|p| fmt(p.1))
                                .unwrap_or_else(|| "–".into());
                            ui.label(RichText::new(last).font(theme::medium(12.0)));
                            ui.label(RichText::new(s.label).size(12.0).color(theme::text_dim()));
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, s.color);
                            ui.add_space(6.0);
                        }
                    });
                });
                ui.add_space(4.0);
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 118.0), Sense::hover());
                let plot = Rect::from_min_max(
                    rect.min + Vec2::new(62.0, 6.0),
                    rect.max - Vec2::new(4.0, 18.0),
                );
                let p = ui.painter_at(rect);
                // Grid and y labels.
                for (i, frac) in [0.0f32, 0.5, 1.0].iter().enumerate() {
                    let y = plot.bottom() - plot.height() * frac;
                    p.line_segment(
                        [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                        Stroke::new(1.0, if i == 0 { theme::line() } else { theme::fill() }),
                    );
                    p.text(
                        Pos2::new(plot.left() - 8.0, y),
                        egui::Align2::RIGHT_CENTER,
                        if i == 0 {
                            "0".to_string()
                        } else {
                            fmt(ymax * *frac as f64)
                        },
                        FontId::proportional(11.0),
                        theme::text_dim(),
                    );
                }
                if t1 <= t0 {
                    return;
                }
                let span = (t1 - t0) as f32;
                let x_of = |t: i64| plot.left() + plot.width() * (t - t0) as f32 / span;
                let y_of =
                    |v: f64| plot.bottom() - plot.height() * (v / ymax).clamp(0.0, 1.0) as f32;
                // Time labels: start, middle, end.
                for (t, align) in [
                    (t0, egui::Align2::LEFT_TOP),
                    ((t0 + t1) / 2, egui::Align2::CENTER_TOP),
                    (t1, egui::Align2::RIGHT_TOP),
                ] {
                    p.text(
                        Pos2::new(x_of(t), plot.bottom() + 4.0),
                        align,
                        tick_label(t, tf),
                        FontId::proportional(11.0),
                        theme::text_dim(),
                    );
                }
                for s in series {
                    // Lines break at gaps (NaN).
                    let mut run: Vec<Pos2> = Vec::new();
                    let flush = |run: &mut Vec<Pos2>| {
                        if run.len() >= 2 {
                            if s.fill {
                                fill_under(&p, run, plot.bottom(), s.color.gamma_multiply(0.16));
                            }
                            p.add(egui::Shape::line(
                                std::mem::take(run),
                                Stroke::new(1.6, s.color),
                            ));
                        } else if let Some(pt) = run.pop() {
                            p.circle_filled(pt, 1.5, s.color);
                        }
                        run.clear();
                    };
                    for (t, v) in &s.pts {
                        if v.is_finite() {
                            run.push(Pos2::new(x_of(*t), y_of(*v)));
                        } else {
                            flush(&mut run);
                        }
                    }
                    flush(&mut run);
                }
                // Values under the pointer.
                let Some(pos) = resp
                    .hover_pos()
                    .filter(|p| p.x >= plot.left() - 2.0 && p.x <= plot.right() + 2.0)
                else {
                    return;
                };
                let Some(first) = series.first() else {
                    return;
                };
                let t = t0 + ((pos.x - plot.left()) / plot.width() * span).round() as i64;
                let Some(idx) = first
                    .pts
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| (p.0 - t).abs())
                    .map(|(i, _)| i)
                else {
                    return;
                };
                let at = first.pts[idx].0;
                let x = x_of(at);
                p.line_segment(
                    [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
                    Stroke::new(1.0, theme::text_dim()),
                );
                let mut lines = vec![tick_label(at, Timeframe::Week)];
                for s in series {
                    if let Some((_, v)) = s.pts.iter().find(|p| p.0 == at) {
                        if v.is_finite() {
                            p.circle_filled(Pos2::new(x, y_of(*v)), 3.5, s.color);
                            lines.push(format!("{}: {}", s.label, fmt(*v)));
                        } else {
                            lines.push(format!("{}: –", s.label));
                        }
                    }
                }
                resp.on_hover_ui_at_pointer(|ui| {
                    for (i, l) in lines.iter().enumerate() {
                        let text = RichText::new(l).size(12.5);
                        ui.label(if i == 0 {
                            text.color(theme::text_dim())
                        } else {
                            text
                        });
                    }
                });
            });
    });
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_units() {
        assert_eq!(nice_ceil(0.0), 1.0);
        assert_eq!(nice_ceil(3.2), 5.0);
        assert_eq!(nice_ceil(23.0), 25.0);
        assert_eq!(nice_ceil(100.0), 100.0);
        assert_eq!(nice_ceil(101.0), 200.0);
        assert_eq!(
            nice_ceil_bytes(3.0 * 1024.0 * 1024.0 * 1024.0),
            4.0 * 1024.0 * 1024.0 * 1024.0
        );
        assert_eq!(nice_ceil_bytes(1000.0), 1024.0);
        assert_eq!(fmt_bytes(512.0), "512 B");
        assert_eq!(fmt_bytes(1536.0), "1.5 KiB");
        assert_eq!(fmt_bytes(8.0 * 1024.0 * 1024.0 * 1024.0), "8.0 GiB");
        assert_eq!(fmt_rate(2048.0), "2.0 KiB/s");
        assert_eq!(fmt_pct(4.25), "4.2 %");
        assert_eq!(fmt_pct(42.0), "42 %");
    }
}
