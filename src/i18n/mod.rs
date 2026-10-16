//! UI language. The source strings are German; `t!("…")` looks up the English text when
//! English is selected. Templates use `{}` placeholders filled in order:
//! `t!("{} Hosts hinzugefügt", n)`.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

mod app;
mod backend;
mod ui_core;
mod ui_files;
mod ui_forms;
mod ui_hosts;
mod ui_v052;
mod ui_v060;
mod ui_v061_rdp;
mod ui_vnc;

static ENGLISH: AtomicBool = AtomicBool::new(false);

const TABLES: &[&[(&str, &str)]] = &[
    app::T,
    backend::T,
    ui_core::T,
    ui_files::T,
    ui_forms::T,
    ui_hosts::T,
    ui_v052::T,
    ui_v060::T,
    ui_v061_rdp::T,
    ui_vnc::T,
];

/// "de", "en" or "" (follow the system: German for German locales, English otherwise).
pub fn set_language(code: &str) {
    let english = match code {
        "en" => true,
        "de" => false,
        _ => !sys_locale::get_locale().is_some_and(|l| l.to_lowercase().starts_with("de")),
    };
    ENGLISH.store(english, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| TABLES.iter().flat_map(|t| t.iter().copied()).collect())
}

/// The text in the current language (German source text if there is no translation).
pub fn tr(s: &str) -> &str {
    if is_english() {
        table().get(s).copied().unwrap_or(s)
    } else {
        s
    }
}

/// Translates `template`, then replaces each `{}` with the next argument.
pub fn trf(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let t = tr(template);
    let mut out = String::with_capacity(t.len() + 16);
    let mut args = args.iter();
    let mut rest = t;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        match args.next() {
            Some(a) => out.push_str(&a.to_string()),
            None => out.push_str("{}"),
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// `t!("Text")` → `&str`, `t!("Text {}", x)` → `String`.
#[macro_export]
macro_rules! t {
    ($s:literal) => {
        $crate::i18n::tr($s)
    };
    ($s:literal, $($a:expr),+ $(,)?) => {
        $crate::i18n::trf($s, &[$(&$a as &dyn ::std::fmt::Display),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `t!("…")` literal in the sources must have an English entry.
    #[test]
    fn every_text_is_translated() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(root)];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if !p.ends_with("i18n") {
                        stack.push(p);
                    }
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                // Windows checkouts have CRLF line endings; rustc normalises them too.
                let src = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
                for lit in literals(&src) {
                    if !table().contains_key(lit.as_str()) {
                        missing.push(format!("{}: {lit}", p.display()));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "missing translations:\n{}",
            missing.join("\n")
        );
    }

    /// String literals that directly follow `t!(`.
    fn literals(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("t!(") {
            // `format!(` and friends end in `t!(` too.
            let standalone = rest[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let after = rest[i + 3..].trim_start();
            rest = &rest[i + 3..];
            if !standalone {
                continue;
            }
            let Some(body) = after.strip_prefix('"') else {
                continue;
            };
            let mut s = String::new();
            let mut chars = body.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('\n') => {
                            // line continuation: skip leading whitespace
                            let r: String = chars.clone().collect();
                            let trimmed = r.trim_start();
                            let skip = r.len() - trimmed.len();
                            for _ in 0..r[..skip].chars().count() {
                                chars.next();
                            }
                        }
                        Some(o) => s.push(o),
                        None => break,
                    },
                    c => s.push(c),
                }
            }
            out.push(s);
        }
        out
    }

    #[test]
    fn templates() {
        set_language("de");
        assert!(!is_english());
        assert_eq!(trf("{} von {}", &[&1, &"x"]), "1 von x");
        assert_eq!(tr("unbekannt"), "unbekannt");
    }
}
