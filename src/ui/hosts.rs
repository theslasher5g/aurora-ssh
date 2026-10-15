//! Connections overview: one search field (doubles as quick connect), a protocol filter and
//! the host table grouped by folder. Rows instead of cards keep many hosts scannable.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, ConnState, TabKind, parse_quick};
use crate::backend::reach::Reach;
use crate::model::{Host, Protocol};
use crate::theme;
use crate::ui::{fuzzy, relative_time};

const ROW_H: f32 = 44.0;
const ROW_H_COMPACT: f32 = 32.0;

/// Visible host order of the last frame, for Shift+click ranges.
pub fn order_id() -> egui::Id {
    egui::Id::new("aurora-host-order")
}

fn sort_hosts(app: &App, hosts: &mut [Host]) {
    match app.lib.settings.sort {
        1 => hosts.sort_by_key(|h| std::cmp::Reverse(h.last_connected)),
        2 => hosts.sort_by_key(|h| {
            (
                !matches!(app.reach.get(h.id), Some(Reach::Up(_))),
                h.display_name().to_lowercase(),
            )
        }),
        _ => hosts.sort_by_key(|h| h.display_name().to_lowercase()),
    }
}

pub fn protocol_color(p: Protocol) -> Color32 {
    match p {
        Protocol::Ssh => theme::accent(),
        Protocol::Rdp => theme::accent2(),
        Protocol::Serial => theme::green(),
    }
}

pub fn protocol_tag(p: Protocol) -> &'static str {
    match p {
        Protocol::Ssh => "SSH",
        Protocol::Rdp => "RDP",
        Protocol::Serial => "COM",
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.lib.settings.reachability {
        app.reach.tick(&app.rt, &app.lib.hosts, ui.ctx());
    }
    // Header: title, search/quick connect, primary action.
    let mut enter = false;
    ui.horizontal(|ui| {
        ui.label(theme::heading(t!("Verbindungen"), 24.0));
        ui.label(theme::mono(app.lib.hosts.len().to_string(), 13.0).color(theme::text_dim()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new(t!("Neu")).font(theme::semibold(13.5)), |ui| {
                ui.set_min_width(190.0);
                for (label, proto) in [
                    ("SSH-Host", Protocol::Ssh),
                    ("Remote Desktop", Protocol::Rdp),
                    ("Serielles Gerät", Protocol::Serial),
                ] {
                    if ui.button(crate::i18n::tr(label)).clicked() {
                        actions.push(Action::NewHostOf(proto, None));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(t!("Gruppe")).clicked() {
                    actions.push(Action::NewGroup(None));
                    ui.close();
                }
                if ui.button(t!("Netz durchsuchen…")).clicked() {
                    actions.push(Action::OpenScan);
                    ui.close();
                }
            });
            if ui
                .button(t!("Netz scannen"))
                .on_hover_text(t!(
                    "Geräte mit SSH/RDP im lokalen Netz finden und übernehmen"
                ))
                .clicked()
            {
                actions.push(Action::OpenScan);
            }
            let search = ui.add(
                egui::TextEdit::singleline(&mut app.search)
                    .hint_text(t!("Suchen oder user@host verbinden"))
                    .desired_width(320.0)
                    .margin(egui::Margin::symmetric(10, 6)),
            );
            enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        });
    });
    ui.add_space(14.0);

    if app.lib.hosts.is_empty() && app.search.trim().is_empty() {
        empty_state(ui, actions);
        return;
    }

    ui.horizontal(|ui| {
        if let Some(i) = theme::segmented(
            ui,
            &["Alle", "SSH", "RDP", "Seriell", "Favoriten"].map(crate::i18n::tr),
            app.host_filter,
        ) {
            app.host_filter = i;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut changed = false;
            let compact = app.lib.settings.compact;
            if ui
                .selectable_label(compact, RichText::new(t!("Kompakt")).size(12.5))
                .on_hover_text(t!("Kleinere Zeilen für viele Hosts"))
                .clicked()
            {
                app.lib.settings.compact = !compact;
                changed = true;
            }
            let labels = ["Name", "Zuletzt verwendet", "Online zuerst"];
            egui::ComboBox::from_id_salt("host-sort")
                .selected_text(
                    RichText::new(crate::i18n::tr(
                        labels[app.lib.settings.sort.min(2) as usize],
                    ))
                    .size(12.5),
                )
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (i, l) in labels.iter().enumerate() {
                        changed |= ui
                            .selectable_value(
                                &mut app.lib.settings.sort,
                                i as u8,
                                crate::i18n::tr(l),
                            )
                            .changed();
                    }
                });
            ui.label(
                RichText::new(t!("Sortierung"))
                    .size(12.5)
                    .color(theme::text_dim()),
            );
            if changed {
                app.save_library();
            }
        });
    });
    theme::hairline(ui);
    ui.add_space(6.0);
    selection_bar(app, ui, actions);
    ui.ctx()
        .data_mut(|d| d.insert_temp(order_id(), Vec::<Uuid>::new()));

    let query = app.search.trim().to_string();
    let live = live_hosts(app);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !query.is_empty() {
                let quick = parse_quick(&query).filter(|_| query.contains('@'));
                if let Some(h) = &quick {
                    quick_row(
                        ui,
                        &format!("{}@{}:{}", h.username, h.hostname, h.port),
                        actions,
                        &query,
                    );
                }
                let hosts = filter_hosts(app, &query);
                if enter {
                    match (&quick, hosts.first()) {
                        (Some(_), _) => actions.push(Action::QuickConnect(query.clone())),
                        (None, Some(h)) => actions.push(Action::Connect(h.id)),
                        _ => {}
                    }
                }
                if hosts.is_empty() && quick.is_none() {
                    ui.add_space(20.0);
                    ui.label(theme::heading(t!("Kein Host passt zu „{}“", query), 16.0));
                    ui.label(
                        RichText::new(t!(
                            "Leg ihn an – oder tippe user@host, um direkt zu verbinden."
                        ))
                        .color(theme::text_dim()),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, &t!("„{}“ als Host anlegen", query)).clicked()
                        {
                            actions.push(Action::NewHostNamed(query.clone()));
                        }
                        if !query.contains(' ') && ui.button(t!("Direkt verbinden")).clicked() {
                            actions.push(Action::QuickConnect(query.clone()));
                        }
                    });
                }
                for h in &hosts {
                    host_row(app, ui, h, &live, actions);
                }
                return;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !app.selected_hosts.is_empty() {
                actions.push(Action::ClearSelection);
            }

            group_tree(app, ui, None, 0, &live, actions);
            let mut loose: Vec<Host> = app
                .lib
                .hosts
                .iter()
                .filter(|h| h.group_id.is_none_or(|g| app.lib.group(g).is_none()))
                .filter(|h| passes_filter(app.host_filter, h))
                .cloned()
                .collect();
            sort_hosts(app, &mut loose);
            subnet_hint(app, ui, &loose, actions);
            if !loose.is_empty() || (!app.lib.groups.is_empty() && dragging_host(ui)) {
                if !app.lib.groups.is_empty() {
                    ui.add_space(14.0);
                    // Drop zone: hosts dragged here leave their group.
                    let (r, resp) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
                    let hot = resp.dnd_hover_payload::<Uuid>().is_some();
                    if hot {
                        ui.painter().rect_stroke(
                            r,
                            CornerRadius::same(6),
                            Stroke::new(1.5, theme::accent()),
                            egui::StrokeKind::Inside,
                        );
                    }
                    ui.painter().text(
                        r.left_center() + Vec2::new(0.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        t!("OHNE GRUPPE"),
                        FontId::monospace(11.0),
                        theme::text_dim(),
                    );
                    if let Some(id) = resp.dnd_release_payload::<Uuid>() {
                        actions.push(Action::MoveHosts(drag_set(app, *id), None));
                    }
                    ui.add_space(2.0);
                }
                for h in &loose {
                    host_row(app, ui, h, &live, actions);
                }
            }
        });
}

