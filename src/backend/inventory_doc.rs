//! Inventory as a system documentation: Markdown (one chapter per host), a self-contained
//! printable HTML page and a JSON file with all collected data. The renderers work on a
//! small neutral document model (headings, tables, code …) so both text formats show the
//! same content and escape in one place. Never contains secrets: no user names,
//! passwords or keys; notes only when asked for.

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::inventory_detail::{Avail, Firewall, FwRule, HostDetail};
use super::inventory_export::{Format, md_escape};
use super::inventory_rules::{Level, Row, duration_short};
use crate::model::{HostFacts, Library, Protocol};

/// What the export dialog remembers.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct ExportPrefs {
    /// `csv`, `md`, `html` or `json`
    pub format: String,
    pub services: bool,
    pub firewall: bool,
    pub ports: bool,
    /// Host address, interface addresses and addresses in ports and rules.
    pub ips: bool,
    /// Free-text notes of the hosts (may hold sensitive remarks, so off by default).
    pub notes: bool,
    /// Output of the info snippets.
    pub info: bool,
    /// Only the table with the columns of the page (the old Markdown/JSON export).
    pub table_only: bool,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        Self {
            format: "md".into(),
            services: true,
            firewall: true,
            ports: true,
            ips: true,
            notes: false,
            info: true,
            table_only: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocOptions {
    pub services: bool,
    pub firewall: bool,
    pub ports: bool,
    pub ips: bool,
    pub notes: bool,
    pub info: bool,
}

impl From<&ExportPrefs> for DocOptions {
    fn from(p: &ExportPrefs) -> Self {
        Self {
            services: p.services,
            firewall: p.firewall,
            ports: p.ports,
            ips: p.ips,
            notes: p.notes,
            info: p.info,
        }
    }
}

/// One host with everything the documentation needs.
#[derive(Clone, Debug)]
pub struct DocHost {
    pub row: Row,
    pub notes: String,
    pub facts: Option<HostFacts>,
    pub detail: Option<HostDetail>,
}

pub struct DocMeta {
    pub title: String,
    pub created: DateTime<Utc>,
    /// Human description of the current filter.
    pub scope: String,
}

/// The hosts of `rows` with their facts, details and notes.
pub fn hosts_from(lib: &Library, rows: &[Row]) -> Vec<DocHost> {
    rows.iter()
        .map(|r| DocHost {
            row: r.clone(),
            notes: lib
                .host(r.id)
                .map(|h| h.notes.trim().to_string())
                .unwrap_or_default(),
            facts: lib.facts.get(&r.id).cloned(),
            detail: lib.detail.get(&r.id).cloned(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Privacy

fn keep_addr(a: &str) -> bool {
    matches!(
        a,
        "0.0.0.0" | "::" | "*" | "::1" | "127.0.0.1" | "localhost"
    ) || a.starts_with("127.")
}

/// Replaces IPv4 and IPv6 addresses in free text by `‹IP›` (wildcards and loopback stay).
pub fn mask_ips(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut tok = String::new();
    let flush = |tok: &mut String, out: &mut String| {
        if tok.is_empty() {
            return;
        }
        let (addr, rest) = match tok.find('/') {
            Some(i) => (&tok[..i], &tok[i..]),
            None => (tok.as_str(), ""),
        };
        // `10.1.2.3:8080` and `reach 10.1.2.3.`: peel a port and trailing punctuation off first.
        let core = addr.trim_end_matches(['.', ':']);
        let punct = &addr[core.len()..];
        let (addr, port) = match core.rfind(':') {
            Some(i)
                if core[..i].matches('.').count() == 3
                    && !core[i + 1..].is_empty()
                    && core[i + 1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                (&core[..i], &core[i..])
            }
            _ => (core, ""),
        };
        let v4 = {
            let p: Vec<&str> = addr.split('.').collect();
            p.len() == 4
                && p.iter()
                    .all(|x| !x.is_empty() && x.len() <= 3 && x.chars().all(|c| c.is_ascii_digit()))
        };
        let v6 = addr.matches(':').count() >= 2
            && addr
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
            && addr.chars().any(|c| c.is_ascii_hexdigit());
        if (v4 || v6) && !keep_addr(addr) {
            out.push_str("‹IP›");
            out.push_str(port);
            out.push_str(punct);
            if rest.starts_with('/') && rest[1..].chars().all(|c| c.is_ascii_digit()) {
                out.push_str(rest);
            }
        } else {
            out.push_str(tok);
        }
        tok.clear();
    };
    for c in s.chars() {
        if c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/') {
            tok.push(c);
        } else {
            flush(&mut tok, &mut out);
            out.push(c);
        }
    }
    flush(&mut tok, &mut out);
    out
}

fn mask_rule(r: &mut FwRule) {
    for f in [
        &mut r.name,
        &mut r.source,
        &mut r.raw,
        &mut r.program,
        &mut r.port,
    ] {
        *f = mask_ips(f);
    }
}

/// Copies of the hosts without the parts the user left out (and without addresses when
/// `ips` is off). Every renderer works on this result.
pub fn prepare(hosts: &[DocHost], o: &DocOptions) -> Vec<DocHost> {
    hosts
        .iter()
        .map(|h| {
            let mut h = h.clone();
            if !o.notes {
                h.notes.clear();
            }
            if !o.info
                && let Some(d) = h.detail.as_mut()
            {
                d.info.clear();
            }
            if !o.ips {
                h.row.address.clear();
            }
            if let Some(d) = h.detail.as_mut() {
                if !o.ips {
                    d.addresses.clear();
                }
                if !o.services {
                    d.services.clear();
                }
                if !o.ports {
                    d.ports.clear();
                } else if !o.ips {
                    for p in &mut d.ports {
                        p.addr = mask_ips(&p.addr);
                    }
                }
                if !o.firewall {
                    d.firewall.rules.clear();
                    d.firewall.summary.clear();
                } else if !o.ips {
                    d.firewall.rules.iter_mut().for_each(mask_rule);
                    d.firewall.summary = d.firewall.summary.iter().map(|s| mask_ips(s)).collect();
                }
            }
            h
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Neutral document model

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Plain,
    Warn,
    Bad,
}

pub type Cell = (String, Tone);

fn c(s: impl Into<String>) -> Cell {
    (s.into(), Tone::Plain)
}

fn ct(s: impl Into<String>, tone: Tone) -> Cell {
    (s.into(), tone)
}

#[derive(Clone, Debug)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
        anchor: String,
        /// Starts a new page when printed.
        page: bool,
    },
    Para(String),
    Muted(String),
    /// `(label, value)` pairs as a two-column table.
    Props(Vec<(String, Cell)>),
    Table {
        head: Vec<String>,
        rows: Vec<Vec<Cell>>,
    },
    List(Vec<String>),
    Code(String),
    Quote(String),
    Toc(Vec<(String, String)>),
}

pub struct Doc {
    pub title: String,
    pub blocks: Vec<Block>,
}

fn size_text(mb: u64) -> String {
    if mb >= 1024 * 1024 {
        format!("{:.1} TB", mb as f64 / 1024.0 / 1024.0)
    } else if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

fn uptime_text(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{} d {} h", secs / 86_400, secs % 86_400 / 3600)
    } else if secs >= 3600 {
        format!("{} h {} min", secs / 3600, secs % 3600 / 60)
    } else {
        duration_short(secs)
    }
}

fn date_text(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

/// Why a host has no (or only partial) facts. Empty = nothing to note.
pub fn no_facts_reason(protocol: Protocol) -> String {
    match protocol {
        Protocol::Ssh => {
            t!("Noch keine Fakten gesammelt – „Fakten aktualisieren“ klicken").to_string()
        }
        Protocol::Rdp => t!("Nur RDP – keine Fakten (SSH/OpenSSH nötig)").to_string(),
        Protocol::Web => t!("Nur Web – keine Fakten").to_string(),
        other => t!("{} liefert keine Fakten (nur SSH)", other.label()),
    }
}

fn avail_text(a: Avail) -> Option<String> {
    match a {
        Avail::Ok => None,
        Avail::NeedsRoot => {
            Some(t!("benötigt root (sudo ohne Passwort nicht möglich)").to_string())
        }
        Avail::Missing => Some(t!("auf dem Host nicht vorhanden").to_string()),
        Avail::Failed => Some(t!("konnte nicht gelesen werden").to_string()),
        Avail::Unknown => Some(t!("noch nicht erfasst – Fakten aktualisieren").to_string()),
    }
}

fn level_tone(r: &Row) -> Tone {
    match r.level() {
        Some(Level::Crit) => Tone::Bad,
        Some(Level::Warn) => Tone::Warn,
        None => Tone::Plain,
    }
}

fn anchor(i: usize) -> String {
    format!("host-{}", i + 1)
}

fn props(v: Vec<(&str, Cell)>) -> Block {
    Block::Props(
        v.into_iter()
            .filter(|(_, (val, _))| !val.trim().is_empty())
            .map(|(k, val)| (k.to_string(), val))
            .collect(),
    )
}

fn host_chapter(i: usize, h: &DocHost, o: &DocOptions, now: DateTime<Utc>, out: &mut Vec<Block>) {
    let r = &h.row;
    out.push(Block::Heading {
        level: 2,
        text: format!("{}. {}", i + 1, r.name),
        anchor: anchor(i),
        page: true,
    });
    let f = h.facts.as_ref();
    let d = h.detail.as_ref();
    let mut ov = vec![
        (t!("Gruppe"), c(&r.group)),
        (
            t!("Umgebung"),
            c(if r.env == crate::model::Env::None {
                String::new()
            } else {
                r.env.label().to_string()
            }),
        ),
        (t!("Protokoll"), c(r.protocol.label())),
        (t!("Adresse"), c(&r.address)),
        (t!("Betriebssystem"), c(&r.os)),
        (t!("Tags"), c(r.tags.join(", "))),
    ];
    if let Some(t) = r.facts_at {
        let days = (now - t).num_days();
        ov.push((
            t!("Fakten vom"),
            c(format!("{} ({})", date_text(t), t!("vor {} Tagen", days))),
        ));
    }
    if let Some(t) = r.last_connected {
        ov.push((t!("Letzte Verbindung"), c(date_text(t))));
    }
    out.push(props(ov));

    if !r.warnings.is_empty() {
        out.push(Block::Heading {
            level: 3,
            text: t!("Warnungen").to_string(),
            anchor: format!("{}-warn", anchor(i)),
            page: false,
        });
        out.push(Block::List(
            r.warnings.iter().map(|w| w.text.clone()).collect(),
        ));
    }
    if !r.has_facts {
        out.push(Block::Muted(no_facts_reason(r.protocol)));
    }

    if let Some(f) = f {
        out.push(Block::Heading {
            level: 3,
            text: t!("Hardware").to_string(),
            anchor: format!("{}-hw", anchor(i)),
            page: false,
        });
        let ram = match (f.mem_total_mb, f.mem_used_pct) {
            (Some(mb), Some(p)) => format!("{} ({} % {})", size_text(mb), p, t!("belegt")),
            (Some(mb), None) => size_text(mb),
            _ => String::new(),
        };
        out.push(props(vec![
            (
                t!("Modell"),
                c(d.map(|d| d.model.clone()).unwrap_or_default()),
            ),
            (
                t!("Prozessor"),
                c(d.map(|d| d.cpu_model.clone()).unwrap_or_default()),
            ),
            (
                t!("CPU-Kerne"),
                c(f.cpus.map(|n| n.to_string()).unwrap_or_default()),
            ),
            (
                t!("Sockel"),
                c(d.and_then(|d| d.sockets)
                    .map(|n| n.to_string())
                    .unwrap_or_default()),
            ),
            (t!("Arbeitsspeicher"), c(ram)),
            (
                t!("Last (1 min)"),
                c(f.load1.map(|l| format!("{l:.2}")).unwrap_or_default()),
            ),
        ]));

        out.push(Block::Heading {
            level: 3,
            text: t!("Betriebssystem").to_string(),
            anchor: format!("{}-os", anchor(i)),
            page: false,
        });
        let upd = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
        out.push(props(vec![
            (
                t!("System"),
                c(d.map(|d| d.os_caption.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| f.os_name.clone())),
            ),
            (
                t!("Version"),
                c(d.map(|d| d.os_version.clone()).unwrap_or_default()),
            ),
            (t!("Kernel / Build"), c(&f.kernel)),
            (
                t!("Architektur"),
                c(d.map(|d| d.arch.clone()).unwrap_or_default()),
            ),
            (t!("Rechnername"), c(&f.hostname)),
            (
                t!("Domäne / FQDN"),
                c(d.map(|d| d.domain.clone()).unwrap_or_default()),
            ),
            (
                t!("Laufzeit seit Neustart"),
                c(if f.uptime_secs > 0 {
                    uptime_text(f.uptime_secs)
                } else {
                    String::new()
                }),
            ),
            (t!("Ausstehende Updates"), c(upd(f.updates))),
            (t!("davon Sicherheits-Updates"), c(upd(f.security_updates))),
            (
                t!("Angemeldete Benutzer"),
                c(d.map(|d| d.users.join(", ")).unwrap_or_default()),
            ),
        ]));
    }

    if let Some(d) = d {
        if o.ips && !d.addresses.is_empty() {
            out.push(Block::Heading {
                level: 3,
                text: t!("Netzwerk").to_string(),
                anchor: format!("{}-net", anchor(i)),
                page: false,
            });
            out.push(Block::Table {
                head: vec![t!("Schnittstelle").to_string(), t!("Adresse").to_string()],
                rows: d
                    .addresses
                    .iter()
                    .map(|a| vec![c(&a.iface), c(&a.addr)])
                    .collect(),
            });
        }
        if !d.disks.is_empty() {
            out.push(Block::Heading {
                level: 3,
                text: t!("Festplatten").to_string(),
                anchor: format!("{}-disk", anchor(i)),
                page: false,
            });
            out.push(Block::Table {
                head: vec![
                    t!("Laufwerk / Mount").to_string(),
                    t!("Bezeichnung").to_string(),
                    t!("Dateisystem").to_string(),
                    t!("Größe").to_string(),
                    t!("Belegt").to_string(),
                    t!("Belegt %").to_string(),
                ],
                rows: d
                    .disks
                    .iter()
                    .map(|k| {
                        let tone = if k.used_pct >= 95 {
                            Tone::Bad
                        } else if k.used_pct >= 85 {
                            Tone::Warn
                        } else {
                            Tone::Plain
                        };
                        vec![
                            c(&k.mount),
                            c(&k.label),
                            c(&k.fs),
                            c(size_text(k.total_mb)),
                            c(size_text(k.used_mb)),
                            ct(format!("{} %", k.used_pct), tone),
                        ]
                    })
                    .collect(),
            });
        }
        if o.services {
            services_section(i, d, out);
        }
        if o.ports {
            ports_section(i, d, out);
        }
        if o.firewall {
            firewall_section(i, &d.firewall, out);
        }
        if o.info {
            info_section(i, d, out);
        }
    }
    if !h.notes.is_empty() {
        out.push(Block::Heading {
            level: 3,
            text: t!("Notizen").to_string(),
            anchor: format!("{}-notes", anchor(i)),
            page: false,
        });
        out.push(Block::Quote(h.notes.clone()));
    }
}

fn services_section(i: usize, d: &HostDetail, out: &mut Vec<Block>) {
    out.push(Block::Heading {
        level: 3,
        text: t!("Dienste").to_string(),
        anchor: format!("{}-svc", anchor(i)),
        page: false,
    });
    if let Some(why) = avail_text(d.services_avail) {
        out.push(Block::Muted(t!("Nicht erfasst: {}", why)));
        return;
    }
    let total = d.services_total.max(d.services.len());
    let mut line = t!(
        "{} Dienste, davon {} laufend, {} mit Fehler",
        total,
        d.running_count(),
        d.failed_count()
    );
    if total > d.services.len() {
        line.push_str(&t!(" – nur die ersten {} gespeichert", d.services.len()));
    }
    out.push(Block::Para(line));
    if d.services.is_empty() {
        return;
    }
    out.push(Block::Table {
        head: vec![
            t!("Dienst").to_string(),
            t!("Status").to_string(),
            t!("Autostart").to_string(),
            t!("Beschreibung").to_string(),
        ],
        rows: d
            .services
            .iter()
            .map(|s| {
                let state = format!("{} ({})", s.active, s.sub);
                let tone = if s.failed { Tone::Bad } else { Tone::Plain };
                vec![c(&s.name), ct(state, tone), c(&s.start), c(&s.description)]
            })
            .collect(),
    });
}

fn ports_section(i: usize, d: &HostDetail, out: &mut Vec<Block>) {
    out.push(Block::Heading {
        level: 3,
        text: t!("Offene Ports (lauschend)").to_string(),
        anchor: format!("{}-ports", anchor(i)),
        page: false,
    });
    if let Some(why) = avail_text(d.ports_avail) {
        out.push(Block::Muted(t!("Nicht erfasst: {}", why)));
        return;
    }
    if d.ports.is_empty() {
        out.push(Block::Muted(
            t!("Keine lauschenden Ports gefunden").to_string(),
        ));
        return;
    }
    out.push(Block::Table {
        head: vec![
            t!("Protokoll").to_string(),
            t!("Adresse").to_string(),
            t!("Port").to_string(),
            t!("Prozess").to_string(),
        ],
        rows: d
            .ports
            .iter()
            .map(|p| {
                vec![
                    c(p.proto.to_uppercase()),
                    c(&p.addr),
                    c(p.port.to_string()),
                    c(&p.process),
                ]
            })
            .collect(),
    });
}

/// One chapter per info snippet: its output as a code block.
fn info_section(i: usize, d: &HostDetail, out: &mut Vec<Block>) {
    for (n, r) in d.info.iter().enumerate() {
        out.push(Block::Heading {
            level: 3,
            text: format!("{}: {}", t!("Info"), r.name),
            anchor: format!("{}-info{}", anchor(i), n + 1),
            page: false,
        });
        let mut status = Vec::new();
        if let Some(t) = r.at {
            status.push(date_text(t));
        }
        if let Some(code) = r.exit.filter(|c| *c != 0) {
            status.push(t!("Exit-Code {}", code));
        }
        if r.truncated {
            status.push(t!("Ausgabe gekürzt").to_string());
        }
        if !status.is_empty() {
            out.push(Block::Muted(status.join(" · ")));
        }
        if !r.error.is_empty() {
            out.push(Block::Muted(t!("Nicht erfasst: {}", r.error)));
        } else if r.output.trim().is_empty() {
            out.push(Block::Muted(t!("Keine Ausgabe").to_string()));
        } else {
            out.push(Block::Code(r.output.clone()));
        }
    }
}

/// A firewall table column: heading and the field it shows.
type FwCol = (&'static str, fn(&FwRule) -> &str);

fn firewall_section(i: usize, fw: &Firewall, out: &mut Vec<Block>) {
    out.push(Block::Heading {
        level: 3,
        text: t!("Firewall").to_string(),
        anchor: format!("{}-fw", anchor(i)),
        page: false,
    });
    if let Some(why) = avail_text(fw.avail) {
        out.push(Block::Muted(t!("Nicht erfasst: {}", why)));
        return;
    }
    let state = match fw.active {
        Some(true) => t!("aktiv"),
        Some(false) => t!("inaktiv"),
        None => t!("unbekannt"),
    };
    out.push(Block::Para(t!(
        "Werkzeug: {} · Status: {}",
        fw.backend,
        state
    )));
    if !fw.summary.is_empty() {
        out.push(Block::List(fw.summary.clone()));
    }
    if fw.total_rules > fw.rules.len() && !fw.rules.is_empty() {
        out.push(Block::Muted(t!(
            "{} Regeln, die ersten {} gespeichert",
            fw.total_rules,
            fw.rules.len()
        )));
    }
    let (structured, raw): (Vec<&FwRule>, Vec<&FwRule>) = fw.rules.iter().partition(|r| {
        !(r.name.is_empty() && r.direction.is_empty() && r.action.is_empty() && r.port.is_empty())
    });
    if !structured.is_empty() {
        let mut cols: Vec<FwCol> = vec![
            (t!("Name"), |r| &r.name),
            (t!("Richtung"), |r| &r.direction),
            (t!("Aktion"), |r| &r.action),
            (t!("Protokoll"), |r| &r.protocol),
            (t!("Port"), |r| &r.port),
            (t!("Quelle"), |r| &r.source),
            (t!("Programm"), |r| &r.program),
            (t!("Profil"), |r| &r.profile),
        ];
        // Columns nobody filled are left out.
        cols.retain(|(_, f)| structured.iter().any(|r| !f(r).is_empty()));
        out.push(Block::Table {
            head: cols.iter().map(|(h, _)| h.to_string()).collect(),
            rows: structured
                .iter()
                .map(|r| {
                    cols.iter()
                        .map(|(_, f)| {
                            let v = f(r);
                            let tone = if v.eq_ignore_ascii_case("block")
                                || v.eq_ignore_ascii_case("deny")
                                || v.eq_ignore_ascii_case("reject")
                                || v == "DROP"
                            {
                                Tone::Warn
                            } else {
                                Tone::Plain
                            };
                            ct(v, tone)
                        })
                        .collect()
                })
                .collect(),
        });
    }
    if !raw.is_empty() {
        out.push(Block::Code(
            raw.iter()
                .map(|r| r.raw.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    if fw.rules.is_empty() && fw.active != Some(false) {
        out.push(Block::Muted(t!("Keine Regeln erfasst").to_string()));
    }
}

/// Everything a document says about the hosts that are missing or incomplete.
fn gaps(hosts: &[DocHost], o: &DocOptions) -> Vec<Vec<Cell>> {
    let mut rows = Vec::new();
    for h in hosts {
        let name = h.row.name.clone();
        if !h.row.has_facts {
            rows.push(vec![
                c(&name),
                c(t!("Fakten")),
                c(no_facts_reason(h.row.protocol)),
            ]);
            continue;
        }
        let Some(d) = &h.detail else {
            rows.push(vec![
                c(&name),
                c(t!("Details")),
                c(t!("noch nicht erfasst – Fakten aktualisieren")),
            ]);
            continue;
        };
        let mut part = |on: bool, what: &str, a: Avail| {
            if on && let Some(why) = avail_text(a) {
                rows.push(vec![c(name.clone()), c(what), c(why)]);
            }
        };
        part(o.services, t!("Dienste"), d.services_avail);
        part(o.ports, t!("Offene Ports (lauschend)"), d.ports_avail);
        part(o.firewall, t!("Firewall"), d.firewall.avail);
        if o.info {
            for r in d.info.iter().filter(|r| !r.error.is_empty()) {
                rows.push(vec![
                    c(name.clone()),
                    c(format!("{}: {}", t!("Info"), r.name)),
                    c(&r.error),
                ]);
            }
        }
    }
    rows
}

/// The documentation as a neutral document.
pub fn build(hosts: &[DocHost], o: &DocOptions, meta: &DocMeta) -> Doc {
    let now = meta.created;
    let mut b: Vec<Block> = Vec::new();
    b.push(Block::Props(vec![
        (t!("Erstellt").to_string(), c(date_text(meta.created))),
        (t!("Anzahl Hosts").to_string(), c(hosts.len().to_string())),
        (t!("Umfang").to_string(), c(&meta.scope)),
        (
            t!("Enthalten").to_string(),
            c([
                (o.services, t!("Dienste")),
                (o.ports, t!("Ports")),
                (o.firewall, t!("Firewall")),
                (o.ips, t!("IP-Adressen")),
                (o.notes, t!("Notizen")),
            ]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join(", ")),
        ),
    ]));

    // Table of contents
    let mut toc = vec![
        (t!("Zusammenfassung").to_string(), "summary".to_string()),
        (t!("Übersicht").to_string(), "overview".to_string()),
    ];
    for (i, h) in hosts.iter().enumerate() {
        toc.push((format!("{}. {}", i + 1, h.row.name), anchor(i)));
    }
    toc.push((
        t!("Hinweise / nicht erfasst").to_string(),
        "gaps".to_string(),
    ));
    b.push(Block::Heading {
        level: 2,
        text: t!("Inhaltsverzeichnis").to_string(),
        anchor: "toc".into(),
        page: false,
    });
    b.push(Block::Toc(toc));

    // Summary
    let rows: Vec<&Row> = hosts.iter().map(|h| &h.row).collect();
    let with_facts = rows.iter().filter(|r| r.has_facts).count();
    let with_warn: Vec<&&Row> = rows.iter().filter(|r| !r.warnings.is_empty()).collect();
    let crit = rows
        .iter()
        .filter(|r| r.level() == Some(Level::Crit))
        .count();
    let upd_hosts: Vec<&&Row> = rows.iter().filter(|r| r.updates.unwrap_or(0) > 0).collect();
    let upd_total: u64 = rows.iter().map(|r| r.updates.unwrap_or(0) as u64).sum();
    let sec_total: u64 = rows.iter().map(|r| r.sec_updates.unwrap_or(0) as u64).sum();
    let failed_total: u32 = hosts
        .iter()
        .filter_map(|h| h.detail.as_ref())
        .map(|d| d.failed_count())
        .sum();
    b.push(Block::Heading {
        level: 2,
        text: t!("Zusammenfassung").to_string(),
        anchor: "summary".into(),
        page: false,
    });
    b.push(Block::Props(vec![
        (t!("Hosts").to_string(), c(hosts.len().to_string())),
        (t!("mit Fakten").to_string(), c(with_facts.to_string())),
        (
            t!("ohne Fakten").to_string(),
            c((hosts.len() - with_facts).to_string()),
        ),
        (
            t!("mit Warnungen").to_string(),
            ct(
                format!("{} ({} {})", with_warn.len(), crit, t!("kritisch")),
                if crit > 0 {
                    Tone::Bad
                } else if with_warn.is_empty() {
                    Tone::Plain
                } else {
                    Tone::Warn
                },
            ),
        ),
        (
            t!("Updates offen").to_string(),
            c(format!(
                "{upd_total} ({} {})",
                sec_total,
                t!("Sicherheits-Updates")
            )),
        ),
        (
            t!("Dienste mit Fehler").to_string(),
            c(failed_total.to_string()),
        ),
    ]));
    if !with_warn.is_empty() {
        b.push(Block::Para(t!("Hosts mit Warnungen:").to_string()));
        b.push(Block::Table {
            head: vec![t!("Host").to_string(), t!("Warnungen").to_string()],
            rows: with_warn
                .iter()
                .map(|r| {
                    vec![
                        c(&r.name),
                        ct(
                            r.warnings
                                .iter()
                                .map(|w| w.text.as_str())
                                .collect::<Vec<_>>()
                                .join("; "),
                            level_tone(r),
                        ),
                    ]
                })
                .collect(),
        });
    }
    if !upd_hosts.is_empty() {
        b.push(Block::Para(
            t!("Hosts mit ausstehenden Updates:").to_string(),
        ));
        b.push(Block::Table {
            head: vec![
                t!("Host").to_string(),
                t!("Updates").to_string(),
                t!("Sicherheits-Updates").to_string(),
            ],
            rows: upd_hosts
                .iter()
                .map(|r| {
                    vec![
                        c(&r.name),
                        c(r.updates.unwrap_or(0).to_string()),
                        ct(
                            r.sec_updates.map(|n| n.to_string()).unwrap_or_default(),
                            if r.sec_updates.unwrap_or(0) > 0 {
                                Tone::Bad
                            } else {
                                Tone::Plain
                            },
                        ),
                    ]
                })
                .collect(),
        });
    }

    // Overview
    b.push(Block::Heading {
        level: 2,
        text: t!("Übersicht").to_string(),
        anchor: "overview".into(),
        page: false,
    });
    let mut head = vec![t!("Host").to_string(), t!("Gruppe").to_string()];
    if o.ips {
        head.push(t!("Adresse").to_string());
    }
    head.extend([
        t!("OS").to_string(),
        t!("Uptime").to_string(),
        t!("Festplatte %").to_string(),
        t!("Updates").to_string(),
        t!("Warnungen").to_string(),
    ]);
    b.push(Block::Table {
        head,
        rows: hosts
            .iter()
            .map(|h| {
                let r = &h.row;
                let mut v = vec![c(&r.name), c(&r.group)];
                if o.ips {
                    v.push(c(&r.address));
                }
                v.extend([
                    c(if r.os.is_empty() && !r.has_facts {
                        "–".to_string()
                    } else {
                        r.os.clone()
                    }),
                    c(r.uptime_secs.map(uptime_text).unwrap_or_default()),
                    c(r.disk.map(|d| format!("{d} %")).unwrap_or_default()),
                    c(r.updates.map(|n| n.to_string()).unwrap_or_default()),
                    ct(r.warnings.len().to_string(), level_tone(r)),
                ]);
                v
            })
            .collect(),
    });

    for (i, h) in hosts.iter().enumerate() {
        host_chapter(i, h, o, now, &mut b);
    }

    // Gaps
    b.push(Block::Heading {
        level: 2,
        text: t!("Hinweise / nicht erfasst").to_string(),
        anchor: "gaps".into(),
        page: true,
    });
    let g = gaps(hosts, o);
    if g.is_empty() {
        b.push(Block::Para(
            t!("Von allen Hosts wurden die gewählten Daten erfasst.").to_string(),
        ));
    } else {
        b.push(Block::Para(
            t!("Für diese Hosts fehlen Angaben. Sie stehen hier, damit die Dokumentation nicht vollständiger wirkt, als sie ist.").to_string(),
        ));
        b.push(Block::Table {
            head: vec![
                t!("Host").to_string(),
                t!("Was").to_string(),
                t!("Grund").to_string(),
            ],
            rows: g,
        });
    }
    Doc {
        title: meta.title.clone(),
        blocks: b,
    }
}

// ---------------------------------------------------------------------------
// Markdown

/// Inline Markdown text: markup and HTML made harmless, one line.
fn md_inline(s: &str) -> String {
    md_escape(s)
}

/// GitHub-style heading anchor.
pub fn slug(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// A code fence longer than any backtick run in the text.
fn fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

pub fn render_markdown(doc: &Doc) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", md_inline(&doc.title)));
    for b in &doc.blocks {
        match b {
            Block::Heading { level, text, .. } => {
                out.push_str(&format!(
                    "{} {}\n\n",
                    "#".repeat(*level as usize),
                    md_inline(text)
                ));
            }
            Block::Para(s) => out.push_str(&format!("{}\n\n", md_inline(s))),
            Block::Muted(s) => out.push_str(&format!("*{}*\n\n", md_inline(s))),
            Block::Props(v) => {
                if v.is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    "| {} | {} |\n| --- | --- |\n",
                    t!("Eigenschaft"),
                    t!("Wert")
                ));
                for (k, (val, _)) in v {
                    out.push_str(&format!("| {} | {} |\n", md_inline(k), md_inline(val)));
                }
                out.push('\n');
            }
            Block::Table { head, rows } => {
                out.push_str(&format!(
                    "| {} |\n|{}\n",
                    head.iter()
                        .map(|h| md_inline(h))
                        .collect::<Vec<_>>()
                        .join(" | "),
                    " --- |".repeat(head.len())
                ));
                for r in rows {
                    out.push_str(&format!(
                        "| {} |\n",
                        r.iter()
                            .map(|(v, _)| md_inline(v))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
                out.push('\n');
            }
            Block::List(items) => {
                for i in items {
                    out.push_str(&format!("- {}\n", md_inline(i)));
                }
                out.push('\n');
            }
            Block::Code(s) => {
                let f = fence(s);
                out.push_str(&format!("{f}\n{}\n{f}\n\n", s.replace('\r', "")));
            }
            Block::Quote(s) => {
                for l in s.lines() {
                    out.push_str(&format!("> {}\n", md_inline(l)));
                }
                out.push('\n');
            }
            Block::Toc(items) => {
                for (n, (text, _)) in items.iter().enumerate() {
                    out.push_str(&format!(
                        "{}. [{}](#{})\n",
                        n + 1,
                        md_inline(text),
                        slug(text)
                    ));
                }
                out.push('\n');
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// HTML

/// Text and attribute values: nothing of it can become markup.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

const CSS: &str = "\
:root{color-scheme:light}\
*{box-sizing:border-box}\
body{font-family:'Segoe UI',system-ui,-apple-system,Helvetica,Arial,sans-serif;color:#1c2330;background:#fff;margin:0;line-height:1.45;font-size:14px}\
main{max-width:1000px;margin:0 auto;padding:32px 28px 60px}\
h1{font-size:28px;margin:0 0 6px}\
h2{font-size:21px;margin:34px 0 10px;padding-bottom:6px;border-bottom:2px solid #d6dbe5}\
h3{font-size:15px;margin:20px 0 8px;color:#2b3a55;text-transform:uppercase;letter-spacing:.04em}\
table{border-collapse:collapse;width:100%;margin:8px 0 14px;font-size:13px}\
th,td{border:1px solid #d6dbe5;padding:5px 8px;text-align:left;vertical-align:top;overflow-wrap:anywhere}\
thead th{background:#eef1f7}\
table.props th{width:30%;background:#f6f8fb;font-weight:600}\
tr.warn td,td.warn{background:#fff6e0;color:#7a4b00}\
tr.bad td,td.bad{background:#fde8e8;color:#9b1c1c;font-weight:600}\
pre{background:#f6f8fb;border:1px solid #d6dbe5;padding:10px;overflow-x:auto;white-space:pre-wrap;font-size:12px}\
blockquote{margin:8px 0;padding:6px 14px;border-left:4px solid #b9c3d6;background:#f6f8fb;white-space:pre-wrap}\
.muted{color:#5b6678;font-style:italic}\
nav ol{columns:2;margin:6px 0 0;padding-left:22px}\
nav a{color:#1d4ed8;text-decoration:none}\
footer{margin-top:40px;color:#5b6678;font-size:12px}\
@media print{body{font-size:11px}main{max-width:none;padding:0}h2.page{break-before:page}h2.page:first-of-type{break-before:auto}table{break-inside:auto}tr{break-inside:avoid}thead{display:table-header-group}a{color:inherit}nav ol{columns:1}}";

pub fn render_html(doc: &Doc, lang: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<!doctype html>\n<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">\n<title>{}</title>\n<style>{CSS}</style>\n</head>\n<body>\n<main>\n<h1>{}</h1>\n",
        html_escape(lang),
        html_escape(&doc.title),
        html_escape(&doc.title)
    ));
    let td = |(v, tone): &Cell| match tone {
        Tone::Plain => format!("<td>{}</td>", html_escape(v)),
        Tone::Warn => format!("<td class=\"warn\">{}</td>", html_escape(v)),
        Tone::Bad => format!("<td class=\"bad\">{}</td>", html_escape(v)),
    };
    for b in &doc.blocks {
        match b {
            Block::Heading {
                level,
                text,
                anchor,
                page,
            } => {
                let class = if *page { " class=\"page\"" } else { "" };
                out.push_str(&format!(
                    "<h{level}{class} id=\"{}\">{}</h{level}>\n",
                    html_escape(anchor),
                    html_escape(text)
                ));
            }
            Block::Para(s) => out.push_str(&format!("<p>{}</p>\n", html_escape(s))),
            Block::Muted(s) => {
                out.push_str(&format!("<p class=\"muted\">{}</p>\n", html_escape(s)))
            }
            Block::Props(v) => {
                if v.is_empty() {
                    continue;
                }
                out.push_str("<table class=\"props\"><tbody>\n");
                for (k, cell) in v {
                    out.push_str(&format!(
                        "<tr><th>{}</th>{}</tr>\n",
                        html_escape(k),
                        td(cell)
                    ));
                }
                out.push_str("</tbody></table>\n");
            }
            Block::Table { head, rows } => {
                out.push_str("<table><thead><tr>");
                for h in head {
                    out.push_str(&format!("<th>{}</th>", html_escape(h)));
                }
                out.push_str("</tr></thead><tbody>\n");
                for r in rows {
                    out.push_str("<tr>");
                    for cell in r {
                        out.push_str(&td(cell));
                    }
                    out.push_str("</tr>\n");
                }
                out.push_str("</tbody></table>\n");
            }
            Block::List(items) => {
                out.push_str("<ul>\n");
                for i in items {
                    out.push_str(&format!("<li>{}</li>\n", html_escape(i)));
                }
                out.push_str("</ul>\n");
            }
            Block::Code(s) => out.push_str(&format!("<pre>{}</pre>\n", html_escape(s))),
            Block::Quote(s) => {
                out.push_str(&format!("<blockquote>{}</blockquote>\n", html_escape(s)))
            }
            Block::Toc(items) => {
                out.push_str("<nav><ol>\n");
                for (text, a) in items {
                    out.push_str(&format!(
                        "<li><a href=\"#{}\">{}</a></li>\n",
                        html_escape(a),
                        html_escape(text)
                    ));
                }
                out.push_str("</ol></nav>\n");
            }
        }
    }
    out.push_str(&format!(
        "<footer>{}</footer>\n</main>\n</body>\n</html>\n",
        html_escape(t!("Erstellt mit Aurora"))
    ));
    out
}

/// The documentation in the given format (CSV is not a documentation format).
pub fn render(f: Format, hosts: &[DocHost], o: &DocOptions, meta: &DocMeta) -> String {
    let prepared = prepare(hosts, o);
    match f {
        Format::Json => render_json(&prepared, o, meta),
        Format::Html => render_html(
            &build(&prepared, o, meta),
            if crate::i18n::is_english() {
                "en"
            } else {
                "de"
            },
        ),
        _ => render_markdown(&build(&prepared, o, meta)),
    }
}

// ---------------------------------------------------------------------------
// JSON

pub fn render_json(hosts: &[DocHost], o: &DocOptions, meta: &DocMeta) -> String {
    let items: Vec<Value> = hosts
        .iter()
        .map(|h| {
            let r = &h.row;
            let mut v = json!({
                "name": r.name,
                "group": r.group,
                "protocol": r.protocol.label(),
                "environment": if r.env == crate::model::Env::None { Value::Null } else { json!(r.env.label()) },
                "tags": r.tags,
                "os": r.os,
                "kernel": r.kernel,
                "uptime_seconds": r.uptime_secs,
                "disk_used_percent": r.disk,
                "updates": r.updates,
                "security_updates": r.sec_updates,
                "cpus": r.cpus,
                "ram": {"total_mb": r.mem_mb, "used_percent": r.mem_pct},
                "load": r.load1,
                "last_connected": r.last_connected.map(|t| t.to_rfc3339()),
                "facts_collected": r.facts_at.map(|t| t.to_rfc3339()),
                "has_facts": r.has_facts,
                "warnings": r.warnings.iter().map(|w| w.text.clone()).collect::<Vec<_>>(),
            });
            let m = v.as_object_mut().expect("object");
            if o.ips {
                m.insert("address".into(), json!(r.address));
            }
            if o.notes {
                m.insert("notes".into(), json!(h.notes));
            }
            if !r.has_facts {
                m.insert("not_collected".into(), json!(no_facts_reason(r.protocol)));
            }
            if let Some(f) = &h.facts {
                m.insert(
                    "facts".into(),
                    json!({
                        "os_id": f.os_id, "os_name": f.os_name, "hostname": f.hostname,
                        "kernel": f.kernel, "uptime_seconds": f.uptime_secs,
                        "disk_used_percent": f.disk_used_pct, "updates": f.updates,
                        "security_updates": f.security_updates, "cpus": f.cpus,
                        "mem_total_mb": f.mem_total_mb, "mem_used_percent": f.mem_used_pct,
                        "load1": f.load1, "proxmox": f.proxmox,
                        "collected": f.collected.map(|t| t.to_rfc3339()),
                    }),
                );
            }
            if let Some(d) = &h.detail {
                let mut dv = serde_json::to_value(d).unwrap_or(Value::Null);
                if let Some(dm) = dv.as_object_mut() {
                    if !o.services {
                        dm.remove("services");
                        dm.remove("services_avail");
                        dm.remove("services_total");
                    }
                    if !o.ports {
                        dm.remove("ports");
                        dm.remove("ports_avail");
                    }
                    if !o.firewall {
                        dm.remove("firewall");
                    }
                    if !o.info {
                        dm.remove("info");
                    }
                    if !o.ips {
                        dm.remove("addresses");
                    }
                }
                m.insert("detail".into(), dv);
            }
            v
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "title": meta.title,
        "exported": meta.created.to_rfc3339(),
        "scope": meta.scope,
        "host_count": hosts.len(),
        "included": {
            "services": o.services, "ports": o.ports, "firewall": o.firewall,
            "ip_addresses": o.ips, "notes": o.notes, "info": o.info,
        },
        "hosts": items,
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::inventory_detail::{DiskInfo, NetAddr, PortInfo, ServiceInfo};
    use crate::backend::inventory_rules::{WarnKind, Warning};
    use crate::model::Env;
    use uuid::Uuid;

    fn row(name: &str, protocol: Protocol, facts: bool) -> Row {
        Row {
            id: Uuid::new_v4(),
            name: name.into(),
            group: "Prod / Web".into(),
            group_chain: vec![],
            address: "10.0.0.5:2222".into(),
            protocol,
            os: if facts {
                "Debian 12".into()
            } else {
                String::new()
            },
            kernel: "6.1.0".into(),
            uptime_secs: facts.then_some(86_400 * 3 + 7200),
            disk: facts.then_some(91),
            updates: facts.then_some(5),
            sec_updates: facts.then_some(2),
            cpus: facts.then_some(4),
            mem_mb: facts.then_some(8192),
            mem_pct: facts.then_some(40),
            load1: None,
            last_connected: None,
            facts_at: facts.then(Utc::now),
            env: Env::Prod,
            tags: vec!["web".into(), "a|b".into()],
            warnings: if facts {
                vec![Warning {
                    kind: WarnKind::Disk,
                    level: Level::Warn,
                    text: "Festplatte zu 91 % voll".into(),
                }]
            } else {
                vec![]
            },
            has_facts: facts,
            failed_services: facts.then_some(1),
        }
    }

    fn detail() -> HostDetail {
        HostDetail {
            platform: "linux".into(),
            os_caption: "Debian GNU/Linux 12 (bookworm)".into(),
            os_version: "12".into(),
            arch: "x86_64".into(),
            cpu_model: "Xeon".into(),
            disks: vec![DiskInfo {
                mount: "/".into(),
                label: "/dev/sda1".into(),
                fs: "ext4".into(),
                total_mb: 40_000,
                used_mb: 36_400,
                used_pct: 91,
            }],
            addresses: vec![NetAddr {
                iface: "eth0".into(),
                addr: "10.0.0.5/24".into(),
            }],
            users: vec!["anna".into()],
            services: vec![
                ServiceInfo {
                    name: "cloud-init".into(),
                    description: "Initial <job> & \"more\"".into(),
                    active: "failed".into(),
                    sub: "failed".into(),
                    start: "enabled".into(),
                    failed: true,
                    ..Default::default()
                },
                ServiceInfo {
                    name: "ssh".into(),
                    active: "active".into(),
                    sub: "running".into(),
                    start: "enabled".into(),
                    running: true,
                    ..Default::default()
                },
            ],
            services_avail: Avail::Ok,
            services_total: 2,
            ports: vec![
                PortInfo {
                    proto: "tcp".into(),
                    addr: "10.0.0.5".into(),
                    port: 22,
                    process: "sshd".into(),
                },
                PortInfo {
                    proto: "tcp".into(),
                    addr: "0.0.0.0".into(),
                    port: 80,
                    process: "nginx".into(),
                },
            ],
            ports_avail: Avail::Ok,
            firewall: Firewall {
                avail: Avail::Ok,
                backend: "ufw".into(),
                active: Some(true),
                summary: vec!["Default: deny (incoming)".into()],
                rules: vec![
                    FwRule {
                        port: "22".into(),
                        protocol: "tcp".into(),
                        action: "ALLOW".into(),
                        direction: "in".into(),
                        source: "203.0.113.7".into(),
                        ..Default::default()
                    },
                    FwRule {
                        raw: "chain input { policy drop; }".into(),
                        ..Default::default()
                    },
                ],
                total_rules: 2,
            },
            ..Default::default()
        }
    }

    fn host(name: &str) -> DocHost {
        DocHost {
            row: row(name, Protocol::Ssh, true),
            notes: "Backup nachts\n```\nrm -rf <b>x</b>".into(),
            facts: Some(HostFacts {
                os_id: "debian".into(),
                os_name: "Debian GNU/Linux 12 (bookworm)".into(),
                hostname: "web-01".into(),
                uptime_secs: 86_400 * 3,
                cpus: Some(4),
                mem_total_mb: Some(8192),
                mem_used_pct: Some(40),
                kernel: "6.1.0".into(),
                updates: Some(5),
                security_updates: Some(2),
                ..Default::default()
            }),
            detail: Some(detail()),
        }
    }

    fn rdp() -> DocHost {
        DocHost {
            row: row("dc-rdp", Protocol::Rdp, false),
            notes: String::new(),
            facts: None,
            detail: None,
        }
    }

    fn meta() -> DocMeta {
        DocMeta {
            title: "Serverdokumentation".into(),
            created: Utc::now(),
            scope: "Alle Hosts".into(),
        }
    }

    fn all() -> DocOptions {
        DocOptions {
            services: true,
            firewall: true,
            ports: true,
            ips: true,
            notes: true,
            info: true,
        }
    }

    fn doc(hosts: &[DocHost], o: &DocOptions) -> Doc {
        build(&prepare(hosts, o), o, &meta())
    }

    #[test]
    fn markdown_chapters() {
        let o = all();
        let md = render_markdown(&doc(&[host("web-01"), rdp()], &o));
        assert!(md.starts_with("# Serverdokumentation\n"));
        for h in [
            "## Inhaltsverzeichnis",
            "## Zusammenfassung",
            "## Übersicht",
            "## 1. web-01",
            "### Hardware",
            "### Betriebssystem",
            "### Netzwerk",
            "### Festplatten",
            "### Dienste",
            "### Offene Ports (lauschend)",
            "### Firewall",
            "### Notizen",
            "## Hinweise / nicht erfasst",
        ] {
            assert!(md.contains(h), "missing {h}\n{md}");
        }
        assert!(md.contains("2 Dienste, davon 1 laufend, 1 mit Fehler"));
        assert!(md.contains("Nur RDP – keine Fakten"));
        // The RDP host has a chapter, but only its known data and a note.
        assert!(md.contains("dc-rdp"));
        assert!(md.contains("chain input { policy drop; }"));
    }

    #[test]
    fn markdown_escapes_special_characters() {
        let mut h = host("<script>alert(1)</script> | *bold* [x](http://e) `c` #");
        h.detail.as_mut().unwrap().services[0].description = "a|b <img src=x onerror=1>".into();
        let md = render_markdown(&doc(&[h], &all()));
        assert!(!md.contains("<script>"), "{md}");
        assert!(!md.contains("<img"));
        assert!(md.contains("&lt;script&gt;"));
        assert!(md.contains("a\\|b &lt;img"));
        assert!(md.contains("\\*bold\\*"));
        assert!(md.contains("\\[x\\]\\(http://e\\)") || md.contains("\\[x\\](http://e)"));
        // Tag with a pipe does not break the table.
        assert!(md.contains("web, a\\|b"));
        // Notes with a code fence cannot break out of the quote.
        assert!(md.contains("> \\`\\`\\`"));
        assert!(md.contains("&lt;b&gt;x&lt;/b&gt;"));
        // Raw firewall lines live in a fence longer than any backtick run inside.
        assert_eq!(fence("a ```` b"), "`````");
        assert_eq!(fence("plain"), "```");
    }

    #[test]
    fn html_is_self_contained_and_escaped() {
        let mut h = host("<b onmouseover=x>web</b> & \"co\"");
        h.detail.as_mut().unwrap().firewall.rules[1].raw = "</pre><script>alert(1)</script>".into();
        h.notes = "<script>steal()</script>".into();
        let html = render_html(&doc(&[h, rdp()], &all()), "de");
        assert!(html.starts_with("<!doctype html>"));
        assert!(!html.contains("<script"), "no script element at all");
        assert!(!html.contains("</pre><script"));
        assert!(html.contains("&lt;b onmouseover=x&gt;web&lt;/b&gt; &amp; &quot;co&quot;"));
        assert!(html.contains("&lt;/pre&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
        // Table of contents links to the chapters.
        assert!(html.contains("<a href=\"#host-1\">1. "));
        assert!(html.contains("id=\"host-2\""));
        // No external resources, printable.
        assert!(!html.contains("http://") || !html.contains("src="));
        assert!(!html.contains("<link"));
        assert!(html.contains("@media print"));
        assert!(html.contains("<td class=\"bad\">failed (failed)</td>"));
        assert!(html.contains("Nur RDP – keine Fakten"));
        assert_eq!(html_escape("a'b"), "a&#39;b");
    }

    #[test]
    fn options_leave_things_out() {
        let o = DocOptions {
            services: false,
            firewall: false,
            ports: false,
            ips: false,
            notes: false,
            info: false,
        };
        let md = render_markdown(&doc(&[host("web-01")], &o));
        for gone in [
            "### Dienste",
            "### Firewall",
            "### Offene Ports",
            "### Netzwerk",
            "### Notizen",
            "10.0.0.5",
            "203.0.113.7",
            "cloud-init",
            "nginx",
            "rm -rf",
        ] {
            assert!(!md.contains(gone), "{gone} should be left out:\n{md}");
        }
        // Still there: hardware, disks, the host chapter.
        assert!(md.contains("### Festplatten"));
        assert!(md.contains("### Hardware"));
        let j = render_json(&prepare(&[host("web-01")], &o), &o, &meta());
        assert!(!j.contains("10.0.0.5") && !j.contains("cloud-init") && !j.contains("rm -rf"));
        let v: Value = serde_json::from_str(&j).unwrap();
        assert!(v["hosts"][0].get("address").is_none());
        assert!(v["hosts"][0]["detail"].get("services").is_none());
    }

    #[test]
    fn ip_masking() {
        assert_eq!(mask_ips("10.0.0.5"), "‹IP›");
        assert_eq!(mask_ips("from 10.0.0.0/24 to any"), "from ‹IP›/24 to any");
        assert_eq!(
            mask_ips("0.0.0.0 127.0.0.1 :: ::1 *"),
            "0.0.0.0 127.0.0.1 :: ::1 *"
        );
        assert_eq!(mask_ips("2001:db8::5/64"), "‹IP›/64");
        assert_eq!(mask_ips("fe80::1%eth0"), "‹IP›%eth0");
        assert_eq!(mask_ips("port 8080 and v1.2.3"), "port 8080 and v1.2.3");
        assert_eq!(mask_ips("deadbeef cafe"), "deadbeef cafe");
        assert_eq!(
            mask_ips("-A INPUT -s 203.0.113.7/32 -j DROP"),
            "-A INPUT -s ‹IP›/32 -j DROP"
        );
    }

    #[test]
    fn gaps_and_summary() {
        let o = all();
        let mut partial = host("db-01");
        partial.detail.as_mut().unwrap().firewall.avail = Avail::NeedsRoot;
        partial.detail.as_mut().unwrap().firewall.rules.clear();
        let md = render_markdown(&doc(
            &[
                partial,
                rdp(),
                DocHost {
                    row: row("new-ssh", Protocol::Ssh, false),
                    notes: String::new(),
                    facts: None,
                    detail: None,
                },
            ],
            &o,
        ));
        assert!(md.contains("benötigt root"));
        let gaps_at = md.find("## Hinweise / nicht erfasst").unwrap();
        let gaps = &md[gaps_at..];
        assert!(gaps.contains("dc-rdp"));
        assert!(gaps.contains("new-ssh"));
        assert!(gaps.contains("Nur RDP"));
        assert!(gaps.contains("Firewall"));
        let sum_at = md.find("## Zusammenfassung").unwrap();
        let summary = &md[sum_at..gaps_at];
        assert!(summary.contains("| Hosts | 3 |"), "{summary}");
        assert!(summary.contains("| ohne Fakten | 2 |"));
        assert!(summary.contains("Hosts mit Warnungen"));
        assert!(summary.contains("Hosts mit ausstehenden Updates"));
        // Everything collected: a plain statement instead of a table.
        let md = render_markdown(&doc(&[host("ok")], &o));
        assert!(md.contains("Von allen Hosts wurden die gewählten Daten erfasst."));
    }

    #[test]
    fn info_snippets_chapters() {
        use crate::backend::inventory_info::InfoResult;
        let mut h = host("web-01");
        h.detail.as_mut().unwrap().info = vec![
            InfoResult {
                name: "Docker <b>x</b>".into(),
                output: "NAMES  IMAGE\n```\n<script>alert(1)</script> | a".into(),
                exit: Some(0),
                at: Some(Utc::now()),
                ..Default::default()
            },
            InfoResult {
                name: "Cron".into(),
                error: "Zeitüberschreitung".into(),
                ..Default::default()
            },
            InfoResult {
                name: "Fail".into(),
                output: "nope".into(),
                exit: Some(127),
                ..Default::default()
            },
        ];
        let o = all();
        let md = render_markdown(&doc(&[h.clone()], &o));
        assert!(md.contains("### Info: Docker &lt;b&gt;x&lt;/b&gt;"));
        assert!(md.contains("### Info: Cron"));
        // The code block is fenced with more backticks than the output holds.
        assert!(md.contains("````\nNAMES  IMAGE\n```\n<script>alert(1)</script> | a\n````"));
        assert!(md.contains("Exit-Code 127"));
        // The failed snippet is listed under the notes.
        let gaps = &md[md.find("## Hinweise / nicht erfasst").unwrap()..];
        assert!(gaps.contains("Info: Cron"));
        let html = render_html(&doc(&[h.clone()], &o), "de");
        assert!(!html.contains("<script"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; | a"));
        let j = render_json(&prepare(&[h.clone()], &o), &o, &meta());
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["hosts"][0]["detail"]["info"][2]["exit"], 127);
        // Switched off: nothing of it anywhere.
        let off = DocOptions { info: false, ..o };
        let md = render_markdown(&doc(&[h.clone()], &off));
        assert!(!md.contains("### Info") && !md.contains("NAMES"));
        let j = render_json(&prepare(&[h], &off), &off, &meta());
        assert!(!j.contains("NAMES"));
    }

    #[test]
    fn json_has_everything() {
        let o = all();
        let j = render_json(&prepare(&[host("web-01"), rdp()], &o), &o, &meta());
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["host_count"], 2);
        assert_eq!(v["scope"], "Alle Hosts");
        let h = &v["hosts"][0];
        assert_eq!(h["address"], "10.0.0.5:2222");
        assert_eq!(h["detail"]["services"][0]["name"], "cloud-init");
        assert_eq!(h["detail"]["ports"][1]["port"], 80);
        assert_eq!(h["detail"]["firewall"]["backend"], "ufw");
        assert_eq!(h["detail"]["disks"][0]["used_pct"], 91);
        assert_eq!(h["facts"]["security_updates"], 2);
        assert_eq!(
            h["notes"].as_str().unwrap().lines().next(),
            Some("Backup nachts")
        );
        assert!(v["hosts"][1].get("detail").is_none());
        assert!(
            v["hosts"][1]["not_collected"]
                .as_str()
                .unwrap()
                .contains("RDP")
        );
        // No credentials anywhere.
        assert!(!j.contains("password") && !j.contains("username"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("1. web-01"), "1-web-01");
        assert_eq!(slug("Hinweise / nicht erfasst"), "hinweise--nicht-erfasst");
    }

    #[test]
    fn masks_addresses_with_port_and_trailing_dot() {
        assert_eq!(mask_ips("listen 10.1.2.3:8080 ok"), "listen ‹IP›:8080 ok");
        assert_eq!(mask_ips("reach 10.1.2.3."), "reach ‹IP›.");
        assert_eq!(mask_ips("net 10.0.0.0/24"), "net ‹IP›/24");
        assert_eq!(mask_ips("local 127.0.0.1:22"), "local 127.0.0.1:22");
    }
}
