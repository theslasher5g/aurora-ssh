//! "Geheimnis-Wächter": checks text before it is pasted into a terminal – private keys,
//! cloud and chat tokens, JWTs, `password=…` assignments, random-looking strings – and
//! multi-line pastes on PROD hosts, which a shell runs line by line right away.

use egui::RichText;

use crate::app::{App, TabKind};
use crate::model::Env;
use crate::theme::{self, Btn};
use crate::ui::icons::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    PrivateKey,
    Aws,
    GitHub,
    Slack,
    Jwt,
    Assignment,
    Entropy,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::PrivateKey => t!("Privater Schlüssel"),
            Kind::Aws => t!("AWS-Zugangsschlüssel"),
            Kind::GitHub => t!("GitHub-Token"),
            Kind::Slack => t!("Slack-Token"),
            Kind::Jwt => t!("JSON Web Token"),
            Kind::Assignment => t!("Passwort/Secret im Klartext"),
            Kind::Entropy => t!("Zufällig wirkende Zeichenfolge"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub kind: Kind,
    /// Start and end (bytes) in the scanned text.
    pub span: (usize, usize),
    /// Shortened form for the dialog (never the whole secret).
    pub masked: String,
}

fn word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn b64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// End of the run of bytes matching `f` starting at `i`.
fn run(b: &[u8], i: usize, f: impl Fn(u8) -> bool) -> usize {
    let mut j = i;
    while j < b.len() && f(b[j]) {
        j += 1;
    }
    j
}

/// Occurrences of `needle` that are not glued to a preceding word character.
fn starts(text: &str, needle: &str) -> Vec<usize> {
    let b = text.as_bytes();
    text.match_indices(needle)
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || !word_char(b[i - 1]))
        .collect()
}

/// `abcd…yz (40 Zeichen)`.
pub fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let n = chars.len();
    if n <= 8 {
        return format!("{} ({})", "•".repeat(n.max(4)), t!("{} Zeichen", n));
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[n - 2..].iter().collect();
    format!("{head}••••{tail} ({})", t!("{} Zeichen", n))
}

fn private_keys(text: &str, out: &mut Vec<Finding>) {
    for (i, _) in text.match_indices("-----BEGIN ") {
        let line_end = text[i..].find('\n').map(|e| i + e).unwrap_or(text.len());
        let header = &text[i..line_end];
        if !header.contains("PRIVATE KEY-----") {
            continue;
        }
        let end = text[i..]
            .find("-----END ")
            .and_then(|e| text[i + e..].find('\n').map(|n| i + e + n))
            .unwrap_or(text.len());
        let lines = text[i..end].lines().count();
        out.push(Finding {
            kind: Kind::PrivateKey,
            span: (i, end),
            masked: format!(
                "{} … ({})",
                header.trim_end(),
                t!("{} Zeilen", lines.max(1))
            ),
        });
    }
}

fn prefixed(text: &str, prefix: &str, min: usize, ok: fn(u8) -> bool) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    starts(text, prefix)
        .into_iter()
        .filter_map(|i| {
            let end = run(b, i + prefix.len(), ok);
            (end - i - prefix.len() >= min && (end == b.len() || !word_char(b[end])))
                .then_some((i, end))
        })
        .collect()
}

fn tokens(text: &str, out: &mut Vec<Finding>) {
    let mut add = |kind, (s, e): (usize, usize)| {
        out.push(Finding {
            kind,
            span: (s, e),
            masked: mask(&text[s..e]),
        })
    };
    for p in ["AKIA", "ASIA"] {
        for (s, e) in prefixed(text, p, 16, |c| {
            c.is_ascii_uppercase() || c.is_ascii_digit()
        }) {
            // Exactly 16 characters after the prefix.
            if e - s == 20 {
                add(Kind::Aws, (s, e));
            }
        }
    }
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        for span in prefixed(text, p, 30, |c| c.is_ascii_alphanumeric()) {
            add(Kind::GitHub, span);
        }
    }
    for span in prefixed(text, "github_pat_", 20, word_char) {
        add(Kind::GitHub, span);
    }
    for p in ["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"] {
        for span in prefixed(text, p, 10, |c| c.is_ascii_alphanumeric() || c == b'-') {
            add(Kind::Slack, span);
        }
    }
    // JWT: base64url(header) . base64url(payload) . signature – both JSON parts start
    // with `{"`, which encodes to `eyJ`.
    let b = text.as_bytes();
    for s in starts(text, "eyJ") {
        let h = run(b, s, b64url);
        if h >= b.len() || b[h] != b'.' || !text[h + 1..].starts_with("eyJ") {
            continue;
        }
        let p = run(b, h + 1, b64url);
        if p >= b.len() || b[p] != b'.' {
            continue;
        }
        let e = run(b, p + 1, b64url);
        if h - s >= 10 && p - h > 10 {
            add(Kind::Jwt, (s, e));
        }
    }
}

