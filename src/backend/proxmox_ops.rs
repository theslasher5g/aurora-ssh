//! Guest operations beyond power actions: snapshots, backups and restore (tracked as
//! Proxmox tasks), the best address of a guest, and the browser / SPICE consoles.

use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use super::{Client, Conn, Error, Kind, Method, PveEvent, Sink, num, valid_node};

/// Generation of a server's live state. Results of jobs started under an older one
/// (before a lock, an edit or a delete) are dropped.
pub type Epoch = u64;

/// One guest as the API addresses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestId {
    pub node: String,
    pub kind: Kind,
    pub vmid: u32,
}

impl GuestId {
    /// `/nodes/{node}/{qemu|lxc}/{vmid}`, or `None` for a node name unfit for a URL.
    fn base(&self) -> Option<String> {
        valid_node(&self.node)
            .then(|| format!("/nodes/{}/{}/{}", self.node, self.kind.api(), self.vmid))
    }
}

// ------------------------------------------------------------------ addresses

/// An address as a guest reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct IpCandidate {
    pub ip: IpAddr,
    pub iface: String,
    pub prefix: Option<u8>,
}

/// `"10.0.0.5/24"` or `"10.0.0.5"` → candidate; anything that is no IP address is dropped.
fn candidate(text: &str, iface: &str, prefix: Option<u8>) -> Option<IpCandidate> {
    let mut parts = text.trim().splitn(2, '/');
    let ip: IpAddr = parts.next()?.trim().parse().ok()?;
    let prefix = parts
        .next()
        .and_then(|p| p.trim().parse::<u8>().ok())
        .or(prefix);
    Some(IpCandidate {
        ip,
        iface: iface.to_string(),
        prefix,
    })
}

/// `data` of `qemu/{vmid}/agent/network-get-interfaces`.
pub fn agent_candidates(data: &Value) -> Vec<IpCandidate> {
    let list = data.get("result").unwrap_or(data);
    let mut out = Vec::new();
    for iface in list.as_array().map(|a| a.as_slice()).unwrap_or_default() {
        let name = iface["name"].as_str().unwrap_or_default();
        for a in iface["ip-addresses"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
        {
            let prefix = a["prefix"].as_u64().and_then(|p| u8::try_from(p).ok());
            if let Some(c) = a["ip-address"]
                .as_str()
                .and_then(|ip| candidate(ip, name, prefix))
            {
                out.push(c);
            }
        }
    }
    out
}

/// `data` of `lxc/{vmid}/interfaces`.
pub fn lxc_candidates(data: &Value) -> Vec<IpCandidate> {
    let mut out = Vec::new();
    for iface in data.as_array().map(|a| a.as_slice()).unwrap_or_default() {
        let name = iface["name"].as_str().unwrap_or_default();
        for key in ["inet", "inet6"] {
            if let Some(s) = iface[key].as_str() {
                out.extend(
                    s.split([',', ' '])
                        .filter(|p| !p.is_empty())
                        .filter_map(|p| candidate(p, name, None)),
                );
            }
        }
        for a in iface["ip-addresses"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
        {
            let prefix = a["prefix"].as_u64().and_then(|p| u8::try_from(p).ok());
            if let Some(c) = a["ip-address"]
                .as_str()
                .and_then(|ip| candidate(ip, name, prefix))
            {
                out.push(c);
            }
        }
    }
    out
}

/// Static addresses from the guest config: CT `netN: name=eth0,…,ip=10.0.0.5/24`,
/// VM cloud-init `ipconfigN: ip=10.0.0.5/24,gw=…`. `dhcp`/`manual` are no addresses.
pub fn config_candidates(data: &Value, kind: Kind) -> Vec<IpCandidate> {
    let prefix = match kind {
        Kind::Lxc => "net",
        Kind::Qemu => "ipconfig",
    };
    let Some(obj) = data.as_object() else {
        return Vec::new();
    };
    let mut keys: Vec<(u32, &str)> = obj
        .keys()
        .filter_map(|k| Some((k.strip_prefix(prefix)?.parse::<u32>().ok()?, k.as_str())))
        .collect();
    keys.sort();
    let mut out = Vec::new();
    for (n, key) in keys {
        let Some(value) = obj[key].as_str() else {
            continue;
        };
        let mut iface = match kind {
            Kind::Lxc => format!("eth{n}"),
            Kind::Qemu => format!("net{n}"),
        };
        let mut ips = Vec::new();
        for part in value.split(',') {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            match k.trim() {
                "name" => iface = v.trim().to_string(),
                "ip" | "ip6" => ips.push(v.trim().to_string()),
                _ => {}
            }
        }
        out.extend(ips.iter().filter_map(|ip| candidate(ip, &iface, None)));
    }
    out
}

/// Addresses nobody can connect to from outside the guest.
fn usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// Container runtimes, libvirt NAT and overlay networks: addresses only the guest itself
/// can use.
fn internal_iface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "lo"
        || n.starts_with("docker")
        || n.starts_with("br-")
        || n.starts_with("veth")
        || n.starts_with("virbr")
        || n.starts_with("cni")
        || n.starts_with("flannel")
        || n.starts_with("cali")
        || n.starts_with("cilium")
        || n.starts_with("vxlan")
        || n.starts_with("kube")
        || n.starts_with("weave")
        || n.starts_with("podman")
        || n.starts_with("lxcbr")
        || n.starts_with("lxdbr")
}

/// VPN overlays: usable, but only when nothing on the LAN is.
fn overlay_iface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("tailscale") || n.starts_with("zt") || n.starts_with("wg")
}

