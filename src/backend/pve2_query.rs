//! Search language of the Proxmox guest list (same spirit as the host list):
//!
//! | Token | Meaning |
//! |---|---|
//! | `kunde/server/name` | customer and server (each a substring), then the guest name |
//! | `type:vm` / `type:ct` | QEMU VM or LXC container |
//! | `is:running` `stopped` `paused` `template` | state |
//! | `tag:web` or `#web` | tag |
//! | `vmid:100`, or just `100` | VMID (a bare number matches the start of the VMID) |
//! | `node:pve1` `server:name` `customer:name` `ip:10.1.` | where it lives |
//!
//! A leading `-` negates; `"double quotes"` keep spaces together; other words match
//! fuzzily over name, VMID, node, tags, server, customer and address.

use crate::nav_query::{fuzzy_lc, path_matches, tokenize};

/// Everything the query can look at.
pub struct Facets<'a> {
    pub name: &'a str,
    pub vmid: u32,
    pub qemu: bool,
    pub status: &'a str,
    pub template: bool,
    pub tags: &'a [String],
    pub node: &'a str,
    pub server: &'a str,
    pub customer: &'a str,
    /// First known address (may be empty).
    pub ip: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Is {
    Running,
    Stopped,
    Paused,
    Template,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// `true` = VM, `false` = container.
    Type(bool),
    Is(Is),
    Tag(String),
    Vmid(String),
    VmidExact(u32),
    Node(String),
    Server(String),
    Customer(String),
    Ip(String),
    /// `customer/server/name`, lowercase; the last part is the guest name.
    Path(Vec<String>),
    /// A value that names nothing (`type:xyz`): matches no guest.
    Never,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub conds: Vec<(bool, Cond)>,
    pub words: Vec<String>,
}

fn is_ip_prefix(t: &str) -> bool {
    let t = t.strip_suffix('.').unwrap_or(t);
    let parts: Vec<&str> = t.split('.').collect();
    (2..=4).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
}

fn parse_is(v: &str) -> Option<Is> {
    Some(match v {
        "running" | "run" | "up" | "on" | "laufend" => Is::Running,
        "stopped" | "stop" | "down" | "off" | "gestoppt" => Is::Stopped,
        "paused" | "pause" | "suspended" => Is::Paused,
        "template" | "vorlage" => Is::Template,
        _ => return None,
    })
}