/// Ids of hosts that currently have a connected session.
fn live_hosts(app: &App) -> Vec<String> {
    app.tabs
        .iter()
        .filter(|t| match &t.kind {
            TabKind::Term(t) => matches!(t.state, ConnState::Connected),
            TabKind::Sftp(t) => matches!(t.state(), ConnState::Connected),
            TabKind::Rdp(t) => t.is_live(),
        })
        .flat_map(|t| {
            [
                t.title.clone(),
                t.host_id.map(|h| h.to_string()).unwrap_or_default(),
            ]
        })
        .collect()
}

fn passes_filter(filter: usize, h: &Host) -> bool {
    match filter {
        1 => h.protocol == Protocol::Ssh,
        2 => h.protocol == Protocol::Rdp,
        3 => h.protocol == Protocol::Serial,
        4 => h.favorite,
        _ => true,
    }
}

fn empty_state(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.add_space(60.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.set_max_width(520.0);
            // A row of empty jacks: the patch bay before anything is plugged in.
            let (rect, _) = ui.allocate_exact_size(Vec2::new(5.0 * 34.0, 30.0), Sense::hover());
            for i in 0..5 {
                let c = Pos2::new(rect.left() + 15.0 + i as f32 * 34.0, rect.center().y);
                let color = if i == 0 { theme::accent() } else { theme::border() };
                ui.painter().circle_stroke(c, 11.0, Stroke::new(2.0, color));
                ui.painter().circle_filled(c, 3.0, color);
            }
            ui.add_space(22.0);
            ui.label(theme::heading(t!("Noch nichts eingesteckt"), 22.0));
            ui.add_space(2.0);
            ui.label(
                RichText::new(t!("Lege deine erste Verbindung an. Für einen schnellen Test reicht user@host oben im Suchfeld."))
                    .color(theme::text_dim()),
            );
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, t!("SSH-Host anlegen")).clicked() {
                    actions.push(Action::NewHostOf(Protocol::Ssh, None));
                }
                if ui.button(t!("Remote Desktop")).clicked() {
                    actions.push(Action::NewHostOf(Protocol::Rdp, None));
                }
                if ui.button(t!("Serielles Gerät")).clicked() {
                    actions.push(Action::NewHostOf(Protocol::Serial, None));
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t!("Oder such dein Netz nach SSH-/RDP-Geräten ab:"))
                        .color(theme::text_dim()),
                );
                if ui.button(t!("Netz scannen")).clicked() {
                    actions.push(Action::OpenScan);
                }
            });
            let found = crate::app::ssh_config_hosts().len();
            if found > 0 {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t!("In deiner ~/.ssh/config stehen {} Hosts.", found))
                            .color(theme::text_dim()),
                    );
                    if ui.button(t!("Importieren")).clicked() {
                        actions.push(Action::ImportSshConfig);
                    }
                });
            }
        });
    });
}

/// "5 Hosts im Netz 192.168.0.x – als Gruppe anlegen?" for loose hosts sharing a /24.
/// Hosts that a drag of `id` carries: the whole selection if `id` is part of it.
fn drag_set(app: &App, id: Uuid) -> Vec<Uuid> {
    if app.selected_hosts.contains(&id) {
        app.selected_hosts.iter().copied().collect()
    } else {
        vec![id]
    }
}

