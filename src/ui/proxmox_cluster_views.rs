//! Pages and sheets of the cluster views: overview cards, task list, task log, node
//! updates and the alert list.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use super::{
    ClusterAction, act, bytes, cell, duration, level_color, local_time, notice, task_label,
    usage_bar,
};
use crate::app::{Action, App};
use crate::backend::proxmox::cluster::{Level, Overview, TaskInfo, TaskState};
use crate::model::PveServer;
use crate::theme;
use crate::ui::proxmox::pve2;

fn no_servers(ui: &mut egui::Ui) {
    ui.add_space(20.0);
    ui.label(
        RichText::new(t!("Noch kein Proxmox-Server – lege ihn unter „Gäste“ an."))
            .color(theme::text_dim()),
    );
}

fn server_heading(
    ui: &mut egui::Ui,
    s: &PveServer,
    cust: &pve2::Cust,
    right: &str,
    color: Color32,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        pve2::customer_tag(ui, cust);
        ui.label(RichText::new(s.display_name()).font(theme::semibold(16.0)));
        if !right.is_empty() {
            ui.label(RichText::new(right).size(13.0).color(color));
        }
    });
}

// ------------------------------------------------------------------ overview

/// Left-aligned label in a fixed-width column.
fn col(ui: &mut egui::Ui, width: f32, label: egui::Label) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(label.truncate());
        },
    );
}

pub fn overview(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.lib.pve_servers.is_empty() {
        no_servers(ui);
        return;
    }
    let servers = pve2::visible_pve(app);
    if servers.is_empty() {
        let mut acts = Vec::new();
        crate::ui::nav2_scope::empty_hint(ui, app, t!("keinen Proxmox-Server"), &mut acts);
        actions.append(&mut acts);
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("pve-overview")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            for (i, s) in servers.iter().enumerate() {
                if i > 0 {
                    ui.add_space(22.0);
                }
                let cust = pve2::cust_of(app, s.group);
                let data = app.pve.cluster.servers.get(&s.id);
                let loading = data.is_some_and(|d| d.overview_loading);
                match data.and_then(|d| d.overview.as_ref()) {
                    None => {
                        server_heading(
                            ui,
                            s,
                            &cust,
                            if s.tls_pin.is_empty() {
                                t!("Zertifikat noch nicht bestätigt")
                            } else if loading {
                                t!("lädt…")
                            } else {
                                ""
                            },
                            theme::text_dim(),
                        );
                    }
                    Some(Err(e)) => {
                        server_heading(ui, s, &cust, "", theme::text_dim());
                        ui.add_space(6.0);
                        notice(ui, theme::red(), e);
                    }
                    Some(Ok(ov)) => {
                        let ov = ov.clone();
                        overview_card(ui, s, &cust, &ov, actions);
                    }
                }
            }
            ui.add_space(40.0);
        });
}

