//! Cloud-init sheet of a VM: every `ipconfigN`, DNS, user, password, SSH keys (from the
//! keychain too), package upgrade, custom snippets and "regenerate drive". The password
//! is only typed here, handed to the task inside a wiping `Secret` and never shown,
//! logged or stored.

use egui::{CornerRadius, RichText};

use crate::app::App;
use crate::backend::proxmox::pve2::ci::{
    self as ci, CiApply, CiConfig, CiEvent, Issue, Nic, Secret, V4, V6,
};
use crate::backend::proxmox::{ExtOp, GuestId, Kind, Op};
use crate::theme;

use super::super::{GuestRef, guest};

const PW_ID: &str = "p2-ci-password";

pub struct Sheet {
    pub g: GuestRef,
    label: String,
    load: Load,
}

enum Load {
    Loading,
    Failed(String),
    Ready(Box<Form>),
}

struct NicEdit {
    n: usize,
    on: bool,
    m4: usize,
    cidr4: String,
    gw4: String,
    m6: usize,
    cidr6: String,
    gw6: String,
}

struct Form {
    old: CiConfig,
    drive: bool,
    nics: Vec<NicEdit>,
    nameserver: String,
    searchdomain: String,
    ciuser: String,
    password: String,
    keys: String,
    /// 0 = Proxmox default, 1 = yes, 2 = no.
    upgrade: usize,
    cicustom: String,
    regen: bool,
    /// Public keys of the keychain: `(label, key)`.
    avail: Vec<(String, String)>,
}

fn nic_edit(n: usize, nic: &Nic) -> NicEdit {
    let (m4, cidr4, gw4) = match &nic.v4 {
        V4::None => (0, String::new(), String::new()),
        V4::Dhcp => (1, String::new(), String::new()),
        V4::Static { cidr, gw } => (2, cidr.clone(), gw.clone()),
        V4::Manual => (3, String::new(), String::new()),
    };
    let (m6, cidr6, gw6) = match &nic.v6 {
        V6::None => (0, String::new(), String::new()),
        V6::Auto => (1, String::new(), String::new()),
        V6::Dhcp => (2, String::new(), String::new()),
        V6::Static { cidr, gw } => (3, cidr.clone(), gw.clone()),
        V6::Manual => (4, String::new(), String::new()),
    };
    NicEdit {
        n,
        on: true,
        m4,
        cidr4,
        gw4,
        m6,
        cidr6,
        gw6,
    }
}

fn to_nic(e: &NicEdit) -> Nic {
    Nic {
        v4: match e.m4 {
            1 => V4::Dhcp,
            2 => V4::Static {
                cidr: e.cidr4.clone(),
                gw: e.gw4.clone(),
            },
            3 => V4::Manual,
            _ => V4::None,
        },
        v6: match e.m6 {
            1 => V6::Auto,
            2 => V6::Dhcp,
            3 => V6::Static {
                cidr: e.cidr6.clone(),
                gw: e.gw6.clone(),
            },
            4 => V6::Manual,
            _ => V6::None,
        },
    }
}

fn to_config(f: &Form) -> CiConfig {
    let mut nics: Vec<Option<Nic>> = Vec::new();
    for e in f.nics.iter().filter(|e| e.on && e.n < ci::MAX_NICS) {
        nics.resize(nics.len().max(e.n + 1), None);
        nics[e.n] = Some(to_nic(e));
    }
    CiConfig {
        nics,
        nameserver: f.nameserver.clone(),
        searchdomain: f.searchdomain.clone(),
        ciuser: f.ciuser.clone(),
        sshkeys: f
            .keys
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        ciupgrade: match f.upgrade {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        },
        cicustom: f.cicustom.clone(),
        has_password: f.old.has_password,
    }
}

