//! Modal editors for hosts, groups, identities and snippets.

use egui::{RichText, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::model::{FlowControl, Group, LineEnding, Parity, Protocol};
use crate::theme;

/// Save / Cancel in a sheet footer; returns (save, cancel).
pub(crate) fn save_cancel(ui: &mut egui::Ui, can_save: bool) -> (bool, bool) {
    let mut out = (false, false);
    ui.add_enabled_ui(can_save, |ui| {
        if theme::btn(ui, theme::Btn::Primary, None, t!("Speichern")).clicked() {
            out.0 = true;
        }
    });
    if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
        out.1 = true;
    }
    out
}

/// Small grey explanation below a form group.
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::text_dim())).wrap());
    });
}

/// Full-width text field inside a form row.
pub(crate) fn field_text(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(RichText::new(hint).color(theme::text_dim()))
            .desired_width(f32::INFINITY),
    )
}

pub(crate) fn color_row(ui: &mut egui::Ui, color: &mut Option<[u8; 3]>, allow_none: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if allow_none {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
            ui.painter()
                .circle_filled(rect.center(), 10.0, theme::fill_strong());
            if color.is_none() {
                ui.painter().circle_stroke(
                    rect.center(),
                    12.5,
                    egui::Stroke::new(2.0, theme::text()),
                );
            }
            if resp.on_hover_text(t!("Gruppenfarbe verwenden")).clicked() {
                *color = None;
            }
        }
        for c in theme::TAG_COLORS {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
            ui.painter()
                .circle_filled(rect.center(), 10.0, theme::rgb(c));
            if *color == Some(c) {
                ui.painter().circle_stroke(
                    rect.center(),
                    12.5,
                    egui::Stroke::new(2.0, theme::text()),
                );
            }
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                *color = Some(c);
            }
        }
    });
}

fn identity_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<Uuid>,
    app_idents: &[(Uuid, String, String)],
    none_label: &str,
) {
    // (id, name, key group) in keys-page order; shown as "Gruppe / Name".
    let selected = value
        .and_then(|v| app_idents.iter().find(|(i, _, _)| *i == v))
        .map(|(_, l, g)| {
            if g.is_empty() {
                l.clone()
            } else {
                format!("{g} / {l}")
            }
        })
        .unwrap_or_else(|| none_label.to_string());
    let grouped = app_idents.iter().any(|(_, _, g)| !g.is_empty());
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width((ui.available_width() - 46.0).max(160.0))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, none_label);
            let mut current: Option<&str> = None;
            for (i, l, g) in app_idents {
                if grouped && current != Some(g.as_str()) {
                    current = Some(g.as_str());
                    ui.add_space(4.0);
                    let heading = if g.is_empty() {
                        t!("Ohne Gruppe").to_string()
                    } else {
                        g.clone()
                    };
                    ui.label(
                        RichText::new(heading.to_uppercase())
                            .font(theme::semibold(11.0))
                            .color(theme::text_dim()),
                    );
                }
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
        .unwrap_or_else(|| t!("— Keine —").to_string());
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width((ui.available_width() - 14.0).max(160.0))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, t!("— Keine —"));
            for g in groups {
                if exclude.is_some_and(|ex| lib.is_descendant(g.id, ex)) {
                    continue;
                }
                ui.selectable_value(value, Some(g.id), format!("{} {}", g.icon, g.name));
            }
        });
}