fn overview_card(
    ui: &mut egui::Ui,
    s: &PveServer,
    cust: &pve2::Cust,
    ov: &Overview,
    actions: &mut Vec<Action>,
) {
    let online = ov.nodes.iter().filter(|n| n.online).count();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        pve2::customer_tag(ui, cust);
        ui.label(RichText::new(s.display_name()).font(theme::semibold(16.0)));
        if let Some(c) = &ov.cluster {
            ui.label(
                RichText::new(t!("Cluster „{}“", c.name))
                    .size(13.0)
                    .color(theme::text_dim()),
            );
            if c.quorate {
                theme::pill(ui, t!("Quorum OK"), theme::green());
            } else {
                theme::pill(ui, t!("kein Quorum"), theme::red());
            }
        }
        ui.label(
            RichText::new(t!("{} von {} Nodes online", online, ov.nodes.len()))
                .size(13.0)
                .color(if online == ov.nodes.len() {
                    theme::text_dim()
                } else {
                    theme::red()
                }),
        );
    });
    ui.add_space(8.0);
    let warnings = ov.warnings();
    if warnings.is_empty() {
        notice(ui, theme::green(), t!("Alles in Ordnung"));
    } else {
        for w in &warnings {
            notice(
                ui,
                match w.level {
                    Level::Crit => theme::red(),
                    Level::Warn => theme::amber(),
                },
                &w.text,
            );
            ui.add_space(4.0);
        }
    }
    for p in &ov.problems {
        ui.add_space(4.0);
        ui.label(
            RichText::new(t!("nicht lesbar: {}", p))
                .size(12.0)
                .color(theme::text_dim()),
        );
    }
    ui.add_space(10.0);
    theme::card(ui, theme::bg_deep(), |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Narrow windows: secondary columns go, bars shrink, the rest truncates.
                let width = ui.available_width();
                let narrow = width < 640.0;
                ui.spacing_mut().item_spacing.y = 6.0;
                theme::section_title(ui, t!("Nodes"));
                for n in &ov.nodes {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        theme::led(
                            ui.painter(),
                            r.center(),
                            if n.online {
                                theme::green()
                            } else {
                                theme::red()
                            },
                            n.online,
                        );
                        col(
                            ui,
                            120.0,
                            egui::Label::new(RichText::new(&n.name).font(theme::medium(13.5)))
                                .truncate(),
                        );
                        if !narrow {
                            col(
                                ui,
                                120.0,
                                egui::Label::new(
                                    theme::mono(
                                        n.version
                                            .as_deref()
                                            .map(|v| format!("PVE {v}"))
                                            .unwrap_or_else(|| "–".into()),
                                        12.0,
                                    )
                                    .color(theme::text_dim()),
                                ),
                            );
                        }
                        col(
                            ui,
                            120.0,
                            egui::Label::new(theme::mono(&n.ip, 12.0).color(theme::text_dim())),
                        );
                        if !n.online {
                            ui.label(RichText::new(t!("offline")).color(theme::red()));
                            return;
                        }
                        match n.updates.as_ref().map(|u| u.len()) {
                            Some(0) => {
                                ui.label(
                                    RichText::new(t!("aktuell"))
                                        .size(12.5)
                                        .color(theme::green()),
                                );
                            }
                            Some(c) => {
                                theme::tag(ui, &t!("{} Updates", c), Some(theme::amber()));
                            }
                            None => {}
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::btn(ui, theme::Btn::Link, None, t!("Updates")).clicked() {
                                actions.push(act(ClusterAction::OpenUpdates(s.id, n.name.clone())));
                            }
                            if theme::btn(
                                ui,
                                theme::Btn::Link,
                                Some(crate::ui::icons::Icon::Terminal),
                                t!("Shell"),
                            )
                            .clicked()
                            {
                                actions.push(act(ClusterAction::NodeShell(s.id, n.name.clone())));
                            }
                        });
                    });
                }

                ui.add_space(10.0);
                theme::section_title(ui, t!("Speicher"));
                if ov.storages.is_empty() {
                    ui.label(RichText::new(t!("keine")).color(theme::text_dim()));
                }
                for st in &ov.storages {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        col(
                            ui,
                            140.0,
                            egui::Label::new(RichText::new(&st.id).font(theme::medium(13.5)))
                                .truncate(),
                        );
                        if !narrow {
                            col(
                                ui,
                                70.0,
                                egui::Label::new(
                                    theme::mono(&st.kind, 12.0).color(theme::text_dim()),
                                ),
                            );
                        }
                        col(
                            ui,
                            90.0,
                            egui::Label::new(
                                RichText::new(
                                    st.node.clone().unwrap_or_else(|| t!("geteilt").to_string()),
                                )
                                .size(12.5)
                                .color(theme::text_dim()),
                            )
                            .truncate(),
                        );
                        if !st.active {
                            ui.label(
                                RichText::new(if st.enabled {
                                    t!("nicht verfügbar")
                                } else {
                                    t!("deaktiviert")
                                })
                                .size(12.5)
                                .color(if st.enabled {
                                    theme::amber()
                                } else {
                                    theme::text_dim()
                                }),
                            );
                            return;
                        }
                        let used = 140.0 + 90.0 + 30.0 + if narrow { 0.0 } else { 80.0 };
                        usage_bar(ui, (width - used - 190.0).clamp(50.0, 220.0), st.usage());
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} %  ·  {} / {}",
                                    (st.usage() * 100.0).round(),
                                    bytes(st.used),
                                    bytes(st.total)
                                ))
                                .size(12.5)
                                .color(
                                    if st.usage() > crate::backend::proxmox::cluster::STORAGE_WARN {
                                        level_color(st.usage())
                                    } else {
                                        theme::text_dim()
                                    },
                                ),
                            )
                            .truncate(),
                        );
                    });
                }

                if !ov.zfs.is_empty() {
                    ui.add_space(10.0);
                    theme::section_title(ui, t!("ZFS-Pools"));
                    for p in &ov.zfs {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            col(
                                ui,
                                140.0,
                                egui::Label::new(RichText::new(&p.name).font(theme::medium(13.5)))
                                    .truncate(),
                            );
                            col(
                                ui,
                                if narrow { 80.0 } else { 160.0 },
                                egui::Label::new(
                                    RichText::new(&p.node).size(12.5).color(theme::text_dim()),
                                ),
                            );
                            theme::pill(
                                ui,
                                &p.health,
                                if p.healthy() {
                                    theme::green()
                                } else {
                                    theme::red()
                                },
                            );
                            let frac = if p.size == 0 {
                                0.0
                            } else {
                                p.alloc as f32 / p.size as f32
                            };
                            let used = 140.0 + 90.0 + 50.0 + if narrow { 80.0 } else { 160.0 };
                            usage_bar(ui, (width - used - 170.0).clamp(40.0, 140.0), frac);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!(
                                        "{} / {} · frag {} %",
                                        bytes(p.alloc),
                                        bytes(p.size),
                                        p.frag
                                    ))
                                    .size(12.5)
                                    .color(theme::text_dim()),
                                )
                                .truncate(),
                            );
                        });
                    }
                }

                if !ov.ha.is_empty() {
                    ui.add_space(10.0);
                    theme::section_title(ui, t!("Hochverfügbarkeit"));
                    for h in &ov.ha {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            col(
                                ui,
                                140.0,
                                egui::Label::new(theme::mono(&h.id, 12.5)).truncate(),
                            );
                            let text = if h.kind == "service" {
                                format!("{} · {}", h.state, h.node)
                            } else {
                                h.status.clone()
                            };
                            ui.add(
                                egui::Label::new(RichText::new(text).size(12.5).color(
                                    if h.bad() {
                                        theme::red()
                                    } else {
                                        theme::text_dim()
                                    },
                                ))
                                .truncate(),
                            );
                        });
                    }
                }

                if let Some(c) = &ov.ceph {
                    ui.add_space(10.0);
                    theme::section_title(ui, "Ceph");
                    ui.horizontal_wrapped(|ui| {
                        theme::pill(
                            ui,
                            &c.status,
                            match c.status.as_str() {
                                "HEALTH_OK" => theme::green(),
                                "HEALTH_WARN" => theme::amber(),
                                _ => theme::red(),
                            },
                        );
                        for chk in &c.checks {
                            ui.label(RichText::new(chk).size(12.5).color(theme::text_dim()));
                        }
                    });
                }
            });
    });
}

