//! Embedded RDP: talks to the `aurora-rdp` helper process (IronRDP) over stdio.
//!
//! The helper streams changed screen regions; they are merged into a shared framebuffer
//! here so the UI only uploads the dirty part of the texture once per frame.

use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use super::Sink;

#[derive(Serialize)]
pub struct RdpConnect {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub width: u16,
    pub height: u16,
    pub known_fingerprint: Option<String>,
    /// Local folder to show as a drive in the session; `None` = no drive channel at all.
    pub drive: Option<RdpDriveCfg>,
}

/// Drive redirection settings sent to the helper.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct RdpDriveCfg {
    pub path: String,
    pub name: String,
    pub writable: bool,
}

/// Commands understood by the helper (one JSON object per line).
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "t")]
pub enum RdpCmd {
    Key {
        code: u16,
        down: bool,
    },
    Unicode {
        ch: char,
        down: bool,
    },
    Move {
        x: u16,
        y: u16,
    },
    Button {
        b: u8,
        down: bool,
    },
    Wheel {
        vertical: bool,
        units: i16,
    },
    Resize {
        w: u16,
        h: u16,
    },
    ClipSet {
        text: String,
    },
    ClipGet,
    /// Offer local files/folders to the host through the clipboard.
    OfferFiles {
        paths: Vec<String>,
    },
    /// Save the files on the host's clipboard into `dir`.
    FetchFiles {
        dir: String,
    },
    /// Show a local folder as a drive (only if the session started with a drive channel).
    DriveShare {
        path: String,
        name: String,
        writable: bool,
    },
    /// Remove the drive from the session.
    DriveUnshare,
    CtrlAltDel,
    ReleaseAll,
    Trust {
        accept: bool,
    },
    Close,
}

#[derive(Debug)]
pub enum RdpEvent {
    Status(String),
    Connected,
    /// The shared framebuffer has new content.
    Frame,
    ClipFromHost(String),
    Files(FileEv),
    Drive(DriveEv),
    /// The server key is unknown or changed; answer with [`RdpCmd::Trust`].
    Cert {
        fingerprint: String,
        previous: Option<String>,
    },
    Closed(Option<String>),
}

/// File transfer events of the helper (see `FileEv` in `crates/aurora-rdp`).
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum FileEv {
    Offered {
        files: usize,
        bytes: u64,
        #[serde(default)]
        skipped: Vec<String>,
    },
    Accepted,
    Progress {
        up: bool,
        done: u64,
        total: u64,
    },
    Remote {
        files: bool,
    },
    Saved {
        dir: String,
        files: usize,
    },
    Error {
        code: String,
        #[serde(default)]
        detail: String,
    },
}

/// Drive redirection events of the helper (see `DriveEv` in `crates/aurora-rdp`).
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum DriveEv {
    State {
        shared: bool,
        #[serde(default)]
        name: String,
        #[serde(default)]
        writable: bool,
    },
    Activity {
        #[serde(default)]
        read: u64,
        #[serde(default)]
        written: u64,
        #[serde(default)]
        denied: u64,
        #[serde(default)]
        last_denied: String,
    },
    Error {
        #[serde(default)]
        detail: String,
    },
}

#[derive(Default)]
pub struct FrameBuffer {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Union of regions changed since the UI last uploaded: (x, y, w, h).
    pub dirty: Option<(usize, usize, usize, usize)>,
}

impl FrameBuffer {
    fn apply(&mut self, payload: &[u8]) {
        if payload.len() < 12 {
            return;
        }
        let v = |i: usize| u16::from_le_bytes([payload[i * 2], payload[i * 2 + 1]]) as usize;
        let (fw, fh, x, y, w, h) = (v(0), v(1), v(2), v(3), v(4), v(5));
        let pixels = &payload[12..];
        if x + w > fw || y + h > fh || pixels.len() < w * h * 4 {
            return;
        }
        if (fw, fh) != (self.width, self.height) {
            self.width = fw;
            self.height = fh;
            self.rgba = vec![0; fw * fh * 4];
            self.dirty = Some((0, 0, fw, fh));
        }
        for row in 0..h {
            let dst = ((y + row) * fw + x) * 4;
            self.rgba[dst..dst + w * 4].copy_from_slice(&pixels[row * w * 4..(row + 1) * w * 4]);
        }
        self.dirty = Some(match self.dirty {
            None => (x, y, w, h),
            Some((dx, dy, dw, dh)) => {
                let (l, t) = (dx.min(x), dy.min(y));
                let (r, b) = ((dx + dw).max(x + w), (dy + dh).max(y + h));
                (l, t, r - l, b - t)
            }
        });
    }
}

/// Cloneable command channel to the helper (usable from other threads).
#[derive(Clone)]
pub struct RdpSender(Arc<Mutex<Option<BufWriter<ChildStdin>>>>);