/// Names whose value is a secret in `name=value` / `name: value`.
const SECRET_NAMES: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "client_secret",
    "private_key",
];

fn assignments(text: &str, out: &mut Vec<Finding>) {
    let lower = text.to_ascii_lowercase();
    let b = text.as_bytes();
    for name in SECRET_NAMES {
        for (i, _) in lower.match_indices(name) {
            // `DB_PASSWORD=` counts, `nopassword=` does not.
            if i > 0 && b[i - 1].is_ascii_alphabetic() {
                continue;
            }
            let mut j = i + name.len();
            // The name may continue (`password_hash` is not a password, `secret_key` is).
            let rest = run(b, j, |c| word_char(c) || c == b'-');
            let suffix = &lower[j..rest];
            if !(suffix.is_empty()
                || suffix.ends_with("key")
                || suffix.ends_with("token")
                || suffix.ends_with("secret"))
            {
                continue;
            }
            j = rest;
            j = run(b, j, |c| c == b'"' || c == b'\'');
            j = run(b, j, |c| c == b' ' || c == b'\t');
            if j >= b.len() || !(b[j] == b'=' || b[j] == b':') {
                continue;
            }
            j = run(b, j + 1, |c| {
                c == b' ' || c == b'\t' || c == b'"' || c == b'\''
            });
            let end = run(b, j, |c| {
                !c.is_ascii_whitespace() && !matches!(c, b'"' | b'\'' | b';' | b'&' | b',' | b'}')
            });
            let value = &text[j..end];
            let placeholder = value.starts_with('$')
                || value.starts_with('<')
                || value.starts_with('{')
                || value.starts_with('%')
                || value.chars().all(|c| c == '*' || c == 'x' || c == '•');
            if value.chars().count() >= 4 && !placeholder {
                out.push(Finding {
                    kind: Kind::Assignment,
                    span: (j, end),
                    masked: format!("{}={}", &text[i..rest], mask(value)),
                });
            }
        }
    }
}

/// Shannon entropy in bits per character.
pub fn entropy(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let n = s.chars().count() as f64;
    counts
        .values()
        .map(|&k| {
            let p = k as f64 / n;
            -p * p.log2()
        })
        .sum()
}

/// Long tokens of mixed case and digits that look random (keys, generated passwords).
/// Hex hashes and UUIDs (no upper case) and paths do not count.
fn random_strings(text: &str, out: &mut Vec<Finding>) {
    let b = text.as_bytes();
    let tok = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'_' | b'-');
    let mut i = 0;
    while i < b.len() {
        if !tok(b[i]) {
            i += 1;
            continue;
        }
        let end = run(b, i, tok);
        let t = text[i..end].trim_end_matches('=');
        let len = t.len();
        if len >= 32
            && !t.starts_with('/')
            && t.matches('/').count() <= 2
            && t.bytes().any(|c| c.is_ascii_uppercase())
            && t.bytes().any(|c| c.is_ascii_lowercase())
            && t.bytes().any(|c| c.is_ascii_digit())
            && entropy(t) >= 4.0
            && !out.iter().any(|f| f.span.0 < end && i < f.span.1)
        {
            out.push(Finding {
                kind: Kind::Entropy,
                span: (i, i + len),
                masked: mask(t),
            });
        }
        i = end;
    }
}

/// Secrets in `text`, in order of appearance, without overlaps.
pub fn scan(text: &str) -> Vec<Finding> {
    let mut found = Vec::new();
    private_keys(text, &mut found);
    tokens(text, &mut found);
    assignments(text, &mut found);
    // Earlier (more specific) findings win over later ones that overlap them.
    let mut out: Vec<Finding> = Vec::new();
    for f in found {
        if !out
            .iter()
            .any(|o| o.span.0 < f.span.1 && f.span.0 < o.span.1)
        {
            out.push(f);
        }
    }
    random_strings(text, &mut out);
    out.sort_by_key(|f| f.span.0);
    out
}

/// Lines a paste would run (a trailing newline does not add one).
pub fn line_count(text: &str) -> usize {
    text.trim_end_matches(['\r', '\n']).lines().count()
}

