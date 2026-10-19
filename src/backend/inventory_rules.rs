//! Server inventory: one row per saved host built from the stored facts, warning rules
//! with thresholds, the small end-of-life table, sorting, filtering and the summary.
//! Pure functions, no I/O.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{Env, Host, HostFacts, Library, Protocol};

/// Thresholds (Settings → Inventar) and the view preferences of the page.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct InventorySettings {
    pub disk_warn: u8,
    pub disk_crit: u8,
    /// Warn when at least this many updates are pending.
    pub updates_warn: u32,
    /// Warn when the host has been up this many days (no reboot for kernel updates).
    pub uptime_days: u32,
    /// Warn when the facts are older than this many days.
    pub facts_stale_days: u32,
    /// Warn about distributions that no longer get updates (table in this file).
    pub eol_check: bool,
    /// Column keys the user switched off.
    pub hidden_cols: Vec<String>,
    pub sort_col: String,
    pub sort_desc: bool,
    pub group_by_group: bool,
    /// Columns that are off unless switched on (older settings files don't know them).
    pub extra_cols: Vec<String>,
    /// Last choices in the export dialog.
    pub export: super::inventory_doc::ExportPrefs,
    /// Commands that run on the hosts when the facts are refreshed.
    pub info_snippets: Vec<super::inventory_info::InfoSnippet>,
}

impl Default for InventorySettings {
    fn default() -> Self {
        Self {
            disk_warn: 85,
            disk_crit: 95,
            updates_warn: 10,
            uptime_days: 365,
            facts_stale_days: 30,
            eol_check: true,
            hidden_cols: ["kernel", "address", "tags", "last", "load"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            sort_col: "name".into(),
            sort_desc: false,
            group_by_group: false,
            extra_cols: Vec::new(),
            export: Default::default(),
            info_snippets: Vec::new(),
        }
    }
}

/// When the table below was last checked against the vendors' published dates.
pub const EOL_TABLE_AS_OF: &str = "2026-10";

/// `(os-release ID, version, end of standard/LTS support)`. Small on purpose: only
/// releases people still meet in the wild. Debian dates are the end of LTS.
pub const EOL_TABLE: &[(&str, &str, &str)] = &[
    ("ubuntu", "14.04", "2019-04-30"),
    ("ubuntu", "16.04", "2021-04-30"),
    ("ubuntu", "18.04", "2023-05-31"),
    ("ubuntu", "20.04", "2025-05-31"),
    ("debian", "8", "2020-06-30"),
    ("debian", "9", "2022-06-30"),
    ("debian", "10", "2024-06-30"),
    ("debian", "11", "2026-08-31"),
    ("centos", "6", "2020-11-30"),
    ("centos", "7", "2024-06-30"),
    ("centos", "8", "2021-12-31"),
    ("rhel", "7", "2024-06-30"),
    ("ol", "7", "2024-12-31"),
    ("opensuse-leap", "15.4", "2023-12-07"),
    ("opensuse-leap", "15.5", "2024-12-31"),
];

/// The version number of a pretty name: `Ubuntu 20.04.6 LTS` → `20.04`,
/// `Debian GNU/Linux 11 (bullseye)` → `11`, `CentOS Linux 7 (Core)` → `7`.
pub fn os_version(os_id: &str, os_name: &str) -> Option<String> {
    let tok = os_name
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let parts: Vec<&str> = tok.split('.').collect();
    let keep = if matches!(os_id, "ubuntu" | "opensuse-leap") {
        2
    } else {
        1
    };
    let v = parts
        .iter()
        .take(keep)
        .cloned()
        .collect::<Vec<_>>()
        .join(".");
    let v: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!v.is_empty()).then_some(v)
}

