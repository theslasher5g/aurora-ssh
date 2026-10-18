//! Quick hardware edits (cores, sockets, memory, balloon/swap, disk growth, NIC bridge
//! and VLAN) with a diff before applying, and the migration sheet (drag & drop onto a
//! node heading or "Migrieren nach ▸").

use super::*;
use crate::backend::proxmox::{HwChange, HwConfig, MigrateSpec, StoreInfo};

// ------------------------------------------------------------------ hardware

struct NetEdit {
    key: String,
    orig: String,
    bridge: String,
    tag: String,
}

pub struct HwSheet {
    pub g: GuestRef,
    running: bool,
    orig: Option<Result<HwConfig, String>>,
    cores: u32,
    sockets: u32,
    memory: u64,
    balloon_on: bool,
    balloon: u64,
    swap: u64,
    /// GiB to add per disk (same order as `orig.disks`).
    grow: Vec<u32>,
    nets: Vec<NetEdit>,
    pub task: Option<u64>,
}

pub fn open_hw(app: &mut App, g: GuestRef) {
    let running = live_guest(app, g.server, g.vmid).is_some_and(|x| x.running());
    app.pve.ext.hw = Some(HwSheet {
        g,
        running,
        orig: None,
        cores: 1,
        sockets: 1,
        memory: 512,
        balloon_on: true,
        balloon: 0,
        swap: 0,
        grow: Vec::new(),
        nets: Vec::new(),
        task: None,
    });
    load_hw(app);
}

fn load_hw(app: &mut App) {
    let Some(g) = app.pve.ext.hw.as_ref().map(|h| h.g.clone()) else {
        return;
    };
    let Some(s) = server_of(app, g.server) else {
        return;
    };
    let c = conn(app, &s);
    pve::spawn_hw_config(
        &app.rt,
        s.id,
        app.pve.epoch(s.id),
        c,
        g.id(),
        app.pve.sink.clone(),
    );
}

pub fn on_config(app: &mut App, server: Uuid, vmid: u32, r: Result<HwConfig, Error>) {
    let Some(h) = app
        .pve
        .ext
        .hw
        .as_mut()
        .filter(|h| h.g.server == server && h.g.vmid == vmid)
    else {
        return;
    };
    if let Ok(hw) = &r {
        h.fill(hw);
    }
    h.orig = Some(r.map_err(|e| e.to_string()));
}

/// A hardware task finished: show the new config.
pub fn task_done(app: &mut App, task: u64) {
    if app
        .pve
        .ext
        .hw
        .as_ref()
        .is_some_and(|h| h.task == Some(task))
    {
        load_hw(app);
    }
}

/// One line of the diff: what, before, after.
type Diff = (String, String, String);

fn push(ch: &mut HwChange, diff: &mut Vec<Diff>, key: &str, label: &str, old: String, new: String) {
    ch.set.push((key.to_string(), new.clone()));
    diff.push((label.to_string(), old, new));
}

impl HwSheet {
    fn fill(&mut self, hw: &HwConfig) {
        self.cores = hw.cores;
        self.sockets = hw.sockets;
        self.memory = hw.memory;
        self.balloon_on = hw.balloon != Some(0);
        self.balloon = hw.balloon.filter(|b| *b > 0).unwrap_or(hw.memory);
        self.swap = hw.swap;
        self.grow = vec![0; hw.disks.len()];
        self.nets = hw
            .nets
            .iter()
            .map(|(k, v)| {
                let parts = pve::parse_net(v);
                NetEdit {
                    key: k.clone(),
                    orig: v.clone(),
                    bridge: pve::net_get(&parts, "bridge")
                        .unwrap_or_default()
                        .to_string(),
                    tag: pve::net_get(&parts, "tag").unwrap_or_default().to_string(),
                }
            })
            .collect();
    }

