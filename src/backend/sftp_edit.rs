//! Support for the built-in editor: text decoding/encoding that keeps line endings, BOM and
//! the final newline, conflict detection for saves, a short diff summary, and the SFTP
//! operations (load, save) that run next to the file browser's commands.

use std::sync::Arc;

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::Sink;
use super::sftp::SftpEvent;

/// Larger files are not opened for editing.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }
}

/// A decoded text file. `text` always uses `\n`; the other fields restore the original
/// layout on save.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub text: String,
    pub eol: Eol,
    /// Both LF and CRLF lines were found (they are unified to the majority style).
    pub mixed: bool,
    /// The file ended with a line break (not part of `text`).
    pub final_newline: bool,
    pub bom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refuse {
    Binary,
    NotUtf8,
    TooLarge(usize),
}

impl Refuse {
    pub fn message(&self) -> String {
        match self {
            Refuse::Binary => t!("Die Datei enthält Binärdaten (Null-Bytes) und lässt sich nicht als Text bearbeiten.").into(),
            Refuse::NotUtf8 => t!("Die Datei ist kein gültiges UTF-8 – der Editor bearbeitet nur UTF-8-Text.").into(),
            Refuse::TooLarge(n) => t!(
                "Die Datei ist mit {} zu groß für den Editor (Grenze 2 MB).",
                crate::ui::human_size(*n as u64)
            ),
        }
    }
}

/// Decodes file content for editing.
pub fn decode(bytes: &[u8]) -> Result<Doc, Refuse> {
    if bytes.len() > MAX_BYTES {
        return Err(Refuse::TooLarge(bytes.len()));
    }
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    if body.contains(&0) {
        return Err(Refuse::Binary);
    }
    let s = std::str::from_utf8(body).map_err(|_| Refuse::NotUtf8)?;
    let crlf = s.matches("\r\n").count();
    let lf = s.matches('\n').count() - crlf;
    let eol = if crlf > lf { Eol::Crlf } else { Eol::Lf };
    let mut text = if crlf > 0 {
        s.replace("\r\n", "\n")
    } else {
        s.to_string()
    };
    let final_newline = if text.ends_with('\n') {
        text.pop();
        true
    } else {
        // An empty file gets a final newline once text is typed.
        text.is_empty()
    };
    Ok(Doc {
        text,
        eol,
        mixed: crlf > 0 && lf > 0,
        final_newline,
        bom,
    })
}

/// The bytes to write for `text` with the layout of the original file.
pub fn encode(text: &str, eol: Eol, final_newline: bool, bom: bool) -> Vec<u8> {
    let mut s = String::with_capacity(text.len() + 8);
    s.push_str(text);
    if final_newline && !text.is_empty() {
        s.push('\n');
    }
    let body = match eol {
        Eol::Lf => s,
        Eol::Crlf => s.replace('\n', "\r\n"),
    };
    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(body.as_bytes());
    out
}

/// What was seen of a file when it was loaded; compared before every save.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Version {
    /// Modification time in seconds (SFTP v3 has no finer resolution).
    pub mtime: Option<u32>,
    pub size: u64,
}

impl Version {
    pub fn of(meta: &russh_sftp::client::fs::Metadata) -> Self {
        Self {
            mtime: meta.mtime,
            size: meta.size.unwrap_or(0),
        }
    }

    pub fn of_local(meta: &std::fs::Metadata) -> Self {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as u32);
        Self {
            mtime,
            size: meta.len(),
        }
    }
}

