//! Proxmox VE page: every VM and container of the configured servers, grouped by server
//! and node, with power actions, consoles and "save as host". Kept apart from the host
//! list on purpose – guests come and go, the API is the source of truth.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{Action, App, Purpose};
use crate::backend::Sink;
use crate::backend::proxmox::{
    self as pve, Auth, Conn, Console, Error, Guest, Ips, Kind, Power, PveEvent, Session, Snapshot,
};
use crate::model::{Group, Host, Protocol, PveServer};
use crate::theme;
use crate::ui::fuzzy;
use crate::ui::icons::{self, Icon};

const ROW_H: f32 = 66.0;
/// Refresh period while the page is visible.
const POLL: Duration = Duration::from_secs(10);
/// Guest addresses are looked up again after this long.
const IP_TTL: Duration = Duration::from_secs(60);

/// Live data of one server (not persisted).
#[derive(Default)]
pub struct Live {
    pub loading: bool,
    pub error: Option<Error>,
    pub snap: Option<Snapshot>,
    pub session: Option<Session>,
    pub ips: HashMap<u32, (Ips, Instant)>,
    /// Guests with a power action in flight.
    pub busy: HashSet<u32>,
    pub last: Option<Instant>,
}

pub struct Editor {
    pub server: PveServer,
    pub is_new: bool,
    pub secret: String,
    pub testing: bool,
    pub result: Option<Result<String, String>>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TrustTarget {
    Editor,
    Server(Uuid),
}

/// "Trust this certificate?" for a server seen for the first time.
pub struct Trust {
    pub target: TrustTarget,
    pub host: String,
    pub fingerprint: String,
}

pub struct PveState {
    sink: Sink<PveEvent>,
    rx: UnboundedReceiver<PveEvent>,
    pub live: HashMap<Uuid, Live>,
    pub search: String,
    /// 0 all, 1 VMs, 2 containers, 3 running.
    pub filter: usize,
    pub show_templates: bool,
    pub editor: Option<Editor>,
    pub trust: Option<Trust>,
}

impl PveState {
    pub fn new(ctx: &egui::Context) -> Self {
        let (sink, rx) = Sink::new(ctx.clone());
        Self {
            sink,
            rx,
            live: HashMap::new(),
            search: String::new(),
            filter: 0,
            show_templates: false,
            editor: None,
            trust: None,
        }
    }

    /// Vault locked: forget tickets and anything showing secrets.
    pub fn lock(&mut self) {
        self.live.clear();
        self.editor = None;
        self.trust = None;
    }
}

/// A guest addressed from an action.
#[derive(Clone, Debug)]
pub struct GuestRef {
    pub server: Uuid,
    pub node: String,
    pub kind: Kind,
    pub vmid: u32,
    pub name: String,
}

impl GuestRef {
    fn of(server: Uuid, g: &Guest) -> Self {
        Self {
            server,
            node: g.node.clone(),
            kind: g.kind,
            vmid: g.vmid,
            name: g.name.clone(),
        }
    }

    fn label(&self) -> String {
        format!("{} {} {}", kind_tag(self.kind), self.vmid, self.name)
    }
}

#[derive(Clone, Debug)]
pub enum PveAction {
    NewServer,
    EditServer(Uuid),
    DeleteServer(Uuid),
    Refresh,
    Power(GuestRef, Power),
    Console(GuestRef),
    SaveAsHost(GuestRef),
}

impl PveAction {
    /// Title and button of the confirmation dialog, for actions that are not deletions.
    pub fn confirm_labels(&self) -> Option<(&'static str, &'static str)> {
        match self {
            PveAction::Power(_, Power::Stop) => Some((t!("Hart stoppen?"), t!("Stoppen"))),
            PveAction::Power(_, Power::Shutdown) => {
                Some((t!("Herunterfahren?"), t!("Herunterfahren")))
            }
            _ => None,
        }
    }
}

fn kind_tag(k: Kind) -> &'static str {
    match k {
        Kind::Qemu => "VM",
        Kind::Lxc => "CT",
    }
}

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Qemu => theme::accent2(),
        Kind::Lxc => theme::green(),
    }
}

// ------------------------------------------------------------------ backend glue

fn conn(app: &App, s: &PveServer) -> Conn {
    let secret = app
        .keychain
        .pve_secrets
        .get(&s.id)
        .cloned()
        .unwrap_or_default();
    conn_with(
        s,
        secret,
        app.pve.live.get(&s.id).and_then(|l| l.session.clone()),
    )
}

fn conn_with(s: &PveServer, secret: String, session: Option<Session>) -> Conn {
    let user = s.user.trim().to_string();
    Conn {
        host: s.host.trim().to_string(),
        port: s.port,
        auth: if s.token {
            Auth::Token { id: user, secret }
        } else {
            Auth::Password {
                user,
                password: secret,
            }
        },
        pin: Some(s.tls_pin.trim().to_string()).filter(|p| !p.is_empty()),
        session,
    }
}

fn refresh(app: &mut App, id: Uuid) {
    let Some(s) = app.lib.pve_servers.iter().find(|s| s.id == id).cloned() else {
        return;
    };
    if s.host.trim().is_empty() {
        return;
    }
    let c = conn(app, &s);
    let live = app.pve.live.entry(id).or_default();
    if live.loading {
        return;
    }
    live.loading = true;
    live.last = Some(Instant::now());
    // Addresses of running guests, unless looked up recently.
    let ips_for: Vec<(String, Kind, u32)> = live
        .snap
        .as_ref()
        .map(|snap| {
            snap.guests
                .iter()
                .filter(|g| g.running() && !g.template)
                .filter(|g| {
                    live.ips
                        .get(&g.vmid)
                        .is_none_or(|(_, at)| at.elapsed() >= IP_TTL)
                })
                .map(|g| (g.node.clone(), g.kind, g.vmid))
                .collect()
        })
        .unwrap_or_default();
    pve::spawn_fetch(&app.rt, id, c, ips_for, app.pve.sink.clone());
}

