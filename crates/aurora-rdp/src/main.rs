//! aurora-rdp — RDP helper process for Aurora.
//!
//! Protocol (stdio):
//! * stdin:  first line is a JSON `Connect` request (credentials never go through argv),
//!   followed by one JSON `Cmd` per line.
//! * stdout: binary frames `[tag: u8][len: u32 LE][payload]`, see [`Out`].
//!
//! Only the changed bounding box of each frame is sent, so idle sessions cost nothing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::sync::{Arc, Mutex};

use ironrdp::client::config::{ClipboardType, ConfigBuilder, Destination};
use ironrdp::client::rdp::{RdpClient, RdpInputEvent, RdpOutputEvent};
use ironrdp::cliprdr::backend::{ClipboardMessage, CliprdrBackend};
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp::input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use serde::Deserialize;
use tokio::sync::mpsc;

#[derive(Deserialize)]
struct Connect {
    host: String,
    port: u16,
    username: String,
    password: String,
    #[serde(default)]
    domain: String,
    width: u16,
    height: u16,
    /// Previously trusted `SHA256:` fingerprint of the server's TLS public key.
    #[serde(default)]
    known_fingerprint: Option<String>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "t")]
enum Cmd {
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
    /// Local clipboard text to offer to the remote session.
    ClipSet {
        text: String,
    },
    /// Ask the remote session for its clipboard text.
    ClipGet,
    CtrlAltDel,
    ReleaseAll,
    /// Answer to a certificate prompt.
    Trust {
        accept: bool,
    },
    Close,
}

/// Output message tags.
mod tag {
    pub const STATUS: u8 = 1;
    pub const CONNECTED: u8 = 2;
    pub const FRAME: u8 = 3;
    pub const CLIP_FROM_HOST: u8 = 4;
    pub const CLOSED: u8 = 5;
    /// Unknown or changed server key: `{"fingerprint": .., "previous": ..}`; expects `Trust`.
    pub const CERT: u8 = 6;
}

type Out = std::sync::mpsc::SyncSender<(u8, Vec<u8>)>;

fn send(out: &Out, tag: u8, payload: Vec<u8>) {
    let _ = out.send((tag, payload));
}

// ---------------------------------------------------------------- clipboard

#[derive(Default)]
struct ClipShared {
    local_text: Option<String>,
    input: Option<mpsc::UnboundedSender<RdpInputEvent>>,
}

impl ClipShared {
    fn post(&self, msg: ClipboardMessage) {
        if let Some(tx) = &self.input {
            let _ = tx.send(RdpInputEvent::Clipboard(msg));
        }
    }

    fn advertise(&self) {
        let formats = if self.local_text.is_some() {
            vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]
        } else {
            Vec::new()
        };
        self.post(ClipboardMessage::SendInitiateCopy(formats));
    }
}

/// Text-only clipboard bridge: remote copies are fetched and forwarded to the UI,
/// local text offered by the UI is served on request.
#[derive(Debug)]
struct Bridge {
    #[allow(dead_code)]
    tag: &'static str,
    shared: SharedClip,
    out: Out,
}

#[derive(Clone)]
struct SharedClip(Arc<Mutex<ClipShared>>);

impl std::fmt::Debug for SharedClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedClip")
    }
}

ironrdp::core::impl_as_any!(Bridge);

