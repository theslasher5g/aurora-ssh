//! Transfer queue UI: glue between the file browser and `backend::xfer` (scheduling,
//! connections, persistence), the transfers panel, and the dialogs for the overwrite
//! policy and for continuing interrupted transfers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, RichText, Sense, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{App, ConnState, RemoteConn, TabKind};
use crate::backend::Sink;
use crate::backend::sftp::{Session, SftpEvent};
use crate::backend::xfer::{
    Batch, Ctl, Ep, Expansion, Item, Job, Limiter, Loc, Outcome, Policy, Queue, State, Status,
};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::{human_size, short_duration};

pub enum XferEvent {
    Finished {
        batch: u64,
        item: u64,
        outcome: Outcome,
    },
    Expanded {
        batch: u64,
        result: Result<Expansion, String>,
    },
}

/// A requested copy, before the policy is known.
#[derive(Clone, Debug)]
pub struct Req {
    pub from: Loc,
    pub to: Loc,
    pub from_label: String,
    pub to_label: String,
    pub src_root: String,
    pub dst_dir: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<i64>,
    /// The target name is already in the destination folder.
    pub exists: bool,
}

pub struct PolicyDialog {
    reqs: Vec<Req>,
    choice: Policy,
    remember: bool,
}

struct PoolConn {
    conn: RemoteConn,
}

pub struct XferUi {
    pub q: Queue,
    limiter: Arc<Limiter>,
    /// Running items: item id → (batch id, control).
    ctls: HashMap<u64, (u64, Arc<Ctl>)>,
    sink: Sink<XferEvent>,
    rx: UnboundedReceiver<XferEvent>,
    /// Connections the queue opened itself (restart, dropped file tabs).
    pool: HashMap<Uuid, PoolConn>,
    /// Hosts a connection was requested for: when and how often.
    requested: HashMap<Uuid, (Instant, u8)>,
    /// Policy chosen with "für alle" until the app is closed.
    pub remembered: Option<Policy>,
    pub dialog: Option<PolicyDialog>,
    /// Restored batches waiting for the user's decision.
    restore_prompt: bool,
    loaded: bool,
    last_saved: String,
    dirty: bool,
    last_save: Instant,
    last_tick: Instant,
    /// Batch whose "show in folder" was asked inside a popover.
    pub show_request: Option<u64>,
    /// Speed/ETA are refreshed twice a second.
    last_speed: Instant,
    /// The transfers popover is open, next to this button.
    open: bool,
    anchor: egui::Rect,
}

