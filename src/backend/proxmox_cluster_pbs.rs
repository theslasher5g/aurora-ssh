//! Proxmox Backup Server: datastores with usage, snapshots grouped by backup group,
//! a read-only prune preview computed locally, and the download of small `.blob` files
//! (guest config, client log). Same pinned-certificate TLS as Proxmox VE; tokens use
//! `PBSAPIToken=user@realm!name:secret`, tickets the `PBSAuthCookie`.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Local, TimeZone, Timelike};
use serde_json::Value;
use uuid::Uuid;

use super::super::{Client, Conn, Epoch, Error, Method, Sink, enc, num};
use super::{ClusterEvent, optional};

/// API client for a backup server.
fn client(conn: &Conn) -> Client {
    let mut c = Client::new(conn);
    c.pbs = true;
    c
}

// ------------------------------------------------------------------ validation

fn name_chars(s: &str, max: usize) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !s.contains("..")
}

/// Datastore names (PBS: `[A-Za-z0-9_][A-Za-z0-9._-]*`).
pub fn valid_datastore(s: &str) -> bool {
    name_chars(s, 64)
}

pub fn valid_backup_type(s: &str) -> bool {
    matches!(s, "vm" | "ct" | "host")
}

/// Backup ids (VMID or host name).
pub fn valid_backup_id(s: &str) -> bool {
    name_chars(s, 128)
}

/// Archive names inside a snapshot that may be downloaded: decoded blobs only.
pub fn valid_blob_name(s: &str) -> bool {
    name_chars(s, 128) && s.ends_with(".blob") && s.len() > ".blob".len()
}

// ------------------------------------------------------------------ data

#[derive(Clone, Debug, PartialEq)]
pub struct Datastore {
    pub name: String,
    pub comment: String,
    pub total: u64,
    pub used: u64,
    /// Usage could not be read (offline, no permission).
    pub error: Option<String>,
}

impl Datastore {
    pub fn usage(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64) as f32
        }
    }
}

