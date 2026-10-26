//! Sheets for Proxmox Backup Server: archive browser with downloads, prune with a
//! server-side dry run first, verify / garbage collection confirmations and the list
//! of running backup-server tasks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use egui::{CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use super::Act;
use crate::app::App;
use crate::backend::proxmox::cluster::pbs::{Keep, PbsSnapshot};
use crate::backend::proxmox::pve2::pbs::{
    self as pb, Entry, PruneReport, SnapRef, Start, VerifyScope,
};
use crate::backend::proxmox::{Error, TaskUpdate};
use crate::theme;
use crate::ui::icons::{self, Icon};

use super::super::cluster::pbs as cpbs;

const MAX_LOG: usize = 500;
const MAX_ROWS: usize = 400;

pub fn fmt_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn fmt_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn ctx_for(app: &App, server: Uuid) -> Option<pb::Ctx> {
    let s = cpbs::server(app, server)?;
    Some(pb::Ctx {
        server,
        epoch: app.pve.cluster.pbs.epoch(server),
        conn: cpbs::conn(app, &s),
        sink: app.pve.p2.pbs_sink.clone(),
    })
}

// ------------------------------------------------------------------ state

#[derive(Clone, Debug, PartialEq)]
pub enum DlState {
    Running,
    Done(PathBuf, u64),
    Failed(String),
}

pub struct Download {
    pub id: u64,
    pub name: String,
    pub done: u64,
    pub total: Option<u64>,
    pub state: DlState,
    pub cancel: Arc<AtomicBool>,
}

pub struct Job {
    pub id: u64,
    pub server: Uuid,
    pub server_name: String,
    pub store: String,
    pub title: String,
    pub upid: Option<String>,
    pub lines: Vec<String>,
    pub done: Option<Result<String, String>>,
    pub cancel: Arc<AtomicBool>,
    pub started: Instant,
    pub show_log: bool,
    pub is_gc: bool,
}

#[derive(Default)]
pub struct Jobs {
    pub open: bool,
    pub list: Vec<Job>,
    pub downloads: Vec<Download>,
}

impl Jobs {
    pub fn running(&self) -> usize {
        self.list.iter().filter(|j| j.done.is_none()).count()
    }
}

// ------------------------------------------------------------------ events

pub fn on_event(app: &mut App, ev: pb::Event) {
    let server = ev.server;
    if app.pve.cluster.pbs.epoch(server) != ev.epoch
        || !app.lib.pbs_servers.iter().any(|s| s.id == server)
    {
        return;
    }
    if ev.session.is_some() {
        app.pve.cluster.pbs.live.entry(server).or_default().session = ev.session;
    }
    match ev.payload {
        pb::Payload::Browse {
            store,
            snap,
            path,
            result,
        } => {
            if let Some(b) = app
                .pve
                .p2
                .browser
                .as_mut()
                .filter(|b| b.server == server && b.store == store && b.snap == snap)
            {
                b.cache
                    .insert(path.clone(), result.map_err(|e| e.to_string()));
                if b.loading.as_deref() == Some(path.as_str()) {
                    b.loading = None;
                }
            }
        }
        pb::Payload::DlProgress { id, done, total } => {
            if let Some(d) = app.pve.p2.jobs.downloads.iter_mut().find(|d| d.id == id) {
                d.done = done;
                d.total = total.or(d.total);
            }
        }
        pb::Payload::DlDone { id, result } => {
            let name = app
                .pve
                .p2
                .jobs
                .downloads
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            if let Some(d) = app.pve.p2.jobs.downloads.iter_mut().find(|d| d.id == id) {
                match &result {
                    Ok((p, n)) => {
                        d.done = *n;
                        d.state = DlState::Done(p.clone(), *n);
                    }
                    Err(e) => d.state = DlState::Failed(e.clone()),
                }
            }
            match result {
                Ok((p, n)) => app.toast(
                    t!("„{}“ gespeichert: {} ({})", name, p.display(), fmt_bytes(n)),
                    theme::green(),
                ),
                Err(e) => app.toast(
                    t!("Download „{}“ fehlgeschlagen: {}", name, e),
                    theme::red(),
                ),
            }
        }
        pb::Payload::Prune { id, dry, report } => on_prune(app, id, dry, report),
        pb::Payload::Task { id, update } => on_task(app, id, update),
        pb::Payload::Gc { store, result } => {
            if let Ok(g) = result {
                app.pve.p2.gc.insert((server, store), g);
            }
        }
    }
}

fn on_task(app: &mut App, id: u64, update: TaskUpdate) {
    let Some(j) = app.pve.p2.jobs.list.iter_mut().find(|j| j.id == id) else {
        return;
    };
    match update {
        TaskUpdate::Started(upid) => j.upid = Some(upid),
        TaskUpdate::Progress(line) => {
            if j.lines.last() != Some(&line) {
                j.lines.push(line);
                if j.lines.len() > MAX_LOG {
                    j.lines.remove(0);
                }
            }
        }
        TaskUpdate::Done(res) => {
            j.done = Some(res.clone());
            let (title, server, store, gc) = (j.title.clone(), j.server, j.store.clone(), j.is_gc);
            match res {
                Ok(s) => app.toast(t!("{}: {}", title, s), theme::green()),
                Err(e) => app.toast(t!("{} fehlgeschlagen: {}", title, e), theme::red()),
            }
            // New verification states / freed space.
            reload_snapshots(app);
            if gc && let Some(cx) = ctx_for(app, server) {
                pb::spawn_gc_status(&app.rt, cx, store);
            }
        }
    }
}

