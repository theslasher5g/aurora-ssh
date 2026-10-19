//! Line icons, drawn from SVG path data on a 24×24 grid so they stay crisp at any size
//! and take the theme colour.

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2, epaint::PathShape};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Hosts,
    Key,
    Folder,
    Bolt,
    Tunnel,
    Gear,
    Sun,
    Help,
    Search,
    Wifi,
    Plus,
    Chevron,
    Star,
    Sparkle,
    Close,
    Split,
    Maximize,
    Exit,
    External,
    Keyboard,
    Expand,
    Monitor,
    Up,
    Refresh,
    File,
    Lock,
    Broadcast,
    Download,
    Terminal,
    Windows,
    Upload,
    Trash,
    Copy,
    Wrap,
    Pause,
    Play,
    Stop,
    Logs,
    Record,
    Server,
    /// Clock with a back arrow: time travel / history.
    Clock,
    /// Certificate radar, host-key checks, SSH CA.
    Shield,
    /// Route through jump hosts.
    Route,
    /// Wake-on-LAN.
    Power,
    /// Runbook: list with check marks.
    Checklist,
    /// Maintenance window.
    Wrench,
    Down,
    /// Diagnostics bundle: heartbeat line.
    Pulse,
    /// Container (cube).
    Cube,
    /// Table of servers.
    Table,
    /// Session marker.
    Flag,
}