impl RdpSender {
    pub fn send(&self, cmd: &RdpCmd) {
        let Ok(line) = serde_json::to_string(cmd) else {
            return;
        };
        let mut guard = self.0.lock().unwrap();
        if let Some(w) = guard.as_mut() {
            let ok = writeln!(w, "{line}").is_ok() && w.flush().is_ok();
            if !ok {
                *guard = None;
            }
        }
    }
}

pub struct RdpHandle {
    stdin: Arc<Mutex<Option<BufWriter<ChildStdin>>>>,
    child: Child,
}

impl RdpHandle {
    pub fn sender(&self) -> RdpSender {
        RdpSender(self.stdin.clone())
    }

    pub fn send(&self, cmd: &RdpCmd) {
        self.sender().send(cmd);
    }

    pub fn close(&mut self) {
        self.send(&RdpCmd::Close);
        self.stdin.lock().unwrap().take();
    }
}

impl Drop for RdpHandle {
    fn drop(&mut self) {
        self.close();
        // Give the helper a moment for a graceful disconnect, then make sure it is gone.
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Finds the helper binary: `AURORA_RDP_HELPER`, next to the executable, or a dev build.
pub fn helper_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "aurora-rdp.exe"
    } else {
        "aurora-rdp"
    };
    if let Ok(p) = std::env::var("AURORA_RDP_HELPER") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(name));
        if cfg!(target_os = "linux") {
            candidates.push(dir.join("../lib/aurora-term").join(name));
        }
    }
    // Dev builds only: never pick up a binary relative to the working directory in a
    // release build (it would receive the user's RDP password).
    if cfg!(debug_assertions) {
        for profile in ["release", "debug"] {
            candidates.push(
                PathBuf::from("crates/aurora-rdp/target")
                    .join(profile)
                    .join(name),
            );
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn spawn(
    connect: RdpConnect,
    events: Sink<RdpEvent>,
    fb: Arc<Mutex<FrameBuffer>>,
) -> Result<RdpHandle> {
    let helper =
        helper_path().ok_or_else(|| anyhow!(t!("RDP-Komponente (aurora-rdp) nicht gefunden")))?;
    let mut cmd = Command::new(&helper);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("{} starten", helper.display()))?;
    let mut stdin = BufWriter::new(child.stdin.take().context("stdin")?);
    let mut stdout = child.stdout.take().context("stdout")?;

    // Credentials go over the pipe, never through argv or the environment.
    writeln!(stdin, "{}", serde_json::to_string(&connect)?)?;
    stdin.flush()?;

    std::thread::spawn(move || {
        let mut closed_sent = false;
        loop {
            let mut header = [0u8; 5];
            if stdout.read_exact(&mut header).is_err() {
                break;
            }
            let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
            // Largest legitimate message is a full 8K frame.
            if len > 8192 * 8192 * 4 + 12 {
                break;
            }
            let mut payload = vec![0u8; len];
            if stdout.read_exact(&mut payload).is_err() {
                break;
            }
            let text = || String::from_utf8_lossy(&payload).to_string();
            match header[0] {
                1 => events.send(RdpEvent::Status(text())),
                2 => events.send(RdpEvent::Connected),
                3 => {
                    fb.lock().unwrap().apply(&payload);
                    events.send(RdpEvent::Frame);
                }
                4 => events.send(RdpEvent::ClipFromHost(text())),
                6 => {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        events.send(RdpEvent::Cert {
                            fingerprint: v["fingerprint"].as_str().unwrap_or_default().to_string(),
                            previous: v["previous"].as_str().map(str::to_string),
                        });
                    }
                }
                7 => {
                    if let Ok(ev) = serde_json::from_slice::<FileEv>(&payload) {
                        events.send(RdpEvent::Files(ev));
                    }
                }
                8 => {
                    if let Ok(ev) = serde_json::from_slice::<DriveEv>(&payload) {
                        events.send(RdpEvent::Drive(ev));
                    }
                }
                5 => {
                    let msg = text();
                    events.send(RdpEvent::Closed((!msg.is_empty()).then_some(msg)));
                    closed_sent = true;
                    break;
                }
                _ => {}
            }
        }
        if !closed_sent {
            events.send(RdpEvent::Closed(Some(
                t!("RDP-Komponente unerwartet beendet").into(),
            )));
        }
    });

    Ok(RdpHandle {
        stdin: Arc::new(Mutex::new(Some(stdin))),
        child,
    })
}

