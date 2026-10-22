//! `report.html` of a diagnostics bundle: the same data as the text files, laid out for
//! reading and printing – header, key facts, a table of contents with status pills and one
//! collapsible section per command. Self-contained (inline CSS, no scripts, no external
//! files); every piece of remote output is escaped.

use super::inventory_doc::html_escape;

/// One collected command.
#[derive(Clone, Debug, Default)]
pub struct HtmlItem {
    pub group: String,
    pub file: String,
    pub command: String,
    /// `None` = was not run.
    pub status: Option<u32>,
    pub error: Option<String>,
    pub secs: f32,
    /// Output as written to the bundle (already redacted when asked for).
    pub output: String,
}

#[derive(Clone, Debug, Default)]
pub struct HtmlReport {
    pub name: String,
    pub address: String,
    pub hostname: String,
    pub user: String,
    pub created: String,
    pub sudo: bool,
    pub redacted: bool,
    pub items: Vec<HtmlItem>,
}

impl HtmlItem {
    fn state(&self) -> State {
        match (&self.status, &self.error) {
            (None, _) => State::Skipped,
            (_, Some(_)) => State::Failed,
            (Some(0), None) => State::Ok,
            (Some(_), None) => State::Failed,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ok,
    Failed,
    Skipped,
}

/// A fact shown on top ("Disk /var 91 %").
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub label: String,
    pub value: String,
    /// Small second line (mount point, sizes …).
    pub detail: String,
    /// 0 fine, 1 warning, 2 critical.
    pub level: u8,
}

/// Anchor id of an item (`file` without extension, letters/digits/dashes only).
fn anchor(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(a, _)| a);
    let mut s: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if s.is_empty() {
        s.push('x');
    }
    format!("i-{s}")
}

/// Highest "Use%" of real file systems in `df -h` output.
fn disk_fact(out: &str) -> Option<Fact> {
    let mut best: Option<(u32, String)> = None;
    for l in out.lines().skip(1) {
        let cols: Vec<&str> = l.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        if matches!(cols[0], "tmpfs" | "devtmpfs" | "udev" | "overlay" | "shm") {
            continue;
        }
        let Some(p) = cols
            .iter()
            .find_map(|c| c.strip_suffix('%')?.parse::<u32>().ok())
        else {
            continue;
        };
        let mount = cols.last().copied().unwrap_or("/").to_string();
        if best.as_ref().is_none_or(|(b, _)| p > *b) {
            best = Some((p, mount));
        }
    }
    best.map(|(p, m)| Fact {
        label: t!("Fullste Platte").to_string(),
        value: format!("{p} %"),
        detail: m,
        level: if p >= 95 {
            2
        } else if p >= 85 {
            1
        } else {
            0
        },
    })
}

/// Memory use from `free -m`.
fn memory_fact(out: &str) -> Option<Fact> {
    let l = out.lines().find(|l| l.starts_with("Mem:"))?;
    let n: Vec<u64> = l
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    let (total, avail) = (
        *n.first()?,
        n.get(5).copied().or_else(|| n.get(2).copied())?,
    );
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(avail) * 100 / total;
    Some(Fact {
        label: t!("Arbeitsspeicher").to_string(),
        value: format!("{used} %"),
        detail: format!("{} / {} MB", total.saturating_sub(avail), total),
        level: if used >= 95 {
            2
        } else if used >= 85 {
            1
        } else {
            0
        },
    })
}

/// Failed units from `systemctl --failed` output.
fn failed_fact(out: &str) -> Option<Fact> {
    let n = out
        .lines()
        .filter(|l| {
            (l.contains(".service") || l.contains(".timer") || l.contains(".mount"))
                && l.contains("failed")
        })
        .count();
    let none = out.contains("0 loaded units") || n == 0;
    Some(Fact {
        label: t!("Fehlgeschlagene Dienste").to_string(),
        value: if none { "0".into() } else { n.to_string() },
        detail: String::new(),
        level: if none { 0 } else { 2 },
    })
}

/// `up 41 days, 3:12` out of `uptime`.
fn uptime_fact(out: &str) -> Option<Fact> {
    let l = out.lines().next()?;
    let i = l.find("up ")?;
    let rest = &l[i + 3..];
    let end = rest
        .find(" user")
        .map_or(rest.len(), |u| rest[..u].rfind(',').unwrap_or(u));
    let text = rest[..end].trim().trim_end_matches(',').to_string();
    let (value, detail) = match text.split_once(',') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (text, String::new()),
    };
    Some(Fact {
        label: t!("Laufzeit").to_string(),
        value,
        detail,
        level: 0,
    })
}