fn refresh_all(app: &mut App) {
    let ids: Vec<Uuid> = app.lib.pve_servers.iter().map(|s| s.id).collect();
    for id in ids {
        refresh(app, id);
    }
}

/// Applies backend results; called every frame.
pub fn pump(app: &mut App) {
    while let Ok(ev) = app.pve.rx.try_recv() {
        match ev {
            PveEvent::Fetched {
                server,
                result,
                session,
            } => {
                let first_ips = {
                    let live = app.pve.live.entry(server).or_default();
                    live.loading = false;
                    live.session = session;
                    match result {
                        Ok(snap) => {
                            let now = Instant::now();
                            for (vmid, ips) in &snap.ips {
                                live.ips.insert(*vmid, (ips.clone(), now));
                            }
                            // First load: look up the addresses right away.
                            let first = live.snap.is_none();
                            live.snap = Some(snap);
                            live.error = None;
                            first
                        }
                        Err(e) => {
                            if let Error::Untrusted(fp) = &e
                                && app.pve.trust.is_none()
                            {
                                let host = app
                                    .lib
                                    .pve_servers
                                    .iter()
                                    .find(|s| s.id == server)
                                    .map(|s| format!("{}:{}", s.host.trim(), s.port))
                                    .unwrap_or_default();
                                app.pve.trust = Some(Trust {
                                    target: TrustTarget::Server(server),
                                    host,
                                    fingerprint: fp.clone(),
                                });
                            }
                            if matches!(e, Error::PinChanged { .. }) {
                                // Never show data from a server we no longer trust.
                                live.snap = None;
                                live.session = None;
                            }
                            live.error = Some(e);
                            false
                        }
                    }
                };
                if first_ips {
                    refresh(app, server);
                }
            }
            PveEvent::PowerDone {
                server,
                vmid,
                power,
                result,
                session,
            } => {
                if let Some(live) = app.pve.live.get_mut(&server) {
                    live.busy.remove(&vmid);
                    live.session = session;
                    live.ips.remove(&vmid);
                }
                match result {
                    Ok(()) => {
                        let what = match power {
                            Power::Start => t!("Start"),
                            Power::Shutdown => t!("Herunterfahren"),
                            Power::Stop => t!("Stopp"),
                            Power::Reboot => t!("Neustart"),
                            Power::Resume => t!("Fortsetzen"),
                        };
                        app.toast(t!("{}: {} ausgelöst", vmid, what), theme::green());
                        // Show the new state soon (the task runs asynchronously).
                        if let Some(live) = app.pve.live.get_mut(&server) {
                            live.last = Some(Instant::now() - POLL + Duration::from_secs(2));
                        }
                        refresh(app, server);
                    }
                    Err(e) => app.toast(t!("{}: {}", vmid, e), theme::red()),
                }
            }
            PveEvent::Tested(result) => {
                let Some(ed) = app.pve.editor.as_mut() else {
                    continue;
                };
                ed.testing = false;
                match result {
                    Ok(v) => ed.result = Some(Ok(v)),
                    Err(Error::Untrusted(fp)) => {
                        ed.result = None;
                        app.pve.trust = Some(Trust {
                            target: TrustTarget::Editor,
                            host: format!("{}:{}", ed.server.host.trim(), ed.server.port),
                            fingerprint: fp,
                        });
                    }
                    Err(e) => ed.result = Some(Err(e.to_string())),
                }
            }
        }
    }
}

fn first_ip(app: &App, server: Uuid, vmid: u32) -> Option<String> {
    match app.pve.live.get(&server)?.ips.get(&vmid) {
        Some((Ips::Found(v), _)) => v.first().cloned(),
        _ => None,
    }
}

pub fn apply(app: &mut App, action: PveAction, ctx: &egui::Context) {
    match action {
        PveAction::NewServer => {
            app.pve.editor = Some(Editor {
                server: PveServer {
                    user: "root@pam!aurora".into(),
                    ..Default::default()
                },
                is_new: true,
                secret: String::new(),
                testing: false,
                result: None,
            });
        }
        PveAction::EditServer(id) => {
            if let Some(s) = app.lib.pve_servers.iter().find(|s| s.id == id) {
                app.pve.editor = Some(Editor {
                    server: s.clone(),
                    is_new: false,
                    secret: app
                        .keychain
                        .pve_secrets
                        .get(&id)
                        .cloned()
                        .unwrap_or_default(),
                    testing: false,
                    result: None,
                });
            }
        }
        PveAction::DeleteServer(id) => {
            app.lib.pve_servers.retain(|s| s.id != id);
            app.pve.live.remove(&id);
            if app.keychain.pve_secrets.remove(&id).is_some() {
                app.save_keychain();
            }
            app.save_library();
        }
        PveAction::Refresh => {
            for live in app.pve.live.values_mut() {
                live.ips.clear();
            }
            refresh_all(app);
        }
        PveAction::Power(g, power) => {
            let Some(s) = app
                .lib
                .pve_servers
                .iter()
                .find(|s| s.id == g.server)
                .cloned()
            else {
                return;
            };
            let c = conn(app, &s);
            app.pve.live.entry(s.id).or_default().busy.insert(g.vmid);
            pve::spawn_power(
                &app.rt,
                s.id,
                c,
                (g.node.clone(), g.kind, g.vmid),
                power,
                app.pve.sink.clone(),
            );
        }
        PveAction::Console(g) => console(app, &g, ctx),
        PveAction::SaveAsHost(g) => save_as_host(app, &g),
    }
}

fn console(app: &mut App, g: &GuestRef, ctx: &egui::Context) {
    let Some(s) = app
        .lib
        .pve_servers
        .iter()
        .find(|s| s.id == g.server)
        .cloned()
    else {
        return;
    };
    let ip = first_ip(app, g.server, g.vmid);
    match pve::console_command(g.kind, g.vmid, &g.node, ip.as_deref()) {
        Some(Console::Ssh(ip)) => {
            let host = Host {
                label: if g.name.is_empty() {
                    format!("VM {}", g.vmid)
                } else {
                    g.name.clone()
                },
                hostname: ip,
                ..Default::default()
            };
            app.connect_host(host, Purpose::Terminal, ctx);
        }
        Some(Console::Node(line)) => {
            let Some(via) = s.ssh_host.and_then(|h| app.lib.host(h)).cloned() else {
                app.toast(
                    t!("Lege im Server-Editor einen SSH-Host für die Nodes fest"),
                    theme::amber(),
                );
                app.pve.editor = None;
                apply(app, PveAction::EditServer(s.id), ctx);
                return;
            };
            app.open_exec(via, g.label(), line, ctx);
        }
        None => app.toast(t!("ungültiger Node-Name"), theme::red()),
    }
}

