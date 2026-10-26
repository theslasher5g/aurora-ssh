//! Dynamic folders: a group whose hosts come from a source instead of being typed in.
//!
//! * Proxmox server: every VM / container of a saved PVE connection is a host entry in the
//!   group, kept in sync (new guests added, vanished ones greyed out or removed, renamed
//!   ones updated).
//! * Inventory rule: a saved search (`type:switch customer/*`) - a smart folder that lists
//!   the matches without copying them.
//! * CSV / URL list: a file or an https address with `name,host,port,protocol,…`.
//!
//! This file is the pure part (data, location checks, CSV mapping, the sync diff); the
//! network and the GUI are in `backend::dyn_source` and `ui::nav2_dyn`.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{Host, Protocol};

/// Most entries one list or one folder may contribute.
pub const MAX_ENTRIES: usize = 20_000;
/// Biggest list that is read (file or download).
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub enum DynSource {
    /// All guests of this saved Proxmox VE connection.
    Proxmox { server: Uuid },
    /// A saved search over the whole inventory.
    Rule { query: String },
    /// File path or `https://` address of a CSV list.
    List { location: String },
}

impl Default for DynSource {
    fn default() -> Self {
        DynSource::Rule {
            query: String::new(),
        }
    }
}

impl DynSource {
    /// Hosts of this folder are generated and stored (Proxmox, list); a rule only shows
    /// existing hosts.
    pub fn stores_hosts(&self) -> bool {
        !matches!(self, DynSource::Rule { .. })
    }
}

/// Result of the last synchronisation, shown in the editor and the group menu.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DynStatus {
    pub last: Option<DateTime<Utc>>,
    pub added: u32,
    pub removed: u32,
    pub updated: u32,
    /// Entries the source delivered (or hosts a rule matched).
    pub total: u32,
    /// Entries the source delivered that could not be used.
    pub skipped: u32,
    /// Empty = the last run worked.
    pub error: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct DynFolder {
    pub source: DynSource,
    pub status: DynStatus,
    /// Entries that vanished from the source are deleted (default: greyed out).
    pub prune: bool,
    /// Sync when Aurora starts (lists and Proxmox).
    pub on_start: bool,
}

impl Default for DynFolder {
    fn default() -> Self {
        Self {
            source: DynSource::default(),
            status: DynStatus::default(),
            prune: false,
            on_start: true,
        }
    }
}

/// Marks a host that a dynamic folder generated.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DynMark {
    /// The group (folder) that owns the entry.
    pub folder: Uuid,
    /// Stable id of the entry in its source (`pve:101`, `host:port`).
    pub key: String,
    /// The source no longer lists it.
    pub gone: bool,
    /// Tags the source set, so a rename in the source does not touch the user's own tags.
    pub gen_tags: Vec<String>,
}

/// One entry of a source, before it becomes a host.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Gen {
    pub key: String,
    pub label: String,
    /// Empty = unknown (Proxmox guest without a known address yet): an existing host keeps
    /// its address; a new one falls back to the label.
    pub hostname: String,
    pub port: u16,
    pub protocol: Protocol,
    pub tags: Vec<String>,
    pub notes: String,
    pub user: String,
    pub web_url: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub added: u32,
    pub removed: u32,
    pub updated: u32,
    pub total: u32,
    pub skipped: u32,
}

// ------------------------------------------------------------------ locations

#[derive(Debug, PartialEq, Eq)]
pub enum Location {
    Https(String),
    File(String),
}