impl XferUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let (sink, rx) = Sink::new(ctx.clone());
        Self {
            q: Queue::new(),
            limiter: Arc::new(Limiter::default()),
            ctls: HashMap::new(),
            sink,
            rx,
            pool: HashMap::new(),
            requested: HashMap::new(),
            remembered: None,
            dialog: None,
            restore_prompt: false,
            loaded: false,
            last_saved: String::new(),
            dirty: false,
            last_save: Instant::now(),
            last_tick: Instant::now(),
            show_request: None,
            last_speed: Instant::now(),
            open: false,
            anchor: egui::Rect::NOTHING,
        }
    }

    /// Items waiting or running (for the badge).
    pub fn active(&self) -> usize {
        self.q.active_count()
    }

    /// Queues copies; asks for the overwrite policy first if a target exists.
    pub fn submit(&mut self, reqs: Vec<Req>) {
        if reqs.is_empty() {
            return;
        }
        let conflict = reqs.iter().any(|r| r.exists);
        match (conflict, self.remembered) {
            (true, None) => {
                self.dialog = Some(PolicyDialog {
                    reqs,
                    choice: Policy::Overwrite,
                    remember: false,
                });
            }
            (true, Some(p)) => self.enqueue(reqs, p),
            (false, _) => self.enqueue(reqs, self.remembered.unwrap_or(Policy::Overwrite)),
        }
    }

    pub fn enqueue(&mut self, reqs: Vec<Req>, policy: Policy) {
        for r in reqs {
            let mut b = Batch {
                name: r.name.clone(),
                from: Some(r.from),
                to: Some(r.to),
                from_label: r.from_label,
                to_label: r.to_label,
                src_root: r.src_root.clone(),
                dst_dir: r.dst_dir.clone(),
                is_dir: r.is_dir,
                policy,
                created: chrono::Utc::now().timestamp(),
                ..Default::default()
            };
            if !r.is_dir {
                let id = self.q.alloc_id();
                let dst = match r.to {
                    Loc::Local => std::path::Path::new(&r.dst_dir)
                        .join(&r.name)
                        .display()
                        .to_string(),
                    Loc::Host(_) => crate::backend::sftp::join(&r.dst_dir, &r.name),
                };
                b.expanded = true;
                b.items.push(Item {
                    id,
                    src: r.src_root,
                    dst,
                    size: r.size,
                    mtime: r.mtime,
                    ..Default::default()
                });
            }
            self.q.add(b);
        }
        self.dirty = true;
    }

    fn stop_items(&mut self, ids: &[u64], cancel: bool) {
        for id in ids {
            if let Some((_, ctl)) = self.ctls.get(id) {
                ctl.request_stop(cancel);
            }
        }
    }

    pub fn pause(&mut self, batch: u64) {
        let ids = self.q.pause(batch);
        self.stop_items(&ids, false);
        self.dirty = true;
    }

    pub fn resume(&mut self, batch: u64) {
        self.q.resume(batch);
        self.dirty = true;
    }

    pub fn cancel(&mut self, batch: u64) {
        let ids = self.q.cancel(batch);
        self.stop_items(&ids, true);
        self.dirty = true;
    }

    pub fn retry(&mut self, batch: u64) {
        self.q.retry(batch);
        self.dirty = true;
    }

    pub fn remove(&mut self, batch: u64) {
        let ids = self.q.remove(batch);
        self.stop_items(&ids, true);
        self.dirty = true;
    }

    /// Stops everything that runs (app exit); partial files stay for the next start.
    pub fn shutdown(&mut self) {
        for (_, c) in self.ctls.values() {
            c.request_stop(false);
        }
        self.save(true);
    }

    fn path() -> std::path::PathBuf {
        crate::store::data_dir().join("transfers.json")
    }

    fn save(&mut self, force: bool) {
        if !self.dirty && !force {
            return;
        }
        let big = self.q.batches.iter().map(|b| b.items.len()).sum::<usize>() > 5000;
        let every = if big { 5 } else { 1 };
        if !force && self.last_save.elapsed() < Duration::from_secs(every) {
            return;
        }
        self.last_save = Instant::now();
        self.dirty = false;
        let json = self.q.to_json();
        let empty = !self
            .q
            .batches
            .iter()
            .any(|b| !b.finished() || b.rt.stats.failed > 0);
        let path = Self::path();
        if empty {
            if !self.last_saved.is_empty() {
                let _ = std::fs::remove_file(&path);
                self.last_saved.clear();
            }
            return;
        }
        if json == self.last_saved {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.last_saved = json;
        }
    }
}

// ---------------------------------------------------------------------------- connections

/// Called when a connection for the queue was opened (`Purpose::XferPool`).
pub fn pool_attach(
    app: &mut App,
    host: &crate::model::Host,
    params: crate::backend::ssh::SshParams,
    ctx: &egui::Context,
) {
    let conn = crate::ui::sftp::spawn_conn(
        &app.rt,
        params,
        host.display_name().to_string(),
        Some(host.id),
        ctx,
        app.prompts_sink.clone(),
    );
    app.xfer.pool.insert(host.id, PoolConn { conn });
    ctx.request_repaint();
}

fn pump_pool(x: &mut XferUi) {
    for pc in x.pool.values_mut() {
        let r = &mut pc.conn;
        while let Ok(ev) = r.rx.try_recv() {
            match ev {
                SftpEvent::Status(s) => r.state = ConnState::Connecting(s),
                SftpEvent::Connected { session, .. } => {
                    r.state = ConnState::Connected;
                    r.session = Some(session);
                }
                SftpEvent::Closed(err) => {
                    r.state = ConnState::Closed(err);
                    r.session = None;
                }
                _ => {}
            }
        }
    }
    // Dead connections are dropped so that a new one can be requested.
    x.pool
        .retain(|_, pc| !matches!(pc.conn.state, ConnState::Closed(_)));
}

