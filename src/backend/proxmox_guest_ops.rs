//! Guest-level extras: live graphs (`rrddata`), clone / new from template with
//! cloud-init, migration, quick hardware edits and the power requests behind batch
//! actions. Every path segment is validated before it goes into a URL; nothing here
//! builds shell commands.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use super::{
    Client, Conn, Epoch, Error, GuestId, GuestPayload, Kind, Method, Power, PveEvent, Sink,
    TaskUpdate, enc, num, task_result, valid_node, valid_storage,
};

// ------------------------------------------------------------------ validation

/// VMIDs Proxmox accepts for guests.
pub fn valid_vmid(id: u32) -> bool {
    (100..=999_999_999).contains(&id)
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit())
}

/// Disk keys of a guest config: `scsi0`, `virtio1`, `sata2`, `ide0`, `rootfs`, `mp0`, …
pub fn valid_disk_key(k: &str) -> bool {
    if k == "rootfs" || k == "efidisk0" || k == "tpmstate0" {
        return true;
    }
    for (prefix, max) in [
        ("scsi", 30),
        ("virtio", 15),
        ("sata", 5),
        ("ide", 3),
        ("mp", 255),
    ] {
        if let Some(n) = k.strip_prefix(prefix) {
            return digits(n) && n.parse::<u32>().is_ok_and(|n| n <= max);
        }
    }
    false
}

/// `net0` … `net31`.
pub fn valid_net_key(k: &str) -> bool {
    k.strip_prefix("net")
        .is_some_and(|n| digits(n) && n.parse::<u32>().is_ok_and(|n| n <= 31))
}

/// Linux bridge names (`vmbr0`, `vmbr1.100`, `br_lan`): at most 15 characters.
pub fn valid_bridge(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 15
        && b.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// VM names and CT host names: DNS labels separated by dots.
pub fn valid_guest_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 63
        && n.split('.').all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Cloud-init user names: lower case letter or `_`, then letters, digits, `_`, `-`.
pub fn valid_ciuser(u: &str) -> bool {
    let mut c = u.chars();
    c.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && u.len() <= 32
        && u.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// `10.0.0.5/24` (or an IPv6 address with prefix).
pub fn valid_cidr(s: &str) -> bool {
    let Some((ip, prefix)) = s.trim().split_once('/') else {
        return false;
    };
    let Ok(ip) = ip.parse::<IpAddr>() else {
        return false;
    };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    prefix.parse::<u8>().is_ok_and(|p| (1..=max).contains(&p))
}

// ------------------------------------------------------------------ graphs

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Timeframe {
    #[default]
    Hour,
    Day,
    Week,
}

impl Timeframe {
    pub const ALL: [Timeframe; 3] = [Timeframe::Hour, Timeframe::Day, Timeframe::Week];

    pub fn api(self) -> &'static str {
        match self {
            Timeframe::Hour => "hour",
            Timeframe::Day => "day",
            Timeframe::Week => "week",
        }
    }
}

/// One sample of `rrddata`. Missing values (no data yet, guest stopped) are NaN.
#[derive(Clone, Debug, PartialEq)]
pub struct RrdPoint {
    pub time: i64,
    /// Fraction of `maxcpu` (0.0–1.0).
    pub cpu: f64,
    pub maxcpu: f64,
    pub mem: f64,
    pub maxmem: f64,
    /// Bytes per second.
    pub netin: f64,
    pub netout: f64,
    pub diskread: f64,
    pub diskwrite: f64,
    /// Nodes only: fraction of time waiting for I/O, and the 1-minute load.
    pub iowait: f64,
    pub load: f64,
}

fn field(v: &Value, keys: &[&str]) -> f64 {
    keys.iter()
        .find_map(|k| v.get(*k).filter(|x| !x.is_null()).map(num))
        .unwrap_or(f64::NAN)
}

/// `data` of `…/rrddata` (guests and nodes), oldest first.
pub fn parse_rrd(data: &Value) -> Vec<RrdPoint> {
    let mut out: Vec<RrdPoint> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            let time = p.get("time").map(num).filter(|t| *t > 0.0)? as i64;
            Some(RrdPoint {
                time,
                cpu: field(p, &["cpu"]),
                maxcpu: field(p, &["maxcpu"]),
                mem: field(p, &["mem", "memused"]),
                maxmem: field(p, &["maxmem", "memtotal"]),
                netin: field(p, &["netin"]),
                netout: field(p, &["netout"]),
                diskread: field(p, &["diskread"]),
                diskwrite: field(p, &["diskwrite"]),
                iowait: field(p, &["iowait"]),
                load: field(p, &["loadavg"]),
            })
        })
        .collect();
    out.sort_by_key(|p| p.time);
    out
}

/// CPU history for a sparkline (percent, NaN samples dropped), at most `max` values.
pub fn spark_values(points: &[RrdPoint], max: usize) -> Vec<f32> {
    let v: Vec<f32> = points
        .iter()
        .filter(|p| p.cpu.is_finite())
        .map(|p| (p.cpu * 100.0).clamp(0.0, 10_000.0) as f32)
        .collect();
    v[v.len().saturating_sub(max)..].to_vec()
}

/// What a graph shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RrdTarget {
    Guest(GuestId),
    Node(String),
}

fn rrd_path(t: &RrdTarget, tf: Timeframe) -> Option<String> {
    let q = format!("rrddata?timeframe={}&cf=AVERAGE", tf.api());
    match t {
        RrdTarget::Guest(g) => Some(format!("{}/{q}", base(g)?)),
        RrdTarget::Node(n) => valid_node(n).then(|| format!("/nodes/{n}/{q}")),
    }
}

/// `/nodes/{node}/{qemu|lxc}/{vmid}` for a valid node and VMID.
fn base(g: &GuestId) -> Option<String> {
    (valid_node(&g.node) && valid_vmid(g.vmid))
        .then(|| format!("/nodes/{}/{}/{}", g.node, g.kind.api(), g.vmid))
}

