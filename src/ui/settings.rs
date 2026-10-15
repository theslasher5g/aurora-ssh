//! Settings and known hosts.

use egui::{RichText, Vec2};

use crate::app::{Action, App};
use crate::theme;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.label(RichText::new("Einstellungen").size(26.0).strong());
    ui.add_space(10.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut changed = false;
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            theme::section_title(ui, "Terminal");
            egui::Grid::new("term-settings").num_columns(2).spacing([24.0, 12.0]).show(ui, |ui| {
                ui.label("Schriftgröße");
                changed |= ui.add(egui::Slider::new(&mut app.lib.settings.font_size, 9.0..=28.0).step_by(0.5)).changed();
                ui.end_row();
                ui.label("Scrollback-Zeilen");
                changed |= ui
                    .add(egui::DragValue::new(&mut app.lib.settings.scrollback).range(100..=100_000).speed(100))
                    .changed();
                ui.end_row();
                ui.label("Kopieren bei Auswahl");
                changed |= ui.checkbox(&mut app.lib.settings.copy_on_select, "").changed();
                ui.end_row();
                ui.label("Akzentfarbe");
                ui.horizontal(|ui| {
                    for c in theme::TAG_COLORS {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
                        ui.painter().circle_filled(rect.center(), 9.0, theme::rgb(c));
                        if app.lib.settings.accent == c {
                            ui.painter().circle_stroke(rect.center(), 11.0, egui::Stroke::new(2.0, theme::TEXT));
                        }
                        if resp.clicked() {
                            app.lib.settings.accent = c;
                            theme::apply(ui.ctx(), theme::rgb(c));
                            changed = true;
                        }
                    }
                });
                ui.end_row();
            });
        });
        ui.add_space(12.0);

        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            theme::section_title(ui, "Sicherheit");
            ui.label(RichText::new("Master-Passwort ändern (verschlüsselt die Keychain neu).").color(theme::TEXT_DIM));
            ui.add(egui::TextEdit::singleline(&mut app.pw_change.new).password(true).hint_text("Neues Passwort"));
            ui.add(egui::TextEdit::singleline(&mut app.pw_change.confirm).password(true).hint_text("Wiederholen"));
            if ui.button("🔐 Passwort ändern").clicked() {
                if app.pw_change.new.chars().count() < 8 {
                    app.toast("Mindestens 8 Zeichen", theme::AMBER);
                } else if app.pw_change.new != app.pw_change.confirm {
                    app.toast("Passwörter stimmen nicht überein", theme::AMBER);
                } else if let Some(v) = app.vault.as_mut() {
                    match v.change_password(&app.pw_change.new, &app.keychain) {
                        Ok(()) => {
                            app.pw_change = Default::default();
                            app.toast("Master-Passwort geändert", theme::GREEN);
                        }
                        Err(e) => app.toast(format!("{e:#}"), theme::RED),
                    }
                }
            }
        });
        ui.add_space(12.0);

        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            theme::section_title(ui, "Daten");
            ui.label(format!("Speicherort: {}", app.data_dir.display()));
            ui.label(RichText::new("library.json (Hosts, Gruppen, Snippets) · vault.json (verschlüsselte Keychain)").color(theme::TEXT_DIM));
            if ui.button("⟳ Serielle Ports neu scannen").clicked() {
                app.serial_ports = crate::backend::serial::available_ports();
                app.toast(format!("{} serielle Ports gefunden", app.serial_ports.len()), theme::TEAL);
            }
        });

        if changed {
            app.save_library();
        }
    });
}

pub fn known_hosts(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.label(RichText::new("Known Hosts").size(26.0).strong());
    ui.label(
        RichText::new("Server-Keys, denen du vertraust. Ändert sich ein Key, warnt Aurora vor einem möglichen Man-in-the-Middle.")
            .color(theme::TEXT_DIM),
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
                        .color(theme::TEXT_DIM),
                );
            }
            for (host, fp) in entries {
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🛡").color(theme::GREEN).size(18.0));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&host).strong());
                            ui.label(
                                RichText::new(&fp)
                                    .monospace()
                                    .size(11.5)
                                    .color(theme::TEXT_DIM),
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
