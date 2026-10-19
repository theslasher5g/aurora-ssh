//! Transfer queue. `Queue` is the pure bookkeeping (batches of file items, states,
//! scheduling with a global concurrency limit, pause/cancel/retry, persistence); the async
//! functions below do the actual copying of one file with resume, bandwidth limit and
//! verification. Nothing here touches the UI.
//!
//! Every file is written to `<target>.aurora-part` first and renamed when complete, so a
//! leftover partial file always means "interrupted transfer" and can be continued from its
//! size – for downloads (append locally), uploads (write remotely at an offset) and
//! server ⇄ server copies alike.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{OpenFlags, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use super::sftp::{Session, join, safe_local_name};

pub const PART_SUFFIX: &str = ".aurora-part";
const CHUNK: usize = 256 * 1024;
/// Automatic retries after a lost connection before an item shows "Fehler".
pub const MAX_AUTO_RETRIES: u8 = 3;
/// Folders with more files than this are refused.
pub const MAX_FILES_PER_BATCH: usize = 100_000;

// ---------------------------------------------------------------------------- model

/// One side of a copy: the local disk or a saved host.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum Loc {
    Local,
    Host(Uuid),
}

/// What to do when the target exists.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Policy {
    #[default]
    Overwrite,
    Skip,
    Rename,
    NewerOnly,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum State {
    #[default]
    Waiting,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub id: u64,
    pub src: String,
    pub dst: String,
    pub size: u64,
    /// Modification time of the source (unix seconds), if known.
    pub mtime: Option<i64>,
    pub done: u64,
    pub state: State,
    pub error: Option<String>,
    /// Automatic retries after connection loss.
    pub retries: u8,
    /// A first attempt was made: a partial file at the target is ours and may be continued.
    pub started: bool,
    pub note: Option<String>,
}

/// Counters of a batch, recomputed by `Batch::recount`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub files: u64,
    pub done: u64,
    pub skipped: u64,
    pub failed: u64,
    pub running: u64,
    pub waiting: u64,
    pub paused: u64,
    pub cancelled: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
}

/// Progress/speed bookkeeping that is not persisted.
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    pub expanding: bool,
    pub stats: Stats,
    pub speed: Speed,
    /// A "finished" notice was already shown.
    pub notified: bool,
    /// The batch had work since it was added/restored (finished notices only then).
    pub was_active: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Batch {
    pub id: u64,
    /// Top-level name (file or folder).
    pub name: String,
    pub from: Option<Loc>,
    pub to: Option<Loc>,
    pub from_label: String,
    pub to_label: String,
    pub src_root: String,
    /// Folder the item is copied into.
    pub dst_dir: String,
    pub is_dir: bool,
    pub policy: Policy,
    /// File list known (folders are listed when the batch starts).
    pub expanded: bool,
    pub items: Vec<Item>,
    /// Held by the user (or after a restart): nothing of this batch starts.
    pub paused: bool,
    /// Items already finished and dropped from the saved queue.
    pub base_files: u64,
    pub base_bytes: u64,
    pub created: i64,
    /// Error of the expansion (folder could not be read).
    pub error: Option<String>,
    /// Entries skipped because of unsafe names.
    pub unsafe_skipped: u32,
    #[serde(skip)]
    pub rt: Runtime,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Waiting,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl Batch {
    pub fn recount(&mut self) {
        let mut s = Stats {
            files: self.base_files,
            done: self.base_files,
            bytes_total: self.base_bytes,
            bytes_done: self.base_bytes,
            ..Default::default()
        };
        for it in &self.items {
            s.files += 1;
            s.bytes_total += it.size;
            match it.state {
                State::Done => {
                    s.done += 1;
                    s.bytes_done += it.size;
                }
                State::Skipped => {
                    s.skipped += 1;
                    s.done += 1;
                    s.bytes_done += it.size;
                }
                State::Failed => {
                    s.failed += 1;
                    s.bytes_done += it.done.min(it.size);
                }
                State::Running => {
                    s.running += 1;
                    s.bytes_done += it.done.min(it.size);
                }
                State::Waiting => {
                    s.waiting += 1;
                    s.bytes_done += it.done.min(it.size);
                }
                State::Paused => {
                    s.paused += 1;
                    s.bytes_done += it.done.min(it.size);
                }
                State::Cancelled => {
                    s.cancelled += 1;
                    s.bytes_done += it.done.min(it.size);
                }
            }
        }
        self.rt.stats = s;
    }

    pub fn status(&self) -> Status {
        let s = &self.rt.stats;
        if s.running > 0 || self.rt.expanding {
            Status::Running
        } else if self.paused && (s.waiting > 0 || s.paused > 0 || !self.expanded) {
            Status::Paused
        } else if s.waiting > 0 || (!self.expanded && self.error.is_none()) {
            Status::Waiting
        } else if s.paused > 0 {
            Status::Paused
        } else if s.failed > 0 || self.error.is_some() {
            Status::Failed
        } else if s.cancelled > 0 {
            Status::Cancelled
        } else {
            Status::Done
        }
    }

    /// Nothing left that could still run by itself.
    pub fn finished(&self) -> bool {
        !self.rt.expanding
            && (self.expanded || self.error.is_some())
            && self.rt.stats.running == 0
            && self.rt.stats.waiting == 0
            && self.rt.stats.paused == 0
    }

    /// Everything copied (skipped files count as done).
    pub fn complete(&self) -> bool {
        self.finished()
            && self.rt.stats.failed == 0
            && self.rt.stats.cancelled == 0
            && self.error.is_none()
    }

    fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        let i = self.items.binary_search_by_key(&id, |i| i.id).ok()?;
        self.items.get_mut(i)
    }
}

/// What a worker reports for one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done {
        note: Option<String>,
        /// Final path if the name had to be changed ("umbenennen").
        dst: Option<String>,
    },
    Skipped(String),
    /// Stopped on request (pause/cancel); partial data stays.
    Stopped,
    Failed {
        error: String,
        /// Connection lost: worth retrying by itself after reconnecting.
        connection: bool,
    },
}

#[derive(Debug, Default)]
pub struct Queue {
    pub batches: Vec<Batch>,
    next_id: u64,
}

