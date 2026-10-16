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
    key_setup(app, ctx);
    scan(app, ctx);
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
    egui::Modal::new(egui::Id::new("danger"))
        .frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                if host.1 != crate::model::Env::None {
                    env_badge(ui, host.1);
                }
                ui.label(theme::heading(
                    t!("Wirklich auf {} ausführen?", host.0),
                    19.0,
                ));
            });
            ui.label(
                RichText::new(t!("Dieser Befehl {}.", crate::i18n::tr(why)))
                    .color(theme::text_dim()),
            );
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
                            egui::Button::new(
                                RichText::new(t!("Ausführen")).color(theme::bg_deep()),
                            )
                            .fill(theme::red()),
                        )
                        .clicked()
                    {
                        answer = Some(true);
                    }
                    if theme::primary_button(ui, t!("Abbrechen")).clicked() {
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
    theme::pill(ui, env.label(), theme::env_color(env));
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
    let (title, body) = (crate::i18n::tr(title), crate::i18n::tr(body));
    let mut done = false;
    let mut next = false;
    let mut action = None;
    egui::Modal::new(egui::Id::new("tour"))
        .frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
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
                    if ui.button(t!("SSH-Host anlegen")).clicked() {
                        action = Some(crate::app::Action::NewHostOf(
                            crate::model::Protocol::Ssh,
                            None,
                        ));
                        next = true;
                    }
                    if !crate::app::ssh_config_hosts().is_empty()
                        && ui.button(t!("Aus ~/.ssh/config importieren")).clicked()
                    {
                        action = Some(crate::app::Action::ImportSshConfig);
                        next = true;
                    }
                });
                ui.add_space(8.0);
            }
            ui.horizontal(|ui| {
                if ui.button(t!("Überspringen")).clicked() {
                    done = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let last = step + 1 == TOUR.len();
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if theme::primary_button(ui, if last { t!("Los geht's") } else { t!("Weiter") })
                        .clicked()
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
    egui::Modal::new(egui::Id::new("hostkey")).frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
        ui.set_width(500.0);
        if changed {
            ui.label(theme::heading(t!("Server-Schlüssel geändert"), 20.0).color(theme::red()));
            ui.label(
                RichText::new(t!("Das passiert nach einer Neuinstallation, kann aber auch ein Angriff sein. Verbinde nur, wenn du den neuen Fingerprint geprüft hast."))
                    .color(theme::text_dim()),
            );
        } else {
            ui.label(theme::heading(t!("Neuer Server"), 20.0));
            ui.label(RichText::new(t!("Vergleiche den Fingerprint mit dem Server, bevor du ihm vertraust.")).color(theme::text_dim()));
        }
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::mono(&p.host, 13.0).color(theme::text()));
            ui.label(RichText::new(&p.algorithm).size(12.5).color(theme::text_dim()));
            ui.add_space(4.0);
            ui.label(theme::mono(&p.fingerprint, 13.0).color(theme::accent()));
            if let Some(prev) = &p.previous {
                ui.label(RichText::new(t!("vorher: {}", prev)).monospace().size(11.5).color(theme::red()));
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, t!("Vertrauen")).clicked() {
                    answer = Some((true, true));
                }
                if ui.button(t!("Nur dieses Mal")).clicked() {
                    answer = Some((true, false));
                }
                if ui.button(t!("Abbrechen")).clicked() {
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
    egui::Modal::new(egui::Id::new("password"))
        .frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(theme::heading(t!("Passwort"), 20.0));
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
                    .hint_text(t!("Passwort"))
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
                ui.checkbox(&mut pp.save, t!("Im Keychain speichern"));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button(ui, t!("Verbinden")).clicked() {
                        submit = true;
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        cancel = true;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    if cancel {
        if let Some(Purpose::Tunnel(tid)) = app.password_prompt.as_ref().map(|p| p.purpose) {
            app.tunnels.remove(&tid);
        }
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
    egui::Modal::new(egui::Id::new("snippet-vars"))
        .frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(theme::heading(run.snippet.name.clone(), 20.0));
            ui.label(
                RichText::new(t!("Werte für die Platzhalter eingeben")).color(theme::text_dim()),
            );
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
                            t!("▶ In allen ausführen")
                        } else {
                            t!("▶ Ausführen")
                        },
                    )
                    .clicked()
                    {
                        submit = true;
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
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
    let Some((text, inner)) = app.confirm_delete.as_ref() else {
        return;
    };
    let text = text.clone();
    // Proxmox power actions confirm with their own wording.
    let (title, button) = match inner {
        crate::app::Action::Pve(a) => a.confirm_labels(),
        _ => None,
    }
    .unwrap_or((t!("Wirklich löschen?"), t!("Löschen")));
    let mut answer = None;
    egui::Modal::new(egui::Id::new("confirm"))
        .frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(theme::heading(title, 20.0));
            ui.label(RichText::new(text).color(theme::text_dim()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(button).color(theme::bg_deep()))
                                .fill(theme::red()),
                        )
                        .clicked()
                    {
                        answer = Some(true);
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
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

/// "SSH-Key einrichten": installs a key on the server and switches the host to it.
fn key_setup(app: &mut App, ctx: &egui::Context) {
    let Some(ks) = app.key_setup.as_ref() else {
        return;
    };
    let host = app
        .lib
        .host(ks.host)
        .map(|h| h.display_name().to_string())
        .unwrap_or_default();
    let keys: Vec<(uuid::Uuid, String)> = app
        .keychain
        .identities
        .iter()
        .filter(|i| !i.private_key.trim().is_empty())
        .map(|i| (i.id, i.label.clone()))
        .collect();
    let mut start = false;
    let mut close = false;
    let ks = app.key_setup.as_mut().expect("checked above");
    egui::Modal::new(egui::Id::new("key-setup"))
        .frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(theme::heading(t!("SSH-Key für {}", host), 20.0));
            ui.label(
                RichText::new(t!(
                    "Aurora meldet sich einmal mit Passwort an, legt den öffentlichen Schlüssel in \
                 ~/.ssh/authorized_keys ab und prüft danach die Anmeldung per Key."
                ))
                .color(theme::text_dim()),
            );
            ui.add_space(10.0);
            ui.add_enabled_ui(!ks.running, |ui| {
                ui.radio_value(
                    &mut ks.identity,
                    None,
                    t!("Neuen Ed25519-Schlüssel erzeugen (empfohlen)"),
                );
                for (id, label) in &keys {
                    let label = if label.is_empty() {
                        t!("(ohne Namen)")
                    } else {
                        label
                    };
                    ui.radio_value(
                        &mut ks.identity,
                        Some(*id),
                        t!("Vorhandenen Schlüssel: {}", label),
                    );
                }
                ui.add_space(8.0);
                ui.label(RichText::new(t!("Passwort für diese eine Anmeldung")).size(12.5));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut ks.password)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    start = true;
                }
            });
            if let Some(e) = &ks.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(theme::red()));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ks.running {
                    ui.spinner();
                    ui.label(RichText::new(t!("Richte ein…")).color(theme::text_dim()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !ks.running,
                            egui::Button::new(RichText::new(t!("Einrichten")).strong()),
                        )
                        .clicked()
                    {
                        start = true;
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        close = true;
                    }
                });
            });
        });
    if close {
        app.key_setup = None;
    } else if start && !app.key_setup.as_ref().is_some_and(|k| k.running) {
        app.start_key_setup();
    }
}