/// Connected sessions per host: file tabs first, then the queue's own connections.
fn sessions(app: &App) -> HashMap<Uuid, Session> {
    let mut m = HashMap::new();
    for t in &app.tabs {
        if let TabKind::Sftp(s) = &t.kind {
            for p in &s.panes {
                if let Some(r) = &p.remote
                    && matches!(r.state, ConnState::Connected)
                    && let (Some(h), Some(sess)) = (r.host_id, &r.session)
                {
                    m.entry(h).or_insert_with(|| sess.clone());
                }
            }
        }
    }
    for (h, pc) in &app.xfer.pool {
        if matches!(pc.conn.state, ConnState::Connected)
            && let Some(sess) = &pc.conn.session
        {
            m.entry(*h).or_insert_with(|| sess.clone());
        }
    }
    m
}

fn ep_of(loc: Loc, sessions: &HashMap<Uuid, Session>) -> Option<Ep> {
    match loc {
        Loc::Local => Some(Ep::Local),
        Loc::Host(h) => sessions.get(&h).cloned().map(Ep::Remote),
    }
}

// ---------------------------------------------------------------------------- pump

/// Runs every frame: collects results, starts work, requests connections, saves.
pub fn pump(app: &mut App, ctx: &egui::Context) {
    if app.vault.is_none() {
        return;
    }
    if !app.xfer.loaded {
        load(app);
    }
    pump_pool(&mut app.xfer);

    // Results of workers.
    let mut events = Vec::new();
    while let Ok(ev) = app.xfer.rx.try_recv() {
        events.push(ev);
    }
    let changed = !events.is_empty();
    for ev in events {
        match ev {
            XferEvent::Finished {
                batch,
                item,
                outcome,
            } => {
                if let Some((_, ctl)) = app.xfer.ctls.remove(&item) {
                    let done = ctl.done.load(std::sync::atomic::Ordering::Relaxed);
                    app.xfer.q.set_progress((batch, item), done);
                }
                app.xfer.q.apply((batch, item), outcome);
            }
            XferEvent::Expanded { batch, result } => match result {
                Ok(ex) => app
                    .xfer
                    .q
                    .set_expanded(batch, ex.root_dst, ex.items, ex.skipped),
                Err(e) => app.xfer.q.expansion_failed(batch, e),
            },
        }
    }
    if changed {
        app.xfer.dirty = true;
    }

    let slots = app.lib.settings.transfer_slots.clamp(1, 16) as usize;
    app.xfer
        .limiter
        .set_kbps(app.lib.settings.transfer_limit_kbps);
    let verify = app.lib.settings.transfer_verify;
    let sessions = sessions(app);

    // Folder batches: list their files first.
    for id in app.xfer.q.to_expand() {
        let Some(b) = app.xfer.q.batch(id) else {
            continue;
        };
        let (Some(from), Some(to)) = (b.from, b.to) else {
            continue;
        };
        let (Some(src), Some(dst)) = (ep_of(from, &sessions), ep_of(to, &sessions)) else {
            continue;
        };
        let (src_root, dst_dir, name, policy) = (
            b.src_root.clone(),
            b.dst_dir.clone(),
            b.name.clone(),
            b.policy,
        );
        let sink = app.xfer.sink.clone();
        app.rt.spawn(async move {
            let result =
                crate::backend::xfer::expand(&src, &dst, &src_root, &dst_dir, &name, policy)
                    .await
                    .map_err(|e| format!("{e:#}"));
            sink.send(XferEvent::Expanded { batch: id, result });
        });
        if let Some(b) = app.xfer.q.batch_mut(id) {
            b.rt.expanding = true;
        }
    }

    // Start waiting items up to the concurrency limit.
    let ready = |loc: Loc| match loc {
        Loc::Local => true,
        Loc::Host(h) => sessions.contains_key(&h),
    };
    for (bid, iid) in app.xfer.q.runnable(slots, ready) {
        let Some(b) = app.xfer.q.batch(bid) else {
            continue;
        };
        let (Some(from), Some(to)) = (b.from, b.to) else {
            continue;
        };
        let Some(it) = b.items.iter().find(|i| i.id == iid) else {
            continue;
        };
        let (Some(src_ep), Some(dst_ep)) = (ep_of(from, &sessions), ep_of(to, &sessions)) else {
            continue;
        };
        let (src, dst, size, mtime, policy) =
            (it.src.clone(), it.dst.clone(), it.size, it.mtime, b.policy);
        let resumable = app.xfer.q.mark_running((bid, iid));
        let ctl = Arc::new(Ctl::default());
        app.xfer.ctls.insert(iid, (bid, ctl.clone()));
        let job = Job {
            src_ep,
            dst_ep,
            src,
            dst,
            size,
            mtime,
            policy,
            verify,
            resumable,
            ctl,
            limiter: app.xfer.limiter.clone(),
        };
        let sink = app.xfer.sink.clone();
        app.rt.spawn(async move {
            let outcome = crate::backend::xfer::run_item(job).await;
            sink.send(XferEvent::Finished {
                batch: bid,
                item: iid,
                outcome,
            });
        });
        app.xfer.dirty = true;
    }

    // Connections for hosts that work is waiting for.
    let now = Instant::now();
    let mut connect: Vec<Uuid> = Vec::new();
    for h in app.xfer.q.needed_hosts() {
        if sessions.contains_key(&h) || app.xfer.pool.contains_key(&h) {
            app.xfer.requested.remove(&h);
            continue;
        }
        match app.xfer.requested.get(&h).copied() {
            None => {
                app.xfer.requested.insert(h, (now, 1));
                connect.push(h);
            }
            Some((at, n)) if now.duration_since(at) > Duration::from_secs(20) => {
                if n >= 3 {
                    app.xfer.requested.remove(&h);
                    fail_waiting(&mut app.xfer.q, h);
                    app.xfer.dirty = true;
                } else {
                    app.xfer.requested.insert(h, (now, n + 1));
                    connect.push(h);
                }
            }
            _ => {}
        }
    }
    for h in connect {
        if let Some(host) = app.lib.host(h).cloned() {
            app.connect_host(
                crate::ui::sftp::sftp_target(host),
                crate::app::Purpose::XferPool,
                ctx,
            );
        } else {
            fail_waiting(&mut app.xfer.q, h);
        }
    }

    // Progress, speed, counters.
    let mut any_running = false;
    let progress: Vec<(u64, u64, u64)> = app
        .xfer
        .ctls
        .iter()
        .map(|(i, (b, c))| (*b, *i, c.done.load(std::sync::atomic::Ordering::Relaxed)))
        .collect();
    for (b, i, done) in progress {
        app.xfer.q.set_progress((b, i), done);
        any_running = true;
    }
    app.xfer.q.recount();
    let tick = app.xfer.last_speed.elapsed() >= Duration::from_millis(500);
    if tick {
        app.xfer.last_speed = Instant::now();
        let now = Instant::now();
        for b in &mut app.xfer.q.batches {
            if b.rt.stats.running > 0 {
                b.rt.speed.update(now, b.rt.stats.bytes_done);
            } else {
                b.rt.speed.reset();
            }
        }
    }

    // Finished batches: notice and refresh the file panes.
    let mut notices: Vec<(String, Color32)> = Vec::new();
    let mut refresh: Vec<Loc> = Vec::new();
    for b in &mut app.xfer.q.batches {
        if b.finished() && b.rt.was_active && !b.rt.notified {
            b.rt.notified = true;
            let s = b.rt.stats;
            if let Some(to) = b.to {
                refresh.push(to);
            }
            if s.cancelled > 0 && s.failed == 0 {
                continue;
            }
            let size = human_size(s.bytes_total);
            let (text, color) = if s.failed == 0 && b.error.is_none() {
                let skipped = if s.skipped > 0 {
                    t!(", {} übersprungen", s.skipped)
                } else {
                    String::new()
                };
                (
                    if s.files <= 1 {
                        t!("„{}“ übertragen ({})", b.name, size)
                    } else {
                        t!(
                            "„{}“ übertragen: {} Dateien, {}{}",
                            b.name,
                            s.done - s.skipped,
                            size,
                            skipped
                        )
                    },
                    theme::green(),
                )
            } else if let Some(e) = &b.error {
                (t!("„{}“ fehlgeschlagen: {}", b.name, e), theme::red())
            } else {
                (
                    t!(
                        "„{}“: {} von {} Dateien übertragen, {} mit Fehler",
                        b.name,
                        s.done,
                        s.files,
                        s.failed
                    ),
                    theme::red(),
                )
            };
            notices.push((text, color));
        }
    }
    if !notices.is_empty() {
        app.xfer.dirty = true;
        if !ctx.input(|i| i.focused) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
    }
    for (t, c) in notices {
        app.toast(t, c);
    }
    for loc in refresh {
        crate::ui::sftp::refresh_panes_for(app, loc);
    }

    app.xfer.save(false);
    if any_running || app.xfer.q.running() > 0 || !app.xfer.q.to_expand().is_empty() {
        ctx.request_repaint_after(Duration::from_millis(250));
    } else if app.xfer.q.active_count() > 0 {
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    app.xfer.last_tick = Instant::now();
}

fn fail_waiting(q: &mut Queue, host: Uuid) {
    let msg = t!("Keine Verbindung zum Server möglich").to_string();
    for b in &mut q.batches {
        if b.from != Some(Loc::Host(host)) && b.to != Some(Loc::Host(host)) {
            continue;
        }
        if !b.expanded {
            b.error = Some(msg.clone());
        }
        for it in b.items.iter_mut().filter(|i| i.state == State::Waiting) {
            it.state = State::Failed;
            it.error = Some(msg.clone());
        }
        b.recount();
    }
}

fn load(app: &mut App) {
    app.xfer.loaded = true;
    let Ok(json) = std::fs::read_to_string(XferUi::path()) else {
        return;
    };
    let hosts: HashSet<Uuid> = app.lib.hosts.iter().map(|h| h.id).collect();
    let q = Queue::from_json(&json, |h| hosts.contains(&h));
    app.xfer.last_saved = json;
    if q.batches.iter().any(|b| !b.finished()) {
        app.xfer.restore_prompt = true;
    }
    app.xfer.q = q;
}

// ---------------------------------------------------------------------------- dialogs

pub fn windows(app: &mut App, ctx: &egui::Context) {
    popover(app, ctx);
    policy_dialog(app, ctx);
    restore_dialog(app, ctx);
}

fn policy_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(d) = app.xfer.dialog.as_mut() else {
        return;
    };
    let mut go = false;
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("xfer-policy"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(theme::heading(t!("Ziel existiert schon"), 18.0));
            ui.add_space(6.0);
            let names: Vec<&str> = d
                .reqs
                .iter()
                .filter(|r| r.exists)
                .map(|r| r.name.as_str())
                .collect();
            let mut shown = names.iter().take(4).copied().collect::<Vec<_>>().join(", ");
            if names.len() > 4 {
                shown.push_str(&format!(" … (+{})", names.len() - 4));
            }
            ui.add(
                egui::Label::new(RichText::new(t!("Schon vorhanden: {}", shown)).size(13.5)).wrap(),
            );
            ui.add_space(8.0);
            for (p, label) in [
                (Policy::Overwrite, t!("Überschreiben")),
                (Policy::Skip, t!("Überspringen")),
                (Policy::Rename, t!("Umbenennen (Name (1).ext)")),
                (
                    Policy::NewerOnly,
                    t!("Nur überschreiben, wenn die Quelle neuer ist"),
                ),
            ] {
                ui.radio_value(&mut d.choice, p, label);
            }
            ui.add_space(6.0);
            ui.checkbox(
                &mut d.remember,
                t!("Für alle weiteren Übertragungen merken"),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Übertragen")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    go = true;
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    cancel = true;
                }
            });
        });
    if go && let Some(d) = app.xfer.dialog.take() {
        if d.remember {
            app.xfer.remembered = Some(d.choice);
        }
        app.xfer.enqueue(d.reqs, d.choice);
    } else if cancel {
        app.xfer.dialog = None;
    }
}

