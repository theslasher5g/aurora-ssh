//! Sheet "Auswahl migrieren": target node per server, live or offline, local disks,
//! how many migrations run at once; shows the plan (what runs, what is skipped and
//! why) and asks once with the counts before the batch starts.

use std::collections::HashMap;

use egui::{CornerRadius, RichText};
use uuid::Uuid;

use crate::app::App;
use crate::backend::proxmox::pve2::migrate::{self as mg, MigGuest, Mode, Options, Outcome};
use crate::theme;

use super::super::{GuestRef, guest};

pub struct Sheet {
    refs: Vec<GuestRef>,
    guests: Vec<MigGuest>,
    /// `(server, label, online nodes)`.
    servers: Vec<(Uuid, String, Vec<String>)>,
    targets: HashMap<Uuid, String>,
    opts: Options,
    parallel: usize,
    customer: String,
    confirm: bool,
}

impl Sheet {
    pub fn involves(&self, server: Uuid) -> bool {
        self.servers.iter().any(|s| s.0 == server)
    }

    fn online(&self) -> HashMap<Uuid, Vec<String>> {
        self.servers
            .iter()
            .map(|(id, _, nodes)| (*id, nodes.clone()))
            .collect()
    }

    fn plan(&self) -> mg::Plan {
        mg::plan(&self.guests, &self.targets, &self.online(), self.opts)
    }
}

