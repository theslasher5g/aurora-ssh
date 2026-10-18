//! Snapshots, backups and statistics of one guest (a sheet), and the Proxmox tasks
//! they start. Clone, migration, hardware, graphs and batch actions live in the
//! `proxmox_guest_*` files next to this one.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::backend::proxmox::{BACKUP_MODES, Backup, Backups, ExtOp, SnapInfo, TaskUpdate};

#[path = "proxmox_guest_batch.rs"]
mod batch;
#[path = "proxmox_guest_clone.rs"]
mod clone;
#[path = "proxmox_guest_graphs.rs"]
mod graphs;
#[path = "proxmox_guest_hw.rs"]
mod hw;

pub use batch::{
    RowExt, menu_entries, more_button_with, more_open, node_heading_hook, row_background, row_ext,
    row_hook, selection_bar, template_actions,
};
pub use clone::save_and_connect;
pub use graphs::paint_spark;

/// Guest-level extras of the Proxmox page (not persisted).
#[derive(Default)]
pub struct GuestExt {
    sparks: graphs::Sparks,
    node_stats: Option<graphs::NodeStats>,
    clone: Option<clone::CloneSheet>,
    hw: Option<hw::HwSheet>,
    migrate: Option<hw::MigrateSheet>,
    sel: batch::Selection,
    batch_form: Option<batch::BatchForm>,
    batch: Option<batch::Batch>,
    /// Cloned guests: what "Verbinden" should use (user, key, static address).
    hints: HashMap<(Uuid, u32), clone::ConnectHint>,
}

impl GuestExt {
    /// User and key a clone made from Aurora was set up with.
    pub fn connect_hint(&self, server: Uuid, vmid: u32) -> Option<(String, Option<Uuid>)> {
        self.hints
            .get(&(server, vmid))
            .map(|h| (h.user.clone(), h.identity))
    }

    /// The server was edited or deleted: drop everything about it.
    pub fn forget(&mut self, server: Uuid) {
        self.sparks.forget(server);
        if self.node_stats.as_ref().is_some_and(|n| n.server == server) {
            self.node_stats = None;
        }
        if self.clone.as_ref().is_some_and(|c| c.src.server == server) {
            self.clone = None;
        }
        if self.hw.as_ref().is_some_and(|h| h.g.server == server) {
            self.hw = None;
        }
        if self.migrate.as_ref().is_some_and(|m| m.g.server == server) {
            self.migrate = None;
        }
        self.sel.set.retain(|(s, _)| *s != server);
        if self
            .batch_form
            .as_ref()
            .is_some_and(|b| b.guests.iter().any(|g| g.server == server))
        {
            self.batch_form = None;
        }
        if let Some(b) = self.batch.as_mut() {
            b.items.retain(|i| i.g.server != server);
        }
        self.hints.retain(|(s, _), _| *s != server);
    }
}

/// Actions of the guest extras (row menus, drag & drop, selection bar).
#[derive(Clone, Debug)]
pub enum GuestAction {
    /// Graphs of a node (click on its heading).
    NodeStats(Uuid, String),
    /// Clone sheet; `true` = "Neu aus Vorlage".
    Clone(GuestRef, bool),
    Hardware(GuestRef),
    /// Migration sheet for the guest and a target node.
    Migrate(GuestRef, String),
    /// Ctrl (1) / Shift (2) / plain (0) click on a row.
    Select(GuestRef, u8),
    SelectTag(Uuid, String),
    ClearSelection,
    /// Power action for every selected guest.
    BatchPower(Power),
    /// Snapshot (0) or backup (1) form for the selection.
    BatchForm(u8),
}

impl GuestAction {
    /// Title and button of the confirmation dialog.
    pub fn confirm_labels(&self) -> Option<(&'static str, &'static str)> {
        match self {
            GuestAction::BatchPower(Power::Shutdown) => {
                Some((t!("Herunterfahren?"), t!("Herunterfahren")))
            }
            GuestAction::BatchPower(Power::Reboot) => Some((t!("Neu starten?"), t!("Neustart"))),
            _ => None,
        }
    }
}

