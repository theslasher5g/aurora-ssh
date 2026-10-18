//! Cluster-level parts of the Proxmox page: the overview (quorum, HA, storages, ZFS,
//! Ceph, versions), the task log, alerts from a background watcher, node shells over
//! termproxy, node package updates and Proxmox Backup Servers.
//!
//! The page itself (`super`) only calls in through a handful of hooks: the page
//! switch and the bell in the header, [`show_page`], [`node_links`] on node headings,
//! [`editor_switch`] in the server editor, [`pump`] and [`windows`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use super::{PveAction, conn, current, server_of};
use crate::app::{Action, App};
use crate::backend::Sink;
use crate::backend::proxmox::cluster::{
    self as cl, AlertKind, AptPackage, ClusterEvent, ClusterPayload, Overview, TaskInfo,
    WatchState, term::ShellParams,
};
use crate::backend::proxmox::{self as pve, Error};
use crate::model::PveServer;
use crate::theme;

#[path = "proxmox_cluster_pbs.rs"]
mod pbs;
#[path = "proxmox_cluster_views.rs"]
mod views;

pub use pbs::PbsAction;

/// Background watcher period per server (alerts).
const WATCH_EVERY: Duration = Duration::from_secs(45);
/// Overview refresh while it is visible.
const OVERVIEW_EVERY: Duration = Duration::from_secs(30);
/// … and while the guest list shows (for the update counts on node headings).
const OVERVIEW_BACKGROUND: Duration = Duration::from_secs(300);
/// Task list refresh while it is visible (faster while something runs).
const TASKS_EVERY: Duration = Duration::from_secs(15);
const TASKS_EVERY_BUSY: Duration = Duration::from_secs(4);
/// Live tail of a running task's log.
const LOG_EVERY: Duration = Duration::from_millis(1500);
/// Stops/shutdowns from Aurora within this window raise no alert.
const OWN_ACTION_WINDOW: Duration = Duration::from_secs(300);
const MAX_ALERTS: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Guests,
    Overview,
    Tasks,
    Backup,
}

/// Live cluster data of one Proxmox VE server.
#[derive(Default)]
pub struct ServerData {
    pub tasks: Option<Result<Vec<TaskInfo>, String>>,
    pub tasks_loading: bool,
    pub tasks_at: Option<Instant>,
    pub overview: Option<Result<Overview, String>>,
    pub overview_loading: bool,
    pub overview_at: Option<Instant>,
    watch: WatchState,
    watch_loading: bool,
    watch_at: Option<Instant>,
}

/// The task log sheet.
pub struct LogView {
    pub server: Uuid,
    pub task: TaskInfo,
    pub lines: Vec<String>,
    pub running: bool,
    pub exit: Option<String>,
    pub loading: bool,
    pub next_at: Option<Instant>,
    pub error: Option<String>,
    /// "Abbrechen" was pressed once: ask again inline.
    pub confirm_cancel: bool,
    pub cancelling: bool,
}

/// Package updates of one node.
pub struct AptView {
    pub server: Uuid,
    pub node: String,
    pub list: Option<Result<Vec<AptPackage>, String>>,
    pub loading: bool,
    pub refreshing: bool,
    pub confirm_upgrade: bool,
}

pub struct Alert {
    pub server: Uuid,
    pub server_name: String,
    pub kind: AlertKind,
    pub text: String,
    pub at: chrono::DateTime<chrono::Local>,
    pub read: bool,
}

pub struct ClusterState {
    sink: Sink<ClusterEvent>,
    rx: UnboundedReceiver<ClusterEvent>,
    pub page: Page,
    pub servers: HashMap<Uuid, ServerData>,
    /// 0 running, 1 failed, 2 all.
    pub task_filter: usize,
    pub log: Option<LogView>,
    pub apt: Option<AptView>,
    pub alerts: Vec<Alert>,
    pub alerts_open: bool,
    /// Power actions from Aurora: (server, vmid) → when.
    own: HashMap<(Uuid, u32), Instant>,
    pub pbs: pbs::PbsState,
}

