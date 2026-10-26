//! Proxmox for managed-service use: customers (groups) for the servers with a selector
//! on every Proxmox page, a flat guest table with customer column, the guest query
//! language, and the glue for the PBS archive browser / prune / verify, multi-guest
//! migration and cloud-init sheets (`pve2_*_ui.rs`).

use std::collections::HashMap;

use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use super::{GuestRef, PveAction, guest, kind_tag};

use crate::app::{Action, App};
use crate::backend::Sink;
use crate::backend::proxmox::pve2::ci::CiEvent;
pub use crate::backend::proxmox::pve2::customer::Sel;
use crate::backend::proxmox::pve2::customer::{self, Customer, GroupRow};
use crate::backend::proxmox::pve2::pbs as pb;
use crate::backend::proxmox::pve2::query::Facets;
pub use crate::backend::proxmox::pve2::query::Query;
use crate::backend::proxmox::{Guest, Ips};
use crate::model::{PbsServer, PveServer};
use crate::theme;
use crate::ui::nav_list::paint_badge;
use crate::ui::nav_state;

#[path = "pve2_ci_ui.rs"]
pub mod ci_ui;
#[path = "pve2_migrate_ui.rs"]
pub mod mig_ui;
#[path = "pve2_pbs_ui.rs"]
pub mod pbs_ui;

/// Everything these extras keep (not persisted; the customer of a server lives in
/// `PveServer::group` / `PbsServer::group`).
pub struct State {
    pbs_sink: Sink<pb::Event>,
    pbs_rx: UnboundedReceiver<pb::Event>,
    ci_sink: Sink<CiEvent>,
    ci_rx: UnboundedReceiver<CiEvent>,
    /// Customer shown on the Proxmox pages.
    pub sel: Sel,
    /// Flat table instead of the server → node → guest list.
    pub table: bool,
    /// Table sort: column and direction.
    pub sort: (Col, bool),
    pub browser: Option<pbs_ui::Browser>,
    pub prune: Option<pbs_ui::PruneSheet>,
    pub confirm: Option<pbs_ui::Confirm>,
    pub jobs: pbs_ui::Jobs,
    pub gc: HashMap<(Uuid, String), pb::GcStatus>,
    /// Datastores whose GC status was requested already.
    pub gc_asked: std::collections::HashSet<(Uuid, String)>,
    pub migrate: Option<mig_ui::Sheet>,
    pub ci: Option<ci_ui::Sheet>,
    next_id: u64,
}

impl State {
    pub fn new(ctx: &egui::Context) -> Self {
        let (pbs_sink, pbs_rx) = Sink::new(ctx.clone());
        let (ci_sink, ci_rx) = Sink::new(ctx.clone());
        Self {
            pbs_sink,
            pbs_rx,
            ci_sink,
            ci_rx,
            sel: Sel::All,
            table: false,
            sort: (Col::Customer, false),
            browser: None,
            prune: None,
            confirm: None,
            jobs: pbs_ui::Jobs::default(),
            gc: HashMap::new(),
            gc_asked: Default::default(),
            migrate: None,
            ci: None,
            next_id: 1,
        }
    }