// ------------------------------------------------------------------ storages

/// A storage for guest disks (or backups) on one node.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreInfo {
    pub id: String,
    pub kind: String,
    pub avail: u64,
    pub shared: bool,
}

/// Storages per node name.
pub type NodeStores = Vec<(String, Vec<StoreInfo>)>;

/// `data` of `/nodes/{node}/storage?content=…`: active, enabled storages holding
/// `content` (`images`, `rootdir`, `backup`).
pub fn parse_stores(data: &Value, content: &str) -> Vec<StoreInfo> {
    let mut out: Vec<StoreInfo> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|s| {
            s["content"]
                .as_str()
                .is_none_or(|c| c.split(',').any(|x| x.trim() == content))
        })
        .filter(|s| s.get("enabled").is_none_or(|e| num(e) != 0.0))
        .filter(|s| s.get("active").is_none_or(|e| num(e) != 0.0))
        .filter_map(|s| {
            let id = s["storage"].as_str()?;
            valid_storage(id).then(|| StoreInfo {
                id: id.to_string(),
                kind: s["type"].as_str().unwrap_or_default().to_string(),
                avail: num(&s["avail"]) as u64,
                shared: num(&s["shared"]) != 0.0,
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ------------------------------------------------------------------ clone

/// Cloud-init settings for a cloned VM.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CloudInit {
    pub user: String,
    /// `None` = DHCP, else `ip/cidr` and an optional gateway.
    pub ip: Option<(String, String)>,
    /// OpenSSH public keys, one per entry.
    pub sshkeys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloneSpec {
    pub newid: u32,
    pub name: String,
    pub target: String,
    pub full: bool,
    /// Target storage for a full clone (`None` = same as the source).
    pub storage: Option<String>,
    /// The source is a template (only those can be linked-cloned).
    pub from_template: bool,
    pub cloud_init: Option<CloudInit>,
    pub start: bool,
}

/// `ip=dhcp` or `ip=10.0.0.5/24,gw=10.0.0.1` (`ip6=…` for IPv6); `None` for bad input.
pub fn ipconfig_value(ip: Option<(&str, &str)>) -> Option<String> {
    let Some((cidr, gw)) = ip else {
        return Some("ip=dhcp".into());
    };
    let cidr = cidr.trim();
    let gw = gw.trim();
    if !valid_cidr(cidr) {
        return None;
    }
    let v6 = cidr.contains(':');
    let (ipk, gwk) = if v6 { ("ip6", "gw6") } else { ("ip", "gw") };
    let mut out = format!("{ipk}={cidr}");
    if !gw.is_empty() {
        let g: IpAddr = gw.parse().ok()?;
        if g.is_ipv6() != v6 {
            return None;
        }
        out.push_str(&format!(",{gwk}={g}"));
    }
    Some(out)
}

/// An OpenSSH public key line (`ssh-ed25519 AAAA… comment`).
pub fn valid_pubkey(k: &str) -> bool {
    let k = k.trim();
    let mut parts = k.split_whitespace();
    let (Some(algo), Some(blob)) = (parts.next(), parts.next()) else {
        return false;
    };
    (algo.starts_with("ssh-") || algo.starts_with("ecdsa-") || algo.starts_with("sk-"))
        && blob.len() >= 16
        && blob
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
        && !k.contains(['\n', '\r', '\0'])
}

/// `sshkeys` for `…/config`: Proxmox wants the keys (one per line) percent-encoded
/// (`%20` for spaces, `%0A` between keys) on top of the form encoding of the request.
pub fn sshkeys_param(keys: &[String]) -> Option<String> {
    let keys: Vec<&str> = keys
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .collect();
    if keys.is_empty() || !keys.iter().all(|k| valid_pubkey(k)) {
        return None;
    }
    Some(enc(&keys.join("\n")))
}

/// Form of `POST …/config` with the cloud-init settings; `None` for bad input.
fn cloudinit_form(ci: &CloudInit) -> Option<Vec<(String, String)>> {
    let mut form = Vec::new();
    let user = ci.user.trim();
    if !user.is_empty() {
        if !valid_ciuser(user) {
            return None;
        }
        form.push(("ciuser".to_string(), user.to_string()));
    }
    let ip = ci.ip.as_ref().map(|(a, b)| (a.as_str(), b.as_str()));
    form.push(("ipconfig0".to_string(), ipconfig_value(ip)?));
    if ci.sshkeys.iter().any(|k| !k.trim().is_empty()) {
        form.push(("sshkeys".to_string(), sshkeys_param(&ci.sshkeys)?));
    }
    Some(form)
}

type Form = Vec<(String, String)>;

fn f(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

/// `POST …/clone`; `None` for bad input.
fn clone_request(g: &GuestId, s: &CloneSpec) -> Option<(Method, String, Form)> {
    let base = base(g)?;
    if !valid_vmid(s.newid) || s.newid == g.vmid || !valid_node(&s.target) {
        return None;
    }
    let name = s.name.trim();
    if !name.is_empty() && !valid_guest_name(name) {
        return None;
    }
    // Linked clones need a template; containers that are no template clone fully.
    if !s.full && !s.from_template {
        return None;
    }
    let mut form = vec![f("newid", s.newid.to_string())];
    if !name.is_empty() {
        form.push(f(
            match g.kind {
                Kind::Qemu => "name",
                Kind::Lxc => "hostname",
            },
            name,
        ));
    }
    if s.target != g.node {
        form.push(f("target", s.target.clone()));
    }
    form.push(f("full", if s.full { "1" } else { "0" }));
    if s.full
        && let Some(st) = &s.storage
    {
        if !valid_storage(st) {
            return None;
        }
        form.push(f("storage", st.clone()));
    }
    Some((Method::Post, format!("{base}/clone"), form))
}

/// Whether the VM config has a cloud-init drive (`ide2: local-lvm:vm-100-cloudinit,…`).
pub fn has_cloudinit(cfg: &Value) -> bool {
    cfg.as_object().is_some_and(|o| {
        o.iter().any(|(k, v)| {
            valid_disk_key(k)
                && v.as_str()
                    .is_some_and(|s| s.split(',').next().unwrap_or("").contains("cloudinit"))
        })
    })
}

// ------------------------------------------------------------------ migrate

#[derive(Clone, Debug, PartialEq)]
pub struct MigrateSpec {
    pub target: String,
    /// The guest runs: live migration (VM) or restart migration (CT).
    pub running: bool,
    pub storage: Option<String>,
    /// Take local disks along (`with-local-disks`, VMs only; set implicitly with a
    /// target storage).
    pub local_disks: bool,
}

fn migrate_request(g: &GuestId, s: &MigrateSpec) -> Option<(Method, String, Form)> {
    let base = base(g)?;
    if !valid_node(&s.target) || s.target == g.node {
        return None;
    }
    let mut form = vec![f("target", s.target.clone())];
    match g.kind {
        Kind::Qemu => {
            if s.running {
                form.push(f("online", "1"));
            }
            if let Some(st) = &s.storage {
                if !valid_storage(st) {
                    return None;
                }
                form.push(f("targetstorage", st.clone()));
                form.push(f("with-local-disks", "1"));
            } else if s.local_disks {
                form.push(f("with-local-disks", "1"));
            }
        }
        Kind::Lxc => {
            if s.running {
                form.push(f("restart", "1"));
            }
            if let Some(st) = &s.storage {
                if !valid_storage(st) {
                    return None;
                }
                form.push(f("target-storage", st.clone()));
            }
        }
    }
    Some((Method::Post, format!("{base}/migrate"), form))
}

// ------------------------------------------------------------------ hardware

/// `virtio=BC:24:11:00:00:01,bridge=vmbr0,tag=20,firewall=1` → ordered key/value pairs.
/// A part without `=` is kept with an empty value.
pub fn parse_net(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect()
}

pub fn build_net(parts: &[(String, String)]) -> String {
    parts
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn net_get<'a>(parts: &'a [(String, String)], key: &str) -> Option<&'a str> {
    parts
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Sets bridge and VLAN tag (`None` removes the tag), keeping every other key and the
/// order; `None` for an invalid bridge or tag.
pub fn net_with(s: &str, bridge: &str, tag: Option<u16>) -> Option<String> {
    if !valid_bridge(bridge) || tag.is_some_and(|t| !(1..=4094).contains(&t)) {
        return None;
    }
    let mut parts = parse_net(s);
    match parts.iter_mut().find(|(k, _)| k == "bridge") {
        Some(p) => p.1 = bridge.to_string(),
        None => parts.push(("bridge".into(), bridge.to_string())),
    }
    match (parts.iter().position(|(k, _)| k == "tag"), tag) {
        (Some(i), Some(t)) => parts[i].1 = t.to_string(),
        (Some(i), None) => {
            parts.remove(i);
        }
        (None, Some(t)) => {
            // After the bridge, like Proxmox writes it.
            let at = parts
                .iter()
                .position(|(k, _)| k == "bridge")
                .map(|i| i + 1)
                .unwrap_or(parts.len());
            parts.insert(at, ("tag".into(), t.to_string()));
        }
        (None, None) => {}
    }
    Some(build_net(&parts))
}

/// `32G`, `512M`, `1T`, `1048576K` → GiB.
pub fn parse_size(s: &str) -> Option<f64> {
    let s = s.trim();
    let (n, unit) = match s.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((i, _)) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let n: f64 = n.parse().ok()?;
    let gib = match unit.to_ascii_uppercase().as_str() {
        "" => n / (1024.0 * 1024.0 * 1024.0),
        "K" => n / (1024.0 * 1024.0),
        "M" => n / 1024.0,
        "G" => n,
        "T" => n * 1024.0,
        _ => return None,
    };
    Some(gib)
}

/// A disk of the guest config.
#[derive(Clone, Debug, PartialEq)]
pub struct DiskInfo {
    pub key: String,
    pub volume: String,
    pub gib: f64,
}

/// What the hardware sheet edits.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct HwConfig {
    pub cores: u32,
    pub sockets: u32,
    /// MiB.
    pub memory: u64,
    /// MiB; VMs only (`None` = not set: ballooning up to `memory`).
    pub balloon: Option<u64>,
    /// MiB; containers only.
    pub swap: u64,
    pub disks: Vec<DiskInfo>,
    /// `net0` … with their full value.
    pub nets: Vec<(String, String)>,
    pub digest: Option<String>,
    /// `true` when the config already has changes waiting for a restart.
    pub has_pending: bool,
}

fn sorted_keys(o: &serde_json::Map<String, Value>, ok: fn(&str) -> bool) -> Vec<&str> {
    let mut keys: Vec<&str> = o.keys().map(String::as_str).filter(|k| ok(k)).collect();
    keys.sort_by_key(|k| {
        let n: String = k.chars().filter(|c| c.is_ascii_digit()).collect();
        let p: String = k.chars().filter(|c| !c.is_ascii_digit()).collect();
        (p != "rootfs", p, n.parse::<u32>().unwrap_or(0))
    });
    keys
}

/// `data` of `…/config`.
pub fn parse_hw(cfg: &Value, kind: Kind) -> HwConfig {
    let Some(o) = cfg.as_object() else {
        return HwConfig::default();
    };
    let n = |k: &str, d: f64| o.get(k).map(num).unwrap_or(d);
    let mut disks = Vec::new();
    for key in sorted_keys(o, valid_disk_key) {
        let Some(v) = o[key].as_str() else {
            continue;
        };
        let parts = parse_net(v);
        let volume = parts.first().map(|p| p.0.clone()).unwrap_or_default();
        let media_cdrom = net_get(&parts, "media") == Some("cdrom");
        if media_cdrom
            || volume.contains("cloudinit")
            || volume == "none"
            || (kind == Kind::Qemu && (key == "efidisk0" || key == "tpmstate0"))
        {
            continue;
        }
        let Some(gib) = net_get(&parts, "size").and_then(parse_size) else {
            continue;
        };
        disks.push(DiskInfo {
            key: key.to_string(),
            volume,
            gib,
        });
    }
    let nets = sorted_keys(o, valid_net_key)
        .into_iter()
        .filter_map(|k| Some((k.to_string(), o[k].as_str()?.to_string())))
        .collect();
    HwConfig {
        cores: n("cores", 1.0).max(1.0) as u32,
        sockets: n("sockets", 1.0).max(1.0) as u32,
        memory: n("memory", 512.0) as u64,
        balloon: o.get("balloon").map(|b| num(b) as u64),
        swap: n("swap", 512.0) as u64,
        disks,
        nets,
        digest: o
            .get("digest")
            .and_then(|d| d.as_str())
            .filter(|d| d.chars().all(|c| c.is_ascii_hexdigit()) && !d.is_empty())
            .map(str::to_string),
        has_pending: false,
    }
}

/// Changes of the hardware sheet.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct HwChange {
    pub set: Vec<(String, String)>,
    /// Disk key → GiB to add.
    pub resize: Vec<(String, u32)>,
    pub digest: Option<String>,
}

/// Config keys the sheet may set, and whether a value fits.
fn valid_setting(kind: Kind, key: &str, value: &str) -> bool {
    let num_in = |lo: u64, hi: u64| value.parse::<u64>().is_ok_and(|v| (lo..=hi).contains(&v));
    match (kind, key) {
        (_, "cores") => num_in(1, 512),
        (Kind::Qemu, "sockets") => num_in(1, 16),
        (_, "memory") => num_in(16, 64 * 1024 * 1024),
        (Kind::Qemu, "balloon") => num_in(0, 64 * 1024 * 1024),
        (Kind::Lxc, "swap") => num_in(0, 64 * 1024 * 1024),
        (_, k) if valid_net_key(k) => {
            !value.is_empty()
                && value.len() < 512
                && value
                    .chars()
                    .all(|c| c.is_ascii_graphic() && c != '&' && c != '\\')
                && net_get(&parse_net(value), "bridge").is_some_and(valid_bridge)
        }
        _ => false,
    }
}

/// `PUT …/config`, then one `PUT …/resize` per disk; `None` for bad input.
fn hw_requests(g: &GuestId, ch: &HwChange) -> Option<Vec<(Method, String, Form)>> {
    let base = base(g)?;
    let mut out = Vec::new();
    if !ch.set.is_empty() {
        if !ch.set.iter().all(|(k, v)| valid_setting(g.kind, k, v)) {
            return None;
        }
        let mut form: Form = ch.set.clone();
        if let Some(d) = &ch.digest {
            if !d.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            form.push(f("digest", d.clone()));
        }
        out.push((Method::Put, format!("{base}/config"), form));
    }
    for (disk, add) in &ch.resize {
        if !valid_disk_key(disk) || !(1..=65_536).contains(add) {
            return None;
        }
        out.push((
            Method::Put,
            format!("{base}/resize"),
            vec![f("disk", disk.clone()), f("size", format!("+{add}G"))],
        ));
    }
    Some(out)
}

/// Keys of `…/pending` that wait for a restart.
pub fn pending_keys(data: &Value) -> Vec<String> {
    data.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|e| e.get("pending").is_some_and(|p| !p.is_null()) || e.get("delete").is_some())
        .filter_map(|e| e["key"].as_str())
        .filter(|k| *k != "digest")
        .map(str::to_string)
        .collect()
}

// ------------------------------------------------------------------ jobs

/// Actions run as tracked tasks next to the snapshot/backup ones.
#[derive(Clone, Debug, PartialEq)]
pub enum ExtOp {
    Power(Power),
    Clone(CloneSpec),
    Migrate(MigrateSpec),
    Hardware(HwChange),
    /// Cloud-init settings of a VM (`pve2::ci`), optionally regenerating the drive.
    CloudInit(Box<super::pve2::ci::CiApply>),
}

/// Results of the loaders below.
#[derive(Clone, Debug)]
pub enum ExtPayload {
    Rrd {
        target: RrdTarget,
        tf: Timeframe,
        result: Result<Vec<RrdPoint>, Error>,
    },
    /// CPU history of several guests (VMID → percent values).
    Sparks(Vec<(u32, Vec<f32>)>),
    /// Storages per node for `content`.
    Stores {
        content: String,
        result: Result<Vec<(String, Vec<StoreInfo>)>, Error>,
    },
    /// Next free VMID and whether the source has a cloud-init drive.
    CloneInfo(Result<(u32, bool), Error>),
    Hardware(Result<HwConfig, Error>),
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

fn ext(sink: &Sink<PveEvent>, server: Uuid, epoch: Epoch, c: &Client, vmid: u32, p: ExtPayload) {
    send(sink, server, epoch, c, vmid, GuestPayload::Ext(Box::new(p)));
}

fn bad_input() -> Error {
    Error::Net(t!("ungültige Eingabe").into())
}

pub fn spawn_rrd(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    target: RrdTarget,
    tf: Timeframe,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = match rrd_path(&target, tf) {
            Some(p) => c.call(false, &p).map(|d| parse_rrd(&d)),
            None => Err(bad_input()),
        };
        let vmid = match &target {
            RrdTarget::Guest(g) => g.vmid,
            RrdTarget::Node(_) => 0,
        };
        ext(
            &sink,
            server,
            epoch,
            &c,
            vmid,
            ExtPayload::Rrd { target, tf, result },
        );
    });
}

/// Hourly CPU history of the given guests (for the row sparklines).
pub fn spawn_sparks(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guests: Vec<GuestId>,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let mut out = Vec::new();
        for g in guests {
            let Some(p) = rrd_path(&RrdTarget::Guest(g.clone()), Timeframe::Hour) else {
                continue;
            };
            match c.call(false, &p) {
                Ok(d) => out.push((g.vmid, spark_values(&parse_rrd(&d), 60))),
                Err(Error::Untrusted(_) | Error::PinChanged { .. } | Error::Auth(_)) => break,
                // No data (yet): an empty line, asked again later.
                Err(_) => out.push((g.vmid, Vec::new())),
            }
        }
        ext(&sink, server, epoch, &c, 0, ExtPayload::Sparks(out));
    });
}