impl ClusterState {
    pub fn new(ctx: &egui::Context) -> Self {
        let (sink, rx) = Sink::new(ctx.clone());
        Self {
            sink,
            rx,
            page: Page::Guests,
            servers: HashMap::new(),
            task_filter: 0,
            log: None,
            apt: None,
            alerts: Vec::new(),
            alerts_open: false,
            own: HashMap::new(),
            pbs: pbs::PbsState::default(),
        }
    }

    /// Vault locked: drop everything read with the old credentials.
    pub fn lock(&mut self) {
        self.servers.clear();
        self.log = None;
        self.apt = None;
        self.alerts.clear();
        self.alerts_open = false;
        self.own.clear();
        self.pbs.lock();
    }

    /// The server was edited or deleted (its jobs are stale through the epoch).
    pub fn reset_server(&mut self, server: Uuid) {
        self.servers.remove(&server);
        if self.log.as_ref().is_some_and(|l| l.server == server) {
            self.log = None;
        }
        if self.apt.as_ref().is_some_and(|a| a.server == server) {
            self.apt = None;
        }
        self.own.retain(|(s, _), _| *s != server);
        // Edited (new address / account) or deleted: its old alerts no longer apply.
        self.alerts.retain(|a| a.server != server);
    }

    pub fn unread(&self) -> usize {
        self.alerts.iter().filter(|a| !a.read).count()
    }
}

#[derive(Clone, Debug)]
pub enum ClusterAction {
    Page(Page),
    Refresh,
    OpenLog(Uuid, TaskInfo),
    CancelTask(Uuid, String),
    NodeShell(Uuid, String),
    OpenUpdates(Uuid, String),
    AptRefresh(Uuid, String),
    /// `apt-get dist-upgrade` in a terminal on the server's SSH host (confirmed).
    Upgrade(Uuid, String),
    OpenAlerts,
    Pbs(PbsAction),
}

fn act(a: ClusterAction) -> Action {
    Action::Pve(PveAction::Cluster(a))
}

/// A Proxmox power action from Aurora: a stop that follows raises no alert.
pub fn note_own_action(app: &mut App, server: Uuid, vmid: u32) {
    app.pve.cluster.own.insert((server, vmid), Instant::now());
}

// ------------------------------------------------------------------ jobs

fn usable(s: &PveServer) -> bool {
    !s.host.trim().is_empty() && !s.tls_pin.trim().is_empty()
}

/// A changed or unconfirmed certificate is handled by the page; don't hammer it.
fn blocked(app: &App, id: Uuid) -> bool {
    app.pve
        .live
        .get(&id)
        .and_then(|l| l.error.as_ref())
        .is_some_and(|e| matches!(e, Error::Untrusted(_) | Error::PinChanged { .. }))
}

fn refresh_tasks(app: &mut App, id: Uuid) {
    let Some(s) = server_of(app, id).filter(usable) else {
        return;
    };
    let c = conn(app, &s);
    let epoch = app.pve.epoch(id);
    let d = app.pve.cluster.servers.entry(id).or_default();
    if d.tasks_loading {
        return;
    }
    d.tasks_loading = true;
    d.tasks_at = Some(Instant::now());
    cl::spawn_tasks(&app.rt, id, epoch, c, app.pve.cluster.sink.clone());
}

fn refresh_overview(app: &mut App, id: Uuid) {
    let Some(s) = server_of(app, id).filter(usable) else {
        return;
    };
    let c = conn(app, &s);
    let epoch = app.pve.epoch(id);
    let d = app.pve.cluster.servers.entry(id).or_default();
    if d.overview_loading {
        return;
    }
    d.overview_loading = true;
    d.overview_at = Some(Instant::now());
    cl::spawn_overview(&app.rt, id, epoch, c, app.pve.cluster.sink.clone());
}

