//! Embedded VNC (RFB 3.3 / 3.7 / 3.8) client.
//!
//! The session task decodes into a shared RGBA framebuffer and reports changed regions
//! as [`VncEvent::Damage`]; the UI uploads those regions into a texture. Supported:
//! security types None and VNC Authentication, encodings ZRLE, CopyRect, Raw and the
//! DesktopSize / ExtendedDesktopSize pseudo-encodings.
//!
//! Everything the server sends is bounds-checked: malformed data ends the session with
//! an error, it never panics.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

use super::Sink;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest framebuffer we accept (per side).
const MAX_DIM: u16 = 8192;
/// Cut text longer than this is skipped.
const MAX_CUT_TEXT: usize = 1 << 20;
/// Largest compressed ZRLE rectangle we accept.
const MAX_ZRLE_DATA: usize = 64 << 20;

const ENC_RAW: i32 = 0;
const ENC_COPYRECT: i32 = 1;
const ENC_ZRLE: i32 = 16;
const ENC_DESKTOP_SIZE: i32 = -223;
const ENC_EXT_DESKTOP_SIZE: i32 = -308;

#[derive(Clone)]
pub struct VncParams {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
    pub view_only: bool,
}

/// Commands from the UI to the session.
#[derive(Debug, Clone)]
pub enum VncCmd {
    /// Pointer position (framebuffer pixels) and button mask
    /// (bit 0 left, 1 middle, 2 right, 3/4 wheel up/down, 5/6 wheel left/right).
    Pointer {
        x: u16,
        y: u16,
        buttons: u8,
    },
    /// X11 keysym press / release.
    Key {
        keysym: u32,
        down: bool,
    },
    /// Local clipboard text for the server.
    ClientCut(String),
    /// Ask for a full (non-incremental) update.
    Refresh,
    Close,
}

/// Events from the session to the UI.
#[derive(Debug, Clone)]
pub enum VncEvent {
    Status(String),
    Connected {
        name: String,
        width: u16,
        height: u16,
    },
    Resized {
        width: u16,
        height: u16,
    },
    /// The framebuffer changed in this rectangle.
    Damage {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
    },
    ServerCut(String),
    Closed(Option<String>),
}

/// RGBA (alpha always 255), row-major, `width * height * 4` bytes.
#[derive(Default)]
pub struct FrameBuffer {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl FrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut rgba = vec![0u8; width * height * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Clips a rectangle to the framebuffer; `None` if nothing is left.
    fn clip(&self, x: usize, y: usize, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        if x >= self.width || y >= self.height || w == 0 || h == 0 {
            return None;
        }
        Some((x, y, w.min(self.width - x), h.min(self.height - y)))
    }

    /// Copies `w*h` pixels of 4 bytes each (R,G,B,X) from `src` (stride `w*4`) to `(x, y)`,
    /// clipped to the framebuffer.
    fn blit(&mut self, x: usize, y: usize, w: usize, h: usize, src: &[u8]) {
        let Some((x, y, cw, ch)) = self.clip(x, y, w, h) else {
            return;
        };
        for row in 0..ch {
            let s = row * w * 4;
            let Some(line) = src.get(s..s + cw * 4) else {
                return;
            };
            let d = ((y + row) * self.width + x) * 4;
            let dst = &mut self.rgba[d..d + cw * 4];
            for (o, i) in dst.chunks_exact_mut(4).zip(line.chunks_exact(4)) {
                o[..3].copy_from_slice(&i[..3]);
                o[3] = 255;
            }
        }
    }

    fn copy_rect(&mut self, sx: usize, sy: usize, x: usize, y: usize, w: usize, h: usize) {
        // Clip both source and destination.
        let Some((sx, sy, w, h)) = self.clip(sx, sy, w, h) else {
            return;
        };
        let Some((x, y, w, h)) = self.clip(x, y, w, h) else {
            return;
        };
        let stride = self.width * 4;
        let rows: Box<dyn Iterator<Item = usize>> = if y > sy {
            Box::new((0..h).rev())
        } else {
            Box::new(0..h)
        };
        for r in rows {
            let s = (sy + r) * stride + sx * 4;
            let d = (y + r) * stride + x * 4;
            self.rgba.copy_within(s..s + w * 4, d);
        }
    }
}

/// Starts a session on the runtime. Dropping the returned sender (or sending
/// [`VncCmd::Close`]) ends it; the last event is always [`VncEvent::Closed`].
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    params: VncParams,
    fb: Arc<Mutex<FrameBuffer>>,
    events: super::Sink<VncEvent>,
) -> mpsc::UnboundedSender<VncCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel::<VncCmd>();
    let (inner_tx, inner_rx) = mpsc::unbounded_channel::<VncCmd>();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    // Forwarder: turns Close / a dropped sender into a stop signal, so a close also
    // interrupts a hanging connect or handshake.
    rt.spawn(async move {
        while let Some(cmd) = rx.recv().await {
            if matches!(cmd, VncCmd::Close) || inner_tx.send(cmd).is_err() {
                break;
            }
        }
        let _ = stop_tx.send(());
    });
    rt.spawn(async move {
        let result = tokio::select! {
            r = run(params, fb, &events, inner_rx) => r,
            _ = stop_rx => Ok(()),
        };
        events.send(VncEvent::Closed(result.err().map(friendly_error)));
    });
    tx
}

fn friendly_error(e: anyhow::Error) -> String {
    if let Some(io) = e.downcast_ref::<io::Error>() {
        match io.kind() {
            io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe => {
                return t!("Der VNC-Server hat die Verbindung getrennt").into();
            }
            _ => {}
        }
    }
    format!("{e:#}")
}

async fn connect(host: &str, port: u16) -> Result<TcpStream> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
        Err(_) => bail!(t!(
            "Zeitüberschreitung beim Verbinden mit {}:{}",
            host,
            port
        )),
        Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => bail!(t!(
            "Verbindung abgelehnt – läuft auf {}:{} ein VNC-Server?",
            host,
            port
        )),
        Ok(Err(e)) => bail!(t!("Verbindung zu {}:{} fehlgeschlagen: {}", host, port, e)),
        Ok(Ok(s)) => Ok(s),
    }
}