// ------------------------------------------------------------------ tasks

const TASK_ROW_H: f32 = 40.0;

fn passes(filter: usize, t: &TaskInfo) -> bool {
    match filter {
        0 => t.running(),
        1 => t.failed(),
        _ => true,
    }
}

pub fn tasks(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.lib.pve_servers.is_empty() {
        no_servers(ui);
        return;
    }
    ui.horizontal(|ui| {
        if let Some(i) = theme::segmented(
            ui,
            &["Laufend", "Fehlgeschlagen", "Alle"].map(crate::i18n::tr),
            app.pve.cluster.task_filter,
        ) {
            app.pve.cluster.task_filter = i;
        }
    });
    ui.add_space(14.0);
    let servers = pve2::visible_pve(app);
    if servers.is_empty() {
        let mut acts = Vec::new();
        crate::ui::nav2_scope::empty_hint(ui, app, t!("keinen Proxmox-Server"), &mut acts);
        actions.append(&mut acts);
        return;
    }
    let filter = app.pve.cluster.task_filter;
    let now = chrono::Utc::now().timestamp();
    egui::ScrollArea::vertical()
        .id_salt("pve-tasks")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            for (i, s) in servers.iter().enumerate() {
                if i > 0 {
                    ui.add_space(22.0);
                }
                let cust = pve2::cust_of(app, s.group);
                let data = app.pve.cluster.servers.get(&s.id);
                let loading = data.is_some_and(|d| d.tasks_loading);
                let list = match data.and_then(|d| d.tasks.as_ref()) {
                    None => {
                        server_heading(
                            ui,
                            s,
                            &cust,
                            if s.tls_pin.is_empty() {
                                t!("Zertifikat noch nicht bestätigt")
                            } else if loading {
                                t!("lädt…")
                            } else {
                                ""
                            },
                            theme::text_dim(),
                        );
                        continue;
                    }
                    Some(Err(e)) => {
                        server_heading(ui, s, &cust, "", theme::text_dim());
                        ui.add_space(6.0);
                        notice(ui, theme::red(), e);
                        continue;
                    }
                    Some(Ok(list)) => list.clone(),
                };
                let running = list.iter().filter(|t| t.running()).count();
                let failed = list.iter().filter(|t| t.failed()).count();
                server_heading(
                    ui,
                    s,
                    &cust,
                    &t!("{} laufend · {} fehlgeschlagen", running, failed),
                    theme::text_dim(),
                );
                ui.add_space(8.0);
                let shown: Vec<&TaskInfo> = list.iter().filter(|t| passes(filter, t)).collect();
                if shown.is_empty() {
                    ui.label(
                        RichText::new(match filter {
                            0 => t!("Gerade läuft keine Aufgabe."),
                            1 => t!("Keine fehlgeschlagenen Aufgaben."),
                            _ => t!("Keine Aufgaben."),
                        })
                        .size(13.0)
                        .color(theme::text_dim()),
                    );
                    continue;
                }
                let names = guest_names(app, s.id);
                theme::card(ui, theme::bg_deep(), |ui| {
                    task_header(ui);
                    for t in shown.iter().take(300) {
                        if task_row(ui, t, &names, now).clicked() {
                            actions.push(act(ClusterAction::OpenLog(s.id, (*t).clone())));
                        }
                    }
                });
            }
            ui.add_space(40.0);
        });
}

