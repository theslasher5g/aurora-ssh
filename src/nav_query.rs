//! Search language of the host list, the palette and saved views.
//!
//! Plain words match fuzzily (name, address, user, tags, group). On top of that:
//!
//! | Token | Meaning |
//! |---|---|
//! | `kunde/standort/name` | group path (each part is a substring) and host name |
//! | `type:switch` | protocol (`ssh`, `rdp` …), a tag or a group name starting with it |
//! | `tag:core` or `#core` | tag |
//! | `env:prod` | environment (own or inherited) |
//! | `is:down` `up` `unknown` `fav` `prod` `warn` `maint` `live` `failed` | state |
//! | `ip:10.1.` or just `10.1.` | address prefix |
//! | `group:name` | any group of the path contains it |
//! | `os:ubuntu` | operating system from the collected facts |
//!
//! A leading `-` negates (`-is:down`); `"double quotes"` keep spaces together.

use crate::model::{Env, Protocol};

/// Reachability of a host as shown in the list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Unknown,
    Up,
    Down,
    /// The last connection attempt failed.
    Failed,
    /// In a maintenance window (down is expected).
    Maint,
}

impl Status {
    pub fn is_down(self) -> bool {
        matches!(self, Status::Down | Status::Failed)
    }
}

/// Everything about a host the query can look at.
pub struct Facets<'a> {
    pub name: &'a str,
    pub address: &'a str,
    pub tags: &'a [String],
    pub protocol: Protocol,
    pub env: Env,
    /// Group names, outermost first.
    pub path: &'a [String],
    pub status: Status,
    pub live: bool,
    pub favorite: bool,
    /// 0 none, 1 warning, 2 critical (from the collected facts).
    pub warn: u8,
    pub os: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Is {
    Down,
    Up,
    Unknown,
    Failed,
    Fav,
    Prod,
    Warn,
    Maint,
    Live,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    Type(String),
    Tag(String),
    Env(Env),
    Is(Is),
    Ip(String),
    Group(String),
    Os(String),
    /// `kunde/standort/name`, all lowercase; the last part is the host name.
    Path(Vec<String>),
    /// Value that names nothing (`env:xyz`): matches no host.
    Never,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// `(negated, condition)`; all must hold.
    pub conds: Vec<(bool, Cond)>,
    /// Lowercase plain words; all must match fuzzily.
    pub words: Vec<String>,
}

/// Splits on whitespace; double quotes keep spaces inside a token (the quotes vanish).
pub fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|t| !t.is_empty());
    out
}

fn is_ip_prefix(t: &str) -> bool {
    let t = t.strip_suffix('.').unwrap_or(t);
    let parts: Vec<&str> = t.split('.').collect();
    parts.len() >= 2
        && parts.len() <= 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
}

fn parse_env(v: &str) -> Option<Env> {
    if v.is_empty() {
        return None;
    }
    Env::ALL
        .into_iter()
        .filter(|e| *e != Env::None)
        .find(|e| e.label().to_lowercase().starts_with(v))
}

fn parse_is(v: &str) -> Option<Is> {
    Some(match v {
        "down" | "offline" => Is::Down,
        "up" | "online" => Is::Up,
        "unknown" | "unchecked" => Is::Unknown,
        "failed" => Is::Failed,
        "fav" | "favorite" | "favourite" | "pinned" | "pin" => Is::Fav,
        "prod" | "production" => Is::Prod,
        "warn" | "warning" | "problem" => Is::Warn,
        "maint" | "maintenance" => Is::Maint,
        "live" | "open" | "connected" => Is::Live,
        _ => return None,
    })
}