fn form_from(c: CiConfig, drive: bool, avail: Vec<(String, String)>) -> Form {
    let nics = c
        .nics
        .iter()
        .enumerate()
        .filter_map(|(n, nic)| nic.as_ref().map(|nic| nic_edit(n, nic)))
        .collect();
    Form {
        drive,
        nics,
        nameserver: c.nameserver.clone(),
        searchdomain: c.searchdomain.clone(),
        ciuser: c.ciuser.clone(),
        password: String::new(),
        keys: c.sshkeys.join("\n"),
        upgrade: match c.ciupgrade {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        },
        cicustom: c.cicustom.clone(),
        regen: false,
        avail,
        old: c,
    }
}

pub fn open(app: &mut App, g: GuestRef) {
    if g.kind != Kind::Qemu {
        return;
    }
    let Some(s) = super::super::server_of(app, g.server) else {
        return;
    };
    let conn = super::super::conn(app, &s);
    app.pve.p2.ci = Some(Sheet {
        label: format!(
            "{} · {} {}",
            super::pve_label(app, g.server),
            g.vmid,
            g.name
        ),
        g: g.clone(),
        load: Load::Loading,
    });
    let gid = GuestId {
        node: g.node.clone(),
        kind: g.kind,
        vmid: g.vmid,
    };
    crate::backend::proxmox::pve2::ci::spawn_load(
        &app.rt,
        g.server,
        app.pve.epoch(g.server),
        conn,
        gid,
        app.pve.p2.ci_sink.clone(),
    );
}

/// Public keys of the keychain identities (those that decode).
fn keychain_keys(app: &App) -> Vec<(String, String)> {
    app.keychain
        .identities
        .iter()
        .filter(|i| !i.private_key.trim().is_empty())
        .filter_map(|i| {
            let (public, _) = crate::keygen::public_info(&i.private_key, &i.passphrase).ok()?;
            let label = if i.label.trim().is_empty() {
                i.username.clone()
            } else {
                i.label.clone()
            };
            Some((label, public.trim().to_string()))
        })
        .collect()
}

pub fn on_event(app: &mut App, ev: CiEvent) {
    let CiEvent::Loaded {
        server,
        epoch,
        guest,
        session,
        result,
    } = ev;
    if app.pve.epoch(server) != epoch {
        return;
    }
    if session.is_some() {
        app.pve.live.entry(server).or_default().session = session;
    }
    let matches = app
        .pve
        .p2
        .ci
        .as_ref()
        .is_some_and(|c| c.g.server == server && c.g.vmid == guest.vmid);
    if !matches {
        return;
    }
    let avail = if result.is_ok() {
        keychain_keys(app)
    } else {
        Vec::new()
    };
    if let Some(c) = app.pve.p2.ci.as_mut() {
        c.load = match result {
            Ok(l) => Load::Ready(Box::new(form_from(l.config, l.drive, avail))),
            Err(e) => Load::Failed(e.to_string()),
        };
    }
}

fn issue_lines(ui: &mut egui::Ui, issues: &[Issue], field: &str) {
    for i in issues.iter().filter(|i| i.field == field) {
        ui.label(RichText::new(&i.text).size(12.5).color(if i.warning {
            theme::amber()
        } else {
            theme::red()
        }));
    }
}