async fn run(
    params: VncParams,
    fb: Arc<Mutex<FrameBuffer>>,
    events: &Sink<VncEvent>,
    mut cmds: mpsc::UnboundedReceiver<VncCmd>,
) -> Result<()> {
    events.send(VncEvent::Status(t!(
        "Verbinde mit {}:{}…",
        params.host,
        params.port
    )));
    let stream = connect(&params.host, params.port).await?;
    let _ = stream.set_nodelay(true);
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::with_capacity(64 * 1024, r);

    let init = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        handshake(&mut r, &mut w, params.password.as_deref(), events),
    )
    .await
    .map_err(|_| anyhow!(t!("Zeitüberschreitung bei der VNC-Anmeldung")))??;

    {
        let mut f = fb.lock().unwrap_or_else(|p| p.into_inner());
        *f = FrameBuffer::new(init.width as usize, init.height as usize);
    }

    let mut setup = Vec::new();
    setup.extend_from_slice(&set_pixel_format_msg());
    setup.extend_from_slice(&set_encodings_msg(&[
        ENC_ZRLE,
        ENC_COPYRECT,
        ENC_RAW,
        ENC_DESKTOP_SIZE,
        ENC_EXT_DESKTOP_SIZE,
    ]));
    setup.extend_from_slice(&update_request(false, init.width, init.height));
    w.write_all(&setup).await?;
    events.send(VncEvent::Connected {
        name: init.name,
        width: init.width,
        height: init.height,
    });

    let (wtx, mut wrx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = async move {
        while let Some(buf) = wrx.recv().await {
            w.write_all(&buf).await?;
        }
        Ok::<_, io::Error>(())
    };
    let view_only = params.view_only;
    let fb_cmd = fb.clone();
    let wtx_cmd = wtx.clone();
    let commands = async move {
        while let Some(cmd) = cmds.recv().await {
            let msg = match cmd {
                VncCmd::Pointer { .. } | VncCmd::Key { .. } | VncCmd::ClientCut(_) if view_only => {
                    continue;
                }
                VncCmd::Pointer { x, y, buttons } => pointer_msg(x, y, buttons),
                VncCmd::Key { keysym, down } => key_msg(keysym, down),
                VncCmd::ClientCut(text) => client_cut_msg(&text),
                VncCmd::Refresh => {
                    let (w, h) = {
                        let f = fb_cmd.lock().unwrap_or_else(|p| p.into_inner());
                        (f.width as u16, f.height as u16)
                    };
                    update_request(false, w, h)
                }
                VncCmd::Close => break,
            };
            if wtx_cmd.send(msg).is_err() {
                break;
            }
        }
    };
    let mut session = Session {
        fb,
        events,
        zlib: flate2::Decompress::new(true),
        zbuf: Vec::new(),
        out: wtx,
        stats: Stats::default(),
    };
    tokio::select! {
        r = session.read_loop(&mut r) => r,
        r = writer => r.map_err(Into::into),
        _ = commands => Ok(()),
    }
}

struct ServerInit {
    width: u16,
    height: u16,
    name: String,
}

async fn read_reason<R: AsyncRead + Unpin>(r: &mut R) -> Result<String> {
    let len = r.read_u32().await? as usize;
    let bytes = read_vec(r, len.min(4096)).await?;
    Ok(latin1(&bytes))
}

async fn read_vec<R: AsyncRead + Unpin>(r: &mut R, len: usize) -> Result<Vec<u8>> {
    let mut v = vec![0u8; len];
    r.read_exact(&mut v).await?;
    Ok(v)
}

/// Discards `len` bytes without buffering them all.
async fn skip<R: AsyncRead + Unpin>(r: &mut R, mut len: u64) -> Result<()> {
    let mut buf = [0u8; 8192];
    while len > 0 {
        let n = len.min(buf.len() as u64) as usize;
        r.read_exact(&mut buf[..n]).await?;
        len -= n as u64;
    }
    Ok(())
}

fn security_name(t: u8) -> String {
    match t {
        5 => "RA2".into(),
        6 => "RA2ne".into(),
        16 => "Tight".into(),
        17 => "Ultra".into(),
        18 => "TLS".into(),
        19 => "VeNCrypt".into(),
        20 => "SASL".into(),
        21 => "MD5".into(),
        22 => "xvp".into(),
        30 => "Apple ARD".into(),
        35 => "Apple".into(),
        113 => "MS-Logon II".into(),
        n => n.to_string(),
    }
}

/// Explains which security types are offered and how to make the server compatible.
fn unsupported_security(types: &[u8]) -> String {
    let names: Vec<String> = types.iter().map(|t| security_name(*t)).collect();
    let mut msg = t!(
        "Der VNC-Server bietet keine unterstützte Anmeldung an ({}). Unterstützt werden „Keine“ und „VNC-Passwort“.",
        names.join(", ")
    );
    if types.iter().any(|t| matches!(t, 30 | 35)) {
        msg.push(' ');
        msg.push_str(t!(
            "Auf dem Mac unter Systemeinstellungen → Allgemein → Teilen → Bildschirmfreigabe → ⓘ die Option „VNC-Viewer dürfen den Bildschirm mit Passwort steuern“ aktivieren."
        ));
    } else if types.iter().any(|t| matches!(t, 18 | 19)) {
        msg.push(' ');
        msg.push_str(t!(
            "Am Server die Sicherheitstypen „VncAuth“ oder „None“ erlauben (TigerVNC: -SecurityTypes VncAuth)."
        ));
    }
    msg
}

async fn handshake<R, W>(
    r: &mut R,
    w: &mut W,
    password: Option<&str>,
    events: &Sink<VncEvent>,
) -> Result<ServerInit>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut ver = [0u8; 12];
    r.read_exact(&mut ver)
        .await
        .context(t!("Keine Antwort vom VNC-Server"))?;
    let minor = parse_version(&ver).ok_or_else(|| anyhow!(t!("Das ist kein VNC-Server")))?;
    w.write_all(format!("RFB 003.{minor:03}\n").as_bytes())
        .await?;

    let sec_type = if minor == 3 {
        match r.read_u32().await? {
            0 => bail!(t!(
                "Der VNC-Server lehnt die Verbindung ab: {}",
                read_reason(r).await?
            )),
            t @ (1 | 2) => t as u8,
            t => bail!(unsupported_security(&[t.min(255) as u8])),
        }
    } else {
        let n = r.read_u8().await? as usize;
        if n == 0 {
            bail!(t!(
                "Der VNC-Server lehnt die Verbindung ab: {}",
                read_reason(r).await?
            ));
        }
        let types = read_vec(r, n).await?;
        let pick = if types.contains(&1) {
            1
        } else if types.contains(&2) {
            2
        } else {
            bail!(unsupported_security(&types));
        };
        w.write_all(&[pick]).await?;
        pick
    };

    if sec_type == 2 {
        let Some(pw) = password.filter(|p| !p.is_empty()) else {
            bail!(t!("Der VNC-Server verlangt ein Passwort"));
        };
        events.send(VncEvent::Status(t!("Anmeldung…").into()));
        let mut challenge = [0u8; 16];
        r.read_exact(&mut challenge).await?;
        w.write_all(&vnc_auth_response(pw, &challenge)).await?;
    }
    // 3.8 always sends a SecurityResult; older versions only after VNC authentication.
    if sec_type == 2 || minor >= 8 {
        let result = r.read_u32().await?;
        if result != 0 {
            let reason = if minor >= 8 {
                read_reason(r).await.ok().filter(|s| !s.trim().is_empty())
            } else {
                None
            };
            let base = if sec_type == 2 {
                t!("Passwort falsch")
            } else {
                t!("Anmeldung abgelehnt")
            };
            bail!(match reason {
                Some(r) => format!("{base}: {}", r.trim()),
                None => base.to_string(),
            });
        }
    }

    // ClientInit: shared session.
    w.write_all(&[1]).await?;
    let width = r.read_u16().await?;
    let height = r.read_u16().await?;
    let mut _pf = [0u8; 16];
    r.read_exact(&mut _pf).await?;
    let name_len = r.read_u32().await? as u64;
    let name = read_vec(r, name_len.min(4096) as usize).await?;
    skip(r, name_len.saturating_sub(4096)).await?;
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        bail!(t!(
            "Ungültige Bildschirmgröße vom VNC-Server: {}×{}",
            width,
            height
        ));
    }
    Ok(ServerInit {
        width,
        height,
        name: String::from_utf8(name.clone()).unwrap_or_else(|_| latin1(&name)),
    })
}