pub type ItemRef = (u64, u64);

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Makes sure future ids do not collide with restored ones.
    fn bump_ids(&mut self) {
        let max = self
            .batches
            .iter()
            .flat_map(|b| std::iter::once(b.id).chain(b.items.iter().map(|i| i.id)))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max);
    }

    pub fn add(&mut self, mut b: Batch) -> u64 {
        if b.id == 0 {
            b.id = self.alloc_id();
        }
        b.rt.was_active = true;
        b.recount();
        let id = b.id;
        self.batches.push(b);
        id
    }

    pub fn batch(&self, id: u64) -> Option<&Batch> {
        self.batches.iter().find(|b| b.id == id)
    }

    pub fn batch_mut(&mut self, id: u64) -> Option<&mut Batch> {
        self.batches.iter_mut().find(|b| b.id == id)
    }

    pub fn running(&self) -> usize {
        self.batches
            .iter()
            .map(|b| b.items.iter().filter(|i| i.state == State::Running).count())
            .sum()
    }

    /// Batches whose file list must be read before they can run.
    pub fn to_expand(&self) -> Vec<u64> {
        self.batches
            .iter()
            .filter(|b| !b.expanded && !b.paused && !b.rt.expanding && b.error.is_none())
            .map(|b| b.id)
            .collect()
    }

    /// Items that may start now: oldest first, at most `limit - running` of them, only
    /// from batches that are not held and whose both sides `ready` reports as connected.
    pub fn runnable(&self, limit: usize, ready: impl Fn(Loc) -> bool) -> Vec<ItemRef> {
        let free = limit.saturating_sub(self.running());
        let mut out = Vec::new();
        if free == 0 {
            return out;
        }
        for b in &self.batches {
            if b.paused || !b.expanded {
                continue;
            }
            let (Some(from), Some(to)) = (b.from, b.to) else {
                continue;
            };
            if !ready(from) || !ready(to) {
                continue;
            }
            for it in b.items.iter().filter(|i| i.state == State::Waiting) {
                out.push((b.id, it.id));
                if out.len() >= free {
                    return out;
                }
            }
        }
        out
    }

    /// Hosts that waiting work needs a connection to.
    pub fn needed_hosts(&self) -> Vec<Uuid> {
        let mut hosts = Vec::new();
        for b in &self.batches {
            if b.paused || (b.expanded && b.rt.stats.waiting == 0) {
                continue;
            }
            for loc in [b.from, b.to].into_iter().flatten() {
                if let Loc::Host(h) = loc
                    && !hosts.contains(&h)
                {
                    hosts.push(h);
                }
            }
        }
        hosts
    }

    /// Marks the item as running; returns whether it had been started before (a partial
    /// file at the target is then its own and may be continued).
    pub fn mark_running(&mut self, (b, i): ItemRef) -> bool {
        let mut was_started = false;
        if let Some(it) = self.batch_mut(b).and_then(|b| b.item_mut(i)) {
            was_started = it.started;
            it.state = State::Running;
            it.started = true;
            it.error = None;
        }
        was_started
    }

    /// Stores the file list of a folder batch.
    pub fn set_expanded(
        &mut self,
        batch: u64,
        root_dst: String,
        specs: Vec<ItemSpec>,
        skipped: u32,
    ) {
        let mut ids = Vec::with_capacity(specs.len());
        for _ in &specs {
            ids.push(self.alloc_id());
        }
        if let Some(b) = self.batch_mut(batch) {
            b.rt.expanding = false;
            b.expanded = true;
            b.unsafe_skipped = skipped;
            b.dst_dir = root_dst;
            b.items = specs
                .into_iter()
                .zip(ids)
                .map(|(s, id)| Item {
                    id,
                    src: s.src,
                    dst: s.dst,
                    size: s.size,
                    mtime: s.mtime,
                    ..Default::default()
                })
                .collect();
            b.recount();
        }
    }

    pub fn expansion_failed(&mut self, batch: u64, error: String) {
        if let Some(b) = self.batch_mut(batch) {
            b.rt.expanding = false;
            b.error = Some(error);
            b.recount();
        }
    }

    /// Records the end of a worker run.
    pub fn apply(&mut self, (b, i): ItemRef, outcome: Outcome) {
        let Some(batch) = self.batch_mut(b) else {
            return;
        };
        if let Some(it) = batch.item_mut(i) {
            match outcome {
                Outcome::Done { note, dst } => {
                    it.state = State::Done;
                    it.done = it.size;
                    it.error = None;
                    it.note = note;
                    if let Some(d) = dst {
                        it.dst = d;
                    }
                }
                Outcome::Skipped(why) => {
                    it.state = State::Skipped;
                    it.done = it.size;
                    it.note = Some(why);
                }
                // The user already set Paused/Cancelled; only fix a stray Running.
                Outcome::Stopped => {
                    if it.state == State::Running {
                        it.state = State::Paused;
                    }
                }
                Outcome::Failed { error, connection } => {
                    if it.state != State::Running {
                        // Cancelled or paused meanwhile.
                    } else if connection && it.retries < MAX_AUTO_RETRIES {
                        it.retries += 1;
                        it.state = State::Waiting;
                        it.error = Some(error);
                    } else {
                        it.state = State::Failed;
                        it.error = Some(error);
                    }
                }
            }
        }
        batch.recount();
    }

    /// Writes worker progress into the items.
    pub fn set_progress(&mut self, (b, i): ItemRef, done: u64) {
        if let Some(it) = self.batch_mut(b).and_then(|b| b.item_mut(i))
            && it.state == State::Running
        {
            it.done = done;
        }
    }

    /// Holds the batch; returns the running items to stop.
    pub fn pause(&mut self, batch: u64) -> Vec<u64> {
        let mut running = Vec::new();
        if let Some(b) = self.batch_mut(batch) {
            b.paused = true;
            for it in &mut b.items {
                match it.state {
                    State::Running => {
                        running.push(it.id);
                        it.state = State::Paused;
                    }
                    State::Waiting => it.state = State::Paused,
                    _ => {}
                }
            }
            b.recount();
        }
        running
    }

    pub fn resume(&mut self, batch: u64) {
        if let Some(b) = self.batch_mut(batch) {
            b.paused = false;
            for it in &mut b.items {
                if it.state == State::Paused {
                    it.state = State::Waiting;
                }
            }
            b.rt.notified = false;
            b.rt.was_active = true;
            b.recount();
        }
    }

    /// Cancels the unfinished items; returns the running ones to stop.
    pub fn cancel(&mut self, batch: u64) -> Vec<u64> {
        let mut running = Vec::new();
        if let Some(b) = self.batch_mut(batch) {
            b.paused = false;
            for it in &mut b.items {
                match it.state {
                    State::Running => {
                        running.push(it.id);
                        it.state = State::Cancelled;
                    }
                    State::Waiting | State::Paused => it.state = State::Cancelled,
                    _ => {}
                }
            }
            if !b.expanded && b.error.is_none() {
                b.error = Some("abgebrochen".into());
            }
            b.recount();
        }
        running
    }

    /// Failed and cancelled items run again (continuing partial files).
    pub fn retry(&mut self, batch: u64) {
        if let Some(b) = self.batch_mut(batch) {
            b.paused = false;
            if !b.expanded {
                b.error = None;
            }
            for it in &mut b.items {
                if matches!(it.state, State::Failed | State::Cancelled) {
                    it.state = State::Waiting;
                    it.error = None;
                    it.retries = 0;
                }
            }
            b.rt.notified = false;
            b.rt.was_active = true;
            b.recount();
        }
    }

    /// Removes a batch; returns its running items to stop.
    pub fn remove(&mut self, batch: u64) -> Vec<u64> {
        let running = self.cancel(batch);
        self.batches.retain(|b| b.id != batch);
        running
    }

    /// Drops batches that are completely done. Returns how many.
    pub fn clear_finished(&mut self) -> usize {
        let before = self.batches.len();
        self.batches.retain(|b| !b.complete());
        before - self.batches.len()
    }

    pub fn recount(&mut self) {
        for b in &mut self.batches {
            b.recount();
        }
    }

    /// Items waiting or running in all batches.
    pub fn active_count(&self) -> usize {
        self.batches
            .iter()
            .filter(|b| !b.finished() && !(b.paused && b.rt.stats.running == 0))
            .count()
    }

    /// Whether anything is unfinished (incl. paused).
    pub fn unfinished(&self) -> usize {
        self.batches.iter().filter(|b| !b.finished()).count()
    }

    // ------------------------------------------------------------------ persistence

    /// JSON of the unfinished work. No credentials: only host ids, paths and counters.
    pub fn to_json(&self) -> String {
        let batches: Vec<Batch> = self
            .batches
            .iter()
            .filter(|b| !b.finished() || b.rt.stats.failed > 0)
            .map(|b| {
                let mut c = b.clone();
                let mut base_files = c.base_files;
                let mut base_bytes = c.base_bytes;
                c.items.retain(|it| match it.state {
                    State::Done | State::Skipped => {
                        base_files += 1;
                        base_bytes += it.size;
                        false
                    }
                    State::Cancelled => false,
                    _ => true,
                });
                for it in &mut c.items {
                    if it.state == State::Running {
                        it.state = State::Waiting;
                    }
                }
                c.base_files = base_files;
                c.base_bytes = base_bytes;
                c
            })
            .collect();
        serde_json::to_string(&Saved {
            version: 1,
            batches,
        })
        .unwrap_or_default()
    }

    /// Loads a saved queue. Everything unfinished comes back held (paused), so the user
    /// decides whether to continue. Batches for hosts that no longer exist are dropped.
    pub fn from_json(json: &str, host_known: impl Fn(Uuid) -> bool) -> Queue {
        let mut q = Queue::new();
        let Ok(saved) = serde_json::from_str::<Saved>(json) else {
            return q;
        };
        for mut b in saved.batches {
            let known = |l: Option<Loc>| match l {
                Some(Loc::Host(h)) => host_known(h),
                Some(Loc::Local) => true,
                None => false,
            };
            if !known(b.from) || !known(b.to) {
                continue;
            }
            b.paused = true;
            for it in &mut b.items {
                if matches!(it.state, State::Waiting | State::Running) {
                    it.state = State::Paused;
                }
            }
            b.rt = Runtime::default();
            b.recount();
            if b.finished() && b.rt.stats.failed == 0 {
                continue;
            }
            q.batches.push(b);
        }
        q.bump_ids();
        q
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    batches: Vec<Batch>,
}