/// Joins a multi-line paste into one command line: `\`-continuations with a space, other
/// lines with `; `. Blank lines are dropped; nothing ends in a newline.
pub fn single_line(text: &str) -> String {
    let mut out = String::new();
    let mut glue = "";
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        out.push_str(glue);
        match line.strip_suffix('\\') {
            Some(l) => {
                out.push_str(l.trim_end());
                glue = " ";
            }
            None => {
                out.push_str(line);
                glue = "; ";
            }
        }
    }
    out
}

/// A paste waiting for the user's decision.
pub struct PendingPaste {
    pub tab: u64,
    pub text: String,
    pub findings: Vec<Finding>,
    pub lines: usize,
    /// The multi-line warning applies (PROD host or "warn everywhere").
    pub guarded: bool,
    pub bracketed: bool,
}

/// What a paste into `tab` needs: `None` = paste right away.
pub fn check(text: &str, tab: u64, guarded: bool, bracketed: bool) -> Option<PendingPaste> {
    let findings = scan(text);
    let lines = line_count(text);
    (!findings.is_empty() || (guarded && lines >= 2)).then(|| PendingPaste {
        tab,
        text: text.to_string(),
        findings,
        lines,
        guarded,
        bracketed,
    })
}

/// The warning dialog.
pub fn dialog(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.termx.paste.as_ref() else {
        return;
    };
    let (host, env) = app
        .tabs
        .iter()
        .find(|t| t.id == p.tab)
        .map(|t| {
            (
                t.title.split(" · ").next().unwrap_or_default().to_string(),
                t.env,
            )
        })
        .unwrap_or_default();
    let multi = p.guarded && p.lines >= 2;
    // 0 cancel, 1 paste, 2 one line
    let mut answer = None;
    egui::Modal::new(egui::Id::new("paste-guard"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(500.0);
            ui.horizontal(|ui| {
                crate::ui::icons::show(ui, Icon::Shield, 22.0, theme::amber());
                ui.add_space(4.0);
                ui.label(theme::heading(t!("Einfügen in {}?", host), 19.0));
                if env != Env::None {
                    crate::ui::dialogs::env_badge(ui, env);
                }
            });
            ui.add_space(6.0);
            if !p.findings.is_empty() {
                ui.label(
                    RichText::new(t!(
                        "Der Text enthält etwas, das wie ein Geheimnis aussieht. Im Terminal landet es in der Shell-History und ggf. in Logs."
                    ))
                    .color(theme::text_dim()),
                );
                ui.add_space(6.0);
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for f in p.findings.iter().take(6) {
                        ui.horizontal(|ui| {
                            theme::tag(ui, f.kind.label(), Some(theme::amber()));
                            ui.add(egui::Label::new(theme::mono(&f.masked, 12.5)).truncate());
                        });
                    }
                    if p.findings.len() > 6 {
                        ui.label(
                            RichText::new(t!("… und {} weitere", p.findings.len() - 6))
                                .color(theme::text_dim()),
                        );
                    }
                });
            }
            if multi {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    crate::ui::icons::show(ui, Icon::Terminal, 16.0, theme::red());
                    let text = if p.bracketed {
                        t!("{} Zeilen werden eingefügt – eine Enter-Taste führt alle aus.", p.lines)
                    } else {
                        t!("{} Zeilen werden sofort ausgeführt.", p.lines)
                    };
                    ui.label(RichText::new(text).color(theme::red()).strong());
                });
                let preview: String = p
                    .text
                    .lines()
                    .take(4)
                    .map(|l| {
                        let mut l: String = l.chars().take(70).collect();
                        for f in &p.findings {
                            l = l.replace(&p.text[f.span.0..f.span.1], "••••");
                        }
                        l
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::mono(preview, 12.0).color(theme::text_dim()));
                });
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::btn(ui, Btn::Primary, None, t!("Abbrechen")).clicked() {
                        answer = Some(0);
                    }
                    if theme::btn(ui, Btn::Normal, None, t!("Trotzdem einfügen")).clicked() {
                        answer = Some(1);
                    }
                    if p.lines >= 2
                        && theme::btn(ui, Btn::Normal, None, t!("Als einzelne Zeile ohne Enter"))
                            .clicked()
                    {
                        answer = Some(2);
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(t!("Abschalten: Einstellungen → Terminal → Geheimnis-Wächter"))
                    .size(11.5)
                    .color(theme::text_dim()),
            );
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(0);
            }
        });
    let Some(answer) = answer else {
        return;
    };
    let Some(p) = app.termx.paste.take() else {
        return;
    };
    app.focus_term = Some(p.tab);
    let text = match answer {
        1 => p.text,
        2 => single_line(&p.text),
        _ => return,
    };
    let bytes = app
        .tabs
        .iter()
        .find(|t| t.id == p.tab)
        .and_then(|t| match &t.kind {
            TabKind::Term(term) => Some(term.term.paste_bytes(&text)),
            _ => None,
        });
    if let Some(bytes) = bytes {
        app.send_input(p.tab, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Kind> {
        scan(text).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn private_key() {
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n";
        let f = scan(key);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, Kind::PrivateKey);
        assert!(
            f[0].masked
                .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
        );
        assert!(!f[0].masked.contains("b3Blbn"));
        assert!(kinds("-----BEGIN PUBLIC KEY-----\nMFkw\n-----END PUBLIC KEY-----").is_empty());
        assert_eq!(
            kinds("-----BEGIN RSA PRIVATE KEY-----"),
            vec![Kind::PrivateKey]
        );
    }

    #[test]
    fn cloud_tokens() {
        assert_eq!(
            kinds("aws_access_key_id AKIAIOSFODNN7EXAMPLE"),
            vec![Kind::Aws]
        );
        assert!(kinds("AKIAIOSFODNN7EXAMPLE1").is_empty());
        assert!(kinds("xAKIAIOSFODNN7EXAMPLE").is_empty());
        assert_eq!(
            kinds("git clone https://ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8@github.com/x/y"),
            vec![Kind::GitHub]
        );
        assert_eq!(
            kinds("github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz"),
            vec![Kind::GitHub]
        );
        assert!(kinds("ghp_short").is_empty());
        assert_eq!(kinds("SLACK=xoxb-1234567890-abcdef"), vec![Kind::Slack]);
        assert!(kinds("xoxo-hugs").is_empty());
    }

    #[test]
    fn jwt() {
        let t = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let f = scan(t);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, Kind::Jwt);
        assert!(kinds("eyJ.eyJ.x").is_empty());
    }

    #[test]
    fn assignments_found() {
        assert_eq!(kinds("DB_PASSWORD=hunter22"), vec![Kind::Assignment]);
        assert_eq!(kinds("password: \"s3cr3t!\""), vec![Kind::Assignment]);
        assert_eq!(kinds("client_secret='abcd1234'"), vec![Kind::Assignment]);
        assert_eq!(kinds("export SECRET_KEY=abcdef"), vec![Kind::Assignment]);
        // Placeholders, references and unrelated names are fine.
        assert!(kinds("password=$DB_PASS").is_empty());
        assert!(kinds("password=<your password>").is_empty());
        assert!(kinds("password=****").is_empty());
        assert!(kinds("password_hash=abcdef").is_empty());
        assert!(kinds("nopassword=yes").is_empty());
        assert!(kinds("pwd").is_empty());
        let f = scan("token=abcdefghijkl");
        assert!(!f[0].masked.contains("abcdefghijkl"));
        assert!(f[0].masked.starts_with("token="));
    }

    #[test]
    fn random_strings_found() {
        assert_eq!(
            kinds("key: Zx8Qp2Lm9Rt4Vb7Nc1Kd5Hf3Gj6Ws0Ty8Ua2Ie"),
            vec![Kind::Entropy]
        );
        // Hashes, UUIDs, paths and prose are not secrets.
        assert!(kinds("git checkout 3f8a9c2e1b7d4f6a8c0e2b4d6f8a1c3e5b7d9f0a").is_empty());
        assert!(kinds("id 123e4567-e89b-12d3-a456-426614174000").is_empty());
        assert!(kinds("/usr/lib/x86_64-linux-gnu/libSomethingVeryLong2.so").is_empty());
        assert!(kinds("sudo apt-get update && sudo apt-get upgrade -y").is_empty());
        assert!(kinds("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAaaaa1").is_empty());
        assert!(entropy("aaaa") < 0.01);
    }

    #[test]
    fn multiline() {
        assert_eq!(line_count("ls\n"), 1);
        assert_eq!(line_count("ls\npwd\r\nwhoami\n"), 3);
        assert_eq!(
            single_line("apt-get update \\\n  && apt-get upgrade\n\nreboot\n"),
            "apt-get update && apt-get upgrade; reboot"
        );
        assert!(check("ls\npwd", 1, false, false).is_none());
        let p = check("ls\npwd", 1, true, false).unwrap();
        assert_eq!((p.lines, p.findings.len()), (2, 0));
        assert!(check("echo hi", 1, true, false).is_none());
        assert!(check("echo AKIAIOSFODNN7EXAMPLE", 1, false, false).is_some());
    }

    #[test]
    fn masking() {
        assert_eq!(mask("abc"), format!("•••• ({})", t!("{} Zeichen", 3)));
        let m = mask("abcdefghijklmnop");
        assert!(m.starts_with("abcd••••op"));
    }
}