    /// Vault locked: forget everything read with the old credentials (and any password
    /// typed into the cloud-init sheet).
    pub fn lock(&mut self) {
        self.browser = None;
        self.prune = None;
        self.confirm = None;
        for j in &self.jobs.list {
            j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        for d in &self.jobs.downloads {
            d.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.jobs = pbs_ui::Jobs::default();
        self.gc.clear();
        self.gc_asked.clear();
        self.migrate = None;
        self.ci = None;
    }

    /// A server was edited or deleted.
    pub fn reset_server(&mut self, server: Uuid) {
        if self.browser.as_ref().is_some_and(|b| b.server == server) {
            self.browser = None;
        }
        if self.prune.as_ref().is_some_and(|p| p.server == server) {
            self.prune = None;
        }
        if self
            .confirm
            .as_ref()
            .is_some_and(|c| c.run.server() == server)
        {
            self.confirm = None;
        }
        self.jobs.list.retain(|j| j.server != server);
        self.gc.retain(|(s, _), _| *s != server);
        self.gc_asked.retain(|(s, _)| *s != server);
        if self.migrate.as_ref().is_some_and(|m| m.involves(server)) {
            self.migrate = None;
        }
        if self.ci.as_ref().is_some_and(|c| c.g.server == server) {
            self.ci = None;
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

/// Actions of these extras (all go through `PveAction::P2`).
#[derive(Clone, Debug)]
pub enum Act {
    /// Cloud-init sheet of a VM.
    CloudInit(GuestRef),
    /// Migration of the selected guests.
    MigrateSelected,
    /// Browse the archives of a snapshot: server, datastore, snapshot.
    Browse(Uuid, String, pb::SnapRef),
    /// Prune a group or (`None`) a whole datastore.
    Prune(Uuid, String, Option<(String, String)>),
    Verify(Uuid, String, pb::VerifyScope),
    Gc(Uuid, String),
    /// Ask the server for the garbage collection state of a datastore (once).
    GcStatus(Uuid, String),
    Jobs,
}

pub fn act(a: Act) -> Action {
    Action::Pve(PveAction::P2(Box::new(a)))
}

pub fn apply(app: &mut App, a: Act, ctx: &egui::Context) {
    match a {
        Act::CloudInit(g) => ci_ui::open(app, g),
        Act::MigrateSelected => mig_ui::open(app),
        Act::Browse(server, store, snap) => pbs_ui::open_browser(app, server, store, snap),
        Act::Prune(server, store, group) => pbs_ui::open_prune(app, server, store, group),
        Act::Verify(server, store, scope) => pbs_ui::ask_verify(app, server, store, scope),
        Act::Gc(server, store) => pbs_ui::ask_gc(app, server, store),
        Act::GcStatus(server, store) => pbs_ui::refresh_gc(app, server, store),
        Act::Jobs => app.pve.p2.jobs.open = true,
    }
    ctx.request_repaint();
}

/// Results of the background jobs.
pub fn pump(app: &mut App) {
    while let Ok(ev) = app.pve.p2.pbs_rx.try_recv() {
        pbs_ui::on_event(app, ev);
    }
    while let Ok(ev) = app.pve.p2.ci_rx.try_recv() {
        ci_ui::on_event(app, ev);
    }
}

pub fn windows(app: &mut App, ctx: &egui::Context) {
    pbs_ui::windows(app, ctx);
    mig_ui::sheet(app, ctx);
    ci_ui::sheet(app, ctx);
}

// ------------------------------------------------------------------ customers

fn group_rows(app: &App) -> Vec<GroupRow> {
    app.lib
        .groups
        .iter()
        .map(|g| GroupRow {
            id: g.id,
            parent: g.parent_id,
            name: g.name.clone(),
        })
        .collect()
}

/// How a customer is drawn: badge letters, colour and name.
#[derive(Clone, Debug)]
pub struct Cust {
    pub id: Option<Uuid>,
    pub name: String,
    pub letter: String,
    pub color: Color32,
}

pub fn cust_of(app: &App, group: Option<Uuid>) -> Cust {
    let rows = group_rows(app);
    match customer::customer_of(&rows, group).and_then(|id| app.lib.group(id)) {
        Some(g) => Cust {
            id: Some(g.id),
            name: g.name.clone(),
            letter: nav_state::badge_text(&app.lib, g),
            color: theme::rgb(g.color),
        },
        None => Cust {
            id: None,
            name: t!("Ohne Kunde").to_string(),
            letter: "–".into(),
            color: theme::text_dim(),
        },
    }
}

/// `Kunde · Server` for alerts and sheet titles; just the server without a customer.
pub fn label(app: &App, server_name: &str, group: Option<Uuid>) -> String {
    let c = cust_of(app, group);
    if c.id.is_some() {
        format!("{} · {}", c.name, server_name)
    } else {
        server_name.to_string()
    }
}

pub fn pve_label(app: &App, server: Uuid) -> String {
    app.lib
        .pve_servers
        .iter()
        .find(|s| s.id == server)
        .map(|s| label(app, s.display_name(), s.group))
        .unwrap_or_default()
}

pub fn pbs_label(app: &App, server: Uuid) -> String {
    app.lib
        .pbs_servers
        .iter()
        .find(|s| s.id == server)
        .map(|s| label(app, s.display_name(), s.group))
        .unwrap_or_default()
}

/// The group a Proxmox VE server is assigned to.
pub fn server_group(app: &App, server: Uuid) -> Option<Uuid> {
    app.lib
        .pve_servers
        .iter()
        .find(|s| s.id == server)
        .and_then(|s| s.group)
}

/// Do two Proxmox VE servers belong to the same customer?
pub fn same_customer(app: &App, a: Uuid, b: Uuid) -> bool {
    a == b || pve_customer(app, a) == pve_customer(app, b)
}

/// Servers (by their groups) grouped by customer, in selector order.
pub fn grouped(app: &App, server_groups: &[Option<Uuid>]) -> Vec<(Customer, Vec<usize>)> {
    customer::group_servers(&group_rows(app), server_groups)
}

/// `(running, total)` guests (no templates) of the given servers.
pub fn guest_counts(app: &App, servers: impl Iterator<Item = Uuid>) -> (usize, usize) {
    let mut running = 0;
    let mut total = 0;
    for id in servers {
        if let Some(snap) = app.pve.live.get(&id).and_then(|l| l.snap.as_ref()) {
            let g = snap.guests.iter().filter(|g| !g.template);
            total += g.clone().count();
            running += g.filter(|g| g.running()).count();
        }
    }
    (running, total)
}

/// Customer of a Proxmox VE server (`None` = no customer).
pub fn pve_customer(app: &App, server: Uuid) -> Option<Uuid> {
    let g = app
        .lib
        .pve_servers
        .iter()
        .find(|s| s.id == server)
        .and_then(|s| s.group);
    customer::customer_of(&group_rows(app), g)
}

fn shown(app: &App, group: Option<Uuid>) -> bool {
    let rows = group_rows(app);
    customer::in_scope(&rows, group, app.lib.nav.scope)
        && customer::matches(&rows, group, app.pve.p2.sel)
}

/// Proxmox VE servers of the selected customer (and inside the global scope).
pub fn visible_pve(app: &App) -> Vec<PveServer> {
    let focus = crate::ui::nav2_scope::current(app);
    app.lib
        .pve_servers
        .iter()
        .filter(|s| crate::ui::nav2_scope::pve_ok(&app.lib, &focus, s))
        .filter(|s| shown(app, s.group))
        .cloned()
        .collect()
}

pub fn visible_pbs(app: &App) -> Vec<PbsServer> {
    app.lib
        .pbs_servers
        .iter()
        .filter(|s| shown(app, s.group))
        .cloned()
        .collect()
}

pub fn is_visible_pve(app: &App, server: Uuid) -> bool {
    app.lib
        .pve_servers
        .iter()
        .find(|s| s.id == server)
        .is_some_and(|s| shown(app, s.group))
}

/// Entries of the selector: customers of all servers inside the scope.
fn selector_list(app: &App) -> Vec<Customer> {
    let rows = group_rows(app);
    let groups: Vec<Option<Uuid>> = app
        .lib
        .pve_servers
        .iter()
        .map(|s| s.group)
        .chain(app.lib.pbs_servers.iter().map(|s| s.group))
        .filter(|g| customer::in_scope(&rows, *g, app.lib.nav.scope))
        .collect();
    customer::customers(&rows, &groups)
}

fn chip(
    ui: &mut egui::Ui,
    text: &str,
    count: usize,
    badge: Option<(&str, Color32)>,
    on: bool,
) -> egui::Response {
    let font = theme::medium(13.0);
    let col = if on { theme::text() } else { theme::text_dim() };
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), col);
    let c = ui
        .painter()
        .layout_no_wrap(count.to_string(), theme::medium(11.5), theme::text_dim());
    let bw = if badge.is_some() { 26.0 } else { 0.0 };
    let w = 12.0 + bw + g.size().x + 8.0 + c.size().x + 12.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::click());
    let p = ui.painter();
    let bg = if on {
        theme::accent_soft()
    } else if resp.hovered() {
        theme::hover()
    } else {
        theme::fill()
    };
    p.rect(
        rect,
        CornerRadius::same(15),
        bg,
        Stroke::new(1.0, if on { theme::accent() } else { theme::line() }),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + 12.0;
    if let Some((letter, color)) = badge {
        let r = Rect::from_min_size(Pos2::new(x - 4.0, rect.center().y - 9.0), Vec2::splat(18.0));
        paint_badge(p, r, letter, color);
        x += bw;
    }
    let gh = g.size().y;
    p.galley(Pos2::new(x, rect.center().y - gh / 2.0), g.clone(), col);
    x += g.size().x + 8.0;
    p.galley(
        Pos2::new(x, rect.center().y - c.size().y / 2.0),
        c,
        theme::text_dim(),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// "Alle Kunden · Kunde A · Kunde B …": on every Proxmox page when servers belong to
/// customers. Changes `app.pve.p2.sel`.
pub fn selector(app: &mut App, ui: &mut egui::Ui) {
    let list = selector_list(app);
    let sel = customer::normalize(app.pve.p2.sel, &list);
    app.pve.p2.sel = sel;
    if list.len() < 2 && list.first().is_none_or(|c| c.id.is_none()) {
        // One customer-less bucket: nothing to separate.
        return;
    }
    let total: usize = list.iter().map(|c| c.servers).sum();
    let mut new = sel;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        if chip(ui, t!("Alle Kunden"), total, None, sel == Sel::All).clicked() {
            new = Sel::All;
        }
        for c in &list {
            let this = match c.id {
                Some(id) => Sel::Customer(id),
                None => Sel::Unassigned,
            };
            let info = cust_of(app, c.id);
            let name = if c.id.is_some() {
                c.name.as_str()
            } else {
                t!("Ohne Kunde")
            };
            if chip(
                ui,
                name,
                c.servers,
                Some((&info.letter, info.color)),
                sel == this,
            )
            .on_hover_text(t!("Nur die Server dieses Kunden anzeigen"))
            .clicked()
            {
                new = if sel == this { Sel::All } else { this };
            }
        }
    });
    ui.add_space(12.0);
    if new != sel {
        app.pve.p2.sel = new;
        drop_foreign_selection(app);
    }
}

/// The guest selection never spans customers: after switching, keep only the guests of
/// the shown servers.
fn drop_foreign_selection(app: &mut App) {
    let keep: Vec<(Uuid, u32)> = guest::selection(&app.pve)
        .into_iter()
        .filter(|(s, _)| is_visible_pve(app, *s))
        .collect();
    guest::set_selection(&mut app.pve, keep);
}

/// Customer heading above a group of servers (list view, "Alle Kunden").
pub fn customer_header(
    ui: &mut egui::Ui,
    c: &Customer,
    info: &Cust,
    running: usize,
    guests: usize,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
    let p = ui.painter();
    let badge = Rect::from_min_size(
        Pos2::new(rect.left() + 2.0, rect.center().y - 11.0),
        Vec2::splat(22.0),
    );
    paint_badge(p, badge, &info.letter, info.color);
    let name = if c.id.is_some() {
        c.name.clone()
    } else {
        t!("Ohne Kunde").to_string()
    };
    let g = p.layout_no_wrap(name, theme::bold(18.0), theme::text());
    let w = g.size().x;
    p.galley(
        Pos2::new(badge.right() + 10.0, rect.center().y - g.size().y / 2.0),
        g,
        theme::text(),
    );
    let sub = t!(
        "{} Server · {} von {} Gästen laufen",
        c.servers,
        running,
        guests
    );
    p.text(
        Pos2::new(badge.right() + 10.0 + w + 14.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        sub,
        theme::medium(12.5),
        theme::text_dim(),
    );
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, theme::line()),
    );
}

/// Small coloured customer badge + name for server headings on the other pages.
pub fn customer_tag(ui: &mut egui::Ui, info: &Cust) {
    if info.id.is_none() {
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    paint_badge(ui.painter(), rect, &info.letter, info.color);
    resp.on_hover_text(&info.name);
    ui.label(
        RichText::new(&info.name)
            .font(theme::medium(13.0))
            .color(theme::text_dim()),
    );
    ui.label(RichText::new("›").color(theme::text_dim()));
}

/// Customer picker of the server editors.
pub fn group_picker(ui: &mut egui::Ui, app_groups: &[(Uuid, String)], value: &mut Option<Uuid>) {
    let cur = value
        .and_then(|v| app_groups.iter().find(|(id, _)| *id == v))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| t!("Ohne Kunde").to_string());
    egui::ComboBox::from_id_salt("pve2-group")
        .selected_text(cur)
        .width(ui.available_width() - 14.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, t!("Ohne Kunde"));
            for (id, name) in app_groups {
                ui.selectable_value(value, Some(*id), name);
            }
        });
}