fn restore_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.xfer.restore_prompt {
        return;
    }
    let n = app.xfer.q.unfinished();
    if n == 0 {
        app.xfer.restore_prompt = false;
        return;
    }
    let names: Vec<String> = app
        .xfer
        .q
        .batches
        .iter()
        .filter(|b| !b.finished())
        .take(4)
        .map(|b| b.name.clone())
        .collect();
    enum C {
        Resume,
        Discard,
        Later,
    }
    let mut c = None;
    egui::Modal::new(egui::Id::new("xfer-restore"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(theme::heading(
                t!("{} unvollständige Übertragungen fortsetzen?", n),
                18.0,
            ));
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "Aus der letzten Sitzung: {}. Bereits übertragene Teile werden nicht noch einmal gesendet.",
                        names.join(", ")
                    ))
                    .size(13.5),
                )
                .wrap(),
            );
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Fortsetzen")).clicked() {
                    c = Some(C::Resume);
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Später")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    c = Some(C::Later);
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Verwerfen")).clicked() {
                    c = Some(C::Discard);
                }
            });
        });
    match c {
        Some(C::Resume) => {
            app.xfer.restore_prompt = false;
            let ids: Vec<u64> = app
                .xfer
                .q
                .batches
                .iter()
                .filter(|b| !b.finished())
                .map(|b| b.id)
                .collect();
            for id in ids {
                app.xfer.resume(id);
            }
        }
        Some(C::Discard) => {
            app.xfer.restore_prompt = false;
            app.xfer.q.batches.clear();
            app.xfer.dirty = true;
            app.xfer.save(true);
        }
        Some(C::Later) => app.xfer.restore_prompt = false,
        None => {}
    }
}