pub fn apply_ext(app: &mut App, a: GuestAction) {
    match a {
        GuestAction::NodeStats(server, node) => graphs::open_node(app, server, node),
        GuestAction::Clone(g, template) => clone::open(app, g, template),
        GuestAction::Hardware(g) => hw::open_hw(app, g),
        GuestAction::Migrate(g, target) => hw::open_migrate(app, g, target),
        GuestAction::Select(g, mode) => batch::select(app, &g, mode),
        GuestAction::SelectTag(server, tag) => batch::select_tag(app, server, &tag),
        GuestAction::ClearSelection => app.pve.ext.sel.set.clear(),
        GuestAction::BatchPower(p) => batch::run_power(app, p),
        GuestAction::BatchForm(kind) => batch::open_form(app, kind),
    }
}

/// The guest from the last refresh.
fn live_guest(app: &App, server: Uuid, vmid: u32) -> Option<Guest> {
    app.pve
        .live
        .get(&server)?
        .snap
        .as_ref()?
        .guests
        .iter()
        .find(|g| g.vmid == vmid)
        .cloned()
}

/// Online nodes of a server.
fn online_nodes(app: &App, server: Uuid) -> Vec<String> {
    app.pve
        .live
        .get(&server)
        .and_then(|l| l.snap.as_ref())
        .map(|s| {
            s.nodes
                .iter()
                .filter(|n| n.online)
                .map(|n| n.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Finished tasks stay visible in the sheet for this long.
const KEEP_DONE: Duration = Duration::from_secs(90);

/// A Proxmox task started from Aurora.
pub struct TaskView {
    pub id: u64,
    pub server: Uuid,
    pub vmid: u32,
    /// "Snapshot „pre-update“", "Backup nach local", …
    pub what: String,
    /// Guest label for toasts.
    pub guest: String,
    pub op: Op,
    pub started: Instant,
    pub upid: Option<String>,
    /// Last line of the task log.
    pub progress: String,
    pub done: Option<Result<String, String>>,
    pub finished: Option<Instant>,
    pub cancel: Arc<AtomicBool>,
    /// Part of a batch: no toasts of its own.
    pub quiet: bool,
}

/// Data loaded on demand.
struct Load<T> {
    data: Option<Result<T, String>>,
    loading: bool,
}

impl<T> Default for Load<T> {
    fn default() -> Self {
        Self {
            data: None,
            loading: false,
        }
    }
}

struct NewSnap {
    name: String,
    description: String,
    vmstate: bool,
}

struct NewBackup {
    storage: String,
    mode: usize,
}

struct Restore {
    backup: Backup,
    vmid: Option<u32>,
    error: Option<String>,
    typed: String,
}

pub struct GuestPanel {
    pub guest: GuestRef,
    /// 0 snapshots, 1 backups, 2 statistics.
    pub tab: usize,
    snaps: Load<Vec<SnapInfo>>,
    backups: Load<Backups>,
    new_snap: Option<NewSnap>,
    new_backup: Option<NewBackup>,
    restore: Option<Restore>,
    stats: graphs::RrdView,
}

pub fn open(app: &mut App, g: GuestRef, tab: usize) {
    let same = app
        .pve
        .panel
        .as_ref()
        .is_some_and(|p| p.guest.server == g.server && p.guest.vmid == g.vmid);
    if !same {
        app.pve.panel = Some(GuestPanel {
            guest: g,
            tab,
            snaps: Load::default(),
            backups: Load::default(),
            new_snap: None,
            new_backup: None,
            restore: None,
            stats: graphs::RrdView::default(),
        });
    } else if let Some(p) = app.pve.panel.as_mut() {
        p.tab = tab;
    }
    load(app, tab);
}

/// (Re)loads a tab of the open sheet.
fn load(app: &mut App, tab: usize) {
    let Some(g) = app.pve.panel.as_ref().map(|p| p.guest.clone()) else {
        return;
    };
    let Some(s) = server_of(app, g.server) else {
        return;
    };
    if tab == 2 {
        graphs::load_guest(app);
        return;
    }
    let Some(p) = app.pve.panel.as_mut() else {
        return;
    };
    let target = if tab == 0 {
        &mut p.snaps.loading
    } else {
        &mut p.backups.loading
    };
    if *target {
        return;
    }
    *target = true;
    let c = conn(app, &s);
    let epoch = app.pve.epoch(s.id);
    let sink = app.pve.sink.clone();
    if tab == 0 {
        pve::spawn_snapshots(&app.rt, s.id, epoch, c, g.id(), sink);
    } else {
        pve::spawn_backups(&app.rt, s.id, epoch, c, g.id(), sink);
    }
}

fn op_label(op: &Op) -> String {
    match op {
        Op::Snapshot { name, .. } => t!("Snapshot „{}“", name),
        Op::Rollback(name) => t!("Rollback auf „{}“", name),
        Op::DeleteSnapshot(name) => t!("Snapshot „{}“ löschen", name),
        Op::Backup { storage, .. } => t!("Backup nach {}", storage),
        Op::Restore { vmid, .. } => t!("Wiederherstellung als {}", vmid),
        Op::Ext(e) => match e.as_ref() {
            ExtOp::Power(p) => match p {
                Power::Start => t!("Start").to_string(),
                Power::Shutdown => t!("Herunterfahren").to_string(),
                Power::Stop => t!("Stopp").to_string(),
                Power::Reboot => t!("Neustart").to_string(),
                Power::Resume => t!("Fortsetzen").to_string(),
            },
            ExtOp::Clone(c) => t!("Klon als {}", c.newid),
            ExtOp::Migrate(m) => t!("Migration nach {}", m.target),
            ExtOp::Hardware(_) => t!("Hardware ändern").to_string(),
        },
    }
}

pub fn start_task(app: &mut App, g: &GuestRef, op: Op) {
    start_task_with(app, g, op, false);
}

/// Starts a tracked task; returns its id (0 if the server is gone).
fn start_task_with(app: &mut App, g: &GuestRef, op: Op, quiet: bool) -> u64 {
    let Some(s) = server_of(app, g.server) else {
        return 0;
    };
    let id = app.pve.next_task;
    app.pve.next_task += 1;
    let cancel = Arc::new(AtomicBool::new(false));
    let what = op_label(&op);
    app.pve.tasks.push(TaskView {
        id,
        server: s.id,
        vmid: g.vmid,
        what: what.clone(),
        guest: g.display(),
        op: op.clone(),
        started: Instant::now(),
        upid: None,
        progress: String::new(),
        done: None,
        finished: None,
        cancel: cancel.clone(),
        quiet,
    });
    let c = conn(app, &s);
    match op {
        Op::Ext(e) => pve::spawn_ext_task(
            &app.rt,
            s.id,
            app.pve.epoch(s.id),
            c,
            g.id(),
            *e,
            id,
            cancel,
            app.pve.sink.clone(),
        ),
        op => pve::spawn_task(
            &app.rt,
            s.id,
            app.pve.epoch(s.id),
            c,
            g.id(),
            op,
            id,
            cancel,
            app.pve.sink.clone(),
        ),
    }
    // The open sheet shows the task itself.
    let shown = app
        .pve
        .panel
        .as_ref()
        .is_some_and(|p| p.guest.server == g.server && p.guest.vmid == g.vmid);
    if !shown && !quiet {
        app.toast(t!("{}: {} gestartet", g.display(), what), theme::accent());
    }
    id
}

/// Rounded group inside the sheet (same look as the form groups).
fn group<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(theme::group_fill())
        .corner_radius(CornerRadius::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Results for the sheet and task updates.
pub fn on_payload(app: &mut App, server: Uuid, vmid: u32, payload: GuestPayload) {
    let panel = app
        .pve
        .panel
        .as_mut()
        .filter(|p| p.guest.server == server && p.guest.vmid == vmid);
    match payload {
        GuestPayload::Snapshots(r) => {
            if let Some(p) = panel {
                p.snaps.loading = false;
                p.snaps.data = Some(r.map_err(|e| e.to_string()));
            }
        }
        GuestPayload::Backups(r) => {
            if let Some(p) = panel {
                p.backups.loading = false;
                if let (Ok(b), Some(f)) = (&r, p.new_backup.as_mut())
                    && !b.stores.iter().any(|s| s.id == f.storage)
                {
                    f.storage = b.stores.first().map(|s| s.id.clone()).unwrap_or_default();
                }
                p.backups.data = Some(r.map_err(|e| e.to_string()));
            }
        }
        GuestPayload::NextId(r) => {
            if let Some(rs) = panel.and_then(|p| p.restore.as_mut()) {
                match r {
                    Ok(id) => rs.vmid = Some(id),
                    Err(e) => rs.error = Some(e.to_string()),
                }
            }
        }
        GuestPayload::Task { task, update } => task_update(app, server, vmid, task, update),
        GuestPayload::Ext(p) => on_ext(app, server, vmid, *p),
        GuestPayload::Address(_) | GuestPayload::Spice(_) | GuestPayload::Console(_) => {}
    }
}

fn on_ext(app: &mut App, server: Uuid, vmid: u32, p: pve::ExtPayload) {
    use pve::ExtPayload as E;
    match p {
        E::Rrd { target, tf, result } => graphs::on_rrd(app, server, target, tf, result),
        E::Sparks(list) => graphs::on_sparks(app, server, list),
        E::Stores { content, result } => {
            let result = result.map_err(|e| e.to_string());
            if content == "backup" {
                batch::on_stores(app, result);
            } else {
                clone::on_stores(app, server, vmid, &result);
                hw::on_stores(app, server, vmid, &result);
            }
        }
        E::CloneInfo(r) => clone::on_info(app, server, vmid, r),
        E::Hardware(r) => hw::on_config(app, server, vmid, r),
    }
}

fn task_update(app: &mut App, server: Uuid, vmid: u32, task: u64, update: TaskUpdate) {
    let Some(t) = app.pve.tasks.iter_mut().find(|t| t.id == task) else {
        return;
    };
    match update {
        TaskUpdate::Started(upid) => t.upid = Some(upid),
        TaskUpdate::Progress(line) => t.progress = line,
        TaskUpdate::Done(result) => {
            let (text, color) = match &result {
                Ok(status) => (
                    t!("{} · {}: fertig ({})", t.guest, t.what, status),
                    theme::green(),
                ),
                Err(e) => (t!("{} · {}: {}", t.guest, t.what, e), theme::red()),
            };
            let reload = match t.op {
                Op::Backup { .. } | Op::Restore { .. } => 1,
                _ => 0,
            };
            t.done = Some(result.clone());
            t.finished = Some(Instant::now());
            if !t.quiet {
                app.toast(text, color);
            }
            batch::task_done(app, task, &result);
            hw::task_done(app, task);
            if app
                .pve
                .panel
                .as_ref()
                .is_some_and(|p| p.guest.server == server && p.guest.vmid == vmid)
            {
                load(app, reload);
            }
            // Rollbacks change the state, restores add a guest.
            schedule_refresh(app, server, &[1]);
        }
    }
}

fn fmt_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d.%m.%Y %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "–".into())
}

fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}:{:02} min", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// What the sheet asks for; applied after drawing.
enum Cmd {
    Tab(usize),
    Reload(usize),
    Task(Op),
    Confirm(String, Op),
    Restore(Backup),
    Close,
}

pub fn panel(app: &mut App, ctx: &egui::Context) {
    // Tasks of open sheets and batches are needed until those are closed.
    let keep: Vec<u64> = batch::held_tasks(app);
    app.pve
        .tasks
        .retain(|t| t.finished.is_none_or(|f| f.elapsed() < KEEP_DONE) || keep.contains(&t.id));
    graphs::pump_sparks(app);
    batch::pump(app);
    graphs::node_sheet(app, ctx);
    clone::sheet(app, ctx);
    hw::hw_sheet(app, ctx);
    hw::migrate_sheet(app, ctx);
    batch::form_sheet(app, ctx);
    batch::result_sheet(app, ctx);
    stats_refresh(app);
    let running = app
        .pve
        .panel
        .as_ref()
        .map(|p| {
            app.pve
                .live
                .get(&p.guest.server)
                .and_then(|l| l.snap.as_ref())
                .and_then(|s| s.guests.iter().find(|g| g.vmid == p.guest.vmid))
                .is_some_and(|g| g.running())
        })
        .unwrap_or(false);
    let confirm_open = app.confirm_delete.is_some();
    let pve = &mut app.pve;
    let Some(p) = pve.panel.as_mut() else {
        return;
    };
    let tasks: Vec<&TaskView> = pve
        .tasks
        .iter()
        .filter(|t| t.server == p.guest.server && t.vmid == p.guest.vmid)
        .rev()
        .collect();
    let mut cmds: Vec<Cmd> = Vec::new();
    let title = format!(
        "{} {} · {}",
        kind_tag(p.guest.kind),
        p.guest.vmid,
        p.guest.display()
    );
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-guest"),
        660.0,
        &title,
        |ui| {
            ui.horizontal(|ui| {
                if let Some(i) = theme::segmented(
                    ui,
                    &[t!("Snapshots"), t!("Backups"), t!("Statistik")],
                    p.tab,
                ) {
                    cmds.push(Cmd::Tab(i));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::icon_btn(ui, Icon::Refresh, t!("Jetzt aktualisieren")).clicked() {
                        cmds.push(Cmd::Reload(p.tab));
                    }
                    let busy = tasks.iter().any(|t| t.done.is_none());
                    if p.tab == 2 {
                        // Statistics: only the reload button.
                    } else if p.tab == 0 {
                        ui.add_enabled_ui(!busy && p.new_snap.is_none(), |ui| {
                            if theme::btn(
                                ui,
                                theme::Btn::SmallPrimary,
                                Some(Icon::Plus),
                                t!("Snapshot erstellen"),
                            )
                            .clicked()
                            {
                                p.new_snap = Some(NewSnap {
                                    name: format!(
                                        "aurora_{}",
                                        chrono::Local::now().format("%Y%m%d_%H%M")
                                    ),
                                    description: String::new(),
                                    vmstate: false,
                                });
                            }
                        });
                    } else {
                        ui.add_enabled_ui(!busy && p.new_backup.is_none(), |ui| {
                            if theme::btn(
                                ui,
                                theme::Btn::SmallPrimary,
                                Some(Icon::Download),
                                t!("Backup jetzt"),
                            )
                            .clicked()
                            {
                                let storage = match &p.backups.data {
                                    Some(Ok(b)) => {
                                        b.stores.first().map(|s| s.id.clone()).unwrap_or_default()
                                    }
                                    _ => String::new(),
                                };
                                p.new_backup = Some(NewBackup { storage, mode: 0 });
                            }
                        });
                    }
                });
            });
            ui.add_space(6.0);
            task_list(ui, &tasks);
            if p.tab == 2 {
                if let Some(tf) = graphs::stats_body(ui, &mut p.stats, false) {
                    p.stats.tf = tf;
                    cmds.push(Cmd::Reload(2));
                }
            } else if p.tab == 0 {
                snapshots_tab(ui, p, running, &mut cmds);
            } else {
                backups_tab(ui, p, &mut cmds);
            }
        },
        |ui, _| theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked(),
    );
    let restore_open = p.restore.is_some();
    restore_dialog(p, ctx, &mut cmds);
    if close || (dismissed && !confirm_open && !restore_open) {
        cmds.push(Cmd::Close);
    }
    let guest = p.guest.clone();
    for cmd in cmds {
        match cmd {
            Cmd::Tab(i) => {
                if let Some(p) = app.pve.panel.as_mut() {
                    p.tab = i;
                    let loaded = match i {
                        0 => p.snaps.data.is_some(),
                        1 => p.backups.data.is_some(),
                        _ => p.stats.has(p.stats.tf),
                    };
                    if loaded {
                        continue;
                    }
                }
                load(app, i);
            }
            Cmd::Reload(i) => load(app, i),
            Cmd::Task(op) => start_task(app, &guest, op),
            Cmd::Confirm(text, op) => {
                app.confirm_delete = Some((text, Action::Pve(PveAction::Task(guest.clone(), op))));
            }
            Cmd::Restore(b) => {
                if let Some(p) = app.pve.panel.as_mut() {
                    p.restore = Some(Restore {
                        backup: b,
                        vmid: None,
                        error: None,
                        typed: String::new(),
                    });
                }
                if let Some(s) = server_of(app, guest.server) {
                    let c = conn(app, &s);
                    pve::spawn_nextid(
                        &app.rt,
                        s.id,
                        app.pve.epoch(s.id),
                        c,
                        guest.vmid,
                        app.pve.sink.clone(),
                    );
                }
            }
            Cmd::Close => app.pve.panel = None,
        }
    }
    if app.pve.tasks.iter().any(|t| t.done.is_none()) {
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn task_list(ui: &mut egui::Ui, tasks: &[&TaskView]) {
    if tasks.is_empty() {
        return;
    }
    group(ui, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                for t in tasks {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        match &t.done {
                            None => {
                                ui.add(egui::Spinner::new().size(14.0).color(theme::accent()));
                            }
                            Some(Ok(_)) => {
                                ui.label(RichText::new("✔").color(theme::green()));
                            }
                            Some(Err(_)) => {
                                ui.label(RichText::new("✖").color(theme::red()));
                            }
                        }
                        ui.label(RichText::new(&t.what).font(theme::medium(13.0)));
                        let status = match &t.done {
                            None => elapsed(t.started.elapsed()),
                            Some(Ok(s)) => s.clone(),
                            Some(Err(e)) => e.clone(),
                        };
                        let color = match &t.done {
                            Some(Err(_)) => theme::red(),
                            _ => theme::text_dim(),
                        };
                        ui.add(
                            egui::Label::new(RichText::new(status).size(12.5).color(color))
                                .truncate(),
                        );
                    });
                    if t.done.is_none() && !t.progress.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            ui.add(
                                egui::Label::new(
                                    theme::mono(&t.progress, 11.5).color(theme::text_dim()),
                                )
                                .truncate(),
                            );
                        });
                    }
                }
            });
    });
    ui.add_space(6.0);
}