fn dragging_host(ui: &egui::Ui) -> bool {
    egui::DragAndDrop::has_payload_of_type::<Uuid>(ui.ctx())
}

/// Bulk actions for the multi-selection (Ctrl/Shift+click).
fn selection_bar(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let n = app.selected_hosts.len();
    if n == 0 {
        return;
    }
    let ids: Vec<Uuid> = app.selected_hosts.iter().copied().collect();
    egui::Frame::NONE
        .fill(theme::accent().gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, theme::accent().gamma_multiply(0.4)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t!("{} ausgewählt", n))
                        .font(theme::semibold(13.5))
                        .color(theme::text()),
                );
                ui.add_space(8.0);
                if theme::primary_button(
                    ui,
                    if n > 1 {
                        t!("Öffnen (nebeneinander)")
                    } else {
                        t!("Öffnen")
                    },
                )
                .clicked()
                {
                    actions.push(Action::ConnectMany(ids.clone()));
                }
                ui.menu_button(t!("In Gruppe"), |ui| {
                    if ui.button(t!("Ohne Gruppe")).clicked() {
                        actions.push(Action::MoveHosts(ids.clone(), None));
                        ui.close();
                    }
                    for g in &app.lib.groups {
                        if ui.button(&g.name).clicked() {
                            actions.push(Action::MoveHosts(ids.clone(), Some(g.id)));
                            ui.close();
                        }
                    }
                });
                ui.menu_button(t!("Umgebung"), |ui| {
                    for e in crate::model::Env::ALL {
                        let label = if e == crate::model::Env::None {
                            t!("keine")
                        } else {
                            e.label()
                        };
                        if ui.button(label).clicked() {
                            actions.push(Action::SetEnvMany(ids.clone(), e));
                            ui.close();
                        }
                    }
                });
                if ui
                    .button(RichText::new(t!("Löschen")).color(theme::red()))
                    .clicked()
                {
                    actions.push(Action::Confirm(
                        t!("{} Hosts löschen?", n),
                        Box::new(Action::DeleteHosts(ids.clone())),
                    ));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("×")
                        .on_hover_text(t!("Auswahl aufheben (Esc)"))
                        .clicked()
                    {
                        actions.push(Action::ClearSelection);
                    }
                    ui.label(
                        RichText::new(t!("Ctrl/Shift+Klick wählt aus · ziehen in Gruppen"))
                            .size(11.5)
                            .color(theme::text_dim()),
                    );
                });
            });
        });
    ui.add_space(6.0);
}

fn subnet_hint(app: &App, ui: &mut egui::Ui, loose: &[Host], actions: &mut Vec<Action>) {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for h in loose {
        if let Some(net) = crate::model::subnet24(&h.hostname) {
            match counts.iter_mut().find(|(n, _)| *n == net) {
                Some((_, c)) => *c += 1,
                None => counts.push((net, 1)),
            }
        }
    }
    let Some((net, n)) = counts
        .into_iter()
        .filter(|(net, c)| *c >= 3 && !app.lib.settings.dismissed_subnets.contains(net))
        .max_by_key(|(_, c)| *c)
    else {
        return;
    };
    ui.add_space(16.0);
    egui::Frame::NONE
        .fill(theme::accent().gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, theme::accent().gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t!("{} Hosts liegen im Netz {}.x", n, net))
                        .size(13.5)
                        .color(theme::text()),
                );
                ui.label(
                    RichText::new(t!("– zusammen als Gruppe ablegen?"))
                        .size(13.5)
                        .color(theme::text_dim()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("×")
                        .on_hover_text(t!("Nicht mehr vorschlagen"))
                        .clicked()
                    {
                        actions.push(Action::DismissSubnet(net.clone()));
                    }
                    if theme::primary_button(ui, &t!("Gruppe „LAN {}“ anlegen", net)).clicked()
                    {
                        actions.push(Action::GroupSubnet(net.clone()));
                    }
                });
            });
        });
}

