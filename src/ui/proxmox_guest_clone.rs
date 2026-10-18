//! Clone a guest, or create a new one from a template: name, VMID, target node,
//! full/linked clone, target storage and – for VMs with a cloud-init drive – user,
//! address and an SSH key from the keychain. Ends with "Verbinden".

use super::*;
use crate::backend::proxmox::{CloneSpec, CloudInit, StoreInfo};

/// What "Verbinden" uses for a cloned guest.
#[derive(Clone, Debug, Default)]
pub struct ConnectHint {
    pub user: String,
    pub identity: Option<Uuid>,
    /// Static cloud-init address (without prefix).
    pub ip: Option<String>,
}

pub struct CloneSheet {
    pub src: GuestRef,
    /// The source is a template (linked clones possible).
    template: bool,
    newid: Option<u32>,
    info_error: Option<String>,
    /// `None` while the source config loads.
    cloud_init: Option<bool>,
    name: String,
    target: String,
    nodes: Vec<String>,
    full: bool,
    /// Empty = same storage as the source.
    storage: String,
    stores: Option<Result<pve::NodeStores, String>>,
    ci_on: bool,
    ci_user: String,
    dhcp: bool,
    ip: String,
    gw: String,
    key: Option<Uuid>,
    start: bool,
    pub task: Option<u64>,
    error: Option<String>,
    /// The clone that was started (for "Verbinden").
    new_guest: Option<GuestRef>,
}

/// Name proposal: `web-01` → `web-01-clone`, template `debian-12-tpl` → `debian-12-01`.
fn proposed_name(name: &str, template: bool) -> String {
    if name.is_empty() {
        return String::new();
    }
    if template {
        let base = ["-template", "-tpl", "-vorlage"]
            .iter()
            .find_map(|s| name.strip_suffix(s))
            .unwrap_or(name);
        format!("{base}-01")
    } else {
        format!("{name}-clone")
    }
}

pub fn open(app: &mut App, g: GuestRef, from_template: bool) {
    let Some(s) = server_of(app, g.server) else {
        return;
    };
    let template = live_guest(app, g.server, g.vmid).is_some_and(|x| x.template) || from_template;
    let mut nodes = online_nodes(app, g.server);
    if !nodes.contains(&g.node) {
        nodes.insert(0, g.node.clone());
    }
    app.pve.ext.clone = Some(CloneSheet {
        template,
        newid: None,
        info_error: None,
        cloud_init: None,
        name: proposed_name(&g.name, template),
        target: g.node.clone(),
        nodes: nodes.clone(),
        full: !template,
        storage: String::new(),
        stores: None,
        ci_on: true,
        ci_user: String::new(),
        dhcp: true,
        ip: String::new(),
        gw: String::new(),
        key: None,
        start: !template || g.kind == Kind::Qemu,
        task: None,
        error: None,
        new_guest: None,
        src: g.clone(),
    });
    let epoch = app.pve.epoch(s.id);
    let c = conn(app, &s);
    pve::spawn_clone_info(
        &app.rt,
        s.id,
        epoch,
        c.clone(),
        g.id(),
        app.pve.sink.clone(),
    );
    let content = match g.kind {
        Kind::Qemu => "images",
        Kind::Lxc => "rootdir",
    };
    pve::spawn_stores(
        &app.rt,
        s.id,
        epoch,
        c,
        g.vmid,
        nodes,
        content,
        app.pve.sink.clone(),
    );
}

fn sheet_for(app: &mut App, server: Uuid, vmid: u32) -> Option<&mut CloneSheet> {
    app.pve
        .ext
        .clone
        .as_mut()
        .filter(|c| c.src.server == server && c.src.vmid == vmid && c.task.is_none())
}

pub fn on_info(app: &mut App, server: Uuid, vmid: u32, r: Result<(u32, bool), Error>) {
    if let Some(cs) = sheet_for(app, server, vmid) {
        match r {
            Ok((id, ci)) => {
                cs.newid = Some(id);
                cs.cloud_init = Some(ci);
            }
            Err(e) => {
                cs.info_error = Some(e.to_string());
                cs.cloud_init = Some(false);
            }
        }
    }
}