fn parse_type(v: &str) -> Option<bool> {
    match v {
        "vm" | "qemu" | "kvm" => Some(true),
        "ct" | "lxc" | "container" => Some(false),
        _ => None,
    }
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut q = Query::default();
        for tok in tokenize(s) {
            let (neg, t) = match tok.strip_prefix('-') {
                Some(r) if !r.is_empty() => (true, r),
                _ => (false, tok.as_str()),
            };
            let lc = t.to_lowercase();
            let cond = if let Some((key, val)) = lc.split_once(':') {
                match key {
                    "type" | "typ" | "kind" => {
                        Some(parse_type(val).map_or(Cond::Never, Cond::Type))
                    }
                    "is" => Some(parse_is(val).map_or(Cond::Never, Cond::Is)),
                    "tag" => Some(Cond::Tag(val.to_string())),
                    "vmid" | "id" => Some(val.parse::<u32>().map_or(Cond::Never, Cond::VmidExact)),
                    "node" => Some(Cond::Node(val.to_string())),
                    "server" => Some(Cond::Server(val.to_string())),
                    "customer" | "kunde" => Some(Cond::Customer(val.to_string())),
                    "ip" => Some(Cond::Ip(val.to_string())),
                    _ => None,
                }
            } else if let Some(tag) = lc.strip_prefix('#').filter(|t| !t.is_empty()) {
                Some(Cond::Tag(tag.to_string()))
            } else if lc.contains('/') {
                Some(Cond::Path(lc.split('/').map(str::to_string).collect()))
            } else if lc.chars().all(|c| c.is_ascii_digit()) {
                Some(Cond::Vmid(lc.clone()))
            } else if is_ip_prefix(&lc) {
                Some(Cond::Ip(lc.clone()))
            } else {
                None
            };
            match cond {
                Some(c) => q.conds.push((neg, c)),
                // A negated plain word is not supported: keep the dash as text.
                None => q.words.push(tok.to_lowercase()),
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.conds.is_empty() && self.words.is_empty()
    }

    pub fn matches(&self, f: &Facets) -> bool {
        for (neg, c) in &self.conds {
            if self.cond(c, f) == *neg {
                return false;
            }
        }
        if self.words.is_empty() {
            return true;
        }
        let hay = format!(
            "{} {} {} {} {} {} {} {}",
            f.name,
            f.vmid,
            if f.qemu { "vm" } else { "ct" },
            f.node,
            f.tags.join(" "),
            f.server,
            f.customer,
            f.ip
        )
        .to_lowercase();
        self.words.iter().all(|w| fuzzy_lc(w, &hay).is_some())
    }

    fn cond(&self, c: &Cond, f: &Facets) -> bool {
        let has = |hay: &str, needle: &str| hay.to_lowercase().contains(needle);
        match c {
            Cond::Type(vm) => f.qemu == *vm,
            Cond::Is(Is::Running) => f.status == "running",
            Cond::Is(Is::Stopped) => f.status == "stopped",
            Cond::Is(Is::Paused) => matches!(f.status, "paused" | "suspended"),
            Cond::Is(Is::Template) => f.template,
            Cond::Tag(t) => f
                .tags
                .iter()
                .any(|x| x.to_lowercase().starts_with(t.as_str())),
            Cond::Vmid(p) => f.vmid.to_string().starts_with(p.as_str()),
            Cond::VmidExact(n) => f.vmid == *n,
            Cond::Node(n) => has(f.node, n),
            Cond::Server(n) => has(f.server, n),
            Cond::Customer(n) => has(f.customer, n),
            Cond::Ip(p) => f.ip.starts_with(p.as_str()),
            Cond::Path(parts) => {
                let path = [f.customer.to_string(), f.server.to_string()];
                path_matches(parts, &path, f.name)
            }
            Cond::Never => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f<'a>(tags: &'a [String]) -> Facets<'a> {
        Facets {
            name: "web-01",
            vmid: 1042,
            qemu: true,
            status: "running",
            template: false,
            tags,
            node: "pve2",
            server: "Cluster Nord",
            customer: "Müller GmbH",
            ip: "10.1.2.3",
        }
    }

    fn ok(q: &str, f: &Facets) -> bool {
        Query::parse(q).matches(f)
    }

    #[test]
    fn path_type_state() {
        let tags = vec!["prod".to_string(), "Web".to_string()];
        let f = f(&tags);
        assert!(ok("", &f));
        assert!(ok("müller/nord/web", &f));
        assert!(ok("nord/web", &f));
        assert!(ok("müller//web", &f));
        assert!(!ok("müller/süd/web", &f));
        assert!(!ok("andere/nord/web", &f));
        assert!(ok("type:vm is:running", &f));
        assert!(!ok("type:ct", &f));
        assert!(ok("-type:ct", &f));
        assert!(!ok("type:xyz", &f));
        assert!(ok("-is:stopped", &f));
        assert!(!ok("is:template", &f));
    }

    #[test]
    fn tags_ids_words() {
        let tags = vec!["prod".to_string(), "Web".to_string()];
        let f = f(&tags);
        assert!(ok("tag:web", &f) && ok("#prod", &f) && !ok("#db", &f));
        assert!(ok("1042", &f) && ok("104", &f) && !ok("42", &f));
        assert!(ok("vmid:1042", &f) && !ok("vmid:104", &f) && !ok("vmid:abc", &f));
        assert!(ok("10.1.", &f) && ok("ip:10.1.2", &f) && !ok("10.2.", &f));
        assert!(ok("node:pve2", &f) && !ok("node:pve1", &f));
        assert!(ok("customer:müll server:nord", &f));
        assert!(ok("web", &f) && ok("wb01", &f) && !ok("database", &f));
        assert!(ok("\"cluster nord\"", &f));
    }
}
