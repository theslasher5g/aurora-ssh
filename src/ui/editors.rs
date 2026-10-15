//! Modal editors for hosts, groups, identities and snippets.

use egui::{RichText, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::model::{FlowControl, Group, LineEnding, Parity, Protocol};
use crate::theme;

fn modal_header(ui: &mut egui::Ui, title: &str) {
    ui.label(theme::heading(title, 20.0));
    ui.add_space(4.0);
    theme::hairline(ui);
    ui.add_space(8.0);
}

/// Returns (save, cancel).
fn footer(ui: &mut egui::Ui, can_save: bool) -> (bool, bool) {
    ui.add_space(8.0);
    let mut out = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(can_save, |ui| {
                if theme::primary_button(ui, "Speichern").clicked() {
                    out.0 = true;
                }
            });
            if ui.button("Abbrechen").clicked() {
                out.1 = true;
            }
        });
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        out.1 = true;
    }
    out
}

fn color_row(ui: &mut egui::Ui, color: &mut Option<[u8; 3]>, allow_none: bool) {
    ui.horizontal(|ui| {
        if allow_none {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
            ui.painter().circle_stroke(
                rect.center(),
                8.0,
                egui::Stroke::new(1.5, theme::text_dim()),
            );
            if color.is_none() {
                ui.painter().circle_stroke(
                    rect.center(),
                    11.0,
                    egui::Stroke::new(2.0, theme::text()),
                );
            }
            if resp.on_hover_text("Gruppenfarbe verwenden").clicked() {
                *color = None;
            }
        }
        for c in theme::TAG_COLORS {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
            ui.painter()
                .circle_filled(rect.center(), 8.0, theme::rgb(c));
            if *color == Some(c) {
                ui.painter().circle_stroke(
                    rect.center(),
                    11.0,
                    egui::Stroke::new(2.0, theme::text()),
                );
            }
            if resp.clicked() {
                *color = Some(c);
            }
        }
    });
}

fn identity_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<Uuid>,
    app_idents: &[(Uuid, String)],
    none_label: &str,
) {
    let selected = value
        .and_then(|v| app_idents.iter().find(|(i, _)| *i == v))
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| none_label.to_string());
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(260.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, none_label);
            for (i, l) in app_idents {
                ui.selectable_value(value, Some(*i), l);
            }
        });
}

fn group_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<Uuid>,
    groups: &[Group],
    exclude: Option<Uuid>,
    lib: &crate::model::Library,
) {
    let selected = value
        .and_then(|v| groups.iter().find(|g| g.id == v))
        .map(|g| format!("{} {}", g.icon, g.name))
        .unwrap_or_else(|| "— Keine —".into());
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(260.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "— Keine —");
            for g in groups {
                if exclude.is_some_and(|ex| lib.is_descendant(g.id, ex)) {
                    continue;
                }
                ui.selectable_value(value, Some(g.id), format!("{} {}", g.icon, g.name));
            }
        });
}

pub fn show_all(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    host_editor(app, ctx, actions);
    group_editor(app, ctx);
    identity_editor(app, ctx);
    snippet_editor(app, ctx);
}