/// The key facts that can be derived from the collected output.
pub fn facts(items: &[HtmlItem]) -> Vec<Fact> {
    let get = |file: &str| {
        items
            .iter()
            .find(|i| i.file == file && i.status == Some(0) && i.error.is_none())
            .map(|i| i.output.as_str())
    };
    let mut out = Vec::new();
    if let Some(f) = get("uptime.txt").and_then(uptime_fact) {
        out.push(f);
    }
    if let Some(f) = get("df.txt").and_then(disk_fact) {
        out.push(f);
    }
    if let Some(f) = get("free.txt").and_then(memory_fact) {
        out.push(f);
    }
    if let Some(f) = get("systemctl-failed.txt").and_then(failed_fact) {
        out.push(f);
    }
    out
}

/// One output line, escaped, with a class for lines that look like trouble.
fn line_html(l: &str) -> String {
    let low = l.to_lowercase();
    let class = if low.contains("error")
        || low.contains("fail")
        || low.contains("critical")
        || low.contains("panic")
        || low.contains("denied")
    {
        Some("bad")
    } else if low.contains("warn") || low.contains("timeout") || low.contains("deprecated") {
        Some("warn")
    } else {
        None
    };
    match class {
        Some(c) => format!("<span class=\"{c}\">{}</span>", html_escape(l)),
        None => html_escape(l),
    }
}

/// Output as `<pre>` content; very long output keeps its head and tail.
fn output_html(out: &str) -> String {
    const MAX: usize = 1500;
    let lines: Vec<&str> = out.lines().collect();
    let mut s = String::new();
    if lines.len() <= MAX {
        for l in &lines {
            s.push_str(&line_html(l));
            s.push('\n');
        }
    } else {
        for l in &lines[..MAX / 2] {
            s.push_str(&line_html(l));
            s.push('\n');
        }
        s.push_str(&format!(
            "<span class=\"cut\">… {} …</span>\n",
            html_escape(&t!(
                "{} Zeilen ausgelassen – vollständig in der Textdatei",
                lines.len() - MAX
            ))
        ));
        for l in &lines[lines.len() - MAX / 2..] {
            s.push_str(&line_html(l));
            s.push('\n');
        }
    }
    s
}

