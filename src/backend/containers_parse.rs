//! Container overview: validation of names, parsing of `docker ps` / `docker stats` JSON
//! lines and `kubectl get pods -o json`, grouping, summaries and the command lines for the
//! actions. Pure functions, no I/O.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// What the page remembers (saved in the library, no secrets).
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CtrPrefs {
    /// SSH hosts the user marked as container hosts.
    pub marked: Vec<Uuid>,
    /// Hosts the user does not want to see on the page even if docker was found.
    pub hidden: Vec<Uuid>,
    /// Default namespace per host (`local` for this PC).
    pub namespace: HashMap<String, String>,
    /// Kubernetes context per host (`local` for this PC).
    pub context: HashMap<String, String>,
}

/// Key used in the maps of [`CtrPrefs`].
pub fn pref_key(host: Option<Uuid>) -> String {
    host.map(|h| h.to_string())
        .unwrap_or_else(|| "local".into())
}

/// A name, id, namespace, context or image reference that may be put on a command line:
/// only `[A-Za-z0-9_.-/:@]`, not empty, not longer than 253 characters and not starting
/// with `-` (that would be taken for an option).
pub fn valid_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':' | '@'))
}

// ---------------------------------------------------------------------------
// JSON helpers

/// Value of the first key that exists (compared case-insensitively) as text.
fn pick(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    for k in keys {
        let found = obj
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(k))
            .map(|(_, v)| v);
        if let Some(v) = found {
            let s = text_of(v);
            if !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a
            .iter()
            .map(text_of)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

fn num_of(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `"12.5%"` → 12.5; `"--"`, `"NaN"` and garbage → `None`.
pub fn parse_percent(s: &str) -> Option<f32> {
    let v: f32 = s.trim().trim_end_matches('%').trim().parse().ok()?;
    (v.is_finite() && v >= 0.0).then_some(v)
}

/// JSON objects of a command output: one per line (`docker`), a JSON array of objects
/// (`podman`) or a pretty printed array. Everything else is returned as `notes` (warnings,
/// error messages), capped.
fn json_objects(out: &str) -> (Vec<serde_json::Map<String, Value>>, Vec<String>) {
    let mut objs = Vec::new();
    let mut notes = Vec::new();
    let whole = out.trim();
    if whole.starts_with('[')
        && let Ok(Value::Array(a)) = serde_json::from_str::<Value>(whole)
    {
        for v in a {
            if let Value::Object(o) = v {
                objs.push(o);
            }
        }
        return (objs, notes);
    }
    for line in out.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("@@") {
            continue;
        }
        match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(o)) => objs.push(o),
            Ok(Value::Array(a)) => {
                for v in a {
                    if let Value::Object(o) = v {
                        objs.push(o);
                    }
                }
            }
            _ => {
                if notes.len() < 5 {
                    notes.push(l.chars().take(200).collect());
                }
            }
        }
    }
    (objs, notes)
}

// ---------------------------------------------------------------------------
// Docker / Podman

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Health {
    #[default]
    None,
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, `created`, `restarting` …
    pub state: String,
    pub status: String,
    /// Published ports, shortened (`8080->80/tcp, 443->443/tcp`).
    pub ports: String,
    /// Creation time as `2026-10-16 10:21`, or what the CLI said.
    pub created: String,
    pub project: Option<String>,
    pub service: Option<String>,
    pub health: Health,
    pub cpu: Option<f32>,
    pub mem_usage: String,
    pub mem_pct: Option<f32>,
}

impl DockerContainer {
    pub fn running(&self) -> bool {
        matches!(self.state.as_str(), "running" | "restarting" | "paused")
    }
    pub fn paused(&self) -> bool {
        self.state == "paused"
    }
}

/// `key=value` from docker's comma separated label string.
pub fn label_value(labels: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=");
    let mut from = 0;
    while let Some(i) = labels[from..].find(&pat) {
        let at = from + i;
        if at == 0 || labels.as_bytes()[at - 1] == b',' {
            let rest = &labels[at + pat.len()..];
            let end = rest.find(',').unwrap_or(rest.len());
            let v = rest[..end].trim();
            return (!v.is_empty()).then(|| v.to_string());
        }
        from = at + pat.len();
    }
    None
}

/// `0.0.0.0:8080->80/tcp, :::8080->80/tcp` → `8080->80/tcp` (IPv6 duplicates and the
/// wildcard address dropped).
pub fn short_ports(s: &str) -> String {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for p in s.split(',') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let p = p
            .strip_prefix("0.0.0.0:")
            .or_else(|| p.strip_prefix("[::]:"))
            .or_else(|| p.strip_prefix(":::"))
            .unwrap_or(p);
        if seen.insert(p.to_string()) {
            out.push(p.to_string());
        }
    }
    out.join(", ")
}

