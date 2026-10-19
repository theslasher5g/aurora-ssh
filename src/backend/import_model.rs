//! Migration importer: common data model of all sources (mRemoteNG, Royal TS, Remote
//! Desktop Manager, CSV) and the helpers every parser uses (size limits, sanitising).

use std::collections::BTreeMap;

/// Biggest file the importer reads.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Most entries one file may contribute.
pub const MAX_ITEMS: usize = 200_000;
/// Deepest folder nesting that is kept; deeper levels are merged into the last one.
pub const MAX_DEPTH: usize = 12;
/// Longest group / host name.
pub const MAX_NAME: usize = 120;
/// Skipped entries kept for the report (the rest is only counted).
pub const MAX_SKIPPED_LIST: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Source {
    MRemoteNg,
    RoyalTs,
    Rdm,
    Csv,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::MRemoteNg => "mRemoteNG",
            Source::RoyalTs => "Royal TS",
            Source::Rdm => "Remote Desktop Manager",
            Source::Csv => "CSV",
        }
    }
}

/// Kind of connection in the source, before it is mapped to an Aurora protocol.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Kind {
    Ssh,
    Rdp,
    Vnc,
    Telnet,
    Web,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Ssh => "SSH",
            Kind::Rdp => "RDP",
            Kind::Vnc => "VNC",
            Kind::Telnet => "Telnet",
            Kind::Web => "HTTP/HTTPS",
            Kind::Other => "?",
        }
    }

    /// Free text ("SSH2", "RDPConfigured", "https", "vnc") → kind.
    pub fn from_text(s: &str) -> Kind {
        let l = s.trim().to_ascii_lowercase();
        if l.is_empty() {
            return Kind::Ssh;
        }
        if l.contains("ssh") || l == "sftp" || l == "scp" {
            Kind::Ssh
        } else if l.contains("rdp") || l.contains("rds") || l.contains("remotedesktop") || l == "ts"
        {
            Kind::Rdp
        } else if l.contains("vnc") {
            Kind::Vnc
        } else if l.contains("telnet") {
            Kind::Telnet
        } else if l.contains("http") || l.contains("web") {
            Kind::Web
        } else {
            Kind::Other
        }
    }
}

/// One host (or web link) found in a source file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportItem {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub kind: Option<Kind>,
    /// Protocol text as written in the source, for the report.
    pub raw_type: String,
    pub user: String,
    /// Only ever filled from clear-text sources (CSV); used only after an opt-in.
    pub password: String,
    pub group: Vec<String>,
    pub tags: Vec<String>,
    pub notes: String,
}

impl ImportItem {
    pub fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Ssh)
    }
}

/// An entry that was not turned into a host, with the reason.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct ImportDoc {
    pub source: Option<Source>,
    pub items: Vec<ImportItem>,
    /// Entries the parser dropped on its own (credentials, unknown types …).
    pub skipped: Vec<Skipped>,
    pub skipped_total: usize,
    /// Notes shown in the preview (passwords not imported, encrypted file, …).
    pub warnings: Vec<String>,
    /// Entries that carried an encrypted / hidden password that was not imported.
    pub passwords_not_imported: usize,
    /// The whole file is encrypted; nothing can be read.
    pub encrypted: bool,
}

impl ImportDoc {
    pub fn new(source: Source) -> Self {
        Self {
            source: Some(source),
            ..Default::default()
        }
    }

    pub fn skip(&mut self, name: &str, reason: impl Into<String>) {
        self.skipped_total += 1;
        if self.skipped.len() < MAX_SKIPPED_LIST {
            self.skipped.push(Skipped {
                name: name.to_string(),
                reason: reason.into(),
            });
        }
    }

    /// Adds an item unless the item limit is hit.
    pub fn push(&mut self, item: ImportItem) -> bool {
        if self.items.len() >= MAX_ITEMS {
            self.skip(&item.name, t!("Zu viele Einträge (Obergrenze erreicht)"));
            return false;
        }
        self.items.push(item);
        true
    }