fn size_label(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

const CSS: &str = r#"
:root{--bg:#f6f7f9;--card:#fff;--ink:#1c1c1e;--dim:#6b6b73;--line:#e3e4e8;--accent:#e08600;--ok:#1f9d3a;--warn:#c27a00;--bad:#d63a32;--code:#f0f1f4}
@media (prefers-color-scheme:dark){:root{--bg:#0e0e10;--card:#1a1a1d;--ink:#f5f5f7;--dim:#a1a1a6;--line:#2c2c2e;--accent:#ff9f0a;--ok:#32d74b;--warn:#ffb340;--bad:#ff6961;--code:#111113}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.55 -apple-system,"Segoe UI",Inter,Helvetica,Arial,sans-serif}
.wrap{max-width:1040px;margin:0 auto;padding:28px 20px 60px}
header{background:var(--card);border:1px solid var(--line);border-radius:16px;padding:22px 26px;position:relative;overflow:hidden}
header:before{content:"";position:absolute;inset:0 0 auto 0;height:4px;background:linear-gradient(90deg,#ffb340,#ff9f0a,#ff6fa8)}
h1{margin:0 0 4px;font-size:26px;letter-spacing:-.4px}
.sub{color:var(--dim);margin:0 0 14px}
.meta{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:10px 24px;margin:0}
.meta div{min-width:0}.meta dt{font-size:12px;text-transform:uppercase;letter-spacing:.6px;color:var(--dim)}
.meta dd{margin:0;font-family:ui-monospace,Consolas,Menlo,monospace;font-size:14px;overflow-wrap:anywhere}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px;margin:18px 0}
.tile{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:14px 16px}
.tile b{display:block;font-size:24px;letter-spacing:-.4px;white-space:nowrap}.tile span{color:var(--dim);font-size:13px;display:block}.tile small{display:block;color:var(--dim);font-size:12px;font-family:ui-monospace,Consolas,Menlo,monospace;overflow-wrap:anywhere}
.tile.l1 b{color:var(--warn)}.tile.l2 b{color:var(--bad)}
h2{font-size:18px;margin:30px 0 10px}
.toc{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:6px 6px}
.toc h3{margin:12px 14px 4px;font-size:12px;text-transform:uppercase;letter-spacing:.7px;color:var(--dim)}
.toc a{display:flex;gap:10px;align-items:center;padding:6px 14px;border-radius:8px;color:inherit;text-decoration:none}
.toc a:hover{background:var(--code)}.toc .f{flex:1;font-family:ui-monospace,Consolas,Menlo,monospace;font-size:13.5px}
.toc .t{color:var(--dim);font-size:12.5px}
.pill{font-size:11.5px;font-weight:600;padding:2px 9px;border-radius:999px;border:1px solid currentColor}
.pill.ok{color:var(--ok)}.pill.failed{color:var(--bad)}.pill.skipped{color:var(--dim)}
section.item{background:var(--card);border:1px solid var(--line);border-radius:14px;margin:12px 0;overflow:hidden}
section.item>details>summary{list-style:none;cursor:pointer;display:flex;gap:10px;align-items:center;padding:13px 18px}
section.item>details>summary::-webkit-details-marker{display:none}
section.item>details>summary:before{content:"▸";color:var(--dim);transition:transform .15s}
section.item>details[open]>summary:before{transform:rotate(90deg)}
.name{font-weight:600;font-family:ui-monospace,Consolas,Menlo,monospace;font-size:14.5px}
.grow{flex:1}.dim{color:var(--dim);font-size:12.5px}
.cmd{margin:0 18px 10px;padding:8px 12px;background:var(--code);border-radius:8px;font:13px ui-monospace,Consolas,Menlo,monospace;overflow-wrap:anywhere}
.cmd:before{content:"$ ";color:var(--accent)}
pre{margin:0;padding:12px 18px 16px;background:var(--code);font:12.5px/1.5 ui-monospace,Consolas,Menlo,monospace;overflow:auto;max-height:560px;white-space:pre}
pre .bad{color:var(--bad);font-weight:600}pre .warn{color:var(--warn)}pre .cut{color:var(--dim);font-style:italic}
.err{margin:0 18px 10px;color:var(--bad)}
footer{margin-top:30px;color:var(--dim);font-size:12.5px;text-align:center}
@media print{body{background:#fff;color:#000}details{display:block}details>*{display:block!important}pre{max-height:none;white-space:pre-wrap}section.item{break-inside:avoid-page}.toc a:hover{background:none}}
"#;

/// The complete HTML document.
pub fn render(r: &HtmlReport) -> String {
    let e = |s: &str| html_escape(s);
    let (mut ok, mut failed, mut skipped, mut bytes) = (0, 0, 0, 0usize);
    for i in &r.items {
        match i.state() {
            State::Ok => ok += 1,
            State::Failed => failed += 1,
            State::Skipped => skipped += 1,
        }
        bytes += i.output.len();
    }
    let facts = facts(&r.items);
    let mut h = String::new();
    h.push_str("<!doctype html>\n<html lang=\"");
    h.push_str(&e(t!("de")));
    h.push_str("\"><head><meta charset=\"utf-8\">");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    h.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">");
    h.push_str("<meta name=\"color-scheme\" content=\"light dark\">");
    h.push_str(&format!(
        "<title>{} – {}</title><style>{CSS}</style></head><body><div class=\"wrap\">",
        e(t!("Diagnose")),
        e(&r.name)
    ));
    // Header.
    h.push_str(&format!(
        "<header><h1>{}</h1><p class=\"sub\">{}</p><dl class=\"meta\">",
        e(&r.name),
        e(t!("Aurora Diagnose-Bericht"))
    ));
    for (k, v) in [
        (t!("Adresse"), r.address.as_str()),
        (t!("Hostname"), r.hostname.as_str()),
        (t!("Benutzer"), r.user.as_str()),
        (t!("Erstellt"), r.created.as_str()),
        (t!("Mit sudo"), if r.sudo { t!("ja") } else { t!("nein") }),
        (
            t!("Geschwärzt"),
            if r.redacted { t!("ja") } else { t!("nein") },
        ),
    ] {
        if !v.is_empty() {
            h.push_str(&format!("<div><dt>{}</dt><dd>{}</dd></div>", e(k), e(v)));
        }
    }
    h.push_str("</dl></header>");
    // Tiles.
    h.push_str("<div class=\"tiles\">");
    let tile = |label: &str, value: &str, detail: &str, level: u8| {
        format!(
            "<div class=\"tile l{level}\"><b>{}</b><span>{}</span><small>{}</small></div>",
            html_escape(value),
            html_escape(label),
            html_escape(detail)
        )
    };
    for f in &facts {
        h.push_str(&tile(&f.label, &f.value, &f.detail, f.level));
    }
    h.push_str(&tile(
        t!("Befehle erfolgreich"),
        &format!("{ok} / {}", r.items.len()),
        "",
        0,
    ));
    if failed > 0 {
        h.push_str(&tile(t!("Mit Fehler"), &failed.to_string(), "", 2));
    }
    if skipped > 0 {
        h.push_str(&tile(t!("Nicht gelaufen"), &skipped.to_string(), "", 1));
    }
    h.push_str(&tile(t!("Datenmenge"), &size_label(bytes), "", 0));
    h.push_str("</div>");
    // Table of contents grouped.
    h.push_str(&format!("<h2>{}</h2><nav class=\"toc\">", e(t!("Inhalt"))));
    let mut groups: Vec<&str> = Vec::new();
    for i in &r.items {
        if !groups.contains(&i.group.as_str()) {
            groups.push(i.group.as_str());
        }
    }
    for g in &groups {
        h.push_str(&format!("<h3>{}</h3>", e(g)));
        for i in r.items.iter().filter(|i| i.group == *g) {
            let (cls, label) = match i.state() {
                State::Ok => ("ok", t!("ok")),
                State::Failed => ("failed", t!("Fehler")),
                State::Skipped => ("skipped", t!("nicht gelaufen")),
            };
            h.push_str(&format!(
                "<a href=\"#{}\"><span class=\"f\">{}</span><span class=\"t\">{} · {}</span><span class=\"pill {cls}\">{}</span></a>",
                anchor(&i.file),
                e(&i.file),
                e(&size_label(i.output.len())),
                e(&format!("{:.1} s", i.secs)),
                e(label)
            ));
        }
    }
    h.push_str("</nav>");
    // Sections.
    h.push_str(&format!("<h2>{}</h2>", e(t!("Ausgaben"))));
    for i in &r.items {
        let (cls, label) = match i.state() {
            State::Ok => ("ok", "exit 0".to_string()),
            State::Failed => (
                "failed",
                match i.status {
                    Some(s) if i.error.is_none() => format!("exit {s}"),
                    _ => t!("Fehler").to_string(),
                },
            ),
            State::Skipped => ("skipped", t!("nicht gelaufen").to_string()),
        };
        // Failed items and short outputs start open, long ones closed.
        let lines = i.output.lines().count();
        let open = i.state() == State::Failed || (i.state() == State::Ok && lines <= 40);
        h.push_str(&format!(
            "<section class=\"item\" id=\"{}\"><details{}><summary><span class=\"name\">{}</span><span class=\"grow\"></span><span class=\"dim\">{} · {}</span><span class=\"pill {cls}\">{}</span></summary>",
            anchor(&i.file),
            if open { " open" } else { "" },
            e(&i.file),
            e(&size_label(i.output.len())),
            e(&format!("{:.1} s", i.secs)),
            e(&label)
        ));
        h.push_str(&format!("<div class=\"cmd\">{}</div>", e(&i.command)));
        if let Some(err) = &i.error {
            h.push_str(&format!("<p class=\"err\">{}</p>", e(err)));
        }
        if i.status.is_some() {
            if i.output.trim().is_empty() {
                h.push_str(&format!(
                    "<p class=\"dim\" style=\"margin:0 18px 14px\">{}</p>",
                    e(t!("(keine Ausgabe)"))
                ));
            } else {
                h.push_str(&format!("<pre>{}</pre>", output_html(&i.output)));
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

    fn item(file: &str, group: &str, out: &str, status: Option<u32>) -> HtmlItem {
        HtmlItem {
            group: group.into(),
            file: file.into(),
            command: "cmd".into(),
            status,
            output: out.into(),
            ..Default::default()
        }
    }

    #[test]
    fn remote_output_cannot_become_markup() {
        let r = HtmlReport {
            name: "<b>host</b>".into(),
            items: vec![item(
                "evil.txt",
                "System",
                "<script>alert(1)</script>\n</pre><img src=x onerror=1>",
                Some(0),
            )],
            ..Default::default()
        };
        let h = render(&r);
        assert!(!h.contains("<script>") && !h.contains("<img src=x"));
        assert!(h.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(h.contains("&lt;b&gt;host&lt;/b&gt;"));
        assert!(h.contains("Content-Security-Policy"));
    }

    #[test]
    fn facts_from_real_output() {
        let df = "Filesystem Size Used Avail Use% Mounted on\n/dev/sda1 50G 46G 4G 92% /\ntmpfs 2G 0 2G 99% /run\n";
        let f = disk_fact(df).unwrap();
        assert_eq!(
            (f.value.as_str(), f.detail.as_str(), f.level),
            ("92 %", "/", 1)
        );
        let free = "              total used free shared buff/cache available\nMem:           1000 400 100 0 500 600\n";
        let m = memory_fact(free).unwrap();
        assert_eq!(
            (m.value.as_str(), m.detail.as_str()),
            ("40 %", "400 / 1000 MB")
        );
        assert_eq!(failed_fact("  0 loaded units listed.\n").unwrap().level, 0);
        let failed = "  UNIT LOAD ACTIVE SUB DESCRIPTION\n● nginx.service loaded failed failed Web\n\n1 loaded units listed.\n";
        assert_eq!(failed_fact(failed).unwrap().value, "1");
        assert_eq!(
            uptime_fact(" 10:02:11 up 41 days,  3:12,  2 users,  load average: 0.1, 0.2, 0.3")
                .unwrap()
                .value,
            "41 days"
        );
    }

    #[test]
    fn groups_status_and_trouble_lines() {
        let r = HtmlReport {
            name: "web".into(),
            items: vec![
                item("a.txt", "System", "fine\nERROR: disk\nwarning x", Some(0)),
                item("b.txt", "Logs", "", Some(1)),
                item("c.txt", "Logs", "", None),
            ],
            ..Default::default()
        };
        let h = render(&r);
        assert!(h.contains("<span class=\"bad\">ERROR: disk</span>"));
        assert!(h.contains("<span class=\"warn\">warning x</span>"));
        assert!(h.contains("href=\"#i-a\"") && h.contains("id=\"i-b\""));
        assert!(h.contains("pill failed") && h.contains("pill skipped"));
    }

    #[test]
    fn long_output_keeps_head_and_tail() {
        let big: String = (0..4000).map(|i| format!("line {i}\n")).collect();
        let r = HtmlReport {
            items: vec![item("big.txt", "System", &big, Some(0))],
            ..Default::default()
        };
        let h = render(&r);
        assert!(h.contains("line 0\n") && h.contains("line 3999"));
        assert!(!h.contains("line 2000\n"));
    }

    /// Writes a sample report for a visual check: `AURORA_DUMP=/tmp/x.html cargo test dump_sample -- --ignored`.
    #[test]
    #[ignore]
    fn dump_sample() {
        let Ok(path) = std::env::var("AURORA_DUMP") else {
            return;
        };
        let r = HtmlReport {
            name: "web-01 (Kunde A)".into(),
            address: "10.20.0.12:22".into(),
            hostname: "web-01".into(),
            user: "ops".into(),
            created: "2026-10-22 10:41:03 +0200".into(),
            sudo: true,
            redacted: false,
            items: vec![
                HtmlItem { group: "System".into(), file: "uptime.txt".into(), command: "uptime".into(), status: Some(0), secs: 0.2, output: " 10:41:03 up 41 days,  3:12,  2 users,  load average: 0.10, 0.20, 0.30\n".into(), ..Default::default() },
                HtmlItem { group: "System".into(), file: "df.txt".into(), command: "df -h".into(), status: Some(0), secs: 0.3, output: "Filesystem      Size  Used Avail Use% Mounted on\n/dev/sda1        50G   46G  4.0G  92% /\ntmpfs           2.0G     0  2.0G   0% /dev/shm\n".into(), ..Default::default() },
                HtmlItem { group: "System".into(), file: "free.txt".into(), command: "free -m".into(), status: Some(0), secs: 0.1, output: "               total        used        free      shared  buff/cache   available\nMem:            3900        1200         300          10        2400        2500\n".into(), ..Default::default() },
                HtmlItem { group: "Dienste".into(), file: "systemctl-failed.txt".into(), command: "systemctl --failed".into(), status: Some(0), secs: 0.4, output: "  UNIT          LOAD   ACTIVE SUB    DESCRIPTION\n● certbot.service loaded failed failed Certbot\n\n1 loaded units listed.\n".into(), ..Default::default() },
                HtmlItem { group: "Logs".into(), file: "journal-warnings.txt".into(), command: "journalctl -p warning -n 200".into(), status: Some(0), secs: 1.2, output: (0..70).map(|i| format!("Oct 22 10:{:02}:00 web-01 app[{i}]: {} request\n", i % 60, if i % 9 == 0 { "ERROR upstream timeout while handling" } else if i % 5 == 0 { "warning slow" } else { "ok" })).collect(), ..Default::default() },
                HtmlItem { group: "Container".into(), file: "docker-ps.txt".into(), command: "docker ps".into(), status: Some(127), secs: 0.1, output: "docker: command not found\n".into(), ..Default::default() },
                HtmlItem { group: "Eigene".into(), file: "custom-1.txt".into(), command: "cat /var/log/custom".into(), status: None, ..Default::default() },
            ],
        };
        std::fs::write(path, render(&r)).unwrap();
    }
}
