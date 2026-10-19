//! Full-text index over all recordings. Each `.cast` is replayed into a virtual terminal;
//! the lines that appear on screen become the searchable text with the time they showed
//! up. The index is cached in the data directory (`recordings_index.json.gz`) and updated
//! per file when size or modification time changed. Notes (markers) are re-read every time.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde::{Deserialize, Serialize};

use crate::backend::record::{self, Cast};
use crate::terminal::Terminal;
use crate::ui::sessionlog_markers::{self, Marker};

const VERSION: u32 = 1;
const MAX_LINES: usize = 50_000;
const MAX_LINE_CHARS: usize = 400;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub mtime: u64,
    pub size: u64,
    pub title: String,
    /// Unix time of the start.
    pub start: i64,
    pub duration: f64,
    pub host: Option<String>,
    /// (seconds, text)
    pub lines: Vec<(f32, String)>,
    /// Markers and notes, refreshed on every scan.
    pub notes: Vec<(f32, String)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    pub version: u32,
    /// File name → entry.
    pub files: BTreeMap<String, Entry>,
}

impl Index {
    pub fn line_count(&self) -> usize {
        self.files.values().map(|e| e.lines.len()).sum()
    }
}

/// Joins wrapped rows into lines and adds the finished ones.
fn push_rows(
    t: f64,
    rows: Vec<(String, bool)>,
    pending: &mut String,
    out: &mut Vec<(f64, String)>,
) {
    for (text, wrapped) in rows {
        pending.push_str(text.trim_end());
        if !wrapped {
            let line: String = pending.trim().chars().take(MAX_LINE_CHARS).collect();
            pending.clear();
            if !line.is_empty() && out.len() < MAX_LINES {
                out.push((t, line));
            }
        }
    }
}

/// Plain text of a recording with the time each line showed up.
pub fn render_lines(cast: &Cast) -> Vec<(f64, String)> {
    let mut term = Terminal::with_size(cast.width.max(10), cast.height.max(3), 5000);
    let mut mark = 0usize;
    let mut out: Vec<(f64, String)> = Vec::new();
    let mut pending = String::new();
    for f in &cast.frames {
        term.feed(f.data.as_bytes());
        let rows = term.take_new_lines(&mut mark);
        push_rows(f.time, rows, &mut pending, &mut out);
    }
    let end = cast.frames.last().map_or(0.0, |f| f.time);
    let rows = term.take_last_lines(&mut mark);
    push_rows(end, rows, &mut pending, &mut out);
    if !pending.trim().is_empty() && out.len() < MAX_LINES {
        out.push((end, pending.trim().chars().take(MAX_LINE_CHARS).collect()));
    }
    out
}

fn build_entry(path: &Path, mtime: u64, size: u64) -> Option<Entry> {
    let cast = record::read_full(path).ok()?;
    let lines = render_lines(&cast)
        .into_iter()
        .map(|(t, s)| (t as f32, s))
        .collect();
    Some(Entry {
        mtime,
        size,
        title: cast.title.clone(),
        start: cast.timestamp,
        duration: cast.frames.last().map_or(0.0, |f| f.time),
        host: cast.host.clone(),
        lines,
        notes: Vec::new(),
    })
}

fn notes_of(path: &Path) -> Vec<(f32, String)> {
    let in_cast = record::read_full(path)
        .map(|c| c.markers)
        .unwrap_or_default();
    sessionlog_markers::load(path, &in_cast)
        .into_iter()
        .map(|m: Marker| (m.t as f32, m.label))
        .collect()
}

pub fn cache_path() -> PathBuf {
    crate::store::data_dir().join("recordings_index.json.gz")
}

pub fn load_cache(path: &Path) -> Index {
    let Ok(bytes) = std::fs::read(path) else {
        return Index::default();
    };
    let mut text = String::new();
    if flate2::read::GzDecoder::new(&bytes[..])
        .read_to_string(&mut text)
        .is_err()
    {
        return Index::default();
    }
    match serde_json::from_str::<Index>(&text) {
        Ok(i) if i.version == VERSION => i,
        _ => Index::default(),
    }
}

