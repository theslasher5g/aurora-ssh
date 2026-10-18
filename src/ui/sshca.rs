//! Keychain section for Aurora's SSH certificate authority, the per-host certificate
//! settings and the host key history in the host editor.

use egui::RichText;
use uuid::Uuid;

use crate::app::{App, Purpose};
use crate::backend::sshca;
use crate::model::{Host, HostKeyRecord, Protocol};
use crate::theme;
use crate::ui::icons::{self, Icon};

/// Prefix of a `pending_command` that is typed into the terminal without Enter.
pub const PRETYPE: &str = "\u{1}pretype:";

/// Page state of the CA section (kept in egui memory).
#[derive(Clone, Default)]
struct CaUi {
    setup_host: Option<Uuid>,
    confirm_delete: bool,
    show_steps: bool,
}

/// The "Zertifikats-Aussteller (CA)" block at the end of the keys page.
pub fn ca_section(app: &mut App, ui: &mut egui::Ui) {
    let id = egui::Id::new("aurora-ca-ui");
    let mut st: CaUi = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let ssh_hosts: Vec<(Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh)
        .map(|h| (h.id, format!("{} · {}", h.display_name(), h.hostname)))
        .collect();
    let users = app.lib.hosts.iter().filter(|h| h.cert.enabled).count();

    ui.add_space(26.0);
    ui.horizontal(|ui| {
        icons::show(ui, Icon::Shield, 16.0, theme::text_dim());
        theme::section_title(ui, t!("Zertifikats-Aussteller (CA)"));
    });
    ui.add_space(6.0);

    let mut create = false;
    let mut delete = false;
    let mut setup = None;
    let ctx = ui.ctx().clone();
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let Some(ca) = app.keychain.ca.clone() else {
            ui.label(theme::heading(t!("Kurzzeit-Zertifikate statt verteilter Schlüssel"), 16.0));
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "Aurora kann als kleine Zertifizierungsstelle arbeiten: pro Verbindung entsteht ein frischer Schlüssel, \
                         den die CA für 1–24 Stunden signiert. Server vertrauen der CA einmalig (TrustedUserCAKeys) – \
                         keine authorized_keys-Pflege, abgelaufene Zertifikate sind wertlos."
                    ))
                    .color(theme::text_dim()),
                )
                .wrap(),
            );
            ui.add_space(10.0);
            if theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("CA-Schlüssel erzeugen"))
                .clicked()
            {
                create = true;
            }
            return;
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            icons::show(ui, Icon::Key, 18.0, theme::accent());
            ui.label(theme::heading(t!("Aurora-CA"), 16.0));
            theme::tag(ui, "Ed25519", None);
            theme::pill(ui, t!("aktiv"), theme::green());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::btn(ui, theme::Btn::Small, Some(Icon::Copy), t!("Kopieren"))
                    .on_hover_text(t!("Öffentlichen Schlüssel der CA kopieren"))
                    .clicked()
                {
                    ctx.copy_text(ca.public_key.trim().to_string());
                    app.toast(t!("Öffentlicher CA-Schlüssel kopiert"), theme::green());
                }
            });
        });
        ui.add_space(6.0);
        if let Some(fp) = sshca::ca_fingerprint(&ca) {
            ui.label(theme::mono(fp, 12.5).color(theme::text_dim()));
        }
        let created = ca
            .created
            .map(|c| c.with_timezone(&chrono::Local).format("%d.%m.%Y").to_string())
            .unwrap_or_default();
        ui.label(
            RichText::new(t!(
                "Erstellt am {} · {} Hosts melden sich mit Kurzzeit-Zertifikat an",
                created,
                users
            ))
            .size(12.5)
            .color(theme::text_dim()),
        );
        ui.add_space(8.0);
        code_box(ui, ca.public_key.trim());

        ui.add_space(12.0);
        ui.label(RichText::new(t!("Server einrichten")).strong().color(theme::text()));
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Auf dem Server den öffentlichen Schlüssel nach {} legen und in /etc/ssh/sshd_config „TrustedUserCAKeys {}“ eintragen, dann sshd neu laden. Danach im Host-Editor „Anmeldung mit Kurzzeit-Zertifikat“ einschalten.",
                    sshca::CA_PATH,
                    sshca::CA_PATH
                ))
                .size(12.5)
                .color(theme::text_dim()),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let sel = st
                .setup_host
                .and_then(|id| ssh_hosts.iter().find(|(i, _)| *i == id))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("— SSH-Host wählen —").to_string());
            egui::ComboBox::from_id_salt("ca-setup-host")
                .selected_text(sel)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (id, n) in &ssh_hosts {
                        ui.selectable_value(&mut st.setup_host, Some(*id), n);
                    }
                });
            let r = ui.add_enabled_ui(st.setup_host.is_some(), |ui| {
                theme::btn(ui, theme::Btn::Normal, Some(Icon::Terminal), t!("Auf Server einrichten…"))
            });
            if r.inner
                .on_hover_text(t!(
                    "Öffnet ein Terminal zum Host und tippt die Befehle (mit sudo) vor – nichts wird ausgeführt, bevor du Enter drückst."
                ))
                .clicked()
            {
                setup = st.setup_host;
            }
            if theme::btn(
                ui,
                theme::Btn::Link,
                None,
                if st.show_steps {
                    t!("Befehle ausblenden")
                } else {
                    t!("Befehle anzeigen")
                },
            )
            .clicked()
            {
                st.show_steps = !st.show_steps;
            }
        });
        if st.show_steps {
            ui.add_space(8.0);
            let steps = sshca::manual_steps(&ca.public_key);
            code_box(ui, &steps);
            ui.add_space(4.0);
            if theme::btn(ui, theme::Btn::Small, Some(Icon::Copy), t!("Befehle kopieren")).clicked() {
                ctx.copy_text(steps);
                app.toast(t!("Befehle kopiert"), theme::green());
            }
        }

        ui.add_space(12.0);
        theme::hairline(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if st.confirm_delete {
                ui.label(
                    RichText::new(t!(
                        "CA wirklich löschen? Hosts mit Kurzzeit-Zertifikat melden sich dann wieder mit ihrer Identität an."
                    ))
                    .size(12.5)
                    .color(theme::red()),
                );
                if theme::btn(ui, theme::Btn::Small, Some(Icon::Trash), t!("Löschen")).clicked() {
                    delete = true;
                    st.confirm_delete = false;
                }
                if theme::btn(ui, theme::Btn::Link, None, t!("Abbrechen")).clicked() {
                    st.confirm_delete = false;
                }
            } else if theme::btn(ui, theme::Btn::Link, Some(Icon::Trash), t!("CA löschen…"))
                .clicked()
            {
                st.confirm_delete = true;
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(id, st));

    if create {
        match sshca::create_ca() {
            Ok(ca) => {
                app.keychain.ca = Some(ca);
                app.save_keychain();
                app.toast(t!("CA-Schlüssel erzeugt"), theme::green());
            }
            Err(e) => app.toast(format!("{e:#}"), theme::red()),
        }
    }
    if delete {
        app.keychain.ca = None;
        app.save_keychain();
        app.toast(t!("CA gelöscht"), theme::amber());
    }
    if let Some(host) = setup {
        setup_on(app, host, &ctx);
    }
}

