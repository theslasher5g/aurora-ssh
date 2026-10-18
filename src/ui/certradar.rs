//! Certificate radar: TLS certificates of all hosts, sorted by days left (red < 7 days
//! or expired, amber < 30), with a daily background check and a toast when some expire.

use std::time::{Duration, Instant};

use chrono::Utc;
use egui::{CornerRadius, Pos2, Rect, RichText, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::App;
use crate::backend::Sink;
use crate::backend::certradar::{self, CertInfo, RadarEvent, Target};
use crate::model::Library;
use crate::theme;
use crate::ui::icons::Icon;

const DAILY: Duration = Duration::from_secs(24 * 3600);
/// The first background check waits a bit after unlocking.
const FIRST_DELAY: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Radar {
    pub open: bool,
    running: bool,
    background: bool,
    progress: (usize, usize),
    found: Vec<CertInfo>,
    rx: Option<UnboundedReceiver<RadarEvent>>,
    filter: String,
    /// 0 = all, 1 = only problems.
    view: usize,
    since: Option<Instant>,
}

/// Certificates that expire within 30 days (or already did).
pub fn warnings(lib: &Library) -> usize {
    let now = Utc::now();
    lib.net
        .certs
        .iter()
        .filter(|c| c.days_left(now) < certradar::AMBER_DAYS)
        .count()
}

/// Header button of the host list; a dot when certificates expire soon.
pub fn header_button(app: &App, ui: &mut egui::Ui) -> egui::Response {
    let n = warnings(&app.lib);
    let tip = if n > 0 {
        t!("Zertifikats-Radar – {} laufen bald ab", n)
    } else {
        t!("Zertifikats-Radar").to_string()
    };
    let resp = theme::icon_btn(ui, Icon::Shield, &tip);
    if n > 0 {
        let c = Pos2::new(resp.rect.right() - 7.0, resp.rect.top() + 7.0);
        let now = Utc::now();
        let red = app
            .lib
            .net
            .certs
            .iter()
            .any(|c| c.days_left(now) < certradar::RED_DAYS);
        ui.painter()
            .circle_filled(c, 4.5, if red { theme::red() } else { theme::amber() });
    }
    resp
}

/// What to check: every network host (directly reachable, i.e. without jump host) on the
/// TLS ports of its last port scan or the default list, plus the Proxmox servers.
fn targets(lib: &Library) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    let mut push = |t: Target| {
        if !out
            .iter()
            .any(|o| o.address == t.address && o.port == t.port)
        {
            out.push(t);
        }
    };
    for h in &lib.hosts {
        let address = h.hostname.trim();
        if !h.protocol.is_network() || address.is_empty() || h.jump_host.is_some() {
            continue;
        }
        for port in certradar::ports_for(lib.net.open_ports.get(&h.id)) {
            push(Target {
                host_id: Some(h.id),
                name: h.display_name().to_string(),
                address: address.to_string(),
                port,
            });
        }
    }
    for s in &lib.pve_servers {
        if !s.host.trim().is_empty() {
            push(Target {
                host_id: None,
                name: s.name.clone(),
                address: s.host.trim().to_string(),
                port: s.port,
            });
        }
    }
    out
}

fn start(app: &mut App, ctx: &egui::Context, background: bool) {
    let r = &mut app.net.radar;
    if r.running {
        return;
    }
    let targets = targets(&app.lib);
    let (sink, rx) = Sink::new(ctx.clone());
    r.rx = Some(rx);
    r.running = true;
    r.background = background;
    r.found.clear();
    r.progress = (0, targets.len());
    certradar::spawn_scan(&app.rt, targets, sink);
}

pub fn open(app: &mut App, ctx: &egui::Context) {
    app.net.radar.open = true;
    if app.lib.net.certs_checked.is_none() {
        start(app, ctx, false);
    }
}

/// "2 Zertifikate laufen in den nächsten 14 Tagen ab" after a background run.
fn summary(certs: &[CertInfo]) -> Option<(String, egui::Color32)> {
    let now = Utc::now();
    let expired = certs.iter().filter(|c| c.days_left(now) < 0).count();
    let soon: Vec<i64> = certs
        .iter()
        .map(|c| c.days_left(now))
        .filter(|d| (0..certradar::AMBER_DAYS).contains(d))
        .collect();
    let mut parts = Vec::new();
    if expired == 1 {
        parts.push(t!("1 Zertifikat ist abgelaufen").to_string());
    } else if expired > 1 {
        parts.push(t!("{} Zertifikate sind abgelaufen", expired));
    }
    let max = soon.iter().copied().max().unwrap_or(0);
    if soon.len() == 1 {
        parts.push(t!("1 Zertifikat läuft in {} Tagen ab", max));
    } else if soon.len() > 1 {
        parts.push(t!(
            "{} Zertifikate laufen in den nächsten {} Tagen ab",
            soon.len(),
            max
        ));
    }
    if parts.is_empty() {
        return None;
    }
    let color = if expired > 0 || soon.iter().any(|d| *d < certradar::RED_DAYS) {
        theme::red()
    } else {
        theme::amber()
    };
    Some((
        format!("{} – {}", parts.join(", "), t!("siehe Zertifikats-Radar")),
        color,
    ))
}

