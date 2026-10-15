//! Settings and known hosts.

use egui::{CornerRadius, RichText, Vec2};

use crate::app::{Action, App};
use crate::theme;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.label(RichText::new("Einstellungen").size(26.0).strong());
    ui.add_space(10.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut changed = false;

            theme::section_title(ui, "Design");
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(12.0, 12.0);
                for t in theme::THEMES {
                    if theme_card(ui, &t, app.lib.settings.theme == t.id).clicked() {
                        app.lib.settings.theme = t.id.to_string();
                        theme::apply(ui.ctx(), t.id);
                        changed = true;
                    }
                }
            });
            ui.add_space(18.0);

            theme::section_title(ui, "Terminal");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                egui::Grid::new("term-settings")
                    .num_columns(2)
                    .spacing([24.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("Schriftgröße");
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut app.lib.settings.font_size, 9.0..=28.0)
                                    .step_by(0.5),
                            )
                            .changed();
                        ui.end_row();
                        ui.label("Scrollback");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut app.lib.settings.scrollback)
                                    .range(100..=100_000)
                                    .speed(100)
                                    .suffix(" Zeilen"),
                            )
                            .changed();
                        ui.end_row();
                        ui.label("Kopieren bei Auswahl");
                        changed |= ui
                            .checkbox(&mut app.lib.settings.copy_on_select, "")
                            .changed();
                        ui.end_row();
                    });
            });
            ui.add_space(18.0);

            theme::section_title(ui, "Master-Passwort");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut app.pw_change.new)
                            .password(true)
                            .hint_text("Neu")
                            .desired_width(170.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut app.pw_change.confirm)
                            .password(true)
                            .hint_text("Wiederholen")
                            .desired_width(170.0),
                    );
                    if ui.button("Ändern").clicked() {
                        if app.pw_change.new.chars().count() < 8 {
                            app.toast("Mindestens 8 Zeichen", theme::amber());
                        } else if app.pw_change.new != app.pw_change.confirm {
                            app.toast("Passwörter stimmen nicht überein", theme::amber());
                        } else if let Some(v) = app.vault.as_mut() {
                            match v.change_password(&app.pw_change.new, &app.keychain) {
                                Ok(()) => {
                                    app.pw_change = Default::default();
                                    app.toast("Master-Passwort geändert", theme::green());
                                }
                                Err(e) => app.toast(format!("{e:#}"), theme::red()),
                            }
                        }
                    }
                });
            });
            ui.add_space(18.0);

            theme::section_title(ui, "Daten");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(app.data_dir.display().to_string())
                            .monospace()
                            .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, "⟳", "Serielle Ports neu scannen").clicked() {
                            app.serial_ports = crate::backend::serial::available_ports();
                            app.toast(
                                format!("{} serielle Ports", app.serial_ports.len()),
                                theme::teal(),
                            );
                        }
                    });
                });
            });

            if changed {
                app.save_library();
            }
        });
}

/// Clickable mini preview of a theme.
fn theme_card(ui: &mut egui::Ui, t: &theme::Palette, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(168.0, 112.0), egui::Sense::click());
    let p = ui.painter();
    let stroke = if selected {
        egui::Stroke::new(2.0, t.accent)
    } else if resp.hovered() {
        egui::Stroke::new(1.0, t.accent.linear_multiply(0.6))
    } else {
        egui::Stroke::new(1.0, theme::border())
    };
    p.rect(
        rect,
        CornerRadius::same(12),
        t.bg_deep,
        stroke,
        egui::StrokeKind::Inside,
    );
    // Mini window: sidebar + terminal lines.
    let inner = rect.shrink(10.0);
    let side = egui::Rect::from_min_size(inner.min, Vec2::new(28.0, inner.height() - 24.0));
    p.rect_filled(side, CornerRadius::same(4), t.bg_panel);
    let term = egui::Rect::from_min_max(
        side.right_top() + Vec2::new(6.0, 0.0),
        egui::pos2(inner.right(), side.bottom()),
    );
    p.rect_filled(term, CornerRadius::same(4), t.term_bg);
    for (i, w) in [0.7f32, 0.45, 0.85, 0.3].iter().enumerate() {
        let y = term.top() + 8.0 + i as f32 * 9.0;
        let color = t.ansi[[2usize, 6, 4, 5][i] + 8];
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(term.left() + 6.0, y),
                Vec2::new((term.width() - 12.0) * w, 3.0),
            ),
            CornerRadius::same(2),
            color,
        );
    }
    let bar = egui::Rect::from_min_size(
        egui::pos2(inner.left(), inner.bottom() - 16.0),
        Vec2::new(36.0, 4.0),
    );
    theme::gradient_rect(p, bar, t.accent, t.accent2, 2.0);
    p.text(
        egui::pos2(inner.left() + 44.0, inner.bottom() - 14.0),
        egui::Align2::LEFT_CENTER,
        t.name,
        egui::FontId::proportional(13.0),
        t.text,
    );
    if selected {
        p.circle_filled(
            egui::pos2(inner.right() - 6.0, inner.bottom() - 14.0),
            5.0,
            t.accent,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn known_hosts(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.label(RichText::new("Known Hosts").size(26.0).strong());
    ui.label(
        RichText::new("Server-Keys, denen du vertraust. Ändert sich ein Key, warnt Aurora vor einem möglichen Man-in-the-Middle.")
            .color(theme::text_dim()),
    );
    ui.add_space(12.0);
    let mut entries: Vec<_> = app
        .lib
        .known_hosts
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    entries.sort();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if entries.is_empty() {
                ui.label(
                    RichText::new("Noch keine Einträge — sie entstehen beim ersten Verbinden.")
                        .color(theme::text_dim()),
                );
            }
            for (host, fp) in entries {
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🛡").color(theme::green()).size(18.0));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&host).strong());
                            ui.label(
                                RichText::new(&fp)
                                    .monospace()
                                    .size(11.5)
                                    .color(theme::text_dim()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Entfernen").clicked() {
                                actions.push(Action::ForgetKnownHost(host.clone()));
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });
}
