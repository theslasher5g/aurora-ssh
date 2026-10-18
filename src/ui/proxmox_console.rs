//! "Konsole" of a guest: a saved host first, then SSH with the cloud-init user (asked in
//! a small sheet), a serial console over the API, `qm terminal` / `pct enter` on the node
//! or – without address and serial port – noVNC in the browser.

use std::collections::HashMap;

use egui::RichText;
use uuid::Uuid;

use super::{GuestRef, conn, first_ip, save_as_host, server_of};
use crate::app::{App, Purpose};
use crate::backend::proxmox::cluster::term::{GuestTarget, ShellParams};
use crate::backend::proxmox::{self as pve, ConsoleInfo, ConsoleRoute, Error, Kind};
use crate::model::{Host, Protocol, PveServer};
use crate::theme;
use crate::ui::icons::Icon;

#[derive(Default)]
pub struct ConsoleState {
    /// VMs whose console info is being fetched.
    pending: HashMap<(Uuid, u32), GuestRef>,
    login: Option<LoginSheet>,
}

impl ConsoleState {
    pub fn forget(&mut self, server: Uuid) {
        self.pending.retain(|(s, _), _| *s != server);
        if self
            .login
            .as_ref()
            .is_some_and(|l| l.guest.server == server)
        {
            self.login = None;
        }
    }
}

/// "Anmelden an <vm>": SSH user (and key) for a VM that is not saved as host yet.
pub struct LoginSheet {
    guest: GuestRef,
    ip: String,
    user: String,
    identity: Option<Uuid>,
    save: bool,
    serial: Option<String>,
    invalid: bool,
}

/// The host saved for this guest: the "Proxmox VM <vmid> · …" note in the server's
/// Proxmox group, or an SSH host with the guest's address.
pub fn saved_host<'a>(
    hosts: &'a [Host],
    group: Option<Uuid>,
    kind: Kind,
    vmid: u32,
    ip: Option<&str>,
) -> Option<&'a Host> {
    let marker = format!("Proxmox {} {} ", super::kind_tag(kind), vmid);
    let ssh = |h: &&Host| h.protocol == Protocol::Ssh;
    hosts
        .iter()
        .filter(ssh)
        .find(|h| group.is_some() && h.group_id == group && h.notes.starts_with(&marker))
        .or_else(|| {
            let ip = ip.map(str::trim).filter(|i| !i.is_empty())?;
            hosts.iter().filter(ssh).find(|h| h.hostname.trim() == ip)
        })
}

fn pve_group(app: &App, s: &PveServer) -> Option<Uuid> {
    let name = format!("Proxmox {}", s.display_name());
    app.lib.groups.iter().find(|g| g.name == name).map(|g| g.id)
}

/// Whether the server has a (still existing) SSH host for its nodes.
fn node_ssh(app: &App, s: &PveServer) -> Option<Host> {
    s.ssh_host.and_then(|h| app.lib.host(h)).cloned()
}

/// Entry point of `PveAction::Console`.
/// The saved host names its login user (itself, its key or its group). Hosts saved
/// before 0.7.1 often have none, and "root" is rarely allowed on a VM – those go through
/// the login sheet once, which then stores the user on the host.
fn knows_login(app: &App, h: &Host) -> bool {
    let (group_user, group_ident) = app.lib.inherited_defaults(h);
    !h.username.trim().is_empty()
        || !group_user.trim().is_empty()
        || h.identity_id
            .or(group_ident)
            .and_then(|id| app.identity(id))
            .is_some_and(|i| !i.username.trim().is_empty())
}

pub fn open(app: &mut App, g: GuestRef, ctx: &egui::Context) {
    let Some(s) = server_of(app, g.server) else {
        return;
    };
    if g.kind == Kind::Lxc {
        let has_ssh = node_ssh(app, &s).is_some();
        let route = pve::console_route(Kind::Lxc, &ConsoleInfo::default(), has_ssh);
        follow(app, &s, &g, route, ctx);
        return;
    }
    // A host saved for this VM knows user and key: use it right away.
    let known_ip = first_ip(app, g.server, g.vmid);
    if let Some(h) = saved_host(
        &app.lib.hosts,
        pve_group(app, &s),
        g.kind,
        g.vmid,
        known_ip.as_deref(),
    )
    .filter(|h| knows_login(app, h))
    .cloned()
    {
        app.connect_host(h, Purpose::Terminal, ctx);
        return;
    }
    let key = (g.server, g.vmid);
    if app.pve.console.pending.contains_key(&key) {
        return;
    }
    app.pve.console.pending.insert(key, g.clone());
    let c = conn(app, &s);
    pve::spawn_console_info(
        &app.rt,
        s.id,
        app.pve.epoch(s.id),
        c,
        g.id(),
        app.pve.sink.clone(),
    );
}