fn load_log(app: &mut App) {
    let Some(server) = app.pve.cluster.log.as_ref().map(|l| l.server) else {
        return;
    };
    let Some(s) = server_of(app, server) else {
        return;
    };
    let Some(l) = app.pve.cluster.log.as_mut() else {
        return;
    };
    if l.loading {
        return;
    }
    l.loading = true;
    l.next_at = None;
    let (server, upid, start) = (l.server, l.task.upid.clone(), l.lines.len());
    let c = conn(app, &s);
    cl::spawn_task_log(
        &app.rt,
        server,
        app.pve.epoch(server),
        c,
        upid,
        start,
        app.pve.cluster.sink.clone(),
    );
}

fn load_apt(app: &mut App, refresh: bool) {
    let Some(server) = app.pve.cluster.apt.as_ref().map(|a| a.server) else {
        return;
    };
    let Some(s) = server_of(app, server) else {
        return;
    };
    let Some(a) = app.pve.cluster.apt.as_mut() else {
        return;
    };
    if a.loading || a.refreshing {
        return;
    }
    if refresh {
        a.refreshing = true;
    } else {
        a.loading = true;
    }
    let (server, node) = (a.server, a.node.clone());
    let c = conn(app, &s);
    cl::spawn_apt(
        &app.rt,
        server,
        app.pve.epoch(server),
        c,
        node,
        refresh,
        app.pve.cluster.sink.clone(),
    );
}

/// Background watcher: every server with alerts on, also while the page is hidden.
fn watch(app: &mut App) {
    if app.vault.is_none() {
        return;
    }
    let due: Vec<PveServer> = app
        .lib
        .pve_servers
        .iter()
        .filter(|s| s.notify && usable(s))
        .filter(|s| {
            app.pve.cluster.servers.get(&s.id).is_none_or(|d| {
                !d.watch_loading && d.watch_at.is_none_or(|t| t.elapsed() >= WATCH_EVERY)
            })
        })
        .cloned()
        .collect();
    let any = app.lib.pve_servers.iter().any(|s| s.notify && usable(s));
    for s in due {
        if blocked(app, s.id) {
            continue;
        }
        let c = conn(app, &s);
        let epoch = app.pve.epoch(s.id);
        let d = app.pve.cluster.servers.entry(s.id).or_default();
        d.watch_loading = true;
        d.watch_at = Some(Instant::now());
        cl::spawn_watch(&app.rt, s.id, epoch, c, app.pve.cluster.sink.clone());
    }
    if any {
        // Keep the UI loop (and with it this watcher) ticking while idle.
        app.pve
            .cluster
            .sink
            .ctx()
            .request_repaint_after(Duration::from_secs(5));
    }
}

fn raise(app: &mut App, server: Uuid, hits: Vec<cl::AlertHit>) {
    if hits.is_empty() {
        return;
    }
    let name = server_of(app, server)
        .map(|s| s.display_name().to_string())
        .unwrap_or_default();
    for h in hits {
        let color = match h.kind {
            AlertKind::GuestDown | AlertKind::NodeOffline => theme::red(),
            AlertKind::TaskFailed | AlertKind::StorageFull => theme::amber(),
        };
        app.toast(format!("{name}: {}", h.text), color);
        app.pve.cluster.alerts.insert(
            0,
            Alert {
                server,
                server_name: name.clone(),
                kind: h.kind,
                text: h.text,
                at: chrono::Local::now(),
                read: false,
            },
        );
    }
    app.pve.cluster.alerts.truncate(MAX_ALERTS);
    let ctx = app.pve.cluster.sink.ctx();
    if !ctx.input(|i| i.focused) {
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
            egui::UserAttentionType::Informational,
        ));
    }
}

/// Applies backend results and starts due jobs; called every frame from `super::pump`.
pub fn pump(app: &mut App) {
    while let Ok(ev) = app.pve.cluster.rx.try_recv() {
        match ev {
            ClusterEvent::Pve {
                server,
                epoch,
                session,
                payload,
            } => {
                if !current(app, server, epoch) {
                    continue;
                }
                if session.is_some()
                    && let Some(l) = app.pve.live.get_mut(&server)
                {
                    l.session = session;
                }
                on_payload(app, server, payload);
            }
            ClusterEvent::Pbs {
                server,
                epoch,
                session,
                payload,
            } => pbs::on_payload(app, server, epoch, session, payload),
            ClusterEvent::PbsTested(r) => pbs::on_tested(app, r),
        }
    }
    watch(app);
    if let Some(l) = &app.pve.cluster.log
        && l.next_at.is_some_and(|t| t <= Instant::now())
    {
        load_log(app);
    }
}