fn same_subnet(a: &IpAddr, b: &IpAddr, prefix: Option<u8>) -> bool {
    match (a, b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let p = prefix.unwrap_or(24).clamp(8, 32) as u32;
            let mask = u32::MAX.checked_shl(32 - p).unwrap_or(0);
            (u32::from(*a) & mask) == (u32::from(*b) & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let p = prefix.unwrap_or(64).clamp(16, 128) as u32;
            let mask = u128::MAX.checked_shl(128 - p).unwrap_or(0);
            (u128::from(*a) & mask) == (u128::from(*b) & mask)
        }
        _ => false,
    }
}

/// Usable addresses, best first: IPv4 before IPv6, the server's subnet first, LAN before
/// VPN overlays; loopback, link-local and container-internal interfaces are dropped.
pub fn rank_ips(cands: &[IpCandidate], near: Option<IpAddr>) -> Vec<IpAddr> {
    let mut keep: Vec<(usize, &IpCandidate)> = cands
        .iter()
        .enumerate()
        .filter(|(_, c)| usable(&c.ip) && !internal_iface(&c.iface))
        .collect();
    keep.sort_by_key(|(i, c)| {
        let far = near.is_none_or(|n| !same_subnet(&c.ip, &n, c.prefix));
        (c.ip.is_ipv6(), far, overlay_iface(&c.iface), *i)
    });
    let mut out: Vec<IpAddr> = Vec::new();
    for (_, c) in keep {
        if !out.contains(&c.ip) {
            out.push(c.ip);
        }
    }
    out
}

/// The server's own address, to prefer guest addresses in the same subnet.
pub fn host_ip(host: &str, port: u16) -> Option<IpAddr> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse() {
        return Some(ip);
    }
    let addrs: Vec<IpAddr> = (host, port)
        .to_socket_addrs()
        .ok()?
        .map(|a| a.ip())
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
}

// ------------------------------------------------------------------ snapshots

/// One entry of `…/snapshot`, in tree order.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapInfo {
    pub name: String,
    pub description: String,
    /// Unix time; `None` for the "current" entry.
    pub time: Option<i64>,
    pub parent: Option<String>,
    /// VM snapshot including RAM.
    pub vmstate: bool,
    /// The "you are here" entry (`current`).
    pub current: bool,
    /// Depth in the parent tree (0 = root).
    pub depth: usize,
}

/// Proxmox snapshot names: a letter, then letters, digits, `_` or `-`; 2–40 characters.
pub fn valid_snapname(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && (2..=40).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && name != "current"
        && name != "vzdump"
}

/// `data` of `…/snapshot`: a parent tree, flattened depth-first, oldest first.
pub fn parse_snapshots(data: &Value) -> Vec<SnapInfo> {
    let mut all: Vec<SnapInfo> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let name = s["name"].as_str()?.to_string();
            let current = name == "current";
            Some(SnapInfo {
                description: s["description"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                time: s["snaptime"].as_i64().or_else(|| {
                    s["snaptime"]
                        .as_str()
                        .and_then(|t| t.parse().ok())
                        .filter(|_| !current)
                }),
                parent: s["parent"]
                    .as_str()
                    .filter(|p| !p.is_empty())
                    .map(str::to_string),
                vmstate: num(&s["vmstate"]) != 0.0,
                current,
                depth: 0,
                name,
            })
        })
        .collect();
    // "current" last among siblings, everything else by time.
    all.sort_by_key(|s| (s.current, s.time.unwrap_or(i64::MAX)));
    let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
    let is_root = |s: &SnapInfo| s.parent.as_ref().is_none_or(|p| !names.contains(p));
    let mut out = Vec::with_capacity(all.len());
    let mut stack: Vec<(usize, usize)> = all
        .iter()
        .enumerate()
        .filter(|(_, s)| is_root(s))
        .map(|(i, _)| (i, 0))
        .rev()
        .collect();
    let mut seen = vec![false; all.len()];
    while let Some((i, depth)) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let mut s = all[i].clone();
        s.depth = depth;
        let name = s.name.clone();
        out.push(s);
        let children: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(j, c)| !seen[*j] && c.parent.as_deref() == Some(name.as_str()))
            .map(|(j, _)| j)
            .collect();
        stack.extend(children.into_iter().rev().map(|j| (j, depth + 1)));
    }
    // Cycles (broken configs) would leave entries out: append them flat.
    for (i, s) in all.into_iter().enumerate() {
        if !seen[i] {
            out.push(s);
        }
    }
    out
}