fn reload_snapshots(app: &mut App) {
    if let Some(v) = app.pve.cluster.pbs.snaps.as_mut() {
        v.list = None;
    }
    cpbs::load_snapshots(app);
}

// ------------------------------------------------------------------ browser

pub struct Browser {
    pub server: Uuid,
    pub store: String,
    pub snap: SnapRef,
    title: String,
    /// `(name, base64 path)` from the archive down; empty = list of archives.
    crumbs: Vec<(String, String)>,
    cache: HashMap<String, Result<Vec<Entry>, String>>,
    loading: Option<String>,
    filter: String,
    dir: String,
    cap: usize,
    tar: bool,
}

impl Browser {
    fn path(&self) -> String {
        self.crumbs
            .last()
            .map(|c| c.1.clone())
            .unwrap_or_else(|| pb::ROOT.to_string())
    }
}

fn default_dir() -> String {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

pub fn open_browser(app: &mut App, server: Uuid, store: String, snap: SnapRef) {
    if ctx_for(app, server).is_none() {
        return;
    }
    let title = format!(
        "{} · {} · {}/{} · {}",
        super::pbs_label(app, server),
        store,
        snap.btype,
        snap.bid,
        fmt_time(snap.time)
    );
    app.pve.p2.browser = Some(Browser {
        server,
        store,
        snap,
        title,
        crumbs: Vec::new(),
        cache: HashMap::new(),
        loading: None,
        filter: String::new(),
        dir: default_dir(),
        cap: 1,
        tar: false,
    });
    load_current(app);
}

fn load_current(app: &mut App) {
    let Some(b) = app.pve.p2.browser.as_mut() else {
        return;
    };
    let path = b.path();
    if b.cache.contains_key(&path) || b.loading.as_deref() == Some(path.as_str()) {
        return;
    }
    b.loading = Some(path.clone());
    let (server, store, snap) = (b.server, b.store.clone(), b.snap.clone());
    if let Some(cx) = ctx_for(app, server) {
        pb::spawn_browse(&app.rt, cx, store, snap, path);
    }
}

fn start_download(app: &mut App, entry: Entry) {
    let Some(b) = app.pve.p2.browser.as_ref() else {
        return;
    };
    let (server, store, snap, tar) = (b.server, b.store.clone(), b.snap.clone(), b.tar);
    let cap = pb::CAPS[b.cap.min(pb::CAPS.len() - 1)];
    let dir = PathBuf::from(b.dir.trim());
    if !dir.is_dir() {
        app.toast(
            t!("Zielordner „{}“ existiert nicht", dir.display()),
            theme::red(),
        );
        return;
    }
    let Some(cx) = ctx_for(app, server) else {
        return;
    };
    let id = app.pve.p2.next_id();
    let cancel = Arc::new(AtomicBool::new(false));
    app.pve.p2.jobs.downloads.push(Download {
        id,
        name: pb::local_name(&entry, tar),
        done: 0,
        total: entry.size,
        state: DlState::Running,
        cancel: cancel.clone(),
    });
    pb::spawn_download(&app.rt, cx, id, store, snap, entry, tar, dir, cap, cancel);
}

fn breadcrumb(ui: &mut egui::Ui, b: &mut Browser) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if theme::btn(ui, theme::Btn::Link, None, t!("Snapshot")).clicked() && !b.crumbs.is_empty()
        {
            b.crumbs.clear();
            changed = true;
            return;
        }
        let n = b.crumbs.len();
        for i in 0..n {
            ui.label(RichText::new("›").color(theme::text_dim()));
            let name = b.crumbs[i].0.clone();
            if theme::btn(ui, theme::Btn::Link, None, &name).clicked() && i + 1 < n {
                b.crumbs.truncate(i + 1);
                changed = true;
                break;
            }
        }
    });
    changed
}