/// Group of a key on the keys page; a dangling id shows as "Ohne Gruppe".
fn key_group_combo(ui: &mut egui::Ui, value: &mut Option<Uuid>, groups: &[crate::model::KeyGroup]) {
    let current = value.and_then(|v| groups.iter().find(|g| g.id == v));
    let selected = current
        .map(|g| g.name.clone())
        .unwrap_or_else(|| t!("Ohne Gruppe").to_string());
    egui::ComboBox::from_id_salt("key-group")
        .selected_text(selected)
        .width((ui.available_width() - 14.0).max(160.0))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(current.is_none(), t!("Ohne Gruppe"))
                .clicked()
            {
                *value = None;
            }
            for g in groups {
                let r = ui.selectable_label(*value == Some(g.id), format!("     {}", g.name));
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        r.rect.left_center() + Vec2::new(6.0, -4.5),
                        Vec2::splat(9.0),
                    ),
                    egui::CornerRadius::same(2),
                    theme::rgb(g.color),
                );
                if r.clicked() {
                    *value = Some(g.id);
                }
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
    let idents = crate::ui::keychain::choices(&app.keychain, |_| true);
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
    let has_ca = app.keychain.ca.is_some();
    let web_open = app.host_editor.as_ref().is_some_and(|e| {
        app.tabs.iter().any(|t| {
            t.host_id == Some(e.host.id)
                && matches!(&t.kind, crate::app::TabKind::Web(w) if !w.is_closed())
        })
    });
    let Some(ed) = app.host_editor.as_mut() else {
        return;
    };
    let mut new_identity = false;
    let mut pwm_pick = false;
    let mut browse = None;
    let fonts = theme::term_fonts();
    let app_font = app.lib.settings.term_font.clone();
    let app_size = app.lib.settings.font_size;

    let title = if ed.is_new {
        t!("Neuer Host")
    } else {
        t!("Host bearbeiten")
    };
    let lib = &app.lib;
    let finder = app.net.route.as_ref();
    let mac_busy = app.net.wol.looking_up(ed.host.id);
    let mut route_req = None;
    let mut wol_req = None;
    let ((s_, c_), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("host-editor"),
        580.0,
        title,
        |ui| {
            let h = &mut ed.host;
            // Protocol as a segmented control over the full width.
            let labels: Vec<&str> = Protocol::ALL.iter().map(|p| p.label()).collect();
            let cur = Protocol::ALL
                .iter()
                .position(|p| *p == h.protocol)
                .unwrap_or(0);
            if let Some(i) = theme::segmented(ui, &labels, cur) {
                let p = Protocol::ALL[i];
                if h.protocol != p {
                    if h.port == h.protocol.default_port() {
                        h.port = p.default_port();
                    }
                    h.protocol = p;
                }
            }
            ui.add_space(14.0);
            theme::form(ui, 92.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    field_text(ui, &mut h.label, t!("z.B. web-01"));
                });
                if h.protocol.is_network() {
                    if h.protocol == Protocol::Web {
                        crate::ui::web::address_rows(ui, f, h, &mut ed.tags_text);
                    } else {
                        f.field(ui, t!("Adresse"), |ui| {
                            let w = ui.available_width() - 86.0;
                            ui.add(
                                egui::TextEdit::singleline(&mut h.hostname)
                                    .hint_text(
                                        RichText::new(t!("host oder IP")).color(theme::text_dim()),
                                    )
                                    .desired_width(w),
                            );
                            ui.label(RichText::new(":").color(theme::text_dim()));
                            ui.add(egui::DragValue::new(&mut h.port).range(1..=65535));
                        });
                    }
                    if h.protocol != Protocol::Vnc {
                        f.field(ui, t!("Benutzer"), |ui| {
                            field_text(ui, &mut h.username, t!("leer = Identität/Gruppe"));
                        });
                    }
                    f.field(
                        ui,
                        if h.protocol == Protocol::Vnc {
                            t!("Passwort")
                        } else {
                            t!("Identität")
                        },
                        |ui| {
                            identity_combo(
                                ui,
                                "host-ident",
                                &mut h.identity_id,
                                &idents,
                                t!("— Passwort abfragen —"),
                            );
                            if theme::btn(
                                ui,
                                theme::Btn::Small,
                                Some(crate::ui::icons::Icon::Plus),
                                "",
                            )
                            .on_hover_text(t!("Neue Identität"))
                            .clicked()
                            {
                                new_identity = true;
                            }
                        },
                    );
                    if h.protocol != Protocol::Serial {
                        f.field(ui, t!("Manager"), |ui| {
                            pwm_pick |=
                                crate::ui::pwmanager_ui::source_row(ui, &mut h.credential_source);
                        });
                    }
                }
                f.field(ui, t!("Gruppe"), |ui| {
                    group_combo(ui, "host-group", &mut h.group_id, &groups, None, lib);
                });
            });
            ui.add_space(14.0);
            theme::form(ui, 92.0, |ui, f| {
                f.field(ui, t!("Farbe"), |ui| color_row(ui, &mut h.color, true));
                f.field(ui, t!("Umgebung"), |ui| {
                    env_picker(ui, &mut h.env, t!("von der Gruppe"))
                });
                f.field(ui, "Tags", |ui| {
                    field_text(ui, &mut ed.tags_text, "prod, nginx, …");
                });
                f.switch(ui, t!("In Favoriten anzeigen"), &mut h.favorite);
            });

            match h.protocol {
                Protocol::Ssh => {
                    theme::form_caption(ui, t!("Beim Verbinden"));
                    theme::form(ui, 120.0, |ui, f| {
                        f.field(ui, t!("Startup-Snippet"), |ui| {
                            let sel = h
                                .startup_snippet
                                .and_then(|id| snippets.iter().find(|(i, _)| *i == id))
                                .map(|(_, n)| n.clone())
                                .unwrap_or_else(|| t!("— kein Startup-Snippet —").to_string());
                            egui::ComboBox::from_id_salt("startup")
                                .selected_text(sel)
                                .width(ui.available_width() - 14.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut h.startup_snippet,
                                        None,
                                        t!("— kein Startup-Snippet —"),
                                    );
                                    for (id, n) in &snippets {
                                        ui.selectable_value(&mut h.startup_snippet, Some(*id), n);
                                    }
                                });
                        });
                        route_req = route_req.or(crate::ui::route::editor_rows(
                            ui, f, h, lib, finder, &ssh_hosts,
                        ));
                        f.switch(ui, t!("Dauerhafte Session (tmux)"), &mut h.persistent)
                            .on_hover_text(t!(
                                "Die Shell läuft in tmux (sonst screen) auf dem Server weiter: \
                                 nach einem Abbruch landet das Neu-Verbinden in derselben Session \
                                 mit allen laufenden Programmen."
                            ));
                        crate::ui::reconnect_auto::host_row(ui, f, &mut h.auto_reconnect);
                        f.switch(
                            ui,
                            t!("Shell-Integration beim Start automatisch einrichten"),
                            &mut h.shell_integration,
                        )
                        .on_hover_text(t!(
                            "Schickt nach dem Login eine kurze Zeile an bash, zsh oder fish, damit \
                             Befehlsblöcke Exit-Codes und Arbeitsordner kennen. Ändert keine \
                             Konfigurationsdateien und gilt nur für diese Sitzung."
                        ));
                    });
                    crate::ui::sshca::host_section(ui, h, has_ca);
                    let key_id = format!("{}:{}", h.hostname.trim(), h.port);
                    crate::ui::sshca::key_history(
                        ui,
                        lib.host_key_history
                            .get(&key_id)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]),
                        lib.known_hosts.get(&key_id).map(String::as_str),
                    );
                    theme::form_caption(ui, t!("Tunnel (Port-Weiterleitung)"));
                    theme::form(ui, 92.0, |ui, f| {
                        let mut remove = None;
                        for (i, t) in h.tunnels.iter_mut().enumerate() {
                            f.full(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.add(
                                        egui::TextEdit::singleline(&mut t.name)
                                            .hint_text(t!("Name"))
                                            .desired_width(96.0),
                                    );
                                    ui.label(RichText::new(t!("lokal")).color(theme::text_dim()));
                                    ui.add(
                                        egui::DragValue::new(&mut t.local_port).range(1..=65535),
                                    );
                                    ui.label(RichText::new("→").color(theme::text_dim()));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut t.remote_host)
                                            .hint_text("localhost")
                                            .desired_width(96.0),
                                    );
                                    ui.label(":");
                                    ui.add(
                                        egui::DragValue::new(&mut t.remote_port).range(1..=65535),
                                    );
                                    ui.checkbox(&mut t.open_browser, "").on_hover_text(t!(
                                        "Browser öffnen, sobald der Tunnel steht"
                                    ));
                                    if theme::icon_btn(
                                        ui,
                                        crate::ui::icons::Icon::Close,
                                        t!("Entfernen"),
                                    )
                                    .clicked()
                                    {
                                        remove = Some(i);
                                    }
                                });
                            });
                        }
                        if let Some(i) = remove {
                            h.tunnels.remove(i);
                        }
                        f.full(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(t!("localhost:<lokal> → über den Server → Ziel:Port. Ein-/ausschalten in der Hostliste."))
                                            .size(12.5)
                                            .color(theme::text_dim()),
                                    )
                                    .wrap(),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if theme::btn(ui, theme::Btn::Small, None, "+ Tunnel").clicked() {
                                        h.tunnels.push(crate::model::Tunnel::default());
                                    }
                                });
                            });
                        });
                    });
                }
                Protocol::Web => crate::ui::web::section(ui, h, web_open),
                Protocol::Telnet => {
                    ui.add_space(10.0);
                    hint(
                        ui,
                        t!(
                            "Telnet ist unverschlüsselt – nur im eigenen Netz verwenden. Mit Benutzer und Identität meldet sich Aurora automatisch an."
                        ),
                    );
                }
                Protocol::Vnc => {
                    theme::form_caption(ui, "VNC");
                    theme::form(ui, 92.0, |ui, f| {
                        f.switch(
                            ui,
                            t!("Nur zusehen (keine Maus/Tastatur senden)"),
                            &mut h.vnc.view_only,
                        );
                    });
                    ui.add_space(6.0);
                    hint(
                        ui,
                        t!(
                            "Passwort über eine Identität im Keychain – oder beim Verbinden eingeben (leer lassen, wenn der Server keins verlangt). Port 5900 = Bildschirm :0."
                        ),
                    );
                }
                Protocol::Docker | Protocol::Kubernetes => {
                    let k8s = h.protocol == Protocol::Kubernetes;
                    theme::form_caption(ui, if k8s { t!("Pod") } else { t!("Container") });
                    let c = &mut h.container;
                    let proto = h.protocol;
                    theme::form(ui, 120.0, |ui, f| {
                        f.field(ui, t!("Läuft auf"), |ui| {
                            let sel = c
                                .via
                                .and_then(|id| ssh_hosts.iter().find(|(i, _)| *i == id))
                                .map(|(_, n)| n.clone())
                                .unwrap_or_else(|| t!("Dieser PC").to_string());
                            egui::ComboBox::from_id_salt("ct-via-ed")
                                .selected_text(sel)
                                .width(ui.available_width() - 14.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut c.via, None, t!("Dieser PC"));
                                    for (id, n) in &ssh_hosts {
                                        ui.selectable_value(&mut c.via, Some(*id), n);
                                    }
                                });
                        });
                        f.field(ui, if k8s { t!("Pod") } else { t!("Name") }, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut c.name)
                                    .hint_text(if k8s { "api-7f9c…" } else { "nginx" })
                                    .desired_width(ui.available_width() - 130.0),
                            );
                            if theme::btn(ui, theme::Btn::Small, None, t!("Durchsuchen…")).clicked()
                            {
                                browse = Some((proto, c.via));
                            }
                        });
                        if k8s {
                            f.field(ui, "Namespace", |ui| {
                                field_text(ui, &mut c.namespace, "default");
                            });
                            f.field(ui, t!("Container im Pod"), |ui| {
                                field_text(ui, &mut c.container, t!("Standard"));
                            });
                            f.field(ui, t!("Kontext"), |ui| {
                                field_text(ui, &mut c.context, t!("aktueller"));
                            });
                        } else {
                            f.field(ui, t!("Benutzer"), |ui| {
                                field_text(ui, &mut c.user, t!("wie im Image"));
                            });
                            f.switch(ui, t!("Podman statt Docker"), &mut c.podman);
                        }
                        f.field(ui, "Shell", |ui| {
                            field_text(ui, &mut c.shell, t!("bash, sonst sh"));
                        });
                    });
                }
                Protocol::Rdp => {
                    theme::form_caption(ui, "RDP");
                    theme::form(ui, 120.0, |ui, f| {
                        f.field(ui, t!("Domäne"), |ui| {
                            field_text(ui, &mut h.rdp.domain, "");
                        });
                        f.field(ui, t!("Auflösung"), |ui| {
                            ui.add(egui::DragValue::new(&mut h.rdp.width).range(640..=7680));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut h.rdp.height).range(480..=4320));
                        });
                        f.switch(ui, t!("In Aurora anzeigen"), &mut h.rdp.embedded);
                        f.field(ui, t!("Dateien (SFTP)"), |ui| {
                            ui.add(egui::DragValue::new(&mut h.rdp.sftp_port).range(1..=65535));
                            ui.label(
                                RichText::new(t!("OpenSSH-Port des Windows-Hosts"))
                                    .size(12.5)
                                    .color(theme::text_dim()),
                            );
                        });
                        f.switch(ui, t!("Vollbild"), &mut h.rdp.fullscreen);
                        f.switch(ui, t!("Zwischenablage"), &mut h.rdp.clipboard);
                        f.switch(ui, "Multi-Monitor", &mut h.rdp.multimon);
                    });
                }
                Protocol::Serial => {
                    theme::form_caption(ui, t!("Serielle Schnittstelle"));
                    let s = &mut h.serial;
                    theme::form(ui, 120.0, |ui, f| {
                        f.field(ui, "Port", |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.port_name)
                                    .hint_text("/dev/ttyUSB0 · COM3")
                                    .desired_width(ui.available_width() - 120.0),
                            );
                            egui::ComboBox::from_id_salt("ports")
                                .selected_text(t!("Erkannt"))
                                .width(96.0)
                                .show_ui(ui, |ui| {
                                    if ports.is_empty() {
                                        ui.label(t!("keine gefunden"));
                                    }
                                    for p in &ports {
                                        if ui.selectable_label(s.port_name == *p, p).clicked() {
                                            s.port_name = p.clone();
                                        }
                                    }
                                });
                        });
                        f.field(ui, t!("Baudrate"), |ui| {
                            egui::ComboBox::from_id_salt("baud")
                                .selected_text(s.baud_rate.to_string())
                                .width(ui.available_width() - 14.0)
                                .show_ui(ui, |ui| {
                                    for b in [
                                        1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200,
                                        230400, 460800, 921600,
                                    ] {
                                        ui.selectable_value(&mut s.baud_rate, b, b.to_string());
                                    }
                                });
                        });
                        f.field(ui, "Format", |ui| {
                            egui::ComboBox::from_id_salt("bits")
                                .selected_text(t!("{} Datenbits", s.data_bits))
                                .show_ui(ui, |ui| {
                                    for b in [5u8, 6, 7, 8] {
                                        ui.selectable_value(&mut s.data_bits, b, b.to_string());
                                    }
                                });
                            egui::ComboBox::from_id_salt("parity")
                                .selected_text(format!("{:?}", s.parity))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut s.parity, Parity::None, "None");
                                    ui.selectable_value(&mut s.parity, Parity::Odd, "Odd");
                                    ui.selectable_value(&mut s.parity, Parity::Even, "Even");
                                });
                            egui::ComboBox::from_id_salt("stop")
                                .selected_text(t!("{} Stopbit", s.stop_bits))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut s.stop_bits, 1, "1");
                                    ui.selectable_value(&mut s.stop_bits, 2, "2");
                                });
                        });
                        f.field(ui, t!("Flusskontrolle"), |ui| {
                            egui::ComboBox::from_id_salt("flow")
                                .selected_text(format!("{:?}", s.flow_control))
                                .width(ui.available_width() - 14.0)
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
                        });
                        f.field(ui, t!("Enter sendet"), |ui| {
                            let cur = match s.line_ending {
                                LineEnding::Cr => 0,
                                LineEnding::Lf => 1,
                                LineEnding::CrLf => 2,
                            };
                            if let Some(i) = theme::segmented(ui, &["CR", "LF", "CR+LF"], cur) {
                                s.line_ending =
                                    [LineEnding::Cr, LineEnding::Lf, LineEnding::CrLf][i];
                            }
                        });
                        f.switch(ui, t!("Lokales Echo"), &mut s.local_echo);
                    });
                }
            }
            if h.protocol.is_network() {
                wol_req = crate::ui::wol::editor_section(ui, h, mac_busy);
            }
            if h.protocol.is_terminal() {
                theme::form_caption(ui, t!("Terminal-Profil"));
                term_profile(ui, &mut h.term, &fonts, &app_font, app_size);
            }
            theme::form_caption(ui, t!("Notizen"));
            theme::form(ui, 92.0, |ui, f| {
                f.full(ui, |ui| {
                    theme::frameless(ui);
                    ui.add(
                        egui::TextEdit::multiline(&mut h.notes)
                            .hint_text(RichText::new(t!("Notizen")).color(theme::text_dim()))
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
            match h.protocol {
                Protocol::Serial => !h.serial.port_name.trim().is_empty(),
                Protocol::Docker | Protocol::Kubernetes => !h.container.name.trim().is_empty(),
                Protocol::Web => crate::backend::web::proto::parse_url(&h.web.url).is_ok(),
                _ => !h.hostname.trim().is_empty(),
            }
        },
        save_cancel,
    );
    let save = s_;
    let cancel = c_ || dismissed;

    if new_identity {
        actions.push(Action::NewIdentity);
    }
    if pwm_pick {
        let cur = app
            .host_editor
            .as_ref()
            .and_then(|e| e.host.credential_source.clone());
        let settings = app.lib.settings.pwm.clone();
        app.pwm.open_picker(
            crate::ui::pwmanager_ui::PickTarget::Host,
            cur.as_ref(),
            &settings,
        );
    }
    if let Some(host) = app.host_editor.as_ref().map(|e| e.host.clone()) {
        if route_req == Some(crate::ui::route::EditorRequest::Find) {
            crate::ui::route::start(app, host.clone(), crate::ui::route::Mode::Editor, ctx);
        }
        if wol_req == Some(crate::ui::wol::EditorRequest::LookupMac) {
            crate::ui::wol::lookup_mac(app, host, ctx);
        }
    }
    if let Some((proto, via)) = browse {
        actions.push(Action::BrowseContainers(proto, via, true));
    }
    if cancel {
        app.host_editor = None;
    } else if save && let Some(mut ed) = app.host_editor.take() {
        ed.host.hostname = ed.host.hostname.trim().to_string();
        if ed.host.protocol == Protocol::Web {
            // Address, port and pin follow the URL (the form only saves valid ones).
            let _ = crate::backend::web::sync_host(&mut ed.host);
        }
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

/// Theme, font and size of a host's terminal; empty = like the settings.
fn term_profile(
    ui: &mut egui::Ui,
    p: &mut crate::model::TermProfile,
    fonts: &[String],
    app_font: &str,
    app_size: f32,
) {
    // Preview with the profile, falling back to the app settings like the session does.
    let palette = if p.theme.is_empty() {
        theme::palette()
    } else {
        theme::find(&p.theme)
    };
    let font = if p.font.trim().is_empty() {
        app_font
    } else {
        p.font.as_str()
    };
    let family = theme::term_font(ui.ctx(), font);
    let size = if p.font_size >= 6.0 {
        p.font_size
    } else {
        app_size
    };
    theme::form(ui, 92.0, |ui, f| {
        crate::ui::settings::terminal_preview(ui, &palette, family, size);
        f.field(ui, t!("Farben"), |ui| {
            let current = if p.theme.is_empty() {
                t!("— wie die App —").to_string()
            } else {
                theme::find(&p.theme).name.to_string()
            };
            egui::ComboBox::from_id_salt("tp-theme")
                .selected_text(current)
                .width(ui.available_width() - 14.0)
                .height(360.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut p.theme, String::new(), t!("— wie die App —"));
                    for t in theme::all_themes().iter() {
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::new(26.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 3.0, t.term_bg);
                            ui.painter().text(
                                r.center(),
                                egui::Align2::CENTER_CENTER,
                                "$_",
                                egui::FontId::monospace(10.0),
                                t.term_fg,
                            );
                            ui.selectable_value(&mut p.theme, t.id.to_string(), t.name);
                        });
                    }
                });
        });
        f.field(ui, t!("Schrift"), |ui| {
            let custom = !p.font.is_empty() && !fonts.contains(&p.font);
            let current = if p.font.is_empty() {
                t!("— wie in den Einstellungen —").to_string()
            } else if custom {
                t!("eigene Datei").to_string()
            } else {
                p.font.clone()
            };
            egui::ComboBox::from_id_salt("tp-font")
                .selected_text(current)
                .width(ui.available_width() - 100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut p.font,
                        String::new(),
                        t!("— wie in den Einstellungen —"),
                    );
                    for f in fonts {
                        ui.selectable_value(&mut p.font, f.clone(), f);
                    }
                    if ui
                        .selectable_label(custom, t!("Eigene Schriftdatei…"))
                        .clicked()
                        && !custom
                    {
                        p.font = if cfg!(windows) {
                            r"C:\Users\…\font.ttf".into()
                        } else {
                            "/path/to/font.ttf".into()
                        };
                    }
                });
            ui.add(
                egui::DragValue::new(&mut p.font_size)
                    .range(0.0..=40.0)
                    .speed(0.25)
                    .custom_formatter(|v, _| {
                        if v < 6.0 {
                            t!("Standard").to_string()
                        } else {
                            format!("{v:.1} pt")
                        }
                    }),
            )
            .on_hover_text(t!("Schriftgröße (0 = wie in den Einstellungen)"));
        });
        if !p.font.is_empty() && !fonts.contains(&p.font) {
            f.field(ui, t!("Datei"), |ui| {
                field_text(ui, &mut p.font, ".ttf / .otf")
                    .on_hover_text(t!("Pfad zu einer Monospace-Schrift (.ttf/.otf)"));
            });
        }
    });
}