/// Checks the location of a CSV list: `https://` without credentials in the URL, or a
/// plain file path. Plain `http://` and every other scheme are refused.
pub fn parse_location(s: &str) -> Result<Location, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(t!("Keine Quelle angegeben").to_string());
    }
    if s.chars().any(|c| c.is_control()) {
        return Err(t!("Die Quelle enthält Steuerzeichen").to_string());
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("https://") {
        let rest = &s["https://".len()..];
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() {
            return Err(t!("Die Adresse hat keinen Host").to_string());
        }
        if authority.contains('@') {
            return Err(
                t!("Zugangsdaten gehören nicht in die Adresse (user:passwort@host)").to_string(),
            );
        }
        // Tokens in the query string would end up in the saved library: refuse the usual
        // names so a secret is not stored in clear text.
        let q = rest.split_once('?').map(|(_, q)| q.to_ascii_lowercase());
        if let Some(q) = q
            && [
                "token", "password", "passwd", "secret", "apikey", "api_key", "key=",
            ]
            .iter()
            .any(|k| q.contains(k))
        {
            return Err(
                t!("Zugangsdaten gehören nicht in die Adresse (Token, Passwort)").to_string(),
            );
        }
        return Ok(Location::Https(s.to_string()));
    }
    if lower.starts_with("http://") {
        return Err(t!("Nur https:// ist erlaubt (kein unverschlüsseltes http)").to_string());
    }
    if let Some((scheme, _)) = s.split_once("://")
        && scheme.chars().all(|c| c.is_ascii_alphabetic())
        && scheme.len() > 1
    {
        return Err(t!(
            "Unbekanntes Adress-Schema „{}“ – https:// oder ein Dateipfad",
            scheme
        )
        .to_string());
    }
    Ok(Location::File(s.to_string()))
}

// ------------------------------------------------------------------ CSV

/// Parses a CSV list (`name,host,port,protocol,user,group,tags,description`, header
/// optional, `,` `;` or tab) into entries. Returns the entries and how many rows could not
/// be used (no or unsafe host name).
pub fn parse_list(text: &str) -> Result<(Vec<Gen>, u32), String> {
    use crate::backend::import_model::{Kind, valid_host};
    let doc = crate::backend::import_csv::parse(text)?;
    let mut skipped = doc.skipped_total as u32;
    let mut out: Vec<Gen> = Vec::new();
    let mut seen: HashMap<String, u32> = HashMap::new();
    for it in doc.items.iter() {
        if out.len() >= MAX_ENTRIES {
            skipped += 1;
            continue;
        }
        let protocol = match it.kind() {
            Kind::Ssh => Protocol::Ssh,
            Kind::Rdp => Protocol::Rdp,
            Kind::Vnc => Protocol::Vnc,
            Kind::Telnet => Protocol::Telnet,
            Kind::Web => Protocol::Web,
            Kind::Other => {
                skipped += 1;
                continue;
            }
        };
        if !valid_host(&it.host) {
            skipped += 1;
            continue;
        }
        let port = it.port.unwrap_or(protocol.default_port());
        let web_url = if protocol == Protocol::Web
            && (it.url.starts_with("https://") || it.url.starts_with("http://"))
        {
            it.url.clone()
        } else {
            String::new()
        };
        // Same address twice is allowed (two names): the key gets a counter.
        let base = format!("{}:{}:{}", it.host.to_lowercase(), port, protocol.label());
        let n = seen.entry(base.clone()).or_insert(0);
        *n += 1;
        let key = if *n == 1 { base } else { format!("{base}#{n}") };
        out.push(Gen {
            key,
            label: if it.name.trim().is_empty() {
                it.host.clone()
            } else {
                it.name.clone()
            },
            hostname: it.host.clone(),
            port,
            protocol,
            tags: it.tags.clone(),
            notes: it.notes.clone(),
            user: it.user.clone(),
            web_url,
        });
    }
    Ok((out, skipped))
}

// ------------------------------------------------------------------ sync diff

fn push_tag(tags: &mut Vec<String>, t: &str) -> bool {
    if t.trim().is_empty() || tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
        return false;
    }
    tags.push(t.to_string());
    true
}

