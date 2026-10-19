//! Info snippets of the inventory: commands the user defines that run over SSH on every
//! matching host when the facts are refreshed. Only their text output is kept (bounded)
//! and shown as text – it is never interpreted. This is the way to collect anything the
//! built-in facts do not cover (installed packages, cron jobs, containers; on Windows
//! with OpenSSH the user enters their own PowerShell commands).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::Host;

/// Output kept per snippet and host.
pub const MAX_OUTPUT: usize = 64 * 1024;
/// More snippets than this are ignored (keeps a refresh short).
pub const MAX_SNIPPETS: usize = 30;

fn yes() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct InfoSnippet {
    pub id: Uuid,
    pub name: String,
    /// Run as typed through the login shell of the host.
    pub command: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Only this host.
    pub host: Option<Uuid>,
    /// Only hosts in this group or below.
    pub group: Option<Uuid>,
    /// Only hosts with this tag (case-insensitive).
    pub tag: String,
}

impl Default for InfoSnippet {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            command: String::new(),
            enabled: true,
            host: None,
            group: None,
            tag: String::new(),
        }
    }
}

impl InfoSnippet {
    pub fn runnable(&self) -> bool {
        self.enabled && !self.name.trim().is_empty() && !self.command.trim().is_empty()
    }

    /// Whether this snippet runs on the host (`group_chain`: its group and parents).
    pub fn applies(&self, host: &Host, group_chain: &[Uuid]) -> bool {
        self.runnable()
            && self.host.is_none_or(|h| h == host.id)
            && self.group.is_none_or(|g| group_chain.contains(&g))
            && (self.tag.trim().is_empty()
                || host
                    .tags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(self.tag.trim())))
    }
}

/// Result of one snippet on one host.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct InfoResult {
    pub snippet: Option<Uuid>,
    /// Name at the time of the run (the snippet may be renamed or deleted later).
    pub name: String,
    pub output: String,
    pub exit: Option<u32>,
    /// Why there is no output (timeout, channel error).
    pub error: String,
    pub truncated: bool,
    pub at: Option<DateTime<Utc>>,
}

/// Snippets offered to add with one click.
pub fn presets() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            t!("Installierte Pakete"),
            "dpkg-query -W -f='${Package} ${Version}\\n' 2>/dev/null || rpm -qa --qf '%{NAME} %{VERSION}-%{RELEASE}\\n' 2>/dev/null || apk info -v 2>/dev/null",
        ),
        (
            t!("Cron-Jobs"),
            "for f in /etc/crontab /etc/cron.d/*; do [ -f \"$f\" ] && echo \"## $f\" && grep -v '^#' \"$f\"; done 2>/dev/null; crontab -l 2>/dev/null",
        ),
        (
            t!("Docker-Container"),
            "docker ps -a --format 'table {{.Names}}\\t{{.Image}}\\t{{.Status}}' 2>&1",
        ),
        (
            t!("Offene Ports"),
            "ss -tuln 2>/dev/null || netstat -tuln 2>/dev/null",
        ),
    ]
}

/// Output text made safe to store and show: lossy UTF-8, terminal escape sequences and
/// control characters (except line breaks and tabs) removed, cut at [`MAX_OUTPUT`].
/// Returns the text and whether it was cut.
pub fn clean_output(raw: &[u8]) -> (String, bool) {
    let text = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(text.len().min(MAX_OUTPUT));
    let mut chars = text.chars().peekable();
    let mut truncated = false;
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI (ESC [ … final byte) or OSC (ESC ] … BEL / ESC \): skip.
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' || (n == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let c = match c {
            '\r' => continue,
            '\t' => '\t',
            '\n' => '\n',
            c if c.is_control() => continue,
            c => c,
        };
        if out.len() + c.len_utf8() > MAX_OUTPUT {
            truncated = true;
            break;
        }
        out.push(c);
    }
    (out.trim_end().to_string(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(tags: &[&str]) -> Host {
        Host {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn filters() {
        let g1 = Uuid::new_v4();
        let g2 = Uuid::new_v4();
        let h = host(&["Web", "prod"]);
        let mut s = InfoSnippet {
            name: "x".into(),
            command: "uname -a".into(),
            ..Default::default()
        };
        assert!(s.applies(&h, &[]));
        s.tag = "web".into();
        assert!(s.applies(&h, &[]));
        s.tag = "db".into();
        assert!(!s.applies(&h, &[]));
        s.tag.clear();
        s.group = Some(g1);
        assert!(!s.applies(&h, &[g2]));
        assert!(s.applies(&h, &[g1, g2]), "parent groups count");
        s.group = None;
        s.host = Some(Uuid::new_v4());
        assert!(!s.applies(&h, &[]));
        s.host = Some(h.id);
        assert!(s.applies(&h, &[]));
        s.enabled = false;
        assert!(!s.applies(&h, &[]));
        s.enabled = true;
        s.command = "  ".into();
        assert!(!s.applies(&h, &[]), "empty command never runs");
    }

    #[test]
    fn output_is_cleaned_and_bounded() {
        let (t, cut) = clean_output(b"a\x1b[31mred\x1b[0m\r\nb\tc\x07\x00d\x1b]0;title\x07e\n\n");
        assert_eq!(t, "ared\nb\tcde");
        assert!(!cut);
        let big = vec![b'x'; MAX_OUTPUT + 500];
        let (t, cut) = clean_output(&big);
        assert_eq!(t.len(), MAX_OUTPUT);
        assert!(cut);
        // Invalid UTF-8 does not panic.
        let (t, _) = clean_output(&[0xff, b'o', b'k']);
        assert!(t.ends_with("ok"));
    }

    #[test]
    fn old_settings_and_defaults() {
        let s: InfoSnippet = serde_json::from_str(r#"{"name":"a","command":"id"}"#).unwrap();
        assert!(s.enabled && s.runnable());
        assert!(presets().len() >= 4);
        assert!(
            presets()
                .iter()
                .all(|(n, c)| !n.is_empty() && !c.is_empty())
        );
    }
}