// ------------------------------------------------------------------ backups

/// A storage that can hold backups.
#[derive(Clone, Debug, PartialEq)]
pub struct BackupStore {
    pub id: String,
    pub kind: String,
    pub avail: u64,
    pub total: u64,
}

/// One backup archive of a guest.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub volid: String,
    pub storage: String,
    pub ctime: i64,
    pub size: u64,
    pub notes: String,
    pub protected: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Backups {
    pub stores: Vec<BackupStore>,
    pub backups: Vec<Backup>,
    /// Problems with single storages (offline, no permission) – the rest still shows.
    pub errors: Vec<String>,
}

/// Storage IDs go into URL paths: Proxmox allows letters, digits, `-`, `_` and `.`.
pub fn valid_storage(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// `storage:backup/vzdump-qemu-100-….vma.zst` (or a PBS `storage:backup/vm/100/…`).
pub fn valid_volid(volid: &str) -> bool {
    volid.split_once(':').is_some_and(|(store, rest)| {
        valid_storage(store)
            && !rest.is_empty()
            && rest.len() < 512
            && !rest.contains("..")
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:+=".contains(c))
    })
}

/// `data` of `/nodes/{node}/storage?content=backup`.
pub fn parse_backup_stores(data: &Value) -> Vec<BackupStore> {
    let mut out: Vec<BackupStore> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|s| {
            s["content"]
                .as_str()
                .is_none_or(|c| c.split(',').any(|x| x.trim() == "backup"))
        })
        .filter(|s| s.get("enabled").is_none_or(|e| num(e) != 0.0))
        .filter(|s| s.get("active").is_none_or(|e| num(e) != 0.0))
        .filter_map(|s| {
            let id = s["storage"].as_str()?;
            valid_storage(id).then(|| BackupStore {
                id: id.to_string(),
                kind: s["type"].as_str().unwrap_or_default().to_string(),
                avail: num(&s["avail"]) as u64,
                total: num(&s["total"]) as u64,
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// `data` of `/nodes/{node}/storage/{storage}/content?content=backup&vmid=…`.
pub fn parse_backups(data: &Value, storage: &str, vmid: u32) -> Vec<Backup> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|b| b["content"].as_str().is_none_or(|c| c == "backup"))
        .filter(|b| match b.get("vmid") {
            Some(v) if !v.is_null() => num(v) as u32 == vmid,
            _ => true,
        })
        .filter_map(|b| {
            let volid = b["volid"].as_str()?;
            valid_volid(volid).then(|| Backup {
                volid: volid.to_string(),
                storage: storage.to_string(),
                ctime: num(&b["ctime"]) as i64,
                size: num(&b["size"]) as u64,
                notes: b["notes"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                protected: num(&b["protected"]) != 0.0,
            })
        })
        .collect()
}

/// `vzdump` modes.
pub const BACKUP_MODES: [&str; 3] = ["snapshot", "suspend", "stop"];

// ------------------------------------------------------------------ tasks

/// Method, path and form parameters of an API call.
type Request = (Method, String, Vec<(&'static str, String)>);

/// An action that Proxmox runs as a task (answers with a UPID).
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Snapshot {
        name: String,
        description: String,
        vmstate: bool,
    },
    Rollback(String),
    DeleteSnapshot(String),
    Backup {
        storage: String,
        mode: String,
    },
    /// Restore an archive as a new guest (`vmid` must be free – no `force`).
    Restore {
        volid: String,
        vmid: u32,
    },
}

impl Op {
    /// Method, path and form of the request; `None` for unsafe input.
    fn request(&self, g: &GuestId) -> Option<Request> {
        let base = g.base()?;
        Some(match self {
            Op::Snapshot {
                name,
                description,
                vmstate,
            } => {
                if !valid_snapname(name) {
                    return None;
                }
                let mut form = vec![("snapname", name.clone())];
                if !description.trim().is_empty() {
                    form.push(("description", description.trim().to_string()));
                }
                if *vmstate && g.kind == Kind::Qemu {
                    form.push(("vmstate", "1".into()));
                }
                (Method::Post, format!("{base}/snapshot"), form)
            }
            Op::Rollback(name) => {
                if !valid_snapname(name) {
                    return None;
                }
                (
                    Method::Post,
                    format!("{base}/snapshot/{name}/rollback"),
                    vec![],
                )
            }
            Op::DeleteSnapshot(name) => {
                if !valid_snapname(name) {
                    return None;
                }
                (Method::Delete, format!("{base}/snapshot/{name}"), vec![])
            }
            Op::Backup { storage, mode } => {
                if !valid_storage(storage) || !BACKUP_MODES.contains(&mode.as_str()) {
                    return None;
                }
                (
                    Method::Post,
                    format!("/nodes/{}/vzdump", g.node),
                    vec![
                        ("vmid", g.vmid.to_string()),
                        ("storage", storage.clone()),
                        ("mode", mode.clone()),
                        ("compress", "zstd".into()),
                    ],
                )
            }
            Op::Restore { volid, vmid } => {
                if !valid_volid(volid) || *vmid < 100 || *vmid == g.vmid {
                    return None;
                }
                let mut form = vec![("vmid", vmid.to_string())];
                match g.kind {
                    Kind::Qemu => form.push(("archive", volid.clone())),
                    Kind::Lxc => {
                        form.push(("ostemplate", volid.clone()));
                        form.push(("restore", "1".into()));
                    }
                }
                // New MAC addresses: the copy may run next to the original.
                form.push(("unique", "1".into()));
                (
                    Method::Post,
                    format!("/nodes/{}/{}", g.node, g.kind.api()),
                    form,
                )
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskUpdate {
    /// The task runs: its UPID.
    Started(String),
    /// Last line of the task log.
    Progress(String),
    /// Finished: `Ok(exitstatus)` (`OK`, `WARNINGS: n`) or the error.
    Done(Result<String, String>),
}

/// Percent-encoding for a path segment (UPIDs contain `:` and `@`).
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `exitstatus` of a stopped task → success (`OK`, `WARNINGS: …`) or error text.
pub fn task_result(exitstatus: &str) -> Result<String, String> {
    let e = exitstatus.trim();
    if e == "OK" || e.starts_with("WARNINGS") {
        Ok(e.to_string())
    } else if e.is_empty() {
        Err(t!("unbekannter Fehler").to_string())
    } else {
        Err(e.to_string())
    }
}

/// Everything a guest job reports back.
#[derive(Clone, Debug)]
pub enum GuestPayload {
    Snapshots(Result<Vec<SnapInfo>, Error>),
    Backups(Result<Backups, Error>),
    NextId(Result<u32, Error>),
    Task {
        task: u64,
        update: TaskUpdate,
    },
    Address(Result<Option<IpAddr>, Error>),
    /// `.vv` settings from `spiceproxy`, or why there is no SPICE display.
    Spice(Result<Vec<(String, String)>, Error>),
}

fn send(sink: &Sink<PveEvent>, server: Uuid, epoch: Epoch, c: &Client, vmid: u32, p: GuestPayload) {
    sink.send(PveEvent::Guest {
        server,
        epoch,
        vmid,
        session: c.session.clone(),
        payload: p,
    });
}

/// Starts `op`, then polls the task until it stops (or `cancel` is set).
#[allow(clippy::too_many_arguments)]
pub fn spawn_task(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    op: Op,
    task: u64,
    cancel: Arc<AtomicBool>,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let update = |c: &Client, u: TaskUpdate| {
            send(
                &sink,
                server,
                epoch,
                c,
                guest.vmid,
                GuestPayload::Task { task, update: u },
            );
        };
        let Some((method, path, form)) = op.request(&guest) else {
            update(&c, TaskUpdate::Done(Err(t!("ungültige Eingabe").into())));
            return;
        };
        let upid = match c.request(method, &path, &form) {
            Ok(v) => v.as_str().unwrap_or_default().to_string(),
            Err(e) => {
                update(&c, TaskUpdate::Done(Err(e.to_string())));
                return;
            }
        };
        if !upid.starts_with("UPID:") {
            // Some calls finish synchronously (no task).
            update(&c, TaskUpdate::Done(Ok("OK".into())));
            return;
        }
        update(&c, TaskUpdate::Started(upid.clone()));
        // The task runs on the node named in the UPID.
        let node = upid
            .split(':')
            .nth(1)
            .filter(|n| valid_node(n))
            .unwrap_or(&guest.node)
            .to_string();
        let status_path = format!("/nodes/{node}/tasks/{}/status", enc(&upid));
        let log_path = format!("/nodes/{node}/tasks/{}/log", enc(&upid));
        let started = Instant::now();
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
            if let Ok(lines) = c.call(false, &format!("{log_path}?start={log_line}&limit=200"))
                && let Some(a) = lines.as_array()
            {
                log_line += a.len() as u64;
                if let Some(last) = a
                    .iter()
                    .filter_map(|l| l["t"].as_str())
                    .rfind(|t| !t.trim().is_empty() && *t != "no content")
                {
                    update(
                        &c,
                        TaskUpdate::Progress(last.trim().chars().take(160).collect()),
                    );
                }
            }
            match c.call(false, &status_path) {
                Ok(st) => {
                    failures = 0;
                    if st["status"].as_str() == Some("stopped") {
                        let exit = st["exitstatus"].as_str().unwrap_or_default();
                        update(&c, TaskUpdate::Done(task_result(exit)));
                        return;
                    }
                }
                Err(e) => {
                    failures += 1;
                    if failures >= 5 {
                        update(&c, TaskUpdate::Done(Err(e.to_string())));
                        return;
                    }
                }
            }
            if started.elapsed() > Duration::from_secs(12 * 3600) {
                update(
                    &c,
                    TaskUpdate::Done(Err(
                        t!("Aufgabe läuft noch – Status nicht mehr verfolgt").into()
                    )),
                );
                return;
            }
        }
    });
}

pub fn spawn_snapshots(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = match guest.base() {
            Some(base) => c
                .call(false, &format!("{base}/snapshot"))
                .map(|d| parse_snapshots(&d)),
            None => Err(Error::Net(t!("ungültiger Node-Name").into())),
        };
        send(
            &sink,
            server,
            epoch,
            &c,
            guest.vmid,
            GuestPayload::Snapshots(result),
        );
    });
}

/// Backup storages of the guest's node and the guest's archives on them.
pub fn spawn_backups(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = (|| {
            if !valid_node(&guest.node) {
                return Err(Error::Net(t!("ungültiger Node-Name").into()));
            }
            let node = &guest.node;
            let stores = parse_backup_stores(
                &c.call(false, &format!("/nodes/{node}/storage?content=backup"))?,
            );
            let mut out = Backups::default();
            for s in &stores {
                let path = format!(
                    "/nodes/{node}/storage/{}/content?content=backup&vmid={}",
                    enc(&s.id),
                    guest.vmid
                );
                match c.call(false, &path) {
                    Ok(d) => out.backups.extend(parse_backups(&d, &s.id, guest.vmid)),
                    Err(e @ (Error::Untrusted(_) | Error::PinChanged { .. })) => return Err(e),
                    Err(e) => out.errors.push(format!("{}: {e}", s.id)),
                }
            }
            out.backups
                .sort_by(|a, b| b.ctime.cmp(&a.ctime).then(a.volid.cmp(&b.volid)));
            out.stores = stores;
            Ok(out)
        })();
        send(
            &sink,
            server,
            epoch,
            &c,
            guest.vmid,
            GuestPayload::Backups(result),
        );
    });
}

/// Next free VMID of the cluster (for a restore as a new guest).
pub fn spawn_nextid(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    vmid: u32,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = c.call(false, "/cluster/nextid").and_then(|v| {
            let id = num(&v) as u32;
            if id >= 100 {
                Ok(id)
            } else {
                Err(Error::Net(t!("keine freie VMID").into()))
            }
        });
        send(&sink, server, epoch, &c, vmid, GuestPayload::NextId(result));
    });
}

/// Looks up the guest's addresses right now (agent / CT interfaces), falling back to
/// the static config, and picks the best one.
pub fn spawn_address(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let near = host_ip(&conn.host, conn.port);
        let mut c = Client::new(&conn);
        let result = (|| {
            let Some(base) = guest.base() else {
                return Err(Error::Net(t!("ungültiger Node-Name").into()));
            };
            let live = match guest.kind {
                Kind::Qemu => c
                    .call(false, &format!("{base}/agent/network-get-interfaces"))
                    .map(|d| agent_candidates(&d)),
                Kind::Lxc => c
                    .call(false, &format!("{base}/interfaces"))
                    .map(|d| lxc_candidates(&d)),
            };
            let live = match live {
                Ok(v) => v,
                // Stopped guest or no agent: Proxmox answers with an HTTP error.
                Err(e @ (Error::Untrusted(_) | Error::PinChanged { .. } | Error::Auth(_))) => {
                    return Err(e);
                }
                Err(_) => Vec::new(),
            };
            if let Some(ip) = rank_ips(&live, near).first() {
                return Ok(Some(*ip));
            }
            let config = c.call(false, &format!("{base}/config"))?;
            Ok(rank_ips(&config_candidates(&config, guest.kind), near)
                .first()
                .copied())
        })();
        send(
            &sink,
            server,
            epoch,
            &c,
            guest.vmid,
            GuestPayload::Address(result),
        );
    });
}

