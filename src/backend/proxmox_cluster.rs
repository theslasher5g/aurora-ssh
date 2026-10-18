//! Cluster-wide Proxmox views: the task log, the cluster / storage overview, node
//! package updates and the background watcher behind the alerts. The node shell
//! (termproxy over a websocket) and Proxmox Backup Server live in the submodules.
//!
//! Every name that goes into a URL (node, UPID, storage, …) is validated first; values
//! from the API never end up in a shell command.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use super::{Client, Conn, Epoch, Error, Method, Session, Sink, enc, num, task_result, valid_node};

#[path = "proxmox_cluster_pbs.rs"]
pub mod pbs;
#[path = "proxmox_cluster_term.rs"]
pub mod term;

// ------------------------------------------------------------------ validation

/// `UPID:node:pid:pstart:starttime:type:id:user:` – the fields of a Proxmox VE task id.
#[derive(Clone, Debug, PartialEq)]
pub struct Upid {
    pub node: String,
    pub kind: String,
    pub id: String,
    pub user: String,
    pub start: i64,
}

pub fn parse_upid(upid: &str) -> Option<Upid> {
    if upid.len() > 512
        || !upid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ":@!._-=+".contains(c))
    {
        return None;
    }
    let parts: Vec<&str> = upid.split(':').collect();
    if parts.len() < 8 || parts[0] != "UPID" || !valid_node(parts[1]) {
        return None;
    }
    let start = i64::from_str_radix(parts[4], 16).ok()?;
    Some(Upid {
        node: parts[1].to_string(),
        kind: parts[5].to_string(),
        id: parts[6].to_string(),
        user: parts[7].to_string(),
        start,
    })
}

pub fn valid_upid(upid: &str) -> bool {
    parse_upid(upid).is_some()
}

/// `/nodes/{node}/tasks/{upid}` for a valid UPID (the node comes from the UPID itself).
pub fn task_path(upid: &str) -> Option<String> {
    let u = parse_upid(upid)?;
    Some(format!("/nodes/{}/tasks/{}", u.node, enc(upid)))
}

// ------------------------------------------------------------------ tasks

#[derive(Clone, Debug, PartialEq)]
pub enum TaskState {
    Running,
    Ok,
    Warnings(String),
    Failed(String),
}

impl TaskState {
    fn of(status: Option<&str>, ended: bool) -> Self {
        match status.map(str::trim) {
            None | Some("running") if !ended => TaskState::Running,
            None | Some("") => TaskState::Failed(t!("unbekannter Fehler").into()),
            Some(s) => match task_result(s) {
                Ok(s) if s == "OK" => TaskState::Ok,
                Ok(w) => TaskState::Warnings(w),
                Err(e) => TaskState::Failed(e),
            },
        }
    }
}

/// One task from `/cluster/tasks` or `/nodes/{node}/tasks`.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskInfo {
    pub upid: String,
    pub node: String,
    /// `vzdump`, `qmigrate`, `qmclone`, …
    pub kind: String,
    /// Usually the VMID; empty or a storage name for other tasks.
    pub id: String,
    pub user: String,
    pub start: i64,
    pub end: Option<i64>,
    pub state: TaskState,
}

impl TaskInfo {
    pub fn vmid(&self) -> Option<u32> {
        self.id.parse().ok().filter(|v| *v > 0)
    }

    pub fn running(&self) -> bool {
        self.state == TaskState::Running
    }

    pub fn failed(&self) -> bool {
        matches!(self.state, TaskState::Failed(_))
    }

    /// Seconds from start to end (or to `now` while running).
    pub fn duration(&self, now: i64) -> i64 {
        (self.end.unwrap_or(now) - self.start).max(0)
    }
}

pub fn parse_tasks(data: &Value) -> Vec<TaskInfo> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| {
            let upid = t["upid"].as_str()?.to_string();
            let u = parse_upid(&upid)?;
            let end = Some(num(&t["endtime"]) as i64).filter(|e| *e > 0);
            let start = Some(num(&t["starttime"]) as i64)
                .filter(|s| *s > 0)
                .unwrap_or(u.start);
            Some(TaskInfo {
                node: t["node"]
                    .as_str()
                    .filter(|n| valid_node(n))
                    .unwrap_or(&u.node)
                    .to_string(),
                kind: t["type"].as_str().unwrap_or(&u.kind).to_string(),
                id: t["id"].as_str().unwrap_or(&u.id).to_string(),
                user: t["user"].as_str().unwrap_or(&u.user).to_string(),
                state: TaskState::of(t["status"].as_str(), end.is_some()),
                start,
                end,
                upid,
            })
        })
        .collect()
}

/// Joins task lists (cluster list + failures per node): one entry per UPID, the
/// finished state wins, newest first.
pub fn merge_tasks(lists: Vec<Vec<TaskInfo>>) -> Vec<TaskInfo> {
    let mut by: HashMap<String, TaskInfo> = HashMap::new();
    for t in lists.into_iter().flatten() {
        match by.get(&t.upid) {
            Some(old) if !old.running() && t.running() => {}
            _ => {
                by.insert(t.upid.clone(), t);
            }
        }
    }
    let mut out: Vec<TaskInfo> = by.into_values().collect();
    out.sort_by(|a, b| b.start.cmp(&a.start).then(a.upid.cmp(&b.upid)));
    out
}