fn status_line(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(color)).wrap());
    });
}

fn snapshots_tab(ui: &mut egui::Ui, p: &mut GuestPanel, running: bool, cmds: &mut Vec<Cmd>) {
    let vm = p.guest.kind == Kind::Qemu;
    if let Some(f) = p.new_snap.as_mut() {
        let valid = pve::valid_snapname(&f.name);
        let mut create = false;
        let mut cancel = false;
        theme::form_caption(ui, t!("Neuer Snapshot"));
        theme::form(ui, 110.0, |ui, rows| {
            rows.field(ui, t!("Name"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut f.name)
                        .char_limit(40)
                        .desired_width(f32::INFINITY),
                );
            });
            rows.field(ui, t!("Beschreibung"), |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut f.description)
                        .desired_rows(2)
                        .hint_text(RichText::new(t!("optional")).color(theme::text_dim()))
                        .desired_width(f32::INFINITY),
                );
            });
            if vm && running {
                rows.switch(ui, t!("RAM-Inhalt mitsichern (vmstate)"), &mut f.vmstate);
            }
        });
        if !valid && !f.name.is_empty() {
            status_line(
                ui,
                t!(
                    "Name: mit einem Buchstaben beginnen, dann nur A–Z, a–z, 0–9, _ und - (2–40 Zeichen)"
                ),
                theme::red(),
            );
        } else if vm && f.vmstate {
            status_line(
                ui,
                t!(
                    "Mit RAM dauert der Snapshot länger und braucht Platz in Höhe des Arbeitsspeichers."
                ),
                theme::text_dim(),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_enabled_ui(valid, |ui| {
                    if theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Erstellen")).clicked() {
                        create = true;
                    }
                });
                if theme::btn(ui, theme::Btn::Small, None, t!("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if create {
            cmds.push(Cmd::Task(Op::Snapshot {
                name: f.name.trim().to_string(),
                description: f.description.clone(),
                vmstate: f.vmstate && running && vm,
            }));
            p.new_snap = None;
        } else if cancel {
            p.new_snap = None;
        }
        ui.add_space(10.0);
    }
    match &p.snaps.data {
        None => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
            });
        }
        Some(Err(e)) => status_line(ui, e, theme::red()),
        Some(Ok(list)) if list.iter().all(|s| s.current) => {
            status_line(ui, t!("Noch keine Snapshots."), theme::text_dim());
        }
        Some(Ok(list)) => {
            let list = list.clone();
            let guest = p.guest.clone();
            group(ui, |ui| {
                for (i, s) in list.iter().enumerate() {
                    snapshot_row(ui, &guest, s, i > 0, cmds);
                }
            });
        }
    }
}