/// Brings an existing generated host in line with its source entry; true if it changed.
fn update_host(h: &mut Host, g: &Gen, folder: Uuid) -> bool {
    let mut changed = false;
    if h.label != g.label {
        h.label = g.label.clone();
        changed = true;
    }
    if !g.hostname.is_empty() && h.hostname != g.hostname {
        h.hostname = g.hostname.clone();
        changed = true;
    }
    if h.port != g.port {
        h.port = g.port;
        changed = true;
    }
    if h.protocol != g.protocol {
        h.protocol = g.protocol;
        changed = true;
    }
    if !g.web_url.is_empty() && h.web.url != g.web_url {
        h.web.url = g.web_url.clone();
        changed = true;
    }
    if h.group_id != Some(folder) {
        h.group_id = Some(folder);
        changed = true;
    }
    let old = h
        .dynamic
        .as_ref()
        .map(|d| d.gen_tags.clone())
        .unwrap_or_default();
    // Tags the source no longer sets go away; the user's own tags stay.
    let before = h.tags.len();
    h.tags.retain(|t| {
        !old.iter().any(|o| o.eq_ignore_ascii_case(t))
            || g.tags.iter().any(|n| n.eq_ignore_ascii_case(t))
    });
    changed |= h.tags.len() != before;
    for t in &g.tags {
        changed |= push_tag(&mut h.tags, t);
    }
    if let Some(d) = h.dynamic.as_mut() {
        if d.gen_tags != g.tags {
            d.gen_tags = g.tags.clone();
        }
        if d.gone {
            d.gone = false;
            changed = true;
        }
    }
    changed
}

fn new_host(g: &Gen, folder: Uuid) -> Option<Host> {
    let hostname = if g.hostname.is_empty() {
        // Unknown address: the name is the best guess if it is a usable host name.
        if crate::backend::import_model::valid_host(&g.label) {
            g.label.clone()
        } else {
            return None;
        }
    } else {
        g.hostname.clone()
    };
    let mut h = Host {
        label: g.label.clone(),
        hostname,
        port: g.port,
        protocol: g.protocol,
        tags: Vec::new(),
        notes: g.notes.clone(),
        username: g.user.clone(),
        group_id: Some(folder),
        dynamic: Some(DynMark {
            folder,
            key: g.key.clone(),
            gone: false,
            gen_tags: g.tags.clone(),
        }),
        ..Default::default()
    };
    for t in &g.tags {
        push_tag(&mut h.tags, t);
    }
    if !g.web_url.is_empty() {
        h.web.url = g.web_url.clone();
    }
    Some(h)
}

