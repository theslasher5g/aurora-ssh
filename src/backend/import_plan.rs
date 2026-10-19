//! Migration importer: turns a parsed [`ImportDoc`] into hosts, groups and (only on
//! explicit request) keychain identities. Pure logic on `Library` / `Keychain`.

use std::collections::{BTreeMap, HashMap};

use uuid::Uuid;

use super::import_model::*;
use crate::model::{Group, Host, Identity, Keychain, Library, Protocol};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Conflict {
    /// Leave the existing host alone.
    #[default]
    Skip,
    /// Update the existing host with the imported values.
    Overwrite,
    /// Add the imported one next to it.
    KeepBoth,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Existing group to import into (`None` = top level).
    pub parent: Option<Uuid>,
    /// Name of a new group below `parent` that receives everything (`None` = no extra
    /// group).
    pub new_group: Option<String>,
    pub conflict: Conflict,
    /// Store clear-text passwords from the file in the vault (explicit opt-in).
    pub import_passwords: bool,
    /// Source kind → Aurora protocol (`None` = skip these entries).
    pub map: HashMap<Kind, Option<Protocol>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            parent: None,
            new_group: None,
            conflict: Conflict::Skip,
            import_passwords: false,
            map: default_map(),
        }
    }
}

/// Default protocol mapping: everything Aurora can open; web links and unknown types are
/// left out.
/// `https://host[:port]` for a web entry of another tool (`HTTP` in the source = plain http).
fn web_url(it: &ImportItem, host: &str) -> String {
    let scheme = if it.raw_type.trim().eq_ignore_ascii_case("http") {
        "http"
    } else {
        "https"
    };
    if host.contains("://") {
        host.to_string()
    } else {
        match it.port {
            Some(p) => format!("{scheme}://{host}:{p}"),
            None => format!("{scheme}://{host}"),
        }
    }
}

pub fn default_map() -> HashMap<Kind, Option<Protocol>> {
    HashMap::from([
        (Kind::Ssh, Some(Protocol::Ssh)),
        (Kind::Rdp, Some(Protocol::Rdp)),
        (Kind::Vnc, Some(Protocol::Vnc)),
        (Kind::Telnet, Some(Protocol::Telnet)),
        (Kind::Web, Some(Protocol::Web)),
        (Kind::Other, None),
    ])
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub imported: usize,
    pub overwritten: usize,
    pub groups_created: usize,
    pub passwords_stored: usize,
    /// Reason → number of entries that were not imported.
    pub skipped: BTreeMap<String, usize>,
}

impl Report {
    pub fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }
}

fn key(label: &str, host: &str) -> (String, String) {
    (label.trim().to_lowercase(), host.trim().to_lowercase())
}

/// Existing hosts by (name, host address).
fn host_index(lib: &Library) -> HashMap<(String, String), usize> {
    let mut m = HashMap::with_capacity(lib.hosts.len());
    for (i, h) in lib.hosts.iter().enumerate() {
        m.entry(key(&h.label, &h.hostname)).or_insert(i);
    }
    m
}

/// For each item: is there already a host with the same name and address?
pub fn find_conflicts(lib: &Library, doc: &ImportDoc) -> Vec<bool> {
    let idx = host_index(lib);
    doc.items
        .iter()
        .map(|i| idx.contains_key(&key(&i.name, &i.host)))
        .collect()
}

struct Groups {
    /// (parent, lower-case name) → group id.
    index: HashMap<(Option<Uuid>, String), Uuid>,
    created: usize,
}

impl Groups {
    fn new(lib: &Library) -> Self {
        let mut index = HashMap::new();
        for g in &lib.groups {
            index
                .entry((g.parent_id, g.name.trim().to_lowercase()))
                .or_insert(g.id);
        }
        Self { index, created: 0 }
    }

    fn child(&mut self, lib: &mut Library, parent: Option<Uuid>, name: &str) -> Uuid {
        let k = (parent, name.trim().to_lowercase());
        if let Some(id) = self.index.get(&k) {
            return *id;
        }
        let g = Group {
            name: name.to_string(),
            parent_id: parent,
            ..Default::default()
        };
        let id = g.id;
        lib.groups.push(g);
        self.index.insert(k, id);
        self.created += 1;
        id
    }

    fn path(&mut self, lib: &mut Library, root: Option<Uuid>, path: &[String]) -> Option<Uuid> {
        let mut cur = root;
        for p in path {
            cur = Some(self.child(lib, cur, p));
        }
        cur
    }
}

