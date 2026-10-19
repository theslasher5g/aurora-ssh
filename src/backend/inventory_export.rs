//! Server inventory export: CSV, Markdown table and JSON of the (filtered) rows with the
//! columns on screen. Never contains secrets (no user names, passwords, keys, notes).

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::inventory_rules::{Col, Row, cell};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Markdown,
    Json,
    Html,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Markdown => "md",
            Format::Json => "json",
            Format::Html => "html",
        }
    }
}

/// Where exports go: `Documents/Aurora/Inventar`, else the app data folder.
pub fn dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(|d| d.join("Aurora").join("Inventory")))
        .unwrap_or_else(|| crate::store::data_dir().join("inventory"))
}

/// A cell that a spreadsheet would run as a formula gets a leading `'`.
pub fn neutralize(s: &str) -> String {
    if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    }
}

/// One CSV field: formula-safe, quoted when it holds a comma, quote or line break.
pub fn csv_field(s: &str) -> String {
    let s = neutralize(s);
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

pub fn csv(rows: &[Row], cols: &[Col], now: DateTime<Utc>) -> String {
    let mut out = String::new();
    out.push_str(
        &cols
            .iter()
            .map(|c| csv_field(c.label()))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push_str("\r\n");
    for r in rows {
        out.push_str(
            &cols
                .iter()
                .map(|c| csv_field(&cell(r, *c, now)))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
    }
    out
}

/// Text inside a Markdown table cell: pipes, markup and HTML made harmless, one line.
pub fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '|' | '`' | '*' | '_' | '[' | ']' | '#' => {
                out.push('\\');
                out.push(c);
            }
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

pub fn markdown(rows: &[Row], cols: &[Col], now: DateTime<Utc>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "| {} |\n",
        cols.iter()
            .map(|c| md_escape(c.label()))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    out.push_str(&format!(
        "|{}\n",
        cols.iter()
            .map(|c| if c.numeric() { " ---: |" } else { " --- |" })
            .collect::<String>()
    ));
    for r in rows {
        out.push_str(&format!(
            "| {} |\n",
            cols.iter()
                .map(|c| md_escape(&cell(r, *c, now)))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    out
}

/// JSON with stable English keys; numbers stay numbers.
pub fn json(rows: &[Row], cols: &[Col], now: DateTime<Utc>) -> String {
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut m = Map::new();
            for c in cols {
                let v = match c {
                    Col::Uptime => json!(r.uptime_secs),
                    Col::Disk => json!(r.disk),
                    Col::Updates => json!(r.updates),
                    Col::SecUpdates => json!(r.sec_updates),
                    Col::Cpu => json!(r.cpus),
                    Col::Ram => json!({"total_mb": r.mem_mb, "used_percent": r.mem_pct}),
                    Col::Load => json!(r.load1),
                    Col::Last => json!(r.last_connected.map(|t| t.to_rfc3339())),
                    Col::FactsAge => json!(r.facts_at.map(|t| t.to_rfc3339())),
                    Col::Tags => json!(r.tags),
                    Col::Failed => json!(r.failed_services),
                    other => json!(cell(r, *other, now)),
                };
                let key = match c {
                    Col::Uptime => "uptime_seconds",
                    Col::Disk => "disk_used_percent",
                    Col::Last => "last_connected",
                    Col::FactsAge => "facts_collected",
                    other => other.key(),
                };
                m.insert(key.to_string(), v);
            }
            m.insert(
                "warnings".into(),
                json!(
                    r.warnings
                        .iter()
                        .map(|w| w.text.clone())
                        .collect::<Vec<_>>()
                ),
            );
            Value::Object(m)
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "exported": now.to_rfc3339(),
        "hosts": items,
    }))
    .unwrap_or_default()
}

pub fn render(f: Format, rows: &[Row], cols: &[Col], now: DateTime<Utc>) -> String {
    match f {
        Format::Csv => csv(rows, cols, now),
        Format::Markdown => markdown(rows, cols, now),
        Format::Json => json(rows, cols, now),
        // The HTML documentation is built by `inventory_doc`.
        Format::Html => String::new(),
    }
}

/// Writes the export to `Documents/Aurora/Inventory/inventory_<time>.<ext>`.
pub fn save(f: Format, text: &str) -> Result<PathBuf> {
    let d = dir();
    std::fs::create_dir_all(&d).with_context(|| d.display().to_string())?;
    let name = format!(
        "inventory_{}.{}",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"),
        f.ext()
    );
    let path = d.join(name);
    let mut bytes = Vec::new();
    if f == Format::Csv {
        // Excel only reads umlauts right with a byte order mark.
        bytes.extend_from_slice("\u{feff}".as_bytes());
    }
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(&path, bytes).with_context(|| path.display().to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::inventory_rules::{Level, WarnKind, Warning};
    use crate::model::{Env, Protocol};
    use uuid::Uuid;

    fn row(name: &str) -> Row {
        Row {
            id: Uuid::new_v4(),
            name: name.into(),
            group: "Prod / Web".into(),
            group_chain: vec![],
            address: "10.0.0.1:2222".into(),
            protocol: Protocol::Ssh,
            os: "Debian 12".into(),
            kernel: String::new(),
            uptime_secs: Some(86_400 * 2),
            disk: Some(91),
            updates: Some(3),
            sec_updates: None,
            cpus: Some(4),
            mem_mb: None,
            mem_pct: None,
            load1: None,
            last_connected: None,
            facts_at: None,
            env: Env::Prod,
            tags: vec!["a".into(), "b".into()],
            warnings: vec![Warning {
                kind: WarnKind::Disk,
                level: Level::Warn,
                text: "disk".into(),
            }],
            has_facts: true,
            failed_services: None,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn formula_injection() {
        for bad in ["=1+1", "+cmd", "-2", "@SUM(A1)", "\tx", "\rx"] {
            assert!(neutralize(bad).starts_with('\''), "{bad:?}");
        }
        assert_eq!(neutralize("web-01"), "web-01");
        assert_eq!(neutralize(""), "");
        assert_eq!(
            csv_field("=HYPERLINK(\"http://x\")"),
            "\"'=HYPERLINK(\"\"http://x\"\")\""
        );
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field("plain"), "plain");
    }

    #[test]
    fn csv_table() {
        let mut r = row("=evil()");
        r.tags = vec!["x,y".into()];
        let out = csv(&[r], &[Col::Name, Col::Disk, Col::Tags], now());
        let lines: Vec<&str> = out.split("\r\n").collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], "'=evil(),91 %,\"x,y\"");
        assert!(lines[2].is_empty());
    }

    #[test]
    fn markdown_table() {
        let mut r = row("a|b");
        r.os = "x <script> *bold* `c`".into();
        let out = markdown(&[r], &[Col::Name, Col::Os, Col::Disk], now());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains("---:"));
        assert!(lines[2].starts_with("| a\\|b | x &lt;script&gt; \\*bold\\* \\`c\\` | 91 % |"));
        assert_eq!(md_escape("a\nb"), "a b");
    }

    #[test]
    fn json_export() {
        let out = json(
            &[row("web")],
            &[Col::Name, Col::Disk, Col::Tags, Col::Ram],
            now(),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        let h = &v["hosts"][0];
        assert_eq!(h["name"], "web");
        assert_eq!(h["disk_used_percent"], 91);
        assert_eq!(h["tags"][1], "b");
        assert_eq!(h["warnings"][0], "disk");
        assert!(h["ram"]["total_mb"].is_null());
        // Only the chosen columns, nothing like credentials.
        assert!(h.get("password").is_none() && h.get("username").is_none());
    }
}