// ---------------------------------------------------------------------------- panel

fn status_text(s: Status) -> (&'static str, Color32) {
    match s {
        Status::Waiting => (t!("wartet"), theme::amber()),
        Status::Running => (t!("läuft"), theme::accent()),
        Status::Paused => (t!("pausiert"), theme::text_dim()),
        Status::Done => (t!("fertig"), theme::green()),
        Status::Failed => (t!("Fehler"), theme::red()),
        Status::Cancelled => (t!("abgebrochen"), theme::text_dim()),
    }
}

fn direction(b: &Batch) -> (Icon, &'static str) {
    match (b.from, b.to) {
        (Some(Loc::Local), Some(Loc::Host(_))) => (Icon::Upload, t!("Upload")),
        (Some(Loc::Host(_)), Some(Loc::Local)) => (Icon::Download, t!("Download")),
        (Some(Loc::Host(_)), Some(Loc::Host(_))) => (Icon::Transfer, t!("Server ⇄ Server")),
        _ => (Icon::Copy, t!("Kopie")),
    }
}

/// The transfers list (inside a popover or any other container).
pub fn panel(ui: &mut egui::Ui, x: &mut XferUi) {
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.horizontal(|ui| {
        theme::section_title(ui, t!("Übertragungen"));
        let active = x.q.active_count();
        if active > 0 {
            ui.label(
                RichText::new(t!("{} aktiv", active))
                    .size(12.0)
                    .color(theme::accent()),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if x.q.batches.iter().any(|b| b.complete())
                && theme::btn(ui, theme::Btn::Link, None, t!("Fertige entfernen")).clicked()
            {
                x.q.clear_finished();
                x.dirty = true;
            }
            if x.remembered.is_some()
                && theme::btn(
                    ui,
                    theme::Btn::Link,
                    None,
                    t!("Überschreib-Auswahl vergessen"),
                )
                .clicked()
            {
                x.remembered = None;
            }
        });
    });
    ui.add_space(4.0);
    if x.q.batches.is_empty() {
        ui.label(
            RichText::new(t!(
                "Keine Übertragungen. Kopieren im Dateibrowser stellt Dateien und Ordner hier in die Warteschlange; unterbrochene Übertragungen lassen sich fortsetzen."
            ))
            .size(12.5)
            .color(theme::text_dim()),
        );
        return;
    }
    let mut actions: Vec<(u64, Op)> = Vec::new();
    let rows = |ui: &mut egui::Ui, actions: &mut Vec<(u64, Op)>| {
        for (i, b) in x.q.batches.iter().enumerate().rev() {
            batch_row(ui, b, i > 0, actions);
        }
    };
    if x.q.batches.len() > 4 {
        egui::ScrollArea::vertical()
            .max_height(440.0)
            .min_scrolled_height(440.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                rows(ui, &mut actions);
            });
    } else {
        rows(ui, &mut actions);
    }
    for (id, op) in actions {
        match op {
            Op::Pause => x.pause(id),
            Op::Resume => x.resume(id),
            Op::Cancel => x.cancel(id),
            Op::Retry => x.retry(id),
            Op::Remove => x.remove(id),
            Op::Show => x.show_request = Some(id),
        }
    }
}