enum Prim {
    Path(&'static str),
    Rect(f32, f32, f32, f32, f32),
    Circle(f32, f32, f32),
}

use Prim::{Circle, Path, Rect as R};

fn prims(icon: Icon) -> &'static [Prim] {
    match icon {
        Icon::Hosts => &[
            R(9.0, 3.0, 6.0, 5.0, 1.2),
            R(3.0, 16.0, 6.0, 5.0, 1.2),
            R(15.0, 16.0, 6.0, 5.0, 1.2),
            Path("M12 8v4M6 16v-4h12v4"),
        ],
        Icon::Key => &[Circle(8.0, 15.0, 4.0), Path("M11 12l9-9M16 7l3 3M14 9l2 2")],
        Icon::Folder => &[Path(
            "M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
        )],
        Icon::Bolt => &[Path("M13 3L5 13.5h6L10 21l8-10.5h-6z")],
        Icon::Tunnel => &[Path("M4 8h14M14 4l4 4-4 4M20 16H6M10 12l-4 4 4 4")],
        Icon::Gear => &[
            Circle(12.0, 12.0, 3.0),
            Path(
                "M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z",
            ),
        ],
        Icon::Sun => &[
            Circle(12.0, 12.0, 3.0),
            Path(
                "M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M18.4 5.6l-2.1 2.1M7.7 16.3l-2.1 2.1",
            ),
        ],
        Icon::Help => &[
            Circle(12.0, 12.0, 9.0),
            Path("M9.5 9.5a2.5 2.5 0 1 1 3.6 2.2c-.7.4-1.1 1-1.1 1.8M12 17h.01"),
        ],
        Icon::Search => &[Circle(11.0, 11.0, 7.0), Path("M20 20l-3.5-3.5")],
        Icon::Wifi => &[Path(
            "M5 12.5a10 10 0 0 1 14 0M8.5 16a5 5 0 0 1 7 0M12 19.5h.01",
        )],
        Icon::Plus => &[Path("M12 5v14M5 12h14")],
        Icon::Chevron => &[Path("M6 9l6 6 6-6")],
        Icon::Star => &[Path(
            "M12 2.8l2.9 6 6.6.9-4.8 4.6 1.2 6.5-5.9-3.2-5.9 3.2 1.2-6.5L2.5 9.7l6.6-.9z",
        )],
        Icon::Sparkle => &[Path(
            "M12 3l1.8 4.6L18.5 9l-4.7 1.4L12 15l-1.8-4.6L5.5 9l4.7-1.4zM18 15l.8 2.2L21 18l-2.2.8L18 21l-.8-2.2L15 18l2.2-.8z",
        )],
        Icon::Close => &[Path("M6 6l12 12M18 6L6 18")],
        Icon::Split => &[R(3.0, 4.0, 18.0, 16.0, 3.0), Path("M12 4v16")],
        Icon::Maximize => &[Path("M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5")],
        Icon::Exit => &[Path("M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5")],
        Icon::External => &[Path(
            "M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4",
        )],
        Icon::Keyboard => &[
            R(3.0, 6.0, 18.0, 12.0, 2.5),
            Path("M7 10h.01M11 10h.01M15 10h.01M7 14h10"),
        ],
        Icon::Expand => &[Path("M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7")],
        Icon::Monitor => &[R(3.0, 4.0, 18.0, 12.0, 2.5), Path("M8 20h8M12 16v4")],
        Icon::Up => &[Path("M12 19V6M6 11l6-6 6 6")],
        Icon::Refresh => &[Path("M20 12a8 8 0 1 1-2.5-5.8M20 4v5h-5")],
        Icon::File => &[
            Path("M6 3h8l5 5v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"),
            Path("M14 3v5h5"),
        ],
        Icon::Lock => &[
            R(5.0, 11.0, 14.0, 10.0, 2.5),
            Path("M8 11V8a4 4 0 0 1 8 0v3"),
        ],
        Icon::Broadcast => &[
            Circle(12.0, 12.0, 2.0),
            Path(
                "M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M5 5a10 10 0 0 0 0 14M19 5a10 10 0 0 1 0 14",
            ),
        ],
        Icon::Download => &[Path("M12 4v12M6 11l6 6 6-6M5 20h14")],
        Icon::Terminal => &[R(3.0, 4.0, 18.0, 16.0, 3.0), Path("M7 9l3 3-3 3M12 15h5")],
        Icon::Windows => &[
            R(4.0, 4.0, 7.0, 7.0, 1.2),
            R(13.0, 4.0, 7.0, 7.0, 1.2),
            R(4.0, 13.0, 7.0, 7.0, 1.2),
            R(13.0, 13.0, 7.0, 7.0, 1.2),
        ],
        Icon::Upload => &[Path("M12 16V4M7 9l5-5 5 5M5 20h14")],
        Icon::Trash => &[Path("M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6")],
        Icon::Copy => &[
            R(9.0, 9.0, 11.0, 11.0, 2.0),
            Path("M15 9V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h3"),
        ],
        Icon::Wrap => &[Path(
            "M4 6h16M4 12h13a3 3 0 0 1 0 6h-4M15 15l-3 3 3 3M4 18h5",
        )],
        Icon::Pause => &[Path("M9 5v14M15 5v14")],
        Icon::Play => &[Path("M7 4l13 8-13 8z")],
        Icon::Stop => &[R(6.0, 6.0, 12.0, 12.0, 2.0)],
        Icon::Logs => &[R(4.0, 4.0, 16.0, 16.0, 2.5), Path("M8 9h8M8 12h5M8 15h7")],
        Icon::Record => &[Circle(12.0, 12.0, 6.0)],
        Icon::Server => &[
            R(3.0, 3.5, 18.0, 7.0, 2.0),
            R(3.0, 13.5, 18.0, 7.0, 2.0),
            Path("M7 7h2M7 17h2M13 7h4M13 17h4"),
        ],
        Icon::Shield => &[
            Path("M12 3l7.5 3v5.5c0 4.6-3.2 8.2-7.5 9.5-4.3-1.3-7.5-4.9-7.5-9.5V6z"),
            Path("M9 12l2.2 2.2L15.5 10"),
        ],
        Icon::Clock => &[
            Path("M3 12a9 9 0 1 0 9-9a9.75 9.75 0 0 0-6.74 2.74L3 8"),
            Path("M3 3v5h5M12 7v5l4 2"),
        ],
        Icon::Route => &[
            Circle(6.0, 18.0, 2.2),
            Circle(18.0, 6.0, 2.2),
            Path("M8.2 18H15a3 3 0 0 0 0-6H9a3 3 0 0 1 0-6h6.8"),
        ],
        Icon::Power => &[Path("M12 3v8"), Path("M7.1 6.6A7 7 0 1 0 16.9 6.6")],
        Icon::Checklist => &[Path(
            "M4 6.5l1.5 1.5 3-3M4 12.5l1.5 1.5 3-3M4 18.5l1.5 1.5 3-3M12 7h8M12 13h8M12 19h8",
        )],
        Icon::Wrench => &[Path(
            "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.9 6.9a2.1 2.1 0 0 1-3-3l6.9-6.9a6 6 0 0 1 7.9-7.9z",
        )],
        Icon::Down => &[Path("M12 5v14M6 13l6 6 6-6")],
        Icon::Pulse => &[Path("M3 12h4l3-8 4 16 3-8h4")],
        Icon::Cube => &[
            Path("M12 3l8 4.5v9L12 21l-8-4.5v-9z"),
            Path("M4 7.5l8 4.5 8-4.5M12 12v9"),
        ],
        Icon::Table => &[
            R(3.0, 4.0, 18.0, 16.0, 2.0),
            Path("M3 10h18M3 15h18M9 10v10"),
        ],
        Icon::Flag => &[Path("M5 21V4M5 5h12l-2.5 4L17 13H5")],
    }
}