fn windows_browser(app: &mut App, ctx: &egui::Context) {
    if app.pve.p2.browser.is_none() {
        return;
    }
    let mut download: Option<Entry> = None;
    let mut open_dir: Option<Entry> = None;
    let mut crumbs_changed = false;
    let mut cancel_dl: Option<u64> = None;
    let jobs_dl: Vec<(u64, String, u64, Option<u64>, DlState)> = app
        .pve
        .p2
        .jobs
        .downloads
        .iter()
        .map(|d| (d.id, d.name.clone(), d.done, d.total, d.state.clone()))
        .collect();
    let Some(b) = app.pve.p2.browser.as_mut() else {
        return;
    };
    let title = b.title.clone();
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("p2-pbs-browser"),
        900.0,
        &title,
        |ui| {
            crumbs_changed = breadcrumb(ui, b);
            let path = b.path();
            ui.horizontal(|ui| {
                theme::search_field(
                    ui,
                    &mut b.filter,
                    t!("In diesem Ordner filtern"),
                    240.0,
                    egui::Id::new("p2-pbs-filter"),
                );
                if b.loading.is_some() {
                    ui.add(egui::Spinner::new().size(14.0));
                }
            });
            match b.cache.get(&path) {
                None => {
                    if b.loading.is_none() {
                        ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                Some(Ok(list)) => {
                    let f = b.filter.trim().to_lowercase();
                    let rows: Vec<&Entry> = list
                        .iter()
                        .filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f))
                        .collect();
                    if rows.is_empty() {
                        ui.label(RichText::new(t!("Leer")).color(theme::text_dim()));
                    }
                    egui::Frame::NONE
                        .fill(theme::group_fill())
                        .corner_radius(CornerRadius::same(12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (i, e) in rows.iter().take(MAX_ROWS).enumerate() {
                                entry_row(ui, e, i > 0, &mut open_dir, &mut download);
                            }
                        });
                    if rows.len() > MAX_ROWS {
                        ui.label(
                            RichText::new(t!(
                                "… {} weitere Einträge – Filter benutzen",
                                rows.len() - MAX_ROWS
                            ))
                            .size(12.5)
                            .color(theme::text_dim()),
                        );
                    }
                }
            }
            ui.add_space(6.0);
            theme::form_caption(ui, t!("Herunterladen"));
            theme::form(ui, 130.0, |ui, f| {
                f.field(ui, t!("Zielordner"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut b.dir)
                            .desired_width(ui.available_width() - 14.0),
                    );
                });
                f.field(ui, t!("Größenlimit"), |ui| {
                    egui::ComboBox::from_id_salt("p2-pbs-cap")
                        .selected_text(fmt_bytes(pb::CAPS[b.cap.min(pb::CAPS.len() - 1)]))
                        .show_ui(ui, |ui| {
                            for (i, c) in pb::CAPS.iter().enumerate() {
                                ui.selectable_value(&mut b.cap, i, fmt_bytes(*c));
                            }
                        });
                });
                f.field(ui, t!("Ordner als"), |ui| {
                    if let Some(i) = theme::segmented(ui, &["ZIP", "TAR (zstd)"], b.tar as usize) {
                        b.tar = i == 1;
                    }
                });
            });
            ui.label(
                RichText::new(t!(
                    "Einzelne Dateien kommen unverändert, Ordner als Archiv. Es wird nie eine vorhandene Datei überschrieben."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            for (id, name, done, total, st) in &jobs_dl {
                ui.add_space(4.0);
                download_row(ui, *id, name, *done, *total, st, &mut cancel_dl);
            }
        },
        |ui, _| theme::btn(ui, theme::Btn::Primary, None, t!("Schließen")).clicked(),
    );
    if let Some(id) = cancel_dl
        && let Some(d) = app.pve.p2.jobs.downloads.iter().find(|d| d.id == id)
    {
        d.cancel.store(true, Ordering::Relaxed);
    }
    if let Some(e) = open_dir
        && let Some(b) = app.pve.p2.browser.as_mut()
    {
        b.crumbs.push((e.name.clone(), e.path.clone()));
        b.filter.clear();
        crumbs_changed = true;
    }
    if crumbs_changed {
        load_current(app);
    }
    if let Some(e) = download {
        start_download(app, e);
    }
    if close || dismissed {
        app.pve.p2.browser = None;
    }
}

fn download_row(
    ui: &mut egui::Ui,
    id: u64,
    name: &str,
    done: u64,
    total: Option<u64>,
    st: &DlState,
    cancel: &mut Option<u64>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        match st {
            DlState::Running => {
                ui.add(egui::Spinner::new().size(14.0));
            }
            DlState::Done(..) => {
                ui.label(RichText::new("✔").color(theme::green()));
            }
            DlState::Failed(_) => {
                ui.label(RichText::new("✖").color(theme::red()));
            }
        }
        ui.add(egui::Label::new(RichText::new(name).font(theme::medium(13.0))).truncate());
        let text = match (st, total) {
            (DlState::Running, Some(t)) if t > 0 => {
                format!("{} / {}", fmt_bytes(done), fmt_bytes(t))
            }
            (DlState::Running, _) => fmt_bytes(done),
            (DlState::Done(p, n), _) => format!("{} → {}", fmt_bytes(*n), p.display()),
            (DlState::Failed(e), _) => e.clone(),
        };
        let color = if matches!(st, DlState::Failed(_)) {
            theme::red()
        } else {
            theme::text_dim()
        };
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).truncate());
        if *st == DlState::Running {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::btn(ui, theme::Btn::Link, None, t!("Abbrechen")).clicked() {
                    *cancel = Some(id);
                }
            });
        }
    });
    if let (DlState::Running, Some(t)) = (st, total)
        && t > 0
    {
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
        ui.painter()
            .rect_filled(r, CornerRadius::same(3), theme::fill());
        let frac = (done as f32 / t as f32).clamp(0.0, 1.0);
        ui.painter().rect_filled(
            Rect::from_min_size(r.min, Vec2::new(r.width() * frac, r.height())),
            CornerRadius::same(3),
            theme::accent(),
        );
    }
}