fn quick_row(ui: &mut egui::Ui, target: &str, actions: &mut Vec<Action>, query: &str) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(6),
        theme::accent().linear_multiply(0.10),
    );
    p.text(
        rect.left_center() + Vec2::new(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        t!("Direkt verbinden"),
        theme::semibold(14.0),
        theme::text(),
    );
    p.text(
        rect.left_center() + Vec2::new(150.0, 0.0),
        egui::Align2::LEFT_CENTER,
        target,
        FontId::monospace(13.0),
        theme::accent(),
    );
    p.text(
        rect.right_center() - Vec2::new(14.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        "Enter",
        FontId::monospace(11.0),
        theme::text_dim(),
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        actions.push(Action::QuickConnect(query.to_string()));
    }
    ui.add_space(4.0);
}

fn filter_hosts(app: &App, query: &str) -> Vec<Host> {
    if let Some(name) = query.strip_prefix("group:") {
        let name = name.trim().to_lowercase();
        let ids: Vec<Uuid> = app
            .lib
            .groups
            .iter()
            .filter(|g| g.name.to_lowercase() == name)
            .map(|g| g.id)
            .collect();
        return app
            .lib
            .hosts
            .iter()
            .filter(|h| {
                h.group_id
                    .is_some_and(|g| ids.iter().any(|id| app.lib.is_descendant(g, *id)))
            })
            .cloned()
            .collect();
    }
    let mut scored: Vec<(i32, Host)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| passes_filter(app.host_filter, h))
        .filter_map(|h| {
            let group = h
                .group_id
                .and_then(|g| app.lib.group(g))
                .map(|g| g.name.as_str())
                .unwrap_or("");
            let hay = format!(
                "{} {} {} {} {}",
                h.display_name(),
                h.hostname,
                h.username,
                h.tags.join(" "),
                group
            );
            fuzzy(query, &hay).map(|s| (s, h.clone()))
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    scored.into_iter().map(|(_, h)| h).collect()
}

fn group_tree(
    app: &mut App,
    ui: &mut egui::Ui,
    parent: Option<Uuid>,
    depth: usize,
    live: &[String],
    actions: &mut Vec<Action>,
) {
    let groups: Vec<_> = app
        .lib
        .groups
        .iter()
        .filter(|g| g.parent_id == parent)
        .cloned()
        .collect();
    for g in groups {
        let mut hosts: Vec<Host> = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id == Some(g.id) && passes_filter(app.host_filter, h))
            .cloned()
            .collect();
        sort_hosts(app, &mut hosts);
        let total = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id.is_some_and(|hg| app.lib.is_descendant(hg, g.id)))
            .count();
        ui.add_space(if depth == 0 { 16.0 } else { 6.0 });
        let indent = depth as f32 * 16.0;
        let head_h = if app.lib.settings.compact { 28.0 } else { 34.0 };
        let (full, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), head_h), Sense::click());
        // Hosts dropped on a group header move into that group.
        let drop_hot = resp.dnd_hover_payload::<Uuid>().is_some();
        if let Some(id) = resp.dnd_release_payload::<Uuid>() {
            actions.push(Action::MoveHosts(drag_set(app, *id), Some(g.id)));
        }
        let rect = Rect::from_min_max(full.min + Vec2::new(indent, 0.0), full.max);
        let p = ui.painter();
        let cy = rect.center().y;
        // Band with the group colour as a bar on the left.
        p.rect_filled(
            rect,
            CornerRadius::same(6),
            if resp.hovered() || drop_hot {
                theme::bg_hover()
            } else {
                theme::bg_panel()
            },
        );
        if drop_hot {
            p.rect_stroke(
                rect,
                CornerRadius::same(6),
                Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        p.rect_filled(
            Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height())),
            CornerRadius {
                nw: 6,
                sw: 6,
                ne: 0,
                se: 0,
            },
            theme::rgb(g.color),
        );
        let x = rect.left() + 12.0;
        let c = Pos2::new(x + 4.0, cy);
        let tri = if g.collapsed {
            vec![
                c + Vec2::new(-2.0, -4.0),
                c + Vec2::new(3.0, 0.0),
                c + Vec2::new(-2.0, 4.0),
            ]
        } else {
            vec![
                c + Vec2::new(-4.0, -2.0),
                c + Vec2::new(4.0, -2.0),
                c + Vec2::new(0.0, 3.0),
            ]
        };
        p.add(egui::Shape::convex_polygon(
            tri,
            theme::text_dim(),
            Stroke::NONE,
        ));
        let galley = p.layout_no_wrap(g.name.clone(), theme::semibold(14.0), theme::text());
        let w = galley.size().x;
        p.galley(
            Pos2::new(x + 16.0, cy - galley.size().y / 2.0),
            galley,
            theme::text(),
        );
        let mut tx = x + 16.0 + w + 10.0;
        // "3/5 online" from the reachability probe.
        let members = app.lib.hosts_in_group(g.id);
        let up = members
            .iter()
            .filter(|h| matches!(app.reach.get(h.id), Some(Reach::Up(_))))
            .count();
        let count = if app.lib.settings.reachability && total > 0 {
            format!("{up}/{total} online")
        } else {
            total.to_string()
        };
        let cg = p.layout_no_wrap(count, FontId::monospace(11.0), theme::text_dim());
        let cw = cg.size().x;
        p.galley(Pos2::new(tx, cy - cg.size().y / 2.0), cg, theme::text_dim());
        tx += cw + 10.0;
        if g.env != crate::model::Env::None {
            let color = theme::env_color(g.env);
            let gl = p.layout_no_wrap(
                g.env.label().into(),
                FontId::monospace(9.5),
                theme::bg_deep(),
            );
            let r = Rect::from_min_size(Pos2::new(tx, cy - 8.0), gl.size() + Vec2::new(8.0, 4.0));
            p.rect_filled(r, CornerRadius::same(3), color);
            p.galley(r.min + Vec2::new(4.0, 2.0), gl, theme::bg_deep());
        }
        let plus_rect =
            Rect::from_min_max(rect.right_top() - Vec2::new(70.0, 0.0), rect.right_bottom());
        let all_rect = Rect::from_min_max(
            rect.right_top() - Vec2::new(170.0, 0.0),
            rect.right_bottom() - Vec2::new(76.0, 0.0),
        );
        {
            // Actions stay visible; they light up under the pointer.
            let pointer = ui.input(|i| i.pointer.hover_pos());
            let hot = |r: Rect| resp.hovered() && pointer.is_some_and(|p| r.contains(p));
            let dim = theme::text_dim().linear_multiply(if resp.hovered() { 1.0 } else { 0.7 });
            p.text(
                rect.right_center() - Vec2::new(12.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                "+ Host",
                FontId::proportional(12.0),
                if hot(plus_rect) { theme::accent() } else { dim },
            );
            if total > 0 {
                p.text(
                    all_rect.right_center(),
                    egui::Align2::RIGHT_CENTER,
                    t!("▶ Alle öffnen"),
                    FontId::proportional(12.0),
                    if hot(all_rect) { theme::accent() } else { dim },
                );
            }
        }
        ui.add_space(2.0);
        let click_pos = ui.input(|i| i.pointer.interact_pos());
        let clicked_in = |r: Rect| resp.clicked() && click_pos.is_some_and(|p| r.contains(p));
        if clicked_in(plus_rect) {
            actions.push(Action::NewHost(Some(g.id)));
        } else if total > 0 && clicked_in(all_rect) {
            actions.push(Action::OpenGroup(g.id));
        } else if resp.clicked() {
            actions.push(Action::ToggleGroup(g.id));
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .context_menu(|ui| {
                if total > 0
                    && ui
                        .button(t!("Alle öffnen (nebeneinander)"))
                        .on_hover_text(t!(
                            "Verbindet die ersten vier Hosts der Gruppe im Splitscreen"
                        ))
                        .clicked()
                {
                    actions.push(Action::OpenGroup(g.id));
                    ui.close();
                }
                if ui.button(t!("Host hinzufügen")).clicked() {
                    actions.push(Action::NewHost(Some(g.id)));
                    ui.close();
                }
                if ui.button(t!("Untergruppe")).clicked() {
                    actions.push(Action::NewGroup(Some(g.id)));
                    ui.close();
                }
                if ui.button(t!("Bearbeiten")).clicked() {
                    actions.push(Action::EditGroup(g.id));
                    ui.close();
                }
                if ui
                    .button(RichText::new(t!("Löschen")).color(theme::red()))
                    .clicked()
                {
                    actions.push(Action::Confirm(
                        t!("Gruppe „{}“ löschen? Die Hosts bleiben erhalten.", g.name),
                        Box::new(Action::DeleteGroup(g.id)),
                    ));
                    ui.close();
                }
            });
        if !g.collapsed {
            for h in &hosts {
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    ui.vertical(|ui| host_row(app, ui, h, live, actions));
                });
            }
            group_tree(app, ui, Some(g.id), depth + 1, live, actions);
        }
    }
}