fn guest_names(app: &App, server: Uuid) -> std::collections::HashMap<u32, String> {
    app.pve
        .live
        .get(&server)
        .and_then(|l| l.snap.as_ref())
        .map(|s| s.guests.iter().map(|g| (g.vmid, g.name.clone())).collect())
        .unwrap_or_default()
}

/// Column x offsets (fraction of the row width) and their titles. Narrow tables drop the
/// user column (zero width) so the others keep readable widths.
fn columns(width: f32) -> [(f32, &'static str); 7] {
    if width < 700.0 {
        [
            (0.0, t!("Zeit")),
            (0.15, t!("Node")),
            (0.26, t!("Typ")),
            (0.47, t!("Gast")),
            (0.68, t!("Benutzer")),
            (0.68, t!("Status")),
            (0.87, t!("Dauer")),
        ]
    } else {
        [
            (0.0, t!("Zeit")),
            (0.12, t!("Node")),
            (0.22, t!("Typ")),
            (0.40, t!("Gast")),
            (0.58, t!("Benutzer")),
            (0.74, t!("Status")),
            (0.90, t!("Dauer")),
        ]
    }
}

fn task_header(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    let inner = rect.shrink2(Vec2::new(18.0, 0.0));
    let p = ui.painter();
    let cols = columns(inner.width());
    for (i, (x, title)) in cols.iter().copied().enumerate() {
        if cols.get(i + 1).is_some_and(|n| n.0 <= x) {
            continue;
        }
        p.text(
            Pos2::new(inner.left() + inner.width() * x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            title.to_uppercase(),
            theme::semibold(11.0),
            theme::text_dim(),
        );
    }
}

fn task_row(
    ui: &mut egui::Ui,
    t: &TaskInfo,
    names: &std::collections::HashMap<u32, String>,
    now: i64,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), TASK_ROW_H), Sense::click());
    let p = ui.painter();
    p.line_segment(
        [rect.left_top() + Vec2::new(18.0, 0.0), rect.right_top()],
        egui::Stroke::new(1.0, theme::line()),
    );
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    let inner = rect.shrink2(Vec2::new(18.0, 0.0));
    let cy = rect.center().y;
    let cols = columns(inner.width());
    let x = |i: usize| inner.left() + inner.width() * cols[i].0;
    let w = |i: usize| {
        let next = cols.get(i + 1).map(|c| c.0).unwrap_or(1.0);
        inner.width() * (next - cols[i].0) - 10.0
    };
    let font = FontId::proportional(13.0);
    cell(
        p,
        x(0),
        cy,
        w(0),
        &local_time(t.start),
        FontId::monospace(12.0),
        theme::text_dim(),
    );
    cell(p, x(1), cy, w(1), &t.node, font.clone(), theme::text());
    cell(
        p,
        x(2),
        cy,
        w(2),
        &format!("{} ({})", task_label(&t.kind), t.kind),
        font.clone(),
        theme::text(),
    );
    let guest = match (t.vmid(), t.vmid().and_then(|v| names.get(&v))) {
        (Some(v), Some(n)) if !n.is_empty() => format!("{v} {n}"),
        (Some(v), _) => v.to_string(),
        (None, _) => t.id.clone(),
    };
    cell(p, x(3), cy, w(3), &guest, font.clone(), theme::text_dim());
    if w(4) > 0.0 {
        cell(p, x(4), cy, w(4), &t.user, font.clone(), theme::text_dim());
    }
    let (status, color) = status_of(t);
    let sx = x(5);
    let g = theme::truncated(p, status.clone(), theme::medium(11.5), color, w(5) - 14.0);
    let pw = (g.size().x + 14.0).min(w(5));
    let pill = Rect::from_min_size(Pos2::new(sx, cy - 10.0), Vec2::new(pw, 20.0));
    p.rect_filled(pill, CornerRadius::same(10), theme::soft(color));
    p.galley(Pos2::new(sx + 7.0, cy - g.size().y / 2.0), g, color);
    cell(
        p,
        x(6),
        cy,
        w(6) + 10.0,
        &duration(t.duration(now)),
        FontId::monospace(12.0),
        theme::text_dim(),
    );
    let resp = match &t.state {
        TaskState::Failed(e) | TaskState::Warnings(e) => resp.on_hover_text(e),
        _ => resp,
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn status_of(t: &TaskInfo) -> (String, Color32) {
    match &t.state {
        TaskState::Running => (t!("läuft").to_string(), theme::accent()),
        TaskState::Ok => ("OK".to_string(), theme::green()),
        TaskState::Warnings(_) => (t!("Warnungen").to_string(), theme::amber()),
        TaskState::Failed(e) => (
            format!(
                "{}: {}",
                t!("Fehler"),
                e.chars().take(40).collect::<String>()
            ),
            theme::red(),
        ),
    }
}

// ------------------------------------------------------------------ sheets

pub fn log_sheet(app: &mut App, ctx: &egui::Context) {
    let names = match &app.pve.cluster.log {
        Some(l) => guest_names(app, l.server),
        None => return,
    };
    let server_name = app
        .pve
        .cluster
        .log
        .as_ref()
        .map(|l| pve2::pve_label(app, l.server))
        .unwrap_or_default();
    let Some(l) = app.pve.cluster.log.as_mut() else {
        return;
    };
    let now = chrono::Utc::now().timestamp();
    let title = format!("{} · {}", task_label(&l.task.kind), l.task.node);
    let mut cancel = false;
    let confirming = l.confirm_cancel;
    let ((close, copy), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-task-log"),
        780.0,
        &title,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (status, color) = if l.running {
                    (t!("läuft").to_string(), theme::accent())
                } else if let Some(exit) = &l.exit {
                    match crate::backend::proxmox::task_result(exit) {
                        Ok(s) if s == "OK" => ("OK".into(), theme::green()),
                        Ok(s) => (s, theme::amber()),
                        Err(e) => (e, theme::red()),
                    }
                } else {
                    status_of(&l.task)
                };
                theme::tag(ui, &status, Some(color));
                let guest = l
                    .task
                    .vmid()
                    .map(|v| match names.get(&v) {
                        Some(n) if !n.is_empty() => format!("{v} {n}"),
                        _ => v.to_string(),
                    })
                    .unwrap_or_else(|| l.task.id.clone());
                let mut info = vec![
                    server_name.clone(),
                    local_time(l.task.start),
                    l.task.user.clone(),
                ];
                if !guest.is_empty() {
                    info.insert(1, guest);
                }
                if !l.running {
                    info.push(duration(l.task.duration(now)));
                }
                ui.label(
                    RichText::new(info.join("  ·  "))
                        .size(13.0)
                        .color(theme::text_dim()),
                );
            });
            ui.add(
                egui::Label::new(theme::mono(&l.task.upid, 11.0).color(theme::text_dim())).wrap(),
            );
            if let Some(e) = &l.error {
                notice(ui, theme::red(), e);
            }
            egui::Frame::NONE
                .fill(theme::bg_deep())
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::both()
                        .id_salt("pve-task-log-lines")
                        .max_height(420.0)
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if l.lines.is_empty() {
                                ui.label(
                                    RichText::new(if l.loading {
                                        t!("lädt…")
                                    } else {
                                        t!("(leer)")
                                    })
                                    .color(theme::text_dim()),
                                );
                            }
                            for line in &l.lines {
                                let color =
                                    if line.starts_with("TASK ERROR") || line.contains("ERROR:") {
                                        theme::red()
                                    } else if line.contains("WARN") {
                                        theme::amber()
                                    } else {
                                        theme::text()
                                    };
                                ui.label(theme::mono(line, 12.0).color(color));
                            }
                        });
                });
            if confirming {
                ui.add_space(4.0);
                notice(
                    ui,
                    theme::amber(),
                    t!(
                        "Laufende Aufgabe wirklich abbrechen? Ein abgebrochenes Backup oder eine abgebrochene Migration muss neu gestartet werden."
                    ),
                );
            }
        },
        |ui, _| {
            let mut out = (false, false);
            if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                out.0 = true;
            }
            if theme::btn(ui, theme::Btn::Normal, None, t!("Kopieren")).clicked() {
                out.1 = true;
            }
            if l.running {
                let label = if l.cancelling {
                    t!("Wird abgebrochen…")
                } else if l.confirm_cancel {
                    t!("Ja, abbrechen")
                } else {
                    t!("Abbrechen")
                };
                let r = ui.add_enabled_ui(!l.cancelling, |ui| {
                    theme::btn(
                        ui,
                        theme::Btn::Normal,
                        Some(crate::ui::icons::Icon::Stop),
                        label,
                    )
                });
                if r.inner.clicked() {
                    if l.confirm_cancel {
                        cancel = true;
                    } else {
                        l.confirm_cancel = true;
                    }
                }
                ui.add_space(8.0);
                ui.spinner();
            }
            out
        },
    );
    if copy {
        ctx.copy_text(l.lines.join("\n"));
    }
    if cancel {
        let (server, upid) = (l.server, l.task.upid.clone());
        super::apply(app, ClusterAction::CancelTask(server, upid), ctx);
        return;
    }
    if close || dismissed {
        app.pve.cluster.log = None;
    }
}