fn entry_row(
    ui: &mut egui::Ui,
    e: &Entry,
    divider: bool,
    open: &mut Option<Entry>,
    download: &mut Option<Entry>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    let p = ui.painter();
    if divider {
        p.line_segment(
            [rect.left_top() + Vec2::new(40.0, 0.0), rect.right_top()],
            Stroke::new(1.0, theme::line()),
        );
    }
    if resp.hovered() && e.is_dir() {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    let cy = rect.center().y;
    let icon = if e.is_dir() { Icon::Folder } else { Icon::File };
    icons::paint(
        p,
        Rect::from_center_size(Pos2::new(rect.left() + 20.0, cy), Vec2::splat(15.0)),
        icon,
        if e.is_dir() {
            theme::accent()
        } else {
            theme::text_dim()
        },
    );
    // Right side: download link, mtime, size.
    let mut right = rect.right() - 12.0;
    let dl_w = 96.0;
    let dl = Rect::from_min_max(
        Pos2::new(right - dl_w, rect.top() + 3.0),
        Pos2::new(right, rect.bottom() - 3.0),
    );
    if e.downloadable() {
        let r = ui.interact(dl, ui.id().with(("p2-dl", &e.path)), Sense::click());
        let ink = if r.hovered() {
            theme::text()
        } else {
            theme::accent()
        };
        icons::paint(
            p,
            Rect::from_center_size(Pos2::new(dl.left() + 12.0, cy), Vec2::splat(14.0)),
            Icon::Download,
            ink,
        );
        p.text(
            Pos2::new(dl.left() + 26.0, cy),
            egui::Align2::LEFT_CENTER,
            t!("Laden"),
            theme::medium(13.0),
            ink,
        );
        if r.on_hover_text(if e.is_dir() {
            t!("Ordner als Archiv herunterladen")
        } else {
            t!("Datei herunterladen")
        })
        .clicked()
        {
            *download = Some(e.clone());
        }
    }
    right -= dl_w + 6.0;
    let wide = rect.width() > 560.0;
    if wide {
        if let Some(m) = e.mtime {
            p.text(
                Pos2::new(right, cy),
                egui::Align2::RIGHT_CENTER,
                fmt_time(m),
                theme::medium(12.5),
                theme::text_dim(),
            );
        }
        right -= 130.0;
        if let Some(s) = e.size.filter(|_| !e.is_dir()) {
            p.text(
                Pos2::new(right, cy),
                egui::Align2::RIGHT_CENTER,
                fmt_bytes(s),
                theme::medium(12.5),
                theme::text_dim(),
            );
        }
        right -= 80.0;
    }
    let name_w = (right - (rect.left() + 38.0)).max(40.0);
    let g = theme::truncated(p, &e.name, theme::medium(14.0), theme::text(), name_w);
    p.galley(
        Pos2::new(rect.left() + 38.0, cy - g.size().y / 2.0),
        g,
        theme::text(),
    );
    if !e.downloadable() {
        let kind = match e.kind {
            'l' => t!("Link"),
            'h' => t!("Hardlink"),
            _ => t!("Spezialdatei"),
        };
        p.text(
            Pos2::new(dl.right(), cy),
            egui::Align2::RIGHT_CENTER,
            kind,
            theme::medium(12.0),
            theme::text_dim(),
        );
    }
    if e.is_dir()
        && resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    {
        *open = Some(e.clone());
    }
}

// ------------------------------------------------------------------ prune

pub struct PruneSheet {
    pub server: Uuid,
    pub store: String,
    title: String,
    groups: Vec<(String, String)>,
    snapshots: usize,
    keep: Keep,
    preview: Option<(Keep, Result<PruneReport, String>)>,
    /// Job id of the running dry run / prune.
    dry: Option<u64>,
    run: Option<u64>,
    confirm: bool,
    result: Option<Result<PruneReport, String>>,
}

pub fn open_prune(app: &mut App, server: Uuid, store: String, group: Option<(String, String)>) {
    let Some(v) = app
        .pve
        .cluster
        .pbs
        .snaps
        .as_ref()
        .filter(|v| v.server == server && v.store == store)
    else {
        return;
    };
    let Some(Ok(list)) = v.list.as_ref() else {
        return;
    };
    let (groups, snapshots) = match &group {
        Some(g) => (
            vec![g.clone()],
            list.iter()
                .filter(|s| (s.btype.as_str(), s.bid.as_str()) == (g.0.as_str(), g.1.as_str()))
                .count(),
        ),
        None => (pb::group_keys(list), list.len()),
    };
    let keep = app
        .pve
        .cluster
        .pbs
        .live
        .get(&server)
        .and_then(|l| l.keep.get(&store).copied())
        .unwrap_or_default();
    let what = match &group {
        Some((t, i)) => format!("{t}/{i}"),
        None => t!("ganzer Datastore").to_string(),
    };
    let title = format!(
        "{} · {} · {} {}",
        super::pbs_label(app, server),
        store,
        t!("Prune"),
        what
    );
    app.pve.p2.prune = Some(PruneSheet {
        server,
        store,
        title,
        groups,
        snapshots,
        keep,
        preview: None,
        dry: None,
        run: None,
        confirm: false,
        result: None,
    });
}

fn start_prune(app: &mut App, dry: bool) {
    let id = app.pve.p2.next_id();
    let Some(p) = app.pve.p2.prune.as_mut() else {
        return;
    };
    let (server, store, groups, keep) = (p.server, p.store.clone(), p.groups.clone(), p.keep);
    if dry {
        p.dry = Some(id);
        p.preview = None;
    } else {
        p.run = Some(id);
        p.confirm = false;
    }
    if let Some(cx) = ctx_for(app, server) {
        pb::spawn_prune(&app.rt, cx, id, store, groups, keep, dry);
    }
}

fn on_prune(app: &mut App, id: u64, dry: bool, report: Result<PruneReport, Error>) {
    let report = report.map_err(|e| e.to_string());
    let Some(p) = app.pve.p2.prune.as_mut() else {
        return;
    };
    if dry && p.dry == Some(id) {
        p.dry = None;
        p.preview = Some((p.keep, report));
    } else if !dry && p.run == Some(id) {
        p.run = None;
        if let Ok(r) = &report {
            let n = r.removes();
            app.toast(t!("Prune: {} Snapshots gelöscht", n), theme::green());
        }
        if let Some(p) = app.pve.p2.prune.as_mut() {
            p.result = Some(report);
            p.preview = None;
        }
        reload_snapshots(app);
    }
}

fn report_view(ui: &mut egui::Ui, r: &PruneReport, past: bool) {
    let removes = r.removes();
    let color = if removes > 0 {
        theme::amber()
    } else {
        theme::green()
    };
    let text = if past {
        t!(
            "{} Snapshots gelöscht, {} behalten, {} Gruppen betroffen",
            removes,
            r.keeps(),
            r.affected_groups()
        )
    } else {
        t!(
            "{} Snapshots würden gelöscht, {} bleiben, {} Gruppen betroffen",
            removes,
            r.keeps(),
            r.affected_groups()
        )
    };
    ui.label(RichText::new(text).font(theme::semibold(14.0)).color(color));
    if r.failed() > 0 {
        ui.label(RichText::new(t!("{} Gruppen mit Fehler", r.failed())).color(theme::red()));
    }
    egui::Frame::NONE
        .fill(theme::group_fill())
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for ((t, i), res) in r.groups.iter().take(MAX_ROWS) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{t}/{i}")).font(theme::semibold(13.0)));
                    match res {
                        Err(e) => {
                            ui.label(RichText::new(e).color(theme::red()));
                        }
                        Ok(list) => {
                            let rm = list.iter().filter(|x| !x.keep).count();
                            ui.label(
                                RichText::new(t!("{} behalten · {} löschen", list.len() - rm, rm))
                                    .size(12.5)
                                    .color(theme::text_dim()),
                            );
                            for s in list.iter().filter(|x| !x.keep).take(12) {
                                theme::tag(ui, &fmt_time(s.time), Some(theme::red()));
                            }
                            if rm > 12 {
                                ui.label(
                                    RichText::new(format!("+{}", rm - 12)).color(theme::text_dim()),
                                );
                            }
                        }
                    }
                });
            }
        });
}