/// Brand-ish colour for an OS id from os-release.
fn os_color(id: &str) -> Color32 {
    match id {
        "ubuntu" => Color32::from_rgb(233, 84, 32),
        "debian" | "raspbian" => Color32::from_rgb(215, 10, 83),
        "fedora" | "rhel" | "centos" => Color32::from_rgb(81, 162, 218),
        "rocky" | "almalinux" => Color32::from_rgb(16, 185, 129),
        "arch" | "alpine" => Color32::from_rgb(23, 147, 209),
        "windows" => Color32::from_rgb(0, 120, 212),
        "freebsd" | "opnsense" | "pfsense" => Color32::from_rgb(171, 43, 40),
        _ => theme::text_dim(),
    }
}

fn uptime_text(secs: u64) -> String {
    let d = secs / 86_400;
    match (d, secs / 3600) {
        (d, _) if d > 0 => format!("up {d} d"),
        (_, h) if h > 0 => format!("up {h} h"),
        _ => format!("up {} min", secs / 60),
    }
}

/// Small outlined (or filled) mono label at `x`, advancing `x`.
fn chip(p: &egui::Painter, x: &mut f32, cy: f32, label: String, color: Color32, filled: bool) {
    let fg = if filled { theme::bg_deep() } else { color };
    let g = p.layout_no_wrap(label, FontId::monospace(10.5), fg);
    let r = Rect::from_min_size(Pos2::new(*x, cy - 9.0), g.size() + Vec2::new(10.0, 6.0));
    if filled {
        p.rect_filled(r, CornerRadius::same(3), color);
    } else {
        p.rect_stroke(
            r,
            CornerRadius::same(3),
            Stroke::new(1.0, color.linear_multiply(0.4)),
            egui::StrokeKind::Inside,
        );
    }
    p.galley(r.min + Vec2::new(5.0, 3.0), g, fg);
    *x = r.right() + 6.0;
}

pub fn rename_id() -> egui::Id {
    egui::Id::new("aurora-rename")
}