fn snapshot_row(ui: &mut egui::Ui, g: &GuestRef, s: &SnapInfo, divider: bool, cmds: &mut Vec<Cmd>) {
    if divider {
        let r = ui.max_rect();
        let y = ui.cursor().top();
        ui.painter().line_segment(
            [Pos2::new(r.left() + 14.0, y), Pos2::new(r.right(), y)],
            Stroke::new(1.0, theme::line()),
        );
    }
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 14 + (s.depth.min(8) as i8) * 16,
            right: 12,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if s.depth > 0 {
                    ui.label(RichText::new("└").color(theme::text_dim()));
                }
                if s.current {
                    ui.label(
                        RichText::new(t!("Jetzt (aktueller Zustand)"))
                            .font(theme::semibold(13.5))
                            .color(theme::accent()),
                    );
                    return;
                }
                ui.label(RichText::new(&s.name).font(theme::semibold(13.5)));
                if let Some(ts) = s.time {
                    ui.label(RichText::new(fmt_time(ts)).size(12.5).color(theme::text_dim()));
                }
                if s.vmstate {
                    theme::tag(ui, "RAM", Some(theme::accent2()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if theme::btn(ui, theme::Btn::Link, Some(Icon::Trash), t!("Löschen")).clicked()
                    {
                        cmds.push(Cmd::Confirm(
                            t!(
                                "Snapshot „{}“ von „{}“ löschen? Das lässt sich nicht rückgängig machen.",
                                s.name,
                                g.display()
                            ),
                            Op::DeleteSnapshot(s.name.clone()),
                        ));
                    }
                    if theme::btn(ui, theme::Btn::Link, None, t!("Zurücksetzen")).clicked() {
                        cmds.push(Cmd::Confirm(
                            t!(
                                "„{}“ auf den Snapshot „{}“ zurücksetzen? Alle Änderungen seit dem Snapshot gehen verloren.",
                                g.display(),
                                s.name
                            ),
                            Op::Rollback(s.name.clone()),
                        ));
                    }
                });
            });
            if !s.current && !s.description.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(&s.description)
                            .size(12.5)
                            .color(theme::text_dim()),
                    )
                    .wrap(),
                );
            }
        });
}