pub fn on_stores(app: &mut App, server: Uuid, vmid: u32, r: &Result<pve::NodeStores, String>) {
    if let Some(cs) = sheet_for(app, server, vmid) {
        cs.stores = Some(r.clone());
    }
}

impl CloneSheet {
    fn target_stores(&self) -> Vec<StoreInfo> {
        match &self.stores {
            Some(Ok(list)) => list
                .iter()
                .find(|(n, _)| *n == self.target)
                .map(|(_, s)| s.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn uses_cloud_init(&self) -> bool {
        self.src.kind == Kind::Qemu && self.cloud_init == Some(true) && self.ci_on
    }

    /// The spec without SSH keys (looked up when starting), or what is wrong.
    fn spec(&self) -> Result<CloneSpec, String> {
        let newid = self
            .newid
            .ok_or_else(|| t!("Suche freie VMID…").to_string())?;
        if !pve::valid_vmid(newid) || newid == self.src.vmid {
            return Err(t!("Ungültige VMID").into());
        }
        let name = self.name.trim();
        if !name.is_empty() && !pve::valid_guest_name(name) {
            return Err(t!("Name: nur Buchstaben, Ziffern, - und . (wie ein DNS-Name)").into());
        }
        if !pve::valid_node(&self.target) {
            return Err(t!("ungültiger Node-Name").into());
        }
        let full = self.full || !self.template;
        let storage = (full && !self.storage.is_empty()).then(|| self.storage.clone());
        if storage.as_ref().is_some_and(|s| !pve::valid_storage(s)) {
            return Err(t!("ungültige Eingabe").into());
        }
        let cloud_init = if self.uses_cloud_init() {
            let user = self.ci_user.trim();
            if !user.is_empty() && !pve::valid_ciuser(user) {
                return Err(t!(
                    "Benutzer: Kleinbuchstaben, Ziffern, _ und - (mit Buchstabe oder _ am Anfang)"
                )
                .into());
            }
            let ip = if self.dhcp {
                None
            } else {
                let ip = (self.ip.trim().to_string(), self.gw.trim().to_string());
                if pve::ipconfig_value(Some((&ip.0, &ip.1))).is_none() {
                    return Err(t!(
                        "Adresse als IP/Präfix angeben (z.B. 192.168.1.50/24), Gateway als IP derselben Familie"
                    )
                    .into());
                }
                Some(ip)
            };
            Some(CloudInit {
                user: user.to_string(),
                ip,
                sshkeys: Vec::new(),
            })
        } else {
            None
        };
        Ok(CloneSpec {
            newid,
            name: name.to_string(),
            target: self.target.clone(),
            full,
            storage,
            from_template: self.template,
            cloud_init,
            start: self.start,
        })
    }
}

enum Cmd {
    Start,
    Connect,
    Close,
}

pub fn sheet(app: &mut App, ctx: &egui::Context) {
    let keys: Vec<(Uuid, String)> = app
        .keychain
        .identities
        .iter()
        .filter(|i| !i.private_key.trim().is_empty())
        .map(|i| (i.id, i.label.clone()))
        .collect();
    let task = app
        .pve
        .ext
        .clone
        .as_ref()
        .and_then(|c| c.task)
        .and_then(|id| app.pve.tasks.iter().find(|t| t.id == id))
        .map(|t| (t.done.clone(), t.progress.clone(), t.started));
    let Some(cs) = app.pve.ext.clone.as_mut() else {
        return;
    };
    let title = if cs.template {
        t!("Neu aus Vorlage „{}“", cs.src.display())
    } else {
        t!("„{}“ klonen", cs.src.display())
    };
    let spec = cs.spec();
    let started = cs.task.is_some();
    let ok = spec.is_ok();
    let template = cs.template;
    let (footer, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-clone"),
        580.0,
        &title,
        |ui| {
            if cs.task.is_some() {
                progress(ui, cs, task.as_ref());
            } else {
                form(ui, cs, &keys, &spec);
            }
        },
        |ui, _| {
            match (started, task.as_ref().and_then(|t| t.0.as_ref())) {
                (true, Some(Ok(_))) => {
                    if theme::btn(
                        ui,
                        theme::Btn::Primary,
                        Some(Icon::Terminal),
                        t!("Verbinden"),
                    )
                    .on_hover_text(t!("Als Host speichern und eine SSH-Sitzung öffnen"))
                    .clicked()
                    {
                        return Some(Cmd::Connect);
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                        return Some(Cmd::Close);
                    }
                }
                (true, _) => {
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                        return Some(Cmd::Close);
                    }
                }
                (false, _) => {
                    let label = if template {
                        t!("Erstellen")
                    } else {
                        t!("Klonen")
                    };
                    if ui
                        .add_enabled_ui(ok, |ui| theme::btn(ui, theme::Btn::Primary, None, label))
                        .inner
                        .clicked()
                    {
                        return Some(Cmd::Start);
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                        return Some(Cmd::Close);
                    }
                }
            }
            None
        },
    );
    let mut cmd = footer;
    if dismissed {
        cmd = Some(Cmd::Close);
    }
    match cmd {
        Some(Cmd::Start) => start(app),
        Some(Cmd::Connect) => {
            if let Some(g) = app.pve.ext.clone.take().and_then(|c| c.new_guest) {
                connect(app, g);
            }
        }
        Some(Cmd::Close) => app.pve.ext.clone = None,
        None => {}
    }
}

fn form(
    ui: &mut egui::Ui,
    cs: &mut CloneSheet,
    keys: &[(Uuid, String)],
    spec: &Result<CloneSpec, String>,
) {
    let stores = cs.target_stores();
    theme::form(ui, 120.0, |ui, rows| {
        rows.field(ui, t!("Name"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut cs.name)
                    .char_limit(63)
                    .desired_width(f32::INFINITY),
            );
        });
        rows.field(ui, t!("Neue VMID"), |ui| match cs.newid.as_mut() {
            Some(id) => {
                ui.add(egui::DragValue::new(id).range(100..=999_999_999));
            }
            None => {
                ui.add(egui::Spinner::new().size(14.0));
            }
        });
        rows.field(ui, t!("Ziel-Node"), |ui| {
            egui::ComboBox::from_id_salt("pve-clone-node")
                .selected_text(cs.target.clone())
                .width(ui.available_width() - 14.0)
                .show_ui(ui, |ui| {
                    for n in &cs.nodes {
                        if ui.selectable_label(cs.target == *n, n).clicked() && cs.target != *n {
                            cs.target = n.clone();
                            cs.storage.clear();
                        }
                    }
                });
        });
        if cs.template {
            rows.field(ui, t!("Art"), |ui| {
                let cur = if cs.full { 0 } else { 1 };
                if let Some(i) = theme::segmented(ui, &[t!("Vollständig"), t!("Verknüpft")], cur)
                {
                    cs.full = i == 0;
                }
            });
        }
        if cs.full || !cs.template {
            rows.field(ui, t!("Speicher"), |ui| {
                let sel = stores
                    .iter()
                    .find(|s| s.id == cs.storage)
                    .map(store_text)
                    .unwrap_or_else(|| t!("wie Quelle").to_string());
                egui::ComboBox::from_id_salt("pve-clone-store")
                    .selected_text(sel)
                    .width(ui.available_width() - 14.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut cs.storage, String::new(), t!("wie Quelle"));
                        for s in &stores {
                            ui.selectable_value(&mut cs.storage, s.id.clone(), store_text(s));
                        }
                    });
            });
        }
        rows.switch(ui, t!("Danach starten"), &mut cs.start);
    });
    let hint_text = if cs.template && !cs.full {
        t!(
            "Verknüpfter Klon: teilt die Datenträger der Vorlage – schnell und platzsparend, die Vorlage muss bleiben."
        )
    } else {
        t!("Vollständiger Klon: eine unabhängige Kopie aller Datenträger.")
    };
    note(ui, hint_text, theme::text_dim());

    if cs.src.kind == Kind::Qemu {
        match cs.cloud_init {
            None => note(ui, t!("Prüfe Cloud-Init…"), theme::text_dim()),
            Some(false) => {}
            Some(true) => {
                theme::form_caption(ui, t!("Cloud-Init"));
                theme::form(ui, 120.0, |ui, rows| {
                    rows.switch(ui, t!("Cloud-Init anpassen"), &mut cs.ci_on);
                    if !cs.ci_on {
                        return;
                    }
                    rows.field(ui, t!("Benutzer"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut cs.ci_user)
                                .char_limit(32)
                                .hint_text(
                                    RichText::new(t!("Standard des Images"))
                                        .color(theme::text_dim()),
                                )
                                .desired_width(f32::INFINITY),
                        );
                    });
                    rows.field(ui, t!("Netzwerk"), |ui| {
                        let cur = if cs.dhcp { 0 } else { 1 };
                        if let Some(i) = theme::segmented(ui, &[t!("DHCP"), t!("Statisch")], cur) {
                            cs.dhcp = i == 0;
                        }
                    });
                    if !cs.dhcp {
                        rows.field(ui, t!("IP/Präfix"), |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut cs.ip)
                                    .hint_text(
                                        RichText::new("192.168.1.50/24").color(theme::text_dim()),
                                    )
                                    .desired_width(f32::INFINITY),
                            );
                        });
                        rows.field(ui, t!("Gateway"), |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut cs.gw)
                                    .hint_text(
                                        RichText::new("192.168.1.1").color(theme::text_dim()),
                                    )
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    }
                    rows.field(ui, t!("SSH-Key"), |ui| {
                        let sel = cs
                            .key
                            .and_then(|id| keys.iter().find(|(i, _)| *i == id))
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| t!("— keiner —").to_string());
                        egui::ComboBox::from_id_salt("pve-clone-key")
                            .selected_text(sel)
                            .width(ui.available_width() - 14.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut cs.key, None, t!("— keiner —"));
                                for (id, n) in keys {
                                    ui.selectable_value(&mut cs.key, Some(*id), n);
                                }
                            });
                    });
                });
                if cs.ci_on {
                    note(
                        ui,
                        t!(
                            "Der öffentliche Schlüssel wird aus dem Schlüsselbund abgeleitet; der private bleibt in Aurora."
                        ),
                        theme::text_dim(),
                    );
                }
            }
        }
    }
    if let Some(e) = &cs.info_error {
        note(ui, e, theme::red());
    }
    if let Some(e) = &cs.error {
        note(ui, e, theme::red());
    }
    if let Err(e) = spec
        && cs.newid.is_some()
    {
        note(ui, e, theme::red());
    }
}