fn ports_of(o: &serde_json::Map<String, Value>) -> String {
    match o
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("ports"))
        .map(|(_, v)| v)
    {
        Some(Value::Array(a)) => {
            // podman: [{host_ip, container_port, host_port, range, protocol}]
            let list: Vec<String> = a
                .iter()
                .filter_map(|p| {
                    let p = p.as_object()?;
                    let host = num_of(p.get("host_port"))?;
                    let cont = num_of(p.get("container_port"))?;
                    let proto = p.get("protocol").and_then(Value::as_str).unwrap_or("tcp");
                    Some(format!("{host}->{cont}/{proto}"))
                })
                .collect();
            short_ports(&list.join(", "))
        }
        Some(Value::String(s)) => short_ports(s),
        _ => String::new(),
    }
}

fn created_of(o: &serde_json::Map<String, Value>) -> String {
    let at = pick(o, &["CreatedAt"]);
    if !at.is_empty() {
        return at.chars().take(16).collect();
    }
    if let Some(n) = num_of(o.get("Created")).filter(|n| *n > 1_000_000_000)
        && let Some(t) = DateTime::from_timestamp(n as i64, 0)
    {
        return t.format("%Y-%m-%d %H:%M").to_string();
    }
    pick(o, &["Created", "RunningFor"])
}

fn health_of(status: &str) -> Health {
    let low = status.to_lowercase();
    if low.contains("(unhealthy)") {
        Health::Unhealthy
    } else if low.contains("(healthy)") {
        Health::Healthy
    } else if low.contains("health: starting") {
        Health::Starting
    } else {
        Health::None
    }
}

pub struct Parsed<T> {
    pub items: Vec<T>,
    /// Lines that were not JSON (warnings, errors).
    pub notes: Vec<String>,
}

/// Parses `docker ps -a --format '{{json .}}'` (also podman's array form).
pub fn parse_ps(out: &str) -> Parsed<DockerContainer> {
    let (objs, notes) = json_objects(out);
    let items = objs
        .iter()
        .filter_map(|o| {
            let id = pick(o, &["ID", "Id"]);
            let name = pick(o, &["Names", "Name"])
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .trim_start_matches('/')
                .to_string();
            if name.is_empty() && id.is_empty() {
                return None;
            }
            let status = pick(o, &["Status"]);
            let mut state = pick(o, &["State"]).to_lowercase();
            if state.is_empty() {
                state = if status.starts_with("Up") {
                    "running".into()
                } else if status.starts_with("Exited") {
                    "exited".into()
                } else {
                    "created".into()
                };
            }
            let (project, service) = match o
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("labels"))
                .map(|(_, v)| v)
            {
                Some(Value::String(l)) => (
                    label_value(l, "com.docker.compose.project"),
                    label_value(l, "com.docker.compose.service"),
                ),
                Some(Value::Object(l)) => (
                    l.get("com.docker.compose.project")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|s| !s.is_empty()),
                    l.get("com.docker.compose.service")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|s| !s.is_empty()),
                ),
                _ => (None, None),
            };
            Some(DockerContainer {
                name: if name.is_empty() {
                    id.chars().take(12).collect()
                } else {
                    name
                },
                id,
                image: pick(o, &["Image"]),
                health: health_of(&status),
                state,
                status,
                ports: ports_of(o),
                created: created_of(o),
                project,
                service,
                ..Default::default()
            })
        })
        .collect();
    Parsed { items, notes }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Stat {
    pub id: String,
    pub name: String,
    pub cpu: Option<f32>,
    pub mem_usage: String,
    pub mem_pct: Option<f32>,
}

/// Parses `docker stats --no-stream --format '{{json .}}'` (docker and podman key names).
pub fn parse_stats(out: &str) -> Vec<Stat> {
    let (objs, _) = json_objects(out);
    objs.iter()
        .map(|o| Stat {
            id: pick(o, &["ID", "Id", "Container", "ContainerID"]),
            name: pick(o, &["Name", "Names"]),
            cpu: parse_percent(&pick(o, &["CPUPerc", "cpu_percent", "CPU"])),
            mem_usage: pick(o, &["MemUsage", "mem_usage"]),
            mem_pct: parse_percent(&pick(o, &["MemPerc", "mem_percent"])),
        })
        .collect()
}

/// Puts CPU/RAM figures on the containers they belong to (matched by name or id prefix).
pub fn apply_stats(items: &mut [DockerContainer], stats: &[Stat]) {
    for c in items.iter_mut() {
        let hit = stats.iter().find(|s| {
            (!s.name.is_empty() && s.name.trim_start_matches('/') == c.name)
                || (s.id.len() >= 4
                    && !c.id.is_empty()
                    && (c.id.starts_with(&s.id) || s.id.starts_with(&c.id)))
        });
        if let Some(s) = hit {
            c.cpu = s.cpu;
            c.mem_pct = s.mem_pct;
            c.mem_usage = s
                .mem_usage
                .split('/')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
        }
    }
}

/// One group of the container list: a compose project, or the loose containers.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub project: Option<String>,
    pub items: Vec<usize>,
}