fn backups_tab(ui: &mut egui::Ui, p: &mut GuestPanel, cmds: &mut Vec<Cmd>) {
    let stores = match &p.backups.data {
        Some(Ok(b)) => b.stores.clone(),
        _ => Vec::new(),
    };
    if let Some(f) = p.new_backup.as_mut() {
        let mut start = false;
        let mut cancel = false;
        theme::form_caption(ui, t!("Backup jetzt"));
        theme::form(ui, 110.0, |ui, rows| {
            rows.field(ui, t!("Speicher"), |ui| {
                let sel = stores
                    .iter()
                    .find(|s| s.id == f.storage)
                    .map(store_label)
                    .unwrap_or_else(|| t!("— wählen —").to_string());
                egui::ComboBox::from_id_salt("pve-backup-store")
                    .selected_text(sel)
                    .width(ui.available_width() - 14.0)
                    .show_ui(ui, |ui| {
                        for s in &stores {
                            ui.selectable_value(&mut f.storage, s.id.clone(), store_label(s));
                        }
                    });
            });
            rows.field(ui, t!("Modus"), |ui| {
                if let Some(i) =
                    theme::segmented(ui, &[t!("Snapshot"), t!("Anhalten"), t!("Stoppen")], f.mode)
                {
                    f.mode = i;
                }
            });
        });
        status_line(
            ui,
            match f.mode {
                0 => t!("Läuft im Betrieb weiter (Live-Backup). Komprimierung: zstd."),
                1 => t!("Der Gast wird kurz angehalten. Komprimierung: zstd."),
                _ => t!(
                    "Der Gast wird für das Backup gestoppt und danach wieder gestartet. Komprimierung: zstd."
                ),
            },
            theme::text_dim(),
        );
        if stores.is_empty() && p.backups.data.is_some() {
            status_line(
                ui,
                t!("Auf diesem Node gibt es keinen Speicher für Backups."),
                theme::amber(),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let ok = stores.iter().any(|s| s.id == f.storage);
                ui.add_enabled_ui(ok, |ui| {
                    if theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Backup starten"))
                        .clicked()
                    {
                        start = true;
                    }
                });
                if theme::btn(ui, theme::Btn::Small, None, t!("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if start {
            cmds.push(Cmd::Task(Op::Backup {
                storage: f.storage.clone(),
                mode: BACKUP_MODES[f.mode.min(2)].to_string(),
            }));
            p.new_backup = None;
        } else if cancel {
            p.new_backup = None;
        }
        ui.add_space(10.0);
    }
    match &p.backups.data {
        None => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
            });
        }
        Some(Err(e)) => status_line(ui, e, theme::red()),
        Some(Ok(b)) => {
            for e in &b.errors {
                status_line(ui, e, theme::amber());
            }
            if b.backups.is_empty() {
                status_line(
                    ui,
                    if b.stores.is_empty() {
                        t!("Auf diesem Node gibt es keinen Speicher für Backups.")
                    } else {
                        t!("Noch keine Backups.")
                    },
                    theme::text_dim(),
                );
                return;
            }
            let list = b.backups.clone();
            group(ui, |ui| {
                for (i, b) in list.iter().enumerate() {
                    backup_row(ui, b, i > 0, cmds);
                }
            });
        }
    }
}