    /// Count of items per kind, in a stable order.
    pub fn kind_counts(&self) -> BTreeMap<Kind, usize> {
        let mut m = BTreeMap::new();
        for i in &self.items {
            *m.entry(i.kind()).or_insert(0) += 1;
        }
        m
    }
}

/// Name for a group or host: no control characters, trimmed, shortened.
pub fn clean_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > MAX_NAME {
        t.chars().take(MAX_NAME - 1).collect::<String>() + "…"
    } else {
        t
    }
}

/// Splits `Kunde\Standort/Raum` style paths; empty parts vanish.
pub fn split_path(s: &str) -> Vec<String> {
    s.split(['\\', '/'])
        .map(clean_name)
        .filter(|p| !p.is_empty())
        .collect()
}

/// Limits a path to [`MAX_DEPTH`] levels by merging the overflow into the last level.
pub fn limit_depth(mut path: Vec<String>) -> Vec<String> {
    if path.len() > MAX_DEPTH {
        let rest = path.split_off(MAX_DEPTH - 1);
        path.push(clean_name(&rest.join(" / ")));
    }
    path
}

pub fn parse_port(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

/// Host names and addresses only: no spaces, control characters or shell metacharacters.
pub fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']' | '%')
        })
}

/// Host part of a URL (`https://host:8443/x` → `host`), or the text itself.
pub fn host_of_url(s: &str) -> String {
    let s = s.trim();
    let s = s.split_once("://").map_or(s, |(_, r)| r);
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit_once('@').map_or(s, |(_, r)| r);
    if s.starts_with('[') {
        return s
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
            .to_string();
    }
    match s.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.contains(':') => h.to_string(),
        _ => s.to_string(),
    }
}

/// Tags from a free text list (`a, b; c|d`).
pub fn split_tags(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in s.split([',', ';', '|']) {
        let t = clean_name(t);
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Reads a whole file with the size limit applied before reading.
pub fn read_limited(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(t!("Das ist keine Datei.").into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(t!("Die Datei ist zu groß (höchstens 50 MB).").into());
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_end(&mut buf).map(|_| ()))
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return Err(t!("Die Datei ist zu groß (höchstens 50 MB).").into());
    }
    Ok(buf)
}

/// Text of a file: UTF-8 (BOM removed); UTF-16 with BOM is converted; anything else is
/// read lossy.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let u: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let u: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_text() {
        assert_eq!(Kind::from_text("SSH2"), Kind::Ssh);
        assert_eq!(Kind::from_text("RDPConfigured"), Kind::Rdp);
        assert_eq!(Kind::from_text("HTTPS"), Kind::Web);
        assert_eq!(Kind::from_text("WebBrowser"), Kind::Web);
        assert_eq!(Kind::from_text("Telnet"), Kind::Telnet);
        assert_eq!(Kind::from_text("ICA"), Kind::Other);
    }

    #[test]
    fn paths_and_depth() {
        assert_eq!(
            split_path("Kunde\\Standort/ Raum 1 "),
            ["Kunde", "Standort", "Raum 1"]
        );
        let deep: Vec<String> = (0..15).map(|i| format!("g{i}")).collect();
        let l = limit_depth(deep);
        assert_eq!(l.len(), MAX_DEPTH);
        assert!(l[MAX_DEPTH - 1].starts_with("g11 / g12"));
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  a\u{0}\tb\n c "), "a b c");
        assert_eq!(clean_name(&"x".repeat(500)).chars().count(), MAX_NAME);
    }

    #[test]
    fn host_validation() {
        assert!(valid_host("srv-01.example.com"));
        assert!(valid_host("fe80::1%eth0"));
        assert!(!valid_host("a b"));
        assert!(!valid_host("a;rm -rf"));
        assert!(!valid_host(""));
    }

    #[test]
    fn url_host() {
        assert_eq!(
            host_of_url("https://user@wiki.local:8443/a?b"),
            "wiki.local"
        );
        assert_eq!(host_of_url("10.0.0.1"), "10.0.0.1");
        assert_eq!(host_of_url("http://[::1]:80/"), "::1");
    }

    #[test]
    fn decode_variants() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFabc"), "abc");
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
    }
}
