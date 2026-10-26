//! Detail sheet of one inventory host: system facts, services, firewall and listening
//! ports as collected on the last refresh.

use chrono::{DateTime, Utc};
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::backend::inventory_detail::{Avail, Firewall, HostDetail};
use crate::backend::inventory_rules::{self as rules, InventorySettings, Level, Row};
use crate::model::{Env, HostFacts, Protocol};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::inventory_page::InvAction;

#[derive(Default)]
pub struct DetailState {
    pub host: Option<Uuid>,
    tab: usize,
    svc_text: String,
    svc_filter: usize,
    fw_text: String,
    port_text: String,
}

impl DetailState {
    pub fn open(&mut self, id: Uuid) {
        *self = DetailState {
            host: Some(id),
            ..Default::default()
        };
    }
}

pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some(id) = app.inv.detail.host else {
        return;
    };
    let now = Utc::now();
    let Some(row) = crate::ui::nav2_inv::detail_row(app, id, now) else {
        app.inv.detail.host = None;
        return;
    };
    let mut st = std::mem::take(&mut app.inv.detail);
    let busy = app.inv.run.as_ref().is_some_and(|r| r.busy(id));
    let facts = app.lib.facts.get(&id);
    let detail = app.lib.detail.get(&id);
    let settings = &app.lib.settings.inventory;
    let (footer, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("inv-detail"),
        800.0,
        &row.name,
        |ui| body(ui, &mut st, &row, facts, detail, settings, now),
        |ui, _| {
            let mut out = (false, false, false);
            if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                out.0 = true;
            }
            if theme::btn(ui, theme::Btn::Primary, None, t!("Verbinden")).clicked() {
                out.1 = true;
            }
            if row.protocol == Protocol::Ssh {
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::btn(
                        ui,
                        theme::Btn::Normal,
                        Some(Icon::Refresh),
                        t!("Fakten aktualisieren"),
                    )
                    .clicked()
                    {
                        out.2 = true;
                    }
                });
                if busy {
                    ui.spinner();
                }
            }
            out
        },
    );
    app.inv.detail = st;
    if footer.1 {
        actions.push(Action::Connect(id));
    }
    if footer.2 {
        actions.push(Action::Inv(InvAction::Refresh(vec![id])));
    }
    if dismissed || footer.0 || footer.1 {
        app.inv.detail.host = None;
    }
}

fn body(
    ui: &mut egui::Ui,
    st: &mut DetailState,
    row: &Row,
    facts: Option<&HostFacts>,
    d: Option<&HostDetail>,
    s: &InventorySettings,
    now: DateTime<Utc>,
) {
    let top = ui.cursor().top();
    let failed = d.map_or(0, |d| d.failed_count());
    let svc = match d {
        Some(d) if d.services_avail == Avail::Ok => {
            if failed > 0 {
                format!("{} ({}, {} ⚠)", t!("Dienste"), d.services_total, failed)
            } else {
                format!("{} ({})", t!("Dienste"), d.services_total)
            }
        }
        _ => t!("Dienste").to_string(),
    };
    let ports = match d {
        Some(d) if d.ports_avail == Avail::Ok => format!("{} ({})", t!("Ports"), d.ports.len()),
        _ => t!("Ports").to_string(),
    };
    let labels = [
        t!("System").to_string(),
        svc,
        t!("Firewall").to_string(),
        ports,
        match d {
            Some(d) if !d.info.is_empty() => format!("{} ({})", t!("Info"), d.info.len()),
            _ => t!("Info").to_string(),
        },
    ];
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    if let Some(i) = theme::segmented(ui, &refs, st.tab) {
        st.tab = i;
    }
    ui.add_space(6.0);
    match st.tab {
        0 => system_tab(ui, row, facts, d, s, now),
        1 => services_tab(ui, st, row, d),
        2 => firewall_tab(ui, st, row, d),
        3 => ports_tab(ui, st, row, d),
        _ => info_tab(ui, row, d, s),
    }
    // Same height on every tab, so the sheet does not jump around.
    let used = ui.cursor().top() - top;
    if used < 420.0 {
        ui.add_space(420.0 - used);
    }
}