fn store_text(s: &StoreInfo) -> String {
    if s.avail > 0 {
        t!("{} ({}, {} frei)", s.id, s.kind, gib(s.avail))
    } else {
        format!("{} ({})", s.id, s.kind)
    }
}

fn note(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).wrap());
    });
}

type TaskState = (Option<Result<String, String>>, String, Instant);

fn progress(ui: &mut egui::Ui, cs: &CloneSheet, task: Option<&TaskState>) {
    let new = cs.new_guest.as_ref();
    let what = new
        .map(|g| {
            format!(
                "{} {} · {} · {}",
                kind_tag(g.kind),
                g.vmid,
                g.display(),
                g.node
            )
        })
        .unwrap_or_default();
    ui.add_space(6.0);
    group(ui, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    match task.and_then(|t| t.0.as_ref()) {
                        None => {
                            ui.add(egui::Spinner::new().size(16.0).color(theme::accent()));
                            ui.label(RichText::new(t!("Klonen läuft…")).font(theme::semibold(14.0)));
                        }
                        Some(Ok(_)) => {
                            ui.label(RichText::new("✔").color(theme::green()));
                            ui.label(RichText::new(t!("Fertig")).font(theme::semibold(14.0)));
                        }
                        Some(Err(_)) => {
                            ui.label(RichText::new("✖").color(theme::red()));
                            ui.label(
                                RichText::new(t!("Fehlgeschlagen")).font(theme::semibold(14.0)),
                            );
                        }
                    }
                });
                ui.label(theme::mono(&what, 12.5).color(theme::text_dim()));
                match task {
                    Some((None, line, started)) => {
                        ui.label(
                            RichText::new(super::elapsed(started.elapsed()))
                                .size(12.5)
                                .color(theme::text_dim()),
                        );
                        if !line.is_empty() {
                            ui.add(
                                egui::Label::new(theme::mono(line, 11.5).color(theme::text_dim()))
                                    .truncate(),
                            );
                        }
                    }
                    Some((Some(Err(e)), _, _)) => {
                        ui.add(egui::Label::new(RichText::new(e).color(theme::red())).wrap());
                    }
                    Some((Some(Ok(_)), _, _)) => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(t!(
                                    "„Verbinden“ speichert den Gast als Host und öffnet eine SSH-Sitzung, sobald er eine Adresse hat."
                                ))
                                .size(12.5)
                                .color(theme::text_dim()),
                            )
                            .wrap(),
                        );
                    }
                    None => {}
                }
            });
    });
}