/// SPICE needs a QXL display (`vga: qxl`, `qxl2`, …).
pub fn spice_display(vga: Option<&str>) -> bool {
    vga.and_then(|v| v.split(',').next())
        .map(|t| t.trim().trim_start_matches("type="))
        .is_some_and(|t| t.starts_with("qxl") || t.contains("spice"))
}

/// Checks the display, then asks for a SPICE ticket (`spiceproxy`).
pub fn spawn_spice(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let proxy = conn.host.trim().to_string();
        let result = (|| {
            let base = guest
                .base()
                .filter(|_| guest.kind == Kind::Qemu)
                .ok_or_else(|| Error::Net(t!("ungültiger Node-Name").into()))?;
            let config = c.call(false, &format!("{base}/config"))?;
            let vga = config["vga"].as_str();
            if !spice_display(vga) {
                return Err(Error::Net(
                    t!(
                        "Die VM hat kein SPICE-Display (Anzeige: {}). Stelle sie in Proxmox auf „SPICE (qxl)“ um.",
                        vga.unwrap_or("std")
                    )
                    .to_string(),
                ));
            }
            let data = c.request(
                Method::Post,
                &format!("{base}/spiceproxy"),
                &[("proxy", proxy)],
            )?;
            let Some(obj) = data.as_object() else {
                return Err(Error::Net(t!("unerwartete Antwort").into()));
            };
            Ok(obj
                .iter()
                .map(|(k, v)| {
                    let v = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), v)
                })
                .collect())
        })();
        send(
            &sink,
            server,
            epoch,
            &c,
            guest.vmid,
            GuestPayload::Spice(result),
        );
    });
}

