//! "Befehls-Gedächtnis": the commands run in SSH sessions, per host, plus the statistics
//! behind the snippet suggestions. Lives encrypted next to the vault (`history.enc`, vault
//! key) and is only in memory while the vault is unlocked.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Commands kept per host.
pub const MAX_PER_HOST: usize = 2000;
/// File next to the vault.
pub const FILE: &str = "history.enc";
/// Shortest command that may become a snippet suggestion.
pub const SUGGEST_MIN_LEN: usize = 25;
/// Runs (on any hosts) before a suggestion shows.
pub const SUGGEST_RUNS: u32 = 3;
/// Raw commands kept per pattern to find the varying parts.
const SAMPLES: usize = 6;
/// Patterns kept for suggestions (oldest dropped).
const MAX_PATTERNS: usize = 3000;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Entry {
    pub cmd: String,
    /// Last run.
    pub at: DateTime<Utc>,
    /// Working directory of the last run, if the shell reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

/// Runs of commands that share a shape (see `normalize`).
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Pattern {
    pub count: u32,
    /// The latest raw commands, oldest first.
    pub samples: Vec<String>,
    pub last: Option<DateTime<Utc>>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct History {
    pub version: u32,
    /// Host id → commands, newest first.
    pub hosts: HashMap<Uuid, Vec<Entry>>,
    /// Normalized command → runs.
    pub patterns: HashMap<String, Pattern>,
}

impl History {
    /// Remembers a command (moves a repeated one to the top). Returns the pattern key when
    /// the command has now been run often enough to suggest it as a snippet.
    pub fn record(
        &mut self,
        host: Uuid,
        cmd: &str,
        cwd: Option<String>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let list = self.hosts.entry(host).or_default();
        let count = match list.iter().position(|e| e.cmd == cmd) {
            Some(i) => list.remove(i).count.saturating_add(1),
            None => 1,
        };
        list.insert(
            0,
            Entry {
                cmd: cmd.to_string(),
                at: now,
                cwd,
                count,
            },
        );
        list.truncate(MAX_PER_HOST);

        if cmd.chars().count() < SUGGEST_MIN_LEN {
            return None;
        }
        let key = normalize(cmd);
        let p = self.patterns.entry(key.clone()).or_default();
        p.count = p.count.saturating_add(1);
        p.samples.push(cmd.to_string());
        if p.samples.len() > SAMPLES {
            p.samples.remove(0);
        }
        p.last = Some(now);
        let hit = p.count >= SUGGEST_RUNS;
        if self.patterns.len() > MAX_PATTERNS {
            // Forget the stalest patterns.
            let mut by_age: Vec<(String, Option<DateTime<Utc>>)> = self
                .patterns
                .iter()
                .map(|(k, p)| (k.clone(), p.last))
                .collect();
            by_age.sort_by_key(|(_, t)| *t);
            for (k, _) in by_age.into_iter().take(self.patterns.len() - MAX_PATTERNS) {
                self.patterns.remove(&k);
            }
        }
        hit.then_some(key)
    }

    pub fn clear_host(&mut self, host: Uuid) {
        self.hosts.remove(&host);
    }

    pub fn clear(&mut self) {
        self.hosts.clear();
        self.patterns.clear();
    }

    /// (hosts with commands, commands).
    pub fn totals(&self) -> (usize, usize) {
        let hosts = self.hosts.values().filter(|l| !l.is_empty()).count();
        (hosts, self.hosts.values().map(Vec::len).sum())
    }

    pub fn entries(&self, host: Uuid) -> &[Entry] {
        self.hosts.get(&host).map(Vec::as_slice).unwrap_or_default()
    }
}

/// True for commands that must not be remembered: empty, starting with a space (bash
/// `HISTCONTROL=ignorespace`) or carrying something that looks like a secret.
pub fn should_record(cmd: &str) -> bool {
    !cmd.trim().is_empty() && !cmd.starts_with(' ') && !looks_secret(cmd)
}

/// Passwords, tokens and keys typed on the command line.
pub fn looks_secret(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    const WORDS: &[&str] = &[
        "password",
        "passwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "api-key",
        "private_key",
        "--pass",
        "sshpass",
    ];
    if WORDS.iter().any(|w| lower.contains(w)) {
        return true;
    }
    // `export AWS_SECRET_ACCESS_KEY=…`, `export KEY=…`
    for part in lower.split([';', '&', '|']) {
        let mut words = part.split_whitespace();
        if words.next() == Some("export") {
            for w in words {
                if let Some((name, _)) = w.split_once('=')
                    && (name.contains("key") || name.contains("pass") || name.contains("auth"))
                {
                    return true;
                }
            }
        }
    }
    // `mysql -pS3cret`
    if cmd
        .split_whitespace()
        .any(|w| w.len() > 2 && w.starts_with("-p") && is_mysqlish(cmd))
    {
        return true;
    }
    !crate::ui::paste_guard::scan(cmd).is_empty()
}

fn is_mysqlish(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or_default();
    matches!(first, "mysql" | "mysqldump" | "mariadb" | "mysqladmin")
}

/// Kind of a token whose value tends to change between runs of the same command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Ip,
    Number,
    Path,
    Target,
    Host,
}

impl Slot {
    fn marker(self) -> &'static str {
        match self {
            Slot::Ip => "<ip>",
            Slot::Number => "<n>",
            Slot::Path => "<path>",
            Slot::Target => "<target>",
            Slot::Host => "<host>",
        }
    }

    fn var(self) -> &'static str {
        match self {
            Slot::Ip => "ip",
            Slot::Number => "n",
            Slot::Path => "pfad",
            Slot::Target => "ziel",
            Slot::Host => "host",
        }
    }
}