/// Storages for `content` on each of `nodes`; `vmid` routes the answer.
#[allow(clippy::too_many_arguments)]
pub fn spawn_stores(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    vmid: u32,
    nodes: Vec<String>,
    content: &'static str,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = (|| {
            let mut out = Vec::new();
            for n in nodes {
                if !valid_node(&n) {
                    return Err(bad_input());
                }
                let d = c.call(false, &format!("/nodes/{n}/storage?content={content}"))?;
                out.push((n, parse_stores(&d, content)));
            }
            Ok(out)
        })();
        ext(
            &sink,
            server,
            epoch,
            &c,
            vmid,
            ExtPayload::Stores {
                content: content.to_string(),
                result,
            },
        );
    });
}

/// `/cluster/nextid` and the source config (cloud-init drive?).
pub fn spawn_clone_info(
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
            let base = base(&guest).ok_or_else(bad_input)?;
            let id = num(&c.call(false, "/cluster/nextid")?) as u32;
            if !valid_vmid(id) {
                return Err(Error::Net(t!("keine freie VMID").into()));
            }
            let ci = guest.kind == Kind::Qemu
                && has_cloudinit(&c.call(false, &format!("{base}/config"))?);
            Ok((id, ci))
        })();
        ext(
            &sink,
            server,
            epoch,
            &c,
            guest.vmid,
            ExtPayload::CloneInfo(result),
        );
    });
}

