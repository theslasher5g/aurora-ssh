//! Result table of "Run on many hosts": filtering, previews and the exports (Markdown,
//! CSV, JSON and a printable HTML page in the style of the diagnostics report). All
//! remote output is escaped; CSV cells that a spreadsheet would read as a formula are
//! defused.

use serde_json::json;

use super::diag_html::{CSS, output_html, size_label};
use super::inventory_doc::html_escape;
use super::multirun_core::{HostResult, Status};

/// What the report is about.
pub struct Meta {
    /// Command as shown (secret values masked).
    pub command: String,
    pub started: String,
    pub operator: String,
    pub sudo: bool,
    pub powershell: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub ok: usize,
    pub failed: usize,
    pub timeout: usize,
    pub unreachable: usize,
    pub skipped: usize,
    pub pending: usize,
}

pub fn counts(results: &[HostResult]) -> Counts {
    let mut c = Counts {
        total: results.len(),
        ..Default::default()
    };
    for r in results {
        match r.status {
            Status::Ok => c.ok += 1,
            Status::Failed => c.failed += 1,
            Status::Timeout => c.timeout += 1,
            Status::Unreachable => c.unreachable += 1,
            Status::Skipped => c.skipped += 1,
            Status::Pending | Status::Running => c.pending += 1,
        }
    }
    c
}

/// Filter of the result table.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum StatusFilter {
    #[default]
    All,
    /// Everything that did not succeed (failed, timeout, unreachable).
    Problems,
    Only(Status),
}

impl StatusFilter {
    pub fn matches(&self, s: Status) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Problems => s.retryable(),
            StatusFilter::Only(x) => *x == s,
        }
    }
}