fn save_as_host(app: &mut App, g: &GuestRef) {
    let Some(s) = app.lib.pve_servers.iter().find(|s| s.id == g.server) else {
        return;
    };
    let group_name = format!("Proxmox {}", s.display_name());
    let gid = match app.lib.groups.iter().find(|x| x.name == group_name) {
        Some(x) => x.id,
        None => {
            let grp = Group {
                name: group_name.clone(),
                icon: "🗄".into(),
                color: [230, 112, 0],
                ..Default::default()
            };
            let id = grp.id;
            app.lib.groups.push(grp);
            id
        }
    };
    let ip = first_ip(app, g.server, g.vmid);
    let hostname = ip.clone().unwrap_or_else(|| g.name.clone());
    if app
        .lib
        .hosts
        .iter()
        .any(|h| h.group_id == Some(gid) && h.hostname == hostname)
    {
        app.toast(
            t!("„{}“ ist schon in „{}“", g.name, group_name),
            theme::amber(),
        );
        return;
    }
    let tags = app
        .pve
        .live
        .get(&g.server)
        .and_then(|l| l.snap.as_ref())
        .and_then(|s| s.guests.iter().find(|x| x.vmid == g.vmid))
        .map(|x| x.tags.clone())
        .unwrap_or_default();
    app.lib.hosts.push(Host {
        label: if g.name.is_empty() {
            format!("{} {}", kind_tag(g.kind), g.vmid)
        } else {
            g.name.clone()
        },
        hostname,
        group_id: Some(gid),
        tags,
        notes: format!("Proxmox {} {} · {}", kind_tag(g.kind), g.vmid, g.node),
        ..Default::default()
    });
    app.save_library();
    if ip.is_some() {
        app.toast(
            t!("„{}“ unter „{}“ gespeichert", g.name, group_name),
            theme::green(),
        );
    } else {
        app.toast(
            t!(
                "„{}“ gespeichert – ohne bekannte IP, Adresse bitte prüfen",
                g.name
            ),
            theme::amber(),
        );
    }
}

/// Command palette entries: one console per guest.
pub fn palette_items(app: &App) -> Vec<(String, String, String, Action)> {
    let mut out = Vec::new();
    for s in &app.lib.pve_servers {
        let Some(snap) = app.pve.live.get(&s.id).and_then(|l| l.snap.as_ref()) else {
            continue;
        };
        for g in snap.guests.iter().filter(|g| !g.template) {
            let r = GuestRef::of(s.id, g);
            out.push((
                kind_tag(g.kind).to_string(),
                r.label(),
                format!("{} · {} · {}", t!("Konsole"), g.node, s.display_name()),
                Action::Pve(PveAction::Console(r)),
            ));
        }
    }
    out
}

// ------------------------------------------------------------------ page

fn passes(filter: usize, show_templates: bool, g: &Guest) -> bool {
    if g.template && !show_templates {
        return false;
    }
    match filter {
        1 => g.kind == Kind::Qemu,
        2 => g.kind == Kind::Lxc,
        3 => g.running(),
        _ => true,
    }
}

fn matches_query(q: &str, g: &Guest, ip: Option<&str>) -> bool {
    if q.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {} {} {} {}",
        g.name,
        g.vmid,
        kind_tag(g.kind),
        g.node,
        g.tags.join(" "),
        ip.unwrap_or_default()
    );
    fuzzy(q, &hay).is_some_and(|s| s > 0)
}

fn gib(bytes: u64) -> String {
    let g = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if g >= 10.0 {
        format!("{g:.0} GiB")
    } else if g >= 1.0 {
        format!("{g:.1} GiB")
    } else {
        format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn uptime(secs: u64) -> String {
    match (secs / 86_400, secs / 3600) {
        (d, _) if d > 0 => format!("up {d} d"),
        (_, h) if h > 0 => format!("up {h} h"),
        _ => format!("up {} min", secs / 60),
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    // Auto refresh while the page is visible.
    let due: Vec<Uuid> = app
        .lib
        .pve_servers
        .iter()
        .filter(|s| {
            app.pve
                .live
                .get(&s.id)
                .is_none_or(|l| !l.loading && l.last.is_none_or(|t| t.elapsed() >= POLL))
        })
        .map(|s| s.id)
        .collect();
    for id in due {
        // A pending trust question or a changed certificate waits for the user.
        let blocked = app
            .pve
            .live
            .get(&id)
            .and_then(|l| l.error.as_ref())
            .is_some_and(|e| matches!(e, Error::Untrusted(_) | Error::PinChanged { .. }));
        if !blocked {
            refresh(app, id);
        }
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));

    let total: usize = app
        .lib
        .pve_servers
        .iter()
        .filter_map(|s| app.pve.live.get(&s.id).and_then(|l| l.snap.as_ref()))
        .map(|s| {
            s.guests
                .iter()
                .filter(|g| app.pve.show_templates || !g.template)
                .count()
        })
        .sum();
    ui.horizontal(|ui| {
        theme::page_title(ui, "Proxmox", Some(total));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("Server"))
                .on_hover_text(t!("Proxmox-Server hinzufügen"))
                .clicked()
            {
                actions.push(Action::Pve(PveAction::NewServer));
            }
            if !app.lib.pve_servers.is_empty() {
                if theme::icon_btn(ui, Icon::Refresh, t!("Jetzt aktualisieren")).clicked() {
                    actions.push(Action::Pve(PveAction::Refresh));
                }
                let width = (ui.available_width() - 24.0).clamp(180.0, 300.0);
                theme::search_field(
                    ui,
                    &mut app.pve.search,
                    t!("Name, VMID, Tag oder IP"),
                    width,
                    egui::Id::new("pve-search"),
                );
            }
        });
    });
    ui.add_space(18.0);

    if app.lib.pve_servers.is_empty() {
        empty_state(ui, actions);
        return;
    }

    ui.horizontal(|ui| {
        if let Some(i) = theme::segmented(
            ui,
            &["Alle", "VMs", "Container", "Laufend"].map(crate::i18n::tr),
            app.pve.filter,
        ) {
            app.pve.filter = i;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let kind = if app.pve.show_templates {
                theme::Btn::SmallPrimary
            } else {
                theme::Btn::Small
            };
            if theme::btn(ui, kind, None, t!("Vorlagen"))
                .on_hover_text(t!("Vorlagen (Templates) gedimmt anzeigen"))
                .clicked()
            {
                app.pve.show_templates = !app.pve.show_templates;
            }
        });
    });
    ui.add_space(14.0);

    let servers = app.lib.pve_servers.clone();
    let query = app.pve.search.trim().to_string();
    egui::ScrollArea::vertical()
        .id_salt("pve-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, s) in servers.iter().enumerate() {
                if i > 0 {
                    ui.add_space(26.0);
                }
                server_section(app, ui, s, &query, actions);
            }
            ui.add_space(40.0);
        });
}