fn windows_prune(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.pve.p2.prune.as_mut() else {
        return;
    };
    let do_preview = std::cell::Cell::new(false);
    let do_run = std::cell::Cell::new(false);
    let want_confirm = std::cell::Cell::new(false);
    let title = p.title.clone();
    let busy = p.dry.is_some() || p.run.is_some();
    let can_run = matches!(&p.preview, Some((k, Ok(r))) if *k == p.keep && r.removes() > 0)
        && !busy
        && !p.confirm;
    let keep_ok = pb::keep_form(&p.keep).is_some();
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("p2-pbs-prune"),
        760.0,
        &title,
        |ui| {
            ui.label(
                RichText::new(t!(
                    "{} Gruppen, {} Snapshots. Erst die Vorschau vom Server (nichts wird gelöscht), dann bestätigt ausführen.",
                    p.groups.len(),
                    p.snapshots
                ))
                .color(theme::text_dim()),
            );
            let before = p.keep;
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, v) in [
                        ("keep-last", &mut p.keep.last),
                        ("hourly", &mut p.keep.hourly),
                        ("daily", &mut p.keep.daily),
                        ("weekly", &mut p.keep.weekly),
                        ("monthly", &mut p.keep.monthly),
                        ("yearly", &mut p.keep.yearly),
                    ] {
                        ui.label(RichText::new(label).size(12.5).color(theme::text_dim()));
                        ui.add(egui::DragValue::new(v).range(0..=999));
                        ui.add_space(6.0);
                    }
                });
            });
            if p.keep != before {
                p.confirm = false;
            }
            if pb::keep_form(&p.keep).is_none() {
                ui.label(
                    RichText::new(t!("Mindestens eine keep-Regel muss größer als 0 sein."))
                        .color(theme::amber()),
                );
            }
            if p.dry.is_some() {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(
                        RichText::new(t!("Vorschau wird berechnet…")).color(theme::text_dim()),
                    );
                });
            }
            match &p.preview {
                Some((k, Ok(r))) => {
                    report_view(ui, r, false);
                    if *k != p.keep {
                        ui.label(
                            RichText::new(t!(
                                "Die Optionen haben sich geändert – Vorschau neu berechnen."
                            ))
                            .color(theme::amber()),
                        );
                    }
                }
                Some((_, Err(e))) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                None => {}
            }
            if p.run.is_some() {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(t!("Prune läuft…")).color(theme::text_dim()));
                });
            }
            if let Some(res) = &p.result {
                match res {
                    Ok(r) => report_view(ui, r, true),
                    Err(e) => {
                        ui.label(RichText::new(e).color(theme::red()));
                    }
                }
            }
            if p.confirm
                && let Some((_, Ok(r))) = &p.preview
            {
                egui::Frame::NONE
                    .fill(theme::soft(theme::red()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(t!(
                                "{} Snapshots in {} Gruppen von „{}“ endgültig löschen? Das lässt sich nicht rückgängig machen.",
                                r.removes(),
                                r.affected_groups(),
                                p.store
                            ))
                            .font(theme::semibold(13.5)),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if theme::btn(ui, theme::Btn::Primary, None, t!("Ja, jetzt löschen"))
                                .clicked()
                            {
                                do_run.set(true);
                            }
                            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                                p.confirm = false;
                            }
                        });
                    });
            }
        },
        |ui, _| {
            let close = theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked();
            ui.add_enabled_ui(can_run, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Prune ausführen…")).clicked() {
                    want_confirm.set(true);
                }
            });
            ui.add_enabled_ui(!busy && keep_ok, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Vorschau vom Server")).clicked() {
                    do_preview.set(true);
                }
            });
            close
        },
    );
    if want_confirm.get()
        && let Some(p) = app.pve.p2.prune.as_mut()
    {
        p.confirm = true;
    }
    if do_preview.get() {
        start_prune(app, true);
    }
    if do_run.get() {
        start_prune(app, false);
    }
    if close || dismissed {
        app.pve.p2.prune = None;
    }
}