fn host_editor(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let idents: Vec<(Uuid, String)> = app
        .keychain
        .identities
        .iter()
        .map(|i| {
            (
                i.id,
                if i.label.is_empty() {
                    i.username.clone()
                } else {
                    i.label.clone()
                },
            )
        })
        .collect();
    let groups = app.lib.groups.clone();
    let snippets: Vec<(Uuid, String)> = app
        .lib
        .snippets
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    let ports = app.serial_ports.clone();
    let ssh_hosts: Vec<(Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh)
        .map(|h| (h.id, h.display_name().to_string()))
        .collect();
    let Some(ed) = app.host_editor.as_mut() else {
        return;
    };
    let mut save = false;
    let mut cancel = false;
    let mut new_identity = false;

    egui::Modal::new(egui::Id::new("host-editor")).show(ctx, |ui| {
        ui.set_width(520.0);
        modal_header(
            ui,
            if ed.is_new {
                "Neuer Host"
            } else {
                "Host bearbeiten"
            },
        );
        let h = &mut ed.host;
        egui::ScrollArea::vertical()
            .max_height(ctx.content_rect().height() * 0.7)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for p in Protocol::ALL {
                        if ui
                            .selectable_label(
                                h.protocol == p,
                                format!("{} {}", p.icon(), p.label()),
                            )
                            .clicked()
                            && h.protocol != p
                        {
                            if h.port == h.protocol.default_port() {
                                h.port = p.default_port();
                            }
                            h.protocol = p;
                        }
                    }
                });
                ui.add_space(6.0);
                egui::Grid::new("host-grid")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut h.label).hint_text("z.B. web-01"));
                        ui.end_row();

                        if h.protocol != Protocol::Serial {
                            ui.label("Adresse");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut h.hostname)
                                        .hint_text("host oder IP")
                                        .desired_width(190.0),
                                );
                                ui.label(":");
                                ui.add(egui::DragValue::new(&mut h.port).range(1..=65535));
                            });
                            ui.end_row();
                            ui.label("Benutzer");
                            ui.add(
                                egui::TextEdit::singleline(&mut h.username)
                                    .hint_text("leer = Identität/Gruppe"),
                            );
                            ui.end_row();
                            ui.label("Identität");
                            ui.horizontal(|ui| {
                                identity_combo(
                                    ui,
                                    "host-ident",
                                    &mut h.identity_id,
                                    &idents,
                                    "— Passwort abfragen —",
                                );
                                if ui
                                    .small_button("➕")
                                    .on_hover_text("Neue Identität")
                                    .clicked()
                                {
                                    new_identity = true;
                                }
                            });
                            ui.end_row();
                        }

                        ui.label("Gruppe");
                        group_combo(ui, "host-group", &mut h.group_id, &groups, None, &app.lib);
                        ui.end_row();
                        ui.label("Farbe");
                        color_row(ui, &mut h.color, true);
                        ui.end_row();
                        ui.label("Tags");
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.tags_text)
                                .hint_text("prod, nginx, …"),
                        );
                        ui.end_row();
                        ui.label("Umgebung");
                        env_picker(ui, &mut h.env, "von der Gruppe");
                        ui.end_row();
                        ui.label("Favorit");
                        ui.checkbox(&mut h.favorite, "★ in Favoriten anzeigen");
                        ui.end_row();
                    });

                match h.protocol {
                    Protocol::Ssh => {
                        ui.add_space(8.0);
                        theme::section_title(ui, "Beim Verbinden");
                        let sel = h
                            .startup_snippet
                            .and_then(|id| snippets.iter().find(|(i, _)| *i == id))
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| "— kein Startup-Snippet —".into());
                        egui::ComboBox::from_id_salt("startup")
                            .selected_text(sel)
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut h.startup_snippet,
                                    None,
                                    "— kein Startup-Snippet —",
                                );
                                for (id, n) in &snippets {
                                    ui.selectable_value(&mut h.startup_snippet, Some(*id), n);
                                }
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Verbinden über");
                            let me = h.id;
                            let sel = h
                                .jump_host
                                .and_then(|id| ssh_hosts.iter().find(|(i, _)| *i == id))
                                .map(|(_, n)| n.clone())
                                .unwrap_or_else(|| "— direkt —".into());
                            egui::ComboBox::from_id_salt("jump")
                                .selected_text(sel)
                                .width(220.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut h.jump_host, None, "— direkt —");
                                    for (id, n) in ssh_hosts.iter().filter(|(i, _)| *i != me) {
                                        ui.selectable_value(&mut h.jump_host, Some(*id), n);
                                    }
                                });
                        })
                        .response
                        .on_hover_text(
                            "Jump-Host: Aurora verbindet sich zuerst mit diesem Server und von dort weiter. \
                             Der Jump-Host braucht gespeicherte Zugangsdaten (Identität).",
                        );
                        ui.add_space(8.0);
                        theme::section_title(ui, "Tunnel (Port-Weiterleitung)");
                        ui.label(
                            RichText::new("localhost:<lokal> → über den Server → Ziel:Port. Ein-/ausschalten in der Hostliste.")
                                .size(12.0)
                                .color(theme::text_dim()),
                        );
                        let mut remove = None;
                        for (i, t) in h.tunnels.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut t.name)
                                        .hint_text("Name")
                                        .desired_width(110.0),
                                );
                                ui.label("lokal");
                                ui.add(egui::DragValue::new(&mut t.local_port).range(1..=65535));
                                ui.label("→");
                                ui.add(
                                    egui::TextEdit::singleline(&mut t.remote_host)
                                        .hint_text("localhost")
                                        .desired_width(100.0),
                                );
                                ui.label(":");
                                ui.add(egui::DragValue::new(&mut t.remote_port).range(1..=65535));
                                ui.checkbox(&mut t.open_browser, "")
                                    .on_hover_text("Browser öffnen, sobald der Tunnel steht");
                                if ui.small_button("×").on_hover_text("Entfernen").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove {
                            h.tunnels.remove(i);
                        }
                        if ui.button("+ Tunnel").clicked() {
                            h.tunnels.push(crate::model::Tunnel::default());
                        }
                    }
                    Protocol::Rdp => {
                        ui.add_space(8.0);
                        theme::section_title(ui, "RDP");
                        egui::Grid::new("rdp-grid")
                            .num_columns(2)
                            .spacing([16.0, 10.0])
                            .show(ui, |ui| {
                                ui.label("Domäne");
                                ui.text_edit_singleline(&mut h.rdp.domain);
                                ui.end_row();
                                ui.label("Auflösung");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(&mut h.rdp.width).range(640..=7680),
                                    );
                                    ui.label("×");
                                    ui.add(
                                        egui::DragValue::new(&mut h.rdp.height).range(480..=4320),
                                    );
                                });
                                ui.end_row();
                                ui.label("Anzeige");
                                ui.checkbox(&mut h.rdp.embedded, "In Aurora anzeigen");
                                ui.end_row();
                                ui.label("Dateien (SFTP)");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(&mut h.rdp.sftp_port).range(1..=65535),
                                    );
                                    ui.label(
                                        egui::RichText::new("OpenSSH-Port des Windows-Hosts")
                                            .size(12.0)
                                            .color(theme::text_dim()),
                                    );
                                });
                                ui.end_row();
                                ui.label("Optionen");
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut h.rdp.fullscreen, "Vollbild");
                                    ui.checkbox(&mut h.rdp.clipboard, "Zwischenablage");
                                    ui.checkbox(&mut h.rdp.multimon, "Multi-Monitor");
                                });
                                ui.end_row();
                            });
                    }
                    Protocol::Serial => {
                        ui.add_space(8.0);
                        theme::section_title(ui, "Serielle Schnittstelle");
                        let s = &mut h.serial;
                        egui::Grid::new("serial-grid")
                            .num_columns(2)
                            .spacing([16.0, 10.0])
                            .show(ui, |ui| {
                                ui.label("Port");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut s.port_name)
                                            .hint_text("/dev/ttyUSB0 · COM3")
                                            .desired_width(180.0),
                                    );
                                    egui::ComboBox::from_id_salt("ports")
                                        .selected_text("Erkannt")
                                        .width(90.0)
                                        .show_ui(ui, |ui| {
                                            if ports.is_empty() {
                                                ui.label("keine gefunden");
                                            }
                                            for p in &ports {
                                                if ui
                                                    .selectable_label(s.port_name == *p, p)
                                                    .clicked()
                                                {
                                                    s.port_name = p.clone();
                                                }
                                            }
                                        });
                                });
                                ui.end_row();
                                ui.label("Baudrate");
                                egui::ComboBox::from_id_salt("baud")
                                    .selected_text(s.baud_rate.to_string())
                                    .show_ui(ui, |ui| {
                                        for b in [
                                            1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200,
                                            230400, 460800, 921600,
                                        ] {
                                            ui.selectable_value(&mut s.baud_rate, b, b.to_string());
                                        }
                                    });
                                ui.end_row();
                                ui.label("Format");
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("bits")
                                        .selected_text(format!("{} Datenbits", s.data_bits))
                                        .show_ui(ui, |ui| {
                                            for b in [5u8, 6, 7, 8] {
                                                ui.selectable_value(
                                                    &mut s.data_bits,
                                                    b,
                                                    b.to_string(),
                                                );
                                            }
                                        });
                                    egui::ComboBox::from_id_salt("parity")
                                        .selected_text(format!("{:?}", s.parity))
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut s.parity,
                                                Parity::None,
                                                "None",
                                            );
                                            ui.selectable_value(&mut s.parity, Parity::Odd, "Odd");
                                            ui.selectable_value(
                                                &mut s.parity,
                                                Parity::Even,
                                                "Even",
                                            );
                                        });
                                    egui::ComboBox::from_id_salt("stop")
                                        .selected_text(format!("{} Stopbit", s.stop_bits))
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut s.stop_bits, 1, "1");
                                            ui.selectable_value(&mut s.stop_bits, 2, "2");
                                        });
                                });
                                ui.end_row();
                                ui.label("Flusskontrolle");
                                egui::ComboBox::from_id_salt("flow")
                                    .selected_text(format!("{:?}", s.flow_control))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut s.flow_control,
                                            FlowControl::None,
                                            "None",
                                        );
                                        ui.selectable_value(
                                            &mut s.flow_control,
                                            FlowControl::Software,
                                            "XON/XOFF",
                                        );
                                        ui.selectable_value(
                                            &mut s.flow_control,
                                            FlowControl::Hardware,
                                            "RTS/CTS",
                                        );
                                    });
                                ui.end_row();
                                ui.label("Enter sendet");
                                ui.horizontal(|ui| {
                                    ui.selectable_value(&mut s.line_ending, LineEnding::Cr, "CR");
                                    ui.selectable_value(&mut s.line_ending, LineEnding::Lf, "LF");
                                    ui.selectable_value(
                                        &mut s.line_ending,
                                        LineEnding::CrLf,
                                        "CR+LF",
                                    );
                                    ui.checkbox(&mut s.local_echo, "Lokales Echo");
                                });
                                ui.end_row();
                            });
                    }
                }
                ui.add_space(8.0);
                theme::section_title(ui, "Notizen");
                ui.add(
                    egui::TextEdit::multiline(&mut h.notes)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
            });

        let valid = match ed.host.protocol {
            Protocol::Serial => !ed.host.serial.port_name.trim().is_empty(),
            _ => !ed.host.hostname.trim().is_empty(),
        };
        (save, cancel) = footer(ui, valid);
    });

    if new_identity {
        actions.push(Action::NewIdentity);
    }
    if cancel {
        app.host_editor = None;
    } else if save && let Some(mut ed) = app.host_editor.take() {
        ed.host.hostname = ed.host.hostname.trim().to_string();
        ed.host.tags = ed
            .tags_text
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        match app.lib.hosts.iter_mut().find(|h| h.id == ed.host.id) {
            Some(h) => *h = ed.host,
            None => app.lib.hosts.push(ed.host),
        }
        app.save_library();
    }
}