pub fn save_cache(path: &Path, index: &Index) {
    let Ok(json) = serde_json::to_vec(index) else {
        return;
    };
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    if enc.write_all(&json).is_err() {
        return;
    }
    let Ok(bytes) = enc.finish() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn secs(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Brings `index` up to date with `files` (path, size, modified); returns whether any
/// recording had to be rendered. `progress` gets (done, total).
pub fn update(
    index: &mut Index,
    files: &[(PathBuf, u64, std::time::SystemTime)],
    mut progress: impl FnMut(usize, usize),
) -> bool {
    let mut changed = index.version != VERSION;
    index.version = VERSION;
    let names: Vec<String> = files
        .iter()
        .map(|(p, _, _)| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let before = index.files.len();
    index.files.retain(|k, _| names.contains(k));
    changed |= index.files.len() != before;
    for (i, ((path, size, modified), name)) in files.iter().zip(&names).enumerate() {
        progress(i, files.len());
        let m = secs(*modified);
        let fresh = index
            .files
            .get(name)
            .is_some_and(|e| e.mtime == m && e.size == *size);
        if !fresh {
            match build_entry(path, m, *size) {
                Some(e) => {
                    index.files.insert(name.clone(), e);
                    changed = true;
                }
                None => {
                    index.files.remove(name);
                    continue;
                }
            }
        }
        if let Some(e) = index.files.get_mut(name) {
            e.notes = notes_of(path);
        }
    }
    progress(files.len(), files.len());
    changed
}

pub enum Msg {
    Progress(usize, usize),
    Done(Box<Index>),
}

/// Updates the cached index in a worker thread.
pub fn spawn(
    files: Vec<(PathBuf, u64, std::time::SystemTime)>,
    cache: PathBuf,
    wake: impl Fn() + Send + 'static,
) -> mpsc::Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut index = load_cache(&cache);
        let changed = update(&mut index, &files, |d, n| {
            let _ = tx.send(Msg::Progress(d, n));
            wake();
        });
        if changed {
            save_cache(&cache, &index);
        }
        let _ = tx.send(Msg::Done(Box::new(index)));
        wake();
    });
    rx
}

// ---------------------------------------------------------------- search

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Recording file name.
    pub file: String,
    pub title: String,
    pub start: i64,
    /// Seconds into the recording.
    pub t: f64,
    pub snippet: String,
    /// Match inside the snippet, in characters.
    pub range: (usize, usize),
    pub is_note: bool,
    pub score: f32,
}

/// Part of `text` around the first match: (snippet, match range in chars).
pub fn snippet(text: &str, from: usize, len: usize) -> (String, (usize, usize)) {
    let chars: Vec<char> = text.chars().collect();
    let start = from.saturating_sub(40);
    let end = (from + len + 70).min(chars.len());
    let mut s = String::new();
    let mut lead = 0;
    if start > 0 {
        s.push('…');
        lead = 1;
    }
    s.extend(&chars[start..end]);
    if end < chars.len() {
        s.push('…');
    }
    (s, (from - start + lead, from - start + lead + len))
}

/// Where the words of `query` appear in `text` (case-insensitive): all words must be
/// there. Returns the char position and length of the best match and a relevance score.
fn match_line(text: &str, words: &[String], phrase: &str) -> Option<(usize, usize, f32)> {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    if lower.len() != text.chars().count() {
        // Case mapping changed the length: fall back to the plain text.
        return None;
    }
    let find = |needle: &str| -> Option<usize> {
        let n: Vec<char> = needle.chars().collect();
        if n.is_empty() || n.len() > lower.len() {
            return None;
        }
        (0..=lower.len() - n.len()).find(|&i| lower[i..i + n.len()] == n[..])
    };
    let mut score = 0.0;
    let mut first: Option<(usize, usize)> = None;
    for w in words {
        let i = find(w)?;
        let n = w.chars().count();
        // Whole words count more.
        let before = i == 0 || !lower[i - 1].is_alphanumeric();
        let after = lower.get(i + n).is_none_or(|c| !c.is_alphanumeric());
        score += 1.0 + if before && after { 1.0 } else { 0.0 };
        if first.is_none_or(|(p, _)| i < p) {
            first = Some((i, n));
        }
    }
    if words.len() > 1
        && let Some(i) = find(phrase)
    {
        score += 3.0;
        first = Some((i, phrase.chars().count()));
    }
    let (i, n) = first?;
    Some((i, n, score))
}