// --- small helpers -----------------------------------------------------------------

fn dim(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::text_dim())).wrap());
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    theme::section_title(ui, text);
    ui.add_space(2.0);
}

fn props(ui: &mut egui::Ui, id: &str, items: &[(&str, String)]) {
    let items: Vec<&(&str, String)> = items.iter().filter(|(_, v)| !v.trim().is_empty()).collect();
    if items.is_empty() {
        return;
    }
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 4.0])
        .min_row_height(20.0)
        .show(ui, |ui| {
            for (k, v) in items {
                ui.label(RichText::new(*k).size(12.5).color(theme::text_dim()));
                ui.add(egui::Label::new(RichText::new(v).size(13.0)).wrap());
                ui.end_row();
            }
        });
}

fn avail_message(a: Avail) -> Option<String> {
    match a {
        Avail::Ok => None,
        Avail::NeedsRoot => Some(
            t!("Benötigt root: Aurora versucht „sudo -n“ und fragt nie nach einem Passwort. Mit einem sudo-Eintrag ohne Passwort (NOPASSWD) für diese Befehle oder einem root-Login lässt sich das auslesen.").to_string(),
        ),
        Avail::Missing => Some(t!("Auf dem Host nicht vorhanden (kein systemd, keine Firewall-Software oder kein ss/netstat).").to_string()),
        Avail::Failed => Some(t!("Konnte nicht gelesen werden (Zeitüberschreitung oder unerwartete Ausgabe).").to_string()),
        Avail::Unknown => Some(t!("Noch nicht erfasst – „Fakten aktualisieren“ klicken.").to_string()),
    }
}

fn no_data(ui: &mut egui::Ui, row: &Row, why: Option<String>) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !row.has_facts && row.protocol != Protocol::Ssh {
            ui.label(
                RichText::new(crate::backend::inventory_doc::no_facts_reason(row.protocol))
                    .font(theme::semibold(13.5)),
            );
        } else if let Some(w) = why {
            dim(ui, &w);
        } else {
            dim(
                ui,
                t!("Noch keine Details – „Fakten aktualisieren“ klicken."),
            );
        }
    });
}

#[derive(Clone, Copy)]
struct ColSpec {
    title: &'static str,
    /// Pixels; 0 = takes the rest.
    width: f32,
    mono: bool,
    /// Dropped when the sheet is narrow.
    optional: bool,
}

struct RowSpec {
    cells: Vec<(String, Color32)>,
    bg: Option<Color32>,
}

/// Header and rows painted by hand (cheap for hundreds of rows).
fn grid(ui: &mut egui::Ui, all: &[ColSpec], rows: &[RowSpec]) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        grid_inner(ui, all, rows);
    });
}