enum Op {
    Pause,
    Resume,
    Cancel,
    Retry,
    Remove,
    Show,
}

fn batch_row(ui: &mut egui::Ui, b: &Batch, divider: bool, actions: &mut Vec<(u64, Op)>) {
    let status = b.status();
    let (label, color) = status_text(status);
    let s = b.rt.stats;
    if divider {
        ui.add_space(2.0);
        theme::hairline(ui);
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (icon, dir_label) = direction(b);
        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        crate::ui::icons::paint(ui.painter(), r, icon, theme::accent());
        let _ = dir_label;
        // Buttons first (right), so the name gets what is left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if theme::icon_btn(ui, Icon::Close, t!("Aus der Liste entfernen")).clicked() {
                actions.push((b.id, Op::Remove));
            }
            if matches!(status, Status::Failed | Status::Cancelled)
                && theme::icon_btn(ui, Icon::Refresh, t!("Erneut versuchen (setzt fort)")).clicked()
            {
                actions.push((b.id, Op::Retry));
            }
            if matches!(status, Status::Running | Status::Waiting)
                && theme::icon_btn(ui, Icon::Stop, t!("Abbrechen")).clicked()
            {
                actions.push((b.id, Op::Cancel));
            }
            if matches!(status, Status::Paused)
                && theme::icon_btn(ui, Icon::Play, t!("Fortsetzen")).clicked()
            {
                actions.push((b.id, Op::Resume));
            }
            if matches!(status, Status::Running | Status::Waiting)
                && theme::icon_btn(ui, Icon::Pause, t!("Pausieren")).clicked()
            {
                actions.push((b.id, Op::Pause));
            }
            if theme::icon_btn(ui, Icon::Folder, t!("Im Ordner zeigen")).clicked() {
                actions.push((b.id, Op::Show));
            }
            ui.add_space(8.0);
            ui.label(RichText::new(label).size(12.0).color(color));
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&b.name).font(theme::semibold(13.5))).truncate(),
                );
            });
        });
    });
    // Route and counters.
    let files = if b.is_dir || s.files > 1 {
        format!(
            "{} · {}",
            t!("{} von {} Dateien", s.done, s.files.max(s.done)),
            if b.expanded {
                String::new()
            } else {
                t!("liest Ordner…").to_string()
            }
        )
    } else {
        String::new()
    };
    let route = format!("{} → {}", b.from_label, b.to_label);
    ui.add(
        egui::Label::new(
            RichText::new(format!("{route}   {files}"))
                .size(12.0)
                .color(theme::text_dim()),
        )
        .truncate(),
    );
    // Progress.
    let frac = if s.bytes_total > 0 {
        (s.bytes_done as f32 / s.bytes_total as f32).clamp(0.0, 1.0)
    } else if status == Status::Done {
        1.0
    } else {
        0.0
    };
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 250.0).max(80.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(w, 12.0), Sense::hover());
        let line = egui::Rect::from_center_size(r.center(), Vec2::new(r.width(), 5.0));
        ui.painter()
            .rect_filled(line, CornerRadius::same(3), theme::fill_strong());
        let mut done = line;
        done.set_width(line.width() * frac);
        let bar = match status {
            Status::Failed => theme::red(),
            Status::Done => theme::green(),
            Status::Paused | Status::Cancelled => theme::text_dim(),
            _ => theme::accent(),
        };
        ui.painter().rect_filled(done, CornerRadius::same(3), bar);
        let mut info = format!(
            "{} / {}",
            human_size(s.bytes_done),
            human_size(s.bytes_total)
        );
        if status == Status::Running {
            let bps = b.rt.speed.bps;
            if bps > 1.0 {
                info.push_str(&format!(" · {}/s", human_size(bps as u64)));
                if let Some(eta) = b.rt.speed.eta(s.bytes_total.saturating_sub(s.bytes_done)) {
                    info.push_str(&format!(" · {}", short_duration(eta)));
                }
            }
        }
        ui.add(egui::Label::new(theme::mono(info, 11.5).color(theme::text_dim())).truncate());
    });
    // Problems and notes.
    let err = b
        .error
        .clone()
        .or_else(|| {
            b.items
                .iter()
                .find(|i| i.state == State::Failed)
                .and_then(|i| {
                    i.error
                        .clone()
                        .map(|e| format!("{}: {e}", short_name(&i.src)))
                })
        })
        .or_else(|| {
            b.items
                .iter()
                .find(|i| i.state == State::Waiting && i.error.is_some())
                .and_then(|i| {
                    i.error
                        .clone()
                        .map(|e| t!("Verbindung unterbrochen, versuche es erneut ({})", e))
                })
        });
    if let Some(e) = err {
        ui.add(egui::Label::new(RichText::new(e).size(12.0).color(theme::red())).wrap());
    } else if s.running == 0 && s.waiting > 0 && !b.paused {
        ui.label(
            RichText::new(t!("wartet auf freien Platz oder Verbindung"))
                .size(11.5)
                .color(theme::text_dim()),
        );
    }
    if b.unsafe_skipped > 0 {
        ui.label(
            RichText::new(t!(
                "{} Einträge mit unsicheren Namen übersprungen",
                b.unsafe_skipped
            ))
            .size(11.5)
            .color(theme::amber()),
        );
    }
    if status == Status::Done
        && let Some(note) = b.items.iter().rev().find_map(|i| i.note.clone())
    {
        ui.label(RichText::new(note).size(11.5).color(theme::text_dim()));
    }
}

