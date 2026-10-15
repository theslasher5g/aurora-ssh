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
    danger(app, ctx);
    shortcuts(app, ctx);
    tour(app, ctx);
}

/// Enter on a dangerous command in a guarded (PROD) session.
fn danger(app: &mut App, ctx: &egui::Context) {
    let Some((tab, cmd, why)) = app.danger.clone() else {
        return;
    };
    let host = app
        .tabs
        .iter()
        .find(|t| t.id == tab)
        .map(|t| (t.title.clone(), t.env))
        .unwrap_or_default();
    let mut answer = None;
    egui::Modal::new(egui::Id::new("danger")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            if host.1 != crate::model::Env::None {
                env_badge(ui, host.1);
            }
            ui.label(theme::heading(
                format!("Wirklich auf {} ausführen?", host.0),
                19.0,
            ));
        });
        ui.label(RichText::new(format!("Dieser Befehl {why}.")).color(theme::text_dim()));
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::mono(&cmd, 13.5).color(theme::red()));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Ausführen").color(theme::bg_deep()))
                            .fill(theme::red()),
                    )
                    .clicked()
                {
                    answer = Some(true);
                }
                if theme::primary_button(ui, "Abbrechen").clicked() {
                    answer = Some(false);
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
    });
    if let Some(run) = answer {
        app.danger = None;
        // Give the keyboard back to the terminal (next frame).
        app.focus_term = Some(tab);
        if run {
            app.send_input(tab, b"\r".to_vec());
        }
    }
}

/// Small coloured environment label (PROD, STAGING …).
pub fn env_badge(ui: &mut egui::Ui, env: crate::model::Env) {
    let color = theme::env_color(env);
    let text = env.label();
    let galley =
        ui.painter()
            .layout_no_wrap(text.into(), egui::FontId::monospace(10.5), theme::bg_deep());
    let (rect, _) =
        ui.allocate_exact_size(galley.size() + Vec2::new(10.0, 4.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(4), color);
    ui.painter()
        .galley(rect.min + Vec2::new(5.0, 2.0), galley, theme::bg_deep());
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+K", "Suchen, Befehle, user@host verbinden"),
    ("Ctrl+1 … 9", "Zu Session 1 … 9 wechseln"),
    ("Ctrl+0", "Startseite"),
    ("Ctrl+Tab", "Nächste Session"),
    ("Ctrl+Shift+W", "Session schließen"),
    ("Ctrl+Shift+F", "Im Terminal suchen"),
    ("Ctrl+Mausrad", "Schriftgröße des Terminals"),
    ("Ctrl+Shift+C / V", "Kopieren / Einfügen im Terminal"),
    ("Enter", "Getrennte Session neu verbinden"),
    ("Doppelklick Kopf", "Pane im Splitscreen maximieren"),
    ("Port ziehen", "Session neben eine andere legen"),
    ("Mittelklick Port", "Session schließen"),
    ("F1  oder  ?", "Diese Übersicht"),
];

fn shortcuts(app: &mut App, ctx: &egui::Context) {
    if !app.show_shortcuts {
        return;
    }
    let modal = egui::Modal::new(egui::Id::new("shortcuts")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(theme::heading("Tastenkürzel", 20.0));
        ui.add_space(8.0);
        egui::Grid::new("shortcut-grid")
            .num_columns(2)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                for (keys, what) in SHORTCUTS {
                    ui.label(theme::mono(*keys, 12.5).color(theme::accent()));
                    ui.label(RichText::new(*what).size(13.0));
                    ui.end_row();
                }
            });
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Schließen").clicked() {
                app.show_shortcuts = false;
            }
        });
    });
    if modal.should_close() {
        app.show_shortcuts = false;
    }
}

const TOUR: &[(&str, &str, &str)] = &[
    (
        "Willkommen bei Aurora",
        "Hier verwaltest du deine Server. Leg neue Hosts an oder übernimm sie aus deiner ~/.ssh/config.",
        "1",
    ),
    (
        "Schnell verbinden",
        "Ctrl+K öffnet die Suche. Tippe einen Hostnamen oder user@host und drück Enter. Unbekannte Ziele kannst du danach mit einem Klick speichern.",
        "2",
    ),
    (
        "Nebeneinander arbeiten",
        "Ziehe Sessions aus der linken Leiste ins Fenster – bis zu vier nebeneinander. Dateien vom Desktop ziehst du einfach aufs Terminal, sie werden hochgeladen.",
        "3",
    ),
    (
        "Sicher bleiben",
        "Markiere Server als PROD: Sie bekommen einen roten Rahmen und Aurora fragt bei Befehlen wie rm -rf oder reboot nach. Mit ? siehst du jederzeit alle Tastenkürzel.",
        "4",
    ),
];