/// Filled shapes (the favourite star is solid).
fn filled(icon: Icon) -> bool {
    matches!(icon, Icon::Star | Icon::Play | Icon::Record)
}

/// Default stroke width on the 24 grid, as in the design.
fn weight(icon: Icon) -> f32 {
    match icon {
        Icon::Plus | Icon::Close => 2.6,
        Icon::Chevron => 3.0,
        Icon::Search
        | Icon::Wifi
        | Icon::Up
        | Icon::Down
        | Icon::Refresh
        | Icon::Download
        | Icon::Upload => 2.0,
        Icon::Pause => 2.4,
        Icon::Monitor => 1.4,
        Icon::File => 1.6,
        _ => 1.8,
    }
}

/// Paints `icon` into `rect` (square) in `color`.
pub fn paint(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    paint_weighted(p, rect, icon, color, weight(icon));
}

pub fn paint_weighted(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32, w: f32) {
    let scale = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - Vec2::splat(12.0 * scale);
    let map = |x: f32, y: f32| o + Vec2::new(x, y) * scale;
    // Thin strokes look washed out when scaled down; keep at least ~1.2 px.
    let stroke = Stroke::new((w * scale).max(1.2), color);
    let fill = filled(icon);
    for prim in prims(icon) {
        let subpaths: Vec<(Vec<Pos2>, bool)> = match prim {
            Path(d) => parse(d)
                .into_iter()
                .map(|(pts, closed)| (pts.into_iter().map(|(x, y)| map(x, y)).collect(), closed))
                .collect(),
            R(x, y, w, h, r) => vec![(
                round_rect(*x, *y, *w, *h, *r)
                    .into_iter()
                    .map(|(x, y)| map(x, y))
                    .collect(),
                true,
            )],
            Circle(cx, cy, r) => {
                let n = 40;
                vec![(
                    (0..n)
                        .map(|i| {
                            let a = i as f32 / n as f32 * std::f32::consts::TAU;
                            map(cx + r * a.cos(), cy + r * a.sin())
                        })
                        .collect(),
                    true,
                )]
            }
        };
        for (pts, closed) in subpaths {
            if pts.len() == 1 || (pts.len() == 2 && pts[0].distance(pts[1]) < 0.05 * scale) {
                // "h.01" dots.
                p.circle_filled(pts[0], stroke.width * 0.6, color);
                continue;
            }
            if fill && closed {
                p.add(Shape::convex_polygon(pts.clone(), color, Stroke::NONE));
                // Concave star: draw the outline too so the points are covered.
                p.add(Shape::Path(PathShape::closed_line(
                    pts,
                    Stroke::new(stroke.width * 0.5, color),
                )));
                continue;
            }
            for w in pts.windows(2) {
                // Round joins: a small dot at each vertex hides seams between segments.
                p.circle_filled(w[0], stroke.width / 2.0, color);
            }
            if closed {
                p.add(Shape::Path(PathShape::closed_line(pts, stroke)));
            } else {
                if let Some(last) = pts.last() {
                    p.circle_filled(*last, stroke.width / 2.0, color);
                }
                p.add(Shape::Path(PathShape::line(pts, stroke)));
            }
        }
    }
}