/// Compose projects (sorted by name) with their containers (sorted by name); containers
/// without a project form the last group. Empty groups are not returned.
pub fn group_by_project(items: &[DockerContainer]) -> Vec<Group> {
    let mut map: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut loose = Vec::new();
    for (i, c) in items.iter().enumerate() {
        match c.project.as_deref() {
            Some(p) => map.entry(p).or_default().push(i),
            None => loose.push(i),
        }
    }
    let by_name = |v: &mut Vec<usize>| v.sort_by_key(|i| items[*i].name.to_lowercase());
    let mut out: Vec<Group> = map
        .into_iter()
        .map(|(p, mut v)| {
            by_name(&mut v);
            Group {
                project: Some(p.to_string()),
                items: v,
            }
        })
        .collect();
    if !loose.is_empty() {
        by_name(&mut loose);
        out.push(Group {
            project: None,
            items: loose,
        });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Summary {
    pub running: usize,
    pub stopped: usize,
    pub paused: usize,
    pub unhealthy: usize,
}

pub fn summarize(items: &[DockerContainer]) -> Summary {
    let mut s = Summary::default();
    for c in items {
        if c.paused() {
            s.paused += 1;
        } else if c.running() {
            s.running += 1;
        } else {
            s.stopped += 1;
        }
        if c.health == Health::Unhealthy {
            s.unhealthy += 1;
        }
    }
    s
}

impl Summary {
    /// `12 laufen · 2 gestoppt · 1 unhealthy`
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        parts.push(t!("{} laufen", self.running));
        if self.paused > 0 {
            parts.push(t!("{} pausiert", self.paused));
        }
        if self.stopped > 0 {
            parts.push(t!("{} gestoppt", self.stopped));
        }
        if self.unhealthy > 0 {
            parts.push(t!("{} unhealthy", self.unhealthy));
        }
        parts.join(" · ")
    }
}

// ---------------------------------------------------------------------------
// Inspect

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Inspect {
    pub name: String,
    pub image: String,
    pub created: String,
    pub state: String,
    pub health: String,
    pub restart_policy: String,
    pub command: String,
    /// `(name, value)` of the environment; the UI hides the values by default.
    pub env: Vec<(String, String)>,
    pub mounts: Vec<String>,
    pub networks: Vec<String>,
    pub ports: Vec<String>,
}

/// Parses `docker inspect <name>` (a JSON array with one object).
pub fn parse_inspect(text: &str) -> Option<Inspect> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let o = match &v {
        Value::Array(a) => a.first()?,
        other => other,
    };
    let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or_default().to_string();
    let cfg = o.get("Config");
    let state = o.get("State");
    let host = o.get("HostConfig");
    let mut cmd: Vec<String> = Vec::new();
    if let Some(p) = o.get("Path").and_then(Value::as_str) {
        cmd.push(p.into());
    }
    if let Some(Value::Array(a)) = o.get("Args") {
        cmd.extend(a.iter().filter_map(Value::as_str).map(str::to_string));
    }
    let env = match cfg.and_then(|c| c.get("Env")) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|e| match e.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (e.to_string(), String::new()),
            })
            .collect(),
        _ => Vec::new(),
    };
    let mounts = match o.get("Mounts") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|m| {
                let rw = if m.get("RW").and_then(Value::as_bool) == Some(false) {
                    " (ro)"
                } else {
                    ""
                };
                format!(
                    "{} → {}{rw}",
                    {
                        let src = s(m.get("Source"));
                        if src.is_empty() {
                            s(m.get("Name"))
                        } else {
                            src
                        }
                    },
                    s(m.get("Destination"))
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let net = o.get("NetworkSettings");
    let networks = match net.and_then(|n| n.get("Networks")) {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                let ip = s(v.get("IPAddress"));
                if ip.is_empty() {
                    k.clone()
                } else {
                    format!("{k} ({ip})")
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let ports = match net.and_then(|n| n.get("Ports")) {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| match v {
                Value::Array(b) if !b.is_empty() => {
                    let hosts: Vec<String> = b
                        .iter()
                        .map(|x| format!("{}:{}", s(x.get("HostIp")), s(x.get("HostPort"))))
                        .collect();
                    format!("{} → {k}", hosts.join(", "))
                }
                _ => k.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let policy = host.and_then(|h| h.get("RestartPolicy"));
    let mut restart = s(policy.and_then(|p| p.get("Name")));
    if let Some(n) = num_of(policy.and_then(|p| p.get("MaximumRetryCount"))).filter(|n| *n > 0) {
        restart.push_str(&format!(":{n}"));
    }
    Some(Inspect {
        name: s(o.get("Name")).trim_start_matches('/').to_string(),
        image: s(cfg.and_then(|c| c.get("Image"))),
        created: s(o.get("Created")).chars().take(19).collect(),
        state: s(state.and_then(|x| x.get("Status"))),
        health: s(state
            .and_then(|x| x.get("Health"))
            .and_then(|h| h.get("Status"))),
        restart_policy: if restart.is_empty() {
            "no".into()
        } else {
            restart
        },
        command: cmd.join(" "),
        env,
        mounts,
        networks,
        ports,
    })
}

// ---------------------------------------------------------------------------
// Kubernetes

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Pod {
    pub namespace: String,
    pub name: String,
    /// What `kubectl get pods` prints in STATUS (`Running`, `CrashLoopBackOff` …).
    pub status: String,
    pub ready: u32,
    pub total: u32,
    pub restarts: u32,
    pub created: Option<DateTime<Utc>>,
    pub node: String,
    pub containers: Vec<String>,
    /// `deployment/web`, `statefulset/db` … that owns the pod (for a rollout restart).
    pub owner: Option<String>,
}

fn s_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for p in path {
        match cur.get(p) {
            Some(n) => cur = n,
            None => return "",
        }
    }
    cur.as_str().unwrap_or("")
}

/// Pod owner as a rollout target: ReplicaSet `web-5d8f7c9b6` → `deployment/web`.
fn owner_of(meta: &Value) -> Option<String> {
    let o = meta.get("ownerReferences")?.as_array()?.first()?;
    let name = o.get("name")?.as_str()?;
    match o.get("kind")?.as_str()? {
        "ReplicaSet" => {
            let (dep, hash) = name.rsplit_once('-')?;
            (!dep.is_empty() && !hash.is_empty()).then(|| format!("deployment/{dep}"))
        }
        "StatefulSet" => Some(format!("statefulset/{name}")),
        "DaemonSet" => Some(format!("daemonset/{name}")),
        _ => None,
    }
}

/// The STATUS column the way kubectl derives it.
fn pod_status(p: &Value) -> String {
    let mut reason = s_at(p, &["status", "phase"]).to_string();
    let r = s_at(p, &["status", "reason"]);
    if !r.is_empty() {
        reason = r.to_string();
    }
    let inits = p
        .pointer("/status/initContainerStatuses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut init_pending = false;
    for (i, c) in inits.iter().enumerate() {
        let term = c.pointer("/state/terminated");
        if let Some(t) = term
            && num_of(t.get("exitCode")) == Some(0)
        {
            continue;
        }
        init_pending = true;
        let total = inits.len();
        if let Some(t) = term {
            let rsn = t.get("reason").and_then(Value::as_str).unwrap_or("");
            reason = if rsn.is_empty() {
                format!("Init:ExitCode:{}", num_of(t.get("exitCode")).unwrap_or(0))
            } else {
                format!("Init:{rsn}")
            };
        } else if let Some(w) = c.pointer("/state/waiting/reason").and_then(Value::as_str)
            && w != "PodInitializing"
        {
            reason = format!("Init:{w}");
        } else {
            reason = format!("Init:{i}/{total}");
        }
        break;
    }
    if !init_pending {
        let mut has_running = false;
        if let Some(cs) = p
            .pointer("/status/containerStatuses")
            .and_then(Value::as_array)
        {
            for c in cs.iter().rev() {
                if let Some(w) = c.pointer("/state/waiting/reason").and_then(Value::as_str)
                    && !w.is_empty()
                {
                    reason = w.to_string();
                } else if let Some(t) = c.pointer("/state/terminated") {
                    let rsn = t.get("reason").and_then(Value::as_str).unwrap_or("");
                    reason = if !rsn.is_empty() {
                        rsn.to_string()
                    } else if let Some(sig) = num_of(t.get("signal")).filter(|s| *s > 0) {
                        format!("Signal:{sig}")
                    } else {
                        format!("ExitCode:{}", num_of(t.get("exitCode")).unwrap_or(0))
                    };
                } else if c.pointer("/state/running").is_some()
                    && c.get("ready").and_then(Value::as_bool) == Some(true)
                {
                    has_running = true;
                }
            }
        }
        if reason == "Completed" && has_running {
            reason = "Running".into();
        }
    }
    if p.pointer("/metadata/deletionTimestamp").is_some() {
        reason = if reason == "NodeLost" {
            "Unknown".into()
        } else {
            "Terminating".into()
        };
    }
    if reason.is_empty() {
        "Unknown".into()
    } else {
        reason
    }
}

/// Parses `kubectl get pods -o json` (a `List`, or a single `Pod`).
pub fn parse_pods_json(text: &str) -> Result<Vec<Pod>, String> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| e.to_string())?;
    let items: Vec<&Value> = match v.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().collect(),
        None if v.get("metadata").is_some() => vec![&v],
        None => return Err("no items".into()),
    };
    let mut out = Vec::new();
    for p in items {
        let name = s_at(p, &["metadata", "name"]);
        if name.is_empty() {
            continue;
        }
        let statuses = p
            .pointer("/status/containerStatuses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let containers: Vec<String> = p
            .pointer("/spec/containers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let ready = statuses
            .iter()
            .filter(|c| c.get("ready").and_then(Value::as_bool) == Some(true))
            .count() as u32;
        let total = containers.len().max(statuses.len()) as u32;
        let restarts: u64 = statuses
            .iter()
            .map(|c| num_of(c.get("restartCount")).unwrap_or(0))
            .sum();
        out.push(Pod {
            namespace: s_at(p, &["metadata", "namespace"]).to_string(),
            name: name.to_string(),
            status: pod_status(p),
            ready,
            total,
            restarts: restarts.min(u32::MAX as u64) as u32,
            created: DateTime::parse_from_rfc3339(s_at(p, &["metadata", "creationTimestamp"]))
                .ok()
                .map(|t| t.with_timezone(&Utc)),
            node: s_at(p, &["spec", "nodeName"]).to_string(),
            containers,
            owner: p.get("metadata").and_then(owner_of),
        });
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
}

/// How worrying a pod is, from its STATUS and readiness.
pub fn pod_severity(p: &Pod) -> Severity {
    match p.status.as_str() {
        "CrashLoopBackOff"
        | "ImagePullBackOff"
        | "ErrImagePull"
        | "Error"
        | "OOMKilled"
        | "CreateContainerConfigError"
        | "CreateContainerError"
        | "InvalidImageName"
        | "ImageInspectError"
        | "RunContainerError"
        | "Failed"
        | "Evicted"
        | "Unknown"
        | "ErrImageNeverPull"
        | "DeadlineExceeded" => Severity::Bad,
        s if s.starts_with("Init:") && (s.contains("Error") || s.contains("BackOff")) => {
            Severity::Bad
        }
        "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => Severity::Warn,
        s if s.starts_with("Init:") || s.starts_with("Signal:") || s.starts_with("ExitCode:") => {
            Severity::Warn
        }
        "Completed" | "Succeeded" => Severity::Ok,
        "Running" if p.total > 0 && p.ready < p.total => Severity::Warn,
        _ => Severity::Ok,
    }
}

/// Short explanation for a worrying status.
pub fn pod_hint(p: &Pod) -> Option<&'static str> {
    Some(match p.status.as_str() {
        "CrashLoopBackOff" => t!("Der Container startet immer wieder neu – Logs ansehen"),
        "ImagePullBackOff" | "ErrImagePull" => {
            t!("Das Image kann nicht geladen werden (Name, Tag oder Zugangsdaten prüfen)")
        }
        "Pending" => t!("Noch keinem Node zugewiesen oder wartet auf Ressourcen"),
        "OOMKilled" => t!("Zu wenig Arbeitsspeicher – vom Kernel beendet"),
        "Evicted" => t!("Vom Node verdrängt (Ressourcenmangel)"),
        "CreateContainerConfigError" => {
            t!("Fehlerhafte Konfiguration (Secret oder ConfigMap fehlt?)")
        }
        "Error" | "Failed" => t!("Der Pod ist fehlgeschlagen – describe und Logs ansehen"),
        _ if pod_severity(p) == Severity::Warn && p.status == "Running" => {
            t!("Nicht alle Container sind bereit")
        }
        _ => return None,
    })
}

/// `3d`, `5h`, `12m`, `40s` the way kubectl prints ages.
pub fn age_short(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// Sort key: worst first, then namespace and name.
pub fn pod_order(a: &Pod, b: &Pod) -> std::cmp::Ordering {
    pod_severity(b)
        .cmp(&pod_severity(a))
        .then_with(|| a.namespace.cmp(&b.namespace))
        .then_with(|| a.name.cmp(&b.name))
}

/// Namespaces in a pod list, sorted.
pub fn namespaces(pods: &[Pod]) -> Vec<String> {
    pods.iter()
        .map(|p| p.namespace.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Valid lines of a name list (`kubectl config get-contexts -o name`, `command -v`).
pub fn parse_names(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| valid_ref(l))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// Command lines (argv, never a shell string)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctl {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Remove,
    Inspect,
}

impl Ctl {
    pub fn word(self) -> &'static str {
        match self {
            Ctl::Start => "start",
            Ctl::Stop => "stop",
            Ctl::Restart => "restart",
            Ctl::Pause => "pause",
            Ctl::Unpause => "unpause",
            Ctl::Remove => "rm",
            Ctl::Inspect => "inspect",
        }
    }
}

/// `docker|podman` – only these two runtimes are accepted.
pub fn valid_runtime(rt: &str) -> bool {
    matches!(rt, "docker" | "podman")
}

/// `docker stop <name>` and friends; `None` if the runtime or name is not acceptable.
pub fn ctl_argv(rt: &str, op: Ctl, name: &str) -> Option<Vec<String>> {
    if !valid_runtime(rt) || !valid_ref(name) {
        return None;
    }
    Some(vec![rt.into(), op.word().into(), name.into()])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compose {
    Up,
    Stop,
    Restart,
}

/// `docker compose -p <project> up -d` / `stop` / `restart`.
pub fn compose_argv(rt: &str, project: &str, op: Compose) -> Option<Vec<String>> {
    if !valid_runtime(rt) || !valid_ref(project) {
        return None;
    }
    let mut v: Vec<String> = vec![rt.into(), "compose".into(), "-p".into(), project.into()];
    match op {
        Compose::Up => v.extend(["up".into(), "-d".into()]),
        Compose::Stop => v.push("stop".into()),
        Compose::Restart => v.push("restart".into()),
    }
    Some(v)
}

/// `kubectl [--context c]`; `None` for a context that is not acceptable.
pub fn kube_base(context: &str) -> Option<Vec<String>> {
    let mut v = vec!["kubectl".to_string()];
    let c = context.trim();
    if !c.is_empty() {
        if !valid_ref(c) {
            return None;
        }
        v.extend(["--context".into(), c.into()]);
    }
    Some(v)
}

pub fn pods_argv(context: &str, namespace: &str) -> Option<Vec<String>> {
    let mut v = kube_base(context)?;
    v.extend(["get".into(), "pods".into()]);
    let ns = namespace.trim();
    if ns.is_empty() {
        v.push("-A".into());
    } else if valid_ref(ns) {
        v.extend(["-n".into(), ns.into()]);
    } else {
        return None;
    }
    v.extend(["-o".into(), "json".into()]);
    Some(v)
}

fn kube_ns(
    context: &str,
    ns: &str,
    pre: &[&str],
    name: &str,
    post: &[&str],
) -> Option<Vec<String>> {
    if !valid_ref(ns) || !valid_ref(name) {
        return None;
    }
    let mut v = kube_base(context)?;
    v.extend(["-n".into(), ns.into()]);
    v.extend(pre.iter().map(|s| s.to_string()));
    v.push(name.into());
    v.extend(post.iter().map(|s| s.to_string()));
    Some(v)
}

pub fn pod_delete_argv(context: &str, ns: &str, pod: &str) -> Option<Vec<String>> {
    kube_ns(context, ns, &["delete", "pod"], pod, &[])
}

pub fn pod_describe_argv(context: &str, ns: &str, pod: &str) -> Option<Vec<String>> {
    kube_ns(context, ns, &["describe", "pod"], pod, &[])
}

/// `kubectl rollout restart deployment/web` – `target` is `deployment/web` and the like.
pub fn rollout_argv(context: &str, ns: &str, target: &str) -> Option<Vec<String>> {
    let (kind, _) = target.split_once('/')?;
    if !matches!(kind, "deployment" | "statefulset" | "daemonset") {
        return None;
    }
    kube_ns(context, ns, &["rollout", "restart"], target, &[])
}

pub fn probe_argv() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "for c in docker podman kubectl; do command -v $c >/dev/null 2>&1 && echo $c; done; true"
            .into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = r#"{"Command":"\"nginx -g 'daemon off;'\"","CreatedAt":"2026-10-16 10:21:03 +0200 CEST","ID":"a1b2c3d4e5f6","Image":"nginx:1.27","Labels":"com.docker.compose.project=shop,com.docker.compose.service=web,maintainer=x","Names":"shop-web-1","Ports":"0.0.0.0:8080->80/tcp, :::8080->80/tcp","RunningFor":"2 days ago","State":"running","Status":"Up 2 days (healthy)"}
WARNING: something odd on stderr
{"CreatedAt":"2026-10-10 08:00:00 +0000 UTC","ID":"ffeeddccbbaa","Image":"postgres:16","Labels":"","Names":"db","Ports":"","State":"exited","Status":"Exited (0) 3 days ago"}
{"ID":"0123456789ab","Image":"redis","Names":"shop-cache-1","Labels":"a=b,com.docker.compose.project=shop","State":"running","Status":"Up 5 minutes (unhealthy)","Ports":"6379/tcp"}
{broken json
"#;

    #[test]
    fn ps_lines() {
        let p = parse_ps(PS);
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.notes.len(), 2);
        let w = &p.items[0];
        assert_eq!(w.name, "shop-web-1");
        assert_eq!(w.ports, "8080->80/tcp");
        assert_eq!(w.project.as_deref(), Some("shop"));
        assert_eq!(w.service.as_deref(), Some("web"));
        assert_eq!(w.health, Health::Healthy);
        assert_eq!(w.created, "2026-10-16 10:21");
        assert!(w.running());
        assert!(!p.items[1].running());
        assert_eq!(p.items[1].project, None);
        assert_eq!(p.items[2].health, Health::Unhealthy);
        assert_eq!(p.items[2].project.as_deref(), Some("shop"));
    }

    #[test]
    fn ps_podman_array() {
        let out = r#"[{"Id":"abc123def456","Names":["web"],"Image":"docker.io/library/nginx:latest","State":"running","Status":"Up 1 hour","Created":1760000000,"Ports":[{"host_ip":"","container_port":80,"host_port":8080,"range":1,"protocol":"tcp"}],"Labels":{"com.docker.compose.project":"p"}}]"#;
        let p = parse_ps(out);
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].name, "web");
        assert_eq!(p.items[0].ports, "8080->80/tcp");
        assert_eq!(p.items[0].project.as_deref(), Some("p"));
        assert!(p.items[0].created.starts_with("2025-"));
    }

    #[test]
    fn ps_weird() {
        // Not JSON at all, empty, wrong types.
        assert!(parse_ps("").items.is_empty());
        assert!(
            parse_ps("permission denied while trying to connect")
                .items
                .is_empty()
        );
        let p = parse_ps(r#"{"Names":123,"ID":["x"],"State":null,"Status":"Up 1s"}"#);
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].state, "running");
    }

    #[test]
    fn stats() {
        let out = r#"{"BlockIO":"0B / 0B","CPUPerc":"0.07%","Container":"a1b2c3d4e5f6","ID":"a1b2c3d4e5f6","MemPerc":"1.25%","MemUsage":"24.5MiB / 1.9GiB","Name":"shop-web-1","NetIO":"1kB / 2kB","PIDs":"5"}
{"CPUPerc":"--","ID":"0123456789ab","MemPerc":"--","MemUsage":"0B / 0B","Name":"shop-cache-1"}
{"cpu_percent":"3.50%","mem_percent":"10.00%","mem_usage":"100MB / 1GB","name":"podman-one","id":"deadbeef0000"}"#;
        let st = parse_stats(out);
        assert_eq!(st.len(), 3);
        assert_eq!(st[0].cpu, Some(0.07));
        assert_eq!(st[1].cpu, None);
        assert_eq!(st[2].mem_pct, Some(10.0));
        let mut items = parse_ps(PS).items;
        apply_stats(&mut items, &st);
        assert_eq!(items[0].mem_usage, "24.5MiB");
        assert_eq!(items[0].cpu, Some(0.07));
        assert_eq!(items[1].cpu, None);
        assert_eq!(parse_percent("NaN%"), None);
        assert_eq!(parse_percent("-3%"), None);
    }

    #[test]
    fn grouping_and_summary() {
        let items = parse_ps(PS).items;
        let g = group_by_project(&items);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].project.as_deref(), Some("shop"));
        assert_eq!(g[0].items, vec![2, 0]); // shop-cache-1 before shop-web-1
        assert_eq!(g[1].project, None);
        assert_eq!(g[1].items, vec![1]);
        let s = summarize(&items);
        assert_eq!((s.running, s.stopped, s.unhealthy), (2, 1, 1));
        assert!(s.text().starts_with("2 laufen"));
        assert!(group_by_project(&[]).is_empty());
    }

    #[test]
    fn names() {
        for ok in ["web", "my_app-1.2", "ghcr.io/org/img:1.2@sha256", "ns/pod"] {
            assert!(valid_ref(ok), "{ok}");
        }
        for bad in [
            "", "-rf", "a b", "a;b", "$(id)", "a`b`", "a'b", "a\"b", "a\nb", "a|b", "a&b", "a>b",
            "ä", "--all",
        ] {
            assert!(!valid_ref(bad), "{bad:?}");
        }
        assert!(!valid_ref(&"a".repeat(300)));
        assert_eq!(
            ctl_argv("docker", Ctl::Stop, "web").unwrap(),
            ["docker", "stop", "web"]
        );
        assert!(ctl_argv("docker", Ctl::Stop, "web; rm -rf /").is_none());
        assert!(ctl_argv("sudo", Ctl::Stop, "web").is_none());
        assert_eq!(
            compose_argv("docker", "shop", Compose::Up).unwrap(),
            ["docker", "compose", "-p", "shop", "up", "-d"]
        );
        assert!(compose_argv("docker", "-x", Compose::Up).is_none());
        assert_eq!(
            pod_delete_argv("prod", "web", "api-1").unwrap(),
            [
                "kubectl",
                "--context",
                "prod",
                "-n",
                "web",
                "delete",
                "pod",
                "api-1"
            ]
        );
        assert!(pod_delete_argv("", "web", "a;b").is_none());
        assert!(pod_delete_argv("x y", "web", "a").is_none());
        assert_eq!(
            rollout_argv("", "web", "deployment/api").unwrap(),
            [
                "kubectl",
                "-n",
                "web",
                "rollout",
                "restart",
                "deployment/api"
            ]
        );
        assert!(rollout_argv("", "web", "pod/api").is_none());
        assert_eq!(
            pods_argv("", "").unwrap(),
            ["kubectl", "get", "pods", "-A", "-o", "json"]
        );
        assert!(pods_argv("", "a b").is_none());
    }

    const PODS: &str = r#"{"apiVersion":"v1","kind":"List","items":[
 {"metadata":{"name":"api-5d8f7c9b6-abcde","namespace":"prod","creationTimestamp":"2026-10-15T08:00:00Z",
   "ownerReferences":[{"kind":"ReplicaSet","name":"api-5d8f7c9b6"}]},
  "spec":{"nodeName":"node-1","containers":[{"name":"app"},{"name":"side"}]},
  "status":{"phase":"Running","containerStatuses":[
    {"name":"app","ready":true,"restartCount":2,"state":{"running":{}}},
    {"name":"side","ready":false,"restartCount":"7","state":{"waiting":{"reason":"CrashLoopBackOff"}}}]}},
 {"metadata":{"name":"job-1","namespace":"batch","creationTimestamp":"2026-10-17T08:00:00Z"},
  "spec":{"containers":[{"name":"c"}]},
  "status":{"phase":"Succeeded","containerStatuses":[{"name":"c","ready":false,"restartCount":0,"state":{"terminated":{"reason":"Completed","exitCode":0}}}]}},
 {"metadata":{"name":"pend","namespace":"prod"},"spec":{"containers":[{"name":"c"}]},"status":{"phase":"Pending"}},
 {"metadata":{"name":"pull","namespace":"prod"},"spec":{"containers":[{"name":"c"}]},
  "status":{"phase":"Pending","containerStatuses":[{"name":"c","ready":false,"restartCount":0,"state":{"waiting":{"reason":"ImagePullBackOff"}}}]}},
 {"metadata":{"name":"gone","namespace":"prod","deletionTimestamp":"2026-10-17T09:00:00Z"},"spec":{},"status":{"phase":"Running"}},
 {"metadata":{"name":"init","namespace":"prod"},"spec":{"containers":[{"name":"c"}]},
  "status":{"phase":"Pending","initContainerStatuses":[{"state":{"waiting":{"reason":"PodInitializing"}}}]}},
 {"metadata":{"namespace":"prod"}},
 {"metadata":{"name":123}}
]}"#;

    #[test]
    fn pods() {
        let p = parse_pods_json(PODS).unwrap();
        assert_eq!(p.len(), 6);
        let api = &p[0];
        assert_eq!((api.ready, api.total, api.restarts), (1, 2, 9));
        assert_eq!(api.status, "CrashLoopBackOff");
        assert_eq!(api.node, "node-1");
        assert_eq!(api.owner.as_deref(), Some("deployment/api"));
        assert_eq!(pod_severity(api), Severity::Bad);
        assert!(pod_hint(api).is_some());
        assert_eq!(p[1].status, "Completed");
        assert_eq!(pod_severity(&p[1]), Severity::Ok);
        assert_eq!(p[2].status, "Pending");
        assert_eq!(pod_severity(&p[2]), Severity::Warn);
        assert_eq!(p[3].status, "ImagePullBackOff");
        assert_eq!(p[4].status, "Terminating");
        assert_eq!(p[5].status, "Init:0/1");
        let mut sorted = p.clone();
        sorted.sort_by(pod_order);
        assert_eq!(sorted[0].severity_name(), Severity::Bad);
        assert_eq!(namespaces(&p), ["batch", "prod"]);
    }

    impl Pod {
        fn severity_name(&self) -> Severity {
            pod_severity(self)
        }
    }

    #[test]
    fn pods_weird() {
        assert!(parse_pods_json("not json").is_err());
        assert!(parse_pods_json("{}").is_err());
        assert!(parse_pods_json(r#"{"items":[]}"#).unwrap().is_empty());
        let single = r#"{"metadata":{"name":"x","namespace":"n"},"status":{"phase":"Running","containerStatuses":[{"ready":true,"restartCount":null}]},"spec":{"containers":[{"name":"c"}]}}"#;
        let p = parse_pods_json(single).unwrap();
        assert_eq!(p[0].status, "Running");
        assert_eq!((p[0].ready, p[0].total, p[0].restarts), (1, 1, 0));
        assert_eq!(age_short(30), "30s");
        assert_eq!(age_short(7200), "2h");
        assert_eq!(age_short(86400 * 3), "3d");
        assert_eq!(age_short(-5), "0s");
    }

    #[test]
    fn inspect() {
        let j = r#"[{"Name":"/web","Created":"2026-10-16T08:21:03.123Z","Path":"nginx","Args":["-g","daemon off;"],
 "State":{"Status":"running","Health":{"Status":"healthy"}},
 "Config":{"Image":"nginx:1.27","Env":["A=1","SECRET=hunter2","NOVALUE"]},
 "HostConfig":{"RestartPolicy":{"Name":"on-failure","MaximumRetryCount":3}},
 "Mounts":[{"Type":"bind","Source":"/srv","Destination":"/data","RW":false}],
 "NetworkSettings":{"Networks":{"bridge":{"IPAddress":"172.17.0.2"}},"Ports":{"80/tcp":[{"HostIp":"0.0.0.0","HostPort":"8080"}],"443/tcp":null}}}]"#;
        let i = parse_inspect(j).unwrap();
        assert_eq!(i.name, "web");
        assert_eq!(i.restart_policy, "on-failure:3");
        assert_eq!(i.health, "healthy");
        assert_eq!(i.env.len(), 3);
        assert_eq!(i.env[1], ("SECRET".into(), "hunter2".into()));
        assert_eq!(i.mounts, ["/srv → /data (ro)"]);
        assert_eq!(i.networks, ["bridge (172.17.0.2)"]);
        assert_eq!(i.command, "nginx -g daemon off;");
        assert!(parse_inspect("nope").is_none());
    }

    #[test]
    fn label_search() {
        assert_eq!(label_value("a=b,x=1", "x").as_deref(), Some("1"));
        assert_eq!(label_value("ax=b,x=1", "x").as_deref(), Some("1"));
        assert_eq!(label_value("ax=b", "x"), None);
        assert_eq!(
            parse_names("docker\n kubectl \n$(x)\n\n"),
            ["docker", "kubectl"]
        );
    }
}
