//! Blocking prompts: host key verification, password, snippet variables, confirmations.

use egui::{RichText, Vec2};

use crate::app::{App, Purpose};
use crate::model::Identity;
use crate::theme;

pub fn show(app: &mut App, ctx: &egui::Context) {
    hostkey(app, ctx);
    password(app, ctx);
    snippet_vars(app, ctx);
    confirm(app, ctx);
}

fn hostkey(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.hostkey_queue.front() else {
        return;
    };
    let changed = p.previous.is_some();
    let mut answer: Option<(bool, bool)> = None; // (accept, remember)
    egui::Modal::new(egui::Id::new("hostkey")).show(ctx, |ui| {
        ui.set_width(500.0);
        if changed {
            ui.label(RichText::new("⚠ Server-Key hat sich geändert!").size(20.0).strong().color(theme::RED));
            ui.label(
                RichText::new("Das kann eine Neuinstallation sein — oder ein Man-in-the-Middle-Angriff. Verbinde nur, wenn du den neuen Key verifiziert hast.")
                    .color(theme::TEXT_DIM),
            );
        } else {
            ui.label(RichText::new("🛡 Unbekannter Host").size(20.0).strong());
            ui.label(RichText::new("Prüfe den Fingerprint, bevor du dem Server vertraust.").color(theme::TEXT_DIM));
        }
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&p.host).strong());
            ui.label(RichText::new(&p.algorithm).color(theme::TEXT_DIM));
            ui.label(RichText::new(&p.fingerprint).monospace().color(theme::TEAL));
            if let Some(prev) = &p.previous {
                ui.label(RichText::new(format!("vorher: {prev}")).monospace().size(11.5).color(theme::RED));
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "Vertrauen & speichern").clicked() {
                    answer = Some((true, true));
                }
                if ui.button("Nur dieses Mal").clicked() {
                    answer = Some((true, false));
                }
                if ui.button("Abbrechen").clicked() {
                    answer = Some((false, false));
                }
            });
        });
    });
    if let Some((accept, remember)) = answer
        && let Some(p) = app.hostkey_queue.pop_front()
    {
        if accept && remember {
            app.lib
                .known_hosts
                .insert(p.host.clone(), p.fingerprint.clone());
            app.save_library();
        }
        let _ = p.reply.send(accept);
    }
}

fn password(app: &mut App, ctx: &egui::Context) {
    let Some(host) = app.password_prompt.as_ref().map(|p| p.host.clone()) else {
        return;
    };
    let (user, _) = app.credentials(&host);
    let in_library = app.lib.host(host.id).is_some();
    let (mut submit, mut cancel) = (false, false);
    let Some(pp) = app.password_prompt.as_mut() else {
        return;
    };
    egui::Modal::new(egui::Id::new("password")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.horizontal(|ui| {
            theme::avatar(ui, 40.0, theme::TEAL, "🔒");
            ui.vertical(|ui| {
                ui.label(RichText::new("Passwort").size(18.0).strong());
                ui.label(
                    RichText::new(format!("{user}@{}:{}", pp.host.hostname, pp.host.port))
                        .color(theme::TEXT_DIM),
                );
            });
        });
        ui.add_space(8.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut pp.password)
                .password(true)
                .hint_text("Passwort")
                .desired_width(f32::INFINITY),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            submit = true;
        } else {
            r.request_focus();
        }
        if in_library {
            ui.checkbox(&mut pp.save, "Im Keychain speichern");
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "Verbinden").clicked() {
                    submit = true;
                }
                if ui.button("Abbrechen").clicked() {
                    cancel = true;
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
    });
    if cancel {
        app.password_prompt = None;
    } else if submit && let Some(pp) = app.password_prompt.take() {
        let (user, ident) = app.credentials(&pp.host);
        if pp.save {
            let mut id = Identity::new();
            id.label = format!("{user}@{}", pp.host.hostname);
            id.username = user.clone();
            id.password = pp.password.clone();
            if let Some(h) = app.lib.hosts.iter_mut().find(|h| h.id == pp.host.id) {
                h.identity_id = Some(id.id);
            }
            app.keychain.identities.push(id);
            app.save_keychain();
            app.save_library();
        }
        let purpose: Purpose = pp.purpose;
        app.open_session(&pp.host, purpose, user, Some(pp.password), ident, ctx);
    }
}

fn snippet_vars(app: &mut App, ctx: &egui::Context) {
    let Some(run) = app.snippet_run.as_mut() else {
        return;
    };
    let (mut submit, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("snippet-vars")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(
            RichText::new(format!("⚡ {}", run.snippet.name))
                .size(18.0)
                .strong(),
        );
        ui.label(RichText::new("Werte für die Platzhalter eingeben").color(theme::TEXT_DIM));
        ui.add_space(6.0);
        egui::Grid::new("vars")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for (i, (name, value)) in run.values.iter_mut().enumerate() {
                    ui.label(
                        RichText::new(format!("{{{{{name}}}}}"))
                            .monospace()
                            .color(theme::VIOLET),
                    );
                    let r = ui.add(egui::TextEdit::singleline(value).desired_width(260.0));
                    if i == 0 && !ui.memory(|m| m.focused().is_some()) {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    ui.end_row();
                }
            });
        let values = run.values.iter().cloned().collect();
        let preview = run.snippet.render(&values);
        egui::Frame::NONE
            .fill(theme::BG_DEEP)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(preview).monospace().color(theme::TEAL));
            });
        ui.add_space(Vec2::splat(4.0).y);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(
                    ui,
                    if run.all {
                        "▶ In allen ausführen"
                    } else {
                        "▶ Ausführen"
                    },
                )
                .clicked()
                {
                    submit = true;
                }
                if ui.button("Abbrechen").clicked() {
                    cancel = true;
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
    });
    if cancel {
        app.snippet_run = None;
    } else if submit && let Some(run) = app.snippet_run.take() {
        let values = run.values.into_iter().collect();
        let text = run.snippet.render(&values);
        app.run_snippet_text(&text, run.snippet.auto_run, run.all);
    }
}

fn confirm(app: &mut App, ctx: &egui::Context) {
    let Some((text, _)) = app.confirm_delete.as_ref() else {
        return;
    };
    let text = text.clone();
    let mut answer = None;
    egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(RichText::new("Bist du sicher?").size(18.0).strong());
        ui.label(RichText::new(text).color(theme::TEXT_DIM));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Löschen").color(theme::BG_DEEP))
                            .fill(theme::RED),
                    )
                    .clicked()
                {
                    answer = Some(true);
                }
                if ui.button("Abbrechen").clicked() {
                    answer = Some(false);
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
    });
    if let Some(yes) = answer
        && let Some((_, action)) = app.confirm_delete.take()
        && yes
    {
        app.apply(action, ctx);
    }
}