fn on_payload(app: &mut App, server: Uuid, payload: ClusterPayload) {
    let st = &mut app.pve.cluster;
    match payload {
        ClusterPayload::Tasks(r) => {
            let d = st.servers.entry(server).or_default();
            d.tasks_loading = false;
            d.tasks = Some(r.map_err(|e| e.to_string()));
        }
        ClusterPayload::Overview(r) => {
            let d = st.servers.entry(server).or_default();
            d.overview_loading = false;
            d.overview = Some(r.map_err(|e| e.to_string()));
        }
        ClusterPayload::Watch(r) => {
            let own: Vec<u32> = st
                .own
                .iter()
                .filter(|((s, _), t)| *s == server && t.elapsed() < OWN_ACTION_WINDOW)
                .map(|((_, v), _)| *v)
                .collect();
            st.own.retain(|_, t| t.elapsed() < OWN_ACTION_WINDOW);
            let busy: Vec<u32> = app
                .pve
                .live
                .get(&server)
                .map(|l| l.busy.iter().copied().collect())
                .unwrap_or_default();
            let st = &mut app.pve.cluster;
            let d = st.servers.entry(server).or_default();
            d.watch_loading = false;
            let hits = match r {
                Ok(w) => cl::detect(
                    &mut d.watch,
                    &w,
                    &|v| own.contains(&v) || busy.contains(&v),
                    chrono::Utc::now().timestamp(),
                ),
                Err(e) => {
                    log::info!("Proxmox-Wächter: {e}");
                    Vec::new()
                }
            };
            raise(app, server, hits);
        }
        ClusterPayload::TaskLog {
            upid,
            start,
            result,
        } => {
            let Some(l) = st
                .log
                .as_mut()
                .filter(|l| l.server == server && l.task.upid == upid)
            else {
                return;
            };
            l.loading = false;
            match result {
                Ok(log) if start == l.lines.len() => {
                    l.lines.extend(log.lines);
                    l.running = log.running;
                    l.exit = log.exit;
                    l.error = None;
                    if l.running {
                        l.next_at = Some(Instant::now() + LOG_EVERY);
                        st.sink.ctx().request_repaint_after(LOG_EVERY);
                    } else {
                        // Finished: the list shows the new state.
                        l.cancelling = false;
                        l.confirm_cancel = false;
                        refresh_tasks(app, server);
                    }
                }
                Ok(_) => l.next_at = Some(Instant::now()),
                Err(e) => {
                    l.error = Some(e.to_string());
                    l.next_at = Some(Instant::now() + LOG_EVERY * 3);
                }
            }
        }
        ClusterPayload::Cancelled { upid, result } => match result {
            Ok(()) => {
                st.servers
                    .entry(server)
                    .or_default()
                    .watch
                    .ignore_task(&upid);
                app.toast(t!("Aufgabe wird abgebrochen…"), theme::amber());
                if let Some(l) = app.pve.cluster.log.as_mut().filter(|l| l.task.upid == upid) {
                    l.next_at = Some(Instant::now());
                }
                refresh_tasks(app, server);
            }
            Err(e) => {
                if let Some(l) = app.pve.cluster.log.as_mut().filter(|l| l.task.upid == upid) {
                    l.cancelling = false;
                }
                app.toast(t!("Abbrechen fehlgeschlagen: {}", e), theme::red());
            }
        },
        ClusterPayload::Apt {
            node,
            refreshed,
            result,
        } => {
            // The node's update count on the overview follows the new list.
            if let Ok(list) = &result
                && let Some(Ok(ov)) = st
                    .servers
                    .get_mut(&server)
                    .and_then(|d| d.overview.as_mut())
                && let Some(n) = ov.nodes.iter_mut().find(|n| n.name == node)
            {
                n.updates = Some(list.clone());
            }
            let Some(a) = st
                .apt
                .as_mut()
                .filter(|a| a.server == server && a.node == node)
            else {
                return;
            };
            a.loading = false;
            a.refreshing = false;
            if refreshed && result.is_ok() {
                app.toast(t!("Paketliste von {} aktualisiert", node), theme::green());
            }
            if let Some(a) = app.pve.cluster.apt.as_mut() {
                a.list = Some(result.map_err(|e| e.to_string()));
            }
        }
    }
}