fn short_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Header button: opens the panel in a popover.
pub fn header_button(ui: &mut egui::Ui, x: &mut XferUi) {
    let n = x.active();
    let label = if n > 0 {
        t!("Übertragungen · {}", n)
    } else {
        t!("Übertragungen").to_string()
    };
    let resp = theme::btn(
        ui,
        if n > 0 {
            theme::Btn::SmallPrimary
        } else {
            theme::Btn::Small
        },
        Some(Icon::Transfer),
        &label,
    );
    toggle(x, &resp);
}

/// A click on a transfers button opens/closes the popover next to it.
pub fn toggle(x: &mut XferUi, resp: &egui::Response) {
    if resp.clicked() {
        x.open = !x.open;
        x.anchor = resp.rect;
    }
}

fn popover(app: &mut App, ctx: &egui::Context) {
    if !app.xfer.open {
        return;
    }
    let screen = ctx.content_rect();
    let a = app.xfer.anchor;
    let width = 580.0_f32.min(screen.width() - 20.0);
    let pos = if a.center().x < 200.0 {
        egui::pos2(a.right() + 10.0, a.top().max(8.0))
    } else {
        egui::pos2((a.right() - width).max(8.0), a.bottom() + 6.0)
    };
    let area = egui::Area::new(egui::Id::new("xfer-popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    panel(ui, &mut app.xfer);
                });
        });
    let over = area.response.rect.contains(
        ctx.input(|i| i.pointer.interact_pos())
            .unwrap_or(egui::Pos2::ZERO),
    );
    let clicked = ctx.input(|i| i.pointer.any_pressed());
    let on_anchor = ctx
        .input(|i| i.pointer.interact_pos())
        .is_some_and(|p| a.contains(p));
    if (clicked && !over && !on_anchor) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.xfer.open = false;
    }
}

