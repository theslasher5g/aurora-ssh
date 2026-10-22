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
    /// Starts a recording; the host id goes into the header (for the session history).
    pub fn start_for(title: &str, cols: u16, rows: u16, host: Option<uuid::Uuid>) -> Result<Self> {
        let dir = dir();
        std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
        // Never overwrite: two sessions starting in the same second get `-2`, `-3` …
        let base = file_name(title);
        let stem = base.trim_end_matches(".cast").to_string();
        let mut n = 1;
        let (path, file) = loop {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{stem}-{n}.cast")
            };
            let path = dir.join(name);
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            // Raw terminal output: readable by the owner only.
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(f) => break (path, f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
                Err(e) => return Err(e.into()),
            }
        };
        let mut out = BufWriter::new(file);
        let mut header = serde_json::json!({
            "version": 2,
            "width": cols.max(1),
            "height": rows.max(1),
            "timestamp": chrono::Utc::now().timestamp(),
            "title": title,
            "env": { "TERM": "xterm-256color" },
        });
        if let Some(h) = host {
            header["aurora_host"] = serde_json::Value::from(h.to_string());
        }
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

    /// A marker with a note at the current time (asciicast v2 `m` event).
    pub fn marker(&mut self, label: &str) {
        self.line("m", label);
        let _ = self.out.flush();
    }

    /// Seconds since the recording started.
    pub fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
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

/// A whole recording.
#[derive(Debug, Default)]
pub struct Cast {
    pub width: u16,
    pub height: u16,
    pub title: String,
    /// Unix time of the start.
    pub timestamp: i64,
    pub host: Option<String>,
    pub frames: Vec<Frame>,
    /// Markers stored in the file: (seconds, note).
    pub markers: Vec<(f64, String)>,
}

/// Reads a `.cast` file: (width, height, output frames). Other event kinds are skipped.
pub fn read(path: &Path) -> Result<(u16, u16, Vec<Frame>)> {
    let c = read_full(path)?;
    Ok((c.width, c.height, c.frames))
}

/// Reads a `.cast` file with header data and markers.
pub fn read_full(path: &Path) -> Result<Cast> {
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
    let mut markers = Vec::new();
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
        } else if v[1].as_str() == Some("m")
            && let (Some(time), Some(label)) = (v[0].as_f64(), v[2].as_str())
        {
            markers.push((time, label.to_string()));
        }
    }
    Ok(Cast {
        width: w,
        height: h,
        title: header["title"].as_str().unwrap_or_default().to_string(),
        timestamp: header["timestamp"].as_i64().unwrap_or(0),
        host: header["aurora_host"].as_str().map(str::to_string),
        frames,
        markers,
    })
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
            r.marker("deploy \"start\"");
            r.output("\"quoted\"".as_bytes());
        }
        let cast = read_full(&path).unwrap();
        assert_eq!(cast.markers.len(), 1);
        assert_eq!(cast.markers[0].1, "deploy \"start\"");
        let (w, h, frames) = read(&path).unwrap();
        assert_eq!((w, h), (100, 30));
        let text: Vec<&str> = frames.iter().map(|f| f.data.as_str()).collect();
        assert_eq!(text, vec!["hello ", "ä\r\n", "\"quoted\""]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(file_name("root@web 01").starts_with("root_web_01_"));
    }

    #[test]
    fn old_casts_without_markers_still_read() {
        let dir = std::env::temp_dir().join(format!("aurora-cast-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.cast");
        std::fs::write(
            &path,
            "{\"version\": 2, \"width\": 80, \"height\": 24, \"timestamp\": 1700000000, \"title\": \"web\"}\n[0.5, \"o\", \"hi\"]\n[0.9, \"x\", \"?\"]\n",
        )
        .unwrap();
        let c = read_full(&path).unwrap();
        assert_eq!(
            (c.frames.len(), c.markers.len(), c.timestamp),
            (1, 0, 1700000000)
        );
        assert_eq!(c.title, "web");
        assert!(c.host.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