/// All groups as `(id, "Kunde / Standort")` for the picker, sorted by path.
pub fn group_choices(app: &App) -> Vec<(Uuid, String)> {
    let rows = group_rows(app);
    let mut v: Vec<(Uuid, String)> = rows
        .iter()
        .map(|g| (g.id, customer::path_of(&rows, g.id).join(" / ")))
        .collect();
    v.sort_by(|a, b| crate::nav_tree::natural_cmp(&a.1, &b.1));
    v
}

// ------------------------------------------------------------------ guest query

/// Does `g` of `server` match the query? `ip` is its first known address.
pub fn guest_matches(
    app: &App,
    q: &Query,
    server: &PveServer,
    g: &Guest,
    ip: Option<&str>,
) -> bool {
    if q.is_empty() {
        return true;
    }
    let cust = cust_of(app, server.group);
    let name = if g.name.is_empty() {
        format!("{} {}", kind_tag(g.kind), g.vmid)
    } else {
        g.name.clone()
    };
    q.matches(&Facets {
        name: &name,
        vmid: g.vmid,
        qemu: g.kind == crate::backend::proxmox::Kind::Qemu,
        status: &g.status,
        template: g.template,
        tags: &g.tags,
        node: &g.node,
        server: server.display_name(),
        customer: &cust.name,
        ip: ip.unwrap_or_default(),
    })
}

