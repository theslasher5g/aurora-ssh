//! aurora-rdp — RDP helper process for Aurora.
//!
//! Protocol (stdio):
//! * stdin:  first line is a JSON `Connect` request (credentials never go through argv),
//!   followed by one JSON `Cmd` per line.
//! * stdout: binary frames `[tag: u8][len: u32 LE][payload]`, see [`Out`].
//!
//! Only the changed bounding box of each frame is sent, so idle sessions cost nothing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod files;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ironrdp::client::config::{ClipboardType, ConfigBuilder, Destination};
use ironrdp::client::rdp::{RdpClient, RdpInputEvent, RdpOutputEvent};
use ironrdp::cliprdr::backend::{ClipboardMessage, CliprdrBackend};
use ironrdp::cliprdr::pdu::{
    ClipboardFileAttributes, ClipboardFormat, ClipboardFormatId, ClipboardFormatName,
    ClipboardGeneralCapabilityFlags, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor, FormatDataRequest, FormatDataResponse, LockDataId,
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
    /// Offer local files and folders to the remote session (clipboard file list).
    OfferFiles {
        paths: Vec<String>,
    },
    /// Save the files on the remote clipboard into `dir` (created if missing).
    FetchFiles {
        dir: String,
    },
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
    /// File transfer event, JSON with an `ev` field (see [`super::FileEv`]).
    pub const FILES: u8 = 7;
}

/// File transfer events for the UI (tag [`tag::FILES`]).
#[derive(serde::Serialize, Debug, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
enum FileEv {
    /// The list was announced to the host.
    Offered {
        files: usize,
        bytes: u64,
        skipped: Vec<String>,
    },
    /// The host accepted the announced list: time to paste.
    Accepted,
    Progress {
        up: bool,
        done: u64,
        total: u64,
    },
    /// Whether the remote clipboard currently holds files.
    Remote {
        files: bool,
    },
    Saved {
        dir: String,
        files: usize,
    },
    /// `code`: unsupported, empty, rejected, busy, no_files, list, read, remote_read,
    /// write, too_big.
    Error {
        code: String,
        detail: String,
    },
}

fn file_event(out: &Out, ev: &FileEv) {
    if let Ok(j) = serde_json::to_vec(ev) {
        send(out, tag::FILES, j);
    }
}

fn file_error(out: &Out, code: &str, detail: impl Into<String>) {
    file_event(
        out,
        &FileEv::Error {
            code: code.into(),
            detail: detail.into(),
        },
    );
}

type Out = std::sync::mpsc::SyncSender<(u8, Vec<u8>)>;

fn send(out: &Out, tag: u8, payload: Vec<u8>) {
    let _ = out.send((tag, payload));
}

// ---------------------------------------------------------------- clipboard

struct ClipShared {
    local_text: Option<String>,
    input: Option<mpsc::UnboundedSender<RdpInputEvent>>,
    out: Out,
    ready: bool,
    caps: ClipboardGeneralCapabilityFlags,
    /// Files currently on offer, and snapshots the host locked (`clipDataId`).
    offer: Option<Arc<Vec<files::LocalEntry>>>,
    locks: HashMap<u32, Arc<Vec<files::LocalEntry>>>,
    /// Waiting for the host's answer to our file list.
    offer_pending: bool,
    offer_error_sent: bool,
    open_file: Option<(PathBuf, std::fs::File)>,
    /// Highest byte served per (list, index), for the progress badge.
    served: HashMap<(usize, usize), u64>,
    last_progress: Instant,
    /// Format id of `FileGroupDescriptorW` on the remote clipboard.
    remote_files: Option<ClipboardFormatId>,
    pending_fetch: Option<PathBuf>,
    download: Option<files::Download>,
    /// Outstanding request of the download: (stream id, is SIZE).
    download_stream: Option<(u32, bool)>,
    next_stream: u32,
}

impl ClipShared {
    fn new(out: Out) -> Self {
        Self {
            local_text: None,
            input: None,
            out,
            ready: false,
            caps: ClipboardGeneralCapabilityFlags::empty(),
            offer: None,
            locks: HashMap::new(),
            offer_pending: false,
            offer_error_sent: false,
            open_file: None,
            served: HashMap::new(),
            last_progress: Instant::now(),
            remote_files: None,
            pending_fetch: None,
            download: None,
            download_stream: None,
            next_stream: 1,
        }
    }

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

    fn can_transfer(&self) -> bool {
        self.ready
            && self
                .caps
                .contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
    }