/// PROD / STAGING / TEST / DEV chips; `none` names the empty choice.
fn env_picker(ui: &mut egui::Ui, env: &mut crate::model::Env, none: &str) {
    use crate::model::Env;
    ui.horizontal(|ui| {
        for e in Env::ALL {
            let label = if e == Env::None {
                none.to_string()
            } else {
                e.label().to_string()
            };
            let selected = *env == e;
            let mut text = RichText::new(label).size(12.0);
            if selected && e != Env::None {
                text = text.color(theme::env_color(e)).strong();
            }
            if ui.selectable_label(selected, text).clicked() {
                *env = e;
            }
        }
    })
    .response
    .on_hover_text(
        "PROD-Sessions bekommen einen roten Rahmen und fragen bei gefährlichen Befehlen nach",
    );
}

fn group_editor(app: &mut App, ctx: &egui::Context) {
    let idents: Vec<(Uuid, String)> = app
        .keychain
        .identities
        .iter()
        .map(|i| (i.id, i.label.clone()))
        .collect();
    let groups = app.lib.groups.clone();
    let Some((g, is_new)) = app.group_editor.as_mut() else {
        return;
    };
    let (mut save, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("group-editor")).show(ctx, |ui| {
        ui.set_width(460.0);
        modal_header(
            ui,
            if *is_new {
                "Neue Gruppe"
            } else {
                "Gruppe bearbeiten"
            },
        );
        egui::Grid::new("group-grid")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut g.name);
                ui.end_row();
                ui.label("Icon");
                ui.horizontal(|ui| {
                    for icon in ["🗀", "🔥", "🏠", "☁", "🔬", "🛠", "🌐", "🗄", "🎮", "🔒"]
                    {
                        if ui.selectable_label(g.icon == icon, icon).clicked() {
                            g.icon = icon.into();
                        }
                    }
                });
                ui.end_row();
                ui.label("Farbe");
                let mut c = Some(g.color);
                color_row(ui, &mut c, false);
                if let Some(c) = c {
                    g.color = c;
                }
                ui.end_row();
                ui.label("Übergeordnet");
                group_combo(
                    ui,
                    "parent",
                    &mut g.parent_id,
                    &groups,
                    Some(g.id),
                    &app.lib,
                );
                ui.end_row();
                ui.label("Standard-Benutzer");
                ui.add(
                    egui::TextEdit::singleline(&mut g.default_username)
                        .hint_text("für Hosts ohne Benutzer"),
                );
                ui.end_row();
                ui.label("Standard-Identität");
                identity_combo(
                    ui,
                    "group-ident",
                    &mut g.default_identity,
                    &idents,
                    "— keine —",
                );
                ui.end_row();
                ui.label("Umgebung");
                env_picker(ui, &mut g.env, "keine");
                ui.end_row();
            });
        (save, cancel) = footer(ui, !g.name.trim().is_empty());
    });
    if cancel {
        app.group_editor = None;
    } else if save && let Some((g, _)) = app.group_editor.take() {
        match app.lib.groups.iter_mut().find(|x| x.id == g.id) {
            Some(x) => *x = g,
            None => app.lib.groups.push(g),
        }
        app.save_library();
    }
}