fn clean_user(u: &str) -> String {
    u.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

fn count(r: &mut Report, reason: String) {
    *r.skipped.entry(reason).or_insert(0) += 1;
}

/// Imports the selected items. `selected` has one flag per item; a missing flag counts as
/// not selected.
pub fn apply(
    lib: &mut Library,
    keychain: &mut Keychain,
    doc: &ImportDoc,
    selected: &[bool],
    opts: &Options,
) -> Report {
    let mut rep = Report::default();
    if doc.skipped_total > 0 {
        rep.skipped.insert(
            t!("Vom Parser übersprungen (siehe Liste)").to_string(),
            doc.skipped_total,
        );
    }
    let mut groups = Groups::new(lib);
    let mut idx = host_index(lib);
    let mut root: Option<Option<Uuid>> = None;
    let mut idents: HashMap<(String, String), Uuid> = HashMap::new();

    for (n, it) in doc.items.iter().enumerate() {
        if !selected.get(n).copied().unwrap_or(false) {
            continue;
        }
        let Some(Some(proto)) = opts.map.get(&it.kind()).copied() else {
            count(
                &mut rep,
                t!("Protokoll {} nicht übernommen", it.kind().label()),
            );
            continue;
        };
        let host_name = it.host.trim();
        if !valid_host(host_name) {
            count(&mut rep, t!("Ungültiger Hostname").to_string());
            continue;
        }
        let k = key(&it.name, host_name);
        let existing = idx.get(&k).copied();
        if existing.is_some() && opts.conflict == Conflict::Skip {
            count(
                &mut rep,
                t!("Existiert schon (gleicher Name und Host)").to_string(),
            );
            continue;
        }
        // Lazily create the target group so an empty import leaves no empty groups.
        let base = *root.get_or_insert_with(|| match &opts.new_group {
            Some(name) if !clean_name(name).is_empty() => {
                Some(groups.child(lib, opts.parent, &clean_name(name)))
            }
            _ => opts.parent,
        });
        let gid = groups.path(lib, base, &it.group).or(base);
        let username = clean_user(&it.user);
        let mut identity = None;
        if opts.import_passwords && !it.password.is_empty() {
            let id = *idents
                .entry((username.clone(), it.password.clone()))
                .or_insert_with(|| {
                    let ident = Identity {
                        id: Uuid::new_v4(),
                        label: if username.is_empty() {
                            t!("Import: {}", it.name)
                        } else {
                            t!("Import: {}", username)
                        },
                        username: username.clone(),
                        password: it.password.clone(),
                        ..Default::default()
                    };
                    let id = ident.id;
                    keychain.identities.push(ident);
                    id
                });
            identity = Some(id);
            rep.passwords_stored += 1;
        }
        let port = it.port.unwrap_or_else(|| proto.default_port());
        match (existing, opts.conflict) {
            (Some(i), Conflict::Overwrite) => {
                let h = &mut lib.hosts[i];
                h.protocol = proto;
                h.hostname = host_name.to_string();
                h.port = port;
                if !username.is_empty() {
                    h.username = username;
                }
                if !it.notes.is_empty() {
                    h.notes = it.notes.clone();
                }
                for t in &it.tags {
                    if !h.tags.contains(t) {
                        h.tags.push(t.clone());
                    }
                }
                if identity.is_some() {
                    h.identity_id = identity;
                }
                h.group_id = gid;
                if proto == Protocol::Web {
                    h.web.url = web_url(it, host_name);
                    let _ = crate::backend::web::sync_host(h);
                }
                rep.overwritten += 1;
            }
            _ => {
                let mut label = it.name.clone();
                if existing.is_some() {
                    // Same name again: keep both, tell them apart.
                    label = format!("{label} (2)");
                }
                lib.hosts.push(Host {
                    label,
                    group_id: gid,
                    protocol: proto,
                    hostname: host_name.to_string(),
                    port,
                    username,
                    identity_id: identity,
                    tags: it.tags.clone(),
                    notes: it.notes.clone(),
                    ..Default::default()
                });
                if proto == Protocol::Web
                    && let Some(h) = lib.hosts.last_mut()
                {
                    h.web.url = web_url(it, host_name);
                    let _ = crate::backend::web::sync_host(h);
                }
                rep.imported += 1;
            }
        }
        // Later entries of the same file compare against the stored state.
        if existing.is_none() {
            idx.entry(key(
                lib.hosts.last().map_or("", |h| h.label.as_str()),
                host_name,
            ))
            .or_insert(lib.hosts.len() - 1);
        }
    }
    rep.groups_created = groups.created;
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, host: &str, group: &[&str], kind: Kind) -> ImportItem {
        ImportItem {
            name: name.into(),
            host: host.into(),
            kind: Some(kind),
            group: group.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn doc(items: Vec<ImportItem>) -> ImportDoc {
        ImportDoc {
            items,
            ..ImportDoc::new(Source::Csv)
        }
    }

    #[test]
    fn builds_nested_group_paths() {
        let mut lib = Library::default();
        let mut kc = Keychain::default();
        let d = doc(vec![
            item("a", "h1", &["Kunde", "Standort"], Kind::Ssh),
            item("b", "h2", &["Kunde", "Standort"], Kind::Rdp),
            item("c", "h3", &["Kunde"], Kind::Ssh),
            item("d", "h4", &[], Kind::Ssh),
        ]);
        let o = Options {
            new_group: Some("Import mRemoteNG 2026-10-19".into()),
            ..Default::default()
        };
        let r = apply(&mut lib, &mut kc, &d, &[true; 4], &o);
        assert_eq!(r.imported, 4);
        assert_eq!(r.groups_created, 3); // root + Kunde + Standort
        assert_eq!(lib.groups.len(), 3);
        let root = lib.groups.iter().find(|g| g.parent_id.is_none()).unwrap();
        let kunde = lib.groups.iter().find(|g| g.name == "Kunde").unwrap();
        let st = lib.groups.iter().find(|g| g.name == "Standort").unwrap();
        assert_eq!(kunde.parent_id, Some(root.id));
        assert_eq!(st.parent_id, Some(kunde.id));
        assert_eq!(lib.hosts[0].group_id, Some(st.id));
        assert_eq!(lib.hosts[3].group_id, Some(root.id));
        assert_eq!(lib.hosts[1].protocol, Protocol::Rdp);
        assert_eq!(lib.hosts[1].port, 3389);
    }

    #[test]
    fn existing_groups_are_reused_and_parent_respected() {
        let mut lib = Library::default();
        let top = Group {
            name: "Kunden".into(),
            ..Default::default()
        };
        let sub = Group {
            name: "kunde a".into(),
            parent_id: Some(top.id),
            ..Default::default()
        };
        let (top_id, sub_id) = (top.id, sub.id);
        lib.groups.push(top);
        lib.groups.push(sub);
        let mut kc = Keychain::default();
        let d = doc(vec![item("a", "h1", &["Kunde A", "Neu"], Kind::Ssh)]);
        let o = Options {
            parent: Some(top_id),
            ..Default::default()
        };
        apply(&mut lib, &mut kc, &d, &[true], &o);
        assert_eq!(lib.groups.len(), 3);
        let neu = lib.groups.iter().find(|g| g.name == "Neu").unwrap();
        assert_eq!(neu.parent_id, Some(sub_id));
    }

    #[test]
    fn conflict_skip_overwrite_keep_both() {
        let mk = || {
            let mut lib = Library::default();
            lib.hosts.push(Host {
                label: "Web01".into(),
                hostname: "HOST.example".into(),
                port: 22,
                notes: "alt".into(),
                ..Default::default()
            });
            lib
        };
        let mut it = item("web01", "host.example", &[], Kind::Ssh);
        it.port = Some(2222);
        it.notes = "neu".into();
        let d = doc(vec![it]);
        assert_eq!(find_conflicts(&mk(), &d), [true]);

        let mut lib = mk();
        let r = apply(
            &mut lib,
            &mut Keychain::default(),
            &d,
            &[true],
            &Options::default(),
        );
        assert_eq!((r.imported, r.skipped_total(), lib.hosts.len()), (0, 1, 1));
        assert_eq!(lib.hosts[0].notes, "alt");

        let mut lib = mk();
        let o = Options {
            conflict: Conflict::Overwrite,
            ..Default::default()
        };
        let id = lib.hosts[0].id;
        let r = apply(&mut lib, &mut Keychain::default(), &d, &[true], &o);
        assert_eq!((r.overwritten, lib.hosts.len()), (1, 1));
        assert_eq!(lib.hosts[0].id, id);
        assert_eq!(
            (lib.hosts[0].port, lib.hosts[0].notes.as_str()),
            (2222, "neu")
        );

        let mut lib = mk();
        let o = Options {
            conflict: Conflict::KeepBoth,
            ..Default::default()
        };
        let r = apply(&mut lib, &mut Keychain::default(), &d, &[true], &o);
        assert_eq!((r.imported, lib.hosts.len()), (1, 2));
        assert_ne!(lib.hosts[0].label, lib.hosts[1].label);
    }

    #[test]
    fn selection_mapping_and_invalid_hosts() {
        let mut lib = Library::default();
        let d = doc(vec![
            item("a", "h1", &[], Kind::Ssh),
            item("b", "h2", &[], Kind::Ssh),
            item("w", "wiki", &[], Kind::Other),
            item("bad", "h; rm -rf /", &[], Kind::Ssh),
        ]);
        let r = apply(
            &mut lib,
            &mut Keychain::default(),
            &d,
            &[true, false, true, true],
            &Options::default(),
        );
        assert_eq!(r.imported, 1);
        assert_eq!(r.skipped_total(), 2);
        assert!(lib.groups.is_empty());
        // mapping an unmapped kind → Telnet takes it
        let mut o = Options::default();
        o.map.insert(Kind::Other, Some(Protocol::Telnet));
        let mut lib = Library::default();
        let r = apply(
            &mut lib,
            &mut Keychain::default(),
            &d,
            &[false, false, true, false],
            &o,
        );
        assert_eq!(r.imported, 1);
        assert_eq!(lib.hosts[0].protocol, Protocol::Telnet);
    }

    #[test]
    fn passwords_only_with_opt_in_and_deduplicated() {
        let mut a = item("a", "h1", &[], Kind::Ssh);
        a.user = "root".into();
        a.password = "pw".into();
        let mut b = item("b", "h2", &[], Kind::Ssh);
        b.user = "root".into();
        b.password = "pw".into();
        let d = doc(vec![a, b]);

        let (mut lib, mut kc) = (Library::default(), Keychain::default());
        apply(&mut lib, &mut kc, &d, &[true; 2], &Options::default());
        assert!(kc.identities.is_empty());
        assert!(lib.hosts.iter().all(|h| h.identity_id.is_none()));

        let (mut lib, mut kc) = (Library::default(), Keychain::default());
        let o = Options {
            import_passwords: true,
            ..Default::default()
        };
        let r = apply(&mut lib, &mut kc, &d, &[true; 2], &o);
        assert_eq!(kc.identities.len(), 1);
        assert_eq!(r.passwords_stored, 2);
        assert_eq!(lib.hosts[0].identity_id, Some(kc.identities[0].id));
        // the library JSON never contains the secret
        assert!(!serde_json::to_string(&lib).unwrap().contains("\"pw\""));
    }

    #[test]
    fn duplicates_inside_one_file_stay_apart() {
        let mut lib = Library::default();
        let d = doc(vec![
            item("a", "h1", &[], Kind::Ssh),
            item("a", "h1", &[], Kind::Ssh),
        ]);
        let r = apply(
            &mut lib,
            &mut Keychain::default(),
            &d,
            &[true; 2],
            &Options::default(),
        );
        // second one conflicts with the first and is skipped (default policy)
        assert_eq!((r.imported, r.skipped_total()), (1, 1));
    }

    #[test]
    fn large_import_is_fast() {
        let items: Vec<ImportItem> = (0..5000)
            .map(|i| {
                item(
                    &format!("srv{i}"),
                    &format!("10.{}.{}.1", i / 250, i % 250),
                    &["G", &format!("S{}", i % 50)],
                    Kind::Ssh,
                )
            })
            .collect();
        let d = doc(items);
        let mut lib = Library::default();
        for i in 0..3000 {
            lib.hosts.push(Host {
                label: format!("x{i}"),
                hostname: format!("x{i}"),
                ..Default::default()
            });
        }
        let t = std::time::Instant::now();
        let c = find_conflicts(&lib, &d);
        let r = apply(
            &mut lib,
            &mut Keychain::default(),
            &d,
            &vec![true; 5000],
            &Options::default(),
        );
        assert_eq!(c.len(), 5000);
        assert_eq!(r.imported, 5000);
        assert_eq!(lib.groups.len(), 51);
        assert!(t.elapsed().as_secs() < 5);
    }

    #[test]
    fn web_entries_become_web_hosts() {
        let mut lib = Library::default();
        let mut a = item("fw", "10.0.0.1", &[], Kind::Web);
        a.raw_type = "HTTPS".into();
        a.port = Some(8443);
        let mut b = item("old", "switch.lan", &[], Kind::Web);
        b.raw_type = "HTTP".into();
        let d = doc(vec![a, b]);
        let r = apply(
            &mut lib,
            &mut Keychain::default(),
            &d,
            &[true, true],
            &Options::default(),
        );
        assert_eq!(r.imported, 2);
        assert_eq!(lib.hosts[0].web.url, "https://10.0.0.1:8443/");
        assert_eq!(lib.hosts[1].web.url, "http://switch.lan/");
    }
}