/// "Im Ordner zeigen".
pub fn show_in_folder(app: &mut App, batch: u64, ctx: &egui::Context) {
    let Some(b) = app.xfer.q.batch(batch) else {
        return;
    };
    let (Some(to), dir) = (b.to, b.dst_dir.clone()) else {
        return;
    };
    match to {
        Loc::Local => {
            if let Err(e) = open_folder(&dir) {
                app.toast(format!("{e}"), theme::red());
            }
        }
        Loc::Host(h) => crate::ui::sftp::show_remote_folder(app, h, dir, ctx),
    }
}

fn open_folder(dir: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    std::process::Command::new("explorer").arg(dir).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(dir).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(dir).spawn()?;
    Ok(())
}

// ---------------------------------------------------------------------------- settings

pub fn settings_card(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Gleichzeitige Übertragungen"),
            Some(t!(
                "Dateien, die gleichzeitig kopiert werden; Ordner werden in Dateien aufgeteilt"
            )),
            |ui| {
                *changed |= ui
                    .add(
                        egui::DragValue::new(&mut app.lib.settings.transfer_slots)
                            .range(1..=16)
                            .speed(0.1),
                    )
                    .changed();
            },
        );
        rows.setting(
            ui,
            t!("Bandbreite begrenzen"),
            Some(t!("KB/s für alle Übertragungen zusammen, 0 = unbegrenzt")),
            |ui| {
                *changed |= ui
                    .add(
                        egui::DragValue::new(&mut app.lib.settings.transfer_limit_kbps)
                            .range(0..=10_000_000)
                            .speed(10.0)
                            .suffix(" KB/s"),
                    )
                    .changed();
            },
        );
        rows.setting(
            ui,
            t!("Prüfsumme prüfen"),
            Some(t!("Vergleicht nach jeder Datei die Größe und, wenn der Server sha256sum hat, den SHA-256")),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.transfer_verify).changed();
            },
        );
    });
}