fn is_ipv4(s: &str) -> bool {
    let s = s.split(['/', ':']).next().unwrap_or_default();
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u16>().is_ok_and(|v| v <= 255))
}

/// What kind of variable a token could be.
pub fn classify(tok: &str) -> Option<Slot> {
    let t = tok.trim_matches(|c| c == '"' || c == '\'');
    if t.is_empty() {
        return None;
    }
    if is_ipv4(t) {
        return Some(Slot::Ip);
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return Some(Slot::Number);
    }
    if t.starts_with('/') || t.starts_with("./") || t.starts_with("../") || t.starts_with("~/") {
        return Some(Slot::Path);
    }
    if let Some((user, host)) = t.split_once('@')
        && !user.is_empty()
        && !host.is_empty()
        && !user.contains(['/', ':', '='])
    {
        return Some(Slot::Target);
    }
    let hostish = t.contains('.')
        && !t.starts_with('-')
        && !t.ends_with('.')
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    hostish.then_some(Slot::Host)
}

/// Splits `--name=value` / `name=value` into ("--name=", "value").
fn split_assign(tok: &str) -> (&str, &str) {
    match tok.find('=') {
        Some(i) if i > 0 && !tok[..i].contains(['/', '"', '\'']) => (&tok[..=i], &tok[i + 1..]),
        _ => ("", tok),
    }
}