/// Lines of `…/tasks/{upid}/log`.
pub fn parse_log(data: &Value) -> Vec<String> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["t"].as_str().map(str::to_string))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskLog {
    pub lines: Vec<String>,
    pub running: bool,
    /// `exitstatus` once stopped.
    pub exit: Option<String>,
}

// ------------------------------------------------------------------ overview

#[derive(Clone, Debug, PartialEq)]
pub struct ClusterInfo {
    pub name: String,
    pub quorate: bool,
    pub nodes: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeInfo {
    pub name: String,
    pub online: bool,
    pub ip: String,
    /// `8.2.4` from `/nodes/{node}/version`.
    pub version: Option<String>,
    /// Pending package updates (`None` = unknown, e.g. no permission).
    pub updates: Option<Vec<AptPackage>>,
}

/// `data` of `/cluster/status`: the cluster entry (absent on a single node) and nodes.
pub fn parse_cluster_status(data: &Value) -> (Option<ClusterInfo>, Vec<NodeInfo>) {
    let mut cluster = None;
    let mut nodes = Vec::new();
    for e in data.as_array().map(|a| a.as_slice()).unwrap_or_default() {
        match e["type"].as_str() {
            Some("cluster") => {
                cluster = Some(ClusterInfo {
                    name: e["name"].as_str().unwrap_or_default().to_string(),
                    quorate: num(&e["quorate"]) != 0.0,
                    nodes: num(&e["nodes"]) as u32,
                });
            }
            Some("node") => {
                if let Some(name) = e["name"].as_str() {
                    nodes.push(NodeInfo {
                        name: name.to_string(),
                        online: num(&e["online"]) != 0.0,
                        ip: e["ip"].as_str().unwrap_or_default().to_string(),
                        version: None,
                        updates: None,
                    });
                }
            }
            _ => {}
        }
    }
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    (cluster, nodes)
}

/// One line of `/cluster/ha/status/current`.
#[derive(Clone, Debug, PartialEq)]
pub struct HaEntry {
    pub id: String,
    /// `quorum`, `master`, `lrm`, `service`.
    pub kind: String,
    pub status: String,
    /// Services: `started`, `stopped`, `error`, `fence`, …
    pub state: String,
    pub node: String,
}

impl HaEntry {
    /// Needs attention: quorum not OK, a service in error/fence/recovery, a dead LRM.
    pub fn bad(&self) -> bool {
        match self.kind.as_str() {
            "quorum" => !self.status.trim().eq_ignore_ascii_case("ok"),
            "service" => matches!(self.state.as_str(), "error" | "fence" | "recovery"),
            "lrm" => self.status.contains("dead") || self.status.contains("old timestamp"),
            _ => false,
        }
    }
}

pub fn parse_ha(data: &Value) -> Vec<HaEntry> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            Some(HaEntry {
                id: e["id"].as_str()?.to_string(),
                kind: e["type"].as_str().unwrap_or_default().to_string(),
                status: e["status"].as_str().unwrap_or_default().to_string(),
                state: e["state"].as_str().unwrap_or_default().to_string(),
                node: e["node"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct StorageInfo {
    pub id: String,
    pub kind: String,
    /// `None` for shared storages (listed once for the whole cluster).
    pub node: Option<String>,
    pub content: String,
    pub active: bool,
    pub enabled: bool,
    pub used: u64,
    pub total: u64,
}

impl StorageInfo {
    pub fn usage(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64) as f32
        }
    }
}

/// `data` of `/nodes/{node}/storage`.
pub fn parse_storages(node: &str, data: &Value) -> Vec<StorageInfo> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let shared = num(&s["shared"]) != 0.0;
            Some(StorageInfo {
                id: s["storage"].as_str()?.to_string(),
                kind: s["type"].as_str().unwrap_or_default().to_string(),
                node: (!shared).then(|| node.to_string()),
                content: s["content"].as_str().unwrap_or_default().to_string(),
                active: s.get("active").is_none_or(|a| num(a) != 0.0),
                enabled: s.get("enabled").is_none_or(|a| num(a) != 0.0),
                used: num(&s["used"]) as u64,
                total: num(&s["total"]) as u64,
            })
        })
        .collect()
}

