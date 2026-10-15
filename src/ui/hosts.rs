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
        ui.label(theme::heading("Verbindungen", 24.0));
        ui.label(theme::mono(app.lib.hosts.len().to_string(), 13.0).color(theme::text_dim()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new("Neu").font(theme::semibold(13.5)), |ui| {
                ui.set_min_width(190.0);
                for (label, proto) in [
                    ("SSH-Host", Protocol::Ssh),
                    ("Remote Desktop", Protocol::Rdp),
                    ("Serielles Gerät", Protocol::Serial),
                ] {
                    if ui.button(label).clicked() {
                        actions.push(Action::NewHostOf(proto, None));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Gruppe").clicked() {
                    actions.push(Action::NewGroup(None));
                    ui.close();
                }
            });
            let search = ui.add(
                egui::TextEdit::singleline(&mut app.search)
                    .hint_text("Suchen oder user@host verbinden")
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

    if let Some(i) = theme::segmented(
        ui,
        &["Alle", "SSH", "RDP", "Seriell", "Favoriten"],
        app.host_filter,
    ) {
        app.host_filter = i;
    }
    theme::hairline(ui);
    ui.add_space(6.0);

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
                    ui.label(
                        RichText::new("Keine Treffer. Tipp: user@host eingeben und Enter drücken.")
                            .color(theme::text_dim()),
                    );
                }
                for h in &hosts {
                    host_row(app, ui, h, &live, actions);
                }
                return;
            }

            group_tree(app, ui, None, 0, &live, actions);
            let loose: Vec<Host> = app
                .lib
                .hosts
                .iter()
                .filter(|h| h.group_id.is_none_or(|g| app.lib.group(g).is_none()))
                .filter(|h| passes_filter(app.host_filter, h))
                .cloned()
                .collect();
            if !loose.is_empty() {
                if !app.lib.groups.is_empty() {
                    ui.add_space(14.0);
                    theme::section_title(ui, "Ohne Gruppe");
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
        .map(|t| t.title.clone())
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
            ui.label(theme::heading("Noch nichts eingesteckt", 22.0));
            ui.add_space(2.0);
            ui.label(
                RichText::new("Lege deine erste Verbindung an. Für einen schnellen Test reicht user@host oben im Suchfeld.")
                    .color(theme::text_dim()),
            );
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "SSH-Host anlegen").clicked() {
                    actions.push(Action::NewHostOf(Protocol::Ssh, None));
                }
                if ui.button("Remote Desktop").clicked() {
                    actions.push(Action::NewHostOf(Protocol::Rdp, None));
                }
                if ui.button("Serielles Gerät").clicked() {
                    actions.push(Action::NewHostOf(Protocol::Serial, None));
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
        "Direkt verbinden",
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
        let hosts: Vec<Host> = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id == Some(g.id) && passes_filter(app.host_filter, h))
            .cloned()
            .collect();
        let total = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id.is_some_and(|hg| app.lib.is_descendant(hg, g.id)))
            .count();
        ui.add_space(if depth == 0 { 14.0 } else { 6.0 });
        let indent = depth as f32 * 16.0;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
        let p = ui.painter();
        let x = rect.left() + indent;
        let c = Pos2::new(x + 4.0, rect.center().y);
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
        p.rect_filled(
            Rect::from_center_size(Pos2::new(x + 18.0, rect.center().y), Vec2::splat(8.0)),
            CornerRadius::same(2),
            theme::rgb(g.color),
        );
        let label = theme::spaced(&g.name.to_uppercase());
        let galley = p.layout_no_wrap(label, FontId::monospace(11.0), theme::text());
        let w = galley.size().x;
        p.galley(
            Pos2::new(x + 30.0, rect.center().y - galley.size().y / 2.0),
            galley,
            theme::text(),
        );
        p.text(
            Pos2::new(x + 40.0 + w, rect.center().y),
            egui::Align2::LEFT_CENTER,
            total.to_string(),
            FontId::monospace(11.0),
            theme::text_dim(),
        );
        if resp.hovered() {
            p.text(
                rect.right_center() - Vec2::new(8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                "+ Host",
                FontId::proportional(12.0),
                theme::text_dim(),
            );
        }
        let plus_rect =
            Rect::from_min_max(rect.right_top() - Vec2::new(70.0, 0.0), rect.right_bottom());
        let clicked_plus = resp.clicked()
            && ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| plus_rect.contains(p));
        if clicked_plus {
            actions.push(Action::NewHost(Some(g.id)));
        } else if resp.clicked() {
            actions.push(Action::ToggleGroup(g.id));
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .context_menu(|ui| {
                if ui.button("Host hinzufügen").clicked() {
                    actions.push(Action::NewHost(Some(g.id)));
                    ui.close();
                }
                if ui.button("Untergruppe").clicked() {
                    actions.push(Action::NewGroup(Some(g.id)));
                    ui.close();
                }
                if ui.button("Bearbeiten").clicked() {
                    actions.push(Action::EditGroup(g.id));
                    ui.close();
                }
                if ui
                    .button(RichText::new("Löschen").color(theme::red()))
                    .clicked()
                {
                    actions.push(Action::Confirm(
                        format!("Gruppe „{}“ löschen? Die Hosts bleiben erhalten.", g.name),
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

fn host_row(app: &App, ui: &mut egui::Ui, h: &Host, live: &[String], actions: &mut Vec<Action>) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let p = ui.painter().with_clip_rect(rect);
    if hovered {
        p.rect_filled(rect, CornerRadius::same(6), theme::bg_hover());
    }
    // bottom rule
    p.rect_filled(
        Rect::from_min_max(
            Pos2::new(rect.left() + 10.0, rect.bottom() - 1.0),
            rect.right_bottom(),
        ),
        CornerRadius::ZERO,
        theme::border().linear_multiply(0.6),
    );

    let cy = rect.center().y;
    // LED: lit = open session, green = reachable, red = no answer, grey = unknown.
    let is_live = live.iter().any(|t| t == h.display_name());
    let reach = app
        .lib
        .settings
        .reachability
        .then(|| app.reach.get(h.id))
        .flatten();
    let led_color = match (is_live, reach) {
        (true, _) | (false, Some(Reach::Up(_))) => theme::green(),
        (false, Some(Reach::Down)) => theme::red(),
        (false, None) => theme::text_dim(),
    };
    let led_pos = Pos2::new(rect.left() + 14.0, cy);
    theme::led(&p, led_pos, led_color, is_live);
    if let Some(r) = reach {
        let tip_rect = Rect::from_center_size(led_pos, Vec2::splat(18.0));
        let tip = match r {
            Reach::Up(ms) => format!("erreichbar · {ms} ms"),
            Reach::Down => "keine Antwort auf Port".to_string(),
        };
        ui.interact(tip_rect, ui.id().with(("reach", h.id)), Sense::hover())
            .on_hover_text(tip);
    }

    let w = rect.width();
    let col_name = rect.left() + 30.0;
    let col_addr = rect.left() + (w * 0.30).max(200.0);
    let col_proto = rect.left() + (w * 0.62).max(470.0);
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
    if h.favorite {
        p.text(
            Pos2::new(col_name + name_w + 8.0, cy),
            egui::Align2::LEFT_CENTER,
            "★",
            FontId::proportional(11.0),
            theme::accent(),
        );
    }

    // Address: user@host in dim, port in accent.
    let (addr, port) = match h.protocol {
        Protocol::Serial => (
            h.serial.port_name.clone(),
            format!("{} Bd", h.serial.baud_rate),
        ),
        _ => {
            let user = if h.username.is_empty() {
                String::new()
            } else {
                format!("{}@", h.username)
            };
            (format!("{user}{}", h.hostname), format!(":{}", h.port))
        }
    };
    let addr_g = p.layout_no_wrap(addr, FontId::monospace(12.5), theme::text_dim());
    let aw = addr_g.size().x;
    p.galley(
        Pos2::new(col_addr, cy - addr_g.size().y / 2.0),
        addr_g,
        theme::text_dim(),
    );
    p.text(
        Pos2::new(
            col_addr
                + aw
                + if h.protocol == Protocol::Serial {
                    8.0
                } else {
                    0.0
                },
            cy,
        ),
        egui::Align2::LEFT_CENTER,
        port,
        FontId::monospace(12.5),
        protocol_color(h.protocol).linear_multiply(0.9),
    );

    // Protocol + tags.
    let mut x = col_proto;
    let pc = protocol_color(h.protocol);
    for (i, label) in std::iter::once(protocol_tag(h.protocol).to_string())
        .chain(h.tags.iter().take(2).cloned())
        .enumerate()
    {
        let color = if i == 0 { pc } else { theme::text_dim() };
        let g = p.layout_no_wrap(label, FontId::monospace(10.5), color);
        let r = Rect::from_min_size(Pos2::new(x, cy - 9.0), g.size() + Vec2::new(10.0, 6.0));
        p.rect_stroke(
            r,
            CornerRadius::same(3),
            Stroke::new(1.0, color.linear_multiply(0.4)),
            egui::StrokeKind::Inside,
        );
        p.galley(r.min + Vec2::new(5.0, 3.0), g, color);
        x = r.right() + 6.0;
    }

    // Right side: last used, or actions on hover.
    if hovered || resp.context_menu_opened() {
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
            "Verbinden",
            "Verbinden (Doppelklick)",
            Action::Connect(h.id),
            true,
        );
        if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp) {
            button(
                "Dateien",
                "Dateien (SFTP) öffnen",
                Action::OpenSftp(h.id),
                false,
            );
        }
        button(
            "Bearbeiten",
            "Host bearbeiten",
            Action::EditHost(h.id),
            false,
        );
    } else if let Some(t) = h.last_connected {
        p.text(
            rect.right_center() - Vec2::new(14.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            relative_time(t),
            FontId::monospace(11.5),
            theme::text_dim(),
        );
    }

    if resp.double_clicked() {
        actions.push(Action::Connect(h.id));
    }
    resp.context_menu(|ui| {
        if ui.button("Verbinden").clicked() {
            actions.push(Action::Connect(h.id));
            ui.close();
        }
        if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp)
            && ui.button("Dateien (SFTP)").clicked()
        {
            actions.push(Action::OpenSftp(h.id));
            ui.close();
        }
        ui.separator();
        if ui
            .button(if h.favorite {
                "Aus Favoriten entfernen"
            } else {
                "Zu Favoriten"
            })
            .clicked()
        {
            actions.push(Action::ToggleFavorite(h.id));
            ui.close();
        }
        if ui.button("Bearbeiten").clicked() {
            actions.push(Action::EditHost(h.id));
            ui.close();
        }
        if ui.button("Duplizieren").clicked() {
            actions.push(Action::DuplicateHost(h.id));
            ui.close();
        }
        ui.menu_button("Verschieben nach", |ui| {
            if ui.button("Ohne Gruppe").clicked() {
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
            .button(RichText::new("Löschen").color(theme::red()))
            .clicked()
        {
            actions.push(Action::Confirm(
                format!("Host „{}“ löschen?", h.display_name()),
                Box::new(Action::DeleteHost(h.id)),
            ));
            ui.close();
        }
    });
}