/// Config (and pending changes) for the hardware sheet.
pub fn spawn_hw_config(
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
            let base = base(&guest).ok_or_else(bad_input)?;
            let mut hw = parse_hw(&c.call(false, &format!("{base}/config"))?, guest.kind);
            // Older versions or missing rights: no pending info, not fatal.
            if let Ok(p) = c.call(false, &format!("{base}/pending")) {
                hw.has_pending = !pending_keys(&p).is_empty();
            }
            Ok(hw)
        })();
        ext(
            &sink,
            server,
            epoch,
            &c,
            guest.vmid,
            ExtPayload::Hardware(result),
        );
    });
}

/// Polls a UPID until the task stops; `Ok(exitstatus)` or the error text.
fn wait_task(
    c: &mut Client,
    fallback_node: &str,
    upid: &str,
    cancel: &AtomicBool,
    sink: &Sink<PveEvent>,
    progress: &dyn Fn(&Client, TaskUpdate),
) -> Option<Result<String, String>> {
    let node = upid
        .split(':')
        .nth(1)
        .filter(|n| valid_node(n))
        .unwrap_or(fallback_node)
        .to_string();
    let status_path = format!("/nodes/{node}/tasks/{}/status", enc(upid));
    let log_path = format!("/nodes/{node}/tasks/{}/log", enc(upid));
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
            return None;
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
                progress(
                    c,
                    TaskUpdate::Progress(last.trim().chars().take(160).collect()),
                );
            }
        }
        match c.call(false, &status_path) {
            Ok(st) => {
                failures = 0;
                if st["status"].as_str() == Some("stopped") {
                    return Some(task_result(st["exitstatus"].as_str().unwrap_or_default()));
                }
            }
            Err(e) => {
                failures += 1;
                if failures >= 5 {
                    return Some(Err(e.to_string()));
                }
            }
        }
        if started.elapsed() > Duration::from_secs(12 * 3600) {
            return Some(Err(
                t!("Aufgabe läuft noch – Status nicht mehr verfolgt").into()
            ));
        }
    }
}