// ---------------------------------------------------------------------------- pure helpers

/// Where a continued transfer starts: the size of the partial target file if it is ours
/// (`allow`), the source is unchanged and the partial is not longer than the source.
pub fn resume_offset(partial: Option<u64>, total: u64, source_changed: bool, allow: bool) -> u64 {
    match partial {
        Some(p) if allow && !source_changed && p <= total => p,
        _ => 0,
    }
}

/// `report.txt` → `report (1).txt`; dotfiles keep their name in front.
pub fn renamed(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({n}){}", &name[..i], &name[i..]),
        _ => format!("{name} ({n})"),
    }
}

/// Whether a file is copied when `exists` at the target.
pub fn should_copy(
    policy: Policy,
    exists: bool,
    src_mtime: Option<i64>,
    dst_mtime: Option<i64>,
) -> bool {
    if !exists {
        return true;
    }
    match policy {
        Policy::Overwrite | Policy::Rename => true,
        Policy::Skip => false,
        Policy::NewerOnly => match (src_mtime, dst_mtime) {
            (Some(s), Some(d)) => s > d,
            _ => true,
        },
    }
}

/// Single-quotes a path for `sh`; `None` for paths that cannot be quoted safely.
pub fn sh_quote(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    Some(format!("'{}'", path.replace('\'', "'\\''")))
}

/// Command printing the SHA-256 of a remote file (`sha256sum`, else `shasum -a 256`).
pub fn hash_command(path: &str) -> Option<String> {
    let q = sh_quote(path)?;
    Some(format!(
        "sha256sum -- {q} 2>/dev/null || shasum -a 256 -- {q} 2>/dev/null"
    ))
}

/// The hash from `sha256sum` / `shasum` output.
pub fn parse_hash(out: &str) -> Option<String> {
    let first = out.split_whitespace().next()?;
    (first.len() == 64 && first.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| first.to_ascii_lowercase())
}