fn empty_state(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.add_space(40.0);
    theme::card(ui, theme::bg_deep(), |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(28, 28))
            .show(ui, |ui| {
                ui.set_max_width(560.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                ui.painter()
                    .rect_filled(r, CornerRadius::same(11), theme::accent_soft());
                icons::paint(ui.painter(), r.shrink(10.0), Icon::Server, theme::accent());
                ui.add_space(10.0);
                ui.label(theme::heading(t!("Noch kein Proxmox-Server"), 22.0));
                ui.label(
                    RichText::new(t!(
                        "Verbinde Aurora mit der Proxmox-API und sieh alle VMs und Container deines Clusters auf einen Blick – mit Start/Stopp, Konsole und IP-Adressen."
                    ))
                    .color(theme::text_dim()),
                );
                ui.add_space(12.0);
                if theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("Server hinzufügen"))
                    .clicked()
                {
                    actions.push(Action::Pve(PveAction::NewServer));
                }
            });
    });
}

fn server_section(
    app: &mut App,
    ui: &mut egui::Ui,
    s: &PveServer,
    query: &str,
    actions: &mut Vec<Action>,
) {
    let live = app.pve.live.get(&s.id);
    let snap = live.and_then(|l| l.snap.as_ref());
    let error = live.and_then(|l| l.error.clone());
    let loading = live.is_some_and(|l| l.loading);

    // --- heading: dot, name, "x von y laufen", address, links
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let p = ui.painter();
    let cy = rect.center().y;
    let dot = Pos2::new(rect.left() + 10.0, cy);
    match (&error, snap) {
        (Some(_), _) => theme::led(p, dot, theme::red(), true),
        (None, Some(_)) => theme::led(p, dot, theme::green(), true),
        (None, None) => theme::led(p, dot, theme::off_color(), false),
    }
    let g = p.layout_no_wrap(
        s.display_name().to_string(),
        theme::semibold(16.0),
        theme::text(),
    );
    let mut x = rect.left() + 26.0;
    let w = g.size().x;
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, theme::text());
    x += w + 12.0;
    let count = match snap {
        Some(snap) => {
            let guests: Vec<&Guest> = snap.guests.iter().filter(|g| !g.template).collect();
            t!(
                "{} von {} laufen",
                guests.iter().filter(|g| g.running()).count(),
                guests.len()
            )
        }
        None if loading => t!("lädt…").to_string(),
        None => String::new(),
    };
    let g = p.layout_no_wrap(count, FontId::proportional(13.0), theme::text_dim());
    let w = g.size().x;
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, theme::text_dim());
    x += w + 12.0;
    let addr = if s.port == 8006 {
        s.host.trim().to_string()
    } else {
        format!("{}:{}", s.host.trim(), s.port)
    };
    p.text(
        Pos2::new(x, cy),
        egui::Align2::LEFT_CENTER,
        addr,
        FontId::monospace(12.0),
        theme::text_dim(),
    );
    let mut links = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(rect.right() - 260.0, rect.top()),
                rect.right_bottom() - Vec2::new(4.0, 0.0),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    if theme::btn(&mut links, theme::Btn::Link, None, t!("Bearbeiten")).clicked() {
        actions.push(Action::Pve(PveAction::EditServer(s.id)));
    }
    resp.context_menu(|ui| {
        if ui.button(t!("Bearbeiten")).clicked() {
            actions.push(Action::Pve(PveAction::EditServer(s.id)));
            ui.close();
        }
        if ui.button(t!("Jetzt aktualisieren")).clicked() {
            actions.push(Action::Pve(PveAction::Refresh));
            ui.close();
        }
        ui.separator();
        if ui
            .button(RichText::new(t!("Löschen")).color(theme::red()))
            .clicked()
        {
            actions.push(Action::Confirm(
                t!(
                    "Proxmox-Server „{}“ entfernen? Die VMs bleiben unberührt.",
                    s.display_name()
                ),
                Box::new(Action::Pve(PveAction::DeleteServer(s.id))),
            ));
            ui.close();
        }
    });

    if let Some(e) = &error {
        ui.add_space(6.0);
        theme::card(ui, theme::bg_deep(), |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(18, 14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.add(
                            egui::Label::new(RichText::new(e.to_string()).color(theme::red()))
                                .wrap(),
                        );
                    });
                    match e {
                        Error::Untrusted(fp) => {
                            ui.add_space(8.0);
                            if theme::btn(ui, theme::Btn::Small, None, t!("Zertifikat prüfen…"))
                                .clicked()
                            {
                                app.pve.trust = Some(Trust {
                                    target: TrustTarget::Server(s.id),
                                    host: format!("{}:{}", s.host.trim(), s.port),
                                    fingerprint: fp.clone(),
                                });
                            }
                        }
                        Error::PinChanged { pinned, got } => {
                            ui.add_space(6.0);
                            ui.label(
                                theme::mono(t!("gespeichert: {}", pinned), 11.5)
                                    .color(theme::text_dim()),
                            );
                            ui.label(theme::mono(t!("jetzt: {}", got), 11.5).color(theme::red()));
                        }
                        _ => {}
                    }
                });
        });
    }

    let Some(snap) = app.pve.live.get(&s.id).and_then(|l| l.snap.clone()) else {
        return;
    };
    // Nodes in API order, plus any node only the guests mention.
    let mut nodes: Vec<pve::Node> = snap.nodes.clone();
    for g in &snap.guests {
        if !nodes.iter().any(|n| n.name == g.node) {
            nodes.push(pve::Node {
                name: g.node.clone(),
                online: false,
                cpu: 0.0,
                maxcpu: 0,
                mem: 0,
                maxmem: 0,
                uptime: 0,
            });
        }
    }
    let filtering = !query.is_empty() || app.pve.filter != 0;
    for n in &nodes {
        let guests: Vec<&Guest> = snap
            .guests
            .iter()
            .filter(|g| g.node == n.name)
            .filter(|g| passes(app.pve.filter, app.pve.show_templates, g))
            .filter(|g| matches_query(query, g, first_ip(app, s.id, g.vmid).as_deref()))
            .collect();
        if guests.is_empty() && filtering {
            continue;
        }
        ui.add_space(10.0);
        node_heading(ui, n);
        if guests.is_empty() {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.label(
                    RichText::new(t!("Keine Gäste"))
                        .size(12.5)
                        .color(theme::text_dim()),
                );
            });
            continue;
        }
        ui.add_space(6.0);
        let live = app.pve.live.get(&s.id);
        theme::card(ui, theme::bg_deep(), |ui| {
            for (i, g) in guests.iter().enumerate() {
                let ips = live.and_then(|l| l.ips.get(&g.vmid)).map(|(i, _)| i);
                let busy = live.is_some_and(|l| l.busy.contains(&g.vmid));
                guest_row(ui, s, g, ips, busy, i > 0, actions);
            }
        });
    }
}