fn grid_inner(ui: &mut egui::Ui, all: &[ColSpec], rows: &[RowSpec]) {
    const H: f32 = 26.0;
    let avail = ui.available_width();
    let narrow = avail < 560.0;
    // Indices into the cell lists of the columns that fit.
    let kept: Vec<usize> = (0..all.len())
        .filter(|i| !(narrow && all[*i].optional))
        .collect();
    let fixed: f32 = kept.iter().map(|i| all[*i].width).sum();
    let flex_n = kept.iter().filter(|i| all[**i].width == 0.0).count().max(1) as f32;
    let flex = ((avail - fixed) / flex_n).max(60.0);
    let widths: Vec<f32> = kept
        .iter()
        .map(|i| {
            if all[*i].width == 0.0 {
                flex
            } else {
                all[*i].width
            }
        })
        .collect();
    let (head, _) = ui.allocate_exact_size(Vec2::new(avail, H), Sense::hover());
    let mut x = head.left() + 8.0;
    for (i, w) in kept.iter().zip(&widths) {
        let g = theme::truncated(
            ui.painter(),
            all[*i].title,
            theme::semibold(12.0),
            theme::text_dim(),
            w - 8.0,
        );
        ui.painter().galley(
            Pos2::new(x, head.center().y - g.size().y / 2.0),
            g,
            theme::text_dim(),
        );
        x += w;
    }
    ui.painter().line_segment(
        [head.left_bottom(), head.right_bottom()],
        egui::Stroke::new(1.0, theme::line()),
    );
    for r in rows {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail, H), Sense::hover());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let p = ui.painter();
        if let Some(bg) = r.bg {
            p.rect_filled(rect, CornerRadius::ZERO, bg);
        } else if resp.hovered() {
            p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
        }
        let mut x = rect.left() + 8.0;
        for (i, w) in kept.iter().zip(&widths) {
            if let Some((text, color)) = r.cells.get(*i) {
                let font = if all[*i].mono {
                    theme::mono_medium(12.0)
                } else {
                    egui::FontId::proportional(12.5)
                };
                let g = theme::truncated(p, text.clone(), font, *color, w - 8.0);
                p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, *color);
            }
            x += w;
        }
        if resp.hovered() {
            resp.on_hover_text(
                r.cells
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ·  "),
            );
        }
    }
}

fn search(ui: &mut egui::Ui, text: &mut String, hint: &str, id: &str) {
    let w = ui.available_width().min(360.0);
    theme::search_field(ui, text, hint, w, egui::Id::new(id));
}

fn matches(hay: &[&str], needle: &str) -> bool {
    let n = needle.to_lowercase();
    n.split_whitespace()
        .all(|w| hay.iter().any(|h| h.to_lowercase().contains(w)))
}

// --- System ------------------------------------------------------------------------

