//! Session recording in the asciinema v2 format (`.cast`): a JSON header line, then one
//! `[seconds, "o", "text"]` line per output chunk. Recordings can be replayed in a terminal
//! tab or with `asciinema play`.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc;

use super::{Sink, TermCmd, TermEvent};

/// Where recordings go: `Documents/Aurora/Recordings`, else the app data folder.
pub fn dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| {
            u.document_dir()
                .map(|d| d.join("Aurora").join("Recordings"))
        })
        .unwrap_or_else(|| crate::store::data_dir().join("recordings"))
}

/// Shows the recordings folder in the file manager (creates it first).
pub fn open_folder() -> Result<()> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    #[cfg(windows)]
    std::process::Command::new("explorer.exe").arg(&d).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&d).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(&d).spawn()?;
    Ok(())
}

/// `.cast` files in the recordings folder, newest first: (path, size, modified).
pub fn list() -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out: Vec<_> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "cast"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.len(), m.modified().ok()?))
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.2));
    out
}

/// File name for a new recording: `web-01_2026-10-16_14-03-22.cast`.
fn file_name(title: &str) -> String {
    let safe: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_matches('_');
    let safe = if safe.is_empty() { "session" } else { safe };
    format!(
        "{}_{}.cast",
        safe.chars().take(48).collect::<String>(),
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
    )
}

/// Writes one session to a `.cast` file.
pub struct Recorder {
    pub path: PathBuf,
    out: BufWriter<File>,
    start: Instant,
    last_flush: Instant,
    /// Bytes of a UTF-8 sequence split across two chunks.
    pending: Vec<u8>,
}

impl Recorder {
    pub fn start(title: &str, cols: u16, rows: u16) -> Result<Self> {
        let dir = dir();
        std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
        let path = dir.join(file_name(title));
        let mut out = BufWriter::new(File::create(&path)?);
        let header = serde_json::json!({
            "version": 2,
            "width": cols.max(1),
            "height": rows.max(1),
            "timestamp": chrono::Utc::now().timestamp(),
            "title": title,
            "env": { "TERM": "xterm-256color" },
        });
        writeln!(out, "{header}")?;
        Ok(Self {
            path,
            out,
            start: Instant::now(),
            last_flush: Instant::now(),
            pending: Vec::new(),
        })
    }

    fn line(&mut self, kind: &str, data: &str) {
        let t = self.start.elapsed().as_secs_f64();
        let _ = writeln!(
            self.out,
            "[{t:.6}, {}, {}]",
            serde_json::Value::from(kind),
            serde_json::Value::from(data)
        );
        if self.last_flush.elapsed() > Duration::from_secs(2) {
            let _ = self.out.flush();
            self.last_flush = Instant::now();
        }
    }

    pub fn output(&mut self, data: &[u8]) {
        self.pending.extend_from_slice(data);
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            // An incomplete sequence at the end waits for the next chunk.
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => self.pending.len(),
        };
        if valid == 0 {
            return;
        }
        let chunk: Vec<u8> = self.pending.drain(..valid).collect();
        let text = String::from_utf8_lossy(&chunk).into_owned();
        self.line("o", &text);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.line("r", &format!("{cols}x{rows}"));
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

/// One output event of a recording.
#[derive(Debug, PartialEq)]
pub struct Frame {
    pub time: f64,
    pub data: String,
}

/// Reads a `.cast` file: (width, height, output frames). Other event kinds are skipped.
pub fn read(path: &Path) -> Result<(u16, u16, Vec<Frame>)> {
    let mut lines = BufReader::new(File::open(path)?).lines();
    let header: serde_json::Value = serde_json::from_str(
        &lines
            .next()
            .ok_or_else(|| anyhow!("{}", t!("Leere Datei")))??,
    )?;
    if header["version"].as_u64() != Some(2) {
        return Err(anyhow!("{}", t!("Keine asciinema-v2-Aufnahme")));
    }
    let w = header["width"].as_u64().unwrap_or(80) as u16;
    let h = header["height"].as_u64().unwrap_or(24) as u16;
    let mut frames = Vec::new();
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v[1].as_str() == Some("o")
            && let (Some(time), Some(data)) = (v[0].as_f64(), v[2].as_str())
        {
            frames.push(Frame {
                time,
                data: data.to_string(),
            });
        }
    }
    Ok((w, h, frames))
}

/// Plays a recording into a terminal tab. Space pauses, the tab's reconnect starts over.
/// Pauses longer than two seconds are shortened.
pub fn spawn_replay(path: PathBuf, events: Sink<TermEvent>) -> mpsc::UnboundedSender<TermCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel::<TermCmd>();
    std::thread::spawn(move || {
        let (_, _, frames) = match read(&path) {
            Ok(r) => r,
            Err(e) => {
                events.send(TermEvent::Closed(Some(format!("{e:#}"))));
                return;
            }
        };
        events.send(TermEvent::Connected);
        let mut paused = false;
        let mut prev = 0.0f64;
        for f in frames {
            let mut wait = (f.time - prev).clamp(0.0, 2.0);
            prev = f.time;
            loop {
                match rx.try_recv() {
                    Ok(TermCmd::Close) | Err(mpsc::error::TryRecvError::Disconnected) => return,
                    Ok(TermCmd::Input(b)) if b == b" " => paused = !paused,
                    Ok(_) => {}
                    Err(mpsc::error::TryRecvError::Empty) => {
                        if !paused && wait <= 0.0 {
                            break;
                        }
                        let step = if paused { 0.05 } else { wait.min(0.05) };
                        std::thread::sleep(Duration::from_secs_f64(step));
                        if !paused {
                            wait -= step;
                        }
                    }
                }
            }
            events.send(TermEvent::Output(f.data.into_bytes()));
        }
        events.send(TermEvent::Closed(None));
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_read_back() {
        let dir = std::env::temp_dir().join(format!("aurora-cast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.cast");
        {
            let mut r = Recorder {
                path: path.clone(),
                out: BufWriter::new(File::create(&path).unwrap()),
                start: Instant::now(),
                last_flush: Instant::now(),
                pending: Vec::new(),
            };
            writeln!(r.out, r#"{{"version": 2, "width": 100, "height": 30}}"#).unwrap();
            r.output(b"hello \xc3");
            r.output(b"\xa4\r\n");
            r.output("\"quoted\"".as_bytes());
        }
        let (w, h, frames) = read(&path).unwrap();
        assert_eq!((w, h), (100, 30));
        let text: Vec<&str> = frames.iter().map(|f| f.data.as_str()).collect();
        assert_eq!(text, vec!["hello ", "ä\r\n", "\"quoted\""]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(file_name("root@web 01").starts_with("root_web_01_"));
    }
}