/// Terminal to `host` with the setup command typed in (not executed).
fn setup_on(app: &mut App, host: Uuid, ctx: &egui::Context) {
    let Some(cmd) = app
        .keychain
        .ca
        .as_ref()
        .and_then(|ca| sshca::setup_command(&ca.public_key))
    else {
        return;
    };
    let Some(h) = app.lib.host(host).cloned() else {
        return;
    };
    app.pending_command = Some((host, format!("{PRETYPE}{cmd}")));
    app.connect_host(h, Purpose::Terminal, ctx);
    app.toast(
        t!("Der Befehl steht im Terminal – prüfen und mit Enter ausführen"),
        theme::accent(),
    );
}

/// Monospace block, selectable.
fn code_box(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(theme::fill())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(theme::mono(text, 12.0).color(theme::text()))
                    .wrap()
                    .selectable(true),
            );
        });
}

/// Host editor, SSH: certificate login switch, validity, principals.
pub fn host_section(ui: &mut egui::Ui, h: &mut Host, has_ca: bool) {
    theme::form_caption(ui, t!("Kurzzeit-Zertifikat"));
    theme::form(ui, 120.0, |ui, f| {
        f.switch(ui, t!("Anmeldung mit Kurzzeit-Zertifikat"), &mut h.cert.enabled)
            .on_hover_text(t!(
                "Pro Verbindung erzeugt Aurora einen neuen Ed25519-Schlüssel (nie gespeichert) und signiert ihn mit der CA aus dem Schlüsselbund. Klappt das nicht, meldet sich Aurora mit der Identität an."
            ));
        if !h.cert.enabled {
            return;
        }
        f.field(ui, t!("Gültigkeit"), |ui| {
            let labels: Vec<String> = sshca::VALIDITY_HOURS
                .iter()
                .map(|h| t!("{} h", h))
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let cur = sshca::VALIDITY_HOURS
                .iter()
                .position(|x| *x == h.cert.hours)
                .unwrap_or(1);
            if let Some(i) = theme::segmented(ui, &refs, cur) {
                h.cert.hours = sshca::VALIDITY_HOURS[i];
            }
        });
        f.field(ui, t!("Principals"), |ui| {
            let hint = if h.username.trim().is_empty() {
                t!("leer = Login-Benutzer").to_string()
            } else {
                t!("leer = {}", h.username.trim())
            };
            ui.add(
                egui::TextEdit::singleline(&mut h.cert.principals)
                    .hint_text(RichText::new(hint).color(theme::text_dim()))
                    .desired_width(ui.available_width() - 14.0),
            )
            .on_hover_text(t!(
                "Benutzernamen, für die das Zertifikat gilt (Komma-getrennt)"
            ));
        });
        if !has_ca {
            f.full(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(t!(
                            "Noch keine CA – lege sie unter Schlüssel → Zertifikats-Aussteller (CA) an."
                        ))
                        .size(12.5)
                        .color(theme::amber()),
                    )
                    .wrap(),
                );
            });
        }
    });
}