fn system_tab(
    ui: &mut egui::Ui,
    row: &Row,
    facts: Option<&HostFacts>,
    d: Option<&HostDetail>,
    s: &InventorySettings,
    now: DateTime<Utc>,
) {
    if !row.has_facts {
        no_data(ui, row, None);
        if row.protocol == Protocol::Rdp {
            ui.add_space(6.0);
            dim(
                ui,
                t!(
                    "RDP kennt keinen Befehlskanal. Mit einem SSH-Zugang (unter Windows der OpenSSH-Server) liest Aurora Betriebssystem, Dienste, Firewall und Ports ein: denselben Server zusätzlich als SSH-Host anlegen."
                ),
            );
        }
        caption(ui, t!("Bekannt aus dem Host-Eintrag"));
    } else {
        caption(ui, t!("Allgemein"));
    }
    let when = row.facts_at.map(|t| {
        format!(
            "{} ({})",
            t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
            t!("vor {} Tagen", (now - t).num_days())
        )
    });
    props(
        ui,
        "inv-d-general",
        &[
            (t!("Gruppe"), row.group.clone()),
            (
                t!("Umgebung"),
                if row.env == Env::None {
                    String::new()
                } else {
                    row.env.label().to_string()
                },
            ),
            (t!("Protokoll"), row.protocol.label().to_string()),
            (t!("Adresse"), row.address.clone()),
            (t!("Tags"), row.tags.join(", ")),
            (
                t!("Letzte Verbindung"),
                row.last_connected
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default(),
            ),
            (t!("Fakten vom"), when.unwrap_or_default()),
        ],
    );
    if !row.warnings.is_empty() {
        caption(ui, t!("Warnungen"));
        for w in &row.warnings {
            let c = if w.level == Level::Crit {
                theme::red()
            } else {
                theme::amber()
            };
            ui.add(
                egui::Label::new(RichText::new(format!("• {}", w.text)).size(12.5).color(c)).wrap(),
            );
        }
    }
    let Some(f) = facts else {
        return;
    };
    caption(ui, t!("Betriebssystem"));
    let n = |v: Option<u32>| v.map(|n| n.to_string()).unwrap_or_default();
    props(
        ui,
        "inv-d-os",
        &[
            (
                t!("System"),
                d.map(|d| d.os_caption.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| f.os_name.clone()),
            ),
            (
                t!("Version"),
                d.map(|d| d.os_version.clone()).unwrap_or_default(),
            ),
            (t!("Kernel / Build"), f.kernel.clone()),
            (
                t!("Architektur"),
                d.map(|d| d.arch.clone()).unwrap_or_default(),
            ),
            (t!("Rechnername"), f.hostname.clone()),
            (
                t!("Domäne / FQDN"),
                d.map(|d| d.domain.clone()).unwrap_or_default(),
            ),
            (
                t!("Laufzeit seit Neustart"),
                if f.uptime_secs > 0 {
                    rules::duration_short(f.uptime_secs)
                } else {
                    String::new()
                },
            ),
            (t!("Ausstehende Updates"), n(f.updates)),
            (t!("davon Sicherheits-Updates"), n(f.security_updates)),
            (
                t!("Angemeldete Benutzer"),
                d.map(|d| d.users.join(", ")).unwrap_or_default(),
            ),
        ],
    );
    caption(ui, t!("Hardware"));
    let ram = match (f.mem_total_mb, f.mem_used_pct) {
        (Some(mb), Some(p)) => format!("{} · {p} % {}", size(mb), t!("belegt")),
        (Some(mb), None) => size(mb),
        _ => String::new(),
    };
    props(
        ui,
        "inv-d-hw",
        &[
            (t!("Modell"), d.map(|d| d.model.clone()).unwrap_or_default()),
            (
                t!("Prozessor"),
                d.map(|d| d.cpu_model.clone()).unwrap_or_default(),
            ),
            (t!("CPU-Kerne"), n(f.cpus)),
            (t!("Arbeitsspeicher"), ram),
            (
                t!("Last (1 min)"),
                f.load1.map(|l| format!("{l:.2}")).unwrap_or_default(),
            ),
        ],
    );
    let Some(d) = d else {
        ui.add_space(8.0);
        dim(
            ui,
            t!(
                "Festplatten, Netzwerk, Dienste, Firewall und Ports fehlen noch – „Fakten aktualisieren“ klicken."
            ),
        );
        return;
    };
    if !d.disks.is_empty() {
        caption(ui, t!("Festplatten"));
        for k in &d.disks {
            disk_bar(
                ui,
                k.mount.as_str(),
                &k.label,
                &k.fs,
                k.total_mb,
                k.used_mb,
                k.used_pct,
                s,
            );
        }
    }
    if !d.addresses.is_empty() {
        caption(ui, t!("Netzwerk"));
        for a in &d.addresses {
            ui.label(theme::mono(
                if a.iface.is_empty() {
                    a.addr.clone()
                } else {
                    format!("{:<10} {}", a.iface, a.addr)
                },
                12.5,
            ));
        }
    }
}