/// The requests of an [`ExtOp`] in order; `None` for unsafe input.
fn ext_requests(g: &GuestId, op: &ExtOp) -> Option<Vec<(Method, String, Form)>> {
    Some(match op {
        ExtOp::Power(p) => vec![(
            Method::Post,
            format!("{}/status/{}", base(g)?, p.api()),
            vec![],
        )],
        ExtOp::Clone(s) => {
            let mut v = vec![clone_request(g, s)?];
            let new = GuestId {
                node: s.target.clone(),
                kind: g.kind,
                vmid: s.newid,
            };
            if let Some(ci) = &s.cloud_init {
                if g.kind != Kind::Qemu {
                    return None;
                }
                v.push((
                    Method::Post,
                    format!("{}/config", base(&new)?),
                    cloudinit_form(ci)?,
                ));
            }
            if s.start {
                v.push((
                    Method::Post,
                    format!("{}/status/start", base(&new)?),
                    vec![],
                ));
            }
            v
        }
        ExtOp::Migrate(s) => vec![migrate_request(g, s)?],
        ExtOp::CloudInit(a) => {
            if g.kind != Kind::Qemu || a.is_noop() {
                return None;
            }
            let base = base(g)?;
            let mut v = Vec::new();
            if !a.form.is_empty() || a.password.is_some() {
                v.push((Method::Post, format!("{base}/config"), a.config_form()));
            }
            if a.regenerate {
                v.push((Method::Put, format!("{base}/cloudinit"), vec![]));
            }
            v
        }
        ExtOp::Hardware(ch) => {
            let v = hw_requests(g, ch)?;
            if v.is_empty() {
                return None;
            }
            v
        }
    })
}