/// Shared storages once, local ones per node; sorted shared first, then node / name.
pub fn merge_storages(lists: Vec<Vec<StorageInfo>>) -> Vec<StorageInfo> {
    let mut out: Vec<StorageInfo> = Vec::new();
    for s in lists.into_iter().flatten() {
        if let Some(old) = out.iter_mut().find(|o| o.id == s.id && o.node == s.node) {
            // A shared storage seen from a second node: keep the numbers of an active one.
            if !old.active && s.active {
                *old = s;
            }
            continue;
        }
        out.push(s);
    }
    out.sort_by(|a, b| {
        a.node
            .is_some()
            .cmp(&b.node.is_some())
            .then(a.node.cmp(&b.node))
            .then(a.id.cmp(&b.id))
    });
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZfsPool {
    pub node: String,
    pub name: String,
    pub health: String,
    pub size: u64,
    pub alloc: u64,
    pub frag: u32,
}

impl ZfsPool {
    pub fn healthy(&self) -> bool {
        self.health.eq_ignore_ascii_case("online")
    }
}

/// `data` of `/nodes/{node}/disks/zfs`.
pub fn parse_zfs(node: &str, data: &Value) -> Vec<ZfsPool> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            Some(ZfsPool {
                node: node.to_string(),
                name: p["name"].as_str()?.to_string(),
                health: p["health"].as_str().unwrap_or("UNKNOWN").to_string(),
                size: num(&p["size"]) as u64,
                alloc: num(&p["alloc"]) as u64,
                frag: num(&p["frag"]) as u32,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct CephHealth {
    /// `HEALTH_OK`, `HEALTH_WARN`, `HEALTH_ERR`.
    pub status: String,
    /// Summaries of the failing checks.
    pub checks: Vec<String>,
}

/// `data` of `/cluster/ceph/status`; `None` without a usable health entry.
pub fn parse_ceph(data: &Value) -> Option<CephHealth> {
    let h = data.get("health")?;
    let status = h["status"].as_str()?.to_string();
    let mut checks: Vec<String> = h["checks"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(name, c)| {
                    c["summary"]["message"]
                        .as_str()
                        .or_else(|| c["summary"].as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| name.clone())
                })
                .collect()
        })
        .unwrap_or_default();
    checks.sort();
    Some(CephHealth { status, checks })
}

/// One upgradable package.
#[derive(Clone, Debug, PartialEq)]
pub struct AptPackage {
    pub name: String,
    pub old: String,
    pub new: String,
    pub origin: String,
}

/// `data` of `GET /nodes/{node}/apt/update`.
pub fn parse_apt(data: &Value) -> Vec<AptPackage> {
    let mut out: Vec<AptPackage> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            Some(AptPackage {
                name: p["Package"].as_str()?.to_string(),
                old: p["OldVersion"].as_str().unwrap_or_default().to_string(),
                new: p["Version"].as_str().unwrap_or_default().to_string(),
                origin: p["Origin"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub cluster: Option<ClusterInfo>,
    pub nodes: Vec<NodeInfo>,
    pub ha: Vec<HaEntry>,
    pub storages: Vec<StorageInfo>,
    pub zfs: Vec<ZfsPool>,
    pub ceph: Option<CephHealth>,
    /// Parts that could not be read (the rest still shows).
    pub problems: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warn,
    Crit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub level: Level,
    pub text: String,
}

/// Storage usage worth a warning on the overview.
pub const STORAGE_WARN: f32 = 0.85;
/// Storage usage that raises an alert.
pub const STORAGE_ALERT: f32 = 0.90;

impl Overview {
    /// Everything that needs attention, worst first.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut out = Vec::new();
        let mut add = |level, text: String| out.push(Warning { level, text });
        if let Some(c) = &self.cluster
            && !c.quorate
        {
            add(Level::Crit, t!("Cluster „{}“ hat kein Quorum", c.name));
        }
        for n in self.nodes.iter().filter(|n| !n.online) {
            add(Level::Crit, t!("Node {} ist offline", n.name));
        }
        for h in self.ha.iter().filter(|h| h.bad()) {
            let what = if h.kind == "service" {
                format!("{} ({})", h.id, h.state)
            } else {
                format!("{}: {}", h.id, h.status)
            };
            add(Level::Crit, t!("HA: {}", what));
        }
        for p in self.zfs.iter().filter(|p| !p.healthy()) {
            add(
                Level::Crit,
                t!("ZFS-Pool {} auf {} ist {}", p.name, p.node, p.health),
            );
        }
        if let Some(c) = &self.ceph {
            let level = match c.status.as_str() {
                "HEALTH_OK" => None,
                "HEALTH_WARN" => Some(Level::Warn),
                _ => Some(Level::Crit),
            };
            if let Some(level) = level {
                let detail = if c.checks.is_empty() {
                    String::new()
                } else {
                    format!(" – {}", c.checks.join("; "))
                };
                add(level, format!("Ceph {}{detail}", c.status));
            }
        }
        for s in &self.storages {
            let place = s.node.clone().unwrap_or_else(|| t!("geteilt").to_string());
            if s.enabled && !s.active {
                add(
                    Level::Warn,
                    t!("Speicher {} ({}) ist nicht verfügbar", s.id, place),
                );
            } else if s.usage() > STORAGE_WARN {
                add(
                    if s.usage() > 0.95 {
                        Level::Crit
                    } else {
                        Level::Warn
                    },
                    t!(
                        "Speicher {} ({}) ist zu {} % voll",
                        s.id,
                        place,
                        (s.usage() * 100.0).round() as u32
                    ),
                );
            }
        }
        out.sort_by_key(|w| std::cmp::Reverse(w.level));
        out
    }
}

/// Errors that make the whole request pointless (certificate, login).
fn fatal(e: &Error) -> bool {
    matches!(
        e,
        Error::Untrusted(_) | Error::PinChanged { .. } | Error::Auth(_) | Error::Net(_)
    )
}

/// Optional parts: fatal errors abort, anything else (no permission, not installed,
/// 501) is `None`.
fn optional(r: Result<Value, Error>) -> Result<Option<Value>, Error> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if fatal(&e) => Err(e),
        Err(_) => Ok(None),
    }
}

fn fetch_overview(c: &mut Client) -> Result<Overview, Error> {
    let mut ov = Overview::default();
    match c.call(false, "/cluster/status") {
        Ok(v) => {
            let (cluster, nodes) = parse_cluster_status(&v);
            ov.cluster = cluster;
            ov.nodes = nodes;
        }
        Err(e) if fatal(&e) => return Err(e),
        Err(e) => ov.problems.push(format!("/cluster/status: {e}")),
    }
    if ov.nodes.is_empty() {
        ov.nodes = super::parse_nodes(&c.call(false, "/nodes")?)
            .into_iter()
            .map(|n| NodeInfo {
                name: n.name,
                online: n.online,
                ip: String::new(),
                version: None,
                updates: None,
            })
            .collect();
    }
    if let Some(v) = optional(c.call(false, "/cluster/ha/status/current"))? {
        ov.ha = parse_ha(&v);
    }
    if let Some(v) = optional(c.call(false, "/cluster/ceph/status"))? {
        ov.ceph = parse_ceph(&v);
    }
    let mut storages = Vec::new();
    for n in ov
        .nodes
        .iter_mut()
        .filter(|n| n.online && valid_node(&n.name))
    {
        let base = format!("/nodes/{}", n.name);
        match c.call(false, &format!("{base}/storage")) {
            Ok(v) => storages.push(parse_storages(&n.name, &v)),
            Err(e) if fatal(&e) => return Err(e),
            Err(e) => ov.problems.push(format!("{}: {e}", n.name)),
        }
        if let Some(v) = optional(c.call(false, &format!("{base}/disks/zfs")))? {
            ov.zfs.extend(parse_zfs(&n.name, &v));
        }
        if let Some(v) = optional(c.call(false, &format!("{base}/version")))? {
            n.version = v["version"].as_str().map(|s| s.trim().to_string());
        }
        if let Some(v) = optional(c.call(false, &format!("{base}/apt/update")))? {
            n.updates = Some(parse_apt(&v));
        }
    }
    ov.storages = merge_storages(storages);
    Ok(ov)
}

// ------------------------------------------------------------------ watcher / alerts

#[derive(Clone, Debug, PartialEq)]
pub struct WatchGuest {
    pub vmid: u32,
    pub name: String,
    pub node: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WatchStorage {
    pub id: String,
    pub node: String,
    pub shared: bool,
    pub used: u64,
    pub total: u64,
}

/// What the background watcher reads every round: `/cluster/resources` (guests, nodes,
/// storages) and `/cluster/tasks`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Watch {
    pub guests: Vec<WatchGuest>,
    pub nodes: Vec<(String, bool)>,
    pub storages: Vec<WatchStorage>,
    pub tasks: Vec<TaskInfo>,
}

pub fn parse_watch(resources: &Value, tasks: &Value) -> Watch {
    let mut w = Watch {
        tasks: parse_tasks(tasks),
        ..Default::default()
    };
    for r in resources
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let node = r["node"].as_str().unwrap_or_default().to_string();
        match r["type"].as_str() {
            Some("qemu" | "lxc") => {
                let vmid = num(&r["vmid"]) as u32;
                if vmid > 0 && num(&r["template"]) == 0.0 {
                    w.guests.push(WatchGuest {
                        vmid,
                        name: r["name"].as_str().unwrap_or_default().to_string(),
                        node,
                        status: r["status"].as_str().unwrap_or("unknown").to_string(),
                    });
                }
            }
            Some("node") => w.nodes.push((node, r["status"].as_str() == Some("online"))),
            Some("storage") => {
                if let Some(id) = r["storage"].as_str() {
                    w.storages.push(WatchStorage {
                        id: id.to_string(),
                        node,
                        shared: num(&r["shared"]) != 0.0,
                        used: num(&r["disk"]) as u64,
                        total: num(&r["maxdisk"]) as u64,
                    });
                }
            }
            _ => {}
        }
    }
    w
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    GuestDown,
    NodeOffline,
    TaskFailed,
    StorageFull,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlertHit {
    pub kind: AlertKind,
    pub text: String,
}

/// What the watcher remembers between rounds.
#[derive(Clone, Debug, Default)]
pub struct WatchState {
    primed: bool,
    running: HashSet<u32>,
    offline: HashSet<String>,
    seen_tasks: HashSet<String>,
    full: HashSet<String>,
}

impl WatchState {
    /// A task cancelled from Aurora: its failure is no news.
    pub fn ignore_task(&mut self, upid: &str) {
        self.seen_tasks.insert(upid.to_string());
    }
}

/// Tasks that stop a guest on purpose (power actions, migration, backup in stop mode,
/// rollback, deletion).
fn stops_guest(kind: &str) -> bool {
    matches!(
        kind,
        "qmstop"
            | "qmshutdown"
            | "qmreboot"
            | "qmsuspend"
            | "qmigrate"
            | "qmrollback"
            | "qmdestroy"
            | "vzstop"
            | "vzshutdown"
            | "vzreboot"
            | "vzsuspend"
            | "vzmigrate"
            | "vzrollback"
            | "vzdestroy"
            | "vzdump"
            | "hastop"
            | "hamigrate"
            | "harelocate"
    )
}

/// Compares a new round with the last one. The first round only sets the baseline.
/// `own(vmid)`: the guest was stopped from Aurora a moment ago.
pub fn detect(
    state: &mut WatchState,
    w: &Watch,
    own: &dyn Fn(u32) -> bool,
    now: i64,
) -> Vec<AlertHit> {
    let mut out = Vec::new();
    let offline: HashSet<String> = w
        .nodes
        .iter()
        .filter(|(_, on)| !on)
        .map(|(n, _)| n.clone())
        .collect();
    let storage_key = |s: &WatchStorage| {
        if s.shared {
            s.id.clone()
        } else {
            format!("{}/{}", s.node, s.id)
        }
    };
    let usage = |s: &WatchStorage| {
        if s.total == 0 {
            0.0
        } else {
            s.used as f64 / s.total as f64
        }
    };
    let running: HashSet<u32> = w
        .guests
        .iter()
        .filter(|g| g.status == "running")
        .map(|g| g.vmid)
        .collect();
    if state.primed {
        for n in offline.difference(&state.offline) {
            out.push(AlertHit {
                kind: AlertKind::NodeOffline,
                text: t!("Node {} ist offline", n),
            });
        }
        for g in &w.guests {
            let was = state.running.contains(&g.vmid);
            let down = matches!(g.status.as_str(), "stopped" | "unknown");
            if !was || !down || offline.contains(&g.node) || own(g.vmid) {
                continue;
            }
            let intended = w
                .tasks
                .iter()
                .any(|t| t.vmid() == Some(g.vmid) && stops_guest(&t.kind) && t.start >= now - 900);
            if !intended {
                out.push(AlertHit {
                    kind: AlertKind::GuestDown,
                    text: t!(
                        "{} ({}) auf {} ist unerwartet {}",
                        g.name,
                        g.vmid,
                        g.node,
                        if g.status == "stopped" {
                            t!("gestoppt")
                        } else {
                            t!("nicht mehr erreichbar")
                        }
                    ),
                });
            }
        }
        for t in w
            .tasks
            .iter()
            .filter(|t| t.failed() && !state.seen_tasks.contains(&t.upid))
        {
            let TaskState::Failed(e) = &t.state else {
                continue;
            };
            let what = if t.id.is_empty() {
                t.kind.clone()
            } else {
                format!("{} {}", t.kind, t.id)
            };
            out.push(AlertHit {
                kind: AlertKind::TaskFailed,
                text: t!("Aufgabe {} auf {} fehlgeschlagen: {}", what, t.node, e),
            });
        }
        let mut reported = HashSet::new();
        for s in &w.storages {
            let key = storage_key(s);
            if usage(s) > STORAGE_ALERT as f64
                && !state.full.contains(&key)
                && reported.insert(key.clone())
            {
                out.push(AlertHit {
                    kind: AlertKind::StorageFull,
                    text: t!(
                        "Speicher {} ist zu {} % voll",
                        if s.shared {
                            s.id.clone()
                        } else {
                            format!("{} ({})", s.id, s.node)
                        },
                        (usage(s) * 100.0).round() as u32
                    ),
                });
            }
        }
    }
    state.primed = true;
    state.offline = offline;
    // A guest on an offline node keeps its last known state until the node is back.
    let keep: HashSet<u32> = w
        .guests
        .iter()
        .filter(|g| state.offline.contains(&g.node) && state.running.contains(&g.vmid))
        .map(|g| g.vmid)
        .collect();
    state.running = running.union(&keep).copied().collect();
    for t in w.tasks.iter().filter(|t| !t.running()) {
        state.seen_tasks.insert(t.upid.clone());
    }
    if state.seen_tasks.len() > 5000 {
        state.seen_tasks = w.tasks.iter().map(|t| t.upid.clone()).collect();
    }
    // Hysteresis: a storage alerts again only after dropping below 88 %.
    let mut peak: HashMap<String, f64> = HashMap::new();
    for s in &w.storages {
        let p = peak.entry(storage_key(s)).or_default();
        *p = p.max(usage(s));
    }
    for (key, u) in peak {
        if u > STORAGE_ALERT as f64 {
            state.full.insert(key);
        } else if u < 0.88 {
            state.full.remove(&key);
        }
    }
    out
}

// ------------------------------------------------------------------ jobs

pub enum ClusterPayload {
    Tasks(Result<Vec<TaskInfo>, Error>),
    TaskLog {
        upid: String,
        start: usize,
        result: Result<TaskLog, Error>,
    },
    Cancelled {
        upid: String,
        result: Result<(), Error>,
    },
    Overview(Result<Overview, Error>),
    Watch(Result<Watch, Error>),
    /// Package list of a node (`refreshed`: after "Aktualisieren").
    Apt {
        node: String,
        refreshed: bool,
        result: Result<Vec<AptPackage>, Error>,
    },
}

pub enum ClusterEvent {
    Pve {
        server: Uuid,
        epoch: Epoch,
        session: Option<Session>,
        payload: ClusterPayload,
    },
    Pbs {
        server: Uuid,
        epoch: Epoch,
        session: Option<Session>,
        payload: pbs::PbsPayload,
    },
    PbsTested(Result<String, Error>),
}

fn job(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    sink: Sink<ClusterEvent>,
    f: impl FnOnce(&mut Client) -> ClusterPayload + Send + 'static,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let payload = f(&mut c);
        sink.send(ClusterEvent::Pve {
            server,
            epoch,
            session: c.session.clone(),
            payload,
        });
    });
}

