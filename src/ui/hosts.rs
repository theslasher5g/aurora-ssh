//! Host library: quick connect, favorites, recents and the group tree.

use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::model::{Host, Protocol};
use crate::theme;
use crate::ui::{fuzzy, relative_time};

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    // Header
    ui.horizontal(|ui| {
        ui.label(RichText::new("Hosts").size(26.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::primary_button(ui, "➕ Host").clicked() {
                actions.push(Action::NewHost(None));
            }
            if ui.button("🗀 Gruppe").clicked() {
                actions.push(Action::NewGroup(None));
            }
            ui.add(
                egui::TextEdit::singleline(&mut app.search)
                    .hint_text("🔍 Suchen… (group:Name)")
                    .desired_width(240.0),
            );
        });
    });
    ui.add_space(6.0);

    // Quick connect bar
    egui::Frame::NONE
        .fill(theme::BG_CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("⚡").color(theme::TEAL).size(16.0));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.quick_connect)
                        .hint_text("Quick Connect — user@host:port")
                        .frame(egui::Frame::NONE)
                        .desired_width(ui.available_width() - 110.0),
                );
                let go = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.button("Verbinden").clicked();
                if go && !app.quick_connect.trim().is_empty() {
                    actions.push(Action::QuickConnect(app.quick_connect.trim().to_string()));
                    app.quick_connect.clear();
                }
            });
        });
    ui.add_space(12.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let query = app.search.trim().to_string();
            if !query.is_empty() {
                let hosts: Vec<Host> = filter_hosts(app, &query);
                theme::section_title(ui, &format!("{} Treffer", hosts.len()));
                ui.add_space(4.0);
                host_grid(app, ui, &hosts, actions);
                return;
            }

            let favorites: Vec<Host> = app
                .lib
                .hosts
                .iter()
                .filter(|h| h.favorite)
                .cloned()
                .collect();
            if !favorites.is_empty() {
                theme::section_title(ui, "★ Favoriten");
                ui.add_space(4.0);
                host_grid(app, ui, &favorites, actions);
                ui.add_space(14.0);
            }

            let mut recent: Vec<Host> = app
                .lib
                .hosts
                .iter()
                .filter(|h| h.last_connected.is_some())
                .cloned()
                .collect();
            recent.sort_by_key(|h| std::cmp::Reverse(h.last_connected));
            recent.truncate(4);
            if !recent.is_empty() {
                theme::section_title(ui, "🕘 Zuletzt verbunden");
                ui.add_space(4.0);
                host_grid(app, ui, &recent, actions);
                ui.add_space(14.0);
            }

            group_tree(app, ui, None, 0, actions);

            let loose: Vec<Host> = app
                .lib
                .hosts
                .iter()
                .filter(|h| h.group_id.is_none_or(|g| app.lib.group(g).is_none()))
                .cloned()
                .collect();
            if !loose.is_empty() {
                ui.add_space(8.0);
                theme::section_title(ui, "Ohne Gruppe");
                ui.add_space(4.0);
                host_grid(app, ui, &loose, actions);
            }

            if app.lib.hosts.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Noch keine Hosts")
                            .size(18.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        RichText::new("Lege deinen ersten Host an oder nutze Quick Connect.")
                            .color(theme::TEXT_DIM),
                    );
                });
            }
        });
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
            .filter(|h| h.group_id == Some(g.id))
            .cloned()
            .collect();
        let total = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id.is_some_and(|hg| app.lib.is_descendant(hg, g.id)))
            .count();
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 18.0);
            let color = theme::rgb(g.color);
            let header = ui.horizontal(|ui| {
                let chevron = if g.collapsed { "⏵" } else { "⏷" };
                ui.label(RichText::new(chevron).size(10.0).color(theme::TEXT_DIM));
                let (dot, _) = ui.allocate_exact_size(Vec2::new(4.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(dot, CornerRadius::same(2), color);
                ui.label(
                    RichText::new(format!("{} {}", g.icon, g.name))
                        .size(15.0)
                        .strong(),
                );
                theme::chip(ui, &total.to_string(), color);
            });
            let resp = ui.interact(
                header.response.rect,
                ui.id().with(("grp", g.id)),
                egui::Sense::click(),
            );
            if resp.clicked() {
                actions.push(Action::ToggleGroup(g.id));
            }
            resp.context_menu(|ui| {
                if ui.button("➕ Host in Gruppe").clicked() {
                    actions.push(Action::NewHost(Some(g.id)));
                    ui.close();
                }
                if ui.button("🗀 Untergruppe").clicked() {
                    actions.push(Action::NewGroup(Some(g.id)));
                    ui.close();
                }
                if ui.button("✏ Bearbeiten").clicked() {
                    actions.push(Action::EditGroup(g.id));
                    ui.close();
                }
                if ui
                    .button(RichText::new("🗑 Löschen").color(theme::RED))
                    .clicked()
                {
                    actions.push(Action::Confirm(
                        format!(
                            "Gruppe „{}“ löschen? Hosts wandern in die übergeordnete Gruppe.",
                            g.name
                        ),
                        Box::new(Action::DeleteGroup(g.id)),
                    ));
                    ui.close();
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("✏")
                    .on_hover_text("Gruppe bearbeiten")
                    .clicked()
                {
                    actions.push(Action::EditGroup(g.id));
                }
                if ui
                    .small_button("➕")
                    .on_hover_text("Host hinzufügen")
                    .clicked()
                {
                    actions.push(Action::NewHost(Some(g.id)));
                }
            });
        });
        if !g.collapsed {
            ui.add_space(4.0);
            if !hosts.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 18.0 + 14.0);
                    ui.vertical(|ui| host_grid(app, ui, &hosts, actions));
                });
            }
            group_tree(app, ui, Some(g.id), depth + 1, actions);
        }
        ui.add_space(10.0);
    }
}