fn size(mb: u64) -> String {
    if mb >= 1024 * 1024 {
        format!("{:.1} TB", mb as f64 / 1024.0 / 1024.0)
    } else if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

#[allow(clippy::too_many_arguments)]
fn disk_bar(
    ui: &mut egui::Ui,
    mount: &str,
    label: &str,
    fs: &str,
    total: u64,
    used: u64,
    pct: u8,
    s: &InventorySettings,
) {
    let color = if pct >= s.disk_crit {
        theme::red()
    } else if pct >= s.disk_warn {
        theme::amber()
    } else {
        theme::green()
    };
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::hover());
    let p = ui.painter();
    let left = format!(
        "{mount}  {}",
        [label, fs]
            .iter()
            .filter(|x| !x.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ")
    );
    let right = format!("{} / {} · {pct} %", size(used), size(total));
    let gr = p.layout_no_wrap(right, egui::FontId::proportional(12.5), color);
    let rw = gr.size().x;
    let gl = theme::truncated(
        p,
        left,
        egui::FontId::proportional(12.5),
        theme::text(),
        (w - rw - 16.0).max(40.0),
    );
    p.galley(Pos2::new(rect.left(), rect.top() + 2.0), gl, theme::text());
    p.galley(Pos2::new(rect.right() - rw, rect.top() + 2.0), gr, color);
    let bar = Rect::from_min_size(Pos2::new(rect.left(), rect.top() + 24.0), Vec2::new(w, 6.0));
    p.rect_filled(bar, CornerRadius::same(3), theme::fill_strong());
    p.rect_filled(
        Rect::from_min_size(bar.min, Vec2::new(w * (pct.min(100) as f32 / 100.0), 6.0)),
        CornerRadius::same(3),
        color,
    );
}

// --- Services ----------------------------------------------------------------------

fn services_tab(ui: &mut egui::Ui, st: &mut DetailState, row: &Row, d: Option<&HostDetail>) {
    let Some(d) = d else {
        no_data(ui, row, None);
        return;
    };
    if let Some(m) = avail_message(d.services_avail) {
        no_data(ui, row, Some(m));
        return;
    }
    let running = d.running_count();
    let failed = d.failed_count() as usize;
    let stopped = d.services.len() - running - failed.min(d.services.len() - running);
    search(ui, &mut st.svc_text, t!("Dienst suchen"), "inv-svc-search");
    ui.add_space(6.0);
    let labels = [
        format!("{} {}", t!("Alle"), d.services.len()),
        format!("{} {running}", t!("Laufend")),
        format!("{} {stopped}", t!("Gestoppt")),
        format!("{} {failed}", t!("Fehler")),
    ];
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    if let Some(i) = theme::segmented(ui, &refs, st.svc_filter) {
        st.svc_filter = i;
    }
    ui.add_space(6.0);
    if d.services_total > d.services.len() {
        dim(
            ui,
            &t!(
                "{} Dienste gesamt, die ersten {} (Fehler und laufende zuerst) sind gespeichert.",
                d.services_total,
                d.services.len()
            ),
        );
    }
    let list: Vec<RowSpec> = d
        .services
        .iter()
        .filter(|s| match st.svc_filter {
            1 => s.running,
            2 => !s.running && !s.failed,
            3 => s.failed,
            _ => true,
        })
        .filter(|s| {
            st.svc_text.trim().is_empty() || matches(&[&s.name, &s.description], &st.svc_text)
        })
        .map(|s| {
            let (col, bg) = if s.failed {
                (theme::red(), Some(theme::soft(theme::red())))
            } else if s.running {
                (theme::green(), None)
            } else {
                (theme::text_dim(), None)
            };
            let state = format!("{} ({})", s.active, s.sub);
            RowSpec {
                cells: vec![
                    (
                        s.name.clone(),
                        if s.failed {
                            theme::red()
                        } else {
                            theme::text()
                        },
                    ),
                    (state, col),
                    (s.start.clone(), theme::text_dim()),
                    (s.description.clone(), theme::text_dim()),
                ],
                bg,
            }
        })
        .collect();
    grid(
        ui,
        &[
            ColSpec {
                title: t!("Dienst"),
                width: 190.0,
                mono: true,
                optional: false,
            },
            ColSpec {
                title: t!("Status"),
                width: 130.0,
                mono: false,
                optional: false,
            },
            ColSpec {
                title: t!("Autostart"),
                width: 90.0,
                mono: false,
                optional: true,
            },
            ColSpec {
                title: t!("Beschreibung"),
                width: 0.0,
                mono: false,
                optional: true,
            },
        ],
        &list,
    );
    if list.is_empty() {
        ui.add_space(8.0);
        dim(ui, t!("Keine Dienste passen zum Filter."));
    }
}

// --- Firewall ----------------------------------------------------------------------

fn firewall_tab(ui: &mut egui::Ui, st: &mut DetailState, row: &Row, d: Option<&HostDetail>) {
    let Some(d) = d else {
        no_data(ui, row, None);
        return;
    };
    let fw: &Firewall = &d.firewall;
    if let Some(m) = avail_message(fw.avail) {
        no_data(ui, row, Some(m));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t!("Werkzeug: {}", fw.backend)).font(theme::semibold(13.5)));
        match fw.active {
            Some(true) => {
                theme::chip(ui, t!("aktiv"), theme::green());
            }
            Some(false) => {
                theme::chip(ui, t!("inaktiv"), theme::amber());
            }
            None => {}
        }
    });
    for l in &fw.summary {
        ui.label(theme::mono(l, 12.0).color(theme::text_dim()));
    }
    if fw.rules.is_empty() {
        ui.add_space(8.0);
        dim(ui, t!("Keine Regeln erfasst."));
        return;
    }
    if fw.total_rules > fw.rules.len() {
        dim(
            ui,
            &t!(
                "{} Regeln gesamt, die ersten {} sind gespeichert.",
                fw.total_rules,
                fw.rules.len()
            ),
        );
    }
    ui.add_space(6.0);
    search(ui, &mut st.fw_text, t!("Regel suchen"), "inv-fw-search");
    ui.add_space(6.0);
    let all_raw = fw.rules.iter().all(|r| {
        r.name.is_empty() && r.direction.is_empty() && r.action.is_empty() && r.port.is_empty()
    });
    if all_raw {
        // nft and friends: the lines as they are, indentation kept.
        ui.spacing_mut().item_spacing.y = 1.0;
        for r in fw
            .rules
            .iter()
            .filter(|r| st.fw_text.trim().is_empty() || matches(&[&r.raw], &st.fw_text))
        {
            ui.add(
                egui::Label::new(theme::mono(&r.raw, 12.0).color(theme::text())).selectable(true),
            );
        }
        return;
    }
    let list: Vec<RowSpec> = fw
        .rules
        .iter()
        .filter(|r| {
            st.fw_text.trim().is_empty()
                || matches(
                    &[
                        &r.name,
                        &r.action,
                        &r.direction,
                        &r.protocol,
                        &r.port,
                        &r.program,
                        &r.source,
                        &r.raw,
                        &r.profile,
                    ],
                    &st.fw_text,
                )
        })
        .map(|r| {
            let blocked = ["block", "deny", "reject", "drop"]
                .iter()
                .any(|w| r.action.to_lowercase().starts_with(w));
            let details = [&r.name, &r.source, &r.program, &r.profile, &r.raw]
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            RowSpec {
                cells: vec![
                    (
                        r.action.clone(),
                        if blocked {
                            theme::amber()
                        } else {
                            theme::green()
                        },
                    ),
                    (r.direction.clone(), theme::text_dim()),
                    (r.protocol.clone(), theme::text_dim()),
                    (r.port.clone(), theme::text()),
                    (details, theme::text()),
                ],
                bg: None,
            }
        })
        .collect();
    grid(
        ui,
        &[
            ColSpec {
                title: t!("Aktion"),
                width: 70.0,
                mono: false,
                optional: false,
            },
            ColSpec {
                title: t!("Richtung"),
                width: 80.0,
                mono: false,
                optional: false,
            },
            ColSpec {
                title: t!("Protokoll"),
                width: 70.0,
                mono: false,
                optional: true,
            },
            ColSpec {
                title: t!("Port"),
                width: 100.0,
                mono: true,
                optional: false,
            },
            ColSpec {
                title: t!("Details"),
                width: 0.0,
                mono: false,
                optional: false,
            },
        ],
        &list,
    );
}

