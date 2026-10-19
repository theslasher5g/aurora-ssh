//! Session history: an append-only file `sessions.jsonl` in the data directory with one
//! line when a session starts and one when it ends. It holds host, times, the recording's
//! file name and the number of markers – never a command.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Lines read at most (older ones are ignored).
const MAX_LINES: usize = 20_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    /// `start` or `end`.
    pub ev: String,
    pub id: String,
    pub host: Option<String>,
    pub name: String,
    /// Unix seconds.
    pub t: i64,
    /// File name of the recording (not a path).
    pub rec: Option<String>,
    pub markers: usize,
}

/// One session, start and end lines joined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub host: Option<String>,
    pub name: String,
    pub start: i64,
    pub end: Option<i64>,
    pub rec: Option<String>,
    pub markers: usize,
}

impl Session {
    pub fn duration(&self) -> Option<i64> {
        self.end.map(|e| (e - self.start).max(0))
    }
}

/// Path of a recording inside `dir`. The name comes from the history file, so it must be a
/// plain `*.cast` file name: no directories, no `..`, no absolute paths.
pub fn rec_path(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    let p = std::path::Path::new(name);
    let plain = p
        .file_name()
        .is_some_and(|f| f == std::ffi::OsStr::new(name));
    (plain && name.ends_with(".cast") && !name.contains(['/', '\\', '\0'])).then(|| dir.join(name))
}

pub fn path() -> PathBuf {
    crate::store::data_dir().join("sessions.jsonl")
}

pub fn append(line: &Line) {
    append_to(&path(), line);
}

pub fn append_to(path: &std::path::Path, line: &Line) {
    let Ok(json) = serde_json::to_string(line) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{json}");
    }
}

/// Joins the lines of the file; sessions come out newest first.
pub fn parse(text: &str) -> Vec<Session> {
    let mut list: Vec<Session> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(MAX_LINES);
    for l in &lines[skip..] {
        let Ok(line) = serde_json::from_str::<Line>(l) else {
            continue;
        };
        if line.id.is_empty() {
            continue;
        }
        let pos = list.iter().position(|s| s.id == line.id);
        match (line.ev.as_str(), pos) {
            ("start", None) => list.push(Session {
                id: line.id,
                host: line.host,
                name: line.name,
                start: line.t,
                end: None,
                rec: line.rec,
                markers: 0,
            }),
            ("end", Some(i)) => {
                let s = &mut list[i];
                s.end = Some(line.t);
                s.rec = line.rec.or(s.rec.take());
                s.markers = line.markers;
            }
            _ => {}
        }
    }
    list.sort_by_key(|s| std::cmp::Reverse(s.start));
    list
}

/// Sessions without a recording that are kept (the newest ones).
const KEEP_UNRECORDED: usize = 20;
/// Sessions kept in total.
const KEEP_TOTAL: usize = 300;

/// Ids to drop so that only the newest unrecorded sessions and at most `KEEP_TOTAL`
/// remain (recordings are never dropped by age here).
pub fn prune_ids(sessions: &[Session]) -> Vec<String> {
    let mut drop = Vec::new();
    let mut unrecorded = 0;
    for (i, s) in sessions.iter().enumerate() {
        let over = if s.rec.is_none() {
            unrecorded += 1;
            unrecorded > KEEP_UNRECORDED
        } else {
            false
        };
        if over || i >= KEEP_TOTAL {
            drop.push(s.id.clone());
        }
    }
    drop
}

/// Removes the sessions with these ids from the history file.
pub fn remove(ids: &[String]) {
    remove_in(&path(), ids);
}

pub fn remove_in(path: &std::path::Path, ids: &[String]) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            serde_json::from_str::<Line>(l)
                .map(|x| !ids.contains(&x.id))
                .unwrap_or(false)
        })
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let _ = std::fs::write(path, out);
}

/// Once per run: trims the history so it does not fill up.
fn prune_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path()) else {
        return;
    };
    let drop = prune_ids(&parse(&text));
    if !drop.is_empty() {
        remove(&drop);
    }
}

pub fn load() -> Vec<Session> {
    prune_once();
    std::fs::read_to_string(path())
        .map(|t| parse(&t))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_end_lines_are_joined() {
        let dir = std::env::temp_dir().join(format!("aurora-hist-{}", std::process::id()));
        let file = dir.join("sessions.jsonl");
        let l = |ev: &str, id: &str, t: i64| Line {
            ev: ev.into(),
            id: id.into(),
            host: Some("h1".into()),
            name: "web-01".into(),
            t,
            ..Line::default()
        };
        append_to(&file, &l("start", "a", 100));
        append_to(&file, &l("start", "b", 500));
        append_to(
            &file,
            &Line {
                rec: Some("web_a.cast".into()),
                markers: 3,
                ..l("end", "a", 160)
            },
        );
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("garbage\n{\"ev\":\"end\",\"id\":\"zzz\",\"t\":1}\n");
        let list = parse(&text);
        assert_eq!(list.len(), 2);
        // Newest first; "b" never ended (crash, app killed).
        assert_eq!(list[0].id, "b");
        assert_eq!(list[0].duration(), None);
        assert_eq!(list[1].duration(), Some(60));
        assert_eq!(list[1].rec.as_deref(), Some("web_a.cast"));
        assert_eq!(list[1].markers, 3);
        // No command text anywhere in the file.
        assert!(!text.contains("cmd"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    #[test]
    fn recording_names_cannot_leave_the_folder() {
        let d = std::path::Path::new("/rec");
        assert!(rec_path(d, "a.cast").is_some());
        for bad in [
            "../x.cast",
            "/etc/passwd",
            "a/b.cast",
            "a\\b.cast",
            "x.txt",
            "",
            "..",
        ] {
            assert!(rec_path(d, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn keeps_recordings_and_newest_unrecorded() {
        let list: Vec<Session> = (0..30)
            .map(|i| Session {
                id: format!("s{i}"),
                start: 1000 - i,
                rec: (i == 25).then(|| "a.cast".to_string()),
                ..Default::default()
            })
            .collect();
        let drop = prune_ids(&list);
        assert_eq!(drop.len(), 30 - 1 - KEEP_UNRECORDED);
        assert!(!drop.contains(&"s25".to_string()));
        assert!(!drop.contains(&"s0".to_string()));
    }

    #[test]
    fn removes_both_lines_of_a_session() {
        let f = std::env::temp_dir().join(format!("aurora-hist-{}.jsonl", std::process::id()));
        for (ev, id) in [("start", "a"), ("start", "b"), ("end", "a")] {
            append_to(
                &f,
                &Line {
                    ev: ev.into(),
                    id: id.into(),
                    ..Default::default()
                },
            );
        }
        remove_in(&f, &["a".to_string()]);
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("\"b\""));
        let _ = std::fs::remove_file(&f);
    }
}