/// One token as a condition; `None` = it is a plain word (or an unfinished `key:`).
fn parse_cond(tok: &str) -> Option<Option<Cond>> {
    let lower = tok.to_lowercase();
    if let Some(rest) = lower.strip_prefix('#') {
        return Some((!rest.is_empty()).then(|| Cond::Tag(rest.to_string())));
    }
    if let Some((key, val)) = lower.split_once(':') {
        let val = val.trim();
        let c = match key {
            "type" | "t" | "proto" | "protocol" | "typ" => Cond::Type(val.to_string()),
            "tag" => Cond::Tag(val.to_string()),
            "env" | "umgebung" => match parse_env(val) {
                Some(e) => Cond::Env(e),
                None => Cond::Never,
            },
            "is" | "status" => match parse_is(val) {
                Some(i) => Cond::Is(i),
                None => Cond::Never,
            },
            "ip" => Cond::Ip(val.to_string()),
            "group" | "g" | "gruppe" | "customer" | "kunde" => Cond::Group(val.to_string()),
            "os" => Cond::Os(val.to_string()),
            _ => return None,
        };
        // `type:` while still typing: no condition yet.
        if val.is_empty() {
            return Some(None);
        }
        return Some(Some(c));
    }
    if lower.contains('/') {
        // `*` alone means "any" (`kunde/*`).
        let parts: Vec<String> = lower
            .split('/')
            .map(|p| match p.trim() {
                "*" => String::new(),
                p => p.to_string(),
            })
            .collect();
        if parts.len() >= 2 && parts.iter().any(|p| !p.is_empty()) {
            return Some(Some(Cond::Path(parts)));
        }
        return None;
    }
    if is_ip_prefix(&lower) {
        return Some(Some(Cond::Ip(lower)));
    }
    None
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut q = Query::default();
        for tok in tokenize(s) {
            let (neg, body) = match tok.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest.to_string()),
                _ => (false, tok.clone()),
            };
            match parse_cond(&body) {
                Some(Some(c)) => q.conds.push((neg, c)),
                Some(None) => {}
                None => q.words.push(tok.to_lowercase()),
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.conds.is_empty() && self.words.is_empty()
    }

    /// Do all conditions hold for this host?
    pub fn matches(&self, f: &Facets) -> bool {
        self.conds.iter().all(|(neg, c)| cond_holds(c, f) != *neg)
    }

    /// Fuzzy score of the plain words against the lowercase haystack of the host
    /// (`None` = a word does not match). Without words every host scores 0.
    pub fn score(&self, hay_lc: &str) -> Option<i32> {
        let mut total = 0;
        for w in &self.words {
            total += fuzzy_lc(w, hay_lc)?;
        }
        Some(total)
    }
}

fn word_starts(hay: &str, v: &str) -> bool {
    hay.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(v))
}

fn proto_names(p: Protocol) -> &'static [&'static str] {
    match p {
        Protocol::Ssh => &["ssh"],
        Protocol::Rdp => &["rdp", "desktop"],
        Protocol::Vnc => &["vnc", "desktop"],
        Protocol::Telnet => &["telnet"],
        Protocol::Web => &["web", "http", "https", "browser", "interface"],
        Protocol::Serial => &["serial", "com", "console"],
        Protocol::Docker => &["docker", "container"],
        Protocol::Kubernetes => &["kubernetes", "k8s", "pod", "container"],
    }
}

fn cond_holds(c: &Cond, f: &Facets) -> bool {
    match c {
        Cond::Type(v) => {
            proto_names(f.protocol)
                .iter()
                .any(|n| n.starts_with(v.as_str()))
                || f.tags.iter().any(|t| word_starts(t, v))
                || f.path.iter().any(|g| word_starts(g, v))
        }
        Cond::Tag(v) => f
            .tags
            .iter()
            .any(|t| t.to_lowercase().starts_with(v.as_str())),
        Cond::Env(e) => f.env == *e,
        Cond::Is(i) => match i {
            Is::Down => f.status.is_down(),
            Is::Up => f.status == Status::Up || f.live,
            Is::Unknown => f.status == Status::Unknown && !f.live,
            Is::Failed => f.status == Status::Failed,
            Is::Fav => f.favorite,
            Is::Prod => f.env == Env::Prod,
            Is::Warn => f.warn > 0 || f.status.is_down(),
            Is::Maint => f.status == Status::Maint,
            Is::Live => f.live,
        },
        Cond::Ip(v) => f.address.starts_with(v.as_str()),
        Cond::Group(v) => f.path.iter().any(|g| g.to_lowercase().contains(v.as_str())),
        Cond::Os(v) => f.os.to_lowercase().contains(v.as_str()),
        Cond::Path(parts) => path_matches(parts, f.path, f.name),
        Cond::Never => false,
    }
}