/// Short tour on the first start.
fn tour(app: &mut App, ctx: &egui::Context) {
    // Pauses while the user follows a step (e.g. the host editor is open).
    if app.lib.settings.onboarded || app.vault.is_none() || app.host_editor.is_some() {
        return;
    }
    let step = app.tour_step.min(TOUR.len() - 1);
    let (title, body, _) = TOUR[step];
    let mut done = false;
    let mut next = false;
    let mut action = None;
    egui::Modal::new(egui::Id::new("tour")).show(ctx, |ui| {
        ui.set_width(440.0);
        // Step dots
        ui.horizontal(|ui| {
            for i in 0..TOUR.len() {
                let (r, _) = ui.allocate_exact_size(
                    Vec2::new(if i == step { 22.0 } else { 8.0 }, 8.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(
                    r,
                    egui::CornerRadius::same(4),
                    if i == step {
                        theme::accent()
                    } else {
                        theme::border()
                    },
                );
            }
        });
        ui.add_space(10.0);
        ui.label(theme::heading(title, 21.0));
        ui.add_space(4.0);
        ui.label(RichText::new(body).size(14.0).color(theme::text_dim()));
        ui.add_space(14.0);
        if step == 0 {
            ui.horizontal(|ui| {
                if ui.button("SSH-Host anlegen").clicked() {
                    action = Some(crate::app::Action::NewHostOf(
                        crate::model::Protocol::Ssh,
                        None,
                    ));
                    next = true;
                }
                if !crate::app::ssh_config_hosts().is_empty()
                    && ui.button("Aus ~/.ssh/config importieren").clicked()
                {
                    action = Some(crate::app::Action::ImportSshConfig);
                    next = true;
                }
            });
            ui.add_space(8.0);
        }
        ui.horizontal(|ui| {
            if ui.button("Überspringen").clicked() {
                done = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let last = step + 1 == TOUR.len();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if theme::primary_button(ui, if last { "Los geht's" } else { "Weiter" }).clicked()
                    || enter
                {
                    if last {
                        done = true;
                    } else {
                        next = true;
                    }
                }
            });
        });
    });
    if next {
        app.tour_step = step + 1;
    }
    if done {
        app.lib.settings.onboarded = true;
        app.save_library();
    }
    if let Some(a) = action {
        app.apply(a, ctx);
    }
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
            ui.label(theme::heading("Server-Schlüssel geändert", 20.0).color(theme::red()));
            ui.label(
                RichText::new("Das passiert nach einer Neuinstallation, kann aber auch ein Angriff sein. Verbinde nur, wenn du den neuen Fingerprint geprüft hast.")
                    .color(theme::text_dim()),
            );
        } else {
            ui.label(theme::heading("Neuer Server", 20.0));
            ui.label(RichText::new("Vergleiche den Fingerprint mit dem Server, bevor du ihm vertraust.").color(theme::text_dim()));
        }
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::mono(&p.host, 13.0).color(theme::text()));
            ui.label(RichText::new(&p.algorithm).size(12.5).color(theme::text_dim()));
            ui.add_space(4.0);
            ui.label(theme::mono(&p.fingerprint, 13.0).color(theme::accent()));
            if let Some(prev) = &p.previous {
                ui.label(RichText::new(format!("vorher: {prev}")).monospace().size(11.5).color(theme::red()));
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "Vertrauen").clicked() {
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
        ui.label(theme::heading("Passwort", 20.0));
        ui.label(
            theme::mono(
                format!("{user}@{}:{}", pp.host.hostname, pp.host.port),
                13.0,
            )
            .color(theme::text_dim()),
        );
        ui.add_space(8.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut pp.password)
                .password(true)
                .hint_text("Passwort")
                .font(egui::FontId::monospace(14.0))
                .margin(egui::Margin::symmetric(10, 8))
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
        ui.label(theme::heading(run.snippet.name.clone(), 20.0));
        ui.label(RichText::new("Werte für die Platzhalter eingeben").color(theme::text_dim()));
        ui.add_space(6.0);
        egui::Grid::new("vars")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for (i, (name, value)) in run.values.iter_mut().enumerate() {
                    ui.label(
                        RichText::new(format!("{{{{{name}}}}}"))
                            .monospace()
                            .color(theme::accent2()),
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
            .fill(theme::bg_deep())
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(preview).monospace().color(theme::accent()));
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
        ui.label(theme::heading("Wirklich löschen?", 20.0));
        ui.label(RichText::new(text).color(theme::text_dim()));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Löschen").color(theme::bg_deep()))
                            .fill(theme::red()),
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