/// "Netz durchsuchen": find SSH/RDP machines in the local network and add them.
fn scan(app: &mut App, ctx: &egui::Context) {
    if app.scan.is_none() {
        return;
    }
    let known: Vec<String> = app
        .lib
        .hosts
        .iter()
        .map(|h| h.hostname.trim().to_string())
        .collect();
    let mut start = false;
    let mut close = false;
    let mut import = false;
    let sc = app.scan.as_mut().expect("checked above");
    let modal = egui::Modal::new(egui::Id::new("scan")).frame(crate::theme::dialog_frame())
        .backdrop_color(crate::theme::backdrop())
        .show(ctx, |ui| {
        ui.set_width(560.0);
        ui.label(theme::heading(t!("Netz durchsuchen"), 20.0));
        ui.label(
            RichText::new(t!("Findet Geräte mit offenem SSH- oder RDP-Port. Es wird nur eine Verbindung aufgebaut, nichts angemeldet."))
                .color(theme::text_dim()),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(t!("Bereich"));
            ui.add_enabled(
                !sc.running,
                egui::TextEdit::singleline(&mut sc.range)
                    .desired_width(170.0)
                    .font(egui::FontId::monospace(13.0)),
            );
            ui.label("Ports");
            ui.add_enabled(
                !sc.running,
                egui::TextEdit::singleline(&mut sc.ports)
                    .desired_width(90.0)
                    .font(egui::FontId::monospace(13.0)),
            );
            if ui
                .add_enabled(!sc.running, egui::Button::new(RichText::new(t!("Scannen")).strong()))
                .clicked()
            {
                start = true;
            }
        });
        if let Some(e) = &sc.error {
            ui.label(RichText::new(e).color(theme::red()));
        }
        let (done, total) = sc.progress;
        if sc.running || total > 0 {
            ui.add_space(6.0);
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_height(4.0)
                    .fill(theme::accent()),
            );
            ui.label(
                RichText::new(if sc.running {
                    t!("{} / {} Adressen geprüft · {} gefunden", done, total, sc.results.len())
                } else {
                    t!("Fertig · {} Geräte gefunden", sc.results.len())
                })
                .size(12.0)
                .color(theme::text_dim()),
            );
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for (f, ticked) in sc.results.iter_mut() {
                let ip = f.ip.to_string();
                let is_known = known.contains(&ip);
                ui.horizontal(|ui| {
                    ui.add_enabled(!is_known, egui::Checkbox::without_text(ticked));
                    ui.add_sized(
                        [130.0, 20.0],
                        egui::Label::new(theme::mono(&ip, 13.0).color(theme::text())),
                    );
                    for p in &f.ports {
                        let name = match p {
                            22 => "SSH".to_string(),
                            3389 => "RDP".to_string(),
                            p => p.to_string(),
                        };
                        ui.label(theme::mono(name, 11.0).color(theme::accent()));
                    }
                    if let Some(b) = &f.banner {
                        ui.label(
                            RichText::new(crate::backend::scan::banner_hint(b))
                                .size(12.0)
                                .color(theme::text_dim()),
                        );
                    }
                    if is_known {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(t!("schon angelegt")).size(12.0).color(theme::text_dim()));
                        });
                    }
                });
            }
        });
        let picked = sc.results.iter().filter(|(_, t)| *t).count();
        ui.add_space(8.0);
        if !sc.results.is_empty() {
            ui.checkbox(&mut sc.new_group, t!("In eine eigene Gruppe „LAN …“ legen"));
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        picked > 0 && !sc.running,
                        egui::Button::new(RichText::new(t!("{} hinzufügen", picked)).strong()),
                    )
                    .clicked()
                {
                    import = true;
                }
                if ui.button(t!("Schließen")).clicked() {
                    close = true;
                }
            });
        });
    });
    if start {
        app.start_scan(ctx);
    }
    if import {
        app.import_scan();
    } else if close || (modal.should_close() && !app.scan.as_ref().is_some_and(|s| s.running)) {
        app.scan = None;
    }
}