// ------------------------------------------------------------------ actions

pub fn apply(app: &mut App, action: ClusterAction, ctx: &egui::Context) {
    match action {
        ClusterAction::Page(p) => {
            app.pve.cluster.page = p;
            if p == Page::Backup {
                pbs::refresh_all(app);
            }
        }
        ClusterAction::Refresh => {
            let ids: Vec<Uuid> = app.lib.pve_servers.iter().map(|s| s.id).collect();
            match app.pve.cluster.page {
                Page::Overview => ids.into_iter().for_each(|id| refresh_overview(app, id)),
                Page::Tasks => ids.into_iter().for_each(|id| refresh_tasks(app, id)),
                Page::Backup => pbs::refresh_all(app),
                Page::Guests => {}
            }
        }
        ClusterAction::OpenLog(server, task) => {
            app.pve.cluster.log = Some(LogView {
                server,
                running: task.running(),
                task,
                lines: Vec::new(),
                exit: None,
                loading: false,
                next_at: None,
                error: None,
                confirm_cancel: false,
                cancelling: false,
            });
            load_log(app);
        }
        ClusterAction::CancelTask(server, upid) => {
            let Some(s) = server_of(app, server) else {
                return;
            };
            if !cl::valid_upid(&upid) {
                return;
            }
            if let Some(l) = app.pve.cluster.log.as_mut() {
                l.cancelling = true;
                l.confirm_cancel = false;
            }
            let c = conn(app, &s);
            cl::spawn_cancel_task(
                &app.rt,
                server,
                app.pve.epoch(server),
                c,
                upid,
                app.pve.cluster.sink.clone(),
            );
        }
        ClusterAction::NodeShell(server, node) => {
            let Some(s) = server_of(app, server) else {
                return;
            };
            if !pve::valid_node(&node) {
                app.toast(t!("ungültiger Node-Name"), theme::red());
                return;
            }
            if s.tls_pin.trim().is_empty() {
                app.toast(t!("Zertifikat noch nicht bestätigt"), theme::amber());
                return;
            }
            let params = ShellParams {
                conn: conn(app, &s),
                node: node.clone(),
                guest: None,
            };
            app.open_pve_shell(
                node,
                t!("Proxmox-Shell · {}", s.display_name()),
                params,
                ctx,
            );
        }
        ClusterAction::OpenUpdates(server, node) => {
            if !pve::valid_node(&node) {
                return;
            }
            let known = app
                .pve
                .cluster
                .servers
                .get(&server)
                .and_then(|d| d.overview.as_ref())
                .and_then(|o| o.as_ref().ok())
                .and_then(|o| o.nodes.iter().find(|n| n.name == node))
                .and_then(|n| n.updates.clone());
            app.pve.cluster.apt = Some(AptView {
                server,
                node,
                list: known.map(Ok),
                loading: false,
                refreshing: false,
                confirm_upgrade: false,
            });
            load_apt(app, false);
        }
        ClusterAction::AptRefresh(server, node) => {
            if app
                .pve
                .cluster
                .apt
                .as_ref()
                .is_some_and(|a| a.server == server && a.node == node)
            {
                load_apt(app, true);
            }
        }
        ClusterAction::Upgrade(server, node) => {
            let Some(s) = server_of(app, server) else {
                return;
            };
            let Some(line) = cl::upgrade_command(&node) else {
                app.toast(t!("ungültiger Node-Name"), theme::red());
                return;
            };
            let Some(via) = s.ssh_host.and_then(|h| app.lib.host(h)).cloned() else {
                app.toast(
                    t!("Lege im Server-Editor einen SSH-Host für die Nodes fest"),
                    theme::amber(),
                );
                super::apply(app, PveAction::EditServer(s.id), ctx);
                return;
            };
            app.pve.cluster.apt = None;
            app.open_exec(via, t!("Upgrade · {}", node), line, ctx);
        }
        ClusterAction::OpenAlerts => {
            app.pve.cluster.alerts_open = true;
        }
        ClusterAction::Pbs(a) => pbs::apply(app, a, ctx),
    }
}