// --- Info snippets -------------------------------------------------------------------

fn info_tab(ui: &mut egui::Ui, row: &Row, d: Option<&HostDetail>, s: &InventorySettings) {
    let Some(d) = d else {
        no_data(ui, row, None);
        return;
    };
    if d.info.is_empty() {
        let msg = if s.info_snippets.iter().any(|x| x.runnable()) {
            t!(
                "Noch keine Ausgabe – „Fakten aktualisieren“ klicken. Die Info-Snippets laufen dabei auf diesem Host, sofern ihr Filter passt."
            )
        } else {
            t!(
                "Keine Info-Snippets definiert. Eigene Befehle legen Sie unter Einstellungen → Inventar an; ihre Ausgabe erscheint hier und im Export."
            )
        };
        dim(ui, msg);
        return;
    }
    for r in &d.info {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&r.name).font(theme::semibold(13.5)));
            if let Some(t) = r.at {
                ui.label(
                    RichText::new(
                        t.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string(),
                    )
                    .size(12.0)
                    .color(theme::text_dim()),
                );
            }
            if let Some(c) = r.exit.filter(|c| *c != 0) {
                theme::chip(ui, &t!("Exit-Code {}", c), theme::amber());
            }
            if r.truncated {
                theme::chip(ui, t!("Ausgabe gekürzt"), theme::amber());
            }
        });
        if !r.error.is_empty() {
            ui.label(RichText::new(&r.error).size(12.5).color(theme::red()));
            continue;
        }
        if r.output.trim().is_empty() {
            dim(ui, t!("Keine Ausgabe"));
            continue;
        }
        // Only text is shown; very long outputs are cut here (the export has all of it).
        const SHOWN: usize = 300;
        let lines = r.output.lines().count();
        let shown: String = r.output.lines().take(SHOWN).collect::<Vec<_>>().join("\n");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(theme::mono(shown, 12.0).color(theme::text())).selectable(true),
            );
        });
        if lines > SHOWN {
            dim(
                ui,
                &t!(
                    "Die ersten {} von {} Zeilen – der Export enthält alles.",
                    SHOWN,
                    lines
                ),
            );
        }
    }
}