fn identity_editor(app: &mut App, ctx: &egui::Context) {
    let Some((ident, is_new, key_path)) = app.identity_editor.as_mut() else {
        return;
    };
    let (mut save, mut cancel) = (false, false);
    let mut toast = None;
    egui::Modal::new(egui::Id::new("identity-editor")).show(ctx, |ui| {
        ui.set_width(540.0);
        modal_header(
            ui,
            if *is_new {
                "Neue Identität"
            } else {
                "Identität bearbeiten"
            },
        );
        egui::Grid::new("ident-grid")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Bezeichnung");
                ui.add(egui::TextEdit::singleline(&mut ident.label).hint_text("z.B. Deploy-Key"));
                ui.end_row();
                ui.label("Benutzer");
                ui.text_edit_singleline(&mut ident.username);
                ui.end_row();
                ui.label("Passwort");
                ui.add(egui::TextEdit::singleline(&mut ident.password).password(true));
                ui.end_row();
            });
        ui.add_space(8.0);
        theme::section_title(ui, "SSH-Key");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(key_path)
                    .hint_text("~/.ssh/id_ed25519")
                    .desired_width(300.0),
            );
            if ui.button("🗁 Laden").clicked() {
                let path = if let Some(rest) = key_path.strip_prefix("~/") {
                    directories::UserDirs::new()
                        .map(|d| d.home_dir().join(rest))
                        .unwrap_or_else(|| rest.into())
                } else {
                    std::path::PathBuf::from(key_path.as_str())
                };
                match std::fs::read_to_string(&path) {
                    Ok(t) => ident.private_key = t,
                    Err(e) => toast = Some((format!("{}: {e}", path.display()), theme::red())),
                }
            }
            if ui.button("✨ Ed25519 erzeugen").clicked() {
                let comment = if ident.label.is_empty() {
                    "aurora".to_string()
                } else {
                    ident.label.clone()
                };
                match crate::keygen::generate_ed25519(&comment) {
                    Ok((private, public)) => {
                        ident.private_key = private;
                        ident.passphrase.clear();
                        ui.ctx().copy_text(public);
                        toast = Some((
                            "Key erzeugt — Public Key in die Zwischenablage kopiert".into(),
                            theme::accent(),
                        ));
                    }
                    Err(e) => toast = Some((format!("{e}"), theme::red())),
                }
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut ident.private_key)
                .hint_text("-----BEGIN OPENSSH PRIVATE KEY-----")
                .font(egui::TextStyle::Monospace)
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.label("Passphrase");
            ui.add(egui::TextEdit::singleline(&mut ident.passphrase).password(true));
        });
        if !ident.private_key.trim().is_empty() {
            // Parsing encrypted keys runs a KDF, so cache the result per key/passphrase.
            let cache_key = egui::Id::new(("key-info", &ident.private_key, &ident.passphrase));
            let info: Result<String, String> =
                ui.data_mut(|d| d.get_temp(cache_key)).unwrap_or_else(|| {
                    let r = crate::keygen::public_info(&ident.private_key, &ident.passphrase)
                        .map(|(_, fp)| fp)
                        .map_err(|e| e.to_string());
                    ui.data_mut(|d| d.insert_temp(cache_key, r.clone()));
                    r
                });
            match info {
                Ok(fp) => {
                    ui.label(
                        RichText::new(format!("✔ {fp}"))
                            .monospace()
                            .size(11.5)
                            .color(theme::green()),
                    );
                }
                Err(e) => {
                    ui.label(
                        RichText::new(format!("⚠ {e}"))
                            .size(11.5)
                            .color(theme::amber()),
                    );
                }
            }
        }
        (save, cancel) = footer(
            ui,
            !ident.label.trim().is_empty() || !ident.username.trim().is_empty(),
        );
    });
    if let Some((t, c)) = toast {
        app.toast(t, c);
    }
    if cancel {
        app.identity_editor = None;
    } else if save && let Some((mut ident, _, _)) = app.identity_editor.take() {
        if ident.label.trim().is_empty() {
            ident.label = ident.username.clone();
        }
        match app
            .keychain
            .identities
            .iter_mut()
            .find(|x| x.id == ident.id)
        {
            Some(x) => *x = ident.clone(),
            None => app.keychain.identities.push(ident.clone()),
        }
        // A host editor that is open in the background picks up the new identity.
        if let Some(he) = app.host_editor.as_mut()
            && he.host.identity_id.is_none()
        {
            he.host.identity_id = Some(ident.id);
        }
        app.save_keychain();
    }
}