// ------------------------------------------------------------------ verify / gc

#[derive(Clone, Debug)]
pub enum Run {
    Verify(Uuid, String, VerifyScope),
    Gc(Uuid, String),
}

impl Run {
    pub fn server(&self) -> Uuid {
        match self {
            Run::Verify(s, ..) | Run::Gc(s, _) => *s,
        }
    }
}

pub struct Confirm {
    pub run: Run,
    text: String,
    reverify: bool,
    can_reverify: bool,
}

fn counts(app: &App, server: Uuid, store: &str) -> Option<Vec<PbsSnapshot>> {
    app.pve
        .cluster
        .pbs
        .snaps
        .as_ref()
        .filter(|v| v.server == server && v.store == store)
        .and_then(|v| v.list.as_ref())
        .and_then(|l| l.as_ref().ok())
        .cloned()
}

pub fn ask_verify(app: &mut App, server: Uuid, store: String, scope: VerifyScope) {
    let list = counts(app, server, &store).unwrap_or_default();
    let name = super::pbs_label(app, server);
    let (text, can_reverify) = match &scope {
        VerifyScope::Store => {
            let g = pb::group_keys(&list).len();
            (
                t!(
                    "Alle Snapshots von „{}“ prüfen ({} Snapshots in {} Gruppen, Server {})? Das liest viele Daten und kann lange dauern.",
                    store,
                    list.len(),
                    g,
                    name
                ),
                true,
            )
        }
        VerifyScope::Group(t, i) => {
            let n = list
                .iter()
                .filter(|s| (s.btype.as_str(), s.bid.as_str()) == (t.as_str(), i.as_str()))
                .count();
            (
                t!(
                    "Gruppe {}/{} prüfen ({} Snapshots, Server {})?",
                    t,
                    i,
                    n,
                    name
                ),
                true,
            )
        }
        VerifyScope::Snapshot(s) => (
            t!(
                "Snapshot {}/{} vom {} prüfen (Server {})?",
                s.btype,
                s.bid,
                fmt_time(s.time),
                name
            ),
            false,
        ),
    };
    app.pve.p2.confirm = Some(Confirm {
        run: Run::Verify(server, store, scope),
        text,
        reverify: false,
        can_reverify,
    });
}

pub fn refresh_gc(app: &mut App, server: Uuid, store: String) {
    if !app.pve.p2.gc_asked.insert((server, store.clone())) {
        return;
    }
    if let Some(cx) = ctx_for(app, server) {
        pb::spawn_gc_status(&app.rt, cx, store);
    }
}

pub fn ask_gc(app: &mut App, server: Uuid, store: String) {
    let name = super::pbs_label(app, server);
    app.pve.p2.confirm = Some(Confirm {
        text: t!(
            "Garbage Collection auf „{}“ (Server {}) starten? Nicht mehr referenzierte Chunks werden gelöscht; das dauert und belastet den Speicher.",
            store,
            name
        ),
        run: Run::Gc(server, store),
        reverify: false,
        can_reverify: false,
    });
}