fn start(app: &mut App) {
    let Some(cs) = app.pve.ext.clone.as_ref() else {
        return;
    };
    let mut spec = match cs.spec() {
        Ok(s) => s,
        Err(e) => {
            if let Some(cs) = app.pve.ext.clone.as_mut() {
                cs.error = Some(e);
            }
            return;
        }
    };
    let (src, key) = (cs.src.clone(), cs.key);
    if let (Some(ci), Some(id)) = (spec.cloud_init.as_mut(), key) {
        let pubkey = app
            .keychain
            .identities
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| t!("Schlüssel nicht gefunden").to_string())
            .and_then(|i| {
                crate::keygen::public_info(&i.private_key, &i.passphrase)
                    .map(|(p, _)| p)
                    .map_err(|e| t!("Key ungültig: {}", e))
            });
        match pubkey {
            Ok(p) => ci.sshkeys = vec![p],
            Err(e) => {
                if let Some(cs) = app.pve.ext.clone.as_mut() {
                    cs.error = Some(e);
                }
                return;
            }
        }
    }
    let new = GuestRef {
        server: src.server,
        node: spec.target.clone(),
        kind: src.kind,
        vmid: spec.newid,
        name: spec.name.clone(),
    };
    let hint = ConnectHint {
        user: spec
            .cloud_init
            .as_ref()
            .map(|c| c.user.clone())
            .unwrap_or_default(),
        identity: spec.cloud_init.as_ref().and(key),
        ip: spec
            .cloud_init
            .as_ref()
            .and_then(|c| c.ip.as_ref())
            .and_then(|(cidr, _)| cidr.split('/').next())
            .map(str::to_string),
    };
    app.pve.ext.hints.insert((src.server, spec.newid), hint);
    let id = start_task_with(app, &src, Op::Ext(Box::new(ExtOp::Clone(spec))), false);
    if let Some(cs) = app.pve.ext.clone.as_mut() {
        cs.task = Some(id);
        cs.new_guest = Some(new);
    }
}