/// Parses `RFB 003.008\n` and returns the minor version we answer with (3, 7 or 8).
fn parse_version(v: &[u8; 12]) -> Option<u32> {
    if &v[..4] != b"RFB " || v[7] != b'.' || v[11] != b'\n' {
        return None;
    }
    let num = |s: &[u8]| std::str::from_utf8(s).ok()?.parse::<u32>().ok();
    let major = num(&v[4..7])?;
    let minor = num(&v[8..11])?;
    Some(match (major, minor) {
        (0..=2, _) => return None,
        (3, 0..=6) => 3,
        (3, 7) => 7,
        _ => 8, // 3.8, 3.889 (Apple), 4.x, 5.x …
    })
}

/// DES key from the password: first 8 bytes, zero padded, each byte bit-reversed.
fn vnc_auth_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    auth_response(password.as_bytes(), challenge)
}

fn auth_response(password: &[u8], challenge: &[u8; 16]) -> [u8; 16] {
    use des::cipher::{BlockCipherEncrypt, KeyInit};
    let mut key = [0u8; 8];
    for (k, b) in key.iter_mut().zip(password) {
        *k = b.reverse_bits();
    }
    let cipher = des::Des::new(&key.into());
    let mut out = [0u8; 16];
    for (o, c) in out.chunks_exact_mut(8).zip(challenge.chunks_exact(8)) {
        let mut block: [u8; 8] = c.try_into().unwrap_or_default();
        let b = (&mut block).into();
        cipher.encrypt_block(b);
        o.copy_from_slice(&block);
    }
    out
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn set_pixel_format_msg() -> [u8; 20] {
    [
        0, 0, 0, 0, // type, padding
        32, 24, 0, 1, // bpp, depth, big-endian, true-colour
        0, 255, 0, 255, 0, 255, // max r, g, b
        0, 8, 16, // shifts → bytes R, G, B, X (little endian)
        0, 0, 0, // padding
    ]
}

fn set_encodings_msg(encs: &[i32]) -> Vec<u8> {
    let mut m = vec![2, 0];
    m.extend_from_slice(&(encs.len() as u16).to_be_bytes());
    for e in encs {
        m.extend_from_slice(&e.to_be_bytes());
    }
    m
}

fn update_request(incremental: bool, w: u16, h: u16) -> Vec<u8> {
    let mut m = vec![3, incremental as u8, 0, 0, 0, 0];
    m.extend_from_slice(&w.to_be_bytes());
    m.extend_from_slice(&h.to_be_bytes());
    m
}

fn key_msg(keysym: u32, down: bool) -> Vec<u8> {
    let mut m = vec![4, down as u8, 0, 0];
    m.extend_from_slice(&keysym.to_be_bytes());
    m
}

fn pointer_msg(x: u16, y: u16, buttons: u8) -> Vec<u8> {
    let mut m = vec![5, buttons];
    m.extend_from_slice(&x.to_be_bytes());
    m.extend_from_slice(&y.to_be_bytes());
    m
}

fn client_cut_msg(text: &str) -> Vec<u8> {
    let bytes: Vec<u8> = text
        .chars()
        .filter(|c| *c != '\r')
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .take(MAX_CUT_TEXT)
        .collect();
    let mut m = vec![6, 0, 0, 0];
    m.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    m.extend_from_slice(&bytes);
    m
}

#[derive(Default)]
struct Stats {
    raw: u64,
    copyrect: u64,
    zrle: u64,
}

struct Session<'a> {
    fb: Arc<Mutex<FrameBuffer>>,
    events: &'a Sink<VncEvent>,
    zlib: flate2::Decompress,
    zbuf: Vec<u8>,
    out: mpsc::UnboundedSender<Vec<u8>>,
    stats: Stats,
}