fn pump(app: &mut App, ctx: &egui::Context) {
    let r = &mut app.net.radar;
    let mut done = false;
    while let Some(ev) = r.rx.as_mut().and_then(|rx| rx.try_recv().ok()) {
        match ev {
            RadarEvent::Progress(d, t) => r.progress = (d, t),
            RadarEvent::Found(c) => r.found.push(c),
            RadarEvent::Done => done = true,
        }
    }
    if done {
        r.running = false;
        r.rx = None;
        let now = Utc::now();
        let mut certs = std::mem::take(&mut r.found);
        certs.sort_by_key(|c| (c.days_left(now), c.host_name.clone(), c.port));
        let background = r.background;
        app.lib.net.certs = certs;
        app.lib.net.certs_checked = Some(now);
        app.save_library();
        if background && let Some((text, color)) = summary(&app.lib.net.certs) {
            app.toast(text, color);
        }
    }
    // Daily check while the app runs.
    let since = *app.net.radar.since.get_or_insert_with(Instant::now);
    let due = app
        .lib
        .net
        .certs_checked
        .is_none_or(|t| (Utc::now() - t).to_std().is_ok_and(|d| d >= DAILY));
    if !app.lib.net.radar_paused && !app.net.radar.running && due {
        if since.elapsed() >= FIRST_DELAY {
            start(app, ctx, true);
        } else {
            ctx.request_repaint_after(FIRST_DELAY.saturating_sub(since.elapsed()));
        }
    } else if !app.lib.net.radar_paused {
        ctx.request_repaint_after(Duration::from_secs(600));
    }
}

fn matches(c: &CertInfo, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    [&c.host_name, &c.address, &c.subject, &c.issuer]
        .iter()
        .any(|s| s.to_lowercase().contains(&q))
        || c.sans.iter().any(|s| s.to_lowercase().contains(&q))
        || c.port.to_string() == q
}

fn severity_color(days: i64) -> egui::Color32 {
    match certradar::severity(days) {
        2 => theme::red(),
        1 => theme::amber(),
        _ => theme::green(),
    }
}

