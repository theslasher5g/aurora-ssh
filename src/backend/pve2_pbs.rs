//! Proxmox Backup Server extras: browse the catalog of a snapshot's `.pxar` archives,
//! download a single file or a folder (`pxar-file-download`), prune a group or a whole
//! datastore (dry run first), verify and garbage collection, and follow the resulting
//! tasks. Based on the documented PBS API (`/admin/datastore/{store}/catalog`,
//! `pxar-file-download`, `prune`, `verify`, `gc`, `/nodes/localhost/tasks/{upid}`);
//! only exercised against a mock so far.
//!
//! Every name that goes into a URL is validated first, file paths travel as the
//! base64 string the server gave us, local file names go through `safe_local_name`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use super::super::cluster::pbs::{
    Keep, PbsSnapshot, valid_backup_id, valid_backup_type, valid_datastore,
};
use super::super::{
    Client, Conn, Epoch, Error, Method, Session, Sink, TaskUpdate, enc, num, task_result,
};

/// `base64("/")`: the root of a snapshot (lists its archives).
pub const ROOT: &str = "Lw==";

/// Default cap of a single download.
pub const DEFAULT_CAP: u64 = 2 * 1024 * 1024 * 1024;
/// Caps the UI offers (bytes).
pub const CAPS: [u64; 4] = [
    100 * 1024 * 1024,
    DEFAULT_CAP,
    10 * 1024 * 1024 * 1024,
    50 * 1024 * 1024 * 1024,
];

// ------------------------------------------------------------------ snapshots and paths

/// A snapshot as the API addresses it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SnapRef {
    pub btype: String,
    pub bid: String,
    pub time: i64,
}

impl SnapRef {
    pub fn of(s: &PbsSnapshot) -> Self {
        Self {
            btype: s.btype.clone(),
            bid: s.bid.clone(),
            time: s.time,
        }
    }

    pub fn valid(&self) -> bool {
        valid_backup_type(&self.btype) && valid_backup_id(&self.bid) && self.time > 0
    }

    fn query(&self) -> Option<String> {
        self.valid().then(|| {
            format!(
                "backup-type={}&backup-id={}&backup-time={}",
                enc(&self.btype),
                enc(&self.bid),
                self.time
            )
        })
    }
}

#[cfg(test)]
/// Standard base64 of a path (`/root.pxar.didx/etc` → what the API wants).
pub fn b64(path: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(path.as_bytes())
}

/// A base64 path from the server (or from [`b64`]): plain alphabet, bounded.
pub fn valid_b64_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 8192
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
}

/// `GET …/catalog?…&filepath=` for the content of a directory.
pub fn catalog_path(store: &str, snap: &SnapRef, filepath: &str) -> Option<String> {
    (valid_datastore(store) && valid_b64_path(filepath)).then(|| {
        Some(format!(
            "/admin/datastore/{store}/catalog?{}&filepath={}",
            snap.query()?,
            enc(filepath)
        ))
    })?
}

/// `GET …/pxar-file-download?…&filepath=` (a folder comes back as zip, or as tar with
/// `tar=1`).
pub fn download_path(store: &str, snap: &SnapRef, filepath: &str, tar: bool) -> Option<String> {
    (valid_datastore(store) && valid_b64_path(filepath) && filepath != ROOT).then(|| {
        Some(format!(
            "/admin/datastore/{store}/pxar-file-download?{}&filepath={}{}",
            snap.query()?,
            enc(filepath),
            if tar { "&tar=1" } else { "" }
        ))
    })?
}

/// One line of a catalog listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// Base64 path for the next call (as given by the server).
    pub path: String,
    /// `d` directory, `f` file, `l` symlink, `h` hardlink, others: devices, fifos.
    pub kind: char,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == 'd'
    }

    /// Files and folders can be downloaded; links and devices cannot.
    pub fn downloadable(&self) -> bool {
        matches!(self.kind, 'd' | 'f')
    }
}

