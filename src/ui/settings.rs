//! Settings and known hosts.

use egui::{CornerRadius, RichText, Vec2};

use crate::app::{Action, App};
use crate::theme;

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.label(theme::heading("Einstellungen", 24.0));
    ui.add_space(14.0);
    theme::hairline(ui);
    ui.add_space(14.0);
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
                        ui.label("Erreichbarkeit");
                        changed |= ui
                            .checkbox(
                                &mut app.lib.settings.reachability,
                                "Hosts in der Liste regelmäßig anpingen (TCP)",
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

            theme::section_title(ui, "Sicherheit & Hilfe");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                egui::Grid::new("safety-grid")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Automatisch sperren");
                        let m = app.lib.settings.auto_lock_minutes;
                        let text = |m: u32| {
                            if m == 0 {
                                "nie".to_string()
                            } else {
                                format!("nach {m} min")
                            }
                        };
                        egui::ComboBox::from_id_salt("autolock")
                            .selected_text(text(m))
                            .show_ui(ui, |ui| {
                                for v in [0, 5, 15, 30, 60, 120] {
                                    changed |= ui
                                        .selectable_value(
                                            &mut app.lib.settings.auto_lock_minutes,
                                            v,
                                            text(v),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();
                        ui.label("");
                        ui.label(
                            RichText::new("ohne Eingabe – offene Sessions laufen weiter")
                                .size(12.0)
                                .color(theme::text_dim()),
                        );
                        ui.end_row();
                        ui.label("Sessions");
                        changed |= ui
                            .checkbox(
                                &mut app.lib.settings.restore_sessions,
                                "beim Start wieder öffnen",
                            )
                            .changed();
                        ui.end_row();
                        ui.label("Editor");
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut app.lib.settings.editor)
                                    .hint_text("Standard-App (z.B. code, notepad++)")
                                    .desired_width(240.0),
                            )
                            .on_hover_text("Programm zum Bearbeiten von Server-Dateien; leer = Standard-App des Systems")
                            .changed();
                        ui.end_row();
                        ui.label("Gefährliche Befehle");
                        changed |= ui
                            .checkbox(
                                &mut app.lib.settings.warn_everywhere,
                                "auf allen Hosts nachfragen (sonst nur bei PROD)",
                            )
                            .changed();
                        ui.end_row();
                        ui.label("Hilfe");
                        ui.horizontal(|ui| {
                            if ui.button("Einführung zeigen").clicked() {
                                app.lib.settings.onboarded = false;
                                app.tour_step = 0;
                                changed = true;
                            }
                            if ui.button("Tastenkürzel").clicked() {
                                app.show_shortcuts = true;
                            }
                            if ui.button("Aus ~/.ssh/config importieren").clicked() {
                                actions.push(crate::app::Action::ImportSshConfig);
                            }
                        });
                        ui.end_row();
                    });
            });
            ui.add_space(18.0);

            theme::section_title(ui, "Export / Import");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                ui.label(
                    RichText::new(
                        "Hosts, Gruppen, Snippets und Schlüssel in eine verschlüsselte Datei – \
                         zum Umziehen auf einen anderen PC oder als Backup.",
                    )
                    .size(12.5)
                    .color(theme::text_dim()),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut app.export_path)
                        .desired_width(f32::INFINITY)
                        .font(egui::FontId::monospace(12.5)),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut app.export_pw)
                            .password(true)
                            .hint_text("Passwort für die Datei")
                            .desired_width(220.0),
                    );
                    if ui.button("Exportieren").clicked() {
                        app.export_data();
                    }
                    if ui.button("Importieren").clicked() {
                        app.import_data();
                    }
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

            updates(app, ui, &mut changed);
            ui.add_space(18.0);

            known_hosts(app, ui, actions);
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
                                theme::accent(),
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

fn updates(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    theme::section_title(ui, "Updates");
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width().min(560.0));
        ui.horizontal(|ui| {
            ui.label(theme::mono(
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                13.0,
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if app.update.installed {
                    if theme::primary_button(ui, "Neu starten").clicked() {
                        crate::update::restart();
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else if let Some(rel) = app.update.latest.clone() {
                    let kind = crate::update::InstallKind::detect();
                    if app.update.installing {
                        ui.label(
                            theme::mono(format!("{:.0} %", app.update.progress * 100.0), 12.5)
                                .color(theme::accent()),
                        );
                        ui.spinner();
                    } else if kind.can_self_update() {
                        if theme::primary_button(
                            ui,
                            &format!("Auf {} aktualisieren", rel.version_text()),
                        )
                        .clicked()
                        {
                            app.install_update();
                        }
                    } else if theme::primary_button(ui, "Download-Seite öffnen").clicked() {
                        crate::update::open_url(&rel.page);
                    }
                    if ui.button("Änderungen").clicked() {
                        crate::update::open_url(&rel.page);
                    }
                } else if app.update.checking {
                    ui.spinner();
                } else if ui.button("Jetzt prüfen").clicked() {
                    app.check_updates();
                }
            });
        });
        if let Some(rel) = &app.update.latest
            && !app.update.installed
        {
            ui.label(
                RichText::new(format!("Neue Version {} verfügbar.", rel.version_text()))
                    .color(theme::accent()),
            );
        }
        if let Some((msg, color)) = &app.update.message {
            ui.label(RichText::new(msg).size(12.5).color(*color));
        }
        *changed |= ui
            .checkbox(
                &mut app.lib.settings.check_updates,
                "Beim Start automatisch nach Updates suchen",
            )
            .changed();
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
        CornerRadius::same(8),
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
    p.circle_filled(
        egui::pos2(inner.left() + 5.0, inner.bottom() - 9.0),
        4.0,
        t.accent,
    );
    p.text(
        egui::pos2(inner.left() + 16.0, inner.bottom() - 9.0),
        egui::Align2::LEFT_CENTER,
        t.name,
        theme::semibold(13.0),
        t.text,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Trusted server keys (TOFU); a changed key triggers a MITM warning on connect.
fn known_hosts(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    theme::section_title(ui, "Vertrauenswürdige Server");
    let mut entries: Vec<_> = app
        .lib
        .known_hosts
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    entries.sort();
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width().min(560.0));
        if entries.is_empty() {
            ui.label(RichText::new("Noch keine").color(theme::text_dim()));
        }
        for (host, fp) in entries {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::mono(&host, 12.5).color(theme::text()));
                    ui.add(
                        egui::Label::new(
                            RichText::new(&fp)
                                .monospace()
                                .size(11.0)
                                .color(theme::text_dim()),
                        )
                        .truncate(),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, "×", "Entfernen").clicked() {
                        actions.push(Action::ForgetKnownHost(host.clone()));
                    }
                });
            });
        }
    });
}