// ------------------------------------------------------------------ table

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Customer,
    Server,
    Node,
    Kind,
    Vmid,
    Name,
    Status,
    Tags,
}

impl Col {
    fn label(self) -> &'static str {
        match self {
            Col::Customer => t!("Kunde"),
            Col::Server => t!("Server"),
            Col::Node => t!("Node"),
            Col::Kind => t!("Typ"),
            Col::Vmid => t!("VMID"),
            Col::Name => t!("Name"),
            Col::Status => t!("Status"),
            Col::Tags => t!("Tags"),
        }
    }

    fn width(self) -> f32 {
        match self {
            Col::Customer => 150.0,
            Col::Server => 140.0,
            Col::Node => 80.0,
            Col::Kind => 52.0,
            Col::Vmid => 62.0,
            Col::Name => 190.0,
            Col::Status => 96.0,
            Col::Tags => 160.0,
        }
    }

    /// Lower = dropped last when the window gets narrow.
    fn priority(self) -> u8 {
        match self {
            Col::Name => 0,
            Col::Status => 1,
            Col::Customer => 2,
            Col::Server => 3,
            Col::Vmid => 4,
            Col::Node => 5,
            Col::Kind => 6,
            Col::Tags => 7,
        }
    }

    const ALL: [Col; 8] = [
        Col::Customer,
        Col::Server,
        Col::Node,
        Col::Kind,
        Col::Vmid,
        Col::Name,
        Col::Status,
        Col::Tags,
    ];
}