pub fn apt_sheet(app: &mut App, ctx: &egui::Context) {
    let ssh_host = match &app.pve.cluster.apt {
        Some(a) => app
            .lib
            .pve_servers
            .iter()
            .find(|s| s.id == a.server)
            .and_then(|s| s.ssh_host)
            .and_then(|h| app.lib.host(h))
            .map(|h| h.display_name().to_string()),
        None => return,
    };
    let Some(a) = app.pve.cluster.apt.as_mut() else {
        return;
    };
    let mut todo: Option<ClusterAction> = None;
    let title = t!("Updates auf {}", a.node);
    let confirming = a.confirm_upgrade;
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-apt"),
        680.0,
        &title,
        |ui| {
            match &a.list {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
                    });
                }
                Some(Err(e)) => notice(ui, theme::red(), e),
                Some(Ok(list)) if list.is_empty() => {
                    notice(ui, theme::green(), t!("Alle Pakete sind aktuell."));
                }
                Some(Ok(list)) => {
                    ui.label(
                        RichText::new(t!("{} Pakete können aktualisiert werden", list.len()))
                            .color(theme::text_dim()),
                    );
                    theme::form(ui, 110.0, |ui, f| {
                        for p in list {
                            f.full(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    col(
                                        ui,
                                        200.0,
                                        egui::Label::new(theme::mono(&p.name, 12.5)).truncate(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            theme::mono(format!("{} → {}", p.old, p.new), 12.0)
                                                .color(theme::text_dim()),
                                        )
                                        .truncate(),
                                    );
                                });
                            });
                        }
                    });
                }
            }
            if a.refreshing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(t!("apt update läuft auf dem Node…"))
                            .color(theme::text_dim()),
                    );
                });
            }
            ui.add_space(4.0);
            let hint = match &ssh_host {
                Some(h) => t!(
                    "„Upgrade über SSH“ öffnet ein Terminal auf „{}“ und führt dort apt-get update && apt-get dist-upgrade aus (bei einem anderen Node per SSH weiter zum Node). Du bestätigst das Upgrade dort selbst.",
                    h
                ),
                None => t!(
                    "Für das Upgrade braucht der Server einen SSH-Host (Server-Editor → Konsole)."
                )
                .to_string(),
            };
            ui.add(
                egui::Label::new(RichText::new(hint).size(12.5).color(theme::text_dim())).wrap(),
            );
            if confirming {
                notice(
                    ui,
                    theme::amber(),
                    &t!(
                        "Upgrade von {} jetzt starten? Laufende Gäste können durch Neustarts von Diensten kurz gestört werden.",
                        a.node
                    ),
                );
            }
        },
        |ui, _| {
            let mut close = false;
            if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                close = true;
            }
            let busy = a.refreshing || a.loading;
            if ui
                .add_enabled_ui(!busy, |ui| {
                    theme::btn(
                        ui,
                        theme::Btn::Normal,
                        Some(crate::ui::icons::Icon::Refresh),
                        t!("Aktualisieren"),
                    )
                })
                .inner
                .on_hover_text(t!("Paketliste auf dem Node neu laden (apt update)"))
                .clicked()
            {
                todo = Some(ClusterAction::AptRefresh(a.server, a.node.clone()));
            }
            let label = if a.confirm_upgrade {
                t!("Ja, Upgrade starten")
            } else {
                t!("Upgrade über SSH")
            };
            if ui
                .add_enabled_ui(ssh_host.is_some(), |ui| {
                    theme::btn(
                        ui,
                        theme::Btn::Primary,
                        Some(crate::ui::icons::Icon::Terminal),
                        label,
                    )
                })
                .inner
                .clicked()
            {
                if a.confirm_upgrade {
                    todo = Some(ClusterAction::Upgrade(a.server, a.node.clone()));
                } else {
                    a.confirm_upgrade = true;
                }
            }
            close
        },
    );
    if let Some(t) = todo {
        super::apply(app, t, ctx);
        return;
    }
    if close || dismissed {
        app.pve.cluster.apt = None;
    }
}