// ------------------------------------------------------------------ page hooks

/// Segmented switch next to the page title.
pub fn page_switch(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let pages = [Page::Guests, Page::Overview, Page::Tasks, Page::Backup];
    let cur = pages
        .iter()
        .position(|p| *p == app.pve.cluster.page)
        .unwrap_or(0);
    ui.add_space(14.0);
    if let Some(i) = theme::segmented(
        ui,
        &["Gäste", "Übersicht", "Aufgaben", "Backup-Server"].map(crate::i18n::tr),
        cur,
    ) {
        actions.push(act(ClusterAction::Page(pages[i])));
    }
}

/// Bell with the number of unread alerts (right side of the header).
pub fn bell(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let unread = app.pve.cluster.unread();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), theme::hover());
    }
    let color = if unread > 0 {
        theme::amber()
    } else {
        theme::text_dim()
    };
    paint_bell(p, rect.shrink(8.0), color);
    if unread > 0 {
        let text = if unread > 99 {
            "99+".to_string()
        } else {
            unread.to_string()
        };
        let g = p.layout_no_wrap(text, theme::bold(10.0), Color32::WHITE);
        let w = (g.size().x + 8.0).max(16.0);
        let badge = Rect::from_min_size(
            Pos2::new(rect.right() - w + 3.0, rect.top() - 2.0),
            Vec2::new(w, 16.0),
        );
        p.rect_filled(badge, CornerRadius::same(8), theme::red());
        p.galley(badge.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    let resp = resp.on_hover_text(if unread > 0 {
        t!("{} neue Benachrichtigungen", unread)
    } else {
        t!("Benachrichtigungen").to_string()
    });
    if resp.clicked() {
        actions.push(act(ClusterAction::OpenAlerts));
    }
}

/// Bell outline: dome, rim and clapper.
fn paint_bell(p: &egui::Painter, r: Rect, color: Color32) {
    let s = Stroke::new(1.6, color);
    let cx = r.center().x;
    let top = r.top() + r.height() * 0.08;
    let rim = r.top() + r.height() * 0.78;
    let w = r.width() * 0.5;
    let mut pts = Vec::new();
    // Left side up, arc over the top, right side down.
    pts.push(Pos2::new(cx - w, rim));
    pts.push(Pos2::new(cx - w * 0.78, rim - r.height() * 0.14));
    pts.push(Pos2::new(cx - w * 0.72, rim - r.height() * 0.4));
    for i in 0..=10 {
        let a = std::f32::consts::PI * (1.0 - i as f32 / 10.0);
        pts.push(Pos2::new(
            cx + a.cos() * w * 0.72,
            top + r.height() * 0.3 - a.sin() * r.height() * 0.28,
        ));
    }
    pts.push(Pos2::new(cx + w * 0.72, rim - r.height() * 0.4));
    pts.push(Pos2::new(cx + w * 0.78, rim - r.height() * 0.14));
    pts.push(Pos2::new(cx + w, rim));
    pts.push(Pos2::new(cx - w, rim));
    p.add(egui::Shape::line(pts, s));
    p.circle_stroke(Pos2::new(cx, rim + r.height() * 0.12), r.width() * 0.1, s);
}

/// Whether the guest list (and its search / filters) is showing.
pub fn guests_page(app: &App) -> bool {
    app.pve.cluster.page == Page::Guests
}

/// Draws the overview, task or backup page; `false` = the guest list is showing.
pub fn show_page(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
    match app.pve.cluster.page {
        Page::Guests => {
            // Update counts on the node headings: an overview now and then.
            let due: Vec<Uuid> = app
                .lib
                .pve_servers
                .iter()
                .filter(|s| {
                    app.pve.cluster.servers.get(&s.id).is_none_or(|d| {
                        !d.overview_loading
                            && d.overview_at
                                .is_none_or(|t| t.elapsed() >= OVERVIEW_BACKGROUND)
                    })
                })
                .map(|s| s.id)
                .collect();
            for id in due {
                if !blocked(app, id) && app.pve.live.get(&id).is_some_and(|l| l.snap.is_some()) {
                    refresh_overview(app, id);
                }
            }
            false
        }
        Page::Overview => {
            let due: Vec<Uuid> = app
                .lib
                .pve_servers
                .iter()
                .filter(|s| {
                    app.pve.cluster.servers.get(&s.id).is_none_or(|d| {
                        !d.overview_loading
                            && d.overview_at.is_none_or(|t| t.elapsed() >= OVERVIEW_EVERY)
                    })
                })
                .map(|s| s.id)
                .collect();
            for id in due {
                if !blocked(app, id) {
                    refresh_overview(app, id);
                }
            }
            ui.ctx().request_repaint_after(Duration::from_secs(2));
            views::overview(app, ui, actions);
            true
        }
        Page::Tasks => {
            let due: Vec<Uuid> = app
                .lib
                .pve_servers
                .iter()
                .filter(|s| {
                    app.pve.cluster.servers.get(&s.id).is_none_or(|d| {
                        let busy = d
                            .tasks
                            .as_ref()
                            .and_then(|t| t.as_ref().ok())
                            .is_some_and(|t| t.iter().any(|t| t.running()));
                        let every = if busy { TASKS_EVERY_BUSY } else { TASKS_EVERY };
                        !d.tasks_loading && d.tasks_at.is_none_or(|t| t.elapsed() >= every)
                    })
                })
                .map(|s| s.id)
                .collect();
            for id in due {
                if !blocked(app, id) {
                    refresh_tasks(app, id);
                }
            }
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            views::tasks(app, ui, actions);
            true
        }
        Page::Backup => {
            pbs::page(app, ui, actions);
            true
        }
    }
}

/// "Shell" and "Updates" on a node heading (`rect` is the heading row).
pub fn node_links(
    app: &App,
    ui: &mut egui::Ui,
    rect: Rect,
    server: Uuid,
    node: &pve::Node,
    actions: &mut Vec<Action>,
) {
    if !node.online {
        return;
    }
    let updates = app
        .pve
        .cluster
        .servers
        .get(&server)
        .and_then(|d| d.overview.as_ref())
        .and_then(|o| o.as_ref().ok())
        .and_then(|o| o.nodes.iter().find(|n| n.name == node.name))
        .and_then(|n| n.updates.as_ref())
        .map(|u| u.len());
    let mut links = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(rect.right() - 260.0, rect.top()),
                rect.right_bottom() - Vec2::new(4.0, 0.0),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    links.spacing_mut().item_spacing.x = 4.0;
    let label = match updates {
        Some(n) if n > 0 => t!("{} Updates", n),
        _ => t!("Updates").to_string(),
    };
    if theme::btn(&mut links, theme::Btn::Link, None, &label)
        .on_hover_text(t!("Ausstehende Paket-Updates des Nodes"))
        .clicked()
    {
        actions.push(act(ClusterAction::OpenUpdates(server, node.name.clone())));
    }
    if theme::btn(
        &mut links,
        theme::Btn::Link,
        Some(crate::ui::icons::Icon::Terminal),
        t!("Shell"),
    )
    .on_hover_text(t!("Shell auf dem Node über die Proxmox-API (ohne SSH)"))
    .clicked()
    {
        actions.push(act(ClusterAction::NodeShell(server, node.name.clone())));
    }
}

/// "Benachrichtigungen" switch in the Proxmox server editor.
pub fn editor_switch(ui: &mut egui::Ui, s: &mut PveServer) {
    theme::form_caption(ui, t!("Benachrichtigungen"));
    theme::form(ui, 110.0, |ui, f| {
        f.switch(ui, t!("Benachrichtigungen"), &mut s.notify);
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Solange Aurora läuft, prüft es den Server alle 45 s: unerwartet gestoppte Gäste, Nodes offline, fehlgeschlagene Aufgaben und Speicher über 90 %."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            )
            .wrap(),
        );
    });
}