impl CliprdrBackend for Bridge {
    fn temporary_directory(&self) -> &str {
        ".cliprdr"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {}

    fn on_request_format_list(&mut self) {
        self.shared.0.lock().unwrap().advertise();
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        _capabilities: ClipboardGeneralCapabilityFlags,
    ) {
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        if available_formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT)
        {
            self.shared
                .0
                .lock()
                .unwrap()
                .post(ClipboardMessage::SendInitiatePaste(
                    ClipboardFormatId::CF_UNICODETEXT,
                ));
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let shared = self.shared.0.lock().unwrap();
        let response = match (
            &shared.local_text,
            request.format == ClipboardFormatId::CF_UNICODETEXT,
        ) {
            (Some(text), true) => FormatDataResponse::new_unicode_string(text),
            _ => FormatDataResponse::new_error(),
        };
        shared.post(ClipboardMessage::SendFormatData(response));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            return;
        }
        if let Ok(text) = response.to_unicode_string() {
            let text = text.trim_end_matches('\0').to_string();
            send(&self.out, tag::CLIP_FROM_HOST, text.into_bytes());
        }
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

// ---------------------------------------------------------------- certificate

/// OpenSSH-style fingerprint of a public key.
fn fingerprint(public_key: &[u8]) -> String {
    use base64::Engine;
    use sha2::Digest;
    let digest = sha2::Sha256::digest(public_key);
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

/// Trust on first use: silently accept the pinned key, otherwise ask the UI and wait.
fn install_verifier(known: Option<String>, out: Out, answers: std::sync::mpsc::Receiver<bool>) {
    let answers = Mutex::new(answers);
    let _ = ironrdp::client::rdp::SERVER_KEY_VERIFIER.set(Box::new(move |key: &[u8]| {
        let fp = fingerprint(key);
        if known.as_deref() == Some(fp.as_str()) {
            return true;
        }
        let msg = serde_json::json!({ "fingerprint": fp, "previous": known });
        send(&out, tag::CERT, msg.to_string().into_bytes());
        answers
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(300))
            .unwrap_or(false)
    }));
}

// ---------------------------------------------------------------- frames

/// Bounding box `(x, y, w, h)` of pixels that differ between `prev` and `next`.
fn dirty_rect(prev: &[u32], next: &[u32], width: usize) -> Option<(usize, usize, usize, usize)> {
    if prev.len() != next.len() || width == 0 {
        return Some((0, 0, width, next.len() / width.max(1)));
    }
    let height = next.len() / width;
    let row_differs =
        |y: usize| prev[y * width..(y + 1) * width] != next[y * width..(y + 1) * width];
    let top = (0..height).find(|&y| row_differs(y))?;
    let bottom = (top..height).rev().find(|&y| row_differs(y)).unwrap_or(top);
    let (mut left, mut right) = (width, 0);
    for y in top..=bottom {
        let row = y * width;
        if let Some(l) = (0..left).find(|&x| prev[row + x] != next[row + x]) {
            left = l;
        }
        if let Some(r) = (right..width)
            .rev()
            .find(|&x| prev[row + x] != next[row + x])
        {
            right = right.max(r);
        }
    }
    Some((left, top, right + 1 - left, bottom + 1 - top))
}

fn encode_frame(
    buf: &[u32],
    full_w: u16,
    full_h: u16,
    (x, y, w, h): (usize, usize, usize, usize),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + w * h * 4);
    for v in [full_w, full_h, x as u16, y as u16, w as u16, h as u16] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let stride = full_w as usize;
    for row in y..y + h {
        for px in &buf[row * stride + x..row * stride + x + w] {
            let [_, r, g, b] = px.to_be_bytes();
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

// ---------------------------------------------------------------- input

fn mouse_button(b: u8) -> Option<MouseButton> {
    match b {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        3 => Some(MouseButton::X1),
        4 => Some(MouseButton::X2),
        _ => None,
    }
}

fn operations(cmd: &Cmd) -> Vec<Operation> {
    match *cmd {
        Cmd::Key { code, down } => {
            let sc = Scancode::from_u16(code);
            vec![if down {
                Operation::KeyPressed(sc)
            } else {
                Operation::KeyReleased(sc)
            }]
        }
        Cmd::Unicode { ch, down } => {
            vec![if down {
                Operation::UnicodeKeyPressed(ch)
            } else {
                Operation::UnicodeKeyReleased(ch)
            }]
        }
        Cmd::Move { x, y } => vec![Operation::MouseMove(MousePosition { x, y })],
        Cmd::Button { b, down } => mouse_button(b)
            .map(|b| {
                vec![if down {
                    Operation::MouseButtonPressed(b)
                } else {
                    Operation::MouseButtonReleased(b)
                }]
            })
            .unwrap_or_default(),
        Cmd::Wheel { vertical, units } => vec![Operation::WheelRotations(WheelRotations {
            is_vertical: vertical,
            rotation_units: units,
        })],
        Cmd::CtrlAltDel => {
            let (ctrl, alt, del) = (
                Scancode::from_u16(0x1D),
                Scancode::from_u16(0x38),
                Scancode::from_u16(0xE053),
            );
            vec![
                Operation::KeyPressed(ctrl),
                Operation::KeyPressed(alt),
                Operation::KeyPressed(del),
                Operation::KeyReleased(del),
                Operation::KeyReleased(alt),
                Operation::KeyReleased(ctrl),
            ]
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- main

fn main() {
    // Diagnostics go to stderr only when explicitly requested.
    if let Ok(filter) = std::env::var("AURORA_RDP_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(std::io::stderr)
            .try_init();
    }
    let (out_tx, out_rx) = std::sync::mpsc::sync_channel::<(u8, Vec<u8>)>(8);

    // Single writer thread owns stdout.
    let writer = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        while let Ok((tag, payload)) = out_rx.recv() {
            let ok = lock.write_all(&[tag]).is_ok()
                && lock
                    .write_all(&(payload.len() as u32).to_le_bytes())
                    .is_ok()
                && lock.write_all(&payload).is_ok()
                && lock.flush().is_ok();
            if !ok || tag == tag::CLOSED {
                break;
            }
        }
    });

    let error = run(out_tx.clone())
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    send(&out_tx, tag::CLOSED, error.into_bytes());
    drop(out_tx);
    let _ = writer.join();
}

fn run(out: Out) -> anyhow::Result<()> {
    let mut first = String::new();
    if std::io::stdin().read_line(&mut first)? == 0 {
        anyhow::bail!("keine Verbindungsdaten");
    }
    let connect: Connect = serde_json::from_str(&first)?;

    let shared = SharedClip(Arc::new(Mutex::new(ClipShared::default())));
    let bridge_shared = shared.clone();
    let bridge_out = out.clone();
    let mut builder = ConfigBuilder::new()
        .with_destination(Destination::from_parts(connect.host.clone(), connect.port))
        .with_username(connect.username)
        .with_password(connect.password)
        .with_client_name("aurora")
        .with_client_dir("C:\\Windows\\System32\\mstscax.dll")
        .with_client_build(0)
        .with_platform(ironrdp::pdu::rdp::capability_sets::MajorPlatformType::UNSPECIFIED)
        .with_desktop_width(connect.width.clamp(640, 8192))
        .with_desktop_height(connect.height.clamp(480, 8192))
        .with_server_pointer(true)
        .with_pointer_software_rendering(true)
        // Slow-path bulk decompression is not implemented in IronRDP yet; xrdp uses it.
        .with_compression(false)
        .with_autologon(true)
        .with_clipboard(ClipboardType::Disable)
        .with_static_channel(move |_props| {
            Some(ironrdp::cliprdr::Cliprdr::new(Box::new(Bridge {
                tag: "aurora",
                shared: bridge_shared.clone(),
                out: bridge_out.clone(),
            })
                as Box<dyn CliprdrBackend>))
        });
    if !connect.domain.is_empty() {
        builder = builder.with_domain(connect.domain);
    }
    let config = builder.build()?;

    let (trust_tx, trust_rx) = std::sync::mpsc::channel::<bool>();
    install_verifier(connect.known_fingerprint.clone(), out.clone(), trust_rx);

    send(
        &out,
        tag::STATUS,
        format!("Verbinde mit {}:{}…", connect.host, connect.port).into_bytes(),
    );

    let (event_tx, mut event_rx) = mpsc::channel::<RdpOutputEvent>(4);
    let client = RdpClient::new(config, event_tx);
    let input = client.input_sender();
    shared.0.lock().unwrap().input = Some(input.clone());

    // stdin commands -> RDP input events
    let input_cmds = input.clone();
    let shared_cmds = shared.clone();
    std::thread::spawn(move || {
        let mut db = Database::new();
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            let Ok(cmd) = serde_json::from_str::<Cmd>(&line) else {
                continue;
            };
            let event = match &cmd {
                Cmd::Close => {
                    let _ = input_cmds.send(RdpInputEvent::FastPath(db.release_all()));
                    let _ = input_cmds.send(RdpInputEvent::Close);
                    return;
                }
                Cmd::ReleaseAll => Some(RdpInputEvent::FastPath(db.release_all())),
                Cmd::Trust { accept } => {
                    let _ = trust_tx.send(*accept);
                    None
                }
                Cmd::Resize { w, h } => Some(RdpInputEvent::Resize {
                    width: *w,
                    height: *h,
                    scale_factor: 100,
                    physical_size: None,
                }),
                Cmd::ClipSet { text } => {
                    let mut s = shared_cmds.0.lock().unwrap();
                    s.local_text = Some(text.clone());
                    s.advertise();
                    None
                }
                Cmd::ClipGet => {
                    shared_cmds
                        .0
                        .lock()
                        .unwrap()
                        .post(ClipboardMessage::SendInitiatePaste(
                            ClipboardFormatId::CF_UNICODETEXT,
                        ));
                    None
                }
                other => {
                    let events = db.apply(operations(other));
                    (!events.is_empty()).then_some(RdpInputEvent::FastPath(events))
                }
            };
            if let Some(ev) = event
                && input_cmds.send(ev).is_err()
            {
                return;
            }
        }
        // stdin closed: the UI went away.
        let _ = input_cmds.send(RdpInputEvent::Close);
    });

    // `RdpClient::run` is not `Send`, so it gets its own single-threaded runtime.
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(client.run());
        }
    });

    let mut prev: Vec<u32> = Vec::new();
    let mut connected = false;
    let mut result = Ok(());
    while let Some(ev) = event_rx.blocking_recv() {
        match ev {
            RdpOutputEvent::Image {
                buffer,
                width,
                height,
            } => {
                let (w, h) = (width.get(), height.get());
                if !connected {
                    connected = true;
                    let mut p = w.to_le_bytes().to_vec();
                    p.extend_from_slice(&h.to_le_bytes());
                    send(&out, tag::CONNECTED, p);
                }
                if let Some(rect) = dirty_rect(&prev, &buffer, w as usize) {
                    send(&out, tag::FRAME, encode_frame(&buffer, w, h, rect));
                }
                prev = buffer;
            }
            RdpOutputEvent::ConnectionFailure(e) => {
                result = Err(anyhow::anyhow!("{e}"));
                break;
            }
            RdpOutputEvent::Terminated(r) => {
                if let Err(e) = r {
                    result = Err(anyhow::anyhow!("{e}"));
                }
                break;
            }
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_rect_finds_changed_box() {
        let w = 4;
        let prev = vec![0u32; 16];
        let mut next = prev.clone();
        assert_eq!(dirty_rect(&prev, &next, w), None);
        next[1 * w + 1] = 7;
        next[2 * w + 3] = 9;
        assert_eq!(dirty_rect(&prev, &next, w), Some((1, 1, 3, 2)));
        assert_eq!(dirty_rect(&[], &next, w), Some((0, 0, 4, 4)));
    }

    #[test]
    fn frame_encoding_is_rgba() {
        let buf = vec![0x00FF8040u32; 4];
        let f = encode_frame(&buf, 2, 2, (1, 0, 1, 2));
        assert_eq!(&f[..12], &[2, 0, 2, 0, 1, 0, 0, 0, 1, 0, 2, 0]);
        assert_eq!(&f[12..], &[0xFF, 0x80, 0x40, 255, 0xFF, 0x80, 0x40, 255]);
    }

    #[test]
    fn commands_parse() {
        let c: Cmd = serde_json::from_str(r#"{"t":"Key","code":57427,"down":true}"#).unwrap();
        assert_eq!(
            c,
            Cmd::Key {
                code: 0xE053,
                down: true
            }
        );
        assert_eq!(operations(&Cmd::CtrlAltDel).len(), 6);
        let t: Cmd = serde_json::from_str(r#"{"t":"Trust","accept":true}"#).unwrap();
        assert_eq!(t, Cmd::Trust { accept: true });
        assert!(fingerprint(b"key").starts_with("SHA256:"));
    }
}
