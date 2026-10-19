//! Sidebar additions for big libraries: the active scope, saved views, pinned groups and
//! recently used hosts; plus the small sheets for saving a view and editing a badge.

use egui::{CornerRadius, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::nav_model;
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::nav_list::paint_badge;
use crate::ui::nav_state::{self, NavAction};

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            RichText::new(text.to_uppercase())
                .font(theme::semibold(11.0))
                .color(theme::text_dim()),
        );
    });
    ui.add_space(2.0);
}

/// One 28 px row: optional badge, text, optional dim count.
fn row(
    ui: &mut egui::Ui,
    badge: Option<(&str, egui::Color32)>,
    text: &str,
    extra: &str,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(7), theme::hover());
    }
    let mut x = rect.left() + 8.0;
    if let Some((b, c)) = badge {
        paint_badge(
            p,
            Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)),
            b,
            c,
        );
        x += 26.0;
    }
    let extra_w = if extra.is_empty() { 0.0 } else { 34.0 };
    let g = theme::truncated(
        p,
        text,
        theme::medium(13.0),
        theme::text(),
        rect.right() - x - 6.0 - extra_w,
    );
    p.galley(
        Pos2::new(x, rect.center().y - g.size().y / 2.0),
        g,
        theme::text(),
    );
    if !extra.is_empty() {
        p.text(
            Pos2::new(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            extra,
            theme::medium(11.5),
            theme::text_dim(),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Wide sidebar: scope, views, pinned groups, recent hosts.
pub fn sections(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let lib = &app.lib;
    if let Some(g) = lib.nav.scope.and_then(|s| lib.group(s)) {
        ui.add_space(8.0);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(15), theme::accent_soft());
        p.rect_stroke(
            rect,
            CornerRadius::same(15),
            egui::Stroke::new(1.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
        paint_badge(
            p,
            Rect::from_center_size(rect.left_center() + Vec2::new(16.0, 0.0), Vec2::splat(20.0)),
            &nav_state::badge_text(lib, g),
            theme::rgb(g.color),
        );
        let t = theme::truncated(
            p,
            g.name.clone(),
            theme::semibold(13.0),
            theme::text(),
            rect.width() - 74.0,
        );
        p.galley(
            Pos2::new(rect.left() + 32.0, rect.center().y - t.size().y / 2.0),
            t,
            theme::text(),
        );
        let x = Rect::from_center_size(
            rect.right_center() - Vec2::new(15.0, 0.0),
            Vec2::splat(20.0),
        );
        let xr = ui.interact(x, ui.id().with("rail-scope-x"), Sense::click());
        icons::paint(
            ui.painter(),
            x.shrink(4.0),
            Icon::Close,
            if xr.hovered() {
                theme::text()
            } else {
                theme::text_dim()
            },
        );
        if xr.on_hover_text(t!("Fokus beenden")).clicked() {
            actions.push(Action::Nav(NavAction::Scope(None)));
        }
    }
    let views = lib.settings.saved_views.clone();
    if !views.is_empty() {
        heading(ui, t!("Ansichten"));
        let n = views.len();
        for (i, v) in views.iter().enumerate() {
            let r = row(ui, None, &v.name, "").on_hover_text(&v.query);
            if r.clicked() {
                actions.push(Action::Nav(NavAction::ApplyView(v.id)));
            }
            r.context_menu(|ui| {
                if i > 0 && ui.button(t!("Nach oben")).clicked() {
                    actions.push(Action::Nav(NavAction::MoveView(v.id, -1)));
                    ui.close();
                }
                if i + 1 < n && ui.button(t!("Nach unten")).clicked() {
                    actions.push(Action::Nav(NavAction::MoveView(v.id, 1)));
                    ui.close();
                }
                if ui.button(t!("Ansicht löschen")).clicked() {
                    actions.push(Action::Nav(NavAction::DeleteView(v.id)));
                    ui.close();
                }
            });
        }
    }
    let mut pinned: Vec<&crate::model::Group> = lib
        .groups
        .iter()
        .filter(|g| lib.nav.is_favorite(g.id))
        .collect();
    pinned.sort_by_key(|g| g.name.to_lowercase());
    if !pinned.is_empty() {
        heading(ui, t!("Fixiert"));
        for g in pinned.into_iter().take(10) {
            let total = app
                .nav
                .cache
                .as_ref()
                .and_then(|c| c.idx.by_id.get(&g.id).map(|n| c.shown.total[*n]));
            let extra = total.map(|t| t.to_string()).unwrap_or_default();
            let r = row(
                ui,
                Some((&nav_state::badge_text(lib, g), theme::rgb(g.color))),
                &g.name,
                &extra,
            );
            if r.clicked() {
                actions.push(Action::Nav(NavAction::Reveal(g.id)));
            }
            r.context_menu(|ui| {
                if ui.button(t!("Nur diese Gruppe anzeigen")).clicked() {
                    actions.push(Action::Nav(NavAction::Scope(Some(g.id))));
                    ui.close();
                }
                if ui.button(t!("Nicht mehr fixieren")).clicked() {
                    actions.push(Action::Nav(NavAction::PinGroup(g.id, false)));
                    ui.close();
                }
            });
        }
    }
    if !app.nav.recent_hosts.is_empty() {
        heading(ui, t!("Zuletzt"));
        for id in app.nav.recent_hosts.iter().take(5) {
            let Some(h) = lib.host(*id) else { continue };
            if !nav_state::in_scope(lib, h.group_id, lib.nav.scope) {
                continue;
            }
            let r = row(ui, None, h.display_name(), "")
                .on_hover_text(t!("Verbinden mit {}", h.display_name()));
            if r.clicked() {
                actions.push(Action::Connect(h.id));
            }
            r.context_menu(|ui| {
                if ui.button(t!("In der Liste zeigen")).clicked() {
                    actions.push(Action::Nav(NavAction::RevealHost(h.id)));
                    ui.close();
                }
            });
        }
    }
}

/// Narrow rail: just a badge tile while a scope is active (click = leave it).
pub fn scope_tile(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let Some(g) = app.lib.nav.scope.and_then(|s| app.lib.group(s)) else {
        return;
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    paint_badge(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(28.0)),
        &nav_state::badge_text(&app.lib, g),
        theme::rgb(g.color),
    );
    if resp
        .on_hover_text(t!("Fokus: {} – Klick zeigt wieder alles", g.name))
        .clicked()
    {
        actions.push(Action::Nav(NavAction::Scope(None)));
    }
}

/// "Save view" and "badge" sheets.
pub fn sheets(app: &mut App, ctx: &egui::Context) {
    // The group menu asks for the badge editor through the egui memory.
    let open: Option<(Uuid, String)> =
        ctx.data_mut(|d| d.remove_temp(egui::Id::new("nav-badge-open")));
    if open.is_some() {
        app.nav.badge = open;
    }
    if app.nav.view_name.is_some() {
        let mut name = app.nav.view_name.clone().unwrap_or_default();
        let desc = {
            let q = app.search.trim();
            let mut parts = Vec::new();
            if let Some(s) = nav_state::scope_name(&app.lib) {
                parts.push(t!("Fokus: {}", s));
            }
            if !q.is_empty() {
                parts.push(t!("Suche: {}", q));
            }
            if parts.is_empty() {
                t!("Alle Hosts").to_string()
            } else {
                parts.join(" · ")
            }
        };
        let ((save, cancel), dismissed) = theme::sheet(
            ctx,
            egui::Id::new("nav-save-view"),
            420.0,
            t!("Ansicht speichern"),
            |ui| {
                ui.label(RichText::new(&desc).color(theme::text_dim()));
                ui.add_space(8.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .hint_text(t!("Name, z. B. Alle Switches bei Kunde A"))
                        .desired_width(f32::INFINITY),
                );
                r.request_focus();
                !name.trim().is_empty()
            },
            crate::ui::editors::save_cancel,
        );
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty();
        if cancel || dismissed {
            app.nav.view_name = None;
        } else if save || enter {
            app.nav.view_name = None;
            nav_state::apply(app, NavAction::SaveView(name), ctx);
        } else {
            app.nav.view_name = Some(name);
        }
    }
    if let Some((gid, mut text)) = app.nav.badge.clone() {
        let title = app
            .lib
            .group(gid)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let ((save, cancel), dismissed) = theme::sheet(
            ctx,
            egui::Id::new("nav-badge"),
            380.0,
            t!("Abzeichen ändern…"),
            |ui| {
                ui.label(RichText::new(&title).color(theme::text_dim()));
                ui.add_space(8.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .char_limit(2)
                        .hint_text(t!("1–2 Zeichen, z. B. Initialen"))
                        .desired_width(f32::INFINITY),
                );
                r.request_focus();
                ui.add_space(4.0);
                ui.label(
                    RichText::new(t!("Leer = erster Buchstabe des Namens. Die Farbe stellst du unter „Bearbeiten“ ein."))
                        .size(12.5)
                        .color(theme::text_dim()),
                );
                true
            },
            crate::ui::editors::save_cancel,
        );
        if cancel || dismissed {
            app.nav.badge = None;
        } else if save {
            app.nav.badge = None;
            nav_state::apply(app, NavAction::SetBadge(gid, text), ctx);
        } else {
            app.nav.badge = Some((gid, text));
        }
    }
    let _ = nav_model::MAX_VIEWS;
}