/// Result of [`pve::spawn_console_info`].
pub fn info_ready(app: &mut App, server: Uuid, vmid: u32, result: Result<ConsoleInfo, Error>) {
    let Some(g) = app.pve.console.pending.remove(&(server, vmid)) else {
        return;
    };
    let Some(s) = server_of(app, server) else {
        return;
    };
    let info = match result {
        Ok(i) => i,
        Err(e) => {
            app.toast(t!("{}: {}", g.label(), e), theme::red());
            return;
        }
    };
    let ctx = app.pve.sink.ctx();
    let ip = info.ip.map(|i| i.to_string());
    if let Some(h) = saved_host(
        &app.lib.hosts,
        pve_group(app, &s),
        g.kind,
        vmid,
        ip.as_deref(),
    )
    .filter(|h| knows_login(app, h))
    .cloned()
    {
        app.connect_host(h, Purpose::Terminal, &ctx);
        return;
    }
    let has_ssh = node_ssh(app, &s).is_some();
    match pve::console_route(g.kind, &info, has_ssh) {
        ConsoleRoute::Login { ip, user } => {
            // A clone made from Aurora remembers its cloud-init user and key.
            let (user, identity) = match app.pve.ext.connect_hint(server, vmid) {
                Some((u, i)) if !u.is_empty() => (u, i),
                Some((_, i)) => (user, i),
                None => (user, None),
            };
            app.pve.console.login = Some(LoginSheet {
                guest: g,
                ip,
                user,
                identity,
                save: true,
                serial: info.serial,
                invalid: false,
            });
        }
        route => follow(app, &s, &g, route, &ctx),
    }
}

/// Opens every route except the login sheet.
fn follow(app: &mut App, s: &PveServer, g: &GuestRef, route: ConsoleRoute, ctx: &egui::Context) {
    match route {
        ConsoleRoute::ApiTerm { serial } => api_console(app, s, g, serial, ctx),
        ConsoleRoute::NodeTerm { serial } => {
            let Some(via) = node_ssh(app, s) else {
                return;
            };
            match pve::node_console_line(g.kind, g.vmid, &g.node, serial.as_deref()) {
                Some(line) => app.open_exec(via, g.label(), line, ctx),
                None => app.toast(t!("ungültiger Node-Name"), theme::red()),
            }
        }
        ConsoleRoute::Browser => {
            let url = pve::novnc_url(&s.host, s.port, &g.id(), &g.name);
            crate::update::open_url(&url);
            app.toast(
                t!(
                    "„{}“: keine IP und keine serielle Schnittstelle – Konsole im Browser geöffnet",
                    g.display()
                ),
                theme::amber(),
            );
        }
        ConsoleRoute::Login { ip, user } => {
            app.connect_host(
                Host {
                    label: g.display(),
                    hostname: ip,
                    username: user,
                    ..Default::default()
                },
                Purpose::Terminal,
                ctx,
            );
        }
    }
}

/// Terminal tab on the guest through the API (`termproxy`), no SSH needed.
fn api_console(
    app: &mut App,
    s: &PveServer,
    g: &GuestRef,
    serial: Option<String>,
    ctx: &egui::Context,
) {
    if !pve::valid_node(&g.node) {
        app.toast(t!("ungültiger Node-Name"), theme::red());
        return;
    }
    if s.tls_pin.trim().is_empty() {
        app.toast(t!("Zertifikat noch nicht bestätigt"), theme::amber());
        return;
    }
    let subtitle = match g.kind {
        Kind::Qemu => t!("Serielle Konsole · {}", s.display_name()),
        Kind::Lxc => t!("Konsole · {}", s.display_name()),
    };
    let params = ShellParams {
        conn: conn(app, s),
        node: g.node.clone(),
        guest: Some(GuestTarget {
            kind: g.kind,
            vmid: g.vmid,
            serial,
        }),
    };
    app.open_pve_shell(g.display(), subtitle, params, ctx);
}

enum Cmd {
    Connect,
    Serial,
    Close,
}