fn node_heading(ui: &mut egui::Ui, n: &pve::Node) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
    let p = ui.painter();
    let cy = rect.center().y;
    let icon = Rect::from_center_size(Pos2::new(rect.left() + 10.0, cy), Vec2::splat(14.0));
    icons::paint(
        p,
        icon,
        Icon::Server,
        if n.online {
            theme::text_dim()
        } else {
            theme::red()
        },
    );
    let g = p.layout_no_wrap(
        n.name.to_uppercase(),
        theme::semibold(12.0),
        theme::text_dim(),
    );
    let mut x = rect.left() + 26.0;
    let w = g.size().x;
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, theme::text_dim());
    x += w + 10.0;
    let (status, color) = if n.online {
        (t!("online"), theme::green())
    } else {
        (t!("offline"), theme::red())
    };
    let g = p.layout_no_wrap(status.to_string(), theme::medium(12.0), color);
    let w = g.size().x;
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, color);
    x += w + 10.0;
    if n.online && n.maxmem > 0 {
        let text = format!(
            "CPU {:.0} % · RAM {} / {} · {}",
            n.cpu * 100.0,
            gib(n.mem),
            gib(n.maxmem),
            uptime(n.uptime)
        );
        p.text(
            Pos2::new(x, cy),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(12.0),
            theme::text_dim(),
        );
    }
}

/// Paints a rounded tag at `x` (left edge); returns its width.
fn paint_tag(p: &egui::Painter, x: f32, cy: f32, text: &str, color: Option<Color32>) -> f32 {
    let fg = color.unwrap_or_else(theme::text_dim);
    let bg = color.map(theme::soft).unwrap_or_else(theme::fill);
    let g = p.layout_no_wrap(text.to_string(), theme::medium(11.5), fg);
    let w = g.size().x + 14.0;
    let r = Rect::from_min_size(Pos2::new(x, cy - 10.0), Vec2::new(w, 20.0));
    p.rect_filled(r, CornerRadius::same(10), bg);
    p.galley(Pos2::new(r.left() + 7.0, cy - g.size().y / 2.0), g, fg);
    w
}