/// Columns that fit into `width`, in display order.
pub fn fit_columns(width: f32) -> Vec<Col> {
    let mut keep: Vec<Col> = Vec::new();
    let mut by_prio = Col::ALL;
    by_prio.sort_by_key(|c| c.priority());
    let mut used = 56.0;
    for c in by_prio {
        if used + c.width() <= width || keep.len() < 2 {
            used += c.width();
            keep.push(c);
        }
    }
    Col::ALL.into_iter().filter(|c| keep.contains(c)).collect()
}

/// One row of the table.
struct Row {
    server: PveServer,
    cust: Cust,
    g: Guest,
    ip: Option<String>,
}

fn sort_rows(rows: &mut [Row], sort: (Col, bool)) {
    use std::cmp::Ordering;
    let nat = |a: &str, b: &str| crate::nav_tree::natural_cmp(a, b);
    rows.sort_by(|a, b| {
        let primary = match sort.0 {
            Col::Customer => nat(&a.cust.name, &b.cust.name),
            Col::Server => nat(a.server.display_name(), b.server.display_name()),
            Col::Node => nat(&a.g.node, &b.g.node),
            Col::Kind => (a.g.kind as u8).cmp(&(b.g.kind as u8)),
            Col::Vmid => a.g.vmid.cmp(&b.g.vmid),
            Col::Name => nat(&a.g.name, &b.g.name),
            Col::Status => a.g.status.cmp(&b.g.status),
            Col::Tags => a.g.tags.join(",").cmp(&b.g.tags.join(",")),
        };
        let primary = if sort.1 { primary.reverse() } else { primary };
        primary
            .then_with(|| nat(&a.cust.name, &b.cust.name))
            .then_with(|| nat(a.server.display_name(), b.server.display_name()))
            .then_with(|| nat(&a.g.node, &b.g.node))
            .then(a.g.vmid.cmp(&b.g.vmid))
            .then(Ordering::Equal)
    });
}