/// Host editor, SSH: every server key accepted for this host, newest first.
pub fn key_history(ui: &mut egui::Ui, history: &[HostKeyRecord], current: Option<&str>) {
    if history.is_empty() && current.is_none() {
        return;
    }
    theme::form_caption(ui, t!("Server-Schlüssel"));
    theme::form(ui, 92.0, |ui, f| {
        let date = |d: Option<chrono::DateTime<chrono::Utc>>| {
            d.map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%d.%m.%Y %H:%M")
                    .to_string()
            })
        };
        let mut rows: Vec<HostKeyRecord> = history.to_vec();
        if let Some(cur) = current
            && !rows
                .iter()
                .any(|r| r.fingerprint == cur && r.replaced.is_none())
        {
            rows.push(HostKeyRecord {
                fingerprint: cur.to_string(),
                ..Default::default()
            });
        }
        for r in rows.iter().rev() {
            f.full(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let active = r.replaced.is_none() && Some(r.fingerprint.as_str()) == current;
                    if active {
                        theme::pill(ui, t!("aktuell"), theme::green());
                    } else {
                        theme::pill(ui, t!("alt"), theme::text_dim());
                    }
                    ui.add(
                        egui::Label::new(theme::mono(&r.fingerprint, 12.0).color(if active {
                            theme::text()
                        } else {
                            theme::text_dim()
                        }))
                        .truncate(),
                    );
                });
                let mut info = Vec::new();
                if !r.algorithm.is_empty() {
                    info.push(r.algorithm.clone());
                }
                match date(r.trusted) {
                    Some(d) => info.push(t!("vertraut seit {}", d)),
                    None => info.push(t!("vertraut seit unbekannt").to_string()),
                }
                if let Some(d) = date(r.replaced) {
                    info.push(t!("ersetzt am {}", d));
                }
                ui.label(
                    RichText::new(info.join(" · "))
                        .size(12.0)
                        .color(theme::text_dim()),
                );
            });
        }
    });
}
