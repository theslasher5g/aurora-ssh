//! Export of a recording as Markdown or plain text: title, host, date, and the output split
//! at the markers (each marker becomes a heading, the output after it a code block).

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::backend::record;
use crate::ui::sessionlog_index::render_lines;
use crate::ui::sessionlog_markers::{self, Marker};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Markdown,
    Text,
}

impl Kind {
    pub fn extension(self) -> &'static str {
        match self {
            Kind::Markdown => "md",
            Kind::Text => "txt",
        }
    }
}

/// `2:31` or `1:02:05`.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub struct Doc<'a> {
    pub title: &'a str,
    pub host: &'a str,
    /// Formatted start date.
    pub date: &'a str,
    pub duration: f64,
    pub lines: &'a [(f64, String)],
    pub markers: &'a [Marker],
}

/// Fence for a code block that does not occur in the text.
fn fence(text: &str) -> String {
    let mut n = 3;
    while text.contains(&"`".repeat(n)) {
        n += 1;
    }
    "`".repeat(n)
}

fn block(lines: &[&(f64, String)]) -> String {
    lines
        .iter()
        .map(|l| l.1.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render(doc: &Doc, kind: Kind) -> String {
    let mut out = String::new();
    match kind {
        Kind::Markdown => {
            out.push_str(&format!("# {}\n\n", doc.title));
            out.push_str(&format!("- {}: {}\n", t!("Host"), doc.host));
            out.push_str(&format!("- {}: {}\n", t!("Datum"), doc.date));
            out.push_str(&format!("- {}: {}\n\n", t!("Dauer"), clock(doc.duration)));
        }
        Kind::Text => {
            out.push_str(&format!(
                "{}\n{}\n",
                doc.title,
                "=".repeat(doc.title.chars().count().max(3))
            ));
            out.push_str(&format!("{}: {}\n", t!("Host"), doc.host));
            out.push_str(&format!("{}: {}\n", t!("Datum"), doc.date));
            out.push_str(&format!("{}: {}\n\n", t!("Dauer"), clock(doc.duration)));
        }
    }
    // Sections: before the first marker, then one per marker.
    let mut sections: Vec<(Option<&Marker>, f64, f64)> = Vec::new();
    let first = doc.markers.first().map_or(f64::INFINITY, |m| m.t);
    sections.push((None, f64::NEG_INFINITY, first));
    for (i, m) in doc.markers.iter().enumerate() {
        let end = doc.markers.get(i + 1).map_or(f64::INFINITY, |n| n.t);
        sections.push((Some(m), m.t, end));
    }
    for (marker, from, to) in sections {
        let lines: Vec<&(f64, String)> = doc
            .lines
            .iter()
            .filter(|l| l.0 >= from && l.0 < to)
            .collect();
        if marker.is_none() && lines.is_empty() {
            continue;
        }
        let body = block(&lines);
        let head = match marker {
            Some(m) => format!("[{}] {}", clock(m.t), m.label),
            None if doc.markers.is_empty() => t!("Ausgabe").to_string(),
            None => t!("Beginn").to_string(),
        };
        match kind {
            Kind::Markdown => {
                out.push_str(&format!("## {head}\n\n"));
                if !body.is_empty() {
                    let f = fence(&body);
                    out.push_str(&format!("{f}text\n{body}\n{f}\n\n"));
                }
            }
            Kind::Text => {
                out.push_str(&format!("--- {head} ---\n"));
                if !body.is_empty() {
                    out.push_str(&body);
                    out.push('\n');
                }
                out.push('\n');
            }
        }
    }
    out
}

/// Writes `<recording>.md` / `.txt` next to the recording; returns its path.
pub fn export(cast_path: &Path, kind: Kind) -> Result<PathBuf> {
    let cast = record::read_full(cast_path)?;
    let lines = render_lines(&cast);
    let markers = sessionlog_markers::load(cast_path, &cast.markers);
    let date = chrono::DateTime::from_timestamp(cast.timestamp, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d.%m.%Y %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let title = if cast.title.is_empty() {
        cast_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        cast.title.clone()
    };
    let host = title.split(" · ").next().unwrap_or_default().to_string();
    let text = render(
        &Doc {
            title: &title,
            host: &host,
            date: &date,
            duration: cast.frames.last().map_or(0.0, |f| f.time),
            lines: &lines,
            markers: &markers,
        },
        kind,
    );
    let out = cast_path.with_extension(kind.extension());
    if out == cast_path {
        return Err(anyhow!("{}", t!("Ungültiger Dateiname")));
    }
    std::fs::write(&out, text)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_become_headings_with_code_blocks() {
        let lines = vec![
            (1.0, "$ uptime".to_string()),
            (5.0, "up 3 days".to_string()),
            (70.0, "```nested```".to_string()),
        ];
        let markers = vec![
            Marker {
                t: 4.0,
                label: "Check".into(),
            },
            Marker {
                t: 65.0,
                label: "Fix".into(),
            },
        ];
        let doc = Doc {
            title: "web-01 · root",
            host: "web-01",
            date: "01.01.2026 10:00",
            duration: 75.0,
            lines: &lines,
            markers: &markers,
        };
        let md = render(&doc, Kind::Markdown);
        assert!(md.starts_with("# web-01 · root\n"));
        assert!(
            md.contains("## [0:04] Check\n\n```text\nup 3 days\n```"),
            "{md}"
        );
        // Text containing backticks gets a longer fence.
        assert!(
            md.contains("## [1:05] Fix\n\n````text\n```nested```\n````"),
            "{md}"
        );
        // Output before the first marker comes first.
        assert!(md.find("$ uptime").unwrap() < md.find("[0:04]").unwrap());
        let txt = render(&doc, Kind::Text);
        assert!(txt.contains("--- [0:04] Check ---\nup 3 days\n"));
        assert!(!txt.contains("```text"));
        // Without markers there is one block.
        let none = render(
            &Doc {
                markers: &[],
                ..doc
            },
            Kind::Markdown,
        );
        assert_eq!(none.matches("\n## ").count(), 1);
    }
}