fn guest_row(
    ui: &mut egui::Ui,
    s: &PveServer,
    g: &Guest,
    ips: Option<&Ips>,
    busy: bool,
    divider: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if divider {
        p.line_segment(
            [
                Pos2::new(rect.left() + 44.0, rect.top()),
                Pos2::new(rect.right(), rect.top()),
            ],
            Stroke::new(1.0, theme::line()),
        );
    }
    let dim = g.template;
    let ink = if dim {
        theme::text_dim()
    } else {
        theme::text()
    };
    let cy = rect.center().y;
    let (y1, y2) = (rect.top() + 24.0, rect.top() + 44.0);
    let led = Pos2::new(rect.left() + 22.0, cy);
    match g.status.as_str() {
        _ if busy => theme::led(&p, led, theme::amber(), true),
        "running" => theme::led(&p, led, theme::green(), true),
        "paused" | "suspended" => theme::led(&p, led, theme::amber(), true),
        "stopped" => theme::led(&p, led, theme::off_color(), false),
        _ => theme::led(&p, led, theme::amber(), false),
    }
    ui.interact(
        Rect::from_center_size(led, Vec2::splat(18.0)),
        ui.id().with(("pve-led", s.id, g.vmid)),
        Sense::hover(),
    )
    .on_hover_text(if busy {
        t!("Aktion läuft…").to_string()
    } else {
        g.status.clone()
    });

    // --- line 1: name, vmid, type, tags
    let col = rect.left() + 42.0;
    let name = if g.name.is_empty() {
        format!("{} {}", kind_tag(g.kind), g.vmid)
    } else {
        g.name.clone()
    };
    let ng = p.layout_no_wrap(name, theme::semibold(15.0), ink);
    let mut x = col + ng.size().x + 8.0;
    p.galley(Pos2::new(col, y1 - ng.size().y / 2.0), ng, ink);
    let vg = p.layout_no_wrap(
        g.vmid.to_string(),
        FontId::monospace(12.0),
        theme::text_dim(),
    );
    let vw = vg.size().x;
    p.galley(Pos2::new(x, y1 - vg.size().y / 2.0), vg, theme::text_dim());
    x += vw + 10.0;
    x += paint_tag(&p, x, y1, kind_tag(g.kind), Some(kind_color(g.kind))) + 6.0;
    if g.template {
        x += paint_tag(&p, x, y1, t!("Vorlage"), None) + 6.0;
    }
    for t in g.tags.iter().take(3) {
        x += paint_tag(&p, x, y1, t, None) + 6.0;
    }
    let _ = x;

    // --- line 2: address, uptime
    let mut parts: Vec<(String, bool)> = Vec::new();
    // Addresses only mean something while the guest runs.
    let ips = ips.filter(|_| g.running());
    match ips {
        Some(Ips::Found(v)) if !v.is_empty() => {
            let more = if v.len() > 1 {
                format!("  +{}", v.len() - 1)
            } else {
                String::new()
            };
            parts.push((format!("{}{more}", v[0]), true));
        }
        Some(Ips::NoAgent) => parts.push((t!("kein Gast-Agent").to_string(), false)),
        _ => {}
    }
    if g.running() && g.uptime > 0 {
        parts.push((uptime(g.uptime), false));
    }
    if !g.running() && !g.template {
        parts.push((g.status.clone(), false));
    }
    let mut x2 = col;
    for (i, (text, mono)) in parts.iter().enumerate() {
        let sep = if i == 0 { "" } else { "  ·  " };
        let font = if *mono {
            FontId::monospace(12.0)
        } else {
            FontId::proportional(12.5)
        };
        let gl = p.layout_no_wrap(format!("{sep}{text}"), font, theme::text_dim());
        let w = gl.size().x;
        p.galley(Pos2::new(x2, y2 - gl.size().y / 2.0), gl, theme::text_dim());
        x2 += w;
    }
    if let Some(Ips::Found(v)) = ips
        && v.len() > 1
    {
        ui.interact(
            Rect::from_min_max(
                Pos2::new(col, y2 - 9.0),
                Pos2::new(x2.max(col + 1.0), y2 + 9.0),
            ),
            ui.id().with(("pve-ips", s.id, g.vmid)),
            Sense::hover(),
        )
        .on_hover_text(v.join("\n"));
    }

    let r = GuestRef::of(s.id, g);
    let show_actions = (hovered || resp.context_menu_opened()) && !g.template;
    if !show_actions {
        // --- right: CPU and RAM
        if g.running() {
            let right = rect.right() - 18.0;
            let bar_w = 110.0;
            let bar =
                Rect::from_min_size(Pos2::new(right - bar_w, y2 - 2.5), Vec2::new(bar_w, 5.0));
            let frac = if g.maxmem > 0 {
                (g.mem as f32 / g.maxmem as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            p.rect_filled(bar, CornerRadius::same(3), theme::fill_strong());
            let color = if frac >= 0.9 {
                theme::red()
            } else if frac >= 0.75 {
                theme::amber()
            } else {
                kind_color(g.kind)
            };
            p.rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(bar_w * frac, 5.0)),
                CornerRadius::same(3),
                color,
            );
            p.text(
                Pos2::new(right, y1),
                egui::Align2::RIGHT_CENTER,
                format!("{} / {}", gib(g.mem), gib(g.maxmem)),
                FontId::proportional(12.5),
                theme::text_dim(),
            );
            let cpu = g.cpu * 100.0;
            p.text(
                Pos2::new(right - bar_w - 18.0, cy),
                egui::Align2::RIGHT_CENTER,
                format!("{cpu:.0} %"),
                theme::medium(13.0),
                if cpu >= 90.0 {
                    theme::red()
                } else {
                    theme::text()
                },
            );
            p.text(
                Pos2::new(right - bar_w - 70.0, cy),
                egui::Align2::RIGHT_CENTER,
                format!("{} vCPU", g.maxcpu),
                FontId::proportional(12.0),
                theme::text_dim(),
            );
        }
    } else {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - 620.0, rect.top()),
                    rect.right_bottom() - Vec2::new(10.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 6.0;
        child.add_enabled_ui(!busy, |ui| {
            if theme::btn(
                ui,
                theme::Btn::SmallPrimary,
                Some(Icon::Terminal),
                t!("Konsole"),
            )
            .on_hover_text(match g.kind {
                Kind::Lxc => t!("pct enter auf dem Node (über den SSH-Host)"),
                Kind::Qemu => t!("SSH zur IP der VM, sonst qm terminal auf dem Node"),
            })
            .clicked()
            {
                actions.push(Action::Pve(PveAction::Console(r.clone())));
            }
            if theme::btn(ui, theme::Btn::Small, None, t!("Als Host speichern")).clicked() {
                actions.push(Action::Pve(PveAction::SaveAsHost(r.clone())));
            }
            for (power, label) in power_buttons(g) {
                if theme::btn(ui, theme::Btn::Small, None, label).clicked() {
                    actions.push(power_action(&r, power));
                }
            }
        });
    }
    if resp.double_clicked() && !g.template {
        actions.push(Action::Pve(PveAction::Console(r.clone())));
    }
    resp.context_menu(|ui| {
        if g.template {
            ui.label(RichText::new(t!("Vorlage")).color(theme::text_dim()));
            return;
        }
        if ui.button(t!("Konsole")).clicked() {
            actions.push(Action::Pve(PveAction::Console(r.clone())));
            ui.close();
        }
        if ui.button(t!("Als Host speichern")).clicked() {
            actions.push(Action::Pve(PveAction::SaveAsHost(r.clone())));
            ui.close();
        }
        ui.separator();
        for (power, label) in power_buttons(g).into_iter().rev() {
            if ui.button(label).clicked() {
                actions.push(power_action(&r, power));
                ui.close();
            }
        }
    });
}