// ------------------------------------------------------------------ consoles

/// Proxmox' own console page: noVNC for VMs, xterm.js-less noVNC for containers.
pub fn novnc_url(host: &str, port: u16, g: &GuestId, name: &str) -> String {
    let host = host.trim();
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let console = match g.kind {
        Kind::Qemu => "kvm",
        Kind::Lxc => "lxc",
    };
    format!(
        "https://{host}:{port}/?console={console}&novnc=1&vmid={}&vmname={}&node={}&resize=off&cmd=",
        g.vmid,
        enc(name),
        enc(&g.node)
    )
}

/// Contents of a virt-viewer `.vv` file. Keys are checked, values kept on one line
/// (the CA's line breaks become `\n`, as virt-viewer expects).
pub fn vv_file(settings: &[(String, String)]) -> String {
    let mut out = String::from("[virt-viewer]\n");
    for (k, v) in settings {
        if k.is_empty()
            || !k
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            continue;
        }
        let v = v.replace("\r\n", "\\n").replace(['\n', '\r'], "\\n");
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// `remote-viewer` from virt-viewer: in `PATH`, on Windows also under
/// `C:\Program Files\VirtViewer*\bin`.
pub fn find_remote_viewer() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "remote-viewer.exe"
    } else {
        "remote-viewer"
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(windows) {
        for root in [
            std::env::var_os("ProgramFiles"),
            std::env::var_os("ProgramFiles(x86)"),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(rd) = std::fs::read_dir(&root) {
                for e in rd.flatten() {
                    if e.file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .starts_with("virtviewer")
                    {
                        dirs.push(e.path().join("bin"));
                    }
                }
            }
        }
    }
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| is_file(p))
}