/// Heuristic: the error means the connection is gone (worth retrying after reconnect).
pub fn is_connection_error(msg: &str) -> bool {
    let l = msg.to_lowercase();
    [
        "connection",
        "broken pipe",
        "channel",
        "closed",
        "timeout",
        "zeitüberschreitung",
        "unexpected eof",
        "disconnect",
        "reset by peer",
        "no connection",
        "i/o:",
        "recverror",
        "senderror",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Read size per step: small enough that the limiter paces smoothly.
pub fn chunk_size(limit_kbps: u32) -> usize {
    if limit_kbps == 0 {
        return CHUNK;
    }
    ((limit_kbps as usize * 1024) / 8).clamp(4096, CHUNK)
}

// ---------------------------------------------------------------------------- limiter / speed

/// Global bandwidth limit shared by all running transfers.
#[derive(Debug, Default)]
pub struct Limiter {
    kbps: AtomicU32,
    next: std::sync::Mutex<Option<Instant>>,
}

impl Limiter {
    pub fn set_kbps(&self, v: u32) {
        self.kbps.store(v, Ordering::Relaxed);
    }

    pub fn kbps(&self) -> u32 {
        self.kbps.load(Ordering::Relaxed)
    }

    /// How long the caller must wait before it may send `n` bytes. Reserves the slot.
    pub fn reserve(&self, now: Instant, n: u64) -> Duration {
        let kbps = self.kbps();
        if kbps == 0 {
            return Duration::ZERO;
        }
        let rate = kbps as f64 * 1024.0;
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let start = next.map_or(now, |t| t.max(now));
        *next = Some(start + Duration::from_secs_f64(n as f64 / rate));
        start.saturating_duration_since(now)
    }

    pub async fn take(&self, n: u64) {
        let wait = self.reserve(Instant::now(), n);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

/// Smoothed transfer speed in bytes per second.
#[derive(Clone, Debug, Default)]
pub struct Speed {
    last: Option<(Instant, u64)>,
    pub bps: f64,
}

impl Speed {
    /// Feeds the total number of bytes done so far; call every few hundred ms.
    pub fn update(&mut self, now: Instant, bytes: u64) {
        match self.last {
            None => self.last = Some((now, bytes)),
            Some((t, b)) => {
                let dt = now.saturating_duration_since(t).as_secs_f64();
                if dt < 0.4 {
                    return;
                }
                let inst = bytes.saturating_sub(b) as f64 / dt;
                self.bps = if self.bps == 0.0 {
                    inst
                } else {
                    self.bps * 0.6 + inst * 0.4
                };
                self.last = Some((now, bytes));
            }
        }
    }

    pub fn reset(&mut self) {
        *self = Speed::default();
    }

    pub fn eta(&self, remaining: u64) -> Option<Duration> {
        (self.bps > 1.0).then(|| Duration::from_secs_f64(remaining as f64 / self.bps))
    }
}

// ---------------------------------------------------------------------------- endpoints

/// A side of a copy at run time.
#[derive(Clone)]
pub enum Ep {
    Local,
    Remote(Session),
}

#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub size: u64,
    pub mtime: Option<i64>,
    pub is_dir: bool,
}

trait Reader: AsyncRead + AsyncSeek + Send + Unpin {}
impl<T: AsyncRead + AsyncSeek + Send + Unpin> Reader for T {}
trait Writer: AsyncWrite + AsyncSeek + Send + Unpin {}
impl<T: AsyncWrite + AsyncSeek + Send + Unpin> Writer for T {}

fn secs(t: SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

impl Ep {
    pub fn is_local(&self) -> bool {
        matches!(self, Ep::Local)
    }

    pub fn join(&self, dir: &str, name: &str) -> String {
        match self {
            Ep::Local => Path::new(dir).join(name).display().to_string(),
            Ep::Remote(_) => join(dir, name),
        }
    }

    pub async fn stat(&self, path: &str) -> Result<Option<Stat>> {
        match self {
            Ep::Local => match tokio::fs::metadata(path).await {
                Ok(m) => Ok(Some(Stat {
                    size: m.len(),
                    mtime: m.modified().ok().and_then(secs),
                    is_dir: m.is_dir(),
                })),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            Ep::Remote(s) => match s.0.metadata(path.to_string()).await {
                Ok(m) => Ok(Some(Stat {
                    size: m.size.unwrap_or(0),
                    mtime: m.mtime.map(i64::from),
                    is_dir: m.is_dir(),
                })),
                Err(SftpError::Status(st)) if st.status_code == StatusCode::NoSuchFile => Ok(None),
                Err(e) => Err(e.into()),
            },
        }
    }

    pub async fn mkdir_p(&self, path: &str) -> Result<()> {
        match self {
            Ep::Local => tokio::fs::create_dir_all(path).await.map_err(Into::into),
            Ep::Remote(s) => {
                // Find the first existing ancestor, then create downwards.
                let mut missing = Vec::new();
                let mut cur = path.trim_end_matches('/').to_string();
                while !cur.is_empty() && cur != "/" {
                    if s.0.try_exists(cur.clone()).await? {
                        break;
                    }
                    missing.push(cur.clone());
                    cur = super::sftp::parent(&cur);
                }
                for dir in missing.into_iter().rev() {
                    if let Err(e) = s.0.create_dir(dir.clone()).await {
                        // Created by someone else meanwhile?
                        if !s.0.try_exists(dir).await.unwrap_or(false) {
                            return Err(e.into());
                        }
                    }
                }
                Ok(())
            }
        }
    }

    pub async fn remove_file(&self, path: &str) -> Result<()> {
        match self {
            Ep::Local => tokio::fs::remove_file(path).await.map_err(Into::into),
            Ep::Remote(s) => s.0.remove_file(path.to_string()).await.map_err(Into::into),
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Ep::Local => tokio::fs::rename(from, to).await.map_err(Into::into),
            Ep::Remote(s) => {
                s.0.rename(from.to_string(), to.to_string())
                    .await
                    .map_err(Into::into)
            }
        }
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<Box<dyn Reader>> {
        let mut r: Box<dyn Reader> = match self {
            Ep::Local => Box::new(tokio::fs::File::open(path).await?),
            Ep::Remote(s) => Box::new(s.0.open(path.to_string()).await?),
        };
        if offset > 0 {
            r.seek(std::io::SeekFrom::Start(offset)).await?;
        }
        Ok(r)
    }

    async fn open_write(&self, path: &str, offset: u64) -> Result<Box<dyn Writer>> {
        let mut w: Box<dyn Writer> = match (self, offset) {
            (Ep::Local, 0) => Box::new(tokio::fs::File::create(path).await?),
            (Ep::Local, _) => Box::new(
                tokio::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .await?,
            ),
            (Ep::Remote(s), 0) => Box::new(s.0.create(path.to_string()).await?),
            (Ep::Remote(s), _) => Box::new(
                s.0.open_with_flags(path.to_string(), OpenFlags::WRITE | OpenFlags::CREATE)
                    .await?,
            ),
        };
        if offset > 0 {
            w.seek(std::io::SeekFrom::Start(offset)).await?;
        }
        Ok(w)
    }

    /// Sets the modification time (best effort).
    async fn set_mtime(&self, path: &str, mtime: i64) {
        if mtime <= 0 {
            return;
        }
        match self {
            Ep::Local => {
                let p = path.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    let t = std::time::UNIX_EPOCH + Duration::from_secs(mtime as u64);
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(p)
                        .and_then(|f| f.set_modified(t))
                })
                .await;
            }
            Ep::Remote(s) => {
                let mut attrs = russh_sftp::client::fs::Metadata::empty();
                attrs.atime = Some(mtime as u32);
                attrs.mtime = Some(mtime as u32);
                let _ = s.0.set_metadata(path.to_string(), attrs).await;
            }
        }
    }

    /// SHA-256 of a file; `None` if this side cannot compute it (no `sha256sum`).
    pub async fn sha256(&self, path: &str, ctl: &Ctl) -> Result<Option<String>> {
        match self {
            Ep::Local => {
                let p = PathBuf::from(path);
                let stop = ctl.stop_flag();
                tokio::task::spawn_blocking(move || -> Result<Option<String>> {
                    use std::io::Read;
                    let mut f = std::fs::File::open(p)?;
                    let mut h = Sha256::new();
                    let mut buf = vec![0u8; 1024 * 1024];
                    loop {
                        if stop.load(Ordering::Relaxed) == STOP_CANCEL {
                            return Err(anyhow!(t!("abgebrochen")));
                        }
                        let n = f.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        h.update(&buf[..n]);
                    }
                    Ok(Some(hex(&h.finalize())))
                })
                .await?
            }
            Ep::Remote(s) => {
                let Some(cmd) = hash_command(path) else {
                    return Ok(None);
                };
                let out = exec_capture(&s.1, &cmd, Duration::from_secs(3600)).await?;
                Ok(parse_hash(&out))
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Runs a command on its own channel and returns its stdout (capped).
async fn exec_capture(
    handle: &Arc<super::ssh::SshHandle>,
    cmd: &str,
    timeout: Duration,
) -> Result<String> {
    use russh::ChannelMsg;
    let run = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        let mut out = Vec::new();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() < 4096 {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok(String::from_utf8_lossy(&out).into_owned())
    };
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
}

// ---------------------------------------------------------------------------- expansion

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSpec {
    pub src: String,
    pub dst: String,
    pub size: u64,
    pub mtime: Option<i64>,
}

#[derive(Debug)]
pub struct Expansion {
    pub items: Vec<ItemSpec>,
    pub skipped: u32,
    /// Folder the top-level item lands in (after "umbenennen").
    pub root_dst: String,
}

fn safe_name(name: &str, to_local: bool) -> bool {
    if to_local {
        safe_local_name(name).is_some()
    } else {
        !(name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']))
    }
}

/// Lists what a batch copies. Single files become one item; folders are walked (symlinks
/// to files are followed locally, remote symlinks are skipped) and their target folders
/// created, so empty folders arrive too.
pub async fn expand(
    src: &Ep,
    dst: &Ep,
    src_root: &str,
    dst_dir: &str,
    name: &str,
    policy: Policy,
) -> Result<Expansion> {
    let to_local = dst.is_local();
    if !safe_name(name, to_local) {
        return Err(anyhow!(
            "{}",
            t!("unsicherer Dateiname: {}", format!("{name:?}"))
        ));
    }
    let root = src
        .stat(src_root)
        .await?
        .ok_or_else(|| anyhow!(t!("Quelle nicht gefunden: {}", src_root)))?;
    if !root.is_dir {
        let dst_path = dst.join(dst_dir, name);
        return Ok(Expansion {
            items: vec![ItemSpec {
                src: src_root.to_string(),
                dst: dst_path,
                size: root.size,
                mtime: root.mtime,
            }],
            skipped: 0,
            root_dst: dst_dir.to_string(),
        });
    }
    // Target folder name: "umbenennen" avoids merging into an existing folder.
    let mut root_dst = dst.join(dst_dir, name);
    if policy == Policy::Rename && dst.stat(&root_dst).await?.is_some() {
        let mut n = 1;
        loop {
            let cand = dst.join(dst_dir, &renamed(name, n));
            if dst.stat(&cand).await?.is_none() {
                root_dst = cand;
                break;
            }
            n += 1;
        }
    }
    dst.mkdir_p(&root_dst).await?;
    let mut items = Vec::new();
    let mut skipped = 0u32;
    let mut stack = vec![(src_root.to_string(), root_dst.clone(), 0u32)];
    while let Some((sdir, ddir, depth)) = stack.pop() {
        if depth > 64 {
            skipped += 1;
            continue;
        }
        // (name, is_dir, size, mtime)
        let entries: Vec<(String, bool, u64, Option<i64>)> = match src {
            Ep::Local => {
                let mut rd = tokio::fs::read_dir(&sdir).await?;
                let mut v = Vec::new();
                while let Some(e) = rd.next_entry().await? {
                    let n = e.file_name().to_string_lossy().to_string();
                    let Ok(m) = tokio::fs::metadata(e.path()).await else {
                        skipped += 1;
                        continue;
                    };
                    if !m.is_dir() && !m.is_file() {
                        continue;
                    }
                    v.push((n, m.is_dir(), m.len(), m.modified().ok().and_then(secs)));
                }
                v
            }
            Ep::Remote(s) => {
                let mut v = Vec::new();
                for e in s.0.read_dir(sdir.clone()).await? {
                    let n = e.file_name();
                    if n == "." || n == ".." {
                        continue;
                    }
                    let ft = e.file_type();
                    if ft.is_dir() || ft.is_file() {
                        let m = e.metadata();
                        v.push((n, ft.is_dir(), m.size.unwrap_or(0), m.mtime.map(i64::from)));
                    }
                }
                v
            }
        };
        for (n, is_dir, size, mtime) in entries {
            if !safe_name(&n, to_local) {
                skipped += 1;
                continue;
            }
            let (sp, dp) = (src.join(&sdir, &n), dst.join(&ddir, &n));
            if is_dir {
                dst.mkdir_p(&dp).await?;
                stack.push((sp, dp, depth + 1));
            } else {
                items.push(ItemSpec {
                    src: sp,
                    dst: dp,
                    size,
                    mtime,
                });
                if items.len() > MAX_FILES_PER_BATCH {
                    return Err(anyhow!(
                        "{}",
                        t!(
                            "Der Ordner enthält mehr als {} Dateien",
                            MAX_FILES_PER_BATCH
                        )
                    ));
                }
            }
        }
    }
    items.sort_by(|a, b| a.src.cmp(&b.src));
    Ok(Expansion {
        items,
        skipped,
        root_dst,
    })
}

// ---------------------------------------------------------------------------- worker

pub const STOP_NONE: u8 = 0;
pub const STOP_PAUSE: u8 = 1;
pub const STOP_CANCEL: u8 = 2;

/// Shared state between the UI and the worker of one running item.
#[derive(Debug, Default)]
pub struct Ctl {
    pub done: AtomicU64,
    stop: Arc<AtomicU8>,
}

impl Ctl {
    pub fn request_stop(&self, cancel: bool) {
        self.stop.store(
            if cancel { STOP_CANCEL } else { STOP_PAUSE },
            Ordering::Relaxed,
        );
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed) != STOP_NONE
    }

    /// Cancelled for good (a pause lets a finished file complete its verification).
    pub fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Relaxed) == STOP_CANCEL
    }

    pub fn stop_flag(&self) -> Arc<AtomicU8> {
        self.stop.clone()
    }
}

pub struct Job {
    pub src_ep: Ep,
    pub dst_ep: Ep,
    pub src: String,
    pub dst: String,
    pub size: u64,
    pub mtime: Option<i64>,
    pub policy: Policy,
    pub verify: bool,
    /// A partial file at the target belongs to an earlier attempt of this item.
    pub resumable: bool,
    pub ctl: Arc<Ctl>,
    pub limiter: Arc<Limiter>,
}

/// Copies one file. Never panics; every failure becomes an `Outcome`.
pub async fn run_item(job: Job) -> Outcome {
    match copy_file(&job).await {
        Ok(o) => o,
        Err(e) => {
            if job.ctl.stopped() {
                return Outcome::Stopped;
            }
            let msg = format!("{e:#}");
            let error = super::ssh::friendly_text(&msg).unwrap_or(msg);
            Outcome::Failed {
                connection: is_connection_error(&error),
                error,
            }
        }
    }
}

async fn copy_file(job: &Job) -> Result<Outcome> {
    if job.ctl.stopped() {
        return Ok(Outcome::Stopped);
    }
    let src_stat = job
        .src_ep
        .stat(&job.src)
        .await?
        .ok_or_else(|| anyhow!(t!("Quelle nicht gefunden: {}", job.src)))?;
    // Target exists?
    let mut dst = job.dst.clone();
    let existing = job.dst_ep.stat(&dst).await?;
    if let Some(d) = &existing
        && d.is_dir
    {
        return Err(anyhow!(t!("Am Ziel gibt es einen Ordner mit diesem Namen")));
    }
    if !should_copy(
        job.policy,
        existing.is_some(),
        src_stat.mtime,
        existing.and_then(|d| d.mtime),
    ) {
        let why = match job.policy {
            Policy::NewerOnly => t!("Ziel ist neuer oder gleich alt"),
            _ => t!("existiert schon"),
        };
        return Ok(Outcome::Skipped(why.to_string()));
    }
    let mut renamed_to = None;
    if job.policy == Policy::Rename && existing.is_some() {
        let (dir, name) = split_path(&job.dst, job.dst_ep.is_local());
        let mut n = 1;
        loop {
            let cand = job.dst_ep.join(&dir, &renamed(&name, n));
            if job.dst_ep.stat(&cand).await?.is_none() {
                dst = cand.clone();
                renamed_to = Some(cand);
                break;
            }
            n += 1;
        }
    }
    let (dir, _) = split_path(&dst, job.dst_ep.is_local());
    if !dir.is_empty() {
        job.dst_ep.mkdir_p(&dir).await?;
    }

    let part = format!("{dst}{PART_SUFFIX}");
    let total = src_stat.size;
    let source_changed = total != job.size || (job.mtime.is_some() && src_stat.mtime != job.mtime);
    let partial = job.dst_ep.stat(&part).await?.map(|s| s.size);
    let offset = resume_offset(partial, total, source_changed, job.resumable);

    job.ctl.done.store(offset, Ordering::Relaxed);
    let mut reader = job.src_ep.open_read(&job.src, offset).await?;
    let mut writer = job.dst_ep.open_write(&part, offset).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = offset;
    loop {
        if job.ctl.stopped() {
            let _ = writer.shutdown().await;
            return Ok(Outcome::Stopped);
        }
        let want = chunk_size(job.limiter.kbps());
        let n = tokio::time::timeout(Duration::from_secs(90), reader.read(&mut buf[..want]))
            .await
            .map_err(|_| anyhow!(t!("Zeitüberschreitung")))??;
        if n == 0 {
            break;
        }
        job.limiter.take(n as u64).await;
        writer.write_all(&buf[..n]).await?;
        done += n as u64;
        job.ctl.done.store(done, Ordering::Relaxed);
    }
    writer.shutdown().await?;
    drop(reader);

    let written = job.dst_ep.stat(&part).await?.map(|s| s.size).unwrap_or(0);
    if written != total {
        // A short or inconsistent partial file would be "continued" forever.
        let _ = job.dst_ep.remove_file(&part).await;
        return Err(anyhow!(
            "{}",
            t!(
                "Größe stimmt nicht: erwartet {}, geschrieben {}",
                total,
                written
            )
        ));
    }
    if job.dst_ep.stat(&dst).await?.is_some() {
        job.dst_ep.remove_file(&dst).await?;
    }
    job.dst_ep.rename(&part, &dst).await?;
    if let Some(m) = src_stat.mtime {
        job.dst_ep.set_mtime(&dst, m).await;
    }

    let mut note = None;
    if job.verify {
        let (a, b) = tokio::join!(
            job.src_ep.sha256(&job.src, &job.ctl),
            job.dst_ep.sha256(&dst, &job.ctl)
        );
        match (a, b) {
            (Ok(Some(x)), Ok(Some(y))) => {
                if x != y {
                    let _ = job.dst_ep.remove_file(&dst).await;
                    return Err(anyhow!(
                        "{}",
                        t!(
                            "Prüfsumme stimmt nicht überein (Quelle {}…, Ziel {}…) – die beschädigte Zieldatei wurde gelöscht, bitte erneut versuchen",
                            &x[..12],
                            &y[..12]
                        )
                    ));
                }
                note = Some(t!("SHA-256 geprüft").to_string());
            }
            (Err(e), _) | (_, Err(e)) => {
                if job.ctl.cancelled() {
                    return Ok(Outcome::Stopped);
                }
                note = Some(t!("Größe geprüft, Prüfsumme nicht möglich: {}", e));
            }
            _ => {
                note = Some(t!("Größe geprüft (keine Prüfsumme verfügbar)").to_string());
            }
        }
    }
    Ok(Outcome::Done {
        note,
        dst: renamed_to,
    })
}

fn split_path(path: &str, local: bool) -> (String, String) {
    if local {
        let p = Path::new(path);
        (
            p.parent()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    } else {
        let t = path.trim_end_matches('/');
        match t.rfind('/') {
            Some(0) => ("/".into(), t[1..].into()),
            Some(i) => (t[..i].into(), t[i + 1..].into()),
            None => (String::new(), t.into()),
        }
    }
}

// ---------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, size: u64) -> Item {
        Item {
            id,
            src: format!("/s/{id}"),
            dst: format!("/d/{id}"),
            size,
            ..Default::default()
        }
    }

    fn batch(id: u64, n: u64) -> Batch {
        Batch {
            id,
            name: format!("b{id}"),
            from: Some(Loc::Local),
            to: Some(Loc::Host(Uuid::from_u128(1))),
            expanded: true,
            items: (0..n).map(|i| item(id * 1000 + i + 1, 100)).collect(),
            ..Default::default()
        }
    }

    fn all_ready(_: Loc) -> bool {
        true
    }

    #[test]
    fn scheduling_respects_the_limit() {
        let mut q = Queue::new();
        q.add(batch(1, 3));
        q.add(batch(2, 2));
        let first = q.runnable(2, all_ready);
        assert_eq!(first, vec![(1, 1001), (1, 1002)]);
        for r in &first {
            q.mark_running(*r);
        }
        assert_eq!(q.running(), 2);
        // Limit reached: nothing more.
        assert!(q.runnable(2, all_ready).is_empty());
        // One finishes: exactly one slot frees up, oldest waiting item next.
        q.apply(
            first[0],
            Outcome::Done {
                note: None,
                dst: None,
            },
        );
        assert_eq!(q.runnable(2, all_ready), vec![(1, 1003)]);
        // Higher limit fills up in order.
        assert_eq!(q.runnable(10, all_ready).len(), 1 + 2);
    }

    #[test]
    fn disconnected_sides_wait() {
        let mut q = Queue::new();
        q.add(batch(1, 1));
        let host = Uuid::from_u128(1);
        assert!(q.runnable(2, |l| l != Loc::Host(host)).is_empty());
        assert_eq!(q.needed_hosts(), vec![host]);
        assert_eq!(q.runnable(2, all_ready).len(), 1);
    }

    #[test]
    fn pause_resume_cancel_retry() {
        let mut q = Queue::new();
        q.add(batch(1, 3));
        let r = q.runnable(1, all_ready);
        q.mark_running(r[0]);
        let to_stop = q.pause(1);
        assert_eq!(to_stop, vec![1001]);
        assert!(
            q.runnable(5, all_ready).is_empty(),
            "paused batch does not start"
        );
        let b = q.batch(1).unwrap();
        assert_eq!(b.status(), Status::Paused);
        assert!(b.items.iter().all(|i| i.state == State::Paused));
        // A late "stopped" report keeps the paused state.
        q.apply((1, 1001), Outcome::Stopped);
        assert_eq!(q.batch(1).unwrap().items[0].state, State::Paused);
        q.resume(1);
        assert_eq!(q.runnable(5, all_ready).len(), 3);
        // Cancel.
        q.mark_running((1, 1001));
        let stop = q.cancel(1);
        assert_eq!(stop, vec![1001]);
        assert_eq!(q.batch(1).unwrap().status(), Status::Cancelled);
        assert!(q.runnable(5, all_ready).is_empty());
        // Retry puts them back.
        q.retry(1);
        assert_eq!(q.runnable(5, all_ready).len(), 3);
    }

    #[test]
    fn failures_and_automatic_retries() {
        let mut q = Queue::new();
        q.add(batch(1, 1));
        let r = (1, 1001);
        // Connection loss: waits again, up to the limit.
        for n in 1..=MAX_AUTO_RETRIES {
            q.mark_running(r);
            q.apply(
                r,
                Outcome::Failed {
                    error: "connection reset".into(),
                    connection: true,
                },
            );
            let it = &q.batch(1).unwrap().items[0];
            assert_eq!(it.state, State::Waiting, "retry {n}");
            assert_eq!(it.retries, n);
        }
        q.mark_running(r);
        q.apply(
            r,
            Outcome::Failed {
                error: "connection reset".into(),
                connection: true,
            },
        );
        assert_eq!(q.batch(1).unwrap().items[0].state, State::Failed);
        assert_eq!(q.batch(1).unwrap().status(), Status::Failed);
        // Other errors fail at once; retry resets the counter.
        q.retry(1);
        q.mark_running(r);
        q.apply(
            r,
            Outcome::Failed {
                error: "permission denied".into(),
                connection: false,
            },
        );
        assert_eq!(q.batch(1).unwrap().items[0].state, State::Failed);
        q.retry(1);
        assert_eq!(q.batch(1).unwrap().items[0].retries, 0);
        assert_eq!(q.batch(1).unwrap().items[0].state, State::Waiting);
    }

    #[test]
    fn status_and_stats() {
        let mut q = Queue::new();
        q.add(batch(1, 2));
        assert_eq!(q.batch(1).unwrap().status(), Status::Waiting);
        q.mark_running((1, 1001));
        q.recount();
        assert_eq!(q.batch(1).unwrap().status(), Status::Running);
        q.set_progress((1, 1001), 40);
        q.recount();
        assert_eq!(q.batch(1).unwrap().rt.stats.bytes_done, 40);
        q.apply(
            (1, 1001),
            Outcome::Done {
                note: None,
                dst: None,
            },
        );
        q.mark_running((1, 1002));
        q.apply((1, 1002), Outcome::Skipped("x".into()));
        let b = q.batch(1).unwrap();
        assert_eq!(b.status(), Status::Done);
        assert!(b.complete());
        assert_eq!(b.rt.stats.done, 2);
        assert_eq!(b.rt.stats.skipped, 1);
        assert_eq!(b.rt.stats.bytes_done, 200);
        assert_eq!(q.clear_finished(), 1);
        assert!(q.batches.is_empty());
    }

    #[test]
    fn folder_batches_expand_first() {
        let mut q = Queue::new();
        let mut b = batch(1, 0);
        b.expanded = false;
        b.is_dir = true;
        q.add(b);
        assert_eq!(q.to_expand(), vec![1]);
        assert_eq!(q.batch(1).unwrap().status(), Status::Waiting);
        assert!(q.runnable(2, all_ready).is_empty());
        q.batch_mut(1).unwrap().rt.expanding = true;
        assert!(q.to_expand().is_empty());
        assert_eq!(q.batch(1).unwrap().status(), Status::Running);
        q.set_expanded(
            1,
            "/d/x".into(),
            vec![
                ItemSpec {
                    src: "/s/a".into(),
                    dst: "/d/x/a".into(),
                    size: 5,
                    mtime: None,
                },
                ItemSpec {
                    src: "/s/b".into(),
                    dst: "/d/x/b".into(),
                    size: 7,
                    mtime: Some(3),
                },
            ],
            2,
        );
        let b = q.batch(1).unwrap();
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.unsafe_skipped, 2);
        assert_eq!(b.rt.stats.bytes_total, 12);
        assert_eq!(q.runnable(5, all_ready).len(), 2);
        // Failed listing.
        let mut b2 = batch(2, 0);
        b2.expanded = false;
        q.add(b2);
        q.expansion_failed(2, "no access".into());
        assert_eq!(q.batch(2).unwrap().status(), Status::Failed);
        assert!(q.batch(2).unwrap().finished());
    }

    #[test]
    fn persistence_round_trip() {
        let host = Uuid::from_u128(1);
        let mut q = Queue::new();
        q.add(batch(1, 3));
        q.add(batch(2, 1));
        q.mark_running((1, 1001));
        q.set_progress((1, 1001), 55);
        q.mark_running((1, 1002));
        q.apply(
            (1, 1002),
            Outcome::Done {
                note: None,
                dst: None,
            },
        );
        // Batch 2 completes entirely and is not saved.
        q.mark_running((2, 2001));
        q.apply(
            (2, 2001),
            Outcome::Done {
                note: None,
                dst: None,
            },
        );
        q.recount();
        let json = q.to_json();
        assert!(!json.contains("password"));
        let r = Queue::from_json(&json, |h| h == host);
        assert_eq!(r.batches.len(), 1);
        let b = &r.batches[0];
        assert_eq!(b.id, 1);
        assert!(b.paused, "restored work waits for the user");
        assert_eq!(b.items.len(), 2, "finished items are folded into counters");
        assert_eq!(b.base_files, 1);
        assert_eq!(b.base_bytes, 100);
        assert!(b.items.iter().all(|i| i.state == State::Paused));
        assert!(
            b.items[0].started,
            "the interrupted item may continue its partial file"
        );
        assert_eq!(b.rt.stats.files, 3);
        assert_eq!(b.rt.stats.done, 1);
        // Unknown host: dropped.
        assert!(Queue::from_json(&json, |_| false).batches.is_empty());
        // Resume releases it.
        let mut r = r;
        r.resume(1);
        assert_eq!(r.runnable(5, all_ready).len(), 2);
        // New ids do not collide with restored ones.
        assert!(r.alloc_id() > 1003);
        // Garbage is ignored.
        assert!(Queue::from_json("not json", |_| true).batches.is_empty());
        assert!(Queue::from_json("", |_| true).batches.is_empty());
    }

    #[test]
    fn resume_offsets() {
        // Continue where the partial file ends.
        assert_eq!(resume_offset(Some(500), 1000, false, true), 500);
        // Fully written partial: nothing left to read.
        assert_eq!(resume_offset(Some(1000), 1000, false, true), 1000);
        // Partial longer than the source, or the source changed: start over.
        assert_eq!(resume_offset(Some(2000), 1000, false, true), 0);
        assert_eq!(resume_offset(Some(500), 1000, true, true), 0);
        // Not ours.
        assert_eq!(resume_offset(Some(500), 1000, false, false), 0);
        assert_eq!(resume_offset(None, 1000, false, true), 0);
    }

    #[test]
    fn rename_and_policy() {
        assert_eq!(renamed("report.txt", 1), "report (1).txt");
        assert_eq!(renamed("a.tar.gz", 2), "a.tar (2).gz");
        assert_eq!(renamed(".bashrc", 1), ".bashrc (1)");
        assert_eq!(renamed("README", 3), "README (3)");
        assert!(should_copy(Policy::Skip, false, None, None));
        assert!(!should_copy(Policy::Skip, true, Some(9), Some(1)));
        assert!(should_copy(Policy::Overwrite, true, Some(1), Some(9)));
        assert!(should_copy(Policy::Rename, true, None, None));
        assert!(should_copy(Policy::NewerOnly, true, Some(10), Some(5)));
        assert!(!should_copy(Policy::NewerOnly, true, Some(5), Some(5)));
        assert!(!should_copy(Policy::NewerOnly, true, Some(4), Some(5)));
        assert!(should_copy(Policy::NewerOnly, true, None, Some(5)));
    }

    #[test]
    fn quoting_and_hash_parsing() {
        assert_eq!(sh_quote("/a b/c").as_deref(), Some("'/a b/c'"));
        assert_eq!(sh_quote("it's").as_deref(), Some("'it'\\''s'"));
        assert_eq!(sh_quote("$(rm -rf /)").as_deref(), Some("'$(rm -rf /)'"));
        assert_eq!(sh_quote("a\0b"), None);
        assert_eq!(sh_quote(""), None);
        let cmd = hash_command("/tmp/x; reboot").unwrap();
        assert!(cmd.contains("'/tmp/x; reboot'"));
        assert!(hash_command("a\0").is_none());
        let h = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_hash(&format!("{h}  file\n")).as_deref(), Some(h));
        assert_eq!(
            parse_hash(&format!("{}  f", h.to_uppercase())).as_deref(),
            Some(h)
        );
        assert_eq!(parse_hash("sha256sum: x: No such file"), None);
        assert_eq!(parse_hash(""), None);
        assert_eq!(parse_hash("abc  f"), None);
    }

    #[test]
    fn connection_errors() {
        assert!(is_connection_error("Connection reset by peer"));
        assert!(is_connection_error("I/O: channel closed"));
        assert!(is_connection_error("Zeitüberschreitung"));
        assert!(!is_connection_error("Permission denied"));
        assert!(!is_connection_error("No such file"));
    }

    #[test]
    fn limiter_paces_without_bursts() {
        let l = Limiter::default();
        let t0 = Instant::now();
        assert_eq!(l.reserve(t0, 10_000_000), Duration::ZERO, "unlimited");
        l.set_kbps(1000); // 1,024,000 B/s
        // The first chunk goes at once, each next waits for the previous one's time.
        assert_eq!(l.reserve(t0, 1_024_000), Duration::ZERO);
        let w2 = l.reserve(t0, 1_024_000);
        assert!((w2.as_secs_f64() - 1.0).abs() < 0.01, "{w2:?}");
        let w3 = l.reserve(t0, 512_000);
        assert!((w3.as_secs_f64() - 2.0).abs() < 0.01, "{w3:?}");
        // After idle time there is no saved-up credit.
        let later = t0 + Duration::from_secs(60);
        assert_eq!(l.reserve(later, 1_024_000), Duration::ZERO);
        let w = l.reserve(later, 1_024_000);
        assert!((w.as_secs_f64() - 1.0).abs() < 0.01);
        // Chunks shrink with the limit.
        assert_eq!(chunk_size(0), CHUNK);
        assert_eq!(chunk_size(100_000), CHUNK);
        assert_eq!(chunk_size(64), 8192);
        assert_eq!(chunk_size(1), 4096);
    }

    #[test]
    fn speed_and_eta() {
        let mut s = Speed::default();
        let t0 = Instant::now();
        s.update(t0, 0);
        s.update(t0 + Duration::from_millis(100), 1000); // too soon: ignored
        assert_eq!(s.bps, 0.0);
        s.update(t0 + Duration::from_secs(1), 1_000_000);
        assert!((s.bps - 1_000_000.0).abs() < 1.0);
        assert_eq!(s.eta(5_000_000).map(|d| d.as_secs()), Some(5));
        assert!(Speed::default().eta(10).is_none());
    }

    #[test]
    fn path_split() {
        assert_eq!(
            split_path("/a/b/c.txt", false),
            ("/a/b".into(), "c.txt".into())
        );
        assert_eq!(split_path("/c.txt", false), ("/".into(), "c.txt".into()));
        assert_eq!(split_path("c.txt", false), (String::new(), "c.txt".into()));
    }

    // End-to-end on the local disk: resume, policies, folders.
    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aurora-xfer-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn job(src: &Path, dst: &Path, size: u64, resumable: bool, limiter: Arc<Limiter>) -> Job {
        Job {
            src_ep: Ep::Local,
            dst_ep: Ep::Local,
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            size,
            mtime: None,
            policy: Policy::Overwrite,
            verify: true,
            resumable,
            ctl: Arc::new(Ctl::default()),
            limiter,
        }
    }

    #[tokio::test]
    async fn local_copy_resumes_from_the_partial_file() {
        let dir = tmp("resume");
        let src = dir.join("src.bin");
        let data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let dst = dir.join("out").join("dst.bin");
        // An interrupted earlier attempt left 300_000 bytes.
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let part = format!("{}{PART_SUFFIX}", dst.display());
        std::fs::write(&part, &data[..300_000]).unwrap();
        let j = job(
            &src,
            &dst,
            data.len() as u64,
            true,
            Arc::new(Limiter::default()),
        );
        let out = run_item(j).await;
        assert!(matches!(out, Outcome::Done { .. }), "{out:?}");
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        assert!(!Path::new(&part).exists(), "partial renamed away");

        // Not resumable (not ours): the stale partial is replaced, result still correct.
        std::fs::remove_file(&dst).unwrap();
        std::fs::write(&part, vec![9u8; 1000]).unwrap();
        let j = job(
            &src,
            &dst,
            data.len() as u64,
            false,
            Arc::new(Limiter::default()),
        );
        assert!(matches!(run_item(j).await, Outcome::Done { .. }));
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stop_keeps_the_partial_file_and_continues_later() {
        let dir = tmp("stop");
        let src = dir.join("src.bin");
        let data = vec![7u8; 600_000];
        std::fs::write(&src, &data).unwrap();
        let dst = dir.join("dst.bin");
        let limiter = Arc::new(Limiter::default());
        limiter.set_kbps(400); // ~400 KB/s: chunks of 51 KB, stop soon
        let j = job(&src, &dst, data.len() as u64, true, limiter.clone());
        let ctl = j.ctl.clone();
        let h = tokio::spawn(run_item(j));
        tokio::time::sleep(Duration::from_millis(600)).await;
        ctl.request_stop(false);
        assert_eq!(h.await.unwrap(), Outcome::Stopped);
        let part = format!("{}{PART_SUFFIX}", dst.display());
        let got = std::fs::metadata(&part).unwrap().len();
        assert!(got > 0 && got < 600_000, "partial {got}");
        assert!(!dst.exists());
        // Continue unlimited.
        limiter.set_kbps(0);
        let j = job(&src, &dst, data.len() as u64, true, limiter);
        assert!(matches!(run_item(j).await, Outcome::Done { .. }));
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn policies_on_existing_targets() {
        let dir = tmp("policy");
        let src = dir.join("a.txt");
        std::fs::write(&src, "new").unwrap();
        let dst = dir.join("b.txt");
        std::fs::write(&dst, "old").unwrap();
        let lim = Arc::new(Limiter::default());
        let mut j = job(&src, &dst, 3, false, lim.clone());
        j.policy = Policy::Skip;
        assert!(matches!(run_item(j).await, Outcome::Skipped(_)));
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old");

        let mut j = job(&src, &dst, 3, false, lim.clone());
        j.policy = Policy::Rename;
        let Outcome::Done { dst: Some(new), .. } = run_item(j).await else {
            panic!("rename expected");
        };
        assert!(new.ends_with("b (1).txt"), "{new}");
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old");

        let j = job(&src, &dst, 3, false, lim);
        assert!(matches!(run_item(j).await, Outcome::Done { .. }));
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn folders_expand_and_copy() {
        let dir = tmp("folder");
        let src = dir.join("tree");
        std::fs::create_dir_all(src.join("sub/empty")).unwrap();
        std::fs::write(src.join("a.txt"), "aaa").unwrap();
        std::fs::write(src.join("sub/b.txt"), "bbbb").unwrap();
        let dst_dir = dir.join("out");
        std::fs::create_dir_all(&dst_dir).unwrap();
        let ex = expand(
            &Ep::Local,
            &Ep::Local,
            &src.display().to_string(),
            &dst_dir.display().to_string(),
            "tree",
            Policy::Overwrite,
        )
        .await
        .unwrap();
        assert_eq!(ex.items.len(), 2);
        assert_eq!(ex.skipped, 0);
        assert!(
            dst_dir.join("tree/sub/empty").is_dir(),
            "empty folders arrive"
        );
        for it in &ex.items {
            let j = Job {
                src_ep: Ep::Local,
                dst_ep: Ep::Local,
                src: it.src.clone(),
                dst: it.dst.clone(),
                size: it.size,
                mtime: it.mtime,
                policy: Policy::Overwrite,
                verify: false,
                resumable: false,
                ctl: Arc::new(Ctl::default()),
                limiter: Arc::new(Limiter::default()),
            };
            assert!(matches!(run_item(j).await, Outcome::Done { .. }));
        }
        assert_eq!(
            std::fs::read_to_string(dst_dir.join("tree/sub/b.txt")).unwrap(),
            "bbbb"
        );
        // Rename policy picks a fresh folder.
        let ex = expand(
            &Ep::Local,
            &Ep::Local,
            &src.display().to_string(),
            &dst_dir.display().to_string(),
            "tree",
            Policy::Rename,
        )
        .await
        .unwrap();
        assert!(ex.root_dst.ends_with("tree (1)"), "{}", ex.root_dst);
        // Unsafe names from a server never leave the target folder.
        assert!(
            expand(
                &Ep::Local,
                &Ep::Local,
                &src.display().to_string(),
                &dst_dir.display().to_string(),
                "..",
                Policy::Overwrite
            )
            .await
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