fn store_label(s: &pve::BackupStore) -> String {
    if s.total > 0 {
        t!("{} ({}, {} frei)", s.id, s.kind, gib(s.avail))
    } else {
        format!("{} ({})", s.id, s.kind)
    }
}

fn backup_row(ui: &mut egui::Ui, b: &Backup, divider: bool, cmds: &mut Vec<Cmd>) {
    if divider {
        let r = ui.max_rect();
        let y = ui.cursor().top();
        ui.painter().line_segment(
            [Pos2::new(r.left() + 14.0, y), Pos2::new(r.right(), y)],
            Stroke::new(1.0, theme::line()),
        );
    }
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(fmt_time(b.ctime)).font(theme::semibold(13.5)));
                if b.size > 0 {
                    ui.label(
                        RichText::new(gib(b.size))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                }
                theme::tag(ui, &b.storage, None);
                if b.protected {
                    theme::tag(ui, t!("geschützt"), Some(theme::amber()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::btn(ui, theme::Btn::Link, None, t!("Wiederherstellen…"))
                        .on_hover_text(t!("Als neuen Gast mit freier VMID wiederherstellen"))
                        .clicked()
                    {
                        cmds.push(Cmd::Restore(b.clone()));
                    }
                });
            });
            let file = b.volid.split_once(':').map(|(_, f)| f).unwrap_or(&b.volid);
            ui.add(egui::Label::new(theme::mono(file, 11.0).color(theme::text_dim())).truncate());
            if !b.notes.is_empty() {
                ui.label(RichText::new(&b.notes).size(12.5).color(theme::text_dim()));
            }
        });
}