pub fn windows(app: &mut App, ctx: &egui::Context) {
    views::log_sheet(app, ctx);
    views::apt_sheet(app, ctx);
    views::alerts_sheet(app, ctx);
    pbs::windows(app, ctx);
}

// ------------------------------------------------------------------ helpers

/// Usage bar (`frac` 0–1), colored by fill level.
fn usage_bar(ui: &mut egui::Ui, width: f32, frac: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    paint_bar(ui.painter(), r, frac);
}

fn paint_bar(p: &egui::Painter, r: Rect, frac: f32) {
    p.rect_filled(r, CornerRadius::same(4), theme::fill_strong());
    let frac = frac.clamp(0.0, 1.0);
    if frac > 0.0 {
        let mut f = r;
        f.set_width((r.width() * frac).max(4.0));
        p.rect_filled(f, CornerRadius::same(4), level_color(frac));
    }
}

fn level_color(frac: f32) -> Color32 {
    if frac > 0.95 {
        theme::red()
    } else if frac > cl::STORAGE_WARN {
        theme::amber()
    } else {
        theme::green()
    }
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 || v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

fn local_time(ts: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0).single() {
        Some(t) => {
            if t.date_naive() == chrono::Local::now().date_naive() {
                t.format("%H:%M:%S").to_string()
            } else {
                t.format("%d.%m. %H:%M").to_string()
            }
        }
        None => "–".into(),
    }
}