fn nic_block(ui: &mut egui::Ui, e: &mut NicEdit, issues: &[Issue]) {
    let field = format!("ipconfig{}", e.n);
    egui::Frame::NONE
        .fill(theme::group_fill())
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("net{} · {field}", e.n)).font(theme::semibold(13.5)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::btn(ui, theme::Btn::Link, None, t!("Entfernen")).clicked() {
                        e.on = false;
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("IPv4").size(12.5).color(theme::text_dim()));
                if let Some(i) = theme::segmented(
                    ui,
                    &[t!("Aus"), "DHCP", t!("Statisch"), t!("Manuell")],
                    e.m4,
                ) {
                    e.m4 = i;
                }
            });
            if e.m4 == 2 {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut e.cidr4)
                            .hint_text("10.0.0.5/24")
                            .desired_width(150.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut e.gw4)
                            .hint_text(t!("Gateway"))
                            .desired_width(130.0),
                    );
                    if e.gw4.trim().is_empty()
                        && let Some(g) = ci::suggest_gateway(&e.cidr4)
                        && theme::btn(
                            ui,
                            theme::Btn::Link,
                            None,
                            &format!("{} {g}", t!("Gateway")),
                        )
                        .clicked()
                    {
                        e.gw4 = g;
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("IPv6").size(12.5).color(theme::text_dim()));
                if let Some(i) = theme::segmented(
                    ui,
                    &[t!("Aus"), "SLAAC", "DHCP", t!("Statisch"), t!("Manuell")],
                    e.m6,
                ) {
                    e.m6 = i;
                }
            });
            if e.m6 == 3 {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut e.cidr6)
                            .hint_text("fd00::5/64")
                            .desired_width(180.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut e.gw6)
                            .hint_text(t!("Gateway"))
                            .desired_width(130.0),
                    );
                });
            }
            issue_lines(ui, issues, &field);
        });
}