fn host_row(app: &App, ui: &mut egui::Ui, h: &Host, live: &[String], actions: &mut Vec<Action>) {
    let row_h = if app.lib.settings.compact {
        ROW_H_COMPACT
    } else {
        ROW_H
    };
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), row_h),
        Sense::click_and_drag(),
    );
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Uuid>>(order_id())
            .push(h.id)
    });
    let hovered = ui.rect_contains_pointer(rect);
    let selected = app.selected_hosts.contains(&h.id);
    let p = ui.painter().with_clip_rect(rect);
    if selected {
        p.rect_filled(
            rect,
            CornerRadius::same(6),
            theme::accent().gamma_multiply(0.12),
        );
        p.rect_filled(
            Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height())),
            CornerRadius::same(1),
            theme::accent(),
        );
    } else if hovered {
        p.rect_filled(rect, CornerRadius::same(6), theme::bg_hover());
    }
    // Drag the row (or the whole selection) onto a group header.
    resp.dnd_set_drag_payload(h.id);
    if resp.dragged() {
        let n = if selected {
            app.selected_hosts.len()
        } else {
            1
        };
        if let Some(pos) = ui.ctx().pointer_latest_pos() {
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("host-drag"),
            ));
            let text = if n > 1 {
                t!("{} Hosts", n)
            } else {
                h.display_name().to_string()
            };
            let g = painter.layout_no_wrap(text, theme::semibold(13.0), theme::text());
            let r = Rect::from_min_size(
                pos + Vec2::new(14.0, 10.0),
                g.size() + Vec2::new(18.0, 10.0),
            );
            painter.rect(
                r,
                CornerRadius::same(6),
                theme::bg_card(),
                Stroke::new(1.0, theme::accent()),
                egui::StrokeKind::Inside,
            );
            painter.galley(r.min + Vec2::new(9.0, 5.0), g, theme::text());
        }
    }
    if resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        let mode = if m.shift {
            2
        } else if m.command {
            1
        } else {
            0
        };
        if mode != 0 || !app.selected_hosts.is_empty() {
            actions.push(Action::SelectHost(h.id, mode));
        }
    }
    // bottom rule
    p.rect_filled(
        Rect::from_min_max(
            Pos2::new(rect.left() + 10.0, rect.bottom() - 1.0),
            rect.right_bottom(),
        ),
        CornerRadius::ZERO,
        theme::border().linear_multiply(0.45),
    );

    let cy = rect.center().y;
    // LED: lit green = open session, green = reachable, hollow = no answer (probably off),
    // red = the last connection attempt failed.
    let id = h.id.to_string();
    let is_live = live.iter().any(|t| *t == id || t == h.display_name());
    let reach = app
        .lib
        .settings
        .reachability
        .then(|| app.reach.get(h.id))
        .flatten();
    let led_pos = Pos2::new(rect.left() + 14.0, cy);
    if app.failed.contains(&h.id) && !is_live {
        theme::led(&p, led_pos, theme::red(), true);
    } else {
        match (is_live, reach) {
            (true, _) => theme::led(&p, led_pos, theme::green(), true),
            (false, Some(Reach::Up(_))) => theme::led(&p, led_pos, theme::green(), false),
            (false, Some(Reach::Down)) => {
                p.circle_stroke(led_pos, 3.2, Stroke::new(1.2, theme::text_dim()));
            }
            (false, None) => theme::led(&p, led_pos, theme::text_dim(), false),
        }
    }
    {
        let tip_rect = Rect::from_center_size(led_pos, Vec2::splat(18.0));
        let tip = if app.failed.contains(&h.id) && !is_live {
            t!("letzte Verbindung fehlgeschlagen").to_string()
        } else if is_live {
            t!("Session offen").to_string()
        } else {
            match reach {
                Some(Reach::Up(ms)) => t!("erreichbar · {} ms", ms),
                Some(Reach::Down) => t!("keine Antwort – vermutlich aus").to_string(),
                None => t!("noch nicht geprüft").to_string(),
            }
        };
        ui.interact(tip_rect, ui.id().with(("reach", h.id)), Sense::hover())
            .on_hover_text(tip);
    }

    let w = rect.width();
    let col_name = rect.left() + 30.0;
    let col_addr = rect.left() + (w * 0.27).max(190.0);
    let col_info = rect.left() + (w * 0.47).max(360.0);

    // --- name (or inline rename)
    let renaming: Option<(Uuid, String)> = ui.ctx().data(|d| d.get_temp(rename_id()));
    let mut nx;
    if let Some((rid, mut text)) = renaming.filter(|(rid, _)| *rid == h.id) {
        let _ = rid;
        let r = Rect::from_min_size(
            Pos2::new(col_name - 4.0, cy - 13.0),
            Vec2::new((col_addr - col_name - 16.0).max(120.0), 26.0),
        );
        let edit = ui.put(
            r,
            egui::TextEdit::singleline(&mut text)
                .font(theme::semibold(14.0))
                .hint_text(h.hostname.as_str()),
        );
        edit.request_focus();
        let (enter, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if enter || (edit.lost_focus() && !esc) {
            actions.push(Action::RenameHost(h.id, text));
            ui.ctx()
                .data_mut(|d| d.remove::<(Uuid, String)>(rename_id()));
        } else if esc {
            ui.ctx()
                .data_mut(|d| d.remove::<(Uuid, String)>(rename_id()));
        } else {
            ui.ctx()
                .data_mut(|d| d.insert_temp(rename_id(), (h.id, text)));
        }
        nx = r.right() + 8.0;
    } else {
        let name_galley = p.layout_no_wrap(
            h.display_name().to_string(),
            theme::semibold(14.0),
            theme::text(),
        );
        let name_w = name_galley.size().x;
        p.galley(
            Pos2::new(col_name, cy - name_galley.size().y / 2.0),
            name_galley,
            theme::text(),
        );
        nx = col_name + name_w + 8.0;
    }
    if h.favorite {
        p.text(
            Pos2::new(nx, cy),
            egui::Align2::LEFT_CENTER,
            "★",
            FontId::proportional(11.0),
            theme::accent(),
        );
        nx += 16.0;
    }
    let env = app.lib.env_of(h);
    if env != crate::model::Env::None {
        let color = theme::env_color(env);
        let g = p.layout_no_wrap(env.label().into(), FontId::monospace(9.5), theme::bg_deep());
        let r = Rect::from_min_size(Pos2::new(nx, cy - 8.0), g.size() + Vec2::new(8.0, 4.0));
        p.rect_filled(r, CornerRadius::same(3), color);
        p.galley(r.min + Vec2::new(4.0, 2.0), g, theme::bg_deep());
        nx = r.right() + 6.0;
    }
    if !h.notes.trim().is_empty() {
        let r = Rect::from_center_size(Pos2::new(nx + 7.0, cy), Vec2::splat(16.0));
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            "📝",
            FontId::proportional(11.0),
            theme::text_dim(),
        );
        ui.interact(r, ui.id().with(("notes", h.id)), Sense::hover())
            .on_hover_text(h.notes.trim());
    }

    // --- address: only what the name doesn't already say; default ports are hidden.
    let named = !h.label.trim().is_empty();
    let (user, host_part, port) = match h.protocol {
        Protocol::Serial => (
            String::new(),
            h.serial.port_name.clone(),
            format!(" {} Bd", h.serial.baud_rate),
        ),
        _ => (
            h.username.clone(),
            if named {
                h.hostname.clone()
            } else {
                String::new()
            },
            if h.port == h.protocol.default_port() {
                String::new()
            } else {
                format!(":{}", h.port)
            },
        ),
    };
    let mut ax = col_addr;
    let mut addr_part = |text: String, color: Color32| {
        if text.is_empty() {
            return;
        }
        let g = p.layout_no_wrap(text, FontId::monospace(12.5), color);
        let gw = g.size().x;
        p.galley(Pos2::new(ax, cy - g.size().y / 2.0), g, color);
        ax += gw;
    };
    let user_text = match (user.is_empty(), host_part.is_empty()) {
        (false, false) => format!("{user}@"),
        (false, true) => user,
        _ => String::new(),
    };
    addr_part(user_text, theme::text_dim());
    addr_part(host_part, theme::text_dim().linear_multiply(1.15));
    addr_part(port, protocol_color(h.protocol).linear_multiply(0.9));
    if let Some(j) = h.jump_host.and_then(|j| app.lib.host(j)) {
        addr_part(
            format!("  via {}", j.display_name()),
            theme::text_dim().linear_multiply(0.8),
        );
    }

    // --- info: protocol (if not SSH), tags, then what the server told us.
    let mut x = col_info;
    if h.protocol != Protocol::Ssh {
        chip(
            &p,
            &mut x,
            cy,
            protocol_tag(h.protocol).to_string(),
            protocol_color(h.protocol),
            false,
        );
    }
    for t in h.tags.iter().take(2) {
        chip(&p, &mut x, cy, t.clone(), theme::text_dim(), false);
    }
    // Tunnels: one switch each.
    for t in &h.tunnels {
        use crate::app::TunnelState;
        let run = app.tunnels.get(&t.id);
        let (color, tip) = match run.map(|r| &r.state) {
            Some(TunnelState::Up) => (
                theme::green(),
                t!(
                    "offen: localhost:{} → {}:{} · {} Verbindung(en)\nKlick schließt den Tunnel",
                    t.local_port,
                    t.remote_host,
                    t.remote_port,
                    run.map(|r| r.conns).unwrap_or(0)
                ),
            ),
            Some(TunnelState::Connecting) => (theme::amber(), t!("verbindet…").to_string()),
            Some(TunnelState::Failed(e)) => (theme::red(), t!("{}\nKlick startet neu", e)),
            None => (
                theme::text_dim(),
                t!(
                    "localhost:{} → {}:{}\nKlick öffnet den Tunnel",
                    t.local_port,
                    t.remote_host,
                    t.remote_port
                ),
            ),
        };
        let name = if t.name.trim().is_empty() {
            format!(":{}", t.local_port)
        } else {
            t.name.clone()
        };
        let g = p.layout_no_wrap(name, FontId::monospace(10.5), theme::text());
        let r = Rect::from_min_size(Pos2::new(x, cy - 10.0), g.size() + Vec2::new(24.0, 8.0));
        let resp_t = ui.interact(r, ui.id().with(("tunnel", t.id)), Sense::click());
        p.rect(
            r,
            CornerRadius::same(10),
            if resp_t.hovered() {
                theme::bg_card()
            } else {
                Color32::TRANSPARENT
            },
            Stroke::new(1.0, color.linear_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        theme::led(&p, Pos2::new(r.left() + 9.0, cy), color, run.is_some());
        p.galley(r.min + Vec2::new(17.0, 4.0), g, theme::text());
        x = r.right() + 6.0;
        if resp_t
            .on_hover_text(tip)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            actions.push(Action::ToggleTunnel(h.id, t.id));
        }
    }
    let facts = app.lib.facts.get(&h.id);
    if let Some(f) = facts {
        let os = f.short_os();
        let start = x;
        if !os.is_empty() {
            p.circle_filled(Pos2::new(x + 4.0, cy), 3.5, os_color(&f.os_id));
            let g = p.layout_no_wrap(os, FontId::proportional(12.5), theme::text());
            let gw = g.size().x;
            p.galley(Pos2::new(x + 13.0, cy - g.size().y / 2.0), g, theme::text());
            x += 13.0 + gw + 10.0;
        }
        let mut detail = Vec::new();
        if f.uptime_secs > 0 {
            detail.push((uptime_text(f.uptime_secs), theme::text_dim()));
        }
        if let Some(d) = f.disk_used_pct {
            let c = if d >= 90 {
                theme::red()
            } else if d >= 80 {
                theme::amber()
            } else {
                theme::text_dim()
            };
            detail.push((format!("/ {d} %"), c));
        }
        for (text, color) in detail {
            let g = p.layout_no_wrap(text, FontId::monospace(11.5), color);
            let gw = g.size().x;
            p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, color);
            x += gw + 10.0;
        }
        if let Some(n) = f.updates.filter(|n| *n > 0) {
            chip(
                &p,
                &mut x,
                cy,
                if n == 1 {
                    t!("{} Update", n)
                } else {
                    t!("{} Updates", n)
                },
                theme::amber(),
                true,
            );
        }
        let info_rect =
            Rect::from_min_max(Pos2::new(start, rect.top()), Pos2::new(x, rect.bottom()));
        let mut tip = format!("{}\nHostname: {}", f.os_name, f.hostname);
        if let Some(t) = f.collected {
            tip.push_str(&t!("\nStand: {}", relative_time(t)));
        }
        if f.updates.is_some_and(|n| n > 0) {
            tip.push_str(t!("\nRechtsklick → Updates installieren"));
        }
        ui.interact(info_rect, ui.id().with(("facts", h.id)), Sense::hover())
            .on_hover_text(tip);
    }
    if hovered && ui.input(|i| i.key_pressed(egui::Key::F2)) {
        ui.ctx()
            .data_mut(|d| d.insert_temp(rename_id(), (h.id, h.label.clone())));
    }

    // Right side: last used, or actions on hover.
    if hovered || resp.context_menu_opened() {
        // Cover whatever info sits under the buttons.
        ui.painter().rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.right() - 300.0, rect.top() + 1.0),
                rect.right_bottom() - Vec2::new(0.0, 2.0),
            ),
            CornerRadius::same(6),
            if selected {
                theme::lerp(theme::bg_hover(), theme::accent(), 0.12)
            } else {
                theme::bg_hover()
            },
        );
        let mut bx = rect.right() - 8.0;
        let mut button = |label: &str, tip: &str, action: Action, primary: bool| {
            let font = if primary {
                theme::semibold(12.5)
            } else {
                FontId::proportional(12.5)
            };
            let g = ui
                .painter()
                .layout_no_wrap(label.to_string(), font, theme::text());
            let r = Rect::from_min_size(
                Pos2::new(bx - g.size().x - 20.0, cy - 13.0),
                Vec2::new(g.size().x + 20.0, 26.0),
            );
            bx = r.left() - 6.0;
            let b = ui.interact(r, ui.id().with((h.id, label)), Sense::click());
            let (fill, fg) = if primary {
                (
                    if b.hovered() {
                        theme::lerp(theme::accent(), Color32::WHITE, 0.12)
                    } else {
                        theme::accent()
                    },
                    theme::on_accent(),
                )
            } else {
                (
                    if b.hovered() {
                        theme::bg_card()
                    } else {
                        Color32::TRANSPARENT
                    },
                    theme::text(),
                )
            };
            ui.painter().rect(
                r,
                CornerRadius::same(5),
                fill,
                Stroke::new(1.0, if primary { fill } else { theme::border() }),
                egui::StrokeKind::Inside,
            );
            ui.painter().galley(r.center() - g.size() / 2.0, g, fg);
            if b.on_hover_text(tip)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                actions.push(action);
            }
        };
        button(
            t!("Verbinden"),
            t!("Verbinden (Doppelklick)"),
            Action::Connect(h.id),
            true,
        );
        if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp) {
            button(
                t!("Dateien"),
                t!("Dateien (SFTP) öffnen"),
                Action::OpenSftp(h.id),
                false,
            );
        }
        button(
            t!("Bearbeiten"),
            t!("Host bearbeiten"),
            Action::EditHost(h.id),
            false,
        );
    } else {
        let mut parts = Vec::new();
        if let Some(Reach::Up(ms)) = reach {
            parts.push(format!("{ms} ms"));
        }
        if let Some(t) = h.last_connected {
            parts.push(relative_time(t));
        }
        p.text(
            rect.right_center() - Vec2::new(14.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            parts.join("  ·  "),
            FontId::monospace(11.5),
            theme::text_dim(),
        );
    }

    if resp.double_clicked() {
        actions.push(Action::Connect(h.id));
    }
    resp.context_menu(|ui| {
        if ui.button(t!("Verbinden")).clicked() {
            actions.push(Action::Connect(h.id));
            ui.close();
        }
        if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp)
            && ui.button(t!("Dateien (SFTP)")).clicked()
        {
            actions.push(Action::OpenSftp(h.id));
            ui.close();
        }
        ui.separator();
        if ui
            .button(if h.favorite {
                t!("Aus Favoriten entfernen")
            } else {
                t!("Zu Favoriten")
            })
            .clicked()
        {
            actions.push(Action::ToggleFavorite(h.id));
            ui.close();
        }
        if ui.button(t!("Bearbeiten")).clicked() {
            actions.push(Action::EditHost(h.id));
            ui.close();
        }
        if ui.button(t!("Umbenennen  (F2)")).clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(rename_id(), (h.id, h.label.clone())));
            ui.close();
        }
        if h.protocol == Protocol::Ssh {
            if ui
                .button(t!("SSH-Key einrichten…"))
                .on_hover_text(t!(
                    "Schlüssel auf dem Server hinterlegen – danach ohne Passwort anmelden"
                ))
                .clicked()
            {
                actions.push(Action::SetupKey(h.id));
                ui.close();
            }
            if let Some(n) = app
                .lib
                .facts
                .get(&h.id)
                .and_then(|f| f.updates)
                .filter(|n| *n > 0)
                && ui.button(t!("{} Updates installieren", n)).clicked()
            {
                actions.push(Action::RunUpdates(h.id));
                ui.close();
            }
        }
        if ui.button(t!("Duplizieren")).clicked() {
            actions.push(Action::DuplicateHost(h.id));
            ui.close();
        }
        ui.menu_button(t!("Verschieben nach"), |ui| {
            if ui.button(t!("Ohne Gruppe")).clicked() {
                actions.push(Action::MoveHost(h.id, None));
                ui.close();
            }
            for g in &app.lib.groups {
                if ui.button(&g.name).clicked() {
                    actions.push(Action::MoveHost(h.id, Some(g.id)));
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui
            .button(RichText::new(t!("Löschen")).color(theme::red()))
            .clicked()
        {
            actions.push(Action::Confirm(
                t!("Host „{}“ löschen?", h.display_name()),
                Box::new(Action::DeleteHost(h.id)),
            ));
            ui.close();
        }
    });
}