fn start_job(app: &mut App, server: Uuid, store: String, what: Start, title: String) {
    let Some(cx) = ctx_for(app, server) else {
        return;
    };
    let id = app.pve.p2.next_id();
    let cancel = Arc::new(AtomicBool::new(false));
    let is_gc = what == Start::Gc;
    let server_name = super::pbs_label(app, server);
    app.pve.p2.jobs.list.push(Job {
        id,
        server,
        server_name,
        store: store.clone(),
        title,
        upid: None,
        lines: Vec::new(),
        done: None,
        cancel: cancel.clone(),
        started: Instant::now(),
        show_log: false,
        is_gc,
    });
    app.pve.p2.jobs.open = true;
    pb::spawn_task(&app.rt, cx, id, store, what, cancel);
}

fn windows_confirm(app: &mut App, ctx: &egui::Context) {
    let Some(c) = app.pve.p2.confirm.as_mut() else {
        return;
    };
    let mut go = false;
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("p2-pbs-confirm"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0f32.min(ctx.content_rect().width() - 60.0));
            ui.label(
                RichText::new(match c.run {
                    Run::Verify(..) => t!("Verify starten?"),
                    Run::Gc(..) => t!("Garbage Collection starten?"),
                })
                .font(theme::bold(18.0)),
            );
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(&c.text)).wrap());
            if c.can_reverify {
                ui.add_space(8.0);
                ui.checkbox(
                    &mut c.reverify,
                    t!("Auch kürzlich verifizierte Snapshots erneut prüfen"),
                );
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Starten")).clicked() {
                    go = true;
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if go {
        let Some(c) = app.pve.p2.confirm.take() else {
            return;
        };
        match c.run {
            Run::Verify(server, store, scope) => {
                let title = match &scope {
                    VerifyScope::Store => t!("Verify {}", store),
                    VerifyScope::Group(t, i) => t!("Verify {}/{}", t, i),
                    VerifyScope::Snapshot(s) => t!("Verify {}/{}", s.btype, s.bid),
                };
                start_job(app, server, store, Start::Verify(scope, c.reverify), title);
            }
            Run::Gc(server, store) => {
                let title = t!("Garbage Collection {}", store);
                start_job(app, server, store, Start::Gc, title);
            }
        }
    } else if cancel {
        app.pve.p2.confirm = None;
    }
}

// ------------------------------------------------------------------ jobs

fn windows_jobs(app: &mut App, ctx: &egui::Context) {
    if !app.pve.p2.jobs.open {
        return;
    }
    let mut stop: Option<u64> = None;
    let mut remove: Option<u64> = None;
    let jobs = &mut app.pve.p2.jobs;
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("p2-pbs-jobs"),
        720.0,
        t!("Backup-Server-Aufgaben"),
        |ui| {
            if jobs.list.is_empty() {
                ui.label(RichText::new(t!("Keine Aufgaben")).color(theme::text_dim()));
            }
            for j in jobs.list.iter_mut().rev() {
                egui::Frame::NONE
                    .fill(theme::group_fill())
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            match &j.done {
                                None => {
                                    ui.add(egui::Spinner::new().size(14.0));
                                }
                                Some(Ok(_)) => {
                                    ui.label(RichText::new("✔").color(theme::green()));
                                }
                                Some(Err(_)) => {
                                    ui.label(RichText::new("✖").color(theme::red()));
                                }
                            }
                            ui.label(RichText::new(&j.title).font(theme::semibold(14.0)));
                            ui.label(
                                RichText::new(&j.server_name)
                                    .size(12.5)
                                    .color(theme::text_dim()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if j.done.is_none() {
                                        if theme::btn(ui, theme::Btn::Small, None, t!("Stoppen"))
                                            .clicked()
                                        {
                                            stop = Some(j.id);
                                        }
                                    } else if theme::btn(
                                        ui,
                                        theme::Btn::Link,
                                        None,
                                        t!("Entfernen"),
                                    )
                                    .clicked()
                                    {
                                        remove = Some(j.id);
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "{} s",
                                            j.started.elapsed().as_secs()
                                        ))
                                        .size(12.5)
                                        .color(theme::text_dim()),
                                    );
                                },
                            );
                        });
                        let last = match &j.done {
                            Some(Ok(s)) => s.clone(),
                            Some(Err(e)) => e.clone(),
                            None => j
                                .lines
                                .last()
                                .cloned()
                                .unwrap_or_else(|| t!("startet…").to_string()),
                        };
                        let color = if matches!(j.done, Some(Err(_))) {
                            theme::red()
                        } else {
                            theme::text_dim()
                        };
                        ui.add(
                            egui::Label::new(RichText::new(last).size(12.5).color(color))
                                .truncate(),
                        );
                        if !j.lines.is_empty() {
                            let label = if j.show_log {
                                t!("Log ausblenden")
                            } else {
                                t!("Log anzeigen")
                            };
                            if theme::btn(ui, theme::Btn::Link, None, label).clicked() {
                                j.show_log = !j.show_log;
                            }
                            if j.show_log {
                                egui::ScrollArea::vertical()
                                    .id_salt(("p2-job-log", j.id))
                                    .max_height(200.0)
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        for l in &j.lines {
                                            ui.label(theme::mono(l, 11.5).color(theme::text_dim()));
                                        }
                                    });
                            }
                        }
                    });
                ui.add_space(6.0);
            }
        },
        |ui, _| theme::btn(ui, theme::Btn::Primary, None, t!("Schließen")).clicked(),
    );
    if let Some(id) = stop
        && let Some(j) = app.pve.p2.jobs.list.iter().find(|j| j.id == id)
        && let Some(upid) = j.upid.clone()
        && let Some(cx) = ctx_for(app, j.server)
    {
        pb::spawn_stop(&app.rt, cx, upid);
    }
    if let Some(id) = remove {
        app.pve.p2.jobs.list.retain(|j| j.id != id);
    }
    if close || dismissed {
        app.pve.p2.jobs.open = false;
    }
}