/// Power buttons for the row, right to left.
fn power_buttons(g: &Guest) -> Vec<(Power, &'static str)> {
    match g.status.as_str() {
        "running" => vec![
            (Power::Stop, t!("Stopp")),
            (Power::Reboot, t!("Neustart")),
            (Power::Shutdown, t!("Herunterfahren")),
        ],
        "paused" | "suspended" if g.kind == Kind::Qemu => vec![
            (Power::Stop, t!("Stopp")),
            (Power::Resume, t!("Fortsetzen")),
        ],
        _ => vec![(Power::Start, t!("Start"))],
    }
}

/// Stop and shutdown ask first.
fn power_action(r: &GuestRef, power: Power) -> Action {
    let a = Action::Pve(PveAction::Power(r.clone(), power));
    match power {
        Power::Stop => Action::Confirm(
            t!(
                "„{}“ sofort ausschalten? Nicht gespeicherte Daten im Gast gehen verloren.",
                r.label()
            ),
            Box::new(a),
        ),
        Power::Shutdown => {
            Action::Confirm(t!("„{}“ sauber herunterfahren?", r.label()), Box::new(a))
        }
        _ => a,
    }
}

// ------------------------------------------------------------------ dialogs

pub fn windows(app: &mut App, ctx: &egui::Context) {
    editor(app, ctx);
    trust(app, ctx);
}

fn editor(app: &mut App, ctx: &egui::Context) {
    let ssh_hosts: Vec<(Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh)
        .map(|h| (h.id, h.display_name().to_string()))
        .collect();
    let Some(ed) = app.pve.editor.as_mut() else {
        return;
    };
    let title = if ed.is_new {
        t!("Proxmox-Server hinzufügen")
    } else {
        t!("Proxmox-Server bearbeiten")
    };
    let mut test = false;
    let can = !ed.server.host.trim().is_empty() && !ed.server.user.trim().is_empty();
    let testing = ed.testing;
    let ((save, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-editor"),
        560.0,
        title,
        |ui| {
            let s = &mut ed.server;
            theme::form(ui, 110.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    text(ui, &mut s.name, t!("z.B. pve-lab"));
                });
                f.field(ui, t!("Adresse"), |ui| {
                    let w = ui.available_width() - 86.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut s.host)
                            .hint_text(RichText::new("192.168.1.10").color(theme::text_dim()))
                            .desired_width(w),
                    );
                    ui.label(RichText::new(":").color(theme::text_dim()));
                    ui.add(egui::DragValue::new(&mut s.port).range(1..=65535));
                });
            });
            theme::form_caption(ui, t!("Anmeldung"));
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                let cur = if s.token { 0 } else { 1 };
                if let Some(i) =
                    theme::segmented(ui, &[t!("API-Token"), t!("Benutzer + Passwort")], cur)
                    && s.token != (i == 0)
                {
                    s.token = i == 0;
                    // A token secret is no password (and vice versa).
                    ed.secret.clear();
                    if !s.token
                        && let Some((user, _)) = s.user.split_once('!')
                    {
                        s.user = user.to_string();
                    }
                }
            });
            ui.add_space(8.0);
            theme::form(ui, 110.0, |ui, f| {
                if s.token {
                    f.field(ui, t!("Token-ID"), |ui| {
                        text(ui, &mut s.user, "root@pam!aurora");
                    });
                    f.field(ui, t!("Secret"), |ui| {
                        secret(ui, &mut ed.secret, "xxxxxxxx-xxxx-…");
                    });
                } else {
                    f.field(ui, t!("Benutzer"), |ui| {
                        text(ui, &mut s.user, "root@pam");
                    });
                    f.field(ui, t!("Passwort"), |ui| {
                        secret(ui, &mut ed.secret, "");
                    });
                }
            });
            hint(
                ui,
                if s.token {
                    t!(
                        "Token unter Datacenter → Permissions → API Tokens anlegen. Mit „Privilege Separation“ braucht das Token eigene Rechte (z.B. PVEAuditor, für Start/Stopp PVEVMUser)."
                    )
                } else {
                    t!(
                        "Realm mit angeben, z.B. root@pam oder admin@pve. Zwei-Faktor-Logins gehen nur mit API-Token."
                    )
                },
            );
            theme::form_caption(ui, t!("Konsole"));
            theme::form(ui, 110.0, |ui, f| {
                f.field(ui, t!("SSH-Host"), |ui| {
                    let sel = s
                        .ssh_host
                        .and_then(|id| ssh_hosts.iter().find(|(i, _)| *i == id))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| t!("— keiner —").to_string());
                    egui::ComboBox::from_id_salt("pve-ssh-host")
                        .selected_text(sel)
                        .width(ui.available_width() - 14.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.ssh_host, None, t!("— keiner —"));
                            for (id, n) in &ssh_hosts {
                                ui.selectable_value(&mut s.ssh_host, Some(*id), n);
                            }
                        });
                });
            });
            hint(
                ui,
                t!(
                    "SSH-Host für die Nodes: Container öffnen sich dort mit „pct enter“, VMs ohne bekannte IP mit „qm terminal“. Liegt der Gast auf einem anderen Node, springt Aurora per SSH dorthin."
                ),
            );
            theme::form_caption(ui, t!("Zertifikat"));
            theme::form(ui, 110.0, |ui, f| {
                f.full(ui, |ui| {
                    ui.horizontal(|ui| {
                        if s.tls_pin.is_empty() {
                            ui.label(
                                RichText::new(t!("noch nicht bestätigt – wird beim ersten Verbinden angezeigt"))
                                    .size(12.5)
                                    .color(theme::text_dim()),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    theme::mono(&s.tls_pin, 11.0).color(theme::text_dim()),
                                )
                                .wrap(),
                            );
                        }
                    });
                    if !s.tls_pin.is_empty() {
                        ui.add_space(6.0);
                        if theme::btn(ui, theme::Btn::Small, None, t!("Pin zurücksetzen"))
                            .on_hover_text(t!("Nur nach einem gewollten Zertifikatswechsel: beim nächsten Verbinden wird der neue Fingerprint zur Bestätigung angezeigt."))
                            .clicked()
                        {
                            s.tls_pin.clear();
                        }
                    }
                });
            });
            match &ed.result {
                Some(Ok(v)) => {
                    ui.add_space(10.0);
                    ui.label(RichText::new(t!("Verbunden: {}", v)).color(theme::green()));
                }
                Some(Err(e)) => {
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(RichText::new(e).color(theme::red())).wrap());
                }
                None if ed.testing => {
                    ui.add_space(10.0);
                    ui.label(RichText::new(t!("Teste…")).color(theme::text_dim()));
                }
                None => {}
            }
        },
        |ui, _| {
            let mut out = (false, false);
            ui.add_enabled_ui(can, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Speichern")).clicked() {
                    out.0 = true;
                }
            });
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                out.1 = true;
            }
            ui.add_enabled_ui(can && !testing, |ui| {
                if theme::btn(ui, theme::Btn::Normal, None, t!("Verbindung testen")).clicked() {
                    test = true;
                }
            });
            out
        },
    );
    if test {
        ed.testing = true;
        ed.result = None;
        let c = conn_with(&ed.server, ed.secret.clone(), None);
        pve::spawn_test(&app.rt, c, app.pve.sink.clone());
        return;
    }
    if save {
        let ed = app.pve.editor.take().expect("editor");
        let mut server = ed.server;
        server.host = server.host.trim().to_string();
        server.user = server.user.trim().to_string();
        let id = server.id;
        match app.lib.pve_servers.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                // New address or account: drop the old session and data.
                if s.host != server.host
                    || s.port != server.port
                    || s.user != server.user
                    || s.tls_pin != server.tls_pin
                {
                    app.pve.live.remove(&id);
                }
                *s = server;
            }
            None => app.lib.pve_servers.push(server),
        }
        if app.keychain.pve_secrets.get(&id) != Some(&ed.secret) {
            app.keychain.pve_secrets.insert(id, ed.secret);
            app.pve.live.remove(&id);
            app.save_keychain();
        }
        app.save_library();
        refresh(app, id);
    } else if cancel || (dismissed && app.pve.trust.is_none()) {
        app.pve.editor = None;
    }
}