/// Runs `op` step by step (following each UPID until its task stops), reporting like
/// [`super::spawn_task`] so the task list and row indicators show it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_ext_task(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    op: ExtOp,
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
        let Some(steps) = ext_requests(&guest, &op) else {
            update(&c, TaskUpdate::Done(Err(t!("ungültige Eingabe").into())));
            return;
        };
        let mut status = String::from("OK");
        for (method, path, form) in steps {
            let form: Vec<(&str, String)> =
                form.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let v = match c.request(method, &path, &form) {
                Ok(v) => v,
                Err(e) => {
                    update(&c, TaskUpdate::Done(Err(e.to_string())));
                    return;
                }
            };
            let upid = v.as_str().unwrap_or_default().to_string();
            if !upid.starts_with("UPID:") {
                continue;
            }
            update(&c, TaskUpdate::Started(upid.clone()));
            match wait_task(&mut c, &guest.node, &upid, &cancel, &sink, &update) {
                None => return,
                Some(Ok(s)) => {
                    if s != "OK" {
                        status = s;
                    }
                }
                Some(Err(e)) => {
                    update(&c, TaskUpdate::Done(Err(e)));
                    return;
                }
            }
        }
        if let ExtOp::Hardware(_) = &op
            && let Some(base) = base(&guest)
            && let Ok(p) = c.call(false, &format!("{base}/pending"))
        {
            let keys = pending_keys(&p);
            if !keys.is_empty() {
                status = t!("Wird beim nächsten Neustart aktiv: {}", keys.join(", "));
            }
        }
        update(&c, TaskUpdate::Done(Ok(status)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vm() -> GuestId {
        GuestId {
            node: "pve1".into(),
            kind: Kind::Qemu,
            vmid: 100,
        }
    }

    fn ct() -> GuestId {
        GuestId {
            node: "pve1".into(),
            kind: Kind::Lxc,
            vmid: 101,
        }
    }

    fn form(v: &[(String, String)]) -> Vec<(&str, &str)> {
        v.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }

    #[test]
    fn rrd_parsing() {
        let data = json!([
            {"time":1700000060,"cpu":0.25,"maxcpu":4,"mem":1073741824u64,"maxmem":4294967296u64,
             "netin":1200.5,"netout":"300","diskread":0,"diskwrite":4096},
            {"time":1700000000},
            {"cpu":0.5},
            {"time":1700000120,"cpu":null,"memused":5,"memtotal":10,"iowait":0.01,"loadavg":"0.42"}
        ]);
        let p = parse_rrd(&data);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].time, 1700000000);
        assert!(p[0].cpu.is_nan());
        assert_eq!(p[1].cpu, 0.25);
        assert_eq!(p[1].netout, 300.0);
        assert_eq!((p[2].mem, p[2].maxmem), (5.0, 10.0));
        assert!((p[2].load - 0.42).abs() < 1e-9);
        assert_eq!(spark_values(&p, 60), vec![25.0]);
        let many: Vec<RrdPoint> = (0..100)
            .map(|i| RrdPoint {
                cpu: i as f64 / 100.0,
                ..p[1].clone()
            })
            .collect();
        let s = spark_values(&many, 60);
        assert_eq!(s.len(), 60);
        assert_eq!(s[59], 99.0);
        assert_eq!(
            rrd_path(&RrdTarget::Guest(vm()), Timeframe::Day).unwrap(),
            "/nodes/pve1/qemu/100/rrddata?timeframe=day&cf=AVERAGE"
        );
        assert_eq!(
            rrd_path(&RrdTarget::Node("pve2".into()), Timeframe::Week).unwrap(),
            "/nodes/pve2/rrddata?timeframe=week&cf=AVERAGE"
        );
        assert!(rrd_path(&RrdTarget::Node("a/../b".into()), Timeframe::Hour).is_none());
    }

    #[test]
    fn validation() {
        for ok in [
            "scsi0", "virtio15", "sata5", "ide2", "rootfs", "mp0", "mp12",
        ] {
            assert!(valid_disk_key(ok), "{ok}");
        }
        for bad in [
            "scsi", "scsi31", "virtio16", "ide4", "net0", "scsi0/..", "mp1x", "",
        ] {
            assert!(!valid_disk_key(bad), "{bad}");
        }
        assert!(valid_net_key("net0") && valid_net_key("net31"));
        assert!(!valid_net_key("net32") && !valid_net_key("net") && !valid_net_key("net0;"));
        assert!(valid_bridge("vmbr0") && valid_bridge("vmbr1.100"));
        assert!(
            !valid_bridge("vmbr0,tag=1") && !valid_bridge("") && !valid_bridge(&"a".repeat(16))
        );
        assert!(
            valid_guest_name("web-01.lab") && !valid_guest_name("-x") && !valid_guest_name("a b")
        );
        assert!(valid_ciuser("debian") && valid_ciuser("_svc-1"));
        assert!(!valid_ciuser("Root") && !valid_ciuser("1x") && !valid_ciuser("a;b"));
        assert!(valid_vmid(100) && !valid_vmid(99));
        assert!(valid_cidr("10.0.0.5/24") && valid_cidr("fd00::5/64"));
        assert!(!valid_cidr("10.0.0.5") && !valid_cidr("10.0.0.5/33") && !valid_cidr("x/24"));
    }

    #[test]
    fn net_strings() {
        let s = "virtio=BC:24:11:00:00:01,bridge=vmbr0,firewall=1";
        assert_eq!(
            net_with(s, "vmbr1", Some(20)).unwrap(),
            "virtio=BC:24:11:00:00:01,bridge=vmbr1,tag=20,firewall=1"
        );
        let tagged = "name=eth0,bridge=vmbr0,tag=5,ip=dhcp,type=veth";
        assert_eq!(
            net_with(tagged, "vmbr0", None).unwrap(),
            "name=eth0,bridge=vmbr0,ip=dhcp,type=veth"
        );
        assert_eq!(
            net_with(tagged, "vmbr0", Some(7)).unwrap(),
            "name=eth0,bridge=vmbr0,tag=7,ip=dhcp,type=veth"
        );
        assert!(net_with(s, "vmbr0,x=1", None).is_none());
        assert!(net_with(s, "vmbr0", Some(4095)).is_none());
        assert_eq!(build_net(&parse_net(s)), s);
        assert_eq!(net_get(&parse_net(tagged), "tag"), Some("5"));
    }

    #[test]
    fn hardware() {
        let cfg = json!({
            "cores": 2, "sockets": 1, "memory": "4096", "balloon": 1024,
            "scsi0": "local-lvm:vm-100-disk-0,iothread=1,size=32G",
            "virtio1": "local-lvm:vm-100-disk-1,size=512M",
            "ide2": "local:iso/debian.iso,media=cdrom,size=600M",
            "ide0": "local-lvm:vm-100-cloudinit,media=cdrom",
            "efidisk0": "local-lvm:vm-100-disk-2,size=4M",
            "net1": "virtio=AA:BB:CC:DD:EE:02,bridge=vmbr1",
            "net0": "virtio=AA:BB:CC:DD:EE:01,bridge=vmbr0,tag=10",
            "digest": "0123abcd"
        });
        let hw = parse_hw(&cfg, Kind::Qemu);
        assert_eq!(
            (hw.cores, hw.sockets, hw.memory, hw.balloon),
            (2, 1, 4096, Some(1024))
        );
        let disks: Vec<(&str, f64)> = hw.disks.iter().map(|d| (d.key.as_str(), d.gib)).collect();
        assert_eq!(disks, vec![("scsi0", 32.0), ("virtio1", 0.5)]);
        assert_eq!(hw.nets[0].0, "net0");
        assert_eq!(hw.digest.as_deref(), Some("0123abcd"));
        assert!(has_cloudinit(&cfg));
        assert!(!has_cloudinit(
            &json!({"ide2":"local:iso/x.iso,media=cdrom"})
        ));

        let cth = parse_hw(
            &json!({"cores":1,"memory":512,"swap":256,"rootfs":"local-lvm:subvol-101-disk-0,size=8G","mp0":"local:101/x.raw,mp=/data,size=1T"}),
            Kind::Lxc,
        );
        assert_eq!(cth.swap, 256);
        assert_eq!(cth.disks[0].key, "rootfs");
        assert_eq!(cth.disks[1].gib, 1024.0);
        assert_eq!(parse_size("1048576K"), Some(1.0));
        assert_eq!(parse_size("12X"), None);

        let ch = HwChange {
            set: vec![
                ("cores".into(), "4".into()),
                ("net0".into(), "virtio=AA:BB,bridge=vmbr1,tag=20".into()),
            ],
            resize: vec![("scsi0".into(), 10)],
            digest: Some("0123abcd".into()),
        };
        let r = hw_requests(&vm(), &ch).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].0, r[0].1.as_str()),
            (Method::Put, "/nodes/pve1/qemu/100/config")
        );
        assert_eq!(
            form(&r[0].2),
            vec![
                ("cores", "4"),
                ("net0", "virtio=AA:BB,bridge=vmbr1,tag=20"),
                ("digest", "0123abcd")
            ]
        );
        assert_eq!(
            (r[1].0, r[1].1.as_str()),
            (Method::Put, "/nodes/pve1/qemu/100/resize")
        );
        assert_eq!(form(&r[1].2), vec![("disk", "scsi0"), ("size", "+10G")]);
        // Unknown keys, bad values and disks are refused.
        for set in [
            vec![("args".to_string(), "-x".to_string())],
            vec![("cores".into(), "0".into())],
            vec![("swap".into(), "100".into())],
            vec![("net0".into(), "virtio=AA,bridge=vmbr0\nx".into())],
        ] {
            let bad = HwChange {
                set,
                ..Default::default()
            };
            assert!(hw_requests(&vm(), &bad).is_none());
        }
        let bad = HwChange {
            resize: vec![("scsi0/../x".into(), 1)],
            ..Default::default()
        };
        assert!(hw_requests(&vm(), &bad).is_none());
        let ok = HwChange {
            set: vec![("swap".into(), "1024".into())],
            ..Default::default()
        };
        assert!(hw_requests(&ct(), &ok).is_some());
        assert_eq!(
            pending_keys(&json!([
                {"key":"cores","value":2,"pending":4},
                {"key":"memory","value":2048},
                {"key":"net1","delete":1},
                {"key":"digest","value":"x"}
            ])),
            vec!["cores", "net1"]
        );
    }

    #[test]
    fn clone_and_cloudinit() {
        let spec = CloneSpec {
            newid: 120,
            name: "web-02".into(),
            target: "pve2".into(),
            full: true,
            storage: Some("local-lvm".into()),
            from_template: false,
            cloud_init: None,
            start: false,
        };
        let (m, p, fm) = clone_request(&vm(), &spec).unwrap();
        assert_eq!(
            (m, p.as_str()),
            (Method::Post, "/nodes/pve1/qemu/100/clone")
        );
        assert_eq!(
            form(&fm),
            vec![
                ("newid", "120"),
                ("name", "web-02"),
                ("target", "pve2"),
                ("full", "1"),
                ("storage", "local-lvm")
            ]
        );
        // Linked clones only from templates; never onto the own VMID.
        let linked = CloneSpec {
            full: false,
            ..spec.clone()
        };
        assert!(clone_request(&vm(), &linked).is_none());
        let (_, _, fm) = clone_request(
            &vm(),
            &CloneSpec {
                from_template: true,
                ..linked.clone()
            },
        )
        .unwrap();
        assert!(fm.contains(&f("full", "0")) && !fm.iter().any(|(k, _)| k == "storage"));
        assert!(
            clone_request(
                &vm(),
                &CloneSpec {
                    newid: 100,
                    ..spec.clone()
                }
            )
            .is_none()
        );
        assert!(
            clone_request(
                &vm(),
                &CloneSpec {
                    name: "a b".into(),
                    ..spec.clone()
                }
            )
            .is_none()
        );
        let (_, p, fm) = clone_request(
            &ct(),
            &CloneSpec {
                target: "pve1".into(),
                storage: None,
                ..spec.clone()
            },
        )
        .unwrap();
        assert_eq!(p, "/nodes/pve1/lxc/101/clone");
        assert_eq!(
            form(&fm),
            vec![("newid", "120"), ("hostname", "web-02"), ("full", "1")]
        );

        assert_eq!(ipconfig_value(None).unwrap(), "ip=dhcp");
        assert_eq!(
            ipconfig_value(Some(("10.0.0.5/24", "10.0.0.1"))).unwrap(),
            "ip=10.0.0.5/24,gw=10.0.0.1"
        );
        assert_eq!(
            ipconfig_value(Some(("fd00::5/64", ""))).unwrap(),
            "ip6=fd00::5/64"
        );
        assert!(ipconfig_value(Some(("10.0.0.5/24", "fd00::1"))).is_none());
        assert!(ipconfig_value(Some(("10.0.0.5", ""))).is_none());

        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me@host";
        assert_eq!(
            sshkeys_param(&[key.into()]).unwrap(),
            "ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl%20me%40host"
        );
        let two = sshkeys_param(&[key.into(), format!(" {key} ")]).unwrap();
        assert_eq!(two.matches("%0A").count(), 1);
        assert!(!two.contains(' ') && !two.contains('+'));
        assert!(sshkeys_param(&["not a key".into()]).is_none());
        assert!(sshkeys_param(&[format!("{key}\nssh-rsa AAAAAAAAAAAAAAAAAAAA")]).is_none());

        let ci = CloudInit {
            user: "debian".into(),
            ip: Some(("192.168.10.80/24".into(), "192.168.10.1".into())),
            sshkeys: vec![key.into()],
        };
        let full = CloneSpec {
            cloud_init: Some(ci.clone()),
            start: true,
            ..spec.clone()
        };
        let steps = ext_requests(&vm(), &ExtOp::Clone(full.clone())).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1].1, "/nodes/pve2/qemu/120/config");
        let ciform = form(&steps[1].2);
        assert_eq!(ciform[0], ("ciuser", "debian"));
        assert_eq!(
            ciform[1],
            ("ipconfig0", "ip=192.168.10.80/24,gw=192.168.10.1")
        );
        assert_eq!(ciform[2].0, "sshkeys");
        assert_eq!(steps[2].1, "/nodes/pve2/qemu/120/status/start");
        // Cloud-init is a VM feature; a bad user stops the whole clone.
        assert!(ext_requests(&ct(), &ExtOp::Clone(full.clone())).is_none());
        let bad_user = CloneSpec {
            cloud_init: Some(CloudInit {
                user: "x y".into(),
                ..ci
            }),
            ..full
        };
        assert!(ext_requests(&vm(), &ExtOp::Clone(bad_user)).is_none());
    }

    #[test]
    fn migrate_and_power() {
        let s = MigrateSpec {
            target: "pve2".into(),
            running: true,
            storage: None,
            local_disks: false,
        };
        let (_, p, fm) = migrate_request(&vm(), &s).unwrap();
        assert_eq!(p, "/nodes/pve1/qemu/100/migrate");
        assert_eq!(form(&fm), vec![("target", "pve2"), ("online", "1")]);
        let (_, p, fm) = migrate_request(
            &ct(),
            &MigrateSpec {
                storage: Some("local-lvm".into()),
                ..s.clone()
            },
        )
        .unwrap();
        assert_eq!(p, "/nodes/pve1/lxc/101/migrate");
        assert_eq!(
            form(&fm),
            vec![
                ("target", "pve2"),
                ("restart", "1"),
                ("target-storage", "local-lvm")
            ]
        );
        let (_, _, fm) = migrate_request(
            &vm(),
            &MigrateSpec {
                running: false,
                ..s.clone()
            },
        )
        .unwrap();
        assert_eq!(form(&fm), vec![("target", "pve2")]);
        let (_, _, fm) = migrate_request(
            &vm(),
            &MigrateSpec {
                local_disks: true,
                ..s.clone()
            },
        )
        .unwrap();
        assert_eq!(
            form(&fm),
            vec![
                ("target", "pve2"),
                ("online", "1"),
                ("with-local-disks", "1")
            ]
        );
        for bad in ["pve1", "pve2;reboot", ""] {
            assert!(
                migrate_request(
                    &vm(),
                    &MigrateSpec {
                        target: bad.into(),
                        ..s.clone()
                    }
                )
                .is_none()
            );
        }
        let steps = ext_requests(&ct(), &ExtOp::Power(Power::Shutdown)).unwrap();
        assert_eq!(steps[0].1, "/nodes/pve1/lxc/101/status/shutdown");
        let bad = GuestId {
            node: "x/../y".into(),
            ..vm()
        };
        assert!(ext_requests(&bad, &ExtOp::Power(Power::Start)).is_none());
        let stores = parse_stores(
            &json!([
                {"storage":"local-lvm","type":"lvmthin","content":"images,rootdir","active":1,"avail":10},
                {"storage":"local","type":"dir","content":"iso,backup"},
                {"storage":"ceph","type":"rbd","content":"images","shared":1},
                {"storage":"bad id","content":"images"}
            ]),
            "images",
        );
        assert_eq!(
            stores
                .iter()
                .map(|s| (s.id.as_str(), s.shared))
                .collect::<Vec<_>>(),
            vec![("ceph", true), ("local-lvm", false)]
        );
    }
}