fn is_file(p: &Path) -> bool {
    p.metadata().is_ok_and(|m| m.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ips(v: &[IpAddr]) -> Vec<String> {
        v.iter().map(|i| i.to_string()).collect()
    }

    #[test]
    fn ranks_addresses() {
        let agent = json!({"result":[
            {"name":"lo","ip-addresses":[{"ip-address":"127.0.0.1","prefix":8}]},
            {"name":"docker0","ip-addresses":[{"ip-address":"172.17.0.1","prefix":16}]},
            {"name":"br-1a2b","ip-addresses":[{"ip-address":"172.18.0.1","prefix":16}]},
            {"name":"veth9","ip-addresses":[{"ip-address":"169.254.3.3","prefix":16}]},
            {"name":"tailscale0","ip-addresses":[{"ip-address":"100.64.1.2","prefix":32}]},
            {"name":"ens19","ip-addresses":[{"ip-address":"10.20.0.5","prefix":24}]},
            {"name":"ens18","ip-addresses":[
                {"ip-address":"fe80::1","prefix":64},
                {"ip-address":"2001:db8::5","prefix":64},
                {"ip-address":"192.168.10.21","prefix":24},
                {"ip-address":"not-an-ip; rm -rf /","prefix":24}
            ]}
        ]});
        let c = agent_candidates(&agent);
        assert_eq!(c.len(), 9);
        // Without a hint: IPv4 in interface order, VPN last among them.
        assert_eq!(
            ips(&rank_ips(&c, None)),
            vec!["10.20.0.5", "192.168.10.21", "100.64.1.2", "2001:db8::5"]
        );
        // Same subnet as the server wins.
        let near = "192.168.10.2".parse().ok();
        assert_eq!(rank_ips(&c, near)[0].to_string(), "192.168.10.21");
        assert_eq!(
            parse_lxc_like(json!([
                {"name":"lo","inet":"127.0.0.1/8"},
                {"name":"eth0","inet":"10.0.0.21/24","inet6":"fe80::1/64"},
                {"name":"eth1","inet":"$(reboot)/24"}
            ])),
            vec!["10.0.0.21"]
        );
    }

    fn parse_lxc_like(v: Value) -> Vec<String> {
        ips(&rank_ips(&lxc_candidates(&v), None))
    }

    #[test]
    fn static_config_addresses() {
        let ct = json!({"hostname":"dns","net0":"name=eth0,bridge=vmbr0,hwaddr=BC:24:11:00:00:01,ip=192.168.10.53/24,gw=192.168.10.1,type=veth",
                        "net1":"name=eth1,bridge=vmbr1,ip=dhcp"});
        let c = config_candidates(&ct, Kind::Lxc);
        assert_eq!(c.len(), 1);
        assert_eq!(
            (c[0].ip.to_string(), c[0].iface.as_str(), c[0].prefix),
            ("192.168.10.53".into(), "eth0", Some(24))
        );
        let vm = json!({"ipconfig0":"ip=dhcp,ip6=auto","ipconfig1":"ip=10.1.2.3/16,gw=10.1.0.1","net0":"virtio=AA:BB,bridge=vmbr0"});
        assert_eq!(
            ips(&rank_ips(&config_candidates(&vm, Kind::Qemu), None)),
            vec!["10.1.2.3"]
        );
        assert!(config_candidates(&json!({"net0":"ip=evil;cmd/24"}), Kind::Lxc).is_empty());
    }

    #[test]
    fn subnets() {
        let a: IpAddr = "192.168.10.21".parse().unwrap();
        let b: IpAddr = "192.168.10.2".parse().unwrap();
        let c: IpAddr = "192.168.11.2".parse().unwrap();
        assert!(same_subnet(&a, &b, Some(24)));
        assert!(!same_subnet(&a, &c, Some(24)));
        assert!(same_subnet(&a, &c, Some(16)));
        assert_eq!(host_ip("10.0.0.1", 8006), "10.0.0.1".parse().ok());
        assert_eq!(host_ip("[::1]", 8006), "::1".parse().ok());
    }

    #[test]
    fn snapshot_tree() {
        let data = json!([
            {"name":"current","description":"You are here!","parent":"after-upgrade","running":1},
            {"name":"after-upgrade","snaptime":300,"parent":"before-upgrade","description":"ok"},
            {"name":"before-upgrade","snaptime":100,"vmstate":1,"description":"pre\n"},
            {"name":"experiment","snaptime":200,"parent":"before-upgrade"},
            {"name":"orphan","snaptime":50,"parent":"gone"}
        ]);
        let t = parse_snapshots(&data);
        let order: Vec<(&str, usize)> = t.iter().map(|s| (s.name.as_str(), s.depth)).collect();
        assert_eq!(
            order,
            vec![
                ("orphan", 0),
                ("before-upgrade", 0),
                ("experiment", 1),
                ("after-upgrade", 1),
                ("current", 2)
            ]
        );
        assert!(t[1].vmstate && t[1].description == "pre");
        assert!(t[4].current && t[4].time.is_none());
        assert!(valid_snapname("pre_update-2"));
        for bad in [
            "",
            "1abc",
            "a",
            "a b",
            "x/../y",
            "current",
            "a;b",
            &"a".repeat(41),
        ] {
            assert!(!valid_snapname(bad), "{bad}");
        }
    }

    #[test]
    fn backups() {
        let stores = parse_backup_stores(&json!([
            {"storage":"local","type":"dir","content":"iso,vztmpl,backup","avail":100,"total":200,"active":1,"enabled":1},
            {"storage":"pbs","type":"pbs","content":"backup","active":1},
            {"storage":"off","type":"nfs","content":"backup","active":0},
            {"storage":"local-lvm","type":"lvmthin","content":"images,rootdir"},
            {"storage":"bad/../x","type":"dir","content":"backup"}
        ]));
        assert_eq!(
            stores.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["local", "pbs"]
        );
        let list = parse_backups(
            &json!([
                {"volid":"local:backup/vzdump-qemu-100-2026_10_01-02_00_01.vma.zst","content":"backup","ctime":1759276801,"size":123,"vmid":100,"notes":"nightly\nmore","protected":1},
                {"volid":"local:backup/vzdump-qemu-101-2026_10_01-02_00_01.vma.zst","content":"backup","vmid":101},
                {"volid":"local:iso/x.iso","content":"iso"}
            ]),
            "local",
            100,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].notes.as_str(), list[0].protected, list[0].size),
            ("nightly", true, 123)
        );
        assert!(valid_volid("pbs:backup/vm/100/2026-10-01T02:00:01Z"));
        assert!(!valid_volid("local:../../etc/passwd"));
        assert!(!valid_volid("nostorage"));
        assert!(!valid_volid("local:a b"));
    }

    #[test]
    fn op_requests() {
        let vm = GuestId {
            node: "pve1".into(),
            kind: Kind::Qemu,
            vmid: 100,
        };
        let ct = GuestId {
            node: "pve1".into(),
            kind: Kind::Lxc,
            vmid: 101,
        };
        let (m, p, f) = Op::Snapshot {
            name: "pre".into(),
            description: " d ".into(),
            vmstate: true,
        }
        .request(&vm)
        .unwrap();
        assert_eq!(
            (m, p.as_str()),
            (Method::Post, "/nodes/pve1/qemu/100/snapshot")
        );
        assert_eq!(
            f,
            vec![
                ("snapname", "pre".into()),
                ("description", "d".into()),
                ("vmstate", "1".into())
            ]
        );
        // No RAM state for containers.
        let (_, _, f) = Op::Snapshot {
            name: "pre".into(),
            description: String::new(),
            vmstate: true,
        }
        .request(&ct)
        .unwrap();
        assert_eq!(f, vec![("snapname", "pre".into())]);
        assert!(Op::Rollback("x;y".into()).request(&vm).is_none());
        let (m, p, _) = Op::DeleteSnapshot("pre".into()).request(&ct).unwrap();
        assert_eq!(
            (m, p.as_str()),
            (Method::Delete, "/nodes/pve1/lxc/101/snapshot/pre")
        );
        let (_, p, f) = Op::Backup {
            storage: "local".into(),
            mode: "snapshot".into(),
        }
        .request(&vm)
        .unwrap();
        assert_eq!(p, "/nodes/pve1/vzdump");
        assert!(f.contains(&("compress", "zstd".into())) && f.contains(&("vmid", "100".into())));
        assert!(
            Op::Backup {
                storage: "local".into(),
                mode: "rm".into()
            }
            .request(&vm)
            .is_none()
        );
        let volid = "local:backup/vzdump-lxc-101-2026_10_01-02_00_01.tar.zst".to_string();
        let (_, p, f) = Op::Restore {
            volid: volid.clone(),
            vmid: 105,
        }
        .request(&ct)
        .unwrap();
        assert_eq!(p, "/nodes/pve1/lxc");
        assert!(f.contains(&("ostemplate", volid.clone())) && f.contains(&("restore", "1".into())));
        assert!(!f.iter().any(|(k, _)| *k == "force"));
        // Never over the original.
        assert!(Op::Restore { volid, vmid: 101 }.request(&ct).is_none());
        let bad = GuestId {
            node: "pve1/../x".into(),
            kind: Kind::Qemu,
            vmid: 1,
        };
        assert!(Op::Rollback("pre".into()).request(&bad).is_none());
    }

    #[test]
    fn consoles_and_tasks() {
        let g = GuestId {
            node: "pve1".into(),
            kind: Kind::Qemu,
            vmid: 100,
        };
        assert_eq!(
            novnc_url("10.0.0.1", 8006, &g, "web 01"),
            "https://10.0.0.1:8006/?console=kvm&novnc=1&vmid=100&vmname=web%2001&node=pve1&resize=off&cmd="
        );
        assert!(novnc_url("fd00::1", 8006, &g, "").starts_with("https://[fd00::1]:8006/"));
        assert_eq!(
            enc("UPID:pve1:00:root@pam:"),
            "UPID%3Apve1%3A00%3Aroot%40pam%3A"
        );
        assert_eq!(task_result("OK"), Ok("OK".into()));
        assert!(task_result("WARNINGS: 2").is_ok());
        assert_eq!(
            task_result("command 'x' failed"),
            Err("command 'x' failed".into())
        );
        assert!(spice_display(Some("qxl,memory=32")) && spice_display(Some("qxl2")));
        assert!(!spice_display(Some("std")) && !spice_display(None));
        let vv = vv_file(&[
            ("type".into(), "spice".into()),
            ("ca".into(), "-----BEGIN-----\nAAA\n-----END-----\n".into()),
            ("bad key".into(), "x".into()),
            ("password".into(), "a\nproxy=evil".into()),
        ]);
        assert_eq!(
            vv,
            "[virt-viewer]\ntype=spice\nca=-----BEGIN-----\\nAAA\\n-----END-----\\n\npassword=a\\nproxy=evil\n"
        );
    }
}