impl Session<'_> {
    fn lock_fb(&self) -> std::sync::MutexGuard<'_, FrameBuffer> {
        self.fb.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn fb_size(&self) -> (usize, usize) {
        let f = self.lock_fb();
        (f.width, f.height)
    }

    async fn read_loop<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> Result<()> {
        loop {
            match r.read_u8().await? {
                0 => self.framebuffer_update(r).await?,
                1 => {
                    let mut hdr = [0u8; 5];
                    r.read_exact(&mut hdr).await?;
                    let n = u16::from_be_bytes([hdr[3], hdr[4]]) as u64;
                    skip(r, n * 6).await?;
                }
                2 => {} // Bell
                3 => {
                    let mut pad = [0u8; 3];
                    r.read_exact(&mut pad).await?;
                    let len = r.read_u32().await? as usize;
                    if len > MAX_CUT_TEXT {
                        skip(r, len as u64).await?;
                    } else {
                        let bytes = read_vec(r, len).await?;
                        self.events.send(VncEvent::ServerCut(latin1(&bytes)));
                    }
                }
                t => bail!(t!("Unbekannte Nachricht vom VNC-Server ({})", t)),
            }
        }
    }

    async fn framebuffer_update<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> Result<()> {
        let _pad = r.read_u8().await?;
        let n = r.read_u16().await?;
        // Damage as (x0, y0, x1, y1).
        let mut damage: Option<(usize, usize, usize, usize)> = None;
        let mut full_request = false;
        for _ in 0..n {
            let mut hdr = [0u8; 12];
            r.read_exact(&mut hdr).await?;
            let x = u16::from_be_bytes([hdr[0], hdr[1]]) as usize;
            let y = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
            let w = u16::from_be_bytes([hdr[4], hdr[5]]) as usize;
            let h = u16::from_be_bytes([hdr[6], hdr[7]]) as usize;
            let enc = i32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
            let mut touched = true;
            match enc {
                ENC_RAW => {
                    self.stats.raw += 1;
                    let len = w * h * 4;
                    if len > MAX_ZRLE_DATA {
                        bail!(t!("Ungültige Daten vom VNC-Server"));
                    }
                    let data = read_vec(r, len).await?;
                    self.lock_fb().blit(x, y, w, h, &data);
                }
                ENC_COPYRECT => {
                    self.stats.copyrect += 1;
                    let sx = r.read_u16().await? as usize;
                    let sy = r.read_u16().await? as usize;
                    self.lock_fb().copy_rect(sx, sy, x, y, w, h);
                }
                ENC_ZRLE => {
                    self.stats.zrle += 1;
                    #[cfg(test)]
                    tests::ZRLE_RECTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let len = r.read_u32().await? as usize;
                    if len > MAX_ZRLE_DATA {
                        bail!(t!("Ungültige Daten vom VNC-Server"));
                    }
                    let data = read_vec(r, len).await?;
                    let max_out = zrle_max_len(w, h);
                    inflate(&mut self.zlib, &data, &mut self.zbuf, max_out)?;
                    let mut fb = self.lock_fb();
                    decode_zrle(&self.zbuf, &mut fb, x, y, w, h)?;
                }
                ENC_DESKTOP_SIZE => {
                    touched = false;
                    self.resize(w, h)?;
                    full_request = true;
                }
                ENC_EXT_DESKTOP_SIZE => {
                    touched = false;
                    let screens = r.read_u8().await? as u64;
                    skip(r, 3 + screens * 16).await?;
                    // y carries the status; 0 means the change happened.
                    if y == 0 && (w, h) != self.fb_size() {
                        self.resize(w, h)?;
                        full_request = true;
                    }
                }
                other => bail!(t!(
                    "Der VNC-Server sendet eine nicht unterstützte Kodierung ({})",
                    other
                )),
            }
            if touched {
                let (fw, fh) = self.fb_size();
                let (x1, y1) = ((x + w).min(fw), (y + h).min(fh));
                if x < x1 && y < y1 {
                    damage = Some(match damage {
                        None => (x, y, x1, y1),
                        Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x1), d.max(y1)),
                    });
                }
            }
        }
        if let Some((x0, y0, x1, y1)) = damage {
            self.events.send(VncEvent::Damage {
                x: x0 as u16,
                y: y0 as u16,
                w: (x1 - x0) as u16,
                h: (y1 - y0) as u16,
            });
        }
        log::trace!(
            "VNC update: raw {} copyrect {} zrle {}",
            self.stats.raw,
            self.stats.copyrect,
            self.stats.zrle
        );
        let (w, h) = self.fb_size();
        let _ = self
            .out
            .send(update_request(!full_request, w as u16, h as u16));
        Ok(())
    }

    fn resize(&mut self, w: usize, h: usize) -> Result<()> {
        if w == 0 || h == 0 || w > MAX_DIM as usize || h > MAX_DIM as usize {
            bail!(t!("Ungültige Bildschirmgröße vom VNC-Server: {}×{}", w, h));
        }
        *self.lock_fb() = FrameBuffer::new(w, h);
        self.events.send(VncEvent::Resized {
            width: w as u16,
            height: h as u16,
        });
        Ok(())
    }
}

/// Upper bound of the decompressed size of a ZRLE rectangle (raw tiles everywhere).
fn zrle_max_len(w: usize, h: usize) -> usize {
    let tiles = w.div_ceil(64) * h.div_ceil(64);
    w * h * 3 + tiles + 1024
}

/// Inflates one ZRLE rectangle's data with the connection-wide zlib stream into `out`.
fn inflate(z: &mut flate2::Decompress, input: &[u8], out: &mut Vec<u8>, max: usize) -> Result<()> {
    out.clear();
    let mut pos = 0;
    loop {
        if out.len() == out.capacity() {
            if out.len() >= max {
                bail!(t!("Ungültige Daten vom VNC-Server"));
            }
            out.reserve((out.len() + 4096).min(max - out.len()).max(4096));
        }
        let (in0, out0) = (z.total_in(), z.total_out());
        let status = z
            .decompress_vec(&input[pos..], out, flate2::FlushDecompress::Sync)
            .map_err(|_| anyhow!(t!("Ungültige Daten vom VNC-Server")))?;
        let consumed = (z.total_in() - in0) as usize;
        let produced = z.total_out() - out0;
        pos += consumed;
        let room_left = out.len() < out.capacity();
        if matches!(status, flate2::Status::StreamEnd) {
            break;
        }
        if pos >= input.len() && room_left {
            break;
        }
        if consumed == 0 && produced == 0 && room_left {
            // No progress although there is input and room: corrupt stream.
            bail!(t!("Ungültige Daten vom VNC-Server"));
        }
    }
    Ok(())
}

/// Reads from a byte slice; every access is bounds-checked.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| anyhow!(t!("Ungültige Daten vom VNC-Server")))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// One compressed pixel (R, G, B) → RGBA.
    fn cpixel(&mut self) -> Result<[u8; 4]> {
        let p = self.take(3)?;
        Ok([p[0], p[1], p[2], 255])
    }

    fn run_length(&mut self) -> Result<usize> {
        let mut len = 1usize;
        loop {
            let b = self.u8()?;
            len += b as usize;
            if b != 255 {
                return Ok(len);
            }
            if len > 64 * 64 {
                bail!(t!("Ungültige Daten vom VNC-Server"));
            }
        }
    }
}