/// Searches the index: lines and notes of all recordings. Notes rank above output;
/// within a score newer recordings come first. At most `limit` hits and 25 per recording.
pub fn search(index: &Index, query: &str, limit: usize) -> Vec<Hit> {
    let phrase = query.trim().to_lowercase();
    let words: Vec<String> = phrase.split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (file, e) in &index.files {
        let mut n = 0;
        let notes = e.notes.iter().map(|(t, s)| (*t, s, true));
        let lines = e.lines.iter().map(|(t, s)| (*t, s, false));
        for (t, text, is_note) in notes.chain(lines) {
            if n >= 25 {
                break;
            }
            let Some((pos, len, mut score)) = match_line(text, &words, &phrase) else {
                continue;
            };
            if is_note {
                score += 5.0;
            }
            let (snip, range) = snippet(text, pos, len);
            n += 1;
            hits.push(Hit {
                file: file.clone(),
                title: e.title.clone(),
                start: e.start,
                t: t as f64,
                snippet: snip,
                range,
                is_note,
                score,
            });
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(b.start.cmp(&a.start))
            .then(a.t.total_cmp(&b.t))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::record::Frame;

    fn cast(frames: &[(f64, &str)]) -> Cast {
        Cast {
            width: 40,
            height: 5,
            title: "web-01".into(),
            timestamp: 1_700_000_000,
            host: None,
            frames: frames
                .iter()
                .map(|(t, d)| Frame {
                    time: *t,
                    data: d.to_string(),
                })
                .collect(),
            markers: Vec::new(),
        }
    }

    #[test]
    fn renders_text_with_times() {
        // More lines than the 5-row screen: scrolled-off lines are kept.
        let mut frames = vec![(0.5, "$ ls\r\n".to_string())];
        for i in 0..12 {
            frames.push((1.0 + i as f64, format!("file{i}\r\n")));
        }
        frames.push((20.0, "\x1b[31mERROR\x1b[0m: disk full\r\n$ ".to_string()));
        let borrowed: Vec<(f64, &str)> = frames.iter().map(|(t, s)| (*t, s.as_str())).collect();
        let lines = render_lines(&cast(&borrowed));
        let text: Vec<&str> = lines.iter().map(|l| l.1.as_str()).collect();
        assert_eq!(text[0], "$ ls");
        assert!(text.contains(&"file0") && text.contains(&"file11"));
        assert!(text.contains(&"ERROR: disk full"), "{text:?}");
        let err = lines.iter().find(|l| l.1.starts_with("ERROR")).unwrap();
        assert_eq!(err.0, 20.0);
        // No escape sequences leak into the text.
        assert!(lines.iter().all(|l| !l.1.contains('\x1b')));
        // The last prompt (cursor line) is included too.
        assert_eq!(text.last(), Some(&"$"));
    }

    #[test]
    fn search_ranks_snippets_and_notes() {
        let mut index = Index::default();
        let line = |t: f32, s: &str| (t, s.to_string());
        index.files.insert(
            "a.cast".into(),
            Entry {
                title: "web-01".into(),
                start: 100,
                lines: vec![
                    line(1.0, "restarting nginx"),
                    line(2.0, "nginxy thing"),
                    line(
                        3.0,
                        &format!("{} nginx error here {}", "x".repeat(100), "y".repeat(100)),
                    ),
                ],
                notes: vec![line(9.0, "deploy nginx")],
                ..Entry::default()
            },
        );
        index.files.insert(
            "b.cast".into(),
            Entry {
                title: "db-01".into(),
                start: 200,
                lines: vec![line(4.0, "Nginx started")],
                ..Entry::default()
            },
        );
        let hits = search(&index, "NGINX", 10);
        assert_eq!(hits.len(), 5);
        // The note first, then whole-word hits (newer recording first), then the partial.
        assert!(hits[0].is_note && hits[0].t == 9.0);
        assert_eq!(hits[1].file, "b.cast");
        assert_eq!(hits.last().unwrap().snippet, "nginxy thing");
        // Snippet of a long line is cut and the match range points at the word.
        let long = hits.iter().find(|h| h.snippet.contains("error")).unwrap();
        assert!(long.snippet.starts_with('…') && long.snippet.ends_with('…'));
        let (a, b) = long.range;
        let word: String = long.snippet.chars().skip(a).take(b - a).collect();
        assert_eq!(word.to_lowercase(), "nginx");
        // Several words: all must be there, the phrase ranks higher.
        let h = search(&index, "restarting nginx", 10);
        assert_eq!(h.len(), 1);
        assert!(search(&index, "nginx missing", 10).is_empty());
        assert!(search(&index, "   ", 10).is_empty());
        assert_eq!(search(&index, "nginx", 2).len(), 2);
    }

    #[test]
    fn index_updates_by_mtime_and_caches() {
        let dir = std::env::temp_dir().join(format!("aurora-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.cast");
        std::fs::write(
            &file,
            "{\"version\":2,\"width\":40,\"height\":5,\"timestamp\":5,\"title\":\"x\"}\n[0.5,\"o\",\"hello world\\r\\n\"]\n[1.0,\"m\",\"checkpoint\"]\n",
        )
        .unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        let files = vec![(file.clone(), meta.len(), meta.modified().unwrap())];
        let mut idx = Index::default();
        assert!(update(&mut idx, &files, |_, _| {}));
        assert_eq!(idx.files["t.cast"].lines[0].1, "hello world");
        assert_eq!(idx.files["t.cast"].notes[0].1, "checkpoint");
        // Unchanged file: not rendered again (the entry is kept as is).
        idx.files
            .get_mut("t.cast")
            .unwrap()
            .lines
            .push((9.0, "marker".into()));
        assert!(!update(&mut idx, &files, |_, _| {}));
        assert_eq!(idx.files["t.cast"].lines.len(), 2);
        // Sidecar notes show up without re-rendering.
        sessionlog_markers::save_all(
            &file,
            &[Marker {
                t: 2.0,
                label: "later".into(),
            }],
        )
        .unwrap();
        update(&mut idx, &files, |_, _| {});
        assert_eq!(idx.files["t.cast"].notes[0].1, "later");
        // Cache round trip.
        let cache = dir.join("idx.json.gz");
        save_cache(&cache, &idx);
        assert_eq!(load_cache(&cache).files.len(), 1);
        // Deleted recordings leave the index.
        assert!(update(&mut idx, &[], |_, _| {}));
        assert!(idx.files.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