/// PROD / STAGING / TEST / DEV as a segmented control; `none` names the empty choice.
fn env_picker(ui: &mut egui::Ui, env: &mut crate::model::Env, none: &str) {
    use crate::model::Env;
    let labels: Vec<&str> = Env::ALL
        .iter()
        .map(|e| if *e == Env::None { none } else { e.label() })
        .collect();
    let cur = Env::ALL.iter().position(|e| e == env).unwrap_or(0);
    let r = ui.scope(|ui| theme::segmented(ui, &labels, cur));
    if let Some(i) = r.inner {
        *env = Env::ALL[i];
    }
    r.response.on_hover_text(t!(
        "PROD-Sessions bekommen einen roten Rahmen und fragen bei gefährlichen Befehlen nach"
    ));
}

fn group_editor(app: &mut App, ctx: &egui::Context) {
    let idents = crate::ui::keychain::choices(&app.keychain, |_| true);
    let groups = app.lib.groups.clone();
    let Some((g, is_new)) = app.group_editor.as_mut() else {
        return;
    };
    let title = if *is_new {
        t!("Neue Gruppe")
    } else {
        t!("Gruppe bearbeiten")
    };
    let lib = &app.lib;
    let ((s_, c_), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("group-editor"),
        520.0,
        title,
        |ui| {
            theme::form(ui, 140.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    field_text(ui, &mut g.name, "");
                });
                f.field(ui, "Icon", |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for icon in ["🗀", "🔥", "🏠", "☁", "🔬", "🛠", "🌐", "🗄", "🎮", "🔒"]
                    {
                        if ui.selectable_label(g.icon == icon, icon).clicked() {
                            g.icon = icon.into();
                        }
                    }
                });
                f.field(ui, t!("Farbe"), |ui| {
                    let mut c = Some(g.color);
                    color_row(ui, &mut c, false);
                    if let Some(c) = c {
                        g.color = c;
                    }
                });
                f.field(ui, t!("Übergeordnet"), |ui| {
                    group_combo(ui, "parent", &mut g.parent_id, &groups, Some(g.id), lib);
                });
            });
            theme::form_caption(ui, t!("Für alle Hosts der Gruppe"));
            theme::form(ui, 140.0, |ui, f| {
                f.field(ui, t!("Standard-Benutzer"), |ui| {
                    field_text(ui, &mut g.default_username, t!("für Hosts ohne Benutzer"));
                });
                f.field(ui, t!("Standard-Identität"), |ui| {
                    identity_combo(
                        ui,
                        "group-ident",
                        &mut g.default_identity,
                        &idents,
                        t!("— keine —"),
                    );
                });
                f.field(ui, t!("Umgebung"), |ui| {
                    env_picker(ui, &mut g.env, t!("keine"))
                });
            });
            !g.name.trim().is_empty()
        },
        save_cancel,
    );
    let save = s_;
    let cancel = c_ || dismissed;
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
    let key_groups = app.keychain.groups.clone();
    let Some((ident, is_new, key_path)) = app.identity_editor.as_mut() else {
        return;
    };
    let mut toast = None;
    let mut pwm_pick = false;
    let title = if *is_new {
        t!("Neuer Schlüssel")
    } else {
        t!("Schlüssel bearbeiten")
    };
    let ((s_, c_), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("identity-editor"),
        560.0,
        title,
        |ui| {
            theme::form(ui, 100.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    field_text(ui, &mut ident.label, t!("z.B. Deploy-Key"));
                });
                f.field(ui, t!("Benutzer"), |ui| {
                    field_text(ui, &mut ident.username, "root");
                });
                f.field(ui, t!("Passwort"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut ident.password)
                            .password(true)
                            .hint_text(RichText::new(t!("Passwort")).color(theme::text_dim()))
                            .desired_width(f32::INFINITY),
                    );
                });
                f.field(ui, t!("Manager"), |ui| {
                    pwm_pick |= crate::ui::pwmanager_ui::source_row(ui, &mut ident.secret_source);
                });
                if !key_groups.is_empty() {
                    f.field(ui, t!("Gruppe"), |ui| {
                        key_group_combo(ui, &mut ident.group_id, &key_groups);
                    });
                }
            });
            theme::form_caption(ui, "SSH-Key");
            theme::form(ui, 100.0, |ui, f| {
                f.field(ui, t!("Datei"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(key_path)
                            .hint_text("~/.ssh/id_ed25519")
                            .desired_width(ui.available_width() - 90.0),
                    );
                    if theme::btn(ui, theme::Btn::Small, None, t!("Laden")).clicked() {
                        let path = if let Some(rest) = key_path.strip_prefix("~/") {
                            directories::UserDirs::new()
                                .map(|d| d.home_dir().join(rest))
                                .unwrap_or_else(|| rest.into())
                        } else {
                            std::path::PathBuf::from(key_path.as_str())
                        };
                        match std::fs::read_to_string(&path) {
                            Ok(t) => ident.private_key = t,
                            Err(e) => {
                                toast = Some((format!("{}: {e}", path.display()), theme::red()))
                            }
                        }
                    }
                });
                f.full(ui, |ui| {
                    theme::frameless(ui);
                    ui.add(
                        egui::TextEdit::multiline(&mut ident.private_key)
                            .hint_text("-----BEGIN OPENSSH PRIVATE KEY-----")
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                });
                f.field(ui, "Passphrase", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut ident.passphrase)
                            .password(true)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                if !ident.private_key.trim().is_empty() {
                    // Parsing encrypted keys runs a KDF, so cache the result per key/passphrase.
                    let cache_key =
                        egui::Id::new(("key-info", &ident.private_key, &ident.passphrase));
                    let info: Result<String, String> =
                        ui.data_mut(|d| d.get_temp(cache_key)).unwrap_or_else(|| {
                            let r =
                                crate::keygen::public_info(&ident.private_key, &ident.passphrase)
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
                                    .size(12.0)
                                    .color(theme::amber()),
                            );
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::btn(
                        ui,
                        theme::Btn::Small,
                        Some(crate::ui::icons::Icon::Sparkle),
                        t!("Ed25519 erzeugen"),
                    )
                    .clicked()
                    {
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
                                    t!("Key erzeugt — Public Key in die Zwischenablage kopiert")
                                        .to_string(),
                                    theme::accent(),
                                ));
                            }
                            Err(e) => toast = Some((format!("{e}"), theme::red())),
                        }
                    }
                });
            });
            !ident.label.trim().is_empty() || !ident.username.trim().is_empty()
        },
        save_cancel,
    );
    let save = s_;
    let cancel = c_ || dismissed;
    if let Some((t, c)) = toast {
        app.toast(t, c);
    }
    if pwm_pick {
        let cur = app
            .identity_editor
            .as_ref()
            .and_then(|e| e.0.secret_source.clone());
        let settings = app.lib.settings.pwm.clone();
        app.pwm.open_picker(
            crate::ui::pwmanager_ui::PickTarget::Identity,
            cur.as_ref(),
            &settings,
        );
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
    let title = if *is_new {
        t!("Neues Snippet")
    } else {
        t!("Snippet bearbeiten")
    };
    let ((s_, c_), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("snippet-editor"),
        580.0,
        title,
        |ui| {
            theme::form(ui, 110.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    field_text(ui, &mut s.name, "Disk Usage");
                });
                f.field(ui, t!("Beschreibung"), |ui| {
                    field_text(ui, &mut s.description, "");
                });
                f.field(ui, "Tags", |ui| {
                    field_text(ui, tags, "docker, logs");
                });
                f.field(ui, t!("Shell"), |ui| {
                    use crate::model::ShellKind;
                    let cur = usize::from(s.shell == ShellKind::PowerShell);
                    let r = ui.scope(|ui| theme::segmented(ui, &["Shell", "PowerShell"], cur));
                    if let Some(i) = r.inner {
                        s.shell = [ShellKind::Shell, ShellKind::PowerShell][i];
                    }
                    r.response.on_hover_text(t!(
                        "PowerShell-Snippets lassen sich auch in RDP-Sessions auf Windows-Hosts starten"
                    ));
                });
            });
            theme::form_caption(ui, "Script");
            theme::form(ui, 110.0, |ui, f| {
                f.full(ui, |ui| {
                    theme::frameless(ui);
                    ui.add(
                        egui::TextEdit::multiline(&mut s.script)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("sudo systemctl restart {{service}}")
                            .desired_rows(6)
                            .desired_width(f32::INFINITY),
                    );
                });
                f.switch(ui, t!("Nach dem Einfügen Enter drücken"), &mut s.auto_run);
            });
            let vars = s.variables();
            if !vars.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(t!("Variablen:"))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                    for v in vars {
                        theme::chip(ui, &format!("{{{{{v}}}}}"), theme::accent());
                    }
                });
            }
            !s.name.trim().is_empty() && !s.script.trim().is_empty()
        },
        save_cancel,
    );
    let save = s_;
    let cancel = c_ || dismissed;
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