/// PC/AT set-1 scancode for a physical key (`0xE0xx` = extended).
pub fn scancode(key: egui::Key) -> Option<u16> {
    use egui::Key::*;
    Some(match key {
        Escape => 0x01,
        Num1 => 0x02,
        Num2 => 0x03,
        Num3 => 0x04,
        Num4 => 0x05,
        Num5 => 0x06,
        Num6 => 0x07,
        Num7 => 0x08,
        Num8 => 0x09,
        Num9 => 0x0A,
        Num0 => 0x0B,
        Minus => 0x0C,
        Equals => 0x0D,
        Backspace => 0x0E,
        Tab => 0x0F,
        Q => 0x10,
        W => 0x11,
        E => 0x12,
        R => 0x13,
        T => 0x14,
        Y => 0x15,
        U => 0x16,
        I => 0x17,
        O => 0x18,
        P => 0x19,
        OpenBracket => 0x1A,
        CloseBracket => 0x1B,
        Enter => 0x1C,
        A => 0x1E,
        S => 0x1F,
        D => 0x20,
        F => 0x21,
        G => 0x22,
        H => 0x23,
        J => 0x24,
        K => 0x25,
        L => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backtick => 0x29,
        Backslash => 0x2B,
        Z => 0x2C,
        X => 0x2D,
        C => 0x2E,
        V => 0x2F,
        B => 0x30,
        N => 0x31,
        M => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        Space => 0x39,
        F1 => 0x3B,
        F2 => 0x3C,
        F3 => 0x3D,
        F4 => 0x3E,
        F5 => 0x3F,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        F11 => 0x57,
        F12 => 0x58,
        Home => 0xE047,
        ArrowUp => 0xE048,
        PageUp => 0xE049,
        ArrowLeft => 0xE04B,
        ArrowRight => 0xE04D,
        End => 0xE04F,
        ArrowDown => 0xE050,
        PageDown => 0xE051,
        Insert => 0xE052,
        Delete => 0xE053,
        _ => return None,
    })
}

pub const SC_LCTRL: u16 = 0x1D;
pub const SC_LSHIFT: u16 = 0x2A;
pub const SC_LALT: u16 = 0x38;
pub const SC_LWIN: u16 = 0xE05B;
pub const SC_ENTER: u16 = 0x1C;

/// Keys that produce text; without Ctrl/Alt they are sent as Unicode so the
/// remote keyboard layout does not matter.
pub fn is_text_key(key: egui::Key) -> bool {
    use egui::Key::*;
    let name = key.name();
    name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric())
        || matches!(
            key,
            Space
                | Minus
                | Equals
                | OpenBracket
                | CloseBracket
                | Semicolon
                | Quote
                | Backtick
                | Backslash
                | Comma
                | Period
                | Slash
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_merges_patches() {
        let mut fb = FrameBuffer::default();
        let patch = |x: u16, y: u16, w: u16, h: u16| {
            let mut p = Vec::new();
            for v in [4u16, 4, x, y, w, h] {
                p.extend_from_slice(&v.to_le_bytes());
            }
            p.extend(std::iter::repeat_n(9u8, (w * h * 4) as usize));
            p
        };
        fb.apply(&patch(1, 1, 2, 1));
        assert_eq!(fb.dirty, Some((0, 0, 4, 4)));
        fb.dirty = None;
        fb.apply(&patch(0, 3, 1, 1));
        fb.apply(&patch(3, 0, 1, 1));
        assert_eq!(fb.dirty, Some((0, 0, 4, 4)));
        assert_eq!(fb.rgba[(4 + 1) * 4], 9);
        // Out-of-bounds patches are ignored.
        let before = fb.rgba.clone();
        fb.apply(&patch(3, 3, 2, 2));
        assert_eq!(fb.rgba, before);
    }

    #[test]
    fn cmd_json_matches_helper() {
        let j = serde_json::to_string(&RdpCmd::Key {
            code: 0xE053,
            down: true,
        })
        .unwrap();
        assert_eq!(j, r#"{"t":"Key","code":57427,"down":true}"#);
        assert_eq!(scancode(egui::Key::A), Some(0x1E));
        assert!(is_text_key(egui::Key::A) && !is_text_key(egui::Key::Enter));
        let j = serde_json::to_string(&RdpCmd::OfferFiles {
            paths: vec!["/tmp/a".into()],
        })
        .unwrap();
        assert_eq!(j, r#"{"t":"OfferFiles","paths":["/tmp/a"]}"#);
        let ev: FileEv =
            serde_json::from_str(r#"{"ev":"progress","up":true,"done":1,"total":2}"#).unwrap();
        assert_eq!(
            ev,
            FileEv::Progress {
                up: true,
                done: 1,
                total: 2
            }
        );
        let j = serde_json::to_string(&RdpCmd::DriveShare {
            path: "/tmp/x".into(),
            name: "X".into(),
            writable: false,
        })
        .unwrap();
        assert_eq!(
            j,
            r#"{"t":"DriveShare","path":"/tmp/x","name":"X","writable":false}"#
        );
        assert_eq!(
            serde_json::to_string(&RdpCmd::DriveUnshare).unwrap(),
            r#"{"t":"DriveUnshare"}"#
        );
        let ev: DriveEv =
            serde_json::from_str(r#"{"ev":"state","shared":true,"name":"X","writable":false}"#)
                .unwrap();
        assert_eq!(
            ev,
            DriveEv::State {
                shared: true,
                name: "X".into(),
                writable: false
            }
        );
        let ev: FileEv = serde_json::from_str(r#"{"ev":"saved","dir":"/d","files":3}"#).unwrap();
        assert_eq!(
            ev,
            FileEv::Saved {
                dir: "/d".into(),
                files: 3
            }
        );
    }
}