fn node_names(c: &mut Client) -> Result<Vec<String>, Error> {
    Ok(super::parse_nodes(&c.call(false, "/nodes")?)
        .into_iter()
        .filter(|n| n.online && valid_node(&n.name))
        .map(|n| n.name)
        .collect())
}

fn fetch_tasks(c: &mut Client) -> Result<Vec<TaskInfo>, Error> {
    let mut lists = vec![parse_tasks(&c.call(false, "/cluster/tasks")?)];
    for node in node_names(c)? {
        if let Some(v) = optional(c.call(false, &format!("/nodes/{node}/tasks?limit=50&errors=1")))?
        {
            lists.push(parse_tasks(&v));
        }
    }
    Ok(merge_tasks(lists))
}

/// Running and recent tasks of the cluster, plus the last failures of every node.
pub fn spawn_tasks(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, |c| {
        ClusterPayload::Tasks(fetch_tasks(c))
    });
}

/// Log lines from `start` on, and whether the task still runs.
pub fn spawn_task_log(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    upid: String,
    start: usize,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, move |c| {
        let result = (|| {
            let base =
                task_path(&upid).ok_or_else(|| Error::Net(t!("ungültige Task-ID").into()))?;
            let lines = parse_log(&c.call(false, &format!("{base}/log?start={start}&limit=500"))?);
            let st = c.call(false, &format!("{base}/status"))?;
            let running = st["status"].as_str() != Some("stopped");
            Ok(TaskLog {
                lines,
                running,
                exit: (!running).then(|| st["exitstatus"].as_str().unwrap_or_default().to_string()),
            })
        })();
        ClusterPayload::TaskLog {
            upid,
            start,
            result,
        }
    });
}