fn snippet_editor(app: &mut App, ctx: &egui::Context) {
    let Some((s, is_new, tags)) = app.snippet_editor.as_mut() else {
        return;
    };
    let (mut save, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("snippet-editor")).show(ctx, |ui| {
        ui.set_width(560.0);
        modal_header(
            ui,
            if *is_new {
                "Neues Snippet"
            } else {
                "Snippet bearbeiten"
            },
        );
        egui::Grid::new("snip-grid")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut s.name);
                ui.end_row();
                ui.label("Beschreibung");
                ui.text_edit_singleline(&mut s.description);
                ui.end_row();
                ui.label("Tags");
                ui.add(egui::TextEdit::singleline(tags).hint_text("docker, logs"));
                ui.end_row();
            });
        ui.add_space(6.0);
        theme::section_title(ui, "Script");
        ui.add(
            egui::TextEdit::multiline(&mut s.script)
                .font(egui::TextStyle::Monospace)
                .hint_text("sudo systemctl restart {{service}}")
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
        let vars = s.variables();
        if !vars.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Variablen:").color(theme::text_dim()));
                for v in vars {
                    theme::chip(ui, &v, theme::accent2());
                }
            });
        }
        ui.checkbox(&mut s.auto_run, "Nach dem Einfügen Enter drücken");
        (save, cancel) = footer(ui, !s.name.trim().is_empty() && !s.script.trim().is_empty());
    });
    if cancel {
        app.snippet_editor = None;
    } else if save && let Some((mut s, _, tags)) = app.snippet_editor.take() {
        s.tags = tags
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        match app.lib.snippets.iter_mut().find(|x| x.id == s.id) {
            Some(x) => *x = s,
            None => app.lib.snippets.push(s),
        }
        app.save_library();
    }
}