    /// The changes and a readable diff, or what is wrong with the input.
    fn changes(&self) -> Result<(HwChange, Vec<Diff>), String> {
        let Some(Ok(o)) = &self.orig else {
            return Err(String::new());
        };
        let vm = self.g.kind == Kind::Qemu;
        let mut ch = HwChange {
            digest: o.digest.clone(),
            ..Default::default()
        };
        let mut diff: Vec<Diff> = Vec::new();
        if self.cores != o.cores {
            push(
                &mut ch,
                &mut diff,
                "cores",
                t!("Kerne"),
                o.cores.to_string(),
                self.cores.to_string(),
            );
        }
        if vm && self.sockets != o.sockets {
            push(
                &mut ch,
                &mut diff,
                "sockets",
                t!("Sockel"),
                o.sockets.to_string(),
                self.sockets.to_string(),
            );
        }
        if self.memory < 16 {
            return Err(t!("Arbeitsspeicher: mindestens 16 MiB").into());
        }
        if self.memory != o.memory {
            push(
                &mut ch,
                &mut diff,
                "memory",
                t!("Arbeitsspeicher (MiB)"),
                o.memory.to_string(),
                self.memory.to_string(),
            );
        }
        if vm {
            let orig_on = o.balloon != Some(0);
            let orig_min = o.balloon.filter(|b| *b > 0).unwrap_or(o.memory);
            // Not set before and left alone: nothing to write.
            let untouched = o.balloon.is_none() && self.balloon_on && self.balloon == o.memory;
            if self.balloon_on && !untouched {
                if self.balloon > self.memory {
                    return Err(t!(
                        "Ballooning-Minimum darf nicht über dem Arbeitsspeicher liegen"
                    )
                    .into());
                }
                if !orig_on || self.balloon != orig_min {
                    push(
                        &mut ch,
                        &mut diff,
                        "balloon",
                        t!("Ballooning-Minimum (MiB)"),
                        if orig_on {
                            orig_min.to_string()
                        } else {
                            t!("aus").to_string()
                        },
                        self.balloon.to_string(),
                    );
                }
            } else if !self.balloon_on && orig_on {
                push(
                    &mut ch,
                    &mut diff,
                    "balloon",
                    t!("Ballooning-Minimum (MiB)"),
                    orig_min.to_string(),
                    "0".into(),
                );
                if let Some(d) = diff.last_mut() {
                    d.2 = t!("aus").to_string();
                }
            }
        } else if self.swap != o.swap {
            push(
                &mut ch,
                &mut diff,
                "swap",
                t!("Swap (MiB)"),
                o.swap.to_string(),
                self.swap.to_string(),
            );
        }
        for n in &self.nets {
            let tag = match n.tag.trim() {
                "" => None,
                t => match t.parse::<u16>() {
                    Ok(v) if (1..=4094).contains(&v) => Some(v),
                    _ => return Err(t!("{}: VLAN-Tag 1–4094 oder leer", n.key)),
                },
            };
            let bridge = n.bridge.trim();
            if !pve::valid_bridge(bridge) {
                return Err(t!("{}: ungültiger Bridge-Name", n.key));
            }
            let new = pve::net_with(&n.orig, bridge, tag)
                .ok_or_else(|| t!("{}: ungültiger Bridge-Name", n.key))?;
            if new != n.orig {
                let old = pve::parse_net(&n.orig);
                let show = |b: &str, t: Option<&str>| match t {
                    Some(t) => format!("{b} · VLAN {t}"),
                    None => b.to_string(),
                };
                push(
                    &mut ch,
                    &mut diff,
                    &n.key,
                    &n.key,
                    show(
                        pve::net_get(&old, "bridge").unwrap_or("–"),
                        pve::net_get(&old, "tag"),
                    ),
                    new.clone(),
                );
                if let Some(d) = diff.last_mut() {
                    d.2 = show(bridge, tag.map(|t| t.to_string()).as_deref());
                }
            }
        }
        for (d, add) in o.disks.iter().zip(&self.grow) {
            if *add > 0 {
                ch.resize.push((d.key.clone(), *add));
                diff.push((
                    t!("Datenträger {}", d.key),
                    format!("{:.0} GiB", d.gib),
                    format!("{:.0} GiB (+{add})", d.gib + *add as f64),
                ));
            }
        }
        Ok((ch, diff))
    }
}

type TaskState = (Option<Result<String, String>>, String);