pub fn alerts_sheet(app: &mut App, ctx: &egui::Context) {
    if !app.pve.cluster.alerts_open {
        return;
    }
    let mut clear = false;
    let shown: std::collections::HashSet<Uuid> =
        pve2::visible_pve(app).iter().map(|s| s.id).collect();
    let st = &mut app.pve.cluster;
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-alerts"),
        640.0,
        t!("Benachrichtigungen"),
        |ui| {
            if st.alerts.is_empty() {
                ui.label(
                    RichText::new(t!(
                        "Keine Benachrichtigungen. Aurora prüft Server mit eingeschalteten Benachrichtigungen alle 45 s."
                    ))
                    .color(theme::text_dim()),
                );
                return;
            }
            for a in st.alerts.iter().filter(|a| shown.contains(&a.server)) {
                let color = match a.kind {
                    crate::backend::proxmox::cluster::AlertKind::GuestDown
                    | crate::backend::proxmox::cluster::AlertKind::NodeOffline => theme::red(),
                    _ => theme::amber(),
                };
                egui::Frame::NONE
                    .fill(if a.read {
                        theme::group_fill()
                    } else {
                        theme::soft(color)
                    })
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, color);
                            ui.label(
                                RichText::new(a.at.format("%d.%m. %H:%M:%S").to_string())
                                    .font(FontId::monospace(12.0))
                                    .color(theme::text_dim()),
                            );
                            ui.label(
                                RichText::new(&a.server_name)
                                    .font(theme::medium(12.5))
                                    .color(theme::text_dim()),
                            );
                        });
                        ui.add(
                            egui::Label::new(RichText::new(&a.text).color(theme::text())).wrap(),
                        );
                    });
                ui.add_space(2.0);
            }
        },
        |ui, _| {
            let mut close = false;
            if theme::btn(ui, theme::Btn::Primary, None, t!("Schließen")).clicked() {
                close = true;
            }
            if !st.alerts.is_empty()
                && theme::btn(
                    ui,
                    theme::Btn::Normal,
                    Some(crate::ui::icons::Icon::Trash),
                    t!("Leeren"),
                )
                .clicked()
            {
                clear = true;
            }
            close
        },
    );
    if clear {
        st.alerts.clear();
    }
    if close || dismissed {
        st.alerts_open = false;
        for a in &mut st.alerts {
            a.read = true;
        }
    }
}