/// Stops a running task (`DELETE /nodes/{node}/tasks/{upid}`).
pub fn spawn_cancel_task(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    upid: String,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, move |c| {
        let result = match task_path(&upid) {
            Some(path) => c.request(Method::Delete, &path, &[]).map(|_| ()),
            None => Err(Error::Net(t!("ungültige Task-ID").into())),
        };
        ClusterPayload::Cancelled { upid, result }
    });
}

pub fn spawn_overview(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, |c| {
        ClusterPayload::Overview(fetch_overview(c))
    });
}

pub fn spawn_watch(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, |c| {
        let result = (|| {
            let res = c.call(false, "/cluster/resources")?;
            let tasks = c.call(false, "/cluster/tasks")?;
            Ok(parse_watch(&res, &tasks))
        })();
        ClusterPayload::Watch(result)
    });
}

/// Upgradable packages of a node; with `refresh`, runs `apt update` first (as a task).
pub fn spawn_apt(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    node: String,
    refresh: bool,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, move |c| {
        let result = (|| {
            if !valid_node(&node) {
                return Err(Error::Net(t!("ungültiger Node-Name").into()));
            }
            let path = format!("/nodes/{node}/apt/update");
            if refresh {
                let upid = c.call(true, &path)?;
                if let Some(base) = upid.as_str().and_then(task_path) {
                    let started = Instant::now();
                    loop {
                        std::thread::sleep(Duration::from_millis(1200));
                        let st = c.call(false, &format!("{base}/status"))?;
                        if st["status"].as_str() == Some("stopped") {
                            task_result(st["exitstatus"].as_str().unwrap_or_default())
                                .map_err(Error::Net)?;
                            break;
                        }
                        if started.elapsed() > Duration::from_secs(600) {
                            return Err(Error::Net(t!("Zeitüberschreitung").into()));
                        }
                    }
                }
            }
            Ok(parse_apt(&c.call(false, &path)?))
        })();
        ClusterPayload::Apt {
            node,
            refreshed: refresh,
            result,
        }
    });
}