/// Decodes inflated ZRLE data for the rectangle `(x, y, w, h)` into the framebuffer.
fn decode_zrle(
    data: &[u8],
    fb: &mut FrameBuffer,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> Result<()> {
    let bad = || anyhow!(t!("Ungültige Daten vom VNC-Server"));
    let mut r = Reader { data, pos: 0 };
    let mut tile = [[0u8; 4]; 64 * 64];
    let mut rgba = vec![0u8; 64 * 64 * 4];
    for ty in (0..h).step_by(64) {
        for tx in (0..w).step_by(64) {
            let tw = (w - tx).min(64);
            let th = (h - ty).min(64);
            let n = tw * th;
            let px = &mut tile[..n];
            let sub = r.u8()?;
            match sub {
                0 => {
                    for p in px.iter_mut() {
                        *p = r.cpixel()?;
                    }
                }
                1 => px.fill(r.cpixel()?),
                2..=16 => {
                    let mut pal = [[0u8; 4]; 16];
                    for p in pal.iter_mut().take(sub as usize) {
                        *p = r.cpixel()?;
                    }
                    let bits = match sub {
                        2 => 1,
                        3..=4 => 2,
                        _ => 4,
                    };
                    let row_bytes = (tw * bits).div_ceil(8);
                    let mask = (1u8 << bits) - 1;
                    for row in 0..th {
                        let line = r.take(row_bytes)?;
                        for col in 0..tw {
                            let bit = col * bits;
                            let idx = (line[bit / 8] >> (8 - bits - bit % 8)) & mask;
                            px[row * tw + col] = *pal.get(idx as usize).ok_or_else(bad)?;
                        }
                    }
                }
                128 => {
                    let mut i = 0;
                    while i < n {
                        let c = r.cpixel()?;
                        let len = r.run_length()?;
                        let end = i.checked_add(len).filter(|e| *e <= n).ok_or_else(bad)?;
                        px[i..end].fill(c);
                        i = end;
                    }
                }
                130..=255 => {
                    let size = (sub - 128) as usize;
                    let mut pal = [[0u8; 4]; 127];
                    for p in pal.iter_mut().take(size) {
                        *p = r.cpixel()?;
                    }
                    let mut i = 0;
                    while i < n {
                        let b = r.u8()?;
                        let idx = (b & 127) as usize;
                        if idx >= size {
                            return Err(bad());
                        }
                        let len = if b & 128 != 0 { r.run_length()? } else { 1 };
                        let end = i.checked_add(len).filter(|e| *e <= n).ok_or_else(bad)?;
                        px[i..end].fill(pal[idx]);
                        i = end;
                    }
                }
                _ => return Err(bad()),
            }
            for (o, p) in rgba.chunks_exact_mut(4).zip(px.iter()) {
                o.copy_from_slice(p);
            }
            fb.blit(x + tx, y + ty, tw, th, &rgba[..n * 4]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};
    use std::sync::atomic::AtomicUsize;
    use tokio::io::duplex;

    pub(super) static ZRLE_RECTS: AtomicUsize = AtomicUsize::new(0);

    fn sink() -> (Sink<VncEvent>, mpsc::UnboundedReceiver<VncEvent>) {
        Sink::new(egui::Context::default())
    }

    /// Runs the client handshake against a scripted server; returns the client result
    /// and everything the client wrote.
    async fn scripted(server: Vec<u8>, password: Option<&str>) -> (Result<ServerInit>, Vec<u8>) {
        let (client, mut srv) = duplex(1 << 16);
        srv.write_all(&server).await.unwrap();
        let (mut cr, mut cw) = tokio::io::split(client);
        let (events, _rx) = sink();
        let res = handshake(&mut cr, &mut cw, password, &events).await;
        drop(cw);
        drop(cr);
        let mut written = Vec::new();
        let _ =
            tokio::time::timeout(Duration::from_millis(200), srv.read_to_end(&mut written)).await;
        (res, written)
    }

    fn server_init(w: u16, h: u16, name: &str) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        v.extend_from_slice(&(name.len() as u32).to_be_bytes());
        v.extend_from_slice(name.as_bytes());
        v
    }

    #[tokio::test]
    async fn handshake_33_vnc_auth() {
        let mut s = b"RFB 003.003\n".to_vec();
        s.extend_from_slice(&2u32.to_be_bytes());
        let challenge = [7u8; 16];
        s.extend_from_slice(&challenge);
        s.extend_from_slice(&0u32.to_be_bytes());
        s.extend_from_slice(&server_init(640, 480, "desk"));
        let (res, written) = scripted(s, Some("secret")).await;
        let init = res.unwrap();
        assert_eq!(
            (init.width, init.height, init.name.as_str()),
            (640, 480, "desk")
        );
        assert_eq!(&written[..12], b"RFB 003.003\n");
        assert_eq!(&written[12..28], &vnc_auth_response("secret", &challenge));
        assert_eq!(written[28], 1); // shared
        assert_eq!(written.len(), 29);
    }

    #[tokio::test]
    async fn handshake_37_none_has_no_security_result() {
        let mut s = b"RFB 003.007\n".to_vec();
        s.extend_from_slice(&[2, 2, 1]);
        s.extend_from_slice(&server_init(800, 600, "x"));
        let (res, written) = scripted(s, None).await;
        assert_eq!(res.unwrap().width, 800);
        assert_eq!(written, b"RFB 003.007\n\x01\x01");
    }

    #[tokio::test]
    async fn handshake_38_wrong_password_reports_reason() {
        let mut s = b"RFB 003.008\n".to_vec();
        s.extend_from_slice(&[1, 2]);
        s.extend_from_slice(&[0u8; 16]);
        s.extend_from_slice(&1u32.to_be_bytes());
        let reason = b"Authentication failure";
        s.extend_from_slice(&(reason.len() as u32).to_be_bytes());
        s.extend_from_slice(reason);
        let (res, _) = scripted(s, Some("x")).await;
        let e = res.err().unwrap().to_string();
        assert!(
            e.contains("Passwort falsch") && e.contains("Authentication failure"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn handshake_errors() {
        // Apple only offers ARD authentication.
        let mut s = b"RFB 003.889\n".to_vec();
        s.extend_from_slice(&[2, 30, 33]);
        let e = scripted(s, Some("x")).await.0.err().unwrap().to_string();
        assert!(
            e.contains("Apple ARD") && e.contains("VNC-Viewer dürfen"),
            "{e}"
        );
        // Server refuses with a reason.
        let mut s = b"RFB 003.008\n".to_vec();
        s.push(0);
        s.extend_from_slice(&4u32.to_be_bytes());
        s.extend_from_slice(b"busy");
        let e = scripted(s, None).await.0.err().unwrap().to_string();
        assert!(e.ends_with("busy"), "{e}");
        // Password required but none given.
        let mut s = b"RFB 003.008\n".to_vec();
        s.extend_from_slice(&[1, 2]);
        let e = scripted(s, None).await.0.err().unwrap().to_string();
        assert!(e.contains("Passwort"), "{e}");
        // Not VNC at all.
        let e = scripted(b"SSH-2.0-OpenSSH_9\r\n".to_vec(), None)
            .await
            .0
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("kein VNC-Server"), "{e}");
        // Absurd screen size.
        let mut s = b"RFB 003.008\n".to_vec();
        s.extend_from_slice(&[1, 1]);
        s.extend_from_slice(&0u32.to_be_bytes());
        s.extend_from_slice(&server_init(60000, 10, ""));
        assert!(scripted(s, None).await.0.is_err());
    }

    #[tokio::test]
    async fn session_messages_from_scripted_server() {
        // FramebufferUpdate with Raw + CopyRect + DesktopSize, then cut text, bell,
        // colour map, and finally an unknown message type which ends the loop.
        let mut s = vec![0u8, 0, 0, 3];
        s.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 1]);
        s.extend_from_slice(&ENC_RAW.to_be_bytes());
        s.extend_from_slice(&[10, 20, 30, 0, 40, 50, 60, 0]);
        s.extend_from_slice(&[0, 2, 0, 0, 0, 2, 0, 1]);
        s.extend_from_slice(&ENC_COPYRECT.to_be_bytes());
        s.extend_from_slice(&[0, 0, 0, 0]);
        s.extend_from_slice(&[0, 0, 0, 0, 0, 4, 0, 4]);
        s.extend_from_slice(&ENC_DESKTOP_SIZE.to_be_bytes());
        s.extend_from_slice(&[3, 0, 0, 0, 0, 0, 0, 2, 0x68, 0xe9]);
        s.push(2);
        s.extend_from_slice(&[1, 0, 0, 0, 0, 1]);
        s.extend_from_slice(&[0; 6]);
        s.push(99);
        let fb = Arc::new(Mutex::new(FrameBuffer::new(4, 2)));
        let (events, mut rx) = sink();
        let (out, mut out_rx) = mpsc::unbounded_channel();
        let mut session = Session {
            fb: fb.clone(),
            events: &events,
            zlib: flate2::Decompress::new(true),
            zbuf: Vec::new(),
            out,
            stats: Stats::default(),
        };
        let mut r = &s[..];
        let e = session.read_loop(&mut r).await.err().unwrap().to_string();
        assert!(e.contains("99"), "{e}");
        let evs: Vec<VncEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            matches!(
                evs[0],
                VncEvent::Resized {
                    width: 4,
                    height: 4
                }
            ),
            "{evs:?}"
        );
        assert!(
            matches!(
                evs[1],
                VncEvent::Damage {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 1
                }
            ),
            "{evs:?}"
        );
        assert!(
            matches!(&evs[2], VncEvent::ServerCut(t) if t == "hé"),
            "{evs:?}"
        );
        // DesktopSize came last in the update: framebuffer was reset, full request sent.
        assert_eq!(fb.lock().unwrap().width, 4);
        assert_eq!(out_rx.try_recv().unwrap(), update_request(false, 4, 4));
    }

    fn px(fb: &FrameBuffer, x: usize, y: usize) -> [u8; 4] {
        let i = (y * fb.width + x) * 4;
        fb.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn des_known_vector_with_bit_reversed_key() {
        // Classic DES vector: key 0123456789ABCDEF, "Now is t" → 3FA40E8A984D4815.
        // VNC reverses the bits of every password byte, so the "password" bytes that
        // yield this key are the bit-reversed key bytes.
        let password: Vec<u8> = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]
            .iter()
            .map(|b| b.reverse_bits())
            .collect();
        let mut challenge = [0u8; 16];
        challenge[..8].copy_from_slice(b"Now is t");
        challenge[8..].copy_from_slice(b"Now is t");
        let resp = auth_response(&password, &challenge);
        let expected = [0x3F, 0xA4, 0x0E, 0x8A, 0x98, 0x4D, 0x48, 0x15];
        assert_eq!(resp[..8], expected);
        assert_eq!(resp[8..], expected);
    }

    #[test]
    fn vnc_auth_password_truncated_and_padded() {
        let challenge: [u8; 16] = core::array::from_fn(|i| i as u8 * 17);
        let resp = vnc_auth_response("password", &challenge);
        assert_eq!(vnc_auth_response("password123", &challenge), resp);
        assert_ne!(vnc_auth_response("passwor", &challenge), resp);
        assert_eq!(
            vnc_auth_response("pw", &challenge),
            vnc_auth_response("pw\0", &challenge)
        );
    }

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version(b"RFB 003.008\n"), Some(8));
        assert_eq!(parse_version(b"RFB 003.007\n"), Some(7));
        assert_eq!(parse_version(b"RFB 003.003\n"), Some(3));
        assert_eq!(parse_version(b"RFB 003.005\n"), Some(3));
        assert_eq!(parse_version(b"RFB 003.889\n"), Some(8));
        assert_eq!(parse_version(b"RFB 004.001\n"), Some(8));
        assert_eq!(parse_version(b"SSH-2.0-Open"), None);
    }

    #[test]
    fn pixel_format_bytes_are_rgbx() {
        let m = set_pixel_format_msg();
        assert_eq!(m.len(), 20);
        assert_eq!(&m[4..8], &[32, 24, 0, 1]);
        assert_eq!(&m[14..17], &[0, 8, 16]);
        let mut fb = FrameBuffer::new(2, 1);
        fb.blit(0, 0, 2, 1, &[1, 2, 3, 0, 4, 5, 6, 99]);
        assert_eq!(fb.rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn blit_and_copyrect_are_clipped() {
        let mut fb = FrameBuffer::new(4, 4);
        let src: Vec<u8> = [9u8, 9, 9, 0].repeat(9);
        fb.blit(3, 3, 3, 3, &src); // only (3,3) is inside
        assert_eq!(px(&fb, 3, 3), [9, 9, 9, 255]);
        assert_eq!(px(&fb, 2, 3), [0, 0, 0, 255]);
        fb.blit(10, 10, 3, 3, &src);
        fb.blit(0, 0, 3, 3, &src[..4]); // short source: no panic
        fb.copy_rect(3, 3, 0, 0, 100, 100);
        assert_eq!(px(&fb, 0, 0), [9, 9, 9, 255]);
        fb.copy_rect(60000, 0, 0, 0, 2, 2);
        // Overlapping copy downwards.
        let mut fb = FrameBuffer::new(1, 3);
        fb.blit(0, 0, 1, 1, &[7, 7, 7, 0]);
        fb.copy_rect(0, 0, 0, 1, 1, 2);
        assert_eq!(px(&fb, 0, 1), [7, 7, 7, 255]);
        assert_eq!(px(&fb, 0, 2), [0, 0, 0, 255]);
    }

    fn zlib(c: &mut Compress, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 1024);
        c.compress_vec(data, &mut out, FlushCompress::Sync).unwrap();
        out
    }

    /// Tiles for a 66×2 rectangle (two tiles: 64×2 and 2×2) using several subencodings,
    /// sent as two rectangles through one persistent zlib stream.
    #[test]
    fn zrle_subencodings_and_persistent_stream() {
        let red = [255u8, 0, 0];
        let green = [0u8, 255, 0];
        let blue = [0u8, 0, 255];
        let mut c = Compress::new(Compression::default(), true);
        let mut d = flate2::Decompress::new(true);
        let mut buf = Vec::new();
        let mut fb = FrameBuffer::new(70, 8);

        // Rect 1 at (0,0) 66×2: tile A = plain RLE (red ×100, green ×28),
        // tile B = packed palette (2 colours, 1 bit): rows "10", "01".
        let mut t = vec![128u8];
        t.extend_from_slice(&red);
        t.extend_from_slice(&[99]); // run 100
        t.extend_from_slice(&green);
        t.extend_from_slice(&[27]);
        t.push(2);
        t.extend_from_slice(&blue);
        t.extend_from_slice(&red);
        t.extend_from_slice(&[0b1000_0000, 0b0100_0000]);
        let z = zlib(&mut c, &t);
        inflate(&mut d, &z, &mut buf, zrle_max_len(66, 2)).unwrap();
        decode_zrle(&buf, &mut fb, 0, 0, 66, 2).unwrap();
        assert_eq!(px(&fb, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&fb, 63, 0), [255, 0, 0, 255]);
        assert_eq!(px(&fb, 35, 1), [255, 0, 0, 255]); // index 99 = row 1, col 35
        assert_eq!(px(&fb, 36, 1), [0, 255, 0, 255]);
        assert_eq!(px(&fb, 64, 0), [255, 0, 0, 255]);
        assert_eq!(px(&fb, 65, 0), [0, 0, 255, 255]);
        assert_eq!(px(&fb, 64, 1), [0, 0, 255, 255]);
        assert_eq!(px(&fb, 65, 1), [255, 0, 0, 255]);

        // Rect 2 at (0,4) 3×3 on the same stream: palette RLE + then solid + raw rects.
        let mut t = vec![130u8];
        t.extend_from_slice(&green);
        t.extend_from_slice(&blue);
        t.push(0x80); // green, run follows
        t.push(3); // run 4
        t.push(1); // single blue
        t.push(0x80);
        t.push(3); // 4 green → 9 total
        let z = zlib(&mut c, &t);
        inflate(&mut d, &z, &mut buf, zrle_max_len(3, 3)).unwrap();
        decode_zrle(&buf, &mut fb, 0, 4, 3, 3).unwrap();
        assert_eq!(px(&fb, 0, 4), [0, 255, 0, 255]);
        assert_eq!(px(&fb, 1, 5), [0, 0, 255, 255]);
        assert_eq!(px(&fb, 2, 6), [0, 255, 0, 255]);

        let mut t = vec![1u8];
        t.extend_from_slice(&blue);
        t.push(0);
        for i in 0..4u8 {
            t.extend_from_slice(&[i, i, i]);
        }
        let z = zlib(&mut c, &t);
        inflate(&mut d, &z, &mut buf, 10_000).unwrap();
        // Solid 2×2 at (10,0), then raw 2×2 at (68,6) partly outside → clipped.
        let split = 4;
        decode_zrle(&buf[..split], &mut fb, 10, 0, 2, 2).unwrap();
        decode_zrle(&buf[split..], &mut fb, 68, 6, 2, 2).unwrap();
        assert_eq!(px(&fb, 11, 1), [0, 0, 255, 255]);
        assert_eq!(px(&fb, 68, 6), [0, 0, 0, 255]);
        assert_eq!(px(&fb, 69, 7), [3, 3, 3, 255]);
    }

    #[test]
    fn zrle_packed_palette_widths() {
        // 5 colours → 4 bits per pixel, 3 colours → 2 bits; 3 px wide → padded rows.
        let mut fb = FrameBuffer::new(3, 2);
        let mut t = vec![5u8];
        for i in 0..5u8 {
            t.extend_from_slice(&[i * 10, 0, 0]);
        }
        t.extend_from_slice(&[0x01, 0x20, 0x34, 0x00]);
        decode_zrle(&t, &mut fb, 0, 0, 3, 2).unwrap();
        let reds: Vec<u8> = (0..6).map(|i| fb.rgba[i * 4]).collect();
        assert_eq!(reds, vec![0, 10, 20, 30, 40, 0]);

        let mut t = vec![3u8];
        for i in 0..3u8 {
            t.extend_from_slice(&[0, i * 10, 0]);
        }
        t.extend_from_slice(&[0b0001_1000, 0b1001_0000]);
        decode_zrle(&t, &mut fb, 0, 0, 3, 2).unwrap();
        let greens: Vec<u8> = (0..6).map(|i| fb.rgba[i * 4 + 1]).collect();
        assert_eq!(greens, vec![0, 10, 20, 20, 10, 0]);
    }

    #[test]
    fn zrle_malformed_never_panics() {
        let mut fb = FrameBuffer::new(8, 8);
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0, 1, 2],
            vec![17],
            vec![129],
            vec![
                128, 1, 2, 3, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                255, 255, 255, 255, 255,
            ],
            vec![128, 1, 2, 3, 200],
            vec![130, 1, 2, 3, 4, 5, 6, 5],
            vec![2, 1, 1, 1],
        ];
        for c in cases {
            assert!(decode_zrle(&c, &mut fb, 0, 0, 8, 8).is_err());
        }
        // Random garbage of many sizes.
        let mut seed = 12345u32;
        for len in 0..400 {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (seed >> 16) as u8
                })
                .collect();
            let _ = decode_zrle(&data, &mut fb, 0, 0, 70, 70);
            let mut d = flate2::Decompress::new(true);
            let mut out = Vec::new();
            let _ = inflate(&mut d, &data, &mut out, 1000);
        }
    }

    #[test]
    fn client_messages() {
        assert_eq!(pointer_msg(1, 2, 5), vec![5, 5, 0, 1, 0, 2]);
        assert_eq!(key_msg(0xff0d, true), vec![4, 1, 0, 0, 0, 0, 0xff, 0x0d]);
        assert_eq!(
            client_cut_msg("ä€\r\n"),
            vec![6, 0, 0, 0, 0, 0, 0, 3, 0xe4, b'?', b'\n']
        );
        assert_eq!(
            update_request(true, 800, 600),
            vec![3, 1, 0, 0, 0, 0, 3, 32, 2, 88]
        );
        assert_eq!(latin1(&[0x48, 0xe9]), "Hé");
    }

    /// Talks to real TigerVNC servers. Needs `Xvnc`, `vncpasswd`, `xsetroot`, `xclip`
    /// and `xrandr`; run with `cargo test -- --ignored live_xvnc --nocapture`.
    #[test]
    #[ignore]
    fn live_xvnc() {
        use std::process::{Command, Stdio};

        struct Kill(Vec<std::process::Child>);
        impl Drop for Kill {
            fn drop(&mut self) {
                for c in &mut self.0 {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("aurora-vnc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pwfile = dir.join("passwd");
        let out = Command::new("vncpasswd")
            .arg("-f")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(b"geheim12\n")?;
                c.wait_with_output()
            })
            .expect("vncpasswd");
        std::fs::write(&pwfile, out.stdout).unwrap();

        let xvnc = |display: u32, extra: &[&str]| {
            Command::new("Xvnc")
                .arg(format!(":{display}"))
                .args(["-geometry", "800x600", "-depth", "24"])
                .args(["-rfbport", &(5900 + display).to_string()])
                .args(extra)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Xvnc")
        };
        let mut helpers = Kill(Vec::new());
        let _servers = Kill(vec![
            xvnc(55, &["-SecurityTypes", "None"]),
            xvnc(
                56,
                &[
                    "-SecurityTypes",
                    "VncAuth",
                    "-PasswordFile",
                    pwfile.to_str().unwrap(),
                ],
            ),
            xvnc(57, &["-SecurityTypes", "TLSNone"]),
        ]);
        std::thread::sleep(Duration::from_millis(1500));
        let x = |args: &[&str]| {
            let st = Command::new(args[0])
                .args(&args[1..])
                .env("DISPLAY", ":55")
                .status()
                .unwrap();
            assert!(st.success(), "{args:?}");
        };
        x(&["xsetroot", "-solid", "#c03020"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let connect = |port: u16, password: Option<&str>| {
            let fb = Arc::new(Mutex::new(FrameBuffer::default()));
            let (events, rx) = sink();
            let params = VncParams {
                host: "127.0.0.1".into(),
                port,
                password: password.map(Into::into),
                view_only: false,
            };
            let tx = spawn(&rt, params, fb.clone(), events);
            (tx, rx, fb)
        };
        let next = |rx: &mut mpsc::UnboundedReceiver<VncEvent>| {
            rt.block_on(async { tokio::time::timeout(Duration::from_secs(10), rx.recv()).await })
                .expect("event timeout")
                .expect("channel closed")
        };
        let wait_for = |rx: &mut mpsc::UnboundedReceiver<VncEvent>,
                        want: &dyn Fn(&VncEvent) -> bool| {
            loop {
                let ev = next(rx);
                eprintln!("event: {ev:?}");
                if want(&ev) {
                    return ev;
                }
                assert!(!matches!(ev, VncEvent::Closed(_)), "closed early: {ev:?}");
            }
        };
        let closed = |rx: &mut mpsc::UnboundedReceiver<VncEvent>| loop {
            if let VncEvent::Closed(e) = next(rx) {
                eprintln!("closed: {e:?}");
                return e;
            }
        };

        // 1. No authentication: full update with the root colour, ZRLE used.
        let (tx, mut rx, fb) = connect(5955, None);
        wait_for(&mut rx, &|e| {
            matches!(
                e,
                VncEvent::Connected {
                    width: 800,
                    height: 600,
                    ..
                }
            )
        });
        wait_for(&mut rx, &|e| {
            matches!(e, VncEvent::Damage { w: 800, h: 600, .. })
        });
        {
            let f = fb.lock().unwrap();
            let target = [0xc0, 0x30, 0x20, 255];
            let hits = f.rgba.chunks_exact(4).filter(|p| *p == target).count();
            eprintln!("root-colour pixels: {hits} of {}", f.width * f.height);
            assert!(hits > f.width * f.height * 9 / 10);
        }
        let zrle = ZRLE_RECTS.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("ZRLE rectangles so far: {zrle}");
        assert!(zrle > 0);

        // Detailed content (text, shapes): compare with a server-side screenshot.
        for args in [
            &[
                "xterm",
                "-geometry",
                "80x30+10+10",
                "-e",
                "ls -la /usr/bin; sleep 60",
            ][..],
            &["xlogo", "-geometry", "200x200+560+40"][..],
            &["xeyes", "-geometry", "150x100+560+300"][..],
        ] {
            if let Ok(c) = Command::new(args[0])
                .args(&args[1..])
                .env("DISPLAY", ":55")
                .stderr(Stdio::null())
                .spawn()
            {
                helpers.0.push(c);
            }
        }
        // Move the server-drawn cursor out of the way.
        let _ = Command::new("xdotool")
            .args(["mousemove", "799", "599"])
            .env("DISPLAY", ":55")
            .status();
        std::thread::sleep(Duration::from_millis(2000));
        tx.send(VncCmd::Refresh).unwrap();
        // Wait until the updates settle.
        let mut n = 0;
        while let Ok(Some(_)) =
            rt.block_on(async { tokio::time::timeout(Duration::from_millis(800), rx.recv()).await })
        {
            n += 1;
        }
        let shot = Command::new("sh")
            .args(["-c", "xwd -root -silent -display :55 | convert xwd:- rgb:-"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(shot.len(), 800 * 600 * 3);
        {
            let f = fb.lock().unwrap();
            let differ = f
                .rgba
                .chunks_exact(4)
                .zip(shot.chunks_exact(3))
                .filter(|(a, b)| a[..3] != b[..])
                .count();
            let distinct: std::collections::HashSet<&[u8]> = f.rgba.chunks_exact(4).collect();
            eprintln!(
                "after {n} events: {differ} pixels differ from xwd, {} distinct colours, \
                 ZRLE rectangles {}",
                distinct.len(),
                ZRLE_RECTS.load(std::sync::atomic::Ordering::Relaxed)
            );
            assert!(distinct.len() > 3);
            // The server draws the cursor into the framebuffer, xwd does not.
            assert!(differ < 600, "{differ}");
        }

        // Incremental update after a change.
        x(&["xsetroot", "-solid", "#2040e0"]);
        loop {
            wait_for(&mut rx, &|e| matches!(e, VncEvent::Damage { .. }));
            let f = fb.lock().unwrap();
            if f.rgba[(500 * f.width + 780) * 4..][..4] == [0x20, 0x40, 0xe0, 255] {
                break;
            }
        }

        // Clipboard from the server (TigerVNC forwards the X clipboard).
        let mut xclip = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .env("DISPLAY", ":55")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            let mut stdin = xclip.stdin.take().unwrap();
            stdin.write_all("Grüße".as_bytes()).unwrap();
        }
        let _ = xclip.wait();
        wait_for(
            &mut rx,
            &|e| matches!(e, VncEvent::ServerCut(t) if t == "Grüße"),
        );

        // Input is accepted; a server-side resize arrives as an event.
        for cmd in [
            VncCmd::Pointer {
                x: 10,
                y: 10,
                buttons: 1,
            },
            VncCmd::Pointer {
                x: 10,
                y: 10,
                buttons: 0,
            },
            VncCmd::Key {
                keysym: 0x61,
                down: true,
            },
            VncCmd::Key {
                keysym: 0x61,
                down: false,
            },
            VncCmd::ClientCut("hallo".into()),
            VncCmd::Refresh,
        ] {
            tx.send(cmd).unwrap();
        }
        x(&["xrandr", "-s", "1024x768"]);
        wait_for(&mut rx, &|e| {
            matches!(
                e,
                VncEvent::Resized {
                    width: 1024,
                    height: 768
                }
            )
        });
        wait_for(&mut rx, &|e| {
            matches!(
                e,
                VncEvent::Damage {
                    w: 1024,
                    h: 768,
                    ..
                }
            )
        });
        assert_eq!(fb.lock().unwrap().width, 1024);
        tx.send(VncCmd::Close).unwrap();
        assert_eq!(closed(&mut rx), None);

        // 2. VNC authentication: right (truncated to 8) and wrong password.
        let (_tx, mut rx, _) = connect(5956, Some("geheim12-und-mehr"));
        wait_for(&mut rx, &|e| matches!(e, VncEvent::Connected { .. }));
        let (_tx2, mut rx, _) = connect(5956, Some("falsch"));
        assert!(closed(&mut rx).unwrap().contains("Passwort falsch"));

        // 3. Only VeNCrypt offered → explained error.
        let (_tx, mut rx, _) = connect(5957, None);
        assert!(closed(&mut rx).unwrap().contains("VeNCrypt"));

        // 4. Nothing listening.
        let (_tx, mut rx, _) = connect(5999, None);
        assert!(closed(&mut rx).unwrap().contains("abgelehnt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