/// Allocates `size`×`size` and paints the icon.
pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui.painter(), rect, icon, color);
    resp
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    let corners = [
        (x + w - r, y + r, -90.0f32),
        (x + w - r, y + h - r, 0.0),
        (x + r, y + h - r, 90.0),
        (x + r, y + r, 180.0),
    ];
    for (cx, cy, start) in corners {
        for i in 0..=6 {
            let a = (start + i as f32 * 15.0).to_radians();
            v.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    v
}

/// Parses SVG path data (M L H V C A Z, absolute and relative) into polylines.
fn parse(d: &str) -> Vec<(Vec<(f32, f32)>, bool)> {
    let toks = tokenize(d);
    let mut out: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    let mut i = 0;
    let mut cmd = 'M';
    let num = |i: &mut usize| -> f32 {
        match toks.get(*i) {
            Some(Tok::Num(n)) => {
                *i += 1;
                *n
            }
            _ => {
                *i += 1;
                0.0
            }
        }
    };
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), true));
                }
                x = sx;
                y = sy;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), false));
                }
                x = bx + num(&mut i);
                y = by + num(&mut i);
                sx = x;
                sy = y;
                cur.push((x, y));
                // Further pairs after M are implicit L.
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                x = bx + num(&mut i);
                y = by + num(&mut i);
                cur.push((x, y));
            }
            'H' => {
                x = if rel { x } else { 0.0 } + num(&mut i);
                cur.push((x, y));
            }
            'V' => {
                y = if rel { y } else { 0.0 } + num(&mut i);
                cur.push((x, y));
            }
            'C' => {
                let (x1, y1) = (bx + num(&mut i), by + num(&mut i));
                let (x2, y2) = (bx + num(&mut i), by + num(&mut i));
                let (ex, ey) = (bx + num(&mut i), by + num(&mut i));
                for k in 1..=10 {
                    let t = k as f32 / 10.0;
                    let mt = 1.0 - t;
                    let px = mt * mt * mt * x
                        + 3.0 * mt * mt * t * x1
                        + 3.0 * mt * t * t * x2
                        + t * t * t * ex;
                    let py = mt * mt * mt * y
                        + 3.0 * mt * mt * t * y1
                        + 3.0 * mt * t * t * y2
                        + t * t * t * ey;
                    cur.push((px, py));
                }
                x = ex;
                y = ey;
            }
            'A' => {
                let rx = num(&mut i);
                let ry = num(&mut i);
                let rot = num(&mut i);
                let large = num(&mut i) != 0.0;
                let sweep = num(&mut i) != 0.0;
                let (ex, ey) = (bx + num(&mut i), by + num(&mut i));
                arc(&mut cur, (x, y), (ex, ey), rx, ry, rot, large, sweep);
                x = ex;
                y = ey;
            }
            _ => {
                i += 1;
            }
        }
    }
    if !cur.is_empty() {
        out.push((cur, false));
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Tok {
    Cmd(char),
    Num(f32),
}

fn tokenize(d: &str) -> Vec<Tok> {
    let mut v = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    // Arc flags may be written without separators ("0 1 1" or "011"); the parser reads
    // them as numbers, so split single 0/1 digits after the third arc argument.
    let mut arc_arg: Option<usize> = None;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() {
            v.push(Tok::Cmd(c));
            arc_arg = matches!(c, 'A' | 'a').then_some(0);
            i += 1;
        } else if c == '-' || c == '.' || c.is_ascii_digit() {
            let start = i;
            if let Some(k) = arc_arg
                && (k % 7 == 3 || k % 7 == 4)
                && (c == '0' || c == '1')
            {
                v.push(Tok::Num(if c == '1' { 1.0 } else { 0.0 }));
                arc_arg = Some(k + 1);
                i += 1;
                continue;
            }
            i += 1;
            let mut dot = c == '.';
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !dot {
                    dot = true;
                    i += 1;
                } else if (ch == 'e' || ch == 'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            v.push(Tok::Num(d[start..i].parse().unwrap_or(0.0)));
            if let Some(k) = arc_arg {
                arc_arg = Some(k + 1);
            }
        } else {
            i += 1;
        }
    }
    v
}

/// SVG endpoint arc → points (F.6.5 of the SVG spec).
#[allow(clippy::too_many_arguments)]
fn arc(
    out: &mut Vec<(f32, f32)>,
    (x1, y1): (f32, f32),
    (x2, y2): (f32, f32),
    rx: f32,
    ry: f32,
    rot: f32,
    large: bool,
    sweep: bool,
) {
    if rx == 0.0 || ry == 0.0 || (x1 == x2 && y1 == y2) {
        out.push((x2, y2));
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let phi = rot.to_radians();
    let (cp, sp) = (phi.cos(), phi.sin());
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cp * dx + sp * dy;
    let y1p = -sp * dx + cp * dy;
    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        rx *= lam.sqrt();
        ry *= lam.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut co = (num / den).max(0.0).sqrt();
    if large == sweep {
        co = -co;
    }
    let cxp = co * rx * y1p / ry;
    let cyp = -co * ry * x1p / rx;
    let cx = cp * cxp - sp * cyp + (x1 + x2) / 2.0;
    let cy = sp * cxp + cp * cyp + (y1 + y2) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let t1 = ang(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dt = ang(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && dt > 0.0 {
        dt -= std::f32::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f32::consts::TAU;
    }
    let n = ((dt.abs() / std::f32::consts::TAU) * 32.0).ceil().max(2.0) as usize;
    for k in 1..=n {
        let t = t1 + dt * k as f32 / n as f32;
        let (ct, st) = (t.cos(), t.sin());
        out.push((
            cx + rx * cp * ct - ry * sp * st,
            cy + rx * sp * ct + ry * cp * st,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_lines() {
        let p = parse("M12 8v4M6 16v-4h12v4");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].0, vec![(12.0, 8.0), (12.0, 12.0)]);
        assert_eq!(
            p[1].0,
            vec![(6.0, 16.0), (6.0, 12.0), (18.0, 12.0), (18.0, 16.0)]
        );
    }

    #[test]
    fn parses_arc_and_close() {
        let p = parse("M3 7a2 2 0 0 1 2-2h4z");
        assert_eq!(p.len(), 1);
        assert!(p[0].1);
        let last_arc = p[0]
            .0
            .iter()
            .find(|(x, y)| (*x - 5.0).abs() < 1e-3 && (*y - 5.0).abs() < 1e-3);
        assert!(last_arc.is_some());
        // The arc bulges to the top-left (corner of a rounded rect).
        assert!(p[0].0.iter().all(|(x, y)| *x >= 2.99 && *y >= 4.99));
    }

    #[test]
    fn compact_arc_flags() {
        let p = parse("M0 0a2 2 0 1 1 4 0");
        assert!((p[0].0.last().unwrap().0 - 4.0).abs() < 1e-3);
        let q = parse("M0 0a2 2 0 114 0");
        assert!((q[0].0.last().unwrap().0 - 4.0).abs() < 1e-3);
    }

    #[test]
    fn every_icon_parses() {
        use Icon::*;
        for icon in [
            Hosts, Key, Folder, Bolt, Tunnel, Gear, Sun, Help, Search, Wifi, Plus, Chevron, Star,
            Sparkle, Close, Split, Maximize, Exit, External, Keyboard, Expand, Monitor, Up,
            Refresh, File, Lock, Broadcast, Download, Terminal, Windows, Upload, Trash, Copy, Wrap,
            Pause, Play, Stop, Server, Logs, Record, Clock, Shield, Route, Power, Checklist,
            Wrench, Down, Pulse, Cube, Table,
        ] {
            for prim in prims(icon) {
                if let Prim::Path(d) = prim {
                    for (pts, _) in parse(d) {
                        assert!(!pts.is_empty());
                        assert!(pts.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
                        assert!(
                            pts.iter().all(
                                |(x, y)| (-1.0..=25.0).contains(x) && (-1.0..=25.0).contains(y)
                            ),
                            "{icon:?}"
                        );
                    }
                }
            }
        }
    }
}