/// The fixed upgrade line for "Upgrade über SSH": runs on the configured SSH host and
/// hops to the node over the cluster's root SSH trust when it is another one.
pub fn upgrade_command(node: &str) -> Option<String> {
    if !valid_node(node) {
        return None;
    }
    let cmd = "apt-get update && apt-get dist-upgrade";
    Some(format!(
        "if [ \"$(hostname)\" = '{node}' ]; then {cmd}; else ssh -t root@{node} '{cmd}'; fi"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const UPID: &str = "UPID:pve1:0012A3F1:0B2C3D4E:66AABBCC:vzdump:100:root@pam:";

    #[test]
    fn upids() {
        let u = parse_upid(UPID).unwrap();
        assert_eq!(
            (u.node.as_str(), u.kind.as_str(), u.id.as_str()),
            ("pve1", "vzdump", "100")
        );
        assert_eq!(u.user, "root@pam");
        assert_eq!(u.start, 0x66AABBCC);
        assert!(valid_upid(
            "UPID:pve2:00001000:00000000:66AABBCC:aptupdate::root@pam!aurora:"
        ));
        for bad in [
            "",
            "UPID:pve1",
            "UPID:../x:0:0:0:vzdump:1:root@pam:",
            "UPID:pve1:0:0:zz:vzdump:1:root@pam:",
            "UPID:pve1:0:0:1:vzdump:1:root@pam:/../../etc",
            "UPID:pve 1:0:0:1:vzdump:1:root@pam:",
            "XPID:pve1:0:0:1:vzdump:1:root@pam:",
        ] {
            assert!(!valid_upid(bad), "{bad}");
        }
        assert_eq!(
            task_path(UPID).unwrap(),
            "/nodes/pve1/tasks/UPID%3Apve1%3A0012A3F1%3A0B2C3D4E%3A66AABBCC%3Avzdump%3A100%3Aroot%40pam%3A"
        );
        assert_eq!(task_path("UPID:x;rm:1:1:1:a:b:c:"), None);
    }

    #[test]
    fn tasks_parse_and_merge() {
        let cluster = json!([
            {"upid": UPID, "node":"pve1","type":"vzdump","id":"100","user":"root@pam","starttime":100,"endtime":160,"status":"OK"},
            {"upid":"UPID:pve2:00000001:00000000:000000C8:qmigrate:200:admin@pve:","node":"pve2","type":"qmigrate","id":"200","user":"admin@pve","starttime":200},
            {"upid":"UPID:pve1:00000002:00000000:00000096:qmclone:9000:root@pam:","starttime":150,"endtime":151,"status":"WARNINGS: 1"},
            {"upid":"bogus"}
        ]);
        let node = json!([
            {"upid":"UPID:pve1:00000003:00000000:00000032:vzdump:101:root@pam:","node":"pve1","type":"vzdump","id":"101","user":"root@pam","starttime":50,"endtime":70,"status":"job errors"},
            {"upid": UPID, "node":"pve1","type":"vzdump","id":"100","user":"root@pam","starttime":100}
        ]);
        let all = merge_tasks(vec![parse_tasks(&cluster), parse_tasks(&node)]);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].kind, "qmigrate");
        assert!(all[0].running());
        assert_eq!(all[0].duration(260), 60);
        assert_eq!(all[1].state, TaskState::Warnings("WARNINGS: 1".into()));
        assert_eq!(all[1].node, "pve1");
        assert_eq!(all[2].state, TaskState::Ok); // finished entry wins over the stale one
        assert_eq!(all[2].vmid(), Some(100));
        assert_eq!(all[3].state, TaskState::Failed("job errors".into()));
        assert!(all[3].failed());
        assert_eq!(
            parse_log(&json!([{"n":1,"t":"INFO: start"},{"n":2,"t":"TASK OK"}])),
            vec!["INFO: start", "TASK OK"]
        );
    }

    #[test]
    fn overview_and_warnings() {
        let (cluster, nodes) = parse_cluster_status(&json!([
            {"type":"cluster","name":"lab","nodes":3,"quorate":0,"version":7},
            {"type":"node","name":"pve2","online":0,"ip":"10.0.0.2"},
            {"type":"node","name":"pve1","online":1,"ip":"10.0.0.1","local":1}
        ]));
        let cluster = cluster.unwrap();
        assert_eq!(
            (cluster.name.as_str(), cluster.quorate, cluster.nodes),
            ("lab", false, 3)
        );
        assert_eq!(nodes[0].name, "pve1");
        assert!(nodes[0].online && !nodes[1].online);

        let s1 = parse_storages(
            "pve1",
            &json!([
                {"storage":"local","type":"dir","content":"iso","active":1,"enabled":1,"used":90,"total":100,"shared":0},
                {"storage":"ceph","type":"rbd","content":"images","active":1,"enabled":1,"used":10,"total":100,"shared":1},
                {"storage":"nas","type":"nfs","content":"backup","active":0,"enabled":1,"shared":1}
            ]),
        );
        let s2 = parse_storages(
            "pve2",
            &json!([
                {"storage":"local","type":"dir","used":10,"total":100},
                {"storage":"ceph","type":"rbd","used":10,"total":100,"shared":1},
                {"storage":"nas","type":"nfs","active":1,"used":5,"total":10,"shared":1}
            ]),
        );
        let st = merge_storages(vec![s1, s2]);
        let names: Vec<(String, Option<String>)> =
            st.iter().map(|s| (s.id.clone(), s.node.clone())).collect();
        assert_eq!(
            names,
            vec![
                ("ceph".into(), None),
                ("nas".into(), None),
                ("local".into(), Some("pve1".into())),
                ("local".into(), Some("pve2".into())),
            ]
        );
        assert!(st[1].active, "active view of a shared storage wins");

        let zfs = parse_zfs(
            "pve1",
            &json!([{"name":"rpool","health":"ONLINE","size":100,"alloc":40,"frag":3},
                    {"name":"tank","health":"DEGRADED","size":100,"alloc":40}]),
        );
        assert!(zfs[0].healthy() && !zfs[1].healthy());
        let ceph = parse_ceph(&json!({"health":{"status":"HEALTH_WARN","checks":{
            "OSD_DOWN":{"severity":"HEALTH_WARN","summary":{"message":"1 osds down"}}}}}))
        .unwrap();
        assert_eq!(ceph.checks, vec!["1 osds down"]);
        assert_eq!(parse_ceph(&json!(null)), None);
        let ha = parse_ha(&json!([
            {"id":"quorum","type":"quorum","status":"OK"},
            {"id":"service:vm:100","type":"service","state":"error","node":"pve1","status":"error"},
            {"id":"service:vm:101","type":"service","state":"started","node":"pve1"}
        ]));
        assert_eq!(ha.iter().filter(|h| h.bad()).count(), 1);

        let ov = Overview {
            cluster: Some(cluster),
            nodes,
            ha,
            storages: st,
            zfs,
            ceph: Some(ceph),
            problems: vec![],
        };
        let w = ov.warnings();
        let texts: Vec<&str> = w.iter().map(|w| w.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("kein Quorum")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("pve2 ist offline")));
        assert!(
            texts
                .iter()
                .any(|t| t.contains("tank") && t.contains("DEGRADED"))
        );
        assert!(texts.iter().any(|t| t.contains("HEALTH_WARN")));
        assert!(
            texts
                .iter()
                .any(|t| t.contains("local (pve1)") && t.contains("90 %"))
        );
        assert!(texts.iter().any(|t| t.contains("service:vm:100")));
        assert!(!texts.iter().any(|t| t.contains("ceph (")));
        // Critical first.
        assert_eq!(w[0].level, Level::Crit);
        assert_eq!(w.last().unwrap().level, Level::Warn);
    }

    #[test]
    fn apt_packages() {
        let p = parse_apt(&json!([
            {"Package":"pve-manager","OldVersion":"8.2.2","Version":"8.2.4","Origin":"Proxmox"},
            {"Package":"curl","OldVersion":"7.88.1-10","Version":"7.88.1-10+deb12u7","Origin":"Debian"},
            {"Title":"no name"}
        ]));
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].name.as_str(), p[0].new.as_str()),
            ("curl", "7.88.1-10+deb12u7")
        );
        assert_eq!(
            upgrade_command("pve1").unwrap(),
            "if [ \"$(hostname)\" = 'pve1' ]; then apt-get update && apt-get dist-upgrade; else ssh -t root@pve1 'apt-get update && apt-get dist-upgrade'; fi"
        );
        assert_eq!(upgrade_command("pve1'; rm -rf /"), None);
    }

    fn res(status_103: &str, pve2: &str, used: u64) -> Value {
        json!([
            {"type":"qemu","vmid":103,"name":"win","node":"pve1","status":status_103},
            {"type":"qemu","vmid":200,"name":"db","node":"pve2","status":if pve2 == "online" {"running"} else {"unknown"}},
            {"type":"lxc","vmid":102,"name":"mqtt","node":"pve1","status":"running"},
            {"type":"qemu","vmid":9000,"name":"tpl","node":"pve1","status":"stopped","template":1},
            {"type":"node","node":"pve1","status":"online"},
            {"type":"node","node":"pve2","status":pve2},
            {"type":"storage","storage":"local","node":"pve1","disk":used,"maxdisk":100,"shared":0},
            {"type":"storage","storage":"nfs","node":"pve1","disk":95,"maxdisk":100,"shared":1},
            {"type":"storage","storage":"nfs","node":"pve2","disk":95,"maxdisk":100,"shared":1}
        ])
    }

    #[test]
    fn alerts() {
        let mut st = WatchState::default();
        let none = |_: u32| false;
        let failed_old = json!([{"upid":"UPID:pve1:1:0:00000001:vzdump:101:root@pam:","starttime":1,"endtime":2,"status":"failed"}]);
        // First round: baseline only (old failure and the full NFS are not reported).
        let w = parse_watch(&res("running", "online", 10), &failed_old);
        assert_eq!(w.guests.len(), 3);
        assert!(detect(&mut st, &w, &none, 1000).is_empty());

        // 103 crashed, 102 was stopped by a task, pve2 went offline (200 → unknown is
        // covered by the node alert), local storage filled up, a new backup failed.
        let tasks = json!([
            {"upid":"UPID:pve1:1:0:00000001:vzdump:101:root@pam:","starttime":1,"endtime":2,"status":"failed"},
            {"upid":"UPID:pve1:2:0:000003E0:vzstop:102:root@pam:","starttime":992,"endtime":993,"status":"OK"},
            {"upid":"UPID:pve1:3:0:000003E1:vzdump:100:root@pam:","starttime":993,"endtime":999,"status":"ERROR: backup failed"},
            {"upid":"UPID:pve1:4:0:000003E2:qmstart:100:root@pam:","starttime":994}
        ]);
        let mut res2 = res("stopped", "offline", 92);
        res2.as_array_mut().unwrap()[2]["status"] = json!("stopped");
        let w = parse_watch(&res2, &tasks);
        let hits = detect(&mut st, &w, &none, 1000);
        let kinds: Vec<AlertKind> = hits.iter().map(|h| h.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AlertKind::NodeOffline,
                AlertKind::GuestDown,
                AlertKind::TaskFailed,
                AlertKind::StorageFull
            ],
            "{hits:?}"
        );
        assert!(hits[1].text.contains("win (103)"));
        assert!(hits[2].text.contains("backup failed"));
        assert!(hits[3].text.contains("local (pve1)") && hits[3].text.contains("92 %"));

        // Same state again: nothing new.
        assert!(detect(&mut st, &w, &none, 1010).is_empty());

        // 103 back up and down again, but stopped from Aurora: no alert.
        let w = parse_watch(&res("running", "online", 92), &tasks);
        assert!(detect(&mut st, &w, &none, 1020).is_empty());
        let w = parse_watch(&res("stopped", "online", 92), &tasks);
        assert!(detect(&mut st, &w, &|v| v == 103, 1030).is_empty());

        // A task cancelled from Aurora fails without an alert.
        let cancelled = "UPID:pve2:5:0:00000400:qmclone:9000:root@pam:";
        st.ignore_task(cancelled);
        let tasks = json!([{"upid": cancelled, "starttime":1024, "endtime":1030, "status":"interrupted by signal"}]);
        let w = parse_watch(&res("stopped", "online", 92), &tasks);
        assert!(detect(&mut st, &w, &|v| v == 103, 1040).is_empty());
    }
}
