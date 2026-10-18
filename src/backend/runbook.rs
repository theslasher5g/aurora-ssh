//! Runbooks: step-by-step procedures with optional commands. Stored in the library,
//! exchanged as Markdown, and run against terminal sessions; a run ends in a Markdown
//! report with timestamps, notes and the captured output of each step.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::ShellKind;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Runbook {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub steps: Vec<RunbookStep>,
}

impl Default for Runbook {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            title: String::new(),
            description: String::new(),
            steps: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(default)]
pub struct RunbookStep {
    /// Instructions; `**bold**` and `` `code` `` are highlighted.
    pub text: String,
    /// Typed into the session by "Ausführen" (may contain `{{variables}}`).
    pub command: String,
    pub shell: ShellKind,
    /// Ask once more before the command runs.
    pub confirm: bool,
}

impl Runbook {
    /// Distinct `{{variables}}` of all step commands, in order of appearance.
    pub fn variables(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.steps {
            for v in variables_in(&s.command) {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        out
    }
}

/// `{{name}}` placeholders in `text`, in order, without duplicates.
pub fn variables_in(text: &str) -> Vec<String> {
    crate::model::Snippet {
        script: text.to_string(),
        ..Default::default()
    }
    .variables()
}

/// Fills in `{{name}}` placeholders; unknown ones stay as they are.
pub fn render(text: &str, values: &HashMap<String, String>) -> String {
    crate::model::Snippet {
        script: text.to_string(),
        ..Default::default()
    }
    .render(values)
}

// ------------------------------------------------------------------ Markdown

/// Markdown form of a runbook: `# Title`, the description, then one `## n. …` section per
/// step with an optional `> Bestätigung nötig` line and the command in a code fence.
pub fn to_markdown(rb: &Runbook) -> String {
    let mut out = format!("# {}\n\n", one_line(&rb.title));
    if !rb.description.trim().is_empty() {
        out.push_str(rb.description.trim());
        out.push_str("\n\n");
    }
    for (i, s) in rb.steps.iter().enumerate() {
        let text = s.text.trim();
        let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
        out.push_str(&format!("## {}. {}\n\n", i + 1, first.trim()));
        if !rest.trim().is_empty() {
            out.push_str(rest.trim());
            out.push_str("\n\n");
        }
        if s.confirm {
            out.push_str(&format!("> ⚠ {}\n\n", t!("Bestätigung nötig")));
        }
        if !s.command.trim().is_empty() {
            let lang = match s.shell {
                ShellKind::Shell => "bash",
                ShellKind::PowerShell => "powershell",
            };
            out.push_str(&format!("```{lang}\n{}\n```\n\n", s.command.trim_end()));
        }
    }
    out.trim_end().to_string() + "\n"
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reads the format written by [`to_markdown`] (and hand-written variants of it).
pub fn from_markdown(md: &str) -> Result<Runbook> {
    let mut rb = Runbook::default();
    let mut desc: Vec<&str> = Vec::new();
    // Current step: heading text, body lines.
    let mut step: Option<(RunbookStep, Vec<String>)> = None;
    let mut fence: Option<(ShellKind, Vec<String>)> = None;
    let finish = |rb: &mut Runbook, step: &mut Option<(RunbookStep, Vec<String>)>| {
        if let Some((mut s, body)) = step.take() {
            let body = body.join("\n");
            let body = body.trim();
            s.text = match (s.text.trim().is_empty(), body.is_empty()) {
                (_, true) => s.text.trim().to_string(),
                (true, false) => body.to_string(),
                (false, false) => format!("{}\n{body}", s.text.trim()),
            };
            rb.steps.push(s);
        }
    };
    let md = md.replace("\r\n", "\n");
    for line in md.lines() {
        if let Some((shell, lines)) = fence.as_mut() {
            if line.trim_start().starts_with("```") {
                let cmd = lines.join("\n");
                let shell = *shell;
                fence = None;
                if let Some((s, body)) = step.as_mut() {
                    if s.command.is_empty() {
                        s.command = cmd;
                        s.shell = shell;
                    } else {
                        // A second block stays part of the text.
                        body.push(format!("```\n{cmd}\n```"));
                    }
                }
            } else {
                lines.push(line.to_string());
            }
            continue;
        }
        let trimmed = line.trim();
        if let Some(lang) = trimmed.strip_prefix("```") {
            let lang = lang.trim().to_lowercase();
            let shell = if matches!(lang.as_str(), "powershell" | "ps1" | "pwsh" | "ps") {
                ShellKind::PowerShell
            } else {
                ShellKind::Shell
            };
            fence = Some((shell, Vec::new()));
            if step.is_none() {
                // Code before the first step: start an untitled one.
                step = Some((RunbookStep::default(), Vec::new()));
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("# ")
            && rb.title.is_empty()
            && step.is_none()
        {
            rb.title = h.trim().to_string();
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            finish(&mut rb, &mut step);
            step = Some((
                RunbookStep {
                    text: strip_step_number(h).to_string(),
                    ..Default::default()
                },
                Vec::new(),
            ));
            continue;
        }
        if let Some(q) = trimmed.strip_prefix('>') {
            let q = q.to_lowercase();
            if (q.contains("bestätigung") || q.contains("confirm"))
                && let Some((s, _)) = step.as_mut()
            {
                s.confirm = true;
                continue;
            }
        }
        match step.as_mut() {
            Some((_, body)) => body.push(line.to_string()),
            None => desc.push(line),
        }
    }
    if let Some((shell, lines)) = fence.take()
        && let Some((s, _)) = step.as_mut()
        && s.command.is_empty()
    {
        // Unclosed fence at the end of the file.
        s.command = lines.join("\n");
        s.shell = shell;
    }
    finish(&mut rb, &mut step);
    rb.description = desc.join("\n").trim().to_string();
    if rb.title.trim().is_empty() {
        bail!(t!("Keine Überschrift (# Titel) gefunden"));
    }
    Ok(rb)
}

/// `1. Text`, `Schritt 2: Text`, `Step 3 – Text` → `Text`.
fn strip_step_number(h: &str) -> &str {
    let h = h.trim();
    let lower = h.to_lowercase();
    let rest = ["schritt", "step"]
        .iter()
        .find_map(|w| lower.starts_with(w).then(|| &h[w.len()..]))
        .unwrap_or(h)
        .trim_start();
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return h;
    }
    rest[digits..].trim_start_matches(['.', ':', ')', '–', '-', ' '])
}

// ------------------------------------------------------------------ runs

/// State of one step during a run.
#[derive(Clone, Debug, Default)]
pub struct StepRun {
    pub done: bool,
    pub note: String,
    /// "Ausführen" was pressed (first time).
    pub started: Option<DateTime<Local>>,
    pub finished: Option<DateTime<Local>>,
    /// Terminal output from the step's start until the next step started (raw bytes).
    pub output: Vec<u8>,
    /// The command that was sent (variables filled in).
    pub sent: Option<String>,
}

/// Everything the report needs.
#[derive(Clone, Debug)]
pub struct RunLog {
    pub runbook: Runbook,
    pub values: Vec<(String, String)>,
    pub host: String,
    /// `user@host:port` of the session.
    pub target: String,
    /// Local user who ran it.
    pub operator: String,
    pub started: DateTime<Local>,
    pub steps: Vec<StepRun>,
}

/// Captured output is cut off beyond this size (per step).
pub const MAX_OUTPUT: usize = 512 * 1024;

impl RunLog {
    pub fn new(
        runbook: Runbook,
        values: Vec<(String, String)>,
        host: String,
        target: String,
    ) -> Self {
        let steps = vec![StepRun::default(); runbook.steps.len()];
        Self {
            runbook,
            values,
            host,
            target,
            operator: operator(),
            started: Local::now(),
            steps,
        }
    }

    pub fn values_map(&self) -> HashMap<String, String> {
        self.values.iter().cloned().collect()
    }

    /// The step's command with the variables filled in.
    pub fn command(&self, i: usize) -> String {
        self.runbook
            .steps
            .get(i)
            .map(|s| render(&s.command, &self.values_map()))
            .unwrap_or_default()
    }
}

/// Name of the local user (for the report).
pub fn operator() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| {
            directories::UserDirs::new().and_then(|u| {
                u.home_dir()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_else(|| "?".into())
}

/// Plain text of terminal output: escape sequences removed, carriage-return overwrites and
/// backspaces applied, trailing blanks and blank runs trimmed.
pub fn clean_output(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut lines: Vec<String> = Vec::new();
    let mut line: Vec<char> = Vec::new();
    let mut col = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    // CSI: parameters until a final byte 0x40–0x7e. Cursor moves and
                    // line erases on the current line are applied (shell line editing).
                    let mut params = String::new();
                    let mut fin = None;
                    for n in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&n) {
                            fin = Some(n);
                            break;
                        }
                        params.push(n);
                    }
                    let n = params.parse::<usize>().unwrap_or(0);
                    match fin {
                        Some('D') => col = col.saturating_sub(n.max(1)),
                        Some('C') => col += n.max(1),
                        Some('G') => col = n.saturating_sub(1),
                        Some('K') if n == 0 => line.truncate(col),
                        Some('K') if n == 2 => line.clear(),
                        Some('P') => {
                            // Delete characters at the cursor.
                            for _ in 0..n.max(1) {
                                if col < line.len() {
                                    line.remove(col);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(']') | Some('P') | Some('_') | Some('^') => {
                    // OSC/DCS…: until BEL or ESC \.
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(') | Some(')') | Some('#') => {
                    chars.next();
                }
                _ => {}
            },
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                col = 0;
            }
            '\n' => {
                lines.push(line.iter().collect::<String>().trim_end().to_string());
                line.clear();
                col = 0;
            }
            '\x08' => col = col.saturating_sub(1),
            '\t' => {
                let next = (col / 8 + 1) * 8;
                while col < next {
                    put(&mut line, col, ' ');
                    col += 1;
                }
            }
            c if c.is_control() => {}
            c => {
                put(&mut line, col, c);
                col += 1;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line.iter().collect::<String>().trim_end().to_string());
    }
    // No more than one blank line in a row; none at the ends.
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        if l.is_empty() && out.last().is_none_or(|p: &String| p.is_empty()) {
            continue;
        }
        out.push(l);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn put(line: &mut Vec<char>, col: usize, c: char) {
    while line.len() < col {
        line.push(' ');
    }
    if col < line.len() {
        line[col] = c;
    } else {
        line.push(c);
    }
}

fn fmt_time(t: &DateTime<Local>) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Markdown protocol of a run.
pub fn report_markdown(log: &RunLog, now: DateTime<Local>) -> String {
    let rb = &log.runbook;
    let mut out = format!("# {}: {}\n\n", t!("Protokoll"), one_line(&rb.title));
    let done = log.steps.iter().filter(|s| s.done).count();
    let rows = [
        (t!("Host"), log.host.clone()),
        (t!("Ziel"), log.target.clone()),
        (t!("Ausgeführt von"), log.operator.clone()),
        (t!("Beginn"), fmt_time(&log.started)),
        (t!("Ende"), fmt_time(&now)),
        (
            t!("Dauer"),
            crate::ui::short_duration((now - log.started).to_std().unwrap_or_default()),
        ),
        (
            t!("Schritte erledigt"),
            format!("{done} / {}", log.steps.len()),
        ),
    ];
    for (k, v) in rows {
        out.push_str(&format!("- **{k}:** {v}\n"));
    }
    if !log.values.is_empty() {
        out.push_str(&format!("- **{}:** ", t!("Variablen")));
        let vars: Vec<String> = log
            .values
            .iter()
            .map(|(k, v)| format!("`{k}` = `{v}`"))
            .collect();
        out.push_str(&vars.join(", "));
        out.push('\n');
    }
    if !rb.description.trim().is_empty() {
        out.push_str(&format!("\n{}\n", rb.description.trim()));
    }
    let values = log.values_map();
    for (i, (step, run)) in rb.steps.iter().zip(&log.steps).enumerate() {
        let text = render(&step.text, &values);
        let text = text.trim();
        let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
        let mark = if run.done { "✅" } else { "⬜" };
        out.push_str(&format!("\n## {mark} {}. {}\n\n", i + 1, first.trim()));
        if !rest.trim().is_empty() {
            out.push_str(rest.trim());
            out.push_str("\n\n");
        }
        let mut meta = Vec::new();
        if let Some(t) = &run.started {
            meta.push(format!("{}: {}", t!("gestartet"), fmt_time(t)));
        }
        if let Some(t) = &run.finished {
            meta.push(format!("{}: {}", t!("erledigt"), fmt_time(t)));
        }
        if run.started.is_none() && run.finished.is_none() {
            meta.push(t!("nicht ausgeführt").to_string());
        }
        out.push_str(&format!("*{}*\n\n", meta.join(" · ")));
        if let Some(cmd) = &run.sent {
            out.push_str(&format!("```\n$ {}\n```\n\n", cmd.trim_end()));
        }
        if !run.note.trim().is_empty() {
            out.push_str(&format!("> {}: {}\n\n", t!("Notiz"), run.note.trim()));
        }
        let output = clean_output(&run.output);
        if !output.is_empty() {
            out.push_str(&format!("{}:\n\n```text\n{output}\n```\n", t!("Ausgabe")));
        }
    }
    out
}

/// File name without characters that are awkward in paths.
pub fn safe_name(s: &str) -> String {
    let mut out: String = s
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches(['_', '.']).to_string();
    if out.is_empty() {
        "runbook".into()
    } else {
        out
    }
}

/// `Documents/Aurora/<sub>` (created), else `<data dir>/<sub>`.
pub fn output_dir(sub: &str) -> PathBuf {
    if let Some(d) = directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(|d| d.join("Aurora").join(sub)))
        && std::fs::create_dir_all(&d).is_ok()
    {
        return d;
    }
    let d = crate::store::data_dir().join(sub.to_lowercase());
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Shows a folder in the file manager.
pub fn open_folder(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(windows)]
    std::process::Command::new("explorer.exe")
        .arg(dir)
        .spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(dir).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(dir).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Runbook {
        Runbook {
            title: "Webserver neu starten".into(),
            description: "Für **nginx** auf den Web-Hosts.".into(),
            steps: vec![
                RunbookStep {
                    text: "Status prüfen\nAuf `active` achten.".into(),
                    command: "systemctl status {{service}}".into(),
                    ..Default::default()
                },
                RunbookStep {
                    text: "Neu starten".into(),
                    command: "sudo systemctl restart {{service}}\necho {{ host }}".into(),
                    confirm: true,
                    ..Default::default()
                },
                RunbookStep {
                    text: "Im Browser kontrollieren".into(),
                    ..Default::default()
                },
                RunbookStep {
                    text: "Windows-Dienst".into(),
                    command: "Get-Service W3SVC".into(),
                    shell: ShellKind::PowerShell,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn markdown_round_trip() {
        let rb = sample();
        let md = to_markdown(&rb);
        assert!(md.starts_with("# Webserver neu starten\n"));
        assert!(md.contains("## 2. Neu starten"));
        assert!(md.contains("```bash\nsystemctl status {{service}}\n```"));
        assert!(md.contains("```powershell\nGet-Service W3SVC\n```"));
        let back = from_markdown(&md).unwrap();
        assert_eq!(back.title, rb.title);
        assert_eq!(back.description, rb.description);
        assert_eq!(back.steps, rb.steps);
        assert_eq!(rb.variables(), vec!["service", "host"]);
    }

    #[test]
    fn markdown_import_variants() {
        let md = "Intro ohne Titel?\n# Backup\r\nText\n## Schritt 1: Dump\n> Confirmation required\n```sh\npg_dump db > x.sql\n```\n## Step 2 – Copy\nscp x.sql backup:\n```\nrsync -a";
        let rb = from_markdown(md).unwrap();
        assert_eq!(rb.title, "Backup");
        assert_eq!(rb.description, "Intro ohne Titel?\nText");
        assert_eq!(rb.steps.len(), 2);
        assert_eq!(rb.steps[0].text, "Dump");
        assert!(rb.steps[0].confirm);
        assert_eq!(rb.steps[0].command, "pg_dump db > x.sql");
        assert_eq!(rb.steps[1].text, "Copy\nscp x.sql backup:");
        assert_eq!(rb.steps[1].command, "rsync -a");
        assert!(!rb.steps[1].confirm);
        assert!(from_markdown("just text").is_err());
    }

    #[test]
    fn output_is_cleaned() {
        assert_eq!(
            clean_output(b"echo hi\x1b[7Decho hi\r\nhi\r\nabcdef\x1b[3D\x1b[K!\n"),
            "echo hi\nhi\nabc!"
        );
        let raw = b"\x1b]0;user@box: ~\x07\x1b[01;32muser@box\x1b[00m:~$ ls\r\nfile1  file2\r\n\r\n\r\n50%\r100%\r\nab\x08c\n\x1b[?2004h";
        assert_eq!(
            clean_output(raw),
            "user@box:~$ ls\nfile1  file2\n\n100%\nac"
        );
    }

    #[test]
    fn report_contains_steps() {
        let mut log = RunLog::new(
            sample(),
            vec![("service".into(), "nginx".into())],
            "web-01".into(),
            "root@web-01:22".into(),
        );
        assert_eq!(log.command(0), "systemctl status nginx");
        log.steps[0].done = true;
        log.steps[0].started = Some(log.started);
        log.steps[0].sent = Some(log.command(0));
        log.steps[0].note = "lief".into();
        log.steps[0].output = b"\x1b[32mactive (running)\x1b[0m\r\n".to_vec();
        let md = report_markdown(&log, log.started + chrono::Duration::seconds(75));
        assert!(md.contains("web-01"));
        assert!(md.contains("1 / 4"));
        assert!(md.contains("## ✅ 1. Status prüfen"));
        assert!(md.contains("$ systemctl status nginx"));
        assert!(md.contains("lief"));
        assert!(md.contains("```text\nactive (running)\n```"));
        assert!(md.contains("## ⬜ 3. Im Browser kontrollieren"));
        assert!(md.contains("1 min 15 s"));
    }

    #[test]
    fn names_are_safe() {
        assert_eq!(safe_name("Web / neu starten!"), "Web_neu_starten");
        assert_eq!(safe_name("../.."), "runbook");
    }
}