pub fn sheet(app: &mut App, ctx: &egui::Context) {
    let Some(c) = app.pve.p2.ci.as_mut() else {
        return;
    };
    let g = c.g.clone();
    let title = format!("{} · {}", t!("Cloud-Init"), c.label);
    let apply = std::cell::Cell::new(false);
    let ready = matches!(c.load, Load::Ready(_));
    let issues_now: Vec<Issue> = match &c.load {
        Load::Ready(f) => ci::validate(&to_config(f), Some(&Secret::new(f.password.clone()))),
        _ => Vec::new(),
    };
    let blocked = ci::has_errors(&issues_now);
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("p2-ci"),
        760.0,
        &title,
        |ui| match &mut c.load {
            Load::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
                });
            }
            Load::Failed(e) => {
                ui.label(RichText::new(&*e).color(theme::red()));
            }
            Load::Ready(f) => {
                if !f.drive {
                    ui.label(
                        RichText::new(t!(
                            "Diese VM hat kein Cloud-Init-Laufwerk – die Werte greifen erst, wenn eines hinzugefügt ist (Hardware)."
                        ))
                        .color(theme::amber()),
                    );
                }
                theme::form_caption(ui, t!("Netzwerk"));
                for e in f.nics.iter_mut().filter(|e| e.on) {
                    nic_block(ui, e, &issues_now);
                    ui.add_space(6.0);
                }
                let next = (0..ci::MAX_NICS).find(|n| !f.nics.iter().any(|e| e.on && e.n == *n));
                if let Some(n) = next
                    && theme::btn(
                        ui,
                        theme::Btn::Small,
                        Some(crate::ui::icons::Icon::Plus),
                        t!("Schnittstelle hinzufügen"),
                    )
                    .clicked()
                {
                    f.nics.retain(|e| e.n != n);
                    f.nics.push(NicEdit {
                        n,
                        on: true,
                        m4: 1,
                        cidr4: String::new(),
                        gw4: String::new(),
                        m6: 0,
                        cidr6: String::new(),
                        gw6: String::new(),
                    });
                    f.nics.sort_by_key(|e| e.n);
                }
                theme::form_caption(ui, t!("DNS"));
                theme::form(ui, 130.0, |ui, r| {
                    r.field(ui, t!("Nameserver"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.nameserver)
                                .hint_text("1.1.1.1 9.9.9.9")
                                .desired_width(ui.available_width() - 14.0),
                        );
                    });
                    r.field(ui, t!("Suchdomäne"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.searchdomain)
                                .hint_text("lan.example")
                                .desired_width(ui.available_width() - 14.0),
                        );
                    });
                });
                issue_lines(ui, &issues_now, "nameserver");
                issue_lines(ui, &issues_now, "searchdomain");
                theme::form_caption(ui, t!("Zugang"));
                theme::form(ui, 130.0, |ui, r| {
                    r.field(ui, t!("Benutzer"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.ciuser)
                                .hint_text("debian")
                                .desired_width(ui.available_width() - 14.0),
                        );
                    });
                    r.field(ui, t!("Passwort"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.password)
                                .id(egui::Id::new(PW_ID))
                                .password(true)
                                .hint_text(if f.old.has_password {
                                    t!("gesetzt – leer lassen = unverändert")
                                } else {
                                    t!("nicht gesetzt")
                                })
                                .desired_width(ui.available_width() - 14.0),
                        );
                    });
                });
                issue_lines(ui, &issues_now, "ciuser");
                issue_lines(ui, &issues_now, "cipassword");
                ui.label(
                    RichText::new(t!(
                        "Das Passwort wird nie angezeigt oder gespeichert und nach dem Senden verworfen. SSH-Keys sind sicherer."
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                );
                theme::form_caption(ui, t!("SSH-Schlüssel (öffentlich, einer pro Zeile)"));
                ui.add(
                    egui::TextEdit::multiline(&mut f.keys)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                if !f.avail.is_empty() {
                    egui::ComboBox::from_id_salt("p2-ci-keys")
                        .selected_text(t!("Aus dem Schlüsselbund hinzufügen…"))
                        .show_ui(ui, |ui| {
                            for (label, key) in &f.avail {
                                if ui.button(label).clicked() {
                                    if !f.keys.lines().any(|l| l.trim() == key) {
                                        if !f.keys.is_empty() && !f.keys.ends_with('\n') {
                                            f.keys.push('\n');
                                        }
                                        f.keys.push_str(key);
                                    }
                                    ui.close();
                                }
                            }
                        });
                }
                issue_lines(ui, &issues_now, "sshkeys");
                theme::form_caption(ui, t!("Erweitert"));
                theme::form(ui, 130.0, |ui, r| {
                    r.field(ui, t!("Pakete aktualisieren"), |ui| {
                        if let Some(i) =
                            theme::segmented(ui, &[t!("Standard"), t!("Ja"), t!("Nein")], f.upgrade)
                        {
                            f.upgrade = i;
                        }
                    });
                    r.field(ui, "cicustom", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.cicustom)
                                .hint_text("user=local:snippets/ci.yaml")
                                .desired_width(ui.available_width() - 14.0),
                        );
                    });
                    r.switch(ui, t!("Laufwerk neu erzeugen"), &mut f.regen);
                });
                issue_lines(ui, &issues_now, "cicustom");
                ui.label(
                    RichText::new(t!(
                        "„Neu erzeugen“ schreibt das Cloud-Init-Laufwerk sofort neu (sonst beim nächsten Start)."
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                );
            }
        },
        |ui, _| {
            let close = theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked();
            ui.add_enabled_ui(ready && !blocked, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Übernehmen")).clicked() {
                    apply.set(true);
                }
            });
            close
        },
    );
    let mut finished = close || dismissed;
    if apply.get() {
        let planned = match app.pve.p2.ci.as_mut().map(|c| &mut c.load) {
            Some(Load::Ready(f)) => {
                let cfg = to_config(f);
                let pw = std::mem::take(&mut f.password);
                let secret = (!pw.is_empty()).then(|| Secret::new(pw));
                Some(CiApply::plan(&f.old, &cfg, secret, f.regen))
            }
            _ => None,
        };
        match planned {
            Some(Ok(a)) if a.is_noop() => {
                app.toast(t!("Nichts geändert"), theme::text_dim());
                finished = true;
            }
            Some(Ok(a)) => {
                guest::start_task(app, &g, Op::Ext(Box::new(ExtOp::CloudInit(Box::new(a)))));
                finished = true;
            }
            _ => {}
        }
    }
    if finished {
        app.pve.p2.ci = None;
        // egui keeps edit history of text fields: drop the one of the password.
        egui::text_edit::TextEditState::default().store(ctx, egui::Id::new(PW_ID));
    }
}