/// Indexes of the results that pass the status filter and contain every word of `text`
/// (in name, address, status, error or output).
pub fn filter(results: &[HostResult], status: &StatusFilter, text: &str) -> Vec<usize> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    results
        .iter()
        .enumerate()
        .filter(|(_, r)| status.matches(r.status))
        .filter(|(_, r)| {
            if words.is_empty() {
                return true;
            }
            let hay = format!(
                "{}\n{}\n{}\n{}\n{}\n{}",
                r.name,
                r.address,
                r.status.key(),
                r.status.label(),
                r.error,
                r.output
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .map(|(i, _)| i)
        .collect()
}

/// The first non-empty lines of the output, for the table (`n` lines, `max` characters).
pub fn preview(r: &HostResult, n: usize, max: usize) -> String {
    let mut lines: Vec<&str> = r
        .output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(n)
        .collect();
    if lines.is_empty() && !r.error.is_empty() {
        lines.push(r.error.trim());
    }
    let joined = lines.join(" / ");
    if joined.chars().count() > max {
        let cut: String = joined.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

fn secs(r: &HostResult) -> String {
    format!("{:.1}", r.took.as_secs_f32())
}

// ------------------------------------------------------------------ Markdown

pub fn markdown(meta: &Meta, results: &[HostResult]) -> String {
    let c = counts(results);
    let mut s = format!("# {}\n\n", t!("Mehrfachlauf"));
    s.push_str(&format!(
        "- {}: `{}`\n",
        t!("Befehl"),
        one_line(&meta.command)
    ));
    s.push_str(&format!("- {}: {}\n", t!("Gestartet"), meta.started));
    s.push_str(&format!("- {}: {}\n", t!("Benutzer"), meta.operator));
    s.push_str(&format!(
        "- {}: {} ok, {} {}, {} {}, {} {}, {} {}\n\n",
        t!("Ergebnis"),
        c.ok,
        c.failed,
        t!("fehlgeschlagen"),
        c.timeout,
        t!("Zeitüberschreitung"),
        c.unreachable,
        t!("nicht erreichbar"),
        c.skipped,
        t!("übersprungen")
    ));
    s.push_str(&format!(
        "| {} | {} | {} | {} | {} |\n|---|---|---|---|---|\n",
        t!("Host"),
        t!("Status"),
        "Exit",
        t!("Dauer"),
        t!("Ausgabe")
    ));
    for r in results {
        s.push_str(&format!(
            "| {} | {} | {} | {} s | {} |\n",
            md_cell(&format!("{} ({})", r.name, r.address)),
            r.status.label(),
            r.exit.map_or(String::new(), |e| e.to_string()),
            secs(r),
            md_cell(&preview(r, 2, 100))
        ));
    }
    for r in results.iter().filter(|r| r.status.finished()) {
        s.push_str(&format!("\n## {} ({})\n\n", r.name, r.address));
        s.push_str(&format!("{}: {}", t!("Status"), r.status.label()));
        if let Some(e) = r.exit {
            s.push_str(&format!(", exit {e}"));
        }
        s.push_str(&format!(", {} s\n", secs(r)));
        if !r.error.is_empty() {
            s.push_str(&format!("\n{}: {}\n", t!("Hinweis"), one_line(&r.error)));
        }
        if !r.output.trim().is_empty() {
            let fence = fence_for(&r.output);
            s.push_str(&format!(
                "\n{fence}text\n{}\n{fence}\n",
                r.output.trim_end()
            ));
            if r.truncated {
                s.push_str(&format!("\n_{}_\n", t!("Ausgabe gekürzt")));
            }
        }
    }
    s
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ").replace('`', "'")
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// A code fence longer than any run of backticks in the text.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

// ------------------------------------------------------------------ CSV

fn csv_cell(s: &str) -> String {
    // A spreadsheet would run a cell that starts with one of these.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if s.contains([',', '"', '\n', '\r', ';']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

pub fn csv(results: &[HostResult]) -> String {
    let mut s = String::from("host,address,environment,via,status,exit,seconds,error,output\r\n");
    for r in results {
        let row = [
            csv_cell(&r.name),
            csv_cell(&r.address),
            csv_cell(&r.env),
            csv_cell(r.via),
            csv_cell(r.status.key()),
            r.exit.map_or(String::new(), |e| e.to_string()),
            secs(r),
            csv_cell(&r.error),
            csv_cell(&r.output),
        ];
        s.push_str(&row.join(","));
        s.push_str("\r\n");
    }
    s
}

// ------------------------------------------------------------------ JSON

pub fn json(meta: &Meta, results: &[HostResult]) -> String {
    let c = counts(results);
    let v = json!({
        "command": meta.command,
        "started": meta.started,
        "operator": meta.operator,
        "sudo": meta.sudo,
        "powershell": meta.powershell,
        "summary": {
            "total": c.total, "ok": c.ok, "failed": c.failed, "timeout": c.timeout,
            "unreachable": c.unreachable, "skipped": c.skipped,
        },
        "results": results.iter().map(|r| json!({
            "host": r.name,
            "address": r.address,
            "environment": r.env,
            "via": r.via,
            "status": r.status.key(),
            "exit": r.exit,
            "seconds": (r.took.as_secs_f64() * 10.0).round() / 10.0,
            "error": r.error,
            "truncated": r.truncated,
            "output": r.output,
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

// ------------------------------------------------------------------ HTML

const EXTRA_CSS: &str = r#"
table.res{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line);border-radius:14px;overflow:hidden}
table.res th{font-size:12px;text-transform:uppercase;letter-spacing:.6px;color:var(--dim);text-align:left;padding:10px 12px;border-bottom:1px solid var(--line)}
table.res td{padding:8px 12px;border-bottom:1px solid var(--line);vertical-align:top;font-size:14px}
table.res tr:last-child td{border-bottom:0}
table.res a{color:inherit;text-decoration:none;font-weight:600}
table.res .mono{font-family:ui-monospace,Consolas,Menlo,monospace;font-size:13px;overflow-wrap:anywhere}
.pill.timeout,.pill.unreachable{color:var(--warn)}
.env{font-size:11px;font-weight:700;color:var(--bad);margin-left:6px}
@media print{table.res{break-inside:auto}tr{break-inside:avoid}}
"#;

fn anchor(i: usize) -> String {
    format!("h-{i}")
}

fn pill_class(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Failed => "failed",
        Status::Timeout => "timeout",
        Status::Unreachable => "unreachable",
        _ => "skipped",
    }
}

/// The whole HTML document (self-contained, no scripts).
pub fn html(meta: &Meta, results: &[HostResult]) -> String {
    let e = |s: &str| html_escape(s);
    let c = counts(results);
    let mut h = String::new();
    h.push_str("<!doctype html>\n<html lang=\"");
    h.push_str(&e(t!("de")));
    h.push_str("\"><head><meta charset=\"utf-8\">");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    h.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">");
    h.push_str("<meta name=\"color-scheme\" content=\"light dark\">");
    h.push_str(&format!(
        "<title>{}</title><style>{CSS}{EXTRA_CSS}</style></head><body><div class=\"wrap\">",
        e(t!("Mehrfachlauf"))
    ));
    h.push_str(&format!(
        "<header><h1>{}</h1><p class=\"sub\">{}</p><dl class=\"meta\">",
        e(t!("Mehrfachlauf")),
        e(t!("Aurora – Befehl auf mehreren Hosts"))
    ));
    for (k, v) in [
        (t!("Befehl"), meta.command.as_str()),
        (t!("Gestartet"), meta.started.as_str()),
        (t!("Benutzer"), meta.operator.as_str()),
        (
            t!("Mit sudo"),
            if meta.sudo { t!("ja") } else { t!("nein") },
        ),
        (
            "Shell",
            if meta.powershell {
                "PowerShell"
            } else {
                "POSIX sh"
            },
        ),
    ] {
        h.push_str(&format!("<div><dt>{}</dt><dd>{}</dd></div>", e(k), e(v)));
    }
    h.push_str("</dl></header><div class=\"tiles\">");
    let tile = |label: &str, value: usize, level: u8| {
        format!(
            "<div class=\"tile l{level}\"><b>{value}</b><span>{}</span></div>",
            html_escape(label)
        )
    };
    h.push_str(&tile(t!("Hosts"), c.total, 0));
    h.push_str(&tile(t!("ok"), c.ok, 0));
    h.push_str(&tile(
        t!("fehlgeschlagen"),
        c.failed,
        u8::from(c.failed > 0) * 2,
    ));
    h.push_str(&tile(
        t!("Zeitüberschreitung"),
        c.timeout,
        u8::from(c.timeout > 0),
    ));
    h.push_str(&tile(
        t!("nicht erreichbar"),
        c.unreachable,
        u8::from(c.unreachable > 0),
    ));
    h.push_str(&tile(t!("übersprungen"), c.skipped, 0));
    h.push_str("</div>");
    // Table.
    h.push_str(&format!(
        "<h2>{}</h2><table class=\"res\"><thead><tr><th>{}</th><th>{}</th><th>Exit</th><th>{}</th><th>{}</th></tr></thead><tbody>",
        e(t!("Ergebnisse")),
        e(t!("Host")),
        e(t!("Status")),
        e(t!("Dauer")),
        e(t!("Ausgabe"))
    ));
    for (i, r) in results.iter().enumerate() {
        h.push_str(&format!(
            "<tr><td><a href=\"#{}\">{}</a>{}<br><span class=\"dim mono\">{}</span></td><td><span class=\"pill {}\">{}</span></td><td>{}</td><td>{} s</td><td class=\"mono\">{}</td></tr>",
            anchor(i),
            e(&r.name),
            if r.env == "PROD" {
                "<span class=\"env\">PROD</span>"
            } else {
                ""
            },
            e(&r.address),
            pill_class(r.status),
            e(r.status.label()),
            r.exit.map_or(String::new(), |x| x.to_string()),
            secs(r),
            e(&preview(r, 2, 120))
        ));
    }
    h.push_str("</tbody></table>");
    // Sections.
    h.push_str(&format!("<h2>{}</h2>", e(t!("Ausgaben"))));
    for (i, r) in results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status.finished())
    {
        let lines = r.output.lines().count();
        let open = r.status.retryable() || lines <= 25;
        h.push_str(&format!(
            "<section class=\"item\" id=\"{}\"><details{}><summary><span class=\"name\">{}</span><span class=\"grow\"></span><span class=\"dim\">{} · {} s</span><span class=\"pill {}\">{}</span></summary>",
            anchor(i),
            if open { " open" } else { "" },
            e(&r.name),
            e(&size_label(r.output.len())),
            secs(r),
            pill_class(r.status),
            e(r.status.label())
        ));
        if !r.error.is_empty() {
            h.push_str(&format!("<p class=\"err\">{}</p>", e(&r.error)));
        }
        if r.output.trim().is_empty() {
            h.push_str(&format!(
                "<p class=\"dim\" style=\"margin:0 18px 14px\">{}</p>",
                e(t!("(keine Ausgabe)"))
            ));
        } else {
            h.push_str(&format!("<pre>{}</pre>", output_html(&r.output)));
            if r.truncated {
                h.push_str(&format!(
                    "<p class=\"dim\" style=\"margin:0 18px 14px\">{}</p>",
                    e(t!("Ausgabe gekürzt"))
                ));
            }
        }
        h.push_str("</details></section>");
    }
    h.push_str(&format!(
        "<footer>{}</footer></div></body></html>\n",
        e(t!("Erstellt mit Aurora"))
    ));
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn res(name: &str, status: Status, out: &str) -> HostResult {
        HostResult {
            name: name.into(),
            address: format!("{name}.lan"),
            env: String::new(),
            status,
            exit: Some(if status == Status::Ok { 0 } else { 1 }),
            took: Duration::from_millis(1500),
            output: out.into(),
            truncated: false,
            error: String::new(),
            via: "ssh",
        }
    }

    fn meta() -> Meta {
        Meta {
            command: "uptime".into(),
            started: "2026-10-23 10:00".into(),
            operator: "me".into(),
            sudo: false,
            powershell: false,
        }
    }

    fn sample() -> Vec<HostResult> {
        let mut bad = res("db1", Status::Failed, "boom\nsecond");
        bad.error = "exit 1".into();
        let mut off = res("old", Status::Unreachable, "");
        off.exit = None;
        off.error = "connection refused".into();
        vec![res("web1", Status::Ok, "up 3 days\n"), bad, off]
    }

    #[test]
    fn counting_and_filtering() {
        let r = sample();
        let c = counts(&r);
        assert_eq!((c.total, c.ok, c.failed, c.unreachable), (3, 1, 1, 1));
        assert_eq!(filter(&r, &StatusFilter::All, ""), vec![0, 1, 2]);
        assert_eq!(filter(&r, &StatusFilter::Problems, ""), vec![1, 2]);
        assert_eq!(filter(&r, &StatusFilter::Only(Status::Ok), ""), vec![0]);
        assert_eq!(filter(&r, &StatusFilter::All, "BOOM"), vec![1]);
        assert_eq!(filter(&r, &StatusFilter::All, "refused old"), vec![2]);
        assert_eq!(
            filter(&r, &StatusFilter::Problems, "web"),
            Vec::<usize>::new()
        );
        assert_eq!(filter(&r, &StatusFilter::All, "web1.lan"), vec![0]);
    }

    #[test]
    fn previews() {
        let r = sample();
        assert_eq!(preview(&r[1], 2, 100), "boom / second");
        assert_eq!(preview(&r[1], 1, 100), "boom");
        assert_eq!(preview(&r[2], 2, 100), "connection refused");
        let long = res("x", Status::Ok, &"a".repeat(300));
        assert_eq!(preview(&long, 1, 20).chars().count(), 20);
        assert!(preview(&long, 1, 20).ends_with('…'));
    }

    #[test]
    fn markdown_export() {
        let md = markdown(&meta(), &sample());
        assert!(md.starts_with("# "));
        assert!(md.contains("`uptime`"));
        assert!(md.contains("| web1 (web1.lan) |"));
        assert!(md.contains("```text\nup 3 days\n```"));
        // Output with backticks gets a longer fence.
        let mut r = sample();
        r[0].output = "```\nx\n```".into();
        let md = markdown(&meta(), &r);
        assert!(md.contains("````text"), "{md}");
        // Pipes in a table cell are escaped.
        r[0].output = "a|b".into();
        assert!(markdown(&meta(), &r).contains("a\\|b"));
    }

    #[test]
    fn csv_export_defuses_formulas() {
        let mut r = sample();
        r[0].output = "=cmd|' /C calc'!A0".into();
        r[1].output = "line1\nline \"2\", x".into();
        let c = csv(&r);
        assert!(c.starts_with("host,address,environment,via,status,exit,seconds,error,output\r\n"));
        assert!(c.contains("'=cmd|' /C calc'!A0"));
        assert!(c.contains("\"line1\nline \"\"2\"\", x\""));
        assert!(c.contains("web1,web1.lan,,ssh,ok,0,1.5,,"));
        assert_eq!(c.matches("\r\n").count(), 4);
    }

    #[test]
    fn json_export_round_trips() {
        let j = json(&meta(), &sample());
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["summary"]["failed"], 1);
        assert_eq!(v["results"][1]["status"], "failed");
        assert_eq!(v["results"][1]["output"], "boom\nsecond");
        assert_eq!(v["results"][2]["exit"], serde_json::Value::Null);
        assert_eq!(v["command"], "uptime");
    }

    #[test]
    fn html_export_escapes_everything() {
        let mut r = sample();
        r[0].name = "<b>web</b>".into();
        r[0].output = "<script>alert(1)</script>".into();
        r[2].error = "<img src=x onerror=1>".into();
        let mut m = meta();
        m.command = "echo '<i>'".into();
        let h = html(&m, &r);
        assert!(!h.contains("<script>") && !h.contains("<img src=x") && !h.contains("<b>web"));
        assert!(h.contains("&lt;script&gt;"));
        assert!(h.contains("&lt;i&gt;"));
        assert!(h.contains("Content-Security-Policy"));
        assert!(h.contains("<table class=\"res\">"));
        // Failed hosts are expanded, long outputs of good hosts are not.
        assert!(h.contains("id=\"h-1\"><details open>"));
        let mut long = sample();
        long[0].output = "x\n".repeat(60);
        assert!(html(&meta(), &long).contains("id=\"h-0\"><details>"));
    }
}