/// The "Anmelden an …" sheet.
pub fn sheet(app: &mut App, ctx: &egui::Context) {
    let keys: Vec<(Uuid, String)> = app
        .keychain
        .identities
        .iter()
        .map(|i| (i.id, i.label.clone()))
        .collect();
    let Some(ls) = app.pve.console.login.as_mut() else {
        return;
    };
    let title = t!("Anmelden an {}", ls.guest.display());
    let has_serial = ls.serial.is_some();
    let (footer, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-vm-login"),
        460.0,
        &title,
        |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "SSH zu {} – Benutzer aus Cloud-Init, falls gesetzt.",
                        ls.ip
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                )
                .wrap(),
            );
            let mut enter = false;
            theme::form(ui, 110.0, |ui, rows| {
                rows.field(ui, t!("Benutzer"), |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut ls.user)
                            .char_limit(64)
                            .desired_width(f32::INFINITY),
                    );
                    if r.changed() {
                        ls.invalid = false;
                    }
                    enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
                rows.field(ui, t!("Identität"), |ui| {
                    let sel = ls
                        .identity
                        .and_then(|id| keys.iter().find(|(i, _)| *i == id))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| t!("— keine (Passwort) —").to_string());
                    egui::ComboBox::from_id_salt("pve-vm-login-key")
                        .selected_text(sel)
                        .width(ui.available_width() - 14.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut ls.identity, None, t!("— keine (Passwort) —"));
                            for (id, n) in &keys {
                                ui.selectable_value(&mut ls.identity, Some(*id), n);
                            }
                        });
                });
                rows.switch(ui, t!("Als Host speichern"), &mut ls.save);
            });
            if ls.invalid {
                ui.label(
                    RichText::new(t!("Ungültiger Benutzername"))
                        .size(12.5)
                        .color(theme::red()),
                );
            }
            enter
        },
        |ui, enter| {
            if theme::btn(
                ui,
                theme::Btn::Primary,
                Some(Icon::Terminal),
                t!("Verbinden"),
            )
            .clicked()
                || enter
            {
                return Some(Cmd::Connect);
            }
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                return Some(Cmd::Close);
            }
            if has_serial
                && theme::btn(ui, theme::Btn::Normal, None, t!("Serielle Konsole"))
                    .on_hover_text(t!("Serielle Schnittstelle der VM über die Proxmox-API"))
                    .clicked()
            {
                return Some(Cmd::Serial);
            }
            None
        },
    );
    let cmd = if dismissed { Some(Cmd::Close) } else { footer };
    match cmd {
        Some(Cmd::Close) => app.pve.console.login = None,
        Some(Cmd::Serial) => {
            let Some(ls) = app.pve.console.login.take() else {
                return;
            };
            if let Some(s) = server_of(app, ls.guest.server) {
                api_console(app, &s, &ls.guest, ls.serial, ctx);
            }
        }
        Some(Cmd::Connect) => {
            let Some(ls) = app.pve.console.login.as_mut() else {
                return;
            };
            ls.user = ls.user.trim().to_string();
            if !pve::valid_login_user(&ls.user) {
                ls.invalid = true;
                return;
            }
            let ls = app.pve.console.login.take().expect("sheet");
            connect(app, ls, ctx);
        }
        None => {}
    }
}

/// "Verbinden": saves the host (optional) with user and key, then opens SSH.
fn connect(app: &mut App, ls: LoginSheet, ctx: &egui::Context) {
    let fresh = Host {
        label: ls.guest.display(),
        hostname: ls.ip.clone(),
        username: ls.user.clone(),
        identity_id: ls.identity,
        ..Default::default()
    };
    if !ls.save {
        app.connect_host(fresh, Purpose::Terminal, ctx);
        return;
    }
    save_as_host(app, &ls.guest, ls.ip.clone());
    let group = server_of(app, ls.guest.server).and_then(|s| pve_group(app, &s));
    let saved = app.lib.hosts.iter_mut().find(|h| {
        h.protocol == Protocol::Ssh
            && h.group_id.is_some()
            && h.group_id == group
            && h.hostname == ls.ip
    });
    let host = match saved {
        Some(h) => {
            h.username = ls.user;
            if ls.identity.is_some() {
                h.identity_id = ls.identity;
            }
            let h = h.clone();
            app.save_library();
            h
        }
        // Not saved (the address belongs to another host): connect anyway.
        None => fresh,
    };
    app.connect_host(host, Purpose::Terminal, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(label: &str, hostname: &str, group: Option<Uuid>, notes: &str) -> Host {
        Host {
            label: label.into(),
            hostname: hostname.into(),
            group_id: group,
            notes: notes.into(),
            ..Default::default()
        }
    }

    #[test]
    fn finds_saved_host() {
        let grp = Uuid::new_v4();
        let other = Uuid::new_v4();
        let hosts = vec![
            host("web", "10.0.0.5", Some(grp), "Proxmox VM 100 · pve1"),
            host("db", "10.0.0.6", Some(other), "Proxmox VM 101 · pve1"),
            host("dns", "10.0.0.7", None, ""),
            Host {
                protocol: Protocol::Rdp,
                ..host("win", "10.0.0.8", Some(grp), "Proxmox VM 103 · pve1")
            },
        ];
        // Marker in the server's group, even with a changed address.
        let h = saved_host(&hosts, Some(grp), Kind::Qemu, 100, Some("10.9.9.9")).unwrap();
        assert_eq!(h.label, "web");
        // Marker of another server's group does not count …
        assert!(saved_host(&hosts, Some(grp), Kind::Qemu, 101, None).is_none());
        // … nor a container with the same id, nor a VMID prefix.
        assert!(saved_host(&hosts, Some(grp), Kind::Lxc, 100, None).is_none());
        assert!(saved_host(&hosts, Some(grp), Kind::Qemu, 10, None).is_none());
        // Same address anywhere.
        let h = saved_host(&hosts, Some(grp), Kind::Qemu, 200, Some(" 10.0.0.7")).unwrap();
        assert_eq!(h.label, "dns");
        // No group yet: only the address.
        assert!(saved_host(&hosts, None, Kind::Qemu, 100, None).is_none());
        // Non-SSH hosts are no shell.
        assert!(saved_host(&hosts, Some(grp), Kind::Qemu, 103, Some("10.0.0.8")).is_none());
        assert!(saved_host(&hosts, Some(grp), Kind::Qemu, 300, Some("")).is_none());
    }
}