/// End-of-life date of the host's OS if it is in the table.
pub fn eol_date(os_id: &str, os_name: &str) -> Option<NaiveDate> {
    let v = os_version(os_id, os_name)?;
    let id = os_id.trim().to_lowercase();
    EOL_TABLE
        .iter()
        .find(|(i, ver, _)| *i == id && *ver == v)
        .and_then(|(_, _, d)| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warn,
    Crit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarnKind {
    Disk,
    Updates,
    SecurityUpdates,
    Uptime,
    FactsStale,
    NoFacts,
    Eol,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub kind: WarnKind,
    pub level: Level,
    /// Explanation with the numbers and the threshold, in the UI language.
    pub text: String,
}

/// One table row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: Uuid,
    pub name: String,
    /// `Production / Web`
    pub group: String,
    pub group_chain: Vec<Uuid>,
    pub address: String,
    pub protocol: Protocol,
    pub os: String,
    pub kernel: String,
    pub uptime_secs: Option<u64>,
    pub disk: Option<u8>,
    pub updates: Option<u32>,
    pub sec_updates: Option<u32>,
    pub cpus: Option<u32>,
    pub mem_mb: Option<u64>,
    pub mem_pct: Option<u8>,
    pub load1: Option<f32>,
    pub last_connected: Option<DateTime<Utc>>,
    pub facts_at: Option<DateTime<Utc>>,
    pub env: Env,
    pub tags: Vec<String>,
    pub warnings: Vec<Warning>,
    pub has_facts: bool,
    /// Services in state failed (Windows: started automatically but stopped); `None` if
    /// no service list was collected.
    pub failed_services: Option<u32>,
}

impl Row {
    pub fn level(&self) -> Option<Level> {
        self.warnings.iter().map(|w| w.level).max()
    }
}

/// `Parent / Child` and the chain of group ids (outermost first).
pub fn group_path(lib: &Library, host: &Host) -> (String, Vec<Uuid>) {
    let mut names = Vec::new();
    let mut chain = Vec::new();
    let mut next = host.group_id;
    for _ in 0..32 {
        let Some(g) = next.and_then(|id| lib.group(id)) else {
            break;
        };
        names.push(g.name.clone());
        chain.push(g.id);
        next = g.parent_id;
    }
    names.reverse();
    chain.reverse();
    (names.join(" / "), chain)
}

/// Warnings of one host; `facts` is `None` if nothing was ever collected.
pub fn warnings(
    facts: Option<&HostFacts>,
    s: &InventorySettings,
    now: DateTime<Utc>,
    collectable: bool,
) -> Vec<Warning> {
    let mut out = Vec::new();
    let Some(f) = facts else {
        if collectable {
            out.push(Warning {
                kind: WarnKind::NoFacts,
                level: Level::Warn,
                text: t!("Keine Fakten gesammelt – „Fakten aktualisieren“ klicken").into(),
            });
        }
        return out;
    };
    if let Some(d) = f.disk_used_pct {
        if d >= s.disk_crit {
            out.push(Warning {
                kind: WarnKind::Disk,
                level: Level::Crit,
                text: t!("Festplatte zu {} % voll (kritisch ab {} %)", d, s.disk_crit),
            });
        } else if d >= s.disk_warn {
            out.push(Warning {
                kind: WarnKind::Disk,
                level: Level::Warn,
                text: t!("Festplatte zu {} % voll (Warnung ab {} %)", d, s.disk_warn),
            });
        }
    }
    if let Some(n) = f
        .updates
        .filter(|n| s.updates_warn > 0 && *n >= s.updates_warn)
    {
        out.push(Warning {
            kind: WarnKind::Updates,
            level: Level::Warn,
            text: t!("{} Updates ausstehend (Warnung ab {})", n, s.updates_warn),
        });
    }
    if let Some(n) = f.security_updates.filter(|n| *n > 0) {
        out.push(Warning {
            kind: WarnKind::SecurityUpdates,
            level: Level::Crit,
            text: t!("{} Sicherheits-Updates ausstehend", n),
        });
    }
    if s.uptime_days > 0 && f.uptime_secs / 86_400 > s.uptime_days as u64 {
        out.push(Warning {
            kind: WarnKind::Uptime,
            level: Level::Warn,
            text: t!(
                "Seit {} Tagen ohne Neustart (Warnung ab {} Tagen) – Kernel-Updates sind evtl. nicht aktiv",
                f.uptime_secs / 86_400,
                s.uptime_days
            ),
        });
    }
    match f.collected {
        Some(t) if s.facts_stale_days > 0 && (now - t).num_days() > s.facts_stale_days as i64 => {
            out.push(Warning {
                kind: WarnKind::FactsStale,
                level: Level::Warn,
                text: t!(
                    "Fakten sind {} Tage alt (Warnung ab {} Tagen)",
                    (now - t).num_days(),
                    s.facts_stale_days
                ),
            })
        }
        None if collectable => out.push(Warning {
            kind: WarnKind::NoFacts,
            level: Level::Warn,
            text: t!("Keine Fakten gesammelt – „Fakten aktualisieren“ klicken").into(),
        }),
        _ => {}
    }
    if s.eol_check
        && let Some(d) = eol_date(&f.os_id, &f.os_name)
        && d <= now.date_naive()
    {
        out.push(Warning {
            kind: WarnKind::Eol,
            level: Level::Crit,
            text: t!(
                "{} bekommt seit {} keine Updates mehr (Tabelle Stand {})",
                f.short_os(),
                d.format("%Y-%m-%d"),
                EOL_TABLE_AS_OF
            ),
        });
    }
    out
}

/// Hosts whose facts can be collected over SSH (the rest never has facts).
pub fn collectable(h: &Host) -> bool {
    h.protocol == Protocol::Ssh
}

/// All saved hosts as rows.
pub fn rows(lib: &Library, now: DateTime<Utc>) -> Vec<Row> {
    let s = &lib.settings.inventory;
    lib.hosts
        .iter()
        .filter(|h| !h.protocol.is_container())
        .map(|h| {
            let f = lib.facts.get(&h.id);
            let (group, group_chain) = group_path(lib, h);
            let address = if h.protocol == Protocol::Serial {
                h.serial.port_name.clone()
            } else if h.port == h.protocol.default_port() {
                h.hostname.clone()
            } else {
                format!("{}:{}", h.hostname, h.port)
            };
            Row {
                id: h.id,
                name: h.display_name().to_string(),
                group,
                group_chain,
                address,
                protocol: h.protocol,
                os: f.map(|f| f.short_os()).unwrap_or_default(),
                kernel: f.map(|f| f.kernel.clone()).unwrap_or_default(),
                uptime_secs: f.filter(|f| f.uptime_secs > 0).map(|f| f.uptime_secs),
                disk: f.and_then(|f| f.disk_used_pct),
                updates: f.and_then(|f| f.updates),
                sec_updates: f.and_then(|f| f.security_updates),
                cpus: f.and_then(|f| f.cpus),
                mem_mb: f.and_then(|f| f.mem_total_mb),
                mem_pct: f.and_then(|f| f.mem_used_pct),
                load1: f.and_then(|f| f.load1),
                last_connected: h.last_connected,
                facts_at: f.and_then(|f| f.collected),
                env: lib.env_of(h),
                tags: h.tags.clone(),
                warnings: warnings(f, s, now, collectable(h)),
                has_facts: f.is_some(),
                failed_services: lib
                    .detail
                    .get(&h.id)
                    .filter(|d| d.services_avail == super::inventory_detail::Avail::Ok)
                    .map(|d| d.failed_count()),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Columns

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Name,
    Group,
    Address,
    Os,
    Kernel,
    Uptime,
    Disk,
    Updates,
    SecUpdates,
    Cpu,
    Ram,
    Load,
    Last,
    FactsAge,
    Env,
    Tags,
    Failed,
}

impl Col {
    pub const ALL: [Col; 17] = [
        Col::Name,
        Col::Group,
        Col::Address,
        Col::Os,
        Col::Kernel,
        Col::Uptime,
        Col::Disk,
        Col::Updates,
        Col::SecUpdates,
        Col::Cpu,
        Col::Ram,
        Col::Load,
        Col::Last,
        Col::FactsAge,
        Col::Env,
        Col::Tags,
        Col::Failed,
    ];

    /// Stable key (settings file, JSON export).
    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Group => "group",
            Col::Address => "address",
            Col::Os => "os",
            Col::Kernel => "kernel",
            Col::Uptime => "uptime",
            Col::Disk => "disk",
            Col::Updates => "updates",
            Col::SecUpdates => "security_updates",
            Col::Cpu => "cpus",
            Col::Ram => "ram",
            Col::Load => "load",
            Col::Last => "last",
            Col::FactsAge => "facts_age",
            Col::Env => "environment",
            Col::Tags => "tags",
            Col::Failed => "failed_services",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.iter().copied().find(|c| c.key() == k)
    }

    pub fn label(self) -> &'static str {
        match self {
            Col::Name => t!("Name"),
            Col::Group => t!("Gruppe"),
            Col::Address => t!("Adresse"),
            Col::Os => t!("OS"),
            Col::Kernel => t!("Kernel"),
            Col::Uptime => t!("Uptime"),
            Col::Disk => t!("Festplatte %"),
            Col::Updates => t!("Updates"),
            Col::SecUpdates => t!("Sicherheits-Updates"),
            Col::Cpu => t!("CPUs"),
            Col::Ram => t!("RAM"),
            Col::Load => t!("Last"),
            Col::Last => t!("Letzte Verbindung"),
            Col::FactsAge => t!("Fakten-Alter"),
            Col::Env => t!("Umgebung"),
            Col::Tags => t!("Tags"),
            Col::Failed => t!("Dienste mit Fehler"),
        }
    }

    /// Right-aligned numeric column.
    pub fn numeric(self) -> bool {
        matches!(
            self,
            Col::Uptime
                | Col::Disk
                | Col::Updates
                | Col::SecUpdates
                | Col::Cpu
                | Col::Ram
                | Col::Load
                | Col::FactsAge
                | Col::Failed
        )
    }

    /// Default width in the table.
    pub fn width(self) -> f32 {
        match self {
            Col::Name => 170.0,
            Col::Group => 130.0,
            Col::Address => 150.0,
            Col::Os => 120.0,
            Col::Kernel => 130.0,
            Col::Uptime => 80.0,
            Col::Disk => 90.0,
            Col::Updates => 80.0,
            Col::SecUpdates => 110.0,
            Col::Cpu => 60.0,
            Col::Ram => 120.0,
            Col::Load => 60.0,
            Col::Last => 130.0,
            Col::FactsAge => 100.0,
            Col::Env => 90.0,
            Col::Tags => 140.0,
            Col::Failed => 120.0,
        }
    }
}

/// Columns that are only visible when the user switched them on.
pub fn opt_in(c: Col) -> bool {
    c == Col::Failed
}

/// Whether the column is shown with these settings.
pub fn col_visible(s: &InventorySettings, c: Col) -> bool {
    if c == Col::Name {
        true
    } else if opt_in(c) {
        s.extra_cols.iter().any(|k| k == c.key())
    } else {
        !s.hidden_cols.iter().any(|h| h == c.key())
    }
}

/// Switches a column on or off.
pub fn set_col(s: &mut InventorySettings, c: Col, on: bool) {
    let key = c.key().to_string();
    if opt_in(c) {
        s.extra_cols.retain(|k| *k != key);
        if on {
            s.extra_cols.push(key);
        }
    } else {
        s.hidden_cols.retain(|h| *h != key);
        if !on {
            s.hidden_cols.push(key);
        }
    }
}

/// Compact duration: `3 d`, `5 h`, `12 min`.
pub fn duration_short(secs: u64) -> String {
    match secs {
        s if s >= 86_400 => format!("{} d", s / 86_400),
        s if s >= 3600 => format!("{} h", s / 3600),
        s => format!("{} min", s / 60),
    }
}

fn ram_text(r: &Row) -> String {
    match (r.mem_mb, r.mem_pct) {
        (Some(mb), Some(p)) if mb >= 1024 => format!("{:.1} GB · {p} %", mb as f64 / 1024.0),
        (Some(mb), Some(p)) => format!("{mb} MB · {p} %"),
        (Some(mb), None) if mb >= 1024 => format!("{:.1} GB", mb as f64 / 1024.0),
        (Some(mb), None) => format!("{mb} MB"),
        _ => String::new(),
    }
}

/// Plain text of a cell (table display and export; empty = nothing known).
pub fn cell(r: &Row, c: Col, now: DateTime<Utc>) -> String {
    match c {
        Col::Name => r.name.clone(),
        Col::Group => r.group.clone(),
        Col::Address => r.address.clone(),
        Col::Os => r.os.clone(),
        Col::Kernel => r.kernel.clone(),
        Col::Uptime => r.uptime_secs.map(duration_short).unwrap_or_default(),
        Col::Disk => r.disk.map(|d| format!("{d} %")).unwrap_or_default(),
        Col::Updates => r.updates.map(|n| n.to_string()).unwrap_or_default(),
        Col::SecUpdates => r.sec_updates.map(|n| n.to_string()).unwrap_or_default(),
        Col::Cpu => r.cpus.map(|n| n.to_string()).unwrap_or_default(),
        Col::Ram => ram_text(r),
        Col::Load => r.load1.map(|l| format!("{l:.2}")).unwrap_or_default(),
        Col::Last => r
            .last_connected
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default(),
        Col::FactsAge => r
            .facts_at
            .map(|t| {
                let d = (now - t).num_days();
                if d >= 1 {
                    format!("{d} d")
                } else {
                    duration_short((now - t).num_seconds().max(0) as u64)
                }
            })
            .unwrap_or_default(),
        Col::Env => {
            if r.env == Env::None {
                String::new()
            } else {
                r.env.label().to_string()
            }
        }
        Col::Tags => r.tags.join(", "),
        Col::Failed => r.failed_services.map(|n| n.to_string()).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Sorting, filtering, summary

fn env_rank(e: Env) -> u8 {
    match e {
        Env::Prod => 0,
        Env::Staging => 1,
        Env::Test => 2,
        Env::Dev => 3,
        Env::None => 4,
    }
}

/// Sorts in place; rows without a value for the column always end up last.
pub fn sort_rows(rows: &mut [Row], col: Col, desc: bool, now: DateTime<Utc>) {
    use std::cmp::Ordering;
    fn opt<T: PartialOrd>(a: Option<T>, b: Option<T>, desc: bool) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => {
                let o = a.partial_cmp(&b).unwrap_or(Ordering::Equal);
                if desc { o.reverse() } else { o }
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    fn text(a: &str, b: &str, desc: bool) -> Ordering {
        match (a.is_empty(), b.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => {
                let o = natural_cmp(&a.to_lowercase(), &b.to_lowercase());
                if desc { o.reverse() } else { o }
            }
        }
    }
    rows.sort_by(|a, b| {
        let o = match col {
            Col::Name => text(&a.name, &b.name, desc),
            Col::Group => text(&a.group, &b.group, desc),
            Col::Address => text(&a.address, &b.address, desc),
            Col::Os => text(&a.os, &b.os, desc),
            Col::Kernel => text(&a.kernel, &b.kernel, desc),
            Col::Uptime => opt(a.uptime_secs, b.uptime_secs, desc),
            Col::Disk => opt(a.disk, b.disk, desc),
            Col::Updates => opt(a.updates, b.updates, desc),
            Col::SecUpdates => opt(a.sec_updates, b.sec_updates, desc),
            Col::Cpu => opt(a.cpus, b.cpus, desc),
            Col::Ram => opt(a.mem_mb, b.mem_mb, desc),
            Col::Load => opt(a.load1, b.load1, desc),
            Col::Last => opt(a.last_connected, b.last_connected, desc),
            Col::FactsAge => opt(
                a.facts_at.map(|t| (now - t).num_seconds()),
                b.facts_at.map(|t| (now - t).num_seconds()),
                desc,
            ),
            Col::Env => {
                let o = env_rank(a.env).cmp(&env_rank(b.env));
                if desc { o.reverse() } else { o }
            }
            Col::Tags => text(&a.tags.join(","), &b.tags.join(","), desc),
            Col::Failed => opt(a.failed_services, b.failed_services, desc),
        };
        o.then_with(|| natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase()))
    });
}

/// `web2` < `web10`.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.next_if(|c| c.is_ascii_digit()) {
                    na.push(c);
                }
                let mut nb = String::new();
                while let Some(c) = b.next_if(|c| c.is_ascii_digit()) {
                    nb.push(c);
                }
                let (na, nb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub text: String,
    pub only_problems: bool,
    pub only_prod: bool,
    /// Only hosts in this group or below.
    pub group: Option<Uuid>,
}

/// All words of the text must appear in name, group, address, OS, kernel, tags or
/// environment.
pub fn matches(r: &Row, f: &Filter) -> bool {
    if f.only_problems && r.warnings.is_empty() {
        return false;
    }
    if f.only_prod && r.env != Env::Prod {
        return false;
    }
    if let Some(g) = f.group
        && !r.group_chain.contains(&g)
    {
        return false;
    }
    let hay = format!(
        "{} {} {} {} {} {} {}",
        r.name,
        r.group,
        r.address,
        r.os,
        r.kernel,
        r.tags.join(" "),
        r.env.label()
    )
    .to_lowercase();
    f.text
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub hosts: usize,
    pub with_warnings: usize,
    pub critical: usize,
    /// Pending updates of all hosts together.
    pub updates: u64,
    pub security: u64,
}

pub fn summarize(rows: &[Row]) -> Summary {
    let mut s = Summary {
        hosts: rows.len(),
        ..Default::default()
    };
    for r in rows {
        if !r.warnings.is_empty() {
            s.with_warnings += 1;
        }
        if r.level() == Some(Level::Crit) {
            s.critical += 1;
        }
        s.updates += r.updates.unwrap_or(0) as u64;
        s.security += r.sec_updates.unwrap_or(0) as u64;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-18T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn facts() -> HostFacts {
        HostFacts {
            os_id: "ubuntu".into(),
            os_name: "Ubuntu 24.04.1 LTS".into(),
            uptime_secs: 10 * 86_400,
            disk_used_pct: Some(40),
            updates: Some(2),
            collected: Some(now() - chrono::Duration::days(1)),
            ..Default::default()
        }
    }

    fn kinds(w: &[Warning]) -> Vec<WarnKind> {
        w.iter().map(|w| w.kind).collect()
    }

    #[test]
    fn healthy_host_has_no_warnings() {
        assert!(warnings(Some(&facts()), &InventorySettings::default(), now(), true).is_empty());
    }

    #[test]
    fn disk_thresholds() {
        let s = InventorySettings::default();
        let mut f = facts();
        f.disk_used_pct = Some(84);
        assert!(warnings(Some(&f), &s, now(), true).is_empty());
        f.disk_used_pct = Some(85);
        let w = warnings(Some(&f), &s, now(), true);
        assert_eq!((w[0].kind, w[0].level), (WarnKind::Disk, Level::Warn));
        f.disk_used_pct = Some(94);
        assert_eq!(warnings(Some(&f), &s, now(), true)[0].level, Level::Warn);
        f.disk_used_pct = Some(95);
        assert_eq!(warnings(Some(&f), &s, now(), true)[0].level, Level::Crit);
        let custom = InventorySettings { disk_warn: 70, ..s };
        f.disk_used_pct = Some(72);
        assert_eq!(
            kinds(&warnings(Some(&f), &custom, now(), true)),
            [WarnKind::Disk]
        );
    }

    #[test]
    fn updates_uptime_and_age() {
        let s = InventorySettings::default();
        let mut f = facts();
        f.updates = Some(9);
        assert!(warnings(Some(&f), &s, now(), true).is_empty());
        f.updates = Some(10);
        f.security_updates = Some(3);
        f.uptime_secs = 366 * 86_400;
        let k = kinds(&warnings(Some(&f), &s, now(), true));
        assert_eq!(
            k,
            [
                WarnKind::Updates,
                WarnKind::SecurityUpdates,
                WarnKind::Uptime
            ]
        );
        f.uptime_secs = 365 * 86_400;
        assert!(!kinds(&warnings(Some(&f), &s, now(), true)).contains(&WarnKind::Uptime));
        f.collected = Some(now() - chrono::Duration::days(31));
        assert!(kinds(&warnings(Some(&f), &s, now(), true)).contains(&WarnKind::FactsStale));
        f.collected = Some(now() - chrono::Duration::days(30));
        assert!(!kinds(&warnings(Some(&f), &s, now(), true)).contains(&WarnKind::FactsStale));
    }

    #[test]
    fn no_facts() {
        let s = InventorySettings::default();
        assert_eq!(kinds(&warnings(None, &s, now(), true)), [WarnKind::NoFacts]);
        // RDP & co. never have facts: no warning.
        assert!(warnings(None, &s, now(), false).is_empty());
    }

    #[test]
    fn eol() {
        let d = |id, name| eol_date(id, name);
        assert_eq!(
            d("ubuntu", "Ubuntu 20.04.6 LTS").unwrap().to_string(),
            "2025-05-31"
        );
        assert!(d("ubuntu", "Ubuntu 24.04.1 LTS").is_none());
        assert!(d("ubuntu", "Ubuntu 22.04.4 LTS").is_none());
        assert_eq!(
            d("debian", "Debian GNU/Linux 10 (buster)")
                .unwrap()
                .to_string(),
            "2024-06-30"
        );
        assert!(d("debian", "Debian GNU/Linux 12 (bookworm)").is_none());
        assert!(d("centos", "CentOS Linux 7 (Core)").is_some());
        assert!(d("centos", "CentOS Linux 9").is_none());
        assert!(d("arch", "Arch Linux").is_none());
        assert_eq!(
            os_version("ubuntu", "Ubuntu 18.04.6 LTS").as_deref(),
            Some("18.04")
        );
        assert_eq!(
            os_version("debian", "Debian 11.7 (bullseye)").as_deref(),
            Some("11")
        );
        assert_eq!(os_version("x", "Rolling"), None);
        let mut f = facts();
        f.os_id = "ubuntu".into();
        f.os_name = "Ubuntu 20.04.6 LTS".into();
        let s = InventorySettings::default();
        let w = warnings(Some(&f), &s, now(), true);
        assert_eq!((w[0].kind, w[0].level), (WarnKind::Eol, Level::Crit));
        // Not yet EOL on an earlier date.
        let early = DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        f.collected = Some(early);
        assert!(warnings(Some(&f), &s, early, true).is_empty());
        f.collected = Some(now());
        let off = InventorySettings {
            eol_check: false,
            ..s
        };
        assert!(warnings(Some(&f), &off, now(), true).is_empty());
    }

    fn row(name: &str) -> Row {
        Row {
            id: Uuid::new_v4(),
            name: name.into(),
            group: String::new(),
            group_chain: vec![],
            address: format!("{name}.example"),
            protocol: Protocol::Ssh,
            os: String::new(),
            kernel: String::new(),
            uptime_secs: None,
            disk: None,
            updates: None,
            sec_updates: None,
            cpus: None,
            mem_mb: None,
            mem_pct: None,
            load1: None,
            last_connected: None,
            facts_at: None,
            env: Env::None,
            tags: vec![],
            warnings: vec![],
            has_facts: false,
            failed_services: None,
        }
    }

    #[test]
    fn opt_in_column_and_old_settings() {
        // A settings file from 0.8.0 knows neither the new column nor the export choices.
        let s: InventorySettings =
            serde_json::from_str(r#"{"disk_warn":80,"hidden_cols":["kernel"]}"#).unwrap();
        assert!(col_visible(&s, Col::Os));
        assert!(!col_visible(&s, Col::Kernel));
        assert!(
            !col_visible(&s, Col::Failed),
            "new column is off by default"
        );
        assert_eq!(s.export.format, "md");
        assert!(s.export.services && !s.export.notes);
        let mut s = s;
        set_col(&mut s, Col::Failed, true);
        assert!(col_visible(&s, Col::Failed));
        set_col(&mut s, Col::Failed, false);
        assert!(!col_visible(&s, Col::Failed));
        set_col(&mut s, Col::Os, false);
        assert!(!col_visible(&s, Col::Os));
        assert!(col_visible(&s, Col::Name));
    }

    #[test]
    fn sorting() {
        let mut a = row("web10");
        a.disk = Some(50);
        let mut b = row("web2");
        b.disk = Some(90);
        let c = row("db");
        let mut rows = vec![a.clone(), b.clone(), c.clone()];
        sort_rows(&mut rows, Col::Name, false, now());
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["db", "web2", "web10"]
        );
        sort_rows(&mut rows, Col::Disk, true, now());
        assert_eq!(rows[0].name, "web2");
        assert_eq!(rows[1].name, "web10");
        assert_eq!(rows[2].name, "db"); // unknown values stay last
        sort_rows(&mut rows, Col::Disk, false, now());
        assert_eq!(rows[0].name, "web10");
        assert_eq!(rows[2].name, "db");
        let mut p = row("p");
        p.env = Env::Prod;
        let mut v = vec![c, p];
        sort_rows(&mut v, Col::Env, false, now());
        assert_eq!(v[0].name, "p");
        assert_eq!(natural_cmp("a2", "a10"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("a02", "a2"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn filtering() {
        let mut a = row("web-01");
        a.env = Env::Prod;
        a.os = "Debian 12".into();
        a.tags = vec!["nginx".into()];
        a.warnings = vec![Warning {
            kind: WarnKind::Disk,
            level: Level::Warn,
            text: String::new(),
        }];
        let g = Uuid::new_v4();
        a.group_chain = vec![g];
        let b = row("db-01");
        let f = |text: &str| Filter {
            text: text.into(),
            ..Default::default()
        };
        assert!(matches(&a, &f("")));
        assert!(matches(&a, &f("WEB debian")));
        assert!(matches(&a, &f("nginx")));
        assert!(matches(&a, &f("prod")));
        assert!(!matches(&a, &f("web postgres")));
        assert!(!matches(&b, &f("web")));
        let only = Filter {
            only_problems: true,
            ..Default::default()
        };
        assert!(matches(&a, &only) && !matches(&b, &only));
        let prod = Filter {
            only_prod: true,
            ..Default::default()
        };
        assert!(matches(&a, &prod) && !matches(&b, &prod));
        let grp = Filter {
            group: Some(g),
            ..Default::default()
        };
        assert!(matches(&a, &grp) && !matches(&b, &grp));
    }

    #[test]
    fn summary_counts() {
        let mut a = row("a");
        a.updates = Some(4);
        a.sec_updates = Some(1);
        a.warnings = vec![Warning {
            kind: WarnKind::SecurityUpdates,
            level: Level::Crit,
            text: String::new(),
        }];
        let mut b = row("b");
        b.updates = Some(5);
        let s = summarize(&[a, b]);
        assert_eq!(
            s,
            Summary {
                hosts: 2,
                with_warnings: 1,
                critical: 1,
                updates: 9,
                security: 1
            }
        );
    }

    #[test]
    fn cells() {
        let mut r = row("x");
        r.uptime_secs = Some(3 * 86_400 + 5);
        r.mem_mb = Some(2048);
        r.mem_pct = Some(41);
        r.disk = Some(7);
        assert_eq!(cell(&r, Col::Uptime, now()), "3 d");
        assert_eq!(cell(&r, Col::Ram, now()), "2.0 GB · 41 %");
        assert_eq!(cell(&r, Col::Disk, now()), "7 %");
        assert_eq!(cell(&r, Col::Updates, now()), "");
        assert_eq!(Col::from_key("security_updates"), Some(Col::SecUpdates));
        assert!(Col::ALL.iter().all(|c| Col::from_key(c.key()) == Some(*c)));
    }
}