pub fn hw_sheet(app: &mut App, ctx: &egui::Context) {
    let task: Option<TaskState> = app
        .pve
        .ext
        .hw
        .as_ref()
        .and_then(|h| h.task)
        .and_then(|id| app.pve.tasks.iter().find(|t| t.id == id))
        .map(|t| (t.done.clone(), t.progress.clone()));
    let Some(h) = app.pve.ext.hw.as_mut() else {
        return;
    };
    let busy = task.as_ref().is_some_and(|t| t.0.is_none());
    let changes = h.changes();
    let n = changes
        .as_ref()
        .map(|(c, _)| c.set.len() + c.resize.len())
        .unwrap_or(0);
    let can = !busy && n > 0;
    let title = t!("Hardware · {}", h.g.display());
    let ((apply, close), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-hw"),
        600.0,
        &title,
        |ui| hw_body(ui, h, &changes, task.as_ref()),
        |ui, _| {
            let mut out = (false, false);
            let label = if n > 0 {
                t!("Übernehmen ({})", n)
            } else {
                t!("Übernehmen").to_string()
            };
            if ui
                .add_enabled_ui(can, |ui| theme::btn(ui, theme::Btn::Primary, None, &label))
                .inner
                .clicked()
            {
                out.0 = true;
            }
            if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                out.1 = true;
            }
            out
        },
    );
    if apply && let Ok((ch, _)) = changes {
        let g = h.g.clone();
        let id = start_task_with(app, &g, Op::Ext(Box::new(ExtOp::Hardware(ch))), true);
        if let Some(h) = app.pve.ext.hw.as_mut() {
            h.task = Some(id);
        }
    } else if close || dismissed {
        app.pve.ext.hw = None;
    }
}