pub fn windows(app: &mut App, ctx: &egui::Context) {
    windows_browser(app, ctx);
    windows_prune(app, ctx);
    windows_confirm(app, ctx);
    windows_jobs(app, ctx);
}

// ------------------------------------------------------------------ hooks in the snapshot sheet

/// Buttons of the datastore: verify, garbage collection, prune, tasks.
pub fn store_bar(
    ui: &mut egui::Ui,
    st: &super::State,
    server: Uuid,
    store: &str,
    acts: &mut Vec<Act>,
) {
    let key = (server, store.to_string());
    if !st.gc.contains_key(&key) && !st.gc_asked.contains(&key) {
        acts.push(Act::GcStatus(server, store.to_string()));
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if theme::btn(ui, theme::Btn::Small, None, t!("Prune…")).clicked() {
            acts.push(Act::Prune(server, store.to_string(), None));
        }
        if theme::btn(ui, theme::Btn::Small, None, t!("Verify…")).clicked() {
            acts.push(Act::Verify(server, store.to_string(), VerifyScope::Store));
        }
        if theme::btn(ui, theme::Btn::Small, None, t!("Garbage Collection…")).clicked() {
            acts.push(Act::Gc(server, store.to_string()));
        }
        let running = st.jobs.running();
        let label = if running > 0 {
            t!("Aufgaben ({})", running)
        } else {
            t!("Aufgaben").to_string()
        };
        if theme::btn(ui, theme::Btn::Link, None, &label).clicked() {
            acts.push(Act::Jobs);
        }
        if let Some(g) = st.gc.get(&(server, store.to_string())) {
            let mut parts = vec![t!(
                "GC: {}",
                g.last_state.clone().unwrap_or_else(|| "–".into())
            )];
            if let Some(b) = g.removed_bytes {
                parts.push(t!("{} freigegeben", fmt_bytes(b)));
            }
            if let Some(b) = g.pending_bytes.filter(|b| *b > 0) {
                parts.push(t!("{} ausstehend", fmt_bytes(b)));
            }
            ui.label(
                RichText::new(parts.join(" · "))
                    .size(12.5)
                    .color(theme::text_dim()),
            );
        }
    });
}

/// Prune / verify of one group (next to the group name).
pub fn group_buttons(
    ui: &mut egui::Ui,
    server: Uuid,
    store: &str,
    btype: &str,
    bid: &str,
    acts: &mut Vec<Act>,
) {
    if theme::btn(ui, theme::Btn::Link, None, t!("Prune…")).clicked() {
        acts.push(Act::Prune(
            server,
            store.to_string(),
            Some((btype.to_string(), bid.to_string())),
        ));
    }
    if theme::btn(ui, theme::Btn::Link, None, t!("Verify…")).clicked() {
        acts.push(Act::Verify(
            server,
            store.to_string(),
            VerifyScope::Group(btype.to_string(), bid.to_string()),
        ));
    }
}

/// Browse / verify of one snapshot (right side of its row).
pub fn snapshot_buttons(
    ui: &mut egui::Ui,
    server: Uuid,
    store: &str,
    s: &PbsSnapshot,
    acts: &mut Vec<Act>,
) {
    let snap = SnapRef::of(s);
    if theme::btn(ui, theme::Btn::Link, None, t!("Verify…")).clicked() {
        acts.push(Act::Verify(
            server,
            store.to_string(),
            VerifyScope::Snapshot(snap.clone()),
        ));
    }
    let has_archive = s
        .files
        .iter()
        .any(|f| f.name.ends_with(".pxar.didx") || f.name.ends_with(".ppxar.didx"));
    if has_archive
        && theme::btn(ui, theme::Btn::Link, Some(Icon::Folder), t!("Dateien…"))
            .on_hover_text(t!(
                "Inhalt der .pxar-Archive durchsuchen und Dateien herunterladen"
            ))
            .clicked()
    {
        acts.push(Act::Browse(server, store.to_string(), snap));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1536), "1.5 KiB");
        assert_eq!(fmt_bytes(150 * 1024 * 1024), "150 MiB");
    }
}