/// Strong confirmation: restores into a new VMID that has to be typed in.
fn restore_dialog(p: &mut GuestPanel, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
    let Some(r) = p.restore.as_mut() else {
        return;
    };
    let mut answer = None;
    let original = p.guest.vmid;
    egui::Modal::new(egui::Id::new("pve-restore"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(theme::heading(t!("Backup wiederherstellen"), 20.0));
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "Das Backup vom {} wird als neuer Gast mit eigener VMID angelegt (neue MAC-Adressen). „{}“ selbst bleibt unverändert.",
                        fmt_time(r.backup.ctime),
                        p.guest.display()
                    ))
                    .color(theme::text_dim()),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            ui.add(egui::Label::new(theme::mono(&r.backup.volid, 11.5).color(theme::text())).wrap());
            ui.add_space(8.0);
            match (r.vmid.as_mut(), &r.error) {
                (_, Some(e)) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                (None, None) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new(t!("Suche freie VMID…")).color(theme::text_dim()));
                    });
                }
                (Some(id), None) => {
                    ui.horizontal(|ui| {
                        ui.label(t!("Neue VMID"));
                        ui.add(egui::DragValue::new(id).range(100..=999_999_999));
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(t!("Zur Bestätigung die neue VMID {} eintippen:", id))
                            .color(theme::text_dim()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut r.typed)
                            .desired_width(f32::INFINITY),
                    );
                    if *id == original {
                        ui.label(
                            RichText::new(t!("Das ist die VMID des Originals – bitte eine freie wählen."))
                                .color(theme::red()),
                        );
                    }
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let ok = r
                        .vmid
                        .is_some_and(|id| id != original && r.typed.trim() == id.to_string());
                    ui.add_enabled_ui(ok, |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(t!("Wiederherstellen")).color(theme::bg_deep()),
                                )
                                .fill(theme::red()),
                            )
                            .clicked()
                        {
                            answer = Some(true);
                        }
                    });
                    if ui.button(t!("Abbrechen")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    match answer {
        Some(true) => {
            if let Some(vmid) = r.vmid {
                cmds.push(Cmd::Task(Op::Restore {
                    volid: r.backup.volid.clone(),
                    vmid,
                }));
            }
            p.restore = None;
        }
        Some(false) => p.restore = None,
        None => {}
    }
}

/// Statistics tab: reload every minute while it is shown.
fn stats_refresh(app: &mut App) {
    let due = app
        .pve
        .panel
        .as_ref()
        .is_some_and(|p| p.tab == 2 && p.stats.stale());
    if due {
        load(app, 2);
    }
}