/// `data` of the catalog call: directories first, then by name.
pub fn parse_catalog(data: &Value) -> Vec<Entry> {
    let mut out: Vec<Entry> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let name = e["text"].as_str()?.to_string();
            let path = e["filepath"].as_str()?.to_string();
            if name.is_empty() || !valid_b64_path(&path) {
                return None;
            }
            Some(Entry {
                name,
                path,
                kind: e["type"].as_str()?.chars().next()?,
                size: e["size"].as_u64(),
                mtime: e["mtime"].as_i64(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_dir()
            .cmp(&a.is_dir())
            .then_with(|| crate::nav_tree::natural_cmp(&a.name, &b.name))
    });
    out
}

/// Local file name for a download: the entry name (a folder gets `.zip` / `.tar.zst`);
/// anything that is not a plain name falls back to a neutral one.
pub fn local_name(e: &Entry, tar: bool) -> String {
    let base = crate::backend::sftp::safe_local_name(&e.name)
        .map(str::to_string)
        .unwrap_or_else(|| "pxar-download".to_string());
    // Windows trips over trailing dots and spaces.
    let base = base.trim_end_matches(['.', ' ']).to_string();
    let base = if base.is_empty() {
        "pxar-download".to_string()
    } else {
        base
    };
    if e.is_dir() {
        format!("{base}{}", if tar { ".tar.zst" } else { ".zip" })
    } else {
        base
    }
}

/// Creates `dir/name` (or `name-1.ext`, …) without overwriting anything.
pub fn create_unique(dir: &Path, name: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    let name = crate::backend::sftp::safe_local_name(name)
        .ok_or_else(|| std::io::Error::other("unsafe file name"))?;
    let (stem, ext) = match name.split_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for i in 0..1000 {
        let candidate = if i == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{stem}-{i}{ext}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => return Ok((candidate, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

// ------------------------------------------------------------------ prune / verify / gc

/// `keep-*` form fields (only the non-zero ones); `None` if nothing is kept at all
/// (Aurora refuses a prune without any retention rule).
pub fn keep_form(k: &Keep) -> Option<Vec<(&'static str, String)>> {
    let mut form = Vec::new();
    for (key, v) in [
        ("keep-last", k.last),
        ("keep-hourly", k.hourly),
        ("keep-daily", k.daily),
        ("keep-weekly", k.weekly),
        ("keep-monthly", k.monthly),
        ("keep-yearly", k.yearly),
    ] {
        if v > 0 {
            form.push((key, v.to_string()));
        }
    }
    (!form.is_empty()).then_some(form)
}

/// Path and form of a group prune.
pub fn prune_request(
    store: &str,
    group: (&str, &str),
    keep: &Keep,
    dry: bool,
) -> Option<(String, Vec<(&'static str, String)>)> {
    if !valid_datastore(store) || !valid_backup_type(group.0) || !valid_backup_id(group.1) {
        return None;
    }
    let mut form = keep_form(keep)?;
    form.push(("backup-type", group.0.to_string()));
    form.push(("backup-id", group.1.to_string()));
    if dry {
        form.push(("dry-run", "1".to_string()));
    }
    Some((format!("/admin/datastore/{store}/prune"), form))
}

/// One snapshot in the answer of a prune call.
#[derive(Clone, Debug, PartialEq)]
pub struct PruneItem {
    pub time: i64,
    pub keep: bool,
    pub protected: bool,
}

pub fn parse_prune(data: &Value) -> Vec<PruneItem> {
    let mut out: Vec<PruneItem> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|i| {
            Some(PruneItem {
                time: i["backup-time"].as_i64()?,
                keep: i["keep"].as_bool().unwrap_or(num(&i["keep"]) != 0.0),
                protected: i["protected"]
                    .as_bool()
                    .unwrap_or(num(&i["protected"]) != 0.0),
            })
        })
        .collect();
    out.sort_by_key(|i| std::cmp::Reverse(i.time));
    out
}

/// `((type, id), result)` of one group.
pub type GroupResult = ((String, String), Result<Vec<PruneItem>, String>);

/// Result of a prune over several groups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PruneReport {
    /// `(type, id, result)` per group.
    pub groups: Vec<GroupResult>,
}

impl PruneReport {
    pub fn removes(&self) -> usize {
        self.groups
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .map(|l| l.iter().filter(|i| !i.keep).count())
            .sum()
    }

    pub fn keeps(&self) -> usize {
        self.groups
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .map(|l| l.iter().filter(|i| i.keep).count())
            .sum()
    }

    pub fn failed(&self) -> usize {
        self.groups.iter().filter(|(_, r)| r.is_err()).count()
    }

    /// Groups with at least one snapshot to remove.
    pub fn affected_groups(&self) -> usize {
        self.groups
            .iter()
            .filter(|(_, r)| r.as_ref().is_ok_and(|l| l.iter().any(|i| !i.keep)))
            .count()
    }
}

/// What to verify.
#[derive(Clone, Debug, PartialEq)]
pub enum VerifyScope {
    Store,
    Group(String, String),
    Snapshot(SnapRef),
}

pub fn verify_request(
    store: &str,
    scope: &VerifyScope,
    reverify: bool,
) -> Option<(String, Vec<(&'static str, String)>)> {
    if !valid_datastore(store) {
        return None;
    }
    let mut form = Vec::new();
    match scope {
        VerifyScope::Store => {}
        VerifyScope::Group(t, id) => {
            if !valid_backup_type(t) || !valid_backup_id(id) {
                return None;
            }
            form.push(("backup-type", t.clone()));
            form.push(("backup-id", id.clone()));
        }
        VerifyScope::Snapshot(s) => {
            if !s.valid() {
                return None;
            }
            form.push(("backup-type", s.btype.clone()));
            form.push(("backup-id", s.bid.clone()));
            form.push(("backup-time", s.time.to_string()));
        }
    }
    // Skipping what was verified recently is the server default.
    if reverify || matches!(scope, VerifyScope::Snapshot(_)) {
        form.push(("ignore-verified", "0".to_string()));
    }
    Some((format!("/admin/datastore/{store}/verify"), form))
}

/// Garbage collection state of a datastore (`GET …/gc`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GcStatus {
    pub last_state: Option<String>,
    pub last_end: Option<i64>,
    pub removed_bytes: Option<u64>,
    pub removed_chunks: Option<u64>,
    pub pending_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub schedule: Option<String>,
    pub next_run: Option<i64>,
}

pub fn parse_gc(data: &Value) -> GcStatus {
    let u = |k: &str| data[k].as_u64();
    GcStatus {
        last_state: data["last-run-state"].as_str().map(str::to_string),
        last_end: data["last-run-endtime"].as_i64(),
        removed_bytes: u("removed-bytes"),
        removed_chunks: u("removed-chunks"),
        pending_bytes: u("pending-bytes"),
        disk_bytes: u("disk-bytes"),
        schedule: data["schedule"].as_str().map(str::to_string),
        next_run: data["next-run"].as_i64(),
    }
}

/// A PBS task id: `UPID:node:pid:pstart:taskid:start:type:id:user:`.
pub fn valid_upid(upid: &str) -> bool {
    upid.starts_with("UPID:")
        && upid.ends_with(':')
        && upid.len() <= 512
        && !upid.contains("..")
        && upid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ":@!._-\\/".contains(c))
}

pub fn task_status_path(upid: &str) -> Option<String> {
    valid_upid(upid).then(|| format!("/nodes/localhost/tasks/{}/status", enc(upid)))
}

pub fn task_log_path(upid: &str, start: u64) -> Option<String> {
    valid_upid(upid).then(|| {
        format!(
            "/nodes/localhost/tasks/{}/log?start={start}&limit=200",
            enc(upid)
        )
    })
}

pub fn task_stop_path(upid: &str) -> Option<String> {
    valid_upid(upid).then(|| format!("/nodes/localhost/tasks/{}", enc(upid)))
}

// ------------------------------------------------------------------ jobs

#[derive(Clone, Debug)]
pub enum Payload {
    Browse {
        store: String,
        snap: SnapRef,
        path: String,
        result: Result<Vec<Entry>, Error>,
    },
    DlProgress {
        id: u64,
        done: u64,
        total: Option<u64>,
    },
    DlDone {
        id: u64,
        result: Result<(PathBuf, u64), String>,
    },
    Prune {
        id: u64,
        dry: bool,
        report: Result<PruneReport, Error>,
    },
    /// Progress of a started task (verify, gc).
    Task { id: u64, update: TaskUpdate },
    Gc {
        store: String,
        result: Result<GcStatus, Error>,
    },
}

#[derive(Clone, Debug)]
pub struct Event {
    pub server: Uuid,
    pub epoch: Epoch,
    pub session: Option<Session>,
    pub payload: Payload,
}

fn client(conn: &Conn) -> Client {
    let mut c = Client::new(conn);
    c.pbs = true;
    c
}

fn bad_input() -> Error {
    Error::Net(t!("ungültige Eingabe").into())
}

fn emit(sink: &Sink<Event>, server: Uuid, epoch: Epoch, c: &Client, payload: Payload) {
    sink.send(Event {
        server,
        epoch,
        session: c.session.clone(),
        payload,
    });
}

/// Where a job reports to.
#[derive(Clone)]
pub struct Ctx {
    pub server: Uuid,
    pub epoch: Epoch,
    pub conn: Conn,
    pub sink: Sink<Event>,
}

pub fn spawn_browse(
    rt: &tokio::runtime::Runtime,
    cx: Ctx,
    store: String,
    snap: SnapRef,
    path: String,
) {
    rt.spawn_blocking(move || {
        let mut c = client(&cx.conn);
        let result = match catalog_path(&store, &snap, &path) {
            Some(p) => c.call(false, &p).map(|d| parse_catalog(&d)),
            None => Err(bad_input()),
        };
        emit(
            &cx.sink,
            cx.server,
            cx.epoch,
            &c,
            Payload::Browse {
                store,
                snap,
                path,
                result,
            },
        );
    });
}

/// Streams one file or folder into `dir`; progress about four times a second.
#[allow(clippy::too_many_arguments)]
pub fn spawn_download(
    rt: &tokio::runtime::Runtime,
    cx: Ctx,
    id: u64,
    store: String,
    snap: SnapRef,
    entry: Entry,
    tar: bool,
    dir: PathBuf,
    cap: u64,
    cancel: Arc<AtomicBool>,
) {
    rt.spawn_blocking(move || {
        let mut c = client(&cx.conn);
        let result = download(
            &mut c, &cx, id, &store, &snap, &entry, tar, &dir, cap, &cancel,
        );
        emit(
            &cx.sink,
            cx.server,
            cx.epoch,
            &c,
            Payload::DlDone { id, result },
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn download(
    c: &mut Client,
    cx: &Ctx,
    id: u64,
    store: &str,
    snap: &SnapRef,
    entry: &Entry,
    tar: bool,
    dir: &Path,
    cap: u64,
    cancel: &AtomicBool,
) -> Result<(PathBuf, u64), String> {
    if !entry.downloadable() {
        return Err(t!("Nur Dateien und Ordner lassen sich herunterladen").into());
    }
    if let Some(size) = entry.size.filter(|_| !entry.is_dir())
        && size > cap
    {
        return Err(t!("Datei ist größer als das Limit ({} Bytes)", cap));
    }
    let path = download_path(store, snap, &entry.path, tar && entry.is_dir())
        .ok_or_else(|| t!("ungültige Eingabe").to_string())?;
    // Logs in (ticket) and checks the certificate pin before the big request.
    c.call(false, "/version").map_err(|e| e.to_string())?;
    let url = format!("{}{path}", c.base);
    let mut req = c.agent.get(&url);
    for (k, v) in c.auth_headers(Method::Get) {
        req = req.header(k, v);
    }
    let mut resp = req
        .config()
        .timeout_global(Some(Duration::from_secs(6 * 3600)))
        .build()
        .call()
        .map_err(|e| c.net_error(e).to_string())?;
    let status = resp.status().as_u16();
    if status != 200 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(Error::Http(status, super::super::short(&body)).to_string());
    }
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(t) = total
        && t > cap
    {
        return Err(t!("Download ist größer als das Limit ({} Bytes)", cap));
    }
    let (file_path, mut file) =
        create_unique(dir, &local_name(entry, tar)).map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().with_config().limit(cap + 1).reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last = Instant::now();
    let outcome: Result<(), String> = loop {
        if cancel.load(Ordering::Relaxed) || cx.sink.is_closed() {
            break Err(t!("abgebrochen").to_string());
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(e.to_string()),
        };
        done += n as u64;
        if done > cap {
            break Err(t!("Download überschreitet das Limit ({} Bytes)", cap));
        }
        if let Err(e) = file.write_all(&buf[..n]) {
            break Err(e.to_string());
        }
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            emit(
                &cx.sink,
                cx.server,
                cx.epoch,
                c,
                Payload::DlProgress { id, done, total },
            );
        }
    };
    drop(file);
    match outcome {
        Ok(()) => Ok((file_path, done)),
        Err(e) => {
            let _ = std::fs::remove_file(&file_path);
            Err(e)
        }
    }
}

/// Prune (or dry-run) every listed group of a datastore, one call per group.
pub fn spawn_prune(
    rt: &tokio::runtime::Runtime,
    cx: Ctx,
    id: u64,
    store: String,
    groups: Vec<(String, String)>,
    keep: Keep,
    dry: bool,
) {
    rt.spawn_blocking(move || {
        let mut c = client(&cx.conn);
        let report = (|| {
            let mut rep = PruneReport::default();
            for (t, i) in &groups {
                let Some((path, form)) = prune_request(&store, (t, i), &keep, dry) else {
                    return Err(bad_input());
                };
                if cx.sink.is_closed() {
                    break;
                }
                let r = match c.request(Method::Post, &path, &form) {
                    Ok(v) => Ok(parse_prune(&v)),
                    Err(e @ (Error::Untrusted(_) | Error::PinChanged { .. })) => return Err(e),
                    Err(e) => Err(e.to_string()),
                };
                rep.groups.push(((t.clone(), i.clone()), r));
            }
            Ok(rep)
        })();
        emit(
            &cx.sink,
            cx.server,
            cx.epoch,
            &c,
            Payload::Prune { id, dry, report },
        );
    });
}

/// What starts a task.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    Verify(VerifyScope, bool),
    Gc,
}

/// Starts a verify job / garbage collection and follows its task until it stops.
pub fn spawn_task(
    rt: &tokio::runtime::Runtime,
    cx: Ctx,
    id: u64,
    store: String,
    what: Start,
    cancel: Arc<AtomicBool>,
) {
    rt.spawn_blocking(move || {
        let mut c = client(&cx.conn);
        let update = |c: &Client, u: TaskUpdate| {
            emit(
                &cx.sink,
                cx.server,
                cx.epoch,
                c,
                Payload::Task { id, update: u },
            );
        };
        let request = match &what {
            Start::Verify(scope, re) => verify_request(&store, scope, *re),
            Start::Gc => valid_datastore(&store)
                .then(|| (format!("/admin/datastore/{store}/gc"), Vec::new())),
        };
        let Some((path, form)) = request else {
            update(&c, TaskUpdate::Done(Err(t!("ungültige Eingabe").into())));
            return;
        };
        let upid = match c.request(Method::Post, &path, &form) {
            Ok(v) => v.as_str().unwrap_or_default().to_string(),
            Err(e) => {
                update(&c, TaskUpdate::Done(Err(e.to_string())));
                return;
            }
        };
        if !valid_upid(&upid) {
            update(&c, TaskUpdate::Done(Ok("OK".into())));
            return;
        }
        update(&c, TaskUpdate::Started(upid.clone()));
        follow(&mut c, &upid, &cancel, &cx.sink, |c, u| update(c, u));
    });
}

/// Polls status and log of a PBS task until it stops.
fn follow(
    c: &mut Client,
    upid: &str,
    cancel: &AtomicBool,
    sink: &Sink<Event>,
    update: impl Fn(&Client, TaskUpdate),
) {
    let (Some(status_path), started) = (task_status_path(upid), Instant::now()) else {
        return;
    };
    let mut log_line = 0u64;
    let mut failures = 0;
    loop {
        std::thread::sleep(Duration::from_millis(if started.elapsed().as_secs() < 10 {
            700
        } else {
            2000
        }));
        if cancel.load(Ordering::Relaxed) || sink.is_closed() {
            return;
        }
        if let Some(lp) = task_log_path(upid, log_line)
            && let Ok(lines) = c.call(false, &lp)
            && let Some(a) = lines.as_array()
        {
            log_line += a.len() as u64;
            // Every new line, so the sheet can show the whole log.
            for line in a
                .iter()
                .filter_map(|l| l["t"].as_str())
                .filter(|t| !t.trim().is_empty())
                .take(100)
            {
                update(
                    c,
                    TaskUpdate::Progress(line.trim().chars().take(200).collect()),
                );
            }
        }
        match c.call(false, &status_path) {
            Ok(st) => {
                failures = 0;
                if st["status"].as_str() == Some("stopped") {
                    update(
                        c,
                        TaskUpdate::Done(task_result(
                            st["exitstatus"].as_str().unwrap_or_default(),
                        )),
                    );
                    return;
                }
            }
            Err(e) => {
                failures += 1;
                if failures >= 5 {
                    update(c, TaskUpdate::Done(Err(e.to_string())));
                    return;
                }
            }
        }
        if started.elapsed() > Duration::from_secs(12 * 3600) {
            update(
                c,
                TaskUpdate::Done(Err(
                    t!("Aufgabe läuft noch – Status nicht mehr verfolgt").into()
                )),
            );
            return;
        }
    }
}

/// Asks the server to stop a running task.
pub fn spawn_stop(rt: &tokio::runtime::Runtime, cx: Ctx, upid: String) {
    rt.spawn_blocking(move || {
        let mut c = client(&cx.conn);
        if let Some(p) = task_stop_path(&upid) {
            let _ = c.request(Method::Delete, &p, &[]);
        }
    });
}

pub fn spawn_gc_status(rt: &tokio::runtime::Runtime, cx: Ctx, store: String) {
    rt.spawn_blocking(move || {
        let mut c = client(&cx.conn);
        let result = if valid_datastore(&store) {
            c.call(false, &format!("/admin/datastore/{store}/gc"))
                .map(|d| parse_gc(&d))
        } else {
            Err(bad_input())
        };
        emit(
            &cx.sink,
            cx.server,
            cx.epoch,
            &c,
            Payload::Gc { store, result },
        );
    });
}

/// Datastore name → groups, for a prune of everything.
pub fn group_keys(snaps: &[PbsSnapshot]) -> Vec<(String, String)> {
    let mut seen: HashMap<(String, String), ()> = HashMap::new();
    let mut out = Vec::new();
    for s in snaps {
        let k = (s.btype.clone(), s.bid.clone());
        if seen.insert(k.clone(), ()).is_none() {
            out.push(k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap() -> SnapRef {
        SnapRef {
            btype: "ct".into(),
            bid: "101".into(),
            time: 1_700_000_000,
        }
    }

    #[test]
    fn paths_are_built_and_escaped() {
        let p = catalog_path("store1", &snap(), &b64("/root.pxar.didx/etc")).unwrap();
        assert_eq!(
            p,
            "/admin/datastore/store1/catalog?backup-type=ct&backup-id=101&backup-time=1700000000&filepath=L3Jvb3QucHhhci5kaWR4L2V0Yw%3D%3D"
        );
        assert_eq!(b64("/"), ROOT);
        let d = download_path("store1", &snap(), "L2E=", true).unwrap();
        assert!(d.starts_with("/admin/datastore/store1/pxar-file-download?"));
        assert!(d.ends_with("&filepath=L2E%3D&tar=1"));
        // The root is a listing, not a download.
        assert_eq!(download_path("store1", &snap(), ROOT, false), None);
        // Bad pieces never reach a URL.
        assert_eq!(catalog_path("../x", &snap(), ROOT), None);
        assert_eq!(catalog_path("store1", &snap(), "a&b=c"), None);
        assert_eq!(catalog_path("store1", &snap(), ""), None);
        let bad = SnapRef {
            bid: "1&x=2".into(),
            ..snap()
        };
        assert_eq!(catalog_path("store1", &bad, ROOT), None);
        let bad = SnapRef { time: 0, ..snap() };
        assert_eq!(download_path("store1", &bad, "L2E=", false), None);
    }

    #[test]
    fn catalog_listing() {
        let data = json!([
            {"filepath":"L2EvYg==","text":"b.txt","type":"f","size":12,"mtime":1700000000},
            {"filepath":"L2EvZA==","text":"Dir10","type":"d"},
            {"filepath":"L2EvZDI=","text":"Dir2","type":"d"},
            {"filepath":"L2EvbA==","text":"link","type":"l"},
            {"filepath":"bad path","text":"x","type":"f"},
            {"filepath":"L2E=","text":"","type":"f"},
            {"text":"nopath","type":"f"}
        ]);
        let e = parse_catalog(&data);
        let names: Vec<&str> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Dir2", "Dir10", "b.txt", "link"]);
        assert!(e[0].is_dir() && e[0].downloadable());
        assert!(!e[3].downloadable());
        assert_eq!(e[2].size, Some(12));
    }

    #[test]
    fn local_names_are_safe() {
        let mk = |name: &str, kind: char| Entry {
            name: name.into(),
            path: "L2E=".into(),
            kind,
            size: None,
            mtime: None,
        };
        assert_eq!(local_name(&mk("hosts", 'f'), false), "hosts");
        assert_eq!(local_name(&mk("etc", 'd'), false), "etc.zip");
        assert_eq!(local_name(&mk("etc", 'd'), true), "etc.tar.zst");
        for bad in ["..", "../../x", "a/b", "C:\\x", "a\0b", "", "."] {
            assert_eq!(local_name(&mk(bad, 'f'), false), "pxar-download", "{bad:?}");
        }
        assert_eq!(local_name(&mk("name. ", 'f'), false), "name");
        let dir = std::env::temp_dir().join(format!("aurora-pve2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (p1, _) = create_unique(&dir, "a.txt").unwrap();
        let (p2, _) = create_unique(&dir, "a.txt").unwrap();
        assert_eq!(p1.file_name().unwrap(), "a.txt");
        assert_eq!(p2.file_name().unwrap(), "a-1.txt");
        assert!(create_unique(&dir, "../evil").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prune_options() {
        let keep = Keep {
            last: 3,
            hourly: 0,
            daily: 7,
            weekly: 0,
            monthly: 0,
            yearly: 1,
        };
        let (path, form) = prune_request("store1", ("vm", "100"), &keep, true).unwrap();
        assert_eq!(path, "/admin/datastore/store1/prune");
        assert_eq!(
            form,
            vec![
                ("keep-last", "3".to_string()),
                ("keep-daily", "7".into()),
                ("keep-yearly", "1".into()),
                ("backup-type", "vm".into()),
                ("backup-id", "100".into()),
                ("dry-run", "1".into()),
            ]
        );
        let (_, real) = prune_request("store1", ("vm", "100"), &keep, false).unwrap();
        assert!(!real.iter().any(|(k, _)| *k == "dry-run"));
        let none = Keep {
            last: 0,
            hourly: 0,
            daily: 0,
            weekly: 0,
            monthly: 0,
            yearly: 0,
        };
        assert!(prune_request("store1", ("vm", "100"), &none, true).is_none());
        assert!(prune_request("s/../x", ("vm", "100"), &keep, true).is_none());
        assert!(prune_request("store1", ("vm", "1&x"), &keep, true).is_none());
        assert!(prune_request("store1", ("evil", "100"), &keep, true).is_none());
    }

    #[test]
    fn prune_report_counts() {
        let list = parse_prune(&json!([
            {"backup-type":"vm","backup-id":"100","backup-time":10,"keep":false},
            {"backup-type":"vm","backup-id":"100","backup-time":30,"keep":true},
            {"backup-type":"vm","backup-id":"100","backup-time":20,"keep":true,"protected":true},
            {"backup-type":"vm","backup-id":"100"}
        ]));
        assert_eq!(
            list.iter().map(|i| i.time).collect::<Vec<_>>(),
            vec![30, 20, 10]
        );
        assert!(list[1].protected);
        let rep = PruneReport {
            groups: vec![
                (("vm".into(), "100".into()), Ok(list)),
                (("vm".into(), "101".into()), Err("boom".into())),
                (
                    ("ct".into(), "5".into()),
                    Ok(vec![PruneItem {
                        time: 1,
                        keep: true,
                        protected: false,
                    }]),
                ),
            ],
        };
        assert_eq!(
            (
                rep.removes(),
                rep.keeps(),
                rep.failed(),
                rep.affected_groups()
            ),
            (1, 3, 1, 1)
        );
    }

    #[test]
    fn verify_and_gc_requests() {
        let (p, f) = verify_request("store1", &VerifyScope::Store, false).unwrap();
        assert_eq!(p, "/admin/datastore/store1/verify");
        assert!(f.is_empty());
        let (_, f) = verify_request("store1", &VerifyScope::Store, true).unwrap();
        assert_eq!(f, vec![("ignore-verified", "0".to_string())]);
        let (_, f) = verify_request(
            "store1",
            &VerifyScope::Group("vm".into(), "100".into()),
            false,
        )
        .unwrap();
        assert_eq!(f.len(), 2);
        let (_, f) = verify_request("store1", &VerifyScope::Snapshot(snap()), false).unwrap();
        assert!(f.contains(&("backup-time", "1700000000".to_string())));
        assert!(f.contains(&("ignore-verified", "0".to_string())));
        assert!(verify_request("x y", &VerifyScope::Store, false).is_none());
        assert!(
            verify_request(
                "store1",
                &VerifyScope::Group("vm".into(), "../".into()),
                false
            )
            .is_none()
        );
        let gc = parse_gc(
            &json!({"last-run-state":"OK","removed-bytes":1024,"pending-bytes":5,"schedule":"daily"}),
        );
        assert_eq!(gc.last_state.as_deref(), Some("OK"));
        assert_eq!((gc.removed_bytes, gc.pending_bytes), (Some(1024), Some(5)));
        assert_eq!(parse_gc(&json!(null)), GcStatus::default());
    }

    #[test]
    fn upid_paths() {
        let u = "UPID:pbs:000A:000B:000C:65A0B0C0:verificationjob:store1\\x3avm\\x2f100:root@pam:";
        assert!(valid_upid(u));
        let p = task_status_path(u).unwrap();
        assert!(p.starts_with("/nodes/localhost/tasks/UPID%3Apbs"));
        assert!(p.ends_with("/status"));
        assert!(!p.contains("x2f100:root"));
        for bad in ["", "UPID:x", "UPID:a/../b:", "UPID:a b;c:", "UPID:\n:"] {
            assert!(!valid_upid(bad), "{bad:?}");
        }
        assert!(task_log_path("nope", 0).is_none());
    }
}