fn hw_body(
    ui: &mut egui::Ui,
    h: &mut HwSheet,
    changes: &Result<(HwChange, Vec<Diff>), String>,
    task: Option<&TaskState>,
) {
    let orig = match &h.orig {
        None => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
            });
            return;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::red()));
            return;
        }
        Some(Ok(o)) => o.clone(),
    };
    let vm = h.g.kind == Kind::Qemu;
    theme::form_caption(ui, t!("Prozessor und Speicher"));
    theme::form(ui, 170.0, |ui, rows| {
        rows.field(ui, t!("Kerne"), |ui| {
            ui.add(egui::DragValue::new(&mut h.cores).range(1..=512));
        });
        if vm {
            rows.field(ui, t!("Sockel"), |ui| {
                ui.add(egui::DragValue::new(&mut h.sockets).range(1..=16));
            });
        }
        rows.field(ui, t!("Arbeitsspeicher (MiB)"), |ui| {
            ui.add(
                egui::DragValue::new(&mut h.memory)
                    .range(16..=4 * 1024 * 1024)
                    .speed(32.0),
            );
            ui.label(RichText::new(gib(h.memory * 1024 * 1024)).color(theme::text_dim()));
        });
        if vm {
            rows.switch(ui, t!("Ballooning"), &mut h.balloon_on);
            if h.balloon_on {
                rows.field(ui, t!("Minimum (MiB)"), |ui| {
                    ui.add(
                        egui::DragValue::new(&mut h.balloon)
                            .range(0..=4 * 1024 * 1024)
                            .speed(32.0),
                    );
                });
            }
        } else {
            rows.field(ui, t!("Swap (MiB)"), |ui| {
                ui.add(
                    egui::DragValue::new(&mut h.swap)
                        .range(0..=4 * 1024 * 1024)
                        .speed(32.0),
                );
            });
        }
    });
    if !orig.disks.is_empty() {
        theme::form_caption(ui, t!("Datenträger vergrößern"));
        theme::form(ui, 170.0, |ui, rows| {
            for (d, add) in orig.disks.iter().zip(h.grow.iter_mut()) {
                let label = format!("{} · {:.0} GiB", d.key, d.gib);
                rows.field(ui, &label, |ui| {
                    ui.label(RichText::new("+").color(theme::text_dim()));
                    ui.add(egui::DragValue::new(add).range(0..=4096).suffix(" GiB"));
                    ui.add(
                        egui::Label::new(theme::mono(&d.volume, 11.5).color(theme::text_dim()))
                            .truncate(),
                    );
                });
            }
        });
        hint(
            ui,
            t!(
                "Datenträger lassen sich nur vergrößern. Das Dateisystem im Gast muss danach noch erweitert werden."
            ),
        );
    }
    if !h.nets.is_empty() {
        theme::form_caption(ui, t!("Netzwerk"));
        theme::form(ui, 170.0, |ui, rows| {
            for n in h.nets.iter_mut() {
                let key = n.key.clone();
                rows.field(ui, &key, |ui| {
                    ui.label(RichText::new(t!("Bridge")).color(theme::text_dim()));
                    ui.add(
                        egui::TextEdit::singleline(&mut n.bridge)
                            .char_limit(15)
                            .desired_width(110.0),
                    );
                    ui.label(RichText::new(t!("VLAN")).color(theme::text_dim()));
                    ui.add(
                        egui::TextEdit::singleline(&mut n.tag)
                            .char_limit(4)
                            .hint_text(RichText::new(t!("keins")).color(theme::text_dim()))
                            .desired_width(60.0),
                    );
                });
            }
        });
    }
    theme::form_caption(ui, t!("Änderungen"));
    match changes {
        Err(e) if !e.is_empty() => hint_color(ui, e, theme::red()),
        Ok((_, diff)) if diff.is_empty() => hint(ui, t!("Noch nichts geändert.")),
        Ok((_, diff)) => {
            group(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for (what, old, new) in diff {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(what).font(theme::medium(13.0)));
                                ui.label(theme::mono(old, 12.0).color(theme::text_dim()));
                                ui.label(RichText::new("→").color(theme::text_dim()));
                                ui.label(theme::mono(new, 12.0).color(theme::accent()));
                            });
                        }
                    });
            });
            if h.running {
                hint(
                    ui,
                    t!(
                        "Der Gast läuft: was Proxmox nicht im Betrieb ändern kann (z.B. Sockel, ohne Hotplug auch Kerne und RAM), wird beim nächsten Neustart aktiv."
                    ),
                );
            }
        }
        Err(_) => {}
    }
    if orig.has_pending {
        hint_color(
            ui,
            t!("Es gibt schon Änderungen, die auf einen Neustart warten."),
            theme::amber(),
        );
    }
    match task {
        Some((None, line)) => {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.add(egui::Spinner::new().size(14.0).color(theme::accent()));
                ui.label(RichText::new(t!("Wird übernommen…")).color(theme::text_dim()));
                if !line.is_empty() {
                    ui.add(
                        egui::Label::new(theme::mono(line, 11.5).color(theme::text_dim()))
                            .truncate(),
                    );
                }
            });
        }
        Some((Some(Ok(s)), _)) if s == "OK" => {
            hint_color(ui, t!("Änderungen übernommen."), theme::green());
        }
        Some((Some(Ok(s)), _)) => hint_color(ui, s, theme::amber()),
        Some((Some(Err(e)), _)) => hint_color(ui, e, theme::red()),
        None => {}
    }
}

fn hint_color(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).wrap());
    });
}

// ------------------------------------------------------------------ migration

pub struct MigrateSheet {
    pub g: GuestRef,
    running: bool,
    template: bool,
    target: String,
    nodes: Vec<String>,
    /// Empty = keep the storage names.
    storage: String,
    stores: Option<Result<pve::NodeStores, String>>,
}

pub fn open_migrate(app: &mut App, g: GuestRef, target: String) {
    let Some(s) = server_of(app, g.server) else {
        return;
    };
    let live = live_guest(app, g.server, g.vmid);
    let nodes: Vec<String> = online_nodes(app, g.server)
        .into_iter()
        .filter(|n| *n != g.node)
        .collect();
    if nodes.is_empty() {
        app.toast(t!("Kein anderer Node online"), theme::amber());
        return;
    }
    let target = if nodes.contains(&target) {
        target
    } else {
        nodes[0].clone()
    };
    let content = match g.kind {
        Kind::Qemu => "images",
        Kind::Lxc => "rootdir",
    };
    app.pve.ext.migrate = Some(MigrateSheet {
        running: live.as_ref().is_some_and(|x| x.running()),
        template: live.as_ref().is_some_and(|x| x.template),
        target,
        nodes: nodes.clone(),
        storage: String::new(),
        stores: None,
        g: g.clone(),
    });
    let c = conn(app, &s);
    pve::spawn_stores(
        &app.rt,
        s.id,
        app.pve.epoch(s.id),
        c,
        g.vmid,
        nodes,
        content,
        app.pve.sink.clone(),
    );
}