fn text(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(RichText::new(hint).color(theme::text_dim()))
            .desired_width(f32::INFINITY),
    );
}

fn secret(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text(RichText::new(hint).color(theme::text_dim()))
            .desired_width(f32::INFINITY),
    );
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::text_dim())).wrap());
    });
}

fn trust(app: &mut App, ctx: &egui::Context) {
    let Some(t) = app.pve.trust.as_ref() else {
        return;
    };
    let mut answer = None;
    egui::Modal::new(egui::Id::new("pve-trust"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(theme::heading(t!("Zertifikat prüfen"), 20.0));
            ui.add(
                egui::Label::new(
                    RichText::new(t!("Proxmox nutzt ein selbst signiertes Zertifikat. Vergleiche den Fingerprint mit dem in der Proxmox-Oberfläche (Node → System → Zertifikate), bevor du ihm vertraust. Ändert er sich später, verweigert Aurora die Verbindung."))
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
            ui.add_space(8.0);
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::mono(&t.host, 13.0).color(theme::text()));
                ui.label(RichText::new("SHA-256").size(12.5).color(theme::text_dim()));
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(theme::mono(&t.fingerprint, 12.5).color(theme::accent()))
                        .wrap(),
                );
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if theme::btn(ui, theme::Btn::Primary, None, t!("Vertrauen")).clicked() {
                        answer = Some(true);
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    let Some(yes) = answer else {
        return;
    };
    let Some(t) = app.pve.trust.take() else {
        return;
    };
    if !yes {
        return;
    }
    match t.target {
        TrustTarget::Editor => {
            if let Some(ed) = app.pve.editor.as_mut() {
                ed.server.tls_pin = t.fingerprint;
                ed.testing = true;
                let c = conn_with(&ed.server, ed.secret.clone(), None);
                pve::spawn_test(&app.rt, c, app.pve.sink.clone());
            }
        }
        TrustTarget::Server(id) => {
            if let Some(s) = app.lib.pve_servers.iter_mut().find(|s| s.id == id) {
                s.tls_pin = t.fingerprint.clone();
                // Same server open in the editor: keep its copy in sync.
                if let Some(ed) = app.pve.editor.as_mut().filter(|e| e.server.id == id) {
                    ed.server.tls_pin = t.fingerprint;
                }
                app.save_library();
                if let Some(l) = app.pve.live.get_mut(&id) {
                    l.error = None;
                }
                refresh(app, id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest(kind: Kind, status: &str, template: bool) -> Guest {
        Guest {
            vmid: 100,
            name: "web".into(),
            node: "pve1".into(),
            kind,
            status: status.into(),
            cpu: 0.0,
            maxcpu: 1,
            mem: 0,
            maxmem: 0,
            disk: 0,
            maxdisk: 0,
            uptime: 0,
            tags: vec!["prod".into()],
            template,
        }
    }

    #[test]
    fn filters() {
        let vm = guest(Kind::Qemu, "running", false);
        let ct = guest(Kind::Lxc, "stopped", false);
        let tpl = guest(Kind::Qemu, "stopped", true);
        assert!(passes(1, false, &vm) && !passes(1, false, &ct));
        assert!(passes(2, false, &ct) && !passes(2, false, &vm));
        assert!(passes(3, false, &vm) && !passes(3, false, &ct));
        assert!(!passes(0, false, &tpl) && passes(0, true, &tpl));
        assert!(matches_query("prod", &vm, None));
        assert!(matches_query("10.0.0", &vm, Some("10.0.0.5")));
        assert!(!matches_query("zzz", &vm, None));
        assert_eq!(gib(1024 * 1024 * 512), "512 MiB");
        assert_eq!(gib(4 * 1024 * 1024 * 1024), "4.0 GiB");
    }
}