fn host_grid(app: &App, ui: &mut egui::Ui, hosts: &[Host], actions: &mut Vec<Action>) {
    let spacing = 12.0;
    let avail = ui.available_width() - 10.0; // leave room for the floating scroll bar
    let cols = ((avail + spacing) / (290.0 + spacing)).floor().max(1.0) as usize;
    let w = (avail - spacing * (cols as f32 - 1.0)) / cols as f32;
    for row in hosts.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            for h in row {
                host_card(app, ui, h, w, actions);
            }
        });
        ui.add_space(spacing - ui.spacing().item_spacing.y);
    }
}

fn host_card(app: &App, ui: &mut egui::Ui, h: &Host, width: f32, actions: &mut Vec<Action>) {
    let color = h
        .color
        .map(theme::rgb)
        .or_else(|| {
            h.group_id
                .and_then(|g| app.lib.group(g))
                .map(|g| theme::rgb(g.color))
        })
        .unwrap_or_else(|| theme::rgb(theme::hash_color(&h.hostname)));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 88.0), egui::Sense::click());
    // The hover buttons sit on top of the card, so test the pointer position rather than `resp.hovered()`.
    let hovered = ui.rect_contains_pointer(rect);
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(12),
        if hovered {
            theme::BG_HOVER
        } else {
            theme::BG_CARD
        },
        Stroke::new(
            1.0,
            if hovered {
                color.linear_multiply(0.7)
            } else {
                theme::BORDER
            },
        ),
        egui::StrokeKind::Inside,
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 10.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    theme::avatar(&mut child, 44.0, color, h.protocol.icon());
    child.add_space(4.0);
    child.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(RichText::new(h.display_name()).strong().size(14.5)).truncate(),
            );
            if h.favorite {
                ui.label(RichText::new("★").color(theme::AMBER).size(12.0));
            }
        });
        ui.add(
            egui::Label::new(
                RichText::new(h.subtitle())
                    .color(theme::TEXT_DIM)
                    .size(12.0),
            )
            .truncate(),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let pc = match h.protocol {
                Protocol::Ssh => theme::TEAL,
                Protocol::Rdp => Color32::from_rgb(56, 189, 248),
                Protocol::Serial => theme::AMBER,
            };
            theme::chip(ui, h.protocol.label(), pc);
            for t in h.tags.iter().take(2) {
                theme::chip(ui, t, theme::rgb(theme::hash_color(t)));
            }
            if let Some(t) = h.last_connected {
                ui.label(
                    RichText::new(relative_time(t))
                        .size(10.5)
                        .color(theme::TEXT_DIM),
                );
            }
        });
    });

    // Hover actions
    if hovered || resp.context_menu_opened() {
        let mut x = rect.right() - 12.0;
        let mut btn = |glyph: &str, tip: &str, action: Action| {
            let r = egui::Rect::from_min_size(
                egui::pos2(x - 28.0, rect.top() + 8.0),
                Vec2::splat(28.0),
            );
            x -= 32.0;
            let b = ui.interact(r, ui.id().with((h.id, glyph)), egui::Sense::click());
            ui.painter().rect_filled(
                r,
                CornerRadius::same(8),
                if b.hovered() {
                    color.linear_multiply(0.35)
                } else {
                    theme::BG_DEEP
                },
            );
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(13.0),
                theme::TEXT,
            );
            if b.on_hover_text(tip).clicked() {
                actions.push(action);
            }
        };
        btn("✏", "Bearbeiten", Action::EditHost(h.id));
        if h.protocol == Protocol::Ssh {
            btn("🗁", "SFTP öffnen", Action::OpenSftp(h.id));
        }
        btn("▶", "Verbinden", Action::Connect(h.id));
    }

    if resp.double_clicked() {
        actions.push(Action::Connect(h.id));
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.context_menu(|ui| {
        if ui.button("▶ Verbinden").clicked() {
            actions.push(Action::Connect(h.id));
            ui.close();
        }
        if h.protocol == Protocol::Ssh && ui.button("🗁 SFTP").clicked() {
            actions.push(Action::OpenSftp(h.id));
            ui.close();
        }
        ui.separator();
        if ui
            .button(if h.favorite {
                "☆ Kein Favorit"
            } else {
                "★ Favorit"
            })
            .clicked()
        {
            actions.push(Action::ToggleFavorite(h.id));
            ui.close();
        }
        if ui.button("✏ Bearbeiten").clicked() {
            actions.push(Action::EditHost(h.id));
            ui.close();
        }
        if ui.button("🗐 Duplizieren").clicked() {
            actions.push(Action::DuplicateHost(h.id));
            ui.close();
        }
        ui.menu_button("🗀 Verschieben nach", |ui| {
            if ui.button("— Ohne Gruppe —").clicked() {
                actions.push(Action::MoveHost(h.id, None));
                ui.close();
            }
            for g in &app.lib.groups {
                if ui.button(format!("{} {}", g.icon, g.name)).clicked() {
                    actions.push(Action::MoveHost(h.id, Some(g.id)));
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui
            .button(RichText::new("🗑 Löschen").color(theme::RED))
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