/// "Verbinden" for a guest: its static address if known, else ask Proxmox.
pub fn connect(app: &mut App, g: GuestRef) {
    let ip = app
        .pve
        .ext
        .hints
        .get(&(g.server, g.vmid))
        .and_then(|h| h.ip.clone());
    match ip {
        Some(ip) => save_and_connect(app, &g, ip),
        None => lookup_address(app, &g, AddressFor::Connect),
    }
}

/// Saves the guest as host (with the cloud-init user and key) and opens a session.
pub fn save_and_connect(app: &mut App, g: &GuestRef, ip: String) {
    save_as_host(app, g, ip.clone());
    let hint = app.pve.ext.hints.get(&(g.server, g.vmid)).cloned();
    let Some(h) = app.lib.hosts.iter_mut().find(|h| h.hostname == ip) else {
        return;
    };
    if let Some(hint) = hint {
        if h.username.is_empty() && !hint.user.is_empty() {
            h.username = hint.user;
        }
        if h.identity_id.is_none() {
            h.identity_id = hint.identity;
        }
    }
    let host = h.clone();
    app.save_library();
    let ctx = app.pve.sink.ctx();
    app.connect_host(host, Purpose::Terminal, &ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(proposed_name("web-01", false), "web-01-clone");
        assert_eq!(proposed_name("debian-12-tpl", true), "debian-12-01");
        assert_eq!(proposed_name("ubuntu", true), "ubuntu-01");
        assert_eq!(proposed_name("", true), "");
    }
}