fn cert_row(ui: &mut egui::Ui, c: &CertInfo, now: chrono::DateTime<Utc>) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), egui::Sense::hover());
    let p = ui.painter();
    let days = c.days_left(now);
    let color = severity_color(days);
    // Days badge.
    let badge = Rect::from_min_size(
        Pos2::new(rect.left() + 4.0, rect.center().y - 20.0),
        Vec2::new(58.0, 40.0),
    );
    p.rect_filled(badge, CornerRadius::same(9), theme::soft(color));
    if days < 0 {
        p.text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            t!("abgel."),
            theme::semibold(12.5),
            color,
        );
    } else {
        p.text(
            Pos2::new(badge.center().x, badge.top() + 15.0),
            egui::Align2::CENTER_CENTER,
            days.min(9999).to_string(),
            theme::bold(16.0),
            color,
        );
        p.text(
            Pos2::new(badge.center().x, badge.bottom() - 9.0),
            egui::Align2::CENTER_CENTER,
            if days == 1 { t!("Tag") } else { t!("Tage") },
            theme::medium(10.5),
            color,
        );
    }
    let x = badge.right() + 12.0;
    let right = rect.right() - 6.0;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(x, rect.top() + 6.0),
                Pos2::new(right, rect.bottom() - 6.0),
            ))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    ui2.spacing_mut().item_spacing.y = 3.0;
    ui2.horizontal(|ui| {
        ui.label(
            RichText::new(&c.host_name)
                .font(theme::semibold(14.0))
                .color(theme::text()),
        );
        ui.label(theme::mono(format!("{}:{}", c.address, c.port), 12.0).color(theme::text_dim()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(t) = c.not_after {
                ui.label(
                    RichText::new(t!("bis {}", t.format("%d.%m.%Y")))
                        .size(12.0)
                        .color(theme::text_dim()),
                );
            }
        });
    });
    ui2.horizontal(|ui| {
        let issuer = if c.self_signed {
            String::new()
        } else {
            format!(" · {}", t!("von {}", c.issuer))
        };
        ui.add(
            egui::Label::new(
                RichText::new(format!("{}{issuer}", c.subject))
                    .size(12.5)
                    .color(theme::text_dim()),
            )
            .truncate(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !c.name_ok {
                theme::tag(ui, t!("Name passt nicht"), Some(theme::red())).on_hover_text(t!(
                    "„{}“ steht weder im Zertifikat (SAN) noch im Common Name",
                    c.address
                ));
            }
            if c.self_signed {
                theme::tag(ui, t!("selbst signiert"), Some(theme::amber()));
            }
        });
    });
    let mut tip = format!("CN: {}\n{}: {}", c.subject, t!("Aussteller"), c.issuer);
    if !c.sans.is_empty() {
        tip.push_str(&format!("\nSAN: {}", c.sans.join(", ")));
    }
    if let (Some(a), Some(b)) = (c.not_before, c.not_after) {
        tip.push_str(&t!(
            "\nGültig: {} – {}",
            a.format("%d.%m.%Y"),
            b.format("%d.%m.%Y")
        ));
    }
    tip.push_str(&t!("\nKette: {} Zertifikat(e)", c.chain_len));
    resp.on_hover_text(tip);
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    pump(app, ctx);
    if !app.net.radar.open {
        return;
    }
    let now = Utc::now();
    let lib = &mut app.lib;
    let r = &mut app.net.radar;
    let running = r.running;
    // While a manual run is going, show what's been found so far.
    let list: Vec<CertInfo> = if running && !r.background {
        let mut v = r.found.clone();
        v.sort_by_key(|c| c.days_left(now));
        v
    } else {
        lib.net.certs.clone()
    };
    let mut refresh = false;
    let mut daily = !lib.net.radar_paused;
    let ((), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("cert-radar"),
        640.0,
        t!("Zertifikats-Radar"),
        |ui| {
            let problems = list
                .iter()
                .filter(|c| c.days_left(now) < certradar::AMBER_DAYS)
                .count();
            let mut status = t!("{} Zertifikate", list.len());
            if problems > 0 {
                status.push_str(&format!(" · {}", t!("{} laufen bald ab", problems)));
            }
            if let Some(t) = lib.net.certs_checked.filter(|_| !running) {
                status.push_str(&format!(
                    " · {}",
                    t!("Stand: {}", crate::ui::relative_time(t))
                ));
            }
            ui.label(RichText::new(status).size(13.0).color(theme::text_dim()));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(i) = theme::segmented(ui, &[t!("Alle"), t!("Läuft bald ab")], r.view) {
                    r.view = i;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::search_field(
                        ui,
                        &mut r.filter,
                        t!("Filtern"),
                        220.0,
                        egui::Id::new("radar-filter"),
                    );
                });
            });
            if running {
                let (d, t) = r.progress;
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t!("{} von {} Ports geprüft", d, t));
                });
                ui.add(egui::ProgressBar::new(d as f32 / t.max(1) as f32).desired_height(4.0));
            }
            let q = r.filter.trim().to_string();
            let shown: Vec<&CertInfo> = list
                .iter()
                .filter(|c| r.view == 0 || c.days_left(now) < certradar::AMBER_DAYS)
                .filter(|c| matches(c, &q))
                .collect();
            if shown.is_empty() && !running {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(if list.is_empty() {
                        t!("Keine TLS-Zertifikate gefunden. Geprüft werden die Ports aus dem letzten Port-Scan eines Hosts, sonst 443, 8443, 8006, 9443, 636, 993, 995, 465 und 587.")
                    } else {
                        t!("Nichts passt zum Filter")
                    })
                    .color(theme::text_dim()),
                );
            }
            for c in shown {
                cert_row(ui, c, now);
            }
        },
        |ui, ()| {
            if ui
                .add_enabled_ui(!running, |ui| {
                    theme::btn(
                        ui,
                        theme::Btn::Primary,
                        Some(Icon::Refresh),
                        if running {
                            t!("Läuft…")
                        } else {
                            t!("Aktualisieren")
                        },
                    )
                })
                .inner
                .clicked()
            {
                refresh = true;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                theme::toggle(ui, &mut daily);
                ui.label(
                    RichText::new(t!("Täglich im Hintergrund prüfen"))
                        .size(13.0)
                        .color(theme::text_dim()),
                );
            });
        },
    );
    if daily == lib.net.radar_paused {
        lib.net.radar_paused = !daily;
        app.save_library();
    }
    if dismissed {
        app.net.radar.open = false;
    }
    if refresh {
        start(app, ctx, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_texts() {
        crate::i18n::set_language("de");
        let now = Utc::now();
        let cert = |days: i64| CertInfo {
            not_after: Some(now + chrono::Duration::days(days) + chrono::Duration::hours(1)),
            ..Default::default()
        };
        assert!(summary(&[cert(90)]).is_none());
        let (text, _) = summary(&[cert(10), cert(14), cert(200)]).unwrap();
        assert!(text.starts_with("2 Zertifikate laufen in den nächsten 14 Tagen ab"));
        let (text, color) = summary(&[cert(-3)]).unwrap();
        assert!(text.starts_with("1 Zertifikat ist abgelaufen"));
        assert_eq!(color, theme::red());
    }
}