/// `data` of `/admin/datastore` (+ `/status/datastore-usage` for the numbers).
pub fn parse_datastores(list: &Value, usage: Option<&Value>) -> Vec<Datastore> {
    let mut by_name: HashMap<&str, &Value> = HashMap::new();
    for u in usage
        .and_then(|u| u.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        if let Some(n) = u["store"].as_str() {
            by_name.insert(n, u);
        }
    }
    let mut out: Vec<Datastore> = list
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|d| {
            let name = d["store"].as_str().or_else(|| d["name"].as_str())?;
            let u = by_name.get(name);
            Some(Datastore {
                name: name.to_string(),
                comment: d["comment"].as_str().unwrap_or_default().to_string(),
                total: u.map(|u| num(&u["total"]) as u64).unwrap_or_default(),
                used: u.map(|u| num(&u["used"]) as u64).unwrap_or_default(),
                error: u.and_then(|u| u["error"].as_str()).map(str::to_string),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct PbsFile {
    pub name: String,
    pub size: Option<u64>,
    pub encrypted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PbsSnapshot {
    pub btype: String,
    pub bid: String,
    pub time: i64,
    pub size: Option<u64>,
    pub protected: bool,
    /// `ok` / `failed`, `None` = never verified.
    pub verify: Option<String>,
    pub comment: String,
    pub owner: String,
    pub files: Vec<PbsFile>,
}

/// `data` of `/admin/datastore/{store}/snapshots`, newest first.
pub fn parse_snapshots(data: &Value) -> Vec<PbsSnapshot> {
    let mut out: Vec<PbsSnapshot> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let files = s["files"]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .filter_map(|f| match f {
                    Value::String(name) => Some(PbsFile {
                        name: name.clone(),
                        size: None,
                        encrypted: false,
                    }),
                    _ => Some(PbsFile {
                        name: f["filename"].as_str()?.to_string(),
                        size: f["size"].as_u64(),
                        encrypted: f["crypt-mode"].as_str() == Some("encrypt"),
                    }),
                })
                .collect();
            Some(PbsSnapshot {
                btype: s["backup-type"].as_str()?.to_string(),
                bid: s["backup-id"].as_str()?.to_string(),
                time: s["backup-time"].as_i64()?,
                size: s["size"].as_u64(),
                protected: s["protected"]
                    .as_bool()
                    .unwrap_or(num(&s["protected"]) != 0.0),
                verify: s["verification"]["state"].as_str().map(str::to_string),
                comment: s["comment"].as_str().unwrap_or_default().to_string(),
                owner: s["owner"].as_str().unwrap_or_default().to_string(),
                files,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (a.btype.as_str(), a.bid.as_str())
            .cmp(&(b.btype.as_str(), b.bid.as_str()))
            .then(b.time.cmp(&a.time))
    });
    out
}

/// Snapshots by `type/id`, in the order of [`parse_snapshots`].
pub fn groups(snaps: &[PbsSnapshot]) -> Vec<((String, String), Vec<&PbsSnapshot>)> {
    let mut out: Vec<((String, String), Vec<&PbsSnapshot>)> = Vec::new();
    for s in snaps {
        let key = (s.btype.clone(), s.bid.clone());
        match out.last_mut() {
            Some((k, v)) if *k == key => v.push(s),
            _ => out.push((key, vec![s])),
        }
    }
    out
}

// ------------------------------------------------------------------ prune preview

/// Retention like a PBS prune job (`keep-*`); all zero = keep everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keep {
    pub last: u32,
    pub hourly: u32,
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
    pub yearly: u32,
}

impl Default for Keep {
    fn default() -> Self {
        Self {
            last: 3,
            hourly: 0,
            daily: 7,
            weekly: 4,
            monthly: 6,
            yearly: 0,
        }
    }
}

/// `keep-*` options of the first prune job for `store` in `/config/prune`.
pub fn parse_prune_job(data: &Value, store: &str) -> Option<Keep> {
    let job = data
        .as_array()?
        .iter()
        .find(|j| j["store"].as_str() == Some(store))?;
    let k = |name: &str| num(&job[name]) as u32;
    Some(Keep {
        last: k("keep-last"),
        hourly: k("keep-hourly"),
        daily: k("keep-daily"),
        weekly: k("keep-weekly"),
        monthly: k("keep-monthly"),
        yearly: k("keep-yearly"),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Keep,
    Protected,
    Remove,
}

/// Which snapshots of one group a prune with `keep` would keep – the algorithm of
/// `proxmox-backup` (`compute_prune_info`): newest first, every option keeps the newest
/// snapshot of each of its periods, protected snapshots are never removed and do not
/// count. `times` in any order; the result matches it index by index.
pub fn prune_preview<Tz: TimeZone>(times: &[(i64, bool)], keep: Keep, tz: &Tz) -> Vec<Mark> {
    let mut order: Vec<usize> = (0..times.len()).collect();
    order.sort_by(|a, b| times[*b].0.cmp(&times[*a].0));
    let mut mark: Vec<Option<Mark>> = vec![None; times.len()];
    let keep_all = keep
        == Keep {
            last: 0,
            hourly: 0,
            daily: 0,
            weekly: 0,
            monthly: 0,
            yearly: 0,
        };
    if keep_all {
        return times
            .iter()
            .map(|(_, p)| if *p { Mark::Protected } else { Mark::Keep })
            .collect();
    }
    let period = |t: i64, what: u8| -> String {
        let Some(d) = tz.timestamp_opt(t, 0).single() else {
            return t.to_string();
        };
        match what {
            0 => t.to_string(),
            1 => format!("{}/{}/{}/{}", d.year(), d.month(), d.day(), d.hour()),
            2 => format!("{}/{}/{}", d.year(), d.month(), d.day()),
            3 => {
                let w = d.iso_week();
                format!("{}/{}", w.year(), w.week())
            }
            4 => format!("{}/{}", d.year(), d.month()),
            _ => d.year().to_string(),
        }
    };
    let options = [
        (keep.last, 0u8),
        (keep.hourly, 1),
        (keep.daily, 2),
        (keep.weekly, 3),
        (keep.monthly, 4),
        (keep.yearly, 5),
    ];
    for (count, what) in options {
        if count == 0 {
            continue;
        }
        let mut included: HashSet<String> = HashSet::new();
        let already: HashSet<String> = order
            .iter()
            .filter(|i| mark[**i] == Some(Mark::Keep))
            .map(|i| period(times[*i].0, what))
            .collect();
        for &i in &order {
            if mark[i].is_some() {
                continue;
            }
            if times[i].1 {
                mark[i] = Some(Mark::Protected);
                continue;
            }
            let id = period(times[i].0, what);
            if already.contains(&id) {
                continue;
            }
            if !included.contains(&id) {
                if included.len() >= count as usize {
                    break;
                }
                included.insert(id);
                mark[i] = Some(Mark::Keep);
            } else {
                mark[i] = Some(Mark::Remove);
            }
        }
    }
    mark.into_iter()
        .zip(times)
        .map(|(m, (_, p))| match m {
            Some(m) => m,
            None if *p => Mark::Protected,
            None => Mark::Remove,
        })
        .collect()
}

/// [`prune_preview`] in the local time zone, like the server's prune jobs.
pub fn prune_preview_local(times: &[(i64, bool)], keep: Keep) -> Vec<Mark> {
    prune_preview(times, keep, &Local)
}

// ------------------------------------------------------------------ jobs

pub enum PbsPayload {
    Stores {
        result: Result<Vec<Datastore>, Error>,
        /// Retention of each datastore's prune job, if readable.
        keep: HashMap<String, Keep>,
    },
    Snapshots {
        store: String,
        result: Result<Vec<PbsSnapshot>, Error>,
    },
    Download {
        name: String,
        result: Result<Vec<u8>, Error>,
    },
}

fn job(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    sink: Sink<ClusterEvent>,
    f: impl FnOnce(&mut Client) -> PbsPayload + Send + 'static,
) {
    rt.spawn_blocking(move || {
        let mut c = client(&conn);
        let payload = f(&mut c);
        sink.send(ClusterEvent::Pbs {
            server,
            epoch,
            session: c.session.clone(),
            payload,
        });
    });
}

pub fn spawn_stores(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, |c| {
        let mut keep = HashMap::new();
        let result = (|| {
            let list = c.call(false, "/admin/datastore")?;
            let usage = optional(c.call(false, "/status/datastore-usage"))?;
            let mut stores = parse_datastores(&list, usage.as_ref());
            for d in stores
                .iter_mut()
                .filter(|d| d.total == 0 && d.error.is_none())
            {
                if !valid_datastore(&d.name) {
                    continue;
                }
                match c.call(false, &format!("/admin/datastore/{}/status", d.name)) {
                    Ok(v) => {
                        d.total = num(&v["total"]) as u64;
                        d.used = num(&v["used"]) as u64;
                    }
                    Err(e) => d.error = Some(e.to_string()),
                }
            }
            if let Some(jobs) = optional(c.call(false, "/config/prune"))? {
                for d in &stores {
                    if let Some(k) = parse_prune_job(&jobs, &d.name) {
                        keep.insert(d.name.clone(), k);
                    }
                }
            }
            Ok(stores)
        })();
        PbsPayload::Stores { result, keep }
    });
}

pub fn spawn_snapshots(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    store: String,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, move |c| {
        let result = if valid_datastore(&store) {
            c.call(false, &format!("/admin/datastore/{store}/snapshots"))
                .map(|v| parse_snapshots(&v))
        } else {
            Err(Error::Net(t!("ungültiger Datastore-Name").into()))
        };
        PbsPayload::Snapshots { store, result }
    });
}

/// `download-decoded` URL path for a blob of a snapshot, or `None` for anything that
/// does not pass validation.
pub fn download_path(store: &str, s: &PbsSnapshot, file: &str) -> Option<String> {
    (valid_datastore(store)
        && valid_backup_type(&s.btype)
        && valid_backup_id(&s.bid)
        && valid_blob_name(file)
        && s.time > 0)
        .then(|| {
            format!(
                "/admin/datastore/{store}/download-decoded?backup-type={}&backup-id={}&backup-time={}&file-name={}",
                enc(&s.btype),
                enc(&s.bid),
                s.time,
                enc(file)
            )
        })
}

/// Largest blob we download into memory.
const MAX_BLOB: u64 = 16 * 1024 * 1024;

#[allow(clippy::too_many_arguments)]
pub fn spawn_download(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    store: String,
    snap: PbsSnapshot,
    file: String,
    sink: Sink<ClusterEvent>,
) {
    job(rt, server, epoch, conn, sink, move |c| {
        let result = (|| {
            let path = download_path(&store, &snap, &file)
                .ok_or_else(|| Error::Net(t!("ungültiger Dateiname").into()))?;
            // Logs in (ticket) and checks the certificate.
            c.call(false, "/version")?;
            let url = format!("{}{path}", c.base);
            let mut req = c.agent.get(&url);
            for (k, v) in c.auth_headers(Method::Get) {
                req = req.header(k, v);
            }
            let mut resp = req.call().map_err(|e| c.net_error(e))?;
            let status = resp.status().as_u16();
            if status != 200 {
                let body = resp.body_mut().read_to_string().unwrap_or_default();
                return Err(Error::Http(status, super::super::short(&body)));
            }
            resp.body_mut()
                .with_config()
                .limit(MAX_BLOB)
                .read_to_vec()
                .map_err(|e| Error::Net(e.to_string()))
        })();
        PbsPayload::Download { name: file, result }
    });
}

/// "Verbindung testen": logs in and reads `/version`.
pub fn spawn_test(rt: &tokio::runtime::Runtime, conn: Conn, sink: Sink<ClusterEvent>) {
    rt.spawn_blocking(move || {
        let mut c = client(&conn);
        let result = c.call(false, "/version").map(|d| {
            format!(
                "Proxmox Backup Server {}",
                d["version"].as_str().unwrap_or("?").trim()
            )
        });
        sink.send(ClusterEvent::PbsTested(result));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn validation() {
        assert!(valid_datastore("store1") && valid_datastore("off_site.2"));
        for bad in ["", "../x", "a/b", "a b", ".hidden", "x..y", "a?b"] {
            assert!(!valid_datastore(bad), "{bad}");
        }
        assert!(valid_backup_type("vm") && !valid_backup_type("vm/../"));
        assert!(valid_backup_id("100") && valid_backup_id("web-host.lan"));
        assert!(!valid_backup_id("100&backup-type=ct"));
        assert!(valid_blob_name("qemu-server.conf.blob") && valid_blob_name("client.log.blob"));
        assert!(!valid_blob_name("drive-scsi0.img.fidx"));
        assert!(!valid_blob_name("../index.json.blob") && !valid_blob_name(".blob"));
    }

    #[test]
    fn datastores_and_snapshots() {
        let stores = parse_datastores(
            &json!([{"store":"store1","comment":"main"},{"store":"offsite"}]),
            Some(
                &json!([{"store":"store1","total":100,"used":70},{"store":"offsite","error":"offline"}]),
            ),
        );
        assert_eq!(stores[1].name, "store1");
        assert!((stores[1].usage() - 0.7).abs() < 1e-6);
        assert_eq!(stores[0].error.as_deref(), Some("offline"));

        let snaps = parse_snapshots(&json!([
            {"backup-type":"vm","backup-id":"100","backup-time":100,"size":5,"protected":true,
             "verification":{"state":"ok","upid":"x"},"files":[{"filename":"qemu-server.conf.blob","size":300},{"filename":"drive-scsi0.img.fidx","crypt-mode":"encrypt"}]},
            {"backup-type":"vm","backup-id":"100","backup-time":200,"files":["index.json.blob"]},
            {"backup-type":"ct","backup-id":"101","backup-time":150},
            {"backup-type":"vm"}
        ]));
        assert_eq!(snaps.len(), 3);
        assert_eq!(
            (snaps[0].btype.as_str(), snaps[0].bid.as_str()),
            ("ct", "101")
        );
        assert_eq!(snaps[1].time, 200);
        assert!(snaps[2].protected && snaps[2].files[1].encrypted);
        assert_eq!(snaps[2].verify.as_deref(), Some("ok"));
        let g = groups(&snaps);
        assert_eq!(g.len(), 2);
        assert_eq!(g[1].1.len(), 2);

        assert_eq!(
            download_path("store1", &snaps[2], "qemu-server.conf.blob").unwrap(),
            "/admin/datastore/store1/download-decoded?backup-type=vm&backup-id=100&backup-time=100&file-name=qemu-server.conf.blob"
        );
        assert_eq!(
            download_path("store1", &snaps[2], "drive-scsi0.img.fidx"),
            None
        );
        assert_eq!(download_path("../store", &snaps[2], "a.blob"), None);
        let keep = parse_prune_job(
            &json!([{"id":"j1","store":"store1","keep-last":2,"keep-daily":5}]),
            "store1",
        )
        .unwrap();
        assert_eq!((keep.last, keep.daily, keep.weekly), (2, 5, 0));
    }

    #[test]
    fn prune() {
        let day = 86_400;
        let base = 1_700_000_000 - 1_700_000_000 % day + 3600; // 01:00 UTC
        // Two backups a day for 10 days, oldest first; the oldest one is protected.
        let mut times = Vec::new();
        for d in 0..10 {
            times.push((base + d * day, d == 0));
            times.push((base + d * day + 7200, false));
        }
        let keep = Keep {
            last: 2,
            hourly: 0,
            daily: 3,
            weekly: 0,
            monthly: 0,
            yearly: 0,
        };
        let m = prune_preview(&times, keep, &Utc);
        // keep-last: the two newest (day 9 both); keep-daily 3: newest of days 8, 7, 6.
        let kept: Vec<usize> = (0..m.len()).filter(|i| m[*i] == Mark::Keep).collect();
        assert_eq!(kept, vec![13, 15, 17, 18, 19]);
        assert_eq!(m[0], Mark::Protected);
        assert_eq!(m.iter().filter(|x| **x == Mark::Remove).count(), 14);
        let all = prune_preview(
            &times,
            Keep {
                last: 0,
                hourly: 0,
                daily: 0,
                weekly: 0,
                monthly: 0,
                yearly: 0,
            },
            &Utc,
        );
        assert!(all.iter().all(|m| *m != Mark::Remove));
    }
}