/// Shape of a command: whitespace collapsed, IPs, numbers, paths, `user@host` and host
/// names replaced by placeholders. `ssh deploy@10.0.0.5 -p 22` and
/// `ssh deploy@10.0.0.9 -p 2222` share one shape.
pub fn normalize(cmd: &str) -> String {
    cmd.split_whitespace()
        .map(|tok| {
            let (prefix, value) = split_assign(tok);
            match classify(value) {
                Some(slot) => format!("{prefix}{}", slot.marker()),
                None => tok.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Snippet text for runs of one pattern: tokens that differ between the samples become
/// `{{variables}}` named after their kind (`{{ip}}`, `{{host}}`, `{{pfad}}` …). Positions
/// that always carry the same value share one variable.
pub fn template(samples: &[String]) -> String {
    let rows: Vec<Vec<&str>> = samples
        .iter()
        .map(|s| s.split_whitespace().collect())
        .collect();
    let Some(first) = rows.last() else {
        return String::new();
    };
    let n = first.len();
    let same_len: Vec<&Vec<&str>> = rows.iter().filter(|r| r.len() == n).collect();
    let mut names: Vec<String> = Vec::new();
    // Column values (of the value part) → variable name, to reuse for equal columns.
    let mut columns: Vec<(Vec<&str>, String)> = Vec::new();
    let mut out = Vec::with_capacity(n);
    for (i, tok) in first.iter().enumerate() {
        let col: Vec<&str> = same_len.iter().map(|r| split_assign(r[i]).1).collect();
        let prefixes_equal = same_len
            .iter()
            .all(|r| split_assign(r[i]).0 == split_assign(tok).0);
        let varies = col.iter().any(|v| *v != col[0]);
        if !varies || !prefixes_equal {
            out.push(tok.to_string());
            continue;
        }
        let (prefix, value) = split_assign(tok);
        let name = if let Some((_, name)) = columns.iter().find(|(c, _)| *c == col) {
            name.clone()
        } else {
            let base = classify(value).map(Slot::var).unwrap_or("wert");
            let mut name = base.to_string();
            let mut k = 2;
            while names.contains(&name) {
                name = format!("{base}{k}");
                k += 1;
            }
            names.push(name.clone());
            columns.push((col, name.clone()));
            name
        };
        out.push(format!("{prefix}{{{{{name}}}}}"));
    }
    out.join(" ")
}

/// Snippet name proposal: the first words that are not variables.
pub fn snippet_name(template: &str) -> String {
    let words: Vec<&str> = template
        .split_whitespace()
        .filter(|w| !w.contains("{{") && !w.starts_with('-'))
        .take(3)
        .collect();
    let name = words.join(" ");
    name.chars().take(40).collect()
}

/// Short stable id of a pattern (stored when a suggestion is dismissed; no command text
/// ends up in the plain settings file).
pub fn pattern_id(key: &str) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(key.as_bytes());
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// History entries matching `query` (fuzzy), best first; empty query = newest first.
pub fn search<'a>(entries: &'a [Entry], query: &str, limit: usize) -> Vec<&'a Entry> {
    let q = query.trim();
    if q.is_empty() {
        return entries.iter().take(limit).collect();
    }
    let mut hits: Vec<(i32, usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| crate::ui::fuzzy(q, &e.cmd).map(|s| (s, i, e)))
        .collect();
    // Better match first; among equal ones the newer.
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().take(limit).map(|(_, _, e)| e).collect()
}

pub fn load(vault: &crate::store::Vault) -> anyhow::Result<History> {
    Ok(match vault.load_blob(FILE)? {
        Some(bytes) => serde_json::from_slice(&bytes)?,
        None => History::default(),
    })
}

pub fn save(vault: &crate::store::Vault, h: &History) -> anyhow::Result<()> {
    vault.save_blob(FILE, &serde_json::to_vec(h)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        assert!(should_record("ls -la"));
        assert!(!should_record(""));
        assert!(!should_record("   "));
        assert!(!should_record(" ls -la"));
        assert!(!should_record("curl -u admin:x https://a?password=hunter2"));
        assert!(!should_record("export GITHUB_TOKEN=abc"));
        assert!(!should_record(
            "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"
        ));
        assert!(!should_record("export DB_PASS=x"));
        assert!(!should_record("mysql -uroot -pS3cret db"));
        assert!(should_record("export PATH=$PATH:/opt/bin"));
        assert!(should_record("ps -p 1234"));
        assert!(!should_record(
            "echo ghp_abcdefghijklmnopqrstuvwxyz0123456789"
        ));
    }

    #[test]
    fn dedup_and_limit() {
        let mut h = History::default();
        let host = Uuid::new_v4();
        let now = Utc::now();
        h.record(host, "ls", None, now);
        h.record(host, "df -h", Some("/srv".into()), now);
        h.record(host, "ls", None, now);
        let e = h.entries(host);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].cmd.as_str(), e[0].count), ("ls", 2));
        assert_eq!(e[1].cwd.as_deref(), Some("/srv"));
        for i in 0..(MAX_PER_HOST + 50) {
            h.record(host, &format!("echo {i}"), None, now);
        }
        assert_eq!(h.entries(host).len(), MAX_PER_HOST);
        assert_eq!(
            h.entries(host)[0].cmd,
            format!("echo {}", MAX_PER_HOST + 49)
        );
        assert_eq!(h.totals(), (1, MAX_PER_HOST));
        h.clear_host(host);
        assert_eq!(h.totals(), (0, 0));
    }

    #[test]
    fn normalization() {
        assert_eq!(
            normalize("ssh   deploy@10.0.0.5 -p 22"),
            normalize("ssh deploy@10.0.0.9 -p 2222")
        );
        assert_eq!(normalize("ping 10.0.0.5"), "ping <ip>");
        assert_eq!(normalize("ping web-01.example.com"), "ping <host>");
        assert_eq!(normalize("tail -n 200 /var/log/x"), "tail -n <n> <path>");
        assert_eq!(normalize("curl --max-time=30 x"), "curl --max-time=<n> x");
        assert_ne!(
            normalize("systemctl restart nginx"),
            normalize("systemctl stop nginx")
        );
        assert_eq!(classify("-la"), None);
        assert_eq!(classify("10.0.0.1/24"), Some(Slot::Ip));
        assert_eq!(classify("1.2.3"), None);
    }

    #[test]
    fn templates_mark_varying_parts() {
        let runs = [
            "rsync -avz /srv/a/ deploy@10.0.0.5:/srv/a/ --bwlimit=500".to_string(),
            "rsync -avz /srv/b/ deploy@10.0.0.6:/srv/b/ --bwlimit=500".to_string(),
            "rsync -avz /srv/c/ deploy@10.0.0.7:/srv/c/ --bwlimit=900".to_string(),
        ];
        assert_eq!(
            template(&runs),
            "rsync -avz {{pfad}} {{ziel}} --bwlimit={{n}}"
        );
        let same = vec!["docker compose logs -f --tail 100".to_string(); 3];
        assert_eq!(template(&same), "docker compose logs -f --tail 100");
        // The same varying value in two places shares one variable.
        let runs = [
            "ping -c 3 10.0.0.1 && ssh 10.0.0.1 uptime".to_string(),
            "ping -c 3 10.0.0.2 && ssh 10.0.0.2 uptime".to_string(),
        ];
        assert_eq!(template(&runs), "ping -c 3 {{ip}} && ssh {{ip}} uptime");
        let runs = [
            "kubectl -n shop scale deploy api --replicas=3".to_string(),
            "kubectl -n shop scale deploy api --replicas=5".to_string(),
            "kubectl -n shop scale deploy web --replicas=5".to_string(),
        ];
        assert_eq!(
            template(&runs),
            "kubectl -n shop scale deploy {{wert}} --replicas={{n}}"
        );
        assert_eq!(snippet_name(&template(&runs)), "kubectl shop scale");
    }

    #[test]
    fn suggestion_after_three_runs() {
        let mut h = History::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let now = Utc::now();
        assert_eq!(
            h.record(a, "journalctl -u nginx -n 200 --no-pager", None, now),
            None
        );
        assert_eq!(
            h.record(b, "journalctl -u nginx -n 500 --no-pager", None, now),
            None
        );
        let key = h.record(a, "journalctl -u nginx -n 50 --no-pager", None, now);
        assert_eq!(
            key.as_deref(),
            Some("journalctl -u nginx -n <n> --no-pager")
        );
        let p = &h.patterns[key.as_ref().unwrap()];
        assert_eq!(
            template(&p.samples),
            "journalctl -u nginx -n {{n}} --no-pager"
        );
        // Short commands never become suggestions.
        for _ in 0..5 {
            assert_eq!(h.record(a, "ls -la", None, now), None);
        }
        assert_eq!(pattern_id("x").len(), 16);
    }

    #[test]
    fn fuzzy_search() {
        let mut h = History::default();
        let host = Uuid::new_v4();
        let now = Utc::now();
        for c in [
            "git status",
            "docker ps -a",
            "systemctl status nginx",
            "git log",
        ] {
            h.record(host, c, None, now);
        }
        let hits = search(h.entries(host), "stat", 10);
        assert_eq!(hits[0].cmd, "git status");
        assert_eq!(hits.len(), 2);
        assert_eq!(search(h.entries(host), "", 2)[0].cmd, "git log");
        assert_eq!(search(h.entries(host), "dps", 10)[0].cmd, "docker ps -a");
    }

    #[test]
    fn serde_roundtrip() {
        let mut h = History::default();
        h.record(Uuid::new_v4(), "uptime", Some("/".into()), Utc::now());
        let back: History = serde_json::from_slice(&serde_json::to_vec(&h).unwrap()).unwrap();
        assert_eq!(back, h);
    }
}