pub fn open(app: &mut App) {
    let sel = guest::selected_guests(app);
    if sel.is_empty() {
        app.toast(t!("Erst Gäste auswählen"), theme::amber());
        return;
    }
    let mut servers: Vec<(Uuid, String, Vec<String>)> = Vec::new();
    for (r, _) in &sel {
        if servers.iter().any(|s| s.0 == r.server) {
            continue;
        }
        let nodes = app
            .pve
            .live
            .get(&r.server)
            .and_then(|l| l.snap.as_ref())
            .map(|s| {
                s.nodes
                    .iter()
                    .filter(|n| n.online)
                    .map(|n| n.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let name = app
            .lib
            .pve_servers
            .iter()
            .find(|s| s.id == r.server)
            .map(|s| s.display_name().to_string())
            .unwrap_or_default();
        servers.push((r.server, name, nodes));
    }
    let customer = sel
        .first()
        .map(|(r, _)| super::cust_of(app, super::server_group(app, r.server)))
        .filter(|c| c.id.is_some())
        .map(|c| c.name)
        .unwrap_or_default();
    let guests = sel
        .iter()
        .map(|(r, g)| MigGuest {
            server: r.server,
            vmid: g.vmid,
            name: g.name.clone(),
            node: g.node.clone(),
            kind: g.kind,
            running: g.running(),
            template: g.template,
        })
        .collect();
    app.pve.p2.migrate = Some(Sheet {
        refs: sel.into_iter().map(|(r, _)| r).collect(),
        guests,
        servers,
        targets: HashMap::new(),
        opts: Options::default(),
        parallel: 2,
        customer,
        confirm: false,
    });
}

pub fn sheet(app: &mut App, ctx: &egui::Context) {
    let Some(m) = app.pve.p2.migrate.as_mut() else {
        return;
    };
    let plan0 = m.plan();
    let runs0 = plan0.runs();
    let want_confirm = std::cell::Cell::new(false);
    let go = std::cell::Cell::new(false);
    let title = if m.customer.is_empty() {
        t!("Auswahl migrieren").to_string()
    } else {
        t!("Auswahl migrieren · {}", m.customer)
    };
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("p2-migrate"),
        720.0,
        &title,
        |ui| {
            ui.label(
                RichText::new(t!(
                    "{} Gäste auf {} Server(n). Pro Server wählst du den Ziel-Node; gestartet wird erst nach einer Bestätigung.",
                    m.guests.len(),
                    m.servers.len()
                ))
                .color(theme::text_dim()),
            );
            theme::form(ui, 150.0, |ui, f| {
                let servers = m.servers.clone();
                for (id, label, nodes) in &servers {
                    let n = m.guests.iter().filter(|g| g.server == *id).count();
                    f.field(ui, &format!("{label} ({n})"), |ui| {
                        let cur = m
                            .targets
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| t!("Ziel-Node wählen…").to_string());
                        egui::ComboBox::from_id_salt(("p2-mig-target", *id))
                            .selected_text(cur)
                            .width(ui.available_width() - 14.0)
                            .show_ui(ui, |ui| {
                                for node in nodes {
                                    let on = m.targets.get(id) == Some(node);
                                    if ui.selectable_label(on, node).clicked() {
                                        m.targets.insert(*id, node.clone());
                                        m.confirm = false;
                                    }
                                }
                            });
                    });
                }
                f.field(ui, t!("Modus"), |ui| {
                    let cur = (m.opts.mode == Mode::Offline) as usize;
                    if let Some(i) = theme::segmented(
                        ui,
                        &[t!("Live / Neustart"), t!("Nur ausgeschaltete")],
                        cur,
                    ) {
                        m.opts.mode = if i == 1 { Mode::Offline } else { Mode::Live };
                        m.confirm = false;
                    }
                });
                f.switch(
                    ui,
                    t!("Lokale Datenträger mitnehmen (VMs)"),
                    &mut m.opts.local_disks,
                );
                f.field(ui, t!("Gleichzeitig"), |ui| {
                    let labels = [t!("1 (nacheinander)"), "2", "3", "4"];
                    if let Some(i) = theme::segmented(ui, &labels, m.parallel.clamp(1, 4) - 1) {
                        m.parallel = mg::clamp_parallel(i + 1);
                    }
                });
            });
            let hint = match m.opts.mode {
                Mode::Live => t!(
                    "Laufende VMs wandern live, laufende Container werden dafür kurz neu gestartet."
                ),
                Mode::Offline => {
                    t!("Laufende Gäste werden übersprungen – es wird nichts heruntergefahren.")
                }
            };
            ui.label(RichText::new(hint).size(12.5).color(theme::text_dim()));

            let plan = m.plan();
            ui.add_space(4.0);
            let summary = t!(
                "{} werden migriert ({} live, {} Container mit Neustart), {} übersprungen",
                plan.runs(),
                plan.live() - plan.restarts(),
                plan.restarts(),
                plan.skips()
            );
            ui.label(RichText::new(summary).font(theme::semibold(13.5)));
            egui::Frame::NONE
                .fill(theme::group_fill())
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (g, o) in plan.items.iter().take(200) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let name = if g.name.is_empty() {
                                g.vmid.to_string()
                            } else {
                                format!("{} {}", g.vmid, g.name)
                            };
                            ui.add(
                                egui::Label::new(RichText::new(name).font(theme::medium(13.0)))
                                    .truncate(),
                            );
                            match o {
                                Outcome::Run(s) => {
                                    ui.label(
                                        RichText::new(format!("{} → {}", g.node, s.target))
                                            .size(12.5)
                                            .color(theme::text_dim()),
                                    );
                                    if g.running {
                                        theme::tag(
                                            ui,
                                            if g.kind == crate::backend::proxmox::Kind::Lxc {
                                                t!("Neustart")
                                            } else {
                                                t!("live")
                                            },
                                            Some(theme::accent()),
                                        );
                                    }
                                }
                                Outcome::Skip(why) => {
                                    ui.label(
                                        RichText::new(format!("{} – {}", g.node, why.text()))
                                            .size(12.5)
                                            .color(theme::amber()),
                                    );
                                }
                            }
                        });
                    }
                });
            if m.confirm && plan.runs() > 0 {
                egui::Frame::NONE
                    .fill(theme::soft(theme::amber()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let who = if m.customer.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", t!("Kunde: {}", m.customer))
                        };
                        ui.label(
                            RichText::new(format!(
                                "{}{who}",
                                t!(
                                    "{} Gäste jetzt migrieren, {} übersprungen? Bis zu {} laufen gleichzeitig.",
                                    plan.runs(),
                                    plan.skips(),
                                    m.parallel
                                )
                            ))
                            .font(theme::semibold(13.5)),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if theme::btn(ui, theme::Btn::Primary, None, t!("Ja, migrieren"))
                                .clicked()
                            {
                                go.set(true);
                            }
                            if theme::btn(ui, theme::Btn::Normal, None, t!("Zurück")).clicked() {
                                m.confirm = false;
                            }
                        });
                    });
            }
        },
        |ui, _| {
            let close = theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked();
            ui.add_enabled_ui(runs0 > 0, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Migrieren…")).clicked() {
                    want_confirm.set(true);
                }
            });
            close
        },
    );
    if want_confirm.get()
        && let Some(m) = app.pve.p2.migrate.as_mut()
    {
        m.confirm = true;
    }
    if go.get() {
        let Some(m) = app.pve.p2.migrate.take() else {
            return;
        };
        let plan = m.plan();
        let mut run = Vec::new();
        let mut skipped = Vec::new();
        for ((_, outcome), r) in plan.items.into_iter().zip(m.refs) {
            match outcome {
                Outcome::Run(spec) => run.push((r, spec)),
                Outcome::Skip(why) => skipped.push((r, why.text().to_string())),
            }
        }
        guest::start_migrations(app, run, skipped, m.parallel);
        return;
    }
    if close || dismissed {
        app.pve.p2.migrate = None;
    }
}