/// Whether the file changed since it was loaded. `expected` is `None` for a file that did
/// not exist when the editor was opened, `current` is `None` if it does not exist now.
pub fn conflicts(expected: Option<Version>, current: Option<Version>) -> bool {
    match (expected, current) {
        (None, None) => false,
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

/// Difference between the editor's text and the version found on the server.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DiffSummary {
    pub mine_lines: usize,
    pub theirs_lines: usize,
    /// Lines only in the editor's version.
    pub only_mine: usize,
    /// Lines only in the other version.
    pub only_theirs: usize,
    /// First line (1-based) where the two differ.
    pub first_diff: Option<usize>,
}

pub fn diff_summary(mine: &str, theirs: &str) -> DiffSummary {
    let a: Vec<&str> = mine.lines().collect();
    let b: Vec<&str> = theirs.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    // Multiset comparison of the differing middle part.
    let mut counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for l in ma {
        *counts.entry(l).or_default() += 1;
    }
    let mut only_theirs = 0;
    for l in mb {
        match counts.get_mut(l) {
            Some(c) if *c > 0 => *c -= 1,
            _ => only_theirs += 1,
        }
    }
    let only_mine = counts.values().map(|c| (*c).max(0) as usize).sum();
    DiffSummary {
        mine_lines: a.len(),
        theirs_lines: b.len(),
        only_mine,
        only_theirs,
        first_diff: (prefix < a.len().max(b.len())).then_some(prefix + 1),
    }
}

impl DiffSummary {
    pub fn identical(&self) -> bool {
        self.first_diff.is_none()
    }

    pub fn text(&self) -> String {
        if self.identical() {
            return t!("Der Inhalt ist gleich geblieben (nur Datum/Größe haben sich geändert).")
                .into();
        }
        t!(
            "Server: {} Zeilen (hier {}), {} Zeilen nur auf dem Server, {} nur hier, erster Unterschied in Zeile {}.",
            self.theirs_lines,
            self.mine_lines,
            self.only_theirs,
            self.only_mine,
            self.first_diff.unwrap_or(1)
        )
    }
}

// ----------------------------------------------------------------------- local files

/// Reads a local file for editing: bytes (at most `MAX_BYTES + 1`) and its version.
pub fn local_load(path: &std::path::Path) -> std::io::Result<(Vec<u8>, Version)> {
    use std::io::Read;
    let meta = std::fs::metadata(path)?;
    if meta.is_dir() {
        return Err(std::io::Error::other(t!("Das ist ein Ordner.")));
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    Ok((buf, Version::of_local(&meta)))
}

/// Result of a save attempt.
#[derive(Debug)]
pub enum SaveOutcome {
    Saved(Version),
    /// The file changed meanwhile: its current version and content (if readable).
    Conflict(Option<Version>, Option<Vec<u8>>),
}

/// Writes a local file in place (permissions and ownership stay), unless it changed.
pub fn local_save(
    path: &std::path::Path,
    data: &[u8],
    expect: Option<Version>,
    force: bool,
) -> std::io::Result<SaveOutcome> {
    let current = std::fs::metadata(path).ok().map(|m| Version::of_local(&m));
    if !force && conflicts(expect, current) {
        let content = current.and_then(|_| local_load(path).ok().map(|(b, _)| b));
        return Ok(SaveOutcome::Conflict(current, content));
    }
    std::fs::write(path, data)?;
    let meta = std::fs::metadata(path)?;
    Ok(SaveOutcome::Saved(Version::of_local(&meta)))
}

// ----------------------------------------------------------------------- SFTP

#[derive(Debug)]
pub enum EditCmd {
    Load {
        id: u64,
        path: String,
    },
    Save {
        id: u64,
        path: String,
        data: Vec<u8>,
        expect: Option<Version>,
        force: bool,
    },
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum EditEvent {
    Loaded {
        id: u64,
        bytes: Vec<u8>,
        version: Version,
        /// The first MAX_BYTES only (file was larger).
        truncated: bool,
    },
    LoadFailed {
        id: u64,
        error: String,
    },
    Saved {
        id: u64,
        version: Version,
    },
    Conflict {
        id: u64,
        current: Option<Version>,
        remote: Option<Vec<u8>>,
    },
    SaveFailed {
        id: u64,
        error: String,
        denied: bool,
    },
}

/// Runs one editor command; the answer goes to the UI as `SftpEvent::Edit`.
pub async fn run(sftp: Arc<SftpSession>, cmd: EditCmd, ev: Sink<SftpEvent>) {
    match cmd {
        EditCmd::Load { id, path } => {
            let r = load(&sftp, &path).await;
            ev.send(SftpEvent::Edit(match r {
                Ok((bytes, version, truncated)) => EditEvent::Loaded {
                    id,
                    bytes,
                    version,
                    truncated,
                },
                Err(e) => EditEvent::LoadFailed {
                    id,
                    error: friendly(&e),
                },
            }));
        }
        EditCmd::Save {
            id,
            path,
            data,
            expect,
            force,
        } => {
            let r = save(&sftp, &path, &data, expect, force).await;
            ev.send(SftpEvent::Edit(match r {
                Ok(SaveOutcome::Saved(version)) => EditEvent::Saved { id, version },
                Ok(SaveOutcome::Conflict(current, remote)) => EditEvent::Conflict {
                    id,
                    current,
                    remote,
                },
                Err(e) => {
                    let text = format!("{e:#}");
                    EditEvent::SaveFailed {
                        id,
                        denied: is_denied(&text),
                        error: friendly(&e),
                    }
                }
            }));
        }
    }
}

fn is_denied(text: &str) -> bool {
    let l = text.to_lowercase();
    l.contains("permission denied") || l.contains("access denied") || l.contains("read-only")
}

fn friendly(e: &anyhow::Error) -> String {
    let raw = format!("{e:#}");
    super::ssh::friendly_text(&raw).unwrap_or(raw)
}

async fn load(sftp: &SftpSession, path: &str) -> anyhow::Result<(Vec<u8>, Version, bool)> {
    let meta = sftp.metadata(path.to_string()).await?;
    if meta.is_dir() {
        anyhow::bail!(t!("Das ist ein Ordner."));
    }
    let mut file = sftp.open(path.to_string()).await?;
    let mut buf = Vec::new();
    (&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .await?;
    let truncated = buf.len() > MAX_BYTES;
    if truncated {
        buf.truncate(MAX_BYTES + 1);
    }
    Ok((buf, Version::of(&meta), truncated))
}

async fn save(
    sftp: &SftpSession,
    path: &str,
    data: &[u8],
    expect: Option<Version>,
    force: bool,
) -> anyhow::Result<SaveOutcome> {
    let current = match sftp.metadata(path.to_string()).await {
        Ok(m) => Some(Version::of(&m)),
        Err(_) => {
            // Missing is fine for a new file; anything else shows up when writing.
            if sftp.try_exists(path.to_string()).await.unwrap_or(false) {
                anyhow::bail!(t!(
                    "Die Datei lässt sich nicht prüfen (Zugriff verweigert?)"
                ));
            }
            None
        }
    };
    if !force && conflicts(expect, current) {
        let remote = match current {
            Some(_) => load(sftp, path).await.ok().map(|(b, _, _)| b),
            None => None,
        };
        return Ok(SaveOutcome::Conflict(current, remote));
    }
    // In place: permissions, owner and symlinks of the file stay as they are.
    let mut file = sftp
        .open_with_flags(
            path.to_string(),
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await?;
    file.write_all(data).await?;
    file.shutdown().await?;
    let meta = sftp.metadata(path.to_string()).await?;
    Ok(SaveOutcome::Saved(Version::of(&meta)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lf_roundtrip_keeps_final_newline() {
        let d = decode(b"a\nb\n").unwrap();
        assert_eq!(d.text, "a\nb");
        assert!(d.final_newline && d.eol == Eol::Lf && !d.mixed && !d.bom);
        assert_eq!(encode(&d.text, d.eol, d.final_newline, d.bom), b"a\nb\n");
        let d = decode(b"a\nb").unwrap();
        assert!(!d.final_newline);
        assert_eq!(encode(&d.text, d.eol, d.final_newline, d.bom), b"a\nb");
    }

    #[test]
    fn crlf_is_detected_and_restored() {
        let src = b"one\r\ntwo\r\nthree\r\n";
        let d = decode(src).unwrap();
        assert_eq!(d.eol, Eol::Crlf);
        assert_eq!(d.text, "one\ntwo\nthree");
        assert!(!d.mixed);
        assert_eq!(encode(&d.text, d.eol, d.final_newline, d.bom), src);
        // Edits keep the style.
        let edited = format!("{}\nfour", d.text);
        assert_eq!(
            encode(&edited, d.eol, d.final_newline, d.bom),
            b"one\r\ntwo\r\nthree\r\nfour\r\n"
        );
    }

    #[test]
    fn mixed_endings_use_the_majority() {
        let d = decode(b"a\r\nb\r\nc\nd").unwrap();
        assert!(d.mixed);
        assert_eq!(d.eol, Eol::Crlf);
        let d = decode(b"a\nb\nc\r\nd").unwrap();
        assert!(d.mixed);
        assert_eq!(d.eol, Eol::Lf);
    }

    #[test]
    fn bom_and_empty_files() {
        let d = decode(b"\xEF\xBB\xBFhi\n").unwrap();
        assert!(d.bom);
        assert_eq!(d.text, "hi");
        assert_eq!(
            encode(&d.text, d.eol, d.final_newline, d.bom),
            b"\xEF\xBB\xBFhi\n"
        );
        let d = decode(b"").unwrap();
        assert_eq!(d.text, "");
        assert!(d.final_newline);
        assert_eq!(encode("", d.eol, d.final_newline, d.bom), b"");
        assert_eq!(encode("x", d.eol, d.final_newline, d.bom), b"x\n");
    }

    #[test]
    fn refuses_binary_invalid_and_large() {
        assert_eq!(decode(b"ab\0cd"), Err(Refuse::Binary));
        assert_eq!(decode(&[0x66, 0xFF, 0xFE, 0x6f]), Err(Refuse::NotUtf8));
        let big = vec![b'a'; MAX_BYTES + 1];
        assert_eq!(decode(&big), Err(Refuse::TooLarge(MAX_BYTES + 1)));
        assert!(decode(&vec![b'a'; MAX_BYTES]).is_ok());
        // UTF-16 text has NULs.
        assert_eq!(decode(b"h\0i\0"), Err(Refuse::Binary));
    }

    #[test]
    fn umlauts_survive() {
        let src = "Grüße, 世界\n".as_bytes();
        let d = decode(src).unwrap();
        assert_eq!(encode(&d.text, d.eol, d.final_newline, d.bom), src);
    }

    #[test]
    fn conflict_detection() {
        let v = |m, s| Version {
            mtime: Some(m),
            size: s,
        };
        assert!(!conflicts(Some(v(10, 5)), Some(v(10, 5))));
        assert!(conflicts(Some(v(10, 5)), Some(v(11, 5))));
        assert!(conflicts(Some(v(10, 5)), Some(v(10, 6))));
        assert!(conflicts(Some(v(10, 5)), None), "deleted meanwhile");
        assert!(conflicts(None, Some(v(1, 1))), "appeared meanwhile");
        assert!(!conflicts(None, None), "new file stays new");
    }

    #[test]
    fn diff_summary_counts() {
        let d = diff_summary("a\nb\nc\nd", "a\nb\nX\nY\nd");
        assert_eq!(d.mine_lines, 4);
        assert_eq!(d.theirs_lines, 5);
        assert_eq!(d.only_mine, 1);
        assert_eq!(d.only_theirs, 2);
        assert_eq!(d.first_diff, Some(3));
        assert!(!d.identical());
        assert!(diff_summary("x\ny", "x\ny").identical());
        let d = diff_summary("a", "a\nb");
        assert_eq!(d.only_theirs, 1);
        assert_eq!(d.first_diff, Some(2));
    }

    #[test]
    fn local_save_detects_conflicts() {
        let dir = std::env::temp_dir().join(format!("aurora-edit-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.txt");
        std::fs::write(&p, "v1\n").unwrap();
        let (bytes, ver) = local_load(&p).unwrap();
        assert_eq!(bytes, b"v1\n");
        // Unchanged: saves.
        let SaveOutcome::Saved(v2) = local_save(&p, b"v2\n", Some(ver), false).unwrap() else {
            panic!("expected save");
        };
        // Changed by someone else (different size).
        std::fs::write(&p, "other change\n").unwrap();
        match local_save(&p, b"v3\n", Some(v2), false).unwrap() {
            SaveOutcome::Conflict(cur, content) => {
                assert!(cur.is_some());
                assert_eq!(content.unwrap(), b"other change\n");
            }
            SaveOutcome::Saved(_) => panic!("must conflict"),
        }
        // Forced overwrite.
        assert!(matches!(
            local_save(&p, b"v3\n", Some(v2), true).unwrap(),
            SaveOutcome::Saved(_)
        ));
        assert_eq!(std::fs::read(&p).unwrap(), b"v3\n");
        // New file.
        let n = dir.join("new.txt");
        assert!(matches!(
            local_save(&n, b"x", None, false).unwrap(),
            SaveOutcome::Saved(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