pub fn on_stores(app: &mut App, server: Uuid, vmid: u32, r: &Result<pve::NodeStores, String>) {
    if let Some(m) = app
        .pve
        .ext
        .migrate
        .as_mut()
        .filter(|m| m.g.server == server && m.g.vmid == vmid)
    {
        m.stores = Some(r.clone());
    }
}

pub fn migrate_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(m) = app.pve.ext.migrate.as_mut() else {
        return;
    };
    let title = t!("„{}“ migrieren", m.g.display());
    let stores: Vec<StoreInfo> = match &m.stores {
        Some(Ok(l)) => l
            .iter()
            .find(|(n, _)| *n == m.target)
            .map(|(_, s)| s.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let ((go, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-migrate"),
        520.0,
        &title,
        |ui| {
            theme::form(ui, 120.0, |ui, rows| {
                rows.field(ui, t!("Von"), |ui| {
                    ui.label(RichText::new(&m.g.node).font(theme::medium(14.0)));
                });
                rows.field(ui, t!("Nach"), |ui| {
                    egui::ComboBox::from_id_salt("pve-migrate-node")
                        .selected_text(m.target.clone())
                        .width(ui.available_width() - 14.0)
                        .show_ui(ui, |ui| {
                            for n in &m.nodes {
                                if ui.selectable_label(m.target == *n, n).clicked() {
                                    m.target = n.clone();
                                    m.storage.clear();
                                }
                            }
                        });
                });
                rows.field(ui, t!("Ziel-Speicher"), |ui| {
                    let sel = if m.storage.is_empty() {
                        t!("wie bisher").to_string()
                    } else {
                        m.storage.clone()
                    };
                    egui::ComboBox::from_id_salt("pve-migrate-store")
                        .selected_text(sel)
                        .width(ui.available_width() - 14.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut m.storage, String::new(), t!("wie bisher"));
                            for s in &stores {
                                let label = if s.avail > 0 {
                                    t!("{} ({}, {} frei)", s.id, s.kind, gib(s.avail))
                                } else {
                                    format!("{} ({})", s.id, s.kind)
                                };
                                ui.selectable_value(&mut m.storage, s.id.clone(), label);
                            }
                        });
                });
            });
            let (text, color) = match (m.g.kind, m.running) {
                (Kind::Qemu, true) => (
                    t!(
                        "Live-Migration: die VM läuft währenddessen weiter. Lokale Datenträger brauchen einen Ziel-Speicher."
                    ),
                    theme::text_dim(),
                ),
                (Kind::Lxc, true) => (
                    t!(
                        "Neustart-Migration: der Container wird gestoppt, verschoben und auf dem Ziel-Node wieder gestartet."
                    ),
                    theme::amber(),
                ),
                _ => (
                    t!("Offline-Migration: der Gast ist aus und wird nur verschoben."),
                    theme::text_dim(),
                ),
            };
            hint_color(ui, text, color);
            if m.template {
                hint_color(
                    ui,
                    t!("Vorlagen lassen sich nur mit gemeinsamem Speicher verschieben."),
                    theme::text_dim(),
                );
            }
            if let Some(Err(e)) = &m.stores {
                hint_color(ui, e, theme::red());
            }
        },
        |ui, _| {
            let go = theme::btn(ui, theme::Btn::Primary, None, t!("Migrieren")).clicked();
            let cancel = theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked();
            (go, cancel)
        },
    );
    if go {
        let m = app.pve.ext.migrate.take().expect("migrate sheet");
        let spec = MigrateSpec {
            target: m.target,
            running: m.running,
            storage: Some(m.storage).filter(|s| !s.is_empty()),
        };
        start_task_with(app, &m.g, Op::Ext(Box::new(ExtOp::Migrate(spec))), false);
    } else if cancel || dismissed {
        app.pve.ext.migrate = None;
    }
}