// --- Ports -------------------------------------------------------------------------

fn ports_tab(ui: &mut egui::Ui, st: &mut DetailState, row: &Row, d: Option<&HostDetail>) {
    let Some(d) = d else {
        no_data(ui, row, None);
        return;
    };
    if let Some(m) = avail_message(d.ports_avail) {
        no_data(ui, row, Some(m));
        return;
    }
    if d.ports.is_empty() {
        dim(ui, t!("Keine lauschenden Ports gefunden."));
        return;
    }
    search(
        ui,
        &mut st.port_text,
        t!("Port oder Prozess suchen"),
        "inv-port-search",
    );
    ui.add_space(6.0);
    let no_proc = d.ports.iter().all(|p| p.process.is_empty());
    if no_proc {
        dim(
            ui,
            t!("Ohne root zeigt das System keine Prozessnamen anderer Benutzer."),
        );
    }
    let list: Vec<RowSpec> = d
        .ports
        .iter()
        .filter(|p| {
            st.port_text.trim().is_empty()
                || matches(
                    &[&p.port.to_string(), &p.process, &p.addr, &p.proto],
                    &st.port_text,
                )
        })
        .map(|p| {
            let local = p.addr.starts_with("127.") || p.addr == "::1";
            RowSpec {
                cells: vec![
                    (p.proto.to_uppercase(), theme::text_dim()),
                    (
                        p.addr.clone(),
                        if local {
                            theme::text_dim()
                        } else {
                            theme::text()
                        },
                    ),
                    (p.port.to_string(), theme::text()),
                    (p.process.clone(), theme::text()),
                ],
                bg: None,
            }
        })
        .collect();
    grid(
        ui,
        &[
            ColSpec {
                title: t!("Protokoll"),
                width: 80.0,
                mono: false,
                optional: false,
            },
            ColSpec {
                title: t!("Adresse"),
                width: 190.0,
                mono: true,
                optional: false,
            },
            ColSpec {
                title: t!("Port"),
                width: 80.0,
                mono: true,
                optional: false,
            },
            ColSpec {
                title: t!("Prozess"),
                width: 0.0,
                mono: false,
                optional: false,
            },
        ],
        &list,
    );
}