/// Flat guest table across the shown servers.
pub fn table(
    app: &mut App,
    ui: &mut egui::Ui,
    servers: &[PveServer],
    q: &Query,
    actions: &mut Vec<Action>,
) {
    let mut rows: Vec<Row> = Vec::new();
    for s in servers {
        let Some(snap) = app.pve.live.get(&s.id).and_then(|l| l.snap.as_ref()) else {
            continue;
        };
        let cust = cust_of(app, s.group);
        for g in &snap.guests {
            if !super::passes(app.pve.filter, app.pve.show_templates, g) {
                continue;
            }
            let ip = app
                .pve
                .live
                .get(&s.id)
                .and_then(|l| l.ips.get(&g.vmid))
                .and_then(|(i, _)| match i {
                    Ips::Found(v) => v.first().cloned(),
                    Ips::NoAgent => None,
                });
            if !guest_matches(app, q, s, g, ip.as_deref()) {
                continue;
            }
            rows.push(Row {
                server: s.clone(),
                cust: cust.clone(),
                g: g.clone(),
                ip,
            });
        }
    }
    sort_rows(&mut rows, app.pve.p2.sort);
    // Selection rows in drawing order (Shift ranges).
    let order: Vec<(Uuid, u32)> = rows.iter().map(|r| (r.server.id, r.g.vmid)).collect();
    guest::set_row_order(&mut app.pve, order);
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(t!("Keine Gäste gefunden")).color(theme::text_dim()));
        ui.add_space(4.0);
        ui.label(
            RichText::new(t!(
                "Suche: kunde/server/name · type:vm · is:running · #tag · vmid:100 · node:pve1 · ip:10.1."
            ))
            .size(12.5)
            .color(theme::text_dim()),
        );
        return;
    }
    let cols = fit_columns(ui.available_width() - 14.0);
    let selected: Vec<(Uuid, u32)> = guest::selection(&app.pve);
    let selecting = !selected.is_empty();
    let sort = app.pve.p2.sort;
    let mut new_sort = None;
    let total_w: f32 = 56.0 + cols.iter().map(|c| c.width()).sum::<f32>();
    let extra = (ui.available_width() - 14.0 - total_w).max(0.0);
    // Header
    let (hr, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width() - 14.0, 28.0), Sense::hover());
    {
        let mut x = hr.left() + 44.0;
        for c in &cols {
            let w = c.width() + if *c == Col::Name { extra } else { 0.0 };
            let r = Rect::from_min_size(Pos2::new(x, hr.top()), Vec2::new(w, hr.height()));
            let resp = ui.interact(r, ui.id().with(("p2-col", c.label())), Sense::click());
            let arrow = if sort.0 == *c {
                if sort.1 { " ↓" } else { " ↑" }
            } else {
                ""
            };
            ui.painter().text(
                Pos2::new(x, hr.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{}{arrow}", c.label()),
                theme::semibold(12.0),
                if sort.0 == *c {
                    theme::text()
                } else {
                    theme::text_dim()
                },
            );
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                new_sort = Some((*c, if sort.0 == *c { !sort.1 } else { false }));
            }
            x += w;
        }
        ui.painter().line_segment(
            [hr.left_bottom(), hr.right_bottom()],
            Stroke::new(1.0, theme::line()),
        );
    }
    if let Some(s) = new_sort {
        app.pve.p2.sort = s;
    }
    let row_h = 32.0;
    let online: HashMap<Uuid, Vec<String>> = servers
        .iter()
        .map(|s| {
            (
                s.id,
                app.pve
                    .live
                    .get(&s.id)
                    .and_then(|l| l.snap.as_ref())
                    .map(|sn| {
                        sn.nodes
                            .iter()
                            .filter(|n| n.online)
                            .map(|n| n.name.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("pve2-table")
        .auto_shrink([false, false])
        .show_rows(ui, row_h, rows.len(), |ui, range| {
            for r in &rows[range] {
                let gr = GuestRef::of(r.server.id, &r.g);
                let on = selected.contains(&(r.server.id, r.g.vmid));
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width() - 14.0, row_h),
                    Sense::click(),
                );
                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                if on {
                    p.rect_filled(rect, CornerRadius::ZERO, theme::accent_soft());
                } else if resp.hovered() {
                    p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
                }
                let cy = rect.center().y;
                let ink = if r.g.template {
                    theme::text_dim()
                } else {
                    theme::text()
                };
                // Selection box
                let cb =
                    Rect::from_center_size(Pos2::new(rect.left() + 18.0, cy), Vec2::splat(15.0));
                p.rect(
                    cb,
                    CornerRadius::same(4),
                    if on {
                        theme::accent()
                    } else {
                        Color32::TRANSPARENT
                    },
                    Stroke::new(
                        1.2,
                        if on {
                            theme::accent()
                        } else {
                            theme::text_dim()
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                if on {
                    p.text(
                        cb.center(),
                        egui::Align2::CENTER_CENTER,
                        "✓",
                        theme::bold(11.0),
                        theme::on_accent(),
                    );
                }
                let mut x = rect.left() + 44.0;
                for c in &cols {
                    let w = c.width() + if *c == Col::Name { extra } else { 0.0 };
                    let cell =
                        Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w - 8.0, row_h));
                    match c {
                        Col::Customer => {
                            if r.cust.id.is_some() {
                                let b = Rect::from_center_size(
                                    Pos2::new(x + 9.0, cy),
                                    Vec2::splat(18.0),
                                );
                                paint_badge(&p, b, &r.cust.letter, r.cust.color);
                                cell_text(
                                    &p,
                                    x + 26.0,
                                    cy,
                                    w - 34.0,
                                    &r.cust.name,
                                    theme::medium(13.0),
                                    ink,
                                );
                            } else {
                                cell_text(
                                    &p,
                                    x,
                                    cy,
                                    w - 8.0,
                                    "–",
                                    theme::medium(13.0),
                                    theme::text_dim(),
                                );
                            }
                        }
                        Col::Server => cell_text(
                            &p,
                            x,
                            cy,
                            w - 8.0,
                            r.server.display_name(),
                            theme::medium(13.0),
                            ink,
                        ),
                        Col::Node => cell_text(
                            &p,
                            x,
                            cy,
                            w - 8.0,
                            &r.g.node,
                            theme::medium(13.0),
                            theme::text_dim(),
                        ),
                        Col::Kind => {
                            cell_text(
                                &p,
                                x,
                                cy,
                                w - 8.0,
                                kind_tag(r.g.kind),
                                theme::semibold(12.0),
                                super::kind_color(r.g.kind),
                            );
                        }
                        Col::Vmid => cell_text(
                            &p,
                            x,
                            cy,
                            w - 8.0,
                            &r.g.vmid.to_string(),
                            theme::mono_medium(12.5),
                            theme::text_dim(),
                        ),
                        Col::Name => {
                            let name = if r.g.name.is_empty() {
                                format!("{} {}", kind_tag(r.g.kind), r.g.vmid)
                            } else {
                                r.g.name.clone()
                            };
                            let rest =
                                r.ip.as_deref()
                                    .filter(|_| r.g.running())
                                    .unwrap_or_default();
                            cell_text(&p, x, cy, w - 8.0, &name, theme::semibold(14.0), ink);
                            if !rest.is_empty() && cell.width() > 260.0 {
                                p.text(
                                    Pos2::new(cell.right(), cy),
                                    egui::Align2::RIGHT_CENTER,
                                    rest,
                                    theme::mono_medium(11.5),
                                    theme::text_dim(),
                                );
                            }
                        }
                        Col::Status => {
                            let (txt, col) = match r.g.status.as_str() {
                                "running" => (t!("läuft"), theme::green()),
                                "stopped" => (t!("gestoppt"), theme::text_dim()),
                                "paused" | "suspended" => (t!("pausiert"), theme::amber()),
                                _ => (r.g.status.as_str(), theme::amber()),
                            };
                            let led = Pos2::new(x + 6.0, cy);
                            theme::led(&p, led, col, r.g.running());
                            cell_text(&p, x + 18.0, cy, w - 26.0, txt, theme::medium(13.0), col);
                        }
                        Col::Tags => cell_text(
                            &p,
                            x,
                            cy,
                            w - 8.0,
                            &r.g.tags.join(", "),
                            theme::medium(12.5),
                            theme::text_dim(),
                        ),
                    }
                    x += w;
                }
                p.line_segment(
                    [
                        Pos2::new(rect.left() + 44.0, rect.bottom()),
                        rect.right_bottom(),
                    ],
                    Stroke::new(0.5, theme::line()),
                );
                if resp.clicked() && !r.g.template {
                    actions.push(Action::Pve(PveAction::Ext(guest::GuestAction::Select(
                        gr.clone(),
                        if ui.input(|i| i.modifiers.shift) {
                            2
                        } else {
                            1
                        },
                    ))));
                }
                if resp.double_clicked() && !r.g.template {
                    actions.push(Action::Pve(PveAction::Console(gr.clone())));
                }
                let ext = guest::RowExt {
                    spark: None,
                    selected: on,
                    selecting,
                    nodes: online
                        .get(&r.server.id)
                        .map(|v| v.iter().filter(|n| **n != r.g.node).cloned().collect())
                        .unwrap_or_default(),
                };
                resp.context_menu(|ui| {
                    if !r.g.template {
                        if ui.button(t!("Konsole")).clicked() {
                            actions.push(Action::Pve(PveAction::Console(gr.clone())));
                            ui.close();
                        }
                        if ui.button(t!("Snapshots und Backups…")).clicked() {
                            actions.push(Action::Pve(PveAction::OpenGuest(gr.clone(), 0)));
                            ui.close();
                        }
                        for (power, label) in super::power_buttons(&r.g) {
                            if ui.button(label).clicked() {
                                actions.push(super::power_action(&gr, power));
                                ui.close();
                            }
                        }
                        if ui.button(t!("Als Host speichern")).clicked() {
                            actions.push(Action::Pve(PveAction::SaveAsHost(gr.clone())));
                            ui.close();
                        }
                        ui.separator();
                    }
                    guest::menu_entries(ui, &r.g, &gr, &ext, actions);
                });
            }
        });
}

fn cell_text(
    p: &egui::Painter,
    x: f32,
    cy: f32,
    w: f32,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    let g = theme::truncated(p, text, font, color, w.max(10.0));
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, color);
}