/// Synchronises the generated hosts of `folder` with the entries of the source:
/// new entries become hosts, known ones are updated (name, address, port, protocol, the
/// tags the source sets), entries the source no longer lists are greyed out (`gone`) or,
/// with `prune`, removed. The user's own data (tags, notes, user, identity, credentials) is
/// never touched. Hosts are matched by [`DynMark::key`], so running it twice changes
/// nothing the second time.
pub fn sync(hosts: &mut Vec<Host>, folder: Uuid, gens: &[Gen], prune: bool) -> SyncStats {
    let mut st = SyncStats::default();
    let mut pos: HashMap<String, usize> = HashMap::new();
    let mut drop: Vec<usize> = Vec::new();
    for (i, h) in hosts.iter().enumerate() {
        let Some(d) = h.dynamic.as_ref().filter(|d| d.folder == folder) else {
            continue;
        };
        // A second entry with the same key (copied host, old bug) must not stay.
        if pos.insert(d.key.clone(), i).is_some() {
            drop.push(i);
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut add: Vec<Host> = Vec::new();
    for g in gens {
        if !seen.insert(g.key.as_str()) {
            continue;
        }
        st.total += 1;
        match pos.get(&g.key) {
            Some(&i) => {
                if update_host(&mut hosts[i], g, folder) {
                    st.updated += 1;
                }
            }
            None => match new_host(g, folder) {
                Some(h) => {
                    add.push(h);
                    st.added += 1;
                }
                None => {
                    st.total -= 1;
                    st.skipped += 1;
                }
            },
        }
    }
    for (key, &i) in &pos {
        if seen.contains(key.as_str()) {
            continue;
        }
        if prune {
            drop.push(i);
            st.removed += 1;
        } else if let Some(d) = hosts[i].dynamic.as_mut()
            && !d.gone
        {
            d.gone = true;
            st.removed += 1;
        }
    }
    drop.sort_unstable();
    drop.dedup();
    for i in drop.into_iter().rev() {
        hosts.remove(i);
    }
    hosts.extend(add);
    st
}

// ------------------------------------------------------------------ Proxmox

/// Entries for the guests of a Proxmox snapshot (templates are skipped). `ip` returns the
/// best known address of a guest.
pub fn from_proxmox(
    snap: &crate::backend::proxmox::Snapshot,
    ip: &dyn Fn(u32) -> Option<String>,
) -> Vec<Gen> {
    use crate::backend::proxmox::Kind;
    let mut out = Vec::new();
    for g in snap.guests.iter().filter(|g| !g.template) {
        let kind = match g.kind {
            Kind::Qemu => "VM",
            Kind::Lxc => "CT",
        };
        let name = if g.name.trim().is_empty() {
            format!("{}-{}", kind.to_lowercase(), g.vmid)
        } else {
            g.name.trim().to_string()
        };
        let mut tags = vec![kind.to_lowercase()];
        if !g.node.is_empty() {
            tags.push(g.node.clone());
        }
        for t in &g.tags {
            if !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                tags.push(t.clone());
            }
        }
        out.push(Gen {
            key: format!("pve:{}", g.vmid),
            label: name,
            hostname: ip(g.vmid).unwrap_or_default(),
            port: 22,
            protocol: Protocol::Ssh,
            tags,
            notes: format!("Proxmox {kind} {} · {}", g.vmid, g.node),
            user: String::new(),
            web_url: String::new(),
        });
    }
    out
}

/// A signature of what a snapshot contributes, to notice changes without comparing
/// everything every frame.
pub fn proxmox_signature(gens: &[Gen]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for g in gens {
        g.key.hash(&mut h);
        g.label.hash(&mut h);
        g.hostname.hash(&mut h);
        g.tags.hash(&mut h);
    }
    h.finish()
}

// ------------------------------------------------------------------ rules

/// Query of a rule folder (empty rule = matches nothing, so a half-typed folder is not a
/// copy of the whole library).
pub fn rule_query(source: &DynSource) -> Option<crate::nav_query::Query> {
    match source {
        DynSource::Rule { query } => {
            let q = crate::nav_query::Query::parse(query);
            (!q.is_empty()).then_some(q)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, label: &str, host: &str) -> Gen {
        Gen {
            key: key.into(),
            label: label.into(),
            hostname: host.into(),
            port: 22,
            protocol: Protocol::Ssh,
            tags: vec!["vm".into()],
            ..Default::default()
        }
    }

    #[test]
    fn first_sync_adds_everything_once() {
        let f = Uuid::new_v4();
        let mut hosts = Vec::new();
        let gens = vec![
            entry("pve:100", "web", "10.0.0.1"),
            entry("pve:101", "db", "10.0.0.2"),
        ];
        let st = sync(&mut hosts, f, &gens, false);
        assert_eq!((st.added, st.removed, st.updated, st.total), (2, 0, 0, 2));
        assert_eq!(hosts.len(), 2);
        assert!(
            hosts
                .iter()
                .all(|h| h.group_id == Some(f) && h.dynamic.is_some())
        );
        // Idempotent: a second run changes nothing and creates no duplicates.
        let st = sync(&mut hosts, f, &gens, false);
        assert_eq!((st.added, st.removed, st.updated), (0, 0, 0));
        assert_eq!(hosts.len(), 2);
    }

    #[test]
    fn rename_and_new_address_update_the_host_but_not_the_users_data() {
        let f = Uuid::new_v4();
        let mut hosts = Vec::new();
        sync(&mut hosts, f, &[entry("pve:100", "web", "10.0.0.1")], false);
        let id = hosts[0].id;
        hosts[0].tags.push("kunde-a".into());
        hosts[0].notes = "mein Text".into();
        hosts[0].username = "root".into();
        let mut g = entry("pve:100", "web-neu", "10.0.0.9");
        g.tags = vec!["ct".into()];
        let st = sync(&mut hosts, f, &[g], false);
        assert_eq!(st.updated, 1);
        let h = &hosts[0];
        assert_eq!(h.id, id, "same host, not a new one");
        assert_eq!(h.label, "web-neu");
        assert_eq!(h.hostname, "10.0.0.9");
        assert_eq!(h.notes, "mein Text");
        assert_eq!(h.username, "root");
        assert!(h.tags.contains(&"kunde-a".to_string()), "own tag stays");
        assert!(h.tags.contains(&"ct".to_string()));
        assert!(
            !h.tags.contains(&"vm".to_string()),
            "old generated tag goes"
        );
    }

    #[test]
    fn unknown_address_keeps_the_old_one() {
        let f = Uuid::new_v4();
        let mut hosts = Vec::new();
        sync(&mut hosts, f, &[entry("pve:1", "web", "10.0.0.1")], false);
        sync(&mut hosts, f, &[entry("pve:1", "web", "")], false);
        assert_eq!(hosts[0].hostname, "10.0.0.1");
        // A new entry without an address falls back to a usable name ...
        let mut h2 = Vec::new();
        sync(&mut h2, f, &[entry("pve:2", "db-01", "")], false);
        assert_eq!(h2[0].hostname, "db-01");
        // ... and is skipped if the name is no host name.
        let mut h3 = Vec::new();
        let st = sync(&mut h3, f, &[entry("pve:3", "my vm (test)", "")], false);
        assert!(h3.is_empty());
        assert_eq!((st.added, st.skipped), (0, 1));
    }

    #[test]
    fn vanished_entries_are_greyed_then_restored_or_pruned() {
        let f = Uuid::new_v4();
        let mut hosts = Vec::new();
        let a = entry("pve:1", "a", "10.0.0.1");
        let b = entry("pve:2", "b", "10.0.0.2");
        sync(&mut hosts, f, &[a.clone(), b.clone()], false);
        let st = sync(&mut hosts, f, std::slice::from_ref(&a), false);
        assert_eq!(st.removed, 1);
        assert_eq!(hosts.len(), 2);
        let gone: Vec<bool> = hosts
            .iter()
            .map(|h| h.dynamic.as_ref().unwrap().gone)
            .collect();
        assert_eq!(gone, vec![false, true]);
        // Greyed out once; the next run does not count it again.
        let st = sync(&mut hosts, f, std::slice::from_ref(&a), false);
        assert_eq!(st.removed, 0);
        // Comes back.
        let st = sync(&mut hosts, f, &[a.clone(), b.clone()], false);
        assert_eq!(st.updated, 1);
        assert!(!hosts[1].dynamic.as_ref().unwrap().gone);
        // Pruning deletes.
        let st = sync(&mut hosts, f, &[a], true);
        assert_eq!(st.removed, 1);
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn other_folders_and_normal_hosts_are_untouched() {
        let (f1, f2) = (Uuid::new_v4(), Uuid::new_v4());
        let mut hosts = vec![Host {
            label: "normal".into(),
            hostname: "n".into(),
            ..Default::default()
        }];
        sync(&mut hosts, f1, &[entry("k", "x", "10.0.0.1")], false);
        sync(&mut hosts, f2, &[entry("k", "y", "10.0.0.2")], false);
        assert_eq!(hosts.len(), 3, "the same key in two folders is two entries");
        sync(&mut hosts, f1, &[], true);
        assert_eq!(hosts.len(), 2);
        assert!(hosts.iter().any(|h| h.label == "normal"));
        assert!(hosts.iter().any(|h| h.label == "y"));
    }

    #[test]
    fn duplicate_keys_in_source_and_library_collapse() {
        let f = Uuid::new_v4();
        let mut hosts = Vec::new();
        let g = entry("k", "x", "10.0.0.1");
        let st = sync(&mut hosts, f, &[g.clone(), g.clone()], false);
        assert_eq!(st.total, 1);
        let mut copy = hosts[0].clone();
        copy.id = Uuid::new_v4();
        hosts.push(copy);
        sync(&mut hosts, f, &[g], false);
        assert_eq!(hosts.len(), 1, "a copied generated host is removed again");
    }

    #[test]
    fn csv_list_with_header_and_bad_rows() {
        let text = "name,host,port,protocol,tags,description\n\
                    web-01,10.0.0.1,,ssh,\"web, prod\",Webserver\n\
                    rdp-1,10.0.0.2,3390,rdp,,\n\
                    bad,host name; rm -rf /,22,ssh,,\n\
                    ,,,,,\n\
                    nas,nas.lan,,vnc,,\n";
        let (v, skipped) = parse_list(text).unwrap();
        assert_eq!(v.len(), 3);
        assert!(skipped >= 1);
        assert_eq!(v[0].label, "web-01");
        assert_eq!(v[0].port, 22);
        assert_eq!(v[0].tags, vec!["web", "prod"]);
        assert_eq!(v[0].notes, "Webserver");
        assert_eq!(v[1].port, 3390);
        assert_eq!(v[1].protocol, Protocol::Rdp);
        assert_eq!(v[2].protocol, Protocol::Vnc);
        assert_eq!(v[2].port, 5900);
        assert!(v.iter().all(|g| !g.key.is_empty()));
    }

    #[test]
    fn csv_list_semicolons_and_duplicate_addresses() {
        let text = "host;name\n10.0.0.1;a\n10.0.0.1;b\n";
        let (v, _) = parse_list(text).unwrap();
        assert_eq!(v.len(), 2);
        assert_ne!(v[0].key, v[1].key, "same address twice stays two entries");
        assert!(parse_list("").is_err());
        assert!(parse_list("name,port\nx,22\n").is_err(), "no host column");
    }

    #[test]
    fn locations() {
        assert_eq!(
            parse_location(" https://example.org/list.csv "),
            Ok(Location::Https("https://example.org/list.csv".into()))
        );
        assert_eq!(
            parse_location("/srv/list.csv"),
            Ok(Location::File("/srv/list.csv".into()))
        );
        assert_eq!(
            parse_location(r"C:\data\list.csv"),
            Ok(Location::File(r"C:\data\list.csv".into()))
        );
        assert!(parse_location("http://example.org/a.csv").is_err());
        assert!(parse_location("ftp://example.org/a.csv").is_err());
        assert!(parse_location("file:///etc/passwd").is_err());
        assert!(parse_location("https://user:pw@example.org/a.csv").is_err());
        assert!(parse_location("https://example.org/a.csv?token=abc").is_err());
        assert!(parse_location("https://example.org/a.csv?page=2").is_ok());
        assert!(parse_location("https:///a.csv").is_err());
        assert!(parse_location("").is_err());
        assert!(parse_location("a\nb").is_err());
    }

    #[test]
    fn proxmox_guests_become_entries() {
        use crate::backend::proxmox::{Guest, Kind, Snapshot};
        let g = |vmid, name: &str, kind, template| Guest {
            vmid,
            name: name.into(),
            node: "pve1".into(),
            kind,
            status: "running".into(),
            cpu: 0.0,
            maxcpu: 2,
            mem: 0,
            maxmem: 0,
            disk: 0,
            maxdisk: 0,
            uptime: 0,
            tags: vec!["prod".into()],
            template,
        };
        let snap = Snapshot {
            guests: vec![
                g(100, "web", Kind::Qemu, false),
                g(200, "", Kind::Lxc, false),
                g(9000, "tmpl", Kind::Qemu, true),
            ],
            ..Default::default()
        };
        let v = from_proxmox(&snap, &|id| (id == 100).then(|| "10.0.0.5".to_string()));
        assert_eq!(v.len(), 2, "templates are skipped");
        assert_eq!(v[0].key, "pve:100");
        assert_eq!(v[0].hostname, "10.0.0.5");
        assert_eq!(v[0].tags, vec!["vm", "pve1", "prod"]);
        assert_eq!(v[1].label, "ct-200");
        assert_eq!(v[1].hostname, "");
        assert_ne!(proxmox_signature(&v), proxmox_signature(&v[..1]));
    }

    #[test]
    fn rule_folders_and_defaults() {
        let src = DynSource::Rule {
            query: "type:switch customer/*".into(),
        };
        assert!(rule_query(&src).is_some());
        assert!(rule_query(&DynSource::Rule { query: " ".into() }).is_none());
        assert!(!src.stores_hosts());
        assert!(
            DynSource::List {
                location: "x".into()
            }
            .stores_hosts()
        );
        // Old data and partial data load with defaults.
        let f: DynFolder = serde_json::from_str("{}").unwrap();
        assert!(f.on_start && !f.prune);
        let h: Host = serde_json::from_str(r#"{"hostname":"x"}"#).unwrap();
        assert!(h.dynamic.is_none());
    }
}