    fn progress(&mut self, up: bool, done: u64, total: u64, force: bool) {
        if force || self.last_progress.elapsed() > Duration::from_millis(150) {
            self.last_progress = Instant::now();
            file_event(&self.out, &FileEv::Progress { up, done, total });
        }
    }

    /// Announces `list` to the host (from the stdin thread).
    fn offer_files(&mut self, list: files::FileList) {
        if !self.can_transfer() {
            return file_error(&self.out, "unsupported", "");
        }
        if list.entries.is_empty() {
            return file_error(&self.out, "empty", list.skipped.join(", "));
        }
        let descriptors: Vec<FileDescriptor> = list
            .entries
            .iter()
            .map(files::LocalEntry::descriptor)
            .collect();
        let (count, bytes) = (list.file_count(), list.total_bytes());
        self.offer = Some(Arc::new(list.entries));
        self.open_file = None;
        self.served.clear();
        self.offer_pending = true;
        self.offer_error_sent = false;
        self.post(ClipboardMessage::SendInitiateFileCopy(descriptors));
        file_event(
            &self.out,
            &FileEv::Offered {
                files: count,
                bytes,
                skipped: list.skipped,
            },
        );
    }

    /// Answers a `FileContentsRequest` for our offer.
    fn serve(&mut self, req: &FileContentsRequest) -> FileContentsResponse<'static> {
        let list = req
            .data_id
            .and_then(|id| self.locks.get(&id).cloned())
            .or_else(|| self.offer.clone());
        let index = usize::try_from(req.index).unwrap_or(usize::MAX);
        let Some((list, entry)) = list
            .as_ref()
            .and_then(|l| l.get(index).cloned().map(|e| (l.clone(), e)))
        else {
            return FileContentsResponse::new_error(req.stream_id);
        };
        if entry.is_dir {
            return FileContentsResponse::new_error(req.stream_id);
        }
        if req.flags.contains(FileContentsFlags::SIZE) {
            return FileContentsResponse::new_size_response(req.stream_id, entry.size);
        }
        // Files are opened when the host asks, not when they are dropped.
        if self
            .open_file
            .as_ref()
            .is_none_or(|(p, _)| *p != entry.path)
        {
            match std::fs::File::open(&entry.path) {
                Ok(f) => self.open_file = Some((entry.path.clone(), f)),
                Err(e) => {
                    self.read_failed(&entry, &e);
                    return FileContentsResponse::new_error(req.stream_id);
                }
            }
        }
        let (_, f) = self.open_file.as_mut().expect("opened above");
        match files::read_range(f, req.position, req.requested_size) {
            Ok(data) => {
                let key = (Arc::as_ptr(&list) as usize, index);
                let end = req.position + data.len() as u64;
                let seen = self.served.entry(key).or_default();
                *seen = (*seen).max(end.min(entry.size));
                let done: u64 = self.served.values().sum();
                let total: u64 = list.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
                self.progress(true, done.min(total), total, done >= total);
                FileContentsResponse::new_data_response(req.stream_id, data)
            }
            Err(e) => {
                self.read_failed(&entry, &e);
                FileContentsResponse::new_error(req.stream_id)
            }
        }
    }

    fn read_failed(&mut self, entry: &files::LocalEntry, e: &std::io::Error) {
        if !std::mem::replace(&mut self.offer_error_sent, true) {
            file_error(&self.out, "read", format!("{}: {e}", entry.rel.join("/")));
        }
    }

    /// Starts saving the remote clipboard's files into `dir` (from the stdin thread).
    fn fetch_files(&mut self, dir: PathBuf) {
        if !self.can_transfer() {
            return file_error(&self.out, "unsupported", "");
        }
        if self.download.is_some() || self.pending_fetch.is_some() {
            return file_error(&self.out, "busy", "");
        }
        let Some(format) = self.remote_files else {
            return file_error(&self.out, "no_files", "");
        };
        self.pending_fetch = Some(dir);
        self.post(ClipboardMessage::SendInitiatePaste(format));
    }

    /// Issues the next request of the running download, or finishes it.
    fn drive_download(&mut self) {
        let Some(dl) = self.download.as_mut() else {
            return;
        };
        let huge = self
            .caps
            .contains(ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED);
        let step = dl.advance();
        let clip_data_id = dl.clip_data_id;
        let (done, total) = (dl.done_bytes, dl.total_bytes);
        let request =
            |stream_id, index: usize, flags, position, requested_size| FileContentsRequest {
                stream_id,
                index: i32::try_from(index).unwrap_or(i32::MAX),
                flags,
                position,
                requested_size,
                data_id: clip_data_id,
            };
        let stream_id = self.next_stream;
        self.next_stream = self.next_stream.wrapping_add(1).max(1);
        match step {
            Ok(files::Step::Size { index }) => {
                self.download_stream = Some((stream_id, true));
                let r = request(stream_id, index, FileContentsFlags::SIZE, 0, 8);
                self.post(ClipboardMessage::SendFileContentsRequest(r));
            }
            Ok(files::Step::Range { index, pos, len }) => {
                if pos >= 0x8000_0000 && !huge {
                    self.end_download(Some(("too_big", String::new())));
                    return;
                }
                self.download_stream = Some((stream_id, false));
                let r = request(stream_id, index, FileContentsFlags::RANGE, pos, len);
                self.post(ClipboardMessage::SendFileContentsRequest(r));
                self.progress(false, done, total, false);
            }
            Ok(files::Step::Finished) => self.end_download(None),
            Err(e) => self.end_download(Some(("write", e.to_string()))),
        }
    }

    fn end_download(&mut self, error: Option<(&str, String)>) {
        self.download_stream = None;
        let Some(mut dl) = self.download.take() else {
            return;
        };
        match error {
            Some((code, detail)) => {
                dl.abort();
                file_error(&self.out, code, detail);
            }
            None => {
                self.progress(false, dl.done_bytes, dl.total_bytes, true);
                file_event(
                    &self.out,
                    &FileEv::Saved {
                        dir: dl.root.display().to_string(),
                        files: dl.files_written,
                    },
                );
            }
        }
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
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
    }

    fn on_ready(&mut self) {
        self.shared.0.lock().unwrap().ready = true;
    }

    fn on_format_list_response(&mut self, ok: bool) {
        let mut s = self.shared.0.lock().unwrap();
        if std::mem::take(&mut s.offer_pending) {
            if ok {
                file_event(&s.out, &FileEv::Accepted);
            } else {
                s.offer = None;
                file_error(&s.out, "rejected", "");
            }
        }
    }

    fn on_request_format_list(&mut self) {
        self.shared.0.lock().unwrap().advertise();
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        self.shared.0.lock().unwrap().caps = capabilities;
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        {
            let mut s = self.shared.0.lock().unwrap();
            s.remote_files = available_formats
                .iter()
                .find(|f| {
                    f.name
                        .as_ref()
                        .is_some_and(|n| n.value() == ClipboardFormatName::FILE_LIST.value())
                })
                .map(|f| f.id);
            let files = s.remote_files.is_some() && s.can_transfer();
            file_event(&s.out, &FileEv::Remote { files });
        }
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
            let mut s = self.shared.0.lock().unwrap();
            if s.pending_fetch.take().is_some() {
                file_error(&s.out, "list", "");
            }
            return;
        }
        if let Ok(text) = response.to_unicode_string() {
            let text = text.trim_end_matches('\0').to_string();
            send(&self.out, tag::CLIP_FROM_HOST, text.into_bytes());
        }
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        let mut s = self.shared.0.lock().unwrap();
        let response = s.serve(&request);
        s.post(ClipboardMessage::SendFileContentsResponse(response));
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        let mut s = self.shared.0.lock().unwrap();
        let Some((stream, is_size)) = s.download_stream else {
            return;
        };
        if response.stream_id() != stream {
            return;
        }
        s.download_stream = None;
        if response.is_error() {
            return s.end_download(Some(("remote_read", String::new())));
        }
        let Some(dl) = s.download.as_mut() else {
            return;
        };
        let result = if is_size {
            response
                .data_as_size()
                .map(|n| dl.got_size(n))
                .map_err(|e| e.to_string())
        } else {
            dl.got_data(response.data()).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => s.drive_download(),
            Err(e) => s.end_download(Some(("remote_read", e))),
        }
    }

    fn on_lock(&mut self, data_id: LockDataId) {
        let mut s = self.shared.0.lock().unwrap();
        if let Some(offer) = s.offer.clone() {
            s.locks.insert(data_id.0, offer);
        }
    }

    fn on_unlock(&mut self, data_id: LockDataId) {
        let mut s = self.shared.0.lock().unwrap();
        s.locks.remove(&data_id.0);
        if s.locks.is_empty() {
            s.open_file = None;
        }
    }

    fn on_remote_file_list(&mut self, list: &[FileDescriptor], clip_data_id: Option<u32>) {
        let mut s = self.shared.0.lock().unwrap();
        let Some(root) = s.pending_fetch.take() else {
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&root) {
            return file_error(&s.out, "write", format!("{}: {e}", root.display()));
        }
        let entries: Vec<_> = list
            .iter()
            .map(|f| {
                let is_dir = f
                    .attributes
                    .is_some_and(|a| a.contains(ClipboardFileAttributes::DIRECTORY));
                (f.relative_path.clone(), f.name.clone(), is_dir, f.file_size)
            })
            .collect();
        let plan = files::plan_download(&root, &entries);
        s.download = Some(files::Download::new(root, plan, clip_data_id));
        s.drive_download();
    }
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

    let shared = SharedClip(Arc::new(Mutex::new(ClipShared::new(out.clone()))));
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
                Cmd::OfferFiles { paths } => {
                    // Walking big folders must not hold up mouse and keyboard input.
                    let shared = shared_cmds.clone();
                    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                    std::thread::spawn(move || {
                        let list = files::build_file_list(&paths);
                        shared.0.lock().unwrap().offer_files(list);
                    });
                    None
                }
                Cmd::FetchFiles { dir } => {
                    shared_cmds
                        .0
                        .lock()
                        .unwrap()
                        .fetch_files(PathBuf::from(dir));
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
        next[w + 1] = 7;
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
        let o: Cmd = serde_json::from_str(r#"{"t":"OfferFiles","paths":["/a b/ä.txt"]}"#).unwrap();
        assert_eq!(
            o,
            Cmd::OfferFiles {
                paths: vec!["/a b/ä.txt".into()]
            }
        );
        let f: Cmd = serde_json::from_str(r#"{"t":"FetchFiles","dir":"/x"}"#).unwrap();
        assert_eq!(f, Cmd::FetchFiles { dir: "/x".into() });
    }

    #[test]
    fn file_events_are_tagged_json() {
        let j = serde_json::to_string(&FileEv::Progress {
            up: true,
            done: 1,
            total: 2,
        })
        .unwrap();
        assert_eq!(j, r#"{"ev":"progress","up":true,"done":1,"total":2}"#);
        let j = serde_json::to_string(&FileEv::Accepted).unwrap();
        assert_eq!(j, r#"{"ev":"accepted"}"#);
    }

    #[test]
    fn serves_sizes_and_ranges_of_the_offer() {
        let dir = std::env::temp_dir().join(format!("aurora-rdp-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("d")).unwrap();
        std::fs::write(dir.join("d/f.txt"), b"abcdef").unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        let mut s = ClipShared::new(tx);
        // Not ready yet: refused, nothing posted.
        s.offer_files(files::build_file_list(&[dir.join("d")]));
        assert!(s.offer.is_none());
        s.ready = true;
        s.caps = ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED;
        s.offer_files(files::build_file_list(&[dir.join("d")]));
        assert_eq!(s.offer.as_ref().unwrap().len(), 2);
        let req = |index, flags, position, requested_size| FileContentsRequest {
            stream_id: 7,
            index,
            flags,
            position,
            requested_size,
            data_id: None,
        };
        let r = s.serve(&req(1, FileContentsFlags::SIZE, 0, 8));
        assert_eq!(r.data_as_size().unwrap(), 6);
        let r = s.serve(&req(1, FileContentsFlags::RANGE, 2, 3));
        assert_eq!(r.data(), b"cde");
        let r = s.serve(&req(1, FileContentsFlags::RANGE, 5, 100));
        assert_eq!(r.data(), b"f");
        let r = s.serve(&req(1, FileContentsFlags::RANGE, 6, 100));
        assert!(!r.is_error() && r.data().is_empty());
        assert!(s.serve(&req(0, FileContentsFlags::SIZE, 0, 8)).is_error());
        assert!(s.serve(&req(9, FileContentsFlags::SIZE, 0, 8)).is_error());
        // A locked snapshot keeps serving after a new offer.
        s.locks.insert(3, s.offer.clone().unwrap());
        s.offer = None;
        let mut locked = req(1, FileContentsFlags::RANGE, 0, 2);
        locked.data_id = Some(3);
        assert_eq!(s.serve(&locked).data(), b"ab");
        let events: Vec<String> = rx
            .try_iter()
            .filter(|(t, _)| *t == tag::FILES)
            .map(|(_, p)| String::from_utf8(p).unwrap())
            .collect();
        assert!(events[0].contains("unsupported"));
        assert!(events[1].contains("offered"));
        assert!(events.iter().any(|e| e.contains(r#""done":6,"total":6"#)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