/// `parts` = group parts followed by the host-name part (may be empty). The group parts
/// must match consecutive groups of the path, starting at any depth.
pub fn path_matches(parts: &[String], path: &[String], name: &str) -> bool {
    let Some((last, groups)) = parts.split_last() else {
        return true;
    };
    if !last.is_empty() && !name.to_lowercase().contains(last.as_str()) {
        return false;
    }
    if groups.len() > path.len() {
        return false;
    }
    (0..=path.len() - groups.len()).any(|start| {
        groups
            .iter()
            .zip(&path[start..])
            .all(|(p, g)| p.is_empty() || g.to_lowercase().contains(p.as_str()))
    })
}

/// Case-insensitive subsequence match on text that is already lowercase; higher is
/// better. Same scoring as `ui::fuzzy` without lowercasing on every call.
pub fn fuzzy_lc(query_lc: &str, text_lc: &str) -> Option<i32> {
    if query_lc.is_empty() {
        return Some(0);
    }
    if let Some(pos) = text_lc.find(query_lc) {
        return Some(1000 - pos as i32 - text_lc.len() as i32 / 4);
    }
    let mut score = 0;
    let mut chars = text_lc.chars().enumerate();
    let mut last = 0usize;
    for q in query_lc.chars() {
        let (i, _) = chars.by_ref().find(|(_, c)| *c == q)?;
        score -= (i - last) as i32;
        last = i;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facets<'a>(path: &'a [String], tags: &'a [String]) -> Facets<'a> {
        Facets {
            name: "sw-core-01",
            address: "10.1.2.3",
            tags,
            protocol: Protocol::Ssh,
            env: Env::Prod,
            path,
            status: Status::Down,
            live: false,
            favorite: false,
            warn: 0,
            os: "Ubuntu 24.04",
        }
    }

    fn p(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tokenizer_keeps_quotes() {
        assert_eq!(tokenize("a  b"), ["a", "b"]);
        assert_eq!(tokenize(r#""Kunde A"/berlin x"#), ["Kunde A/berlin", "x"]);
        assert_eq!(tokenize(r#"  "" "#), Vec::<String>::new());
        assert_eq!(
            tokenize("open \"unfinished quote"),
            ["open", "unfinished quote"]
        );
    }

    #[test]
    fn parses_conditions_and_words() {
        let q = Query::parse("web type:switch #core env:prod is:down 10.1. -is:up nginx");
        assert_eq!(q.words, ["web", "nginx"]);
        assert_eq!(
            q.conds,
            vec![
                (false, Cond::Type("switch".into())),
                (false, Cond::Tag("core".into())),
                (false, Cond::Env(Env::Prod)),
                (false, Cond::Is(Is::Down)),
                (false, Cond::Ip("10.1.".into())),
                (true, Cond::Is(Is::Up)),
            ]
        );
        assert!(Query::parse("is:down").words.is_empty());
        assert!(Query::parse("").is_empty());
    }

    #[test]
    fn unknown_keys_and_unfinished_input_are_harmless() {
        // `foo:bar` and `user@host` are plain words, `type:` alone is no filter yet.
        let q = Query::parse("foo:bar user@host type: -");
        assert_eq!(q.words, ["foo:bar", "user@host", "-"]);
        assert!(q.conds.is_empty());
        // A value that names nothing matches nothing instead of everything.
        let q = Query::parse("env:xyz");
        let path = p(&[]);
        assert!(!q.matches(&facets(&path, &[])));
        assert!(Query::parse("env:pr").matches(&facets(&path, &[])));
    }

    #[test]
    fn ip_prefixes() {
        assert!(is_ip_prefix("10.1."));
        assert!(is_ip_prefix("192.168.0"));
        assert!(!is_ip_prefix("10"));
        assert!(!is_ip_prefix("1.2.3.4.5"));
        assert!(!is_ip_prefix("web.example"));
        let path = p(&[]);
        let f = facets(&path, &[]);
        assert!(Query::parse("10.1.").matches(&f));
        assert!(Query::parse("ip:10.1.2").matches(&f));
        assert!(!Query::parse("ip:10.2").matches(&f));
        assert!(Query::parse("-ip:10.2").matches(&f));
    }

    #[test]
    fn type_matches_protocol_tag_or_group() {
        let path = p(&["Acme GmbH", "Berlin", "Core Switches"]);
        let tags = p(&["edge"]);
        let f = facets(&path, &tags);
        assert!(Query::parse("type:ssh").matches(&f));
        assert!(
            Query::parse("type:switch").matches(&f),
            "word in a group name"
        );
        assert!(Query::parse("type:ed").matches(&f), "tag prefix");
        assert!(!Query::parse("type:rdp").matches(&f));
        assert!(
            !Query::parse("type:witch").matches(&f),
            "must be a word start"
        );
    }

    #[test]
    fn state_conditions() {
        let path = p(&[]);
        let mut f = facets(&path, &[]);
        assert!(Query::parse("is:down").matches(&f));
        assert!(!Query::parse("is:up").matches(&f));
        assert!(Query::parse("is:prod").matches(&f));
        f.status = Status::Failed;
        assert!(Query::parse("is:down").matches(&f));
        assert!(Query::parse("is:failed").matches(&f));
        f.status = Status::Unknown;
        f.warn = 1;
        assert!(Query::parse("is:warn").matches(&f));
        assert!(Query::parse("is:unknown").matches(&f));
        f.live = true;
        assert!(
            Query::parse("is:up").matches(&f),
            "an open session counts as up"
        );
        assert!(!Query::parse("is:unknown").matches(&f));
        assert!(Query::parse("os:ubuntu").matches(&f));
    }

    #[test]
    fn customer_site_name_path() {
        let path = p(&["Kunde A", "Berlin"]);
        let f = facets(&path, &[]);
        assert!(Query::parse("kunde/berlin/core").matches(&f));
        assert!(Query::parse(r#""kunde a"/berlin/core"#).matches(&f));
        assert!(Query::parse("kunde/berlin/").matches(&f), "empty name part");
        assert!(
            Query::parse("berlin/core").matches(&f),
            "starts at any depth"
        );
        assert!(
            Query::parse("kunde/core").matches(&f),
            "name anywhere below the customer"
        );
        assert!(
            !Query::parse("berlin/kunde/core").matches(&f),
            "wrong order"
        );
        assert!(!Query::parse("kunde/hamburg/core").matches(&f));
        assert!(!Query::parse("kunde/berlin/zzz").matches(&f));
        assert!(
            !Query::parse("a/b/c/core").matches(&f),
            "more groups than the path has"
        );
        assert!(Query::parse("/core").matches(&f));
        assert!(Query::parse("-kunde/hamburg/").matches(&f));
    }

    #[test]
    fn words_score_fuzzy() {
        let q = Query::parse("sw 01");
        assert!(q.score("sw-core-01 10.1.2.3").is_some());
        assert!(q.score("router-02").is_none());
        let exact = Query::parse("core").score("core").unwrap();
        let late = Query::parse("core").score("xxxxxxxx core").unwrap();
        assert!(exact > late);
        assert_eq!(Query::parse("is:down").score("anything"), Some(0));
        assert!(fuzzy_lc("swc", "sw-core").is_some(), "subsequence");
        assert!(fuzzy_lc("zz", "sw-core").is_none());
    }
}