/// Readable name of a Proxmox task type.
pub fn task_label(kind: &str) -> String {
    let s = match kind {
        "vzdump" => t!("Backup"),
        "qmigrate" | "vzmigrate" => t!("Migration"),
        "qmclone" | "vzclone" => t!("Klonen"),
        "qmstart" | "vzstart" => t!("Start"),
        "qmstop" | "vzstop" => t!("Stopp"),
        "qmshutdown" | "vzshutdown" => t!("Herunterfahren"),
        "qmreboot" | "vzreboot" => t!("Neustart"),
        "qmsuspend" | "vzsuspend" => t!("Anhalten"),
        "qmresume" | "vzresume" => t!("Fortsetzen"),
        "qmsnapshot" | "vzsnapshot" => t!("Snapshot"),
        "qmrollback" | "vzrollback" => t!("Rollback"),
        "qmdelsnapshot" | "vzdelsnapshot" => t!("Snapshot löschen"),
        "qmrestore" | "vzrestore" => t!("Wiederherstellen"),
        "qmcreate" | "vzcreate" => t!("Erstellen"),
        "qmdestroy" | "vzdestroy" => t!("Löschen"),
        "qmconfig" | "vzconfig" => t!("Konfiguration"),
        "aptupdate" => t!("Paketliste aktualisieren"),
        "vncproxy" | "vncshell" | "termproxy" | "spiceproxy" | "spiceshell" => t!("Konsole"),
        "imgcopy" | "qmmove" | "move_volume" => t!("Disk verschieben"),
        "startall" => t!("Alle starten"),
        "stopall" => t!("Alle stoppen"),
        "srvreload" | "srvrestart" | "srvstart" | "srvstop" => t!("Dienst"),
        _ => return kind.to_string(),
    };
    s.to_string()
}

/// Background-tinted message box (warnings, errors).
fn notice(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::NONE
        .fill(theme::soft(color))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, color);
                ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
            });
        });
}

/// Label painted at the left of a row cell, cut to `w`.
fn cell(p: &egui::Painter, x: f32, cy: f32, w: f32, text: &str, font: FontId, color: Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(w);
    let g = p.layout_job(job);
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(250 * 1024 * 1024 * 1024), "250 GiB");
        assert_eq!(duration(42), "42 s");
        assert_eq!(duration(125), "2 min 5 s");
        assert_eq!(duration(7260), "2 h 1 min");
        assert_eq!(task_label("vzdump"), "Backup");
        assert_eq!(task_label("custom"), "custom");
    }
}
