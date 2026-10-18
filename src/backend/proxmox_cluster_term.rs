//! Node shell without SSH: Proxmox' `termproxy` behind the API websocket.
//!
//! `POST /nodes/{node}/termproxy` returns a port and a one-time ticket; the websocket
//! `…/vncwebsocket?port=…&vncticket=…` (authenticated like any API call) then speaks a
//! small text protocol: `user:ticket\n` first (answered with `OK`), then
//! `0:LEN:DATA` for input, `1:COLS:ROWS:` for a resize and `2` as keep-alive; output
//! arrives as plain websocket messages. The websocket runs over the same pinned
//! certificate check as the REST calls; the framing (RFC 6455, client side) is here.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rand::RngCore;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use tokio::sync::mpsc;

use super::super::{Client, Conn, Error, Method, PinVerifier, Sink, enc, valid_node};
use crate::backend::{TermCmd, TermEvent};

#[derive(Clone, Debug)]
pub struct ShellParams {
    pub conn: Conn,
    pub node: String,
}

// ------------------------------------------------------------------ termproxy protocol

/// Keyboard input: `0:LEN:DATA` (LEN in bytes).
pub fn input_msg(data: &[u8]) -> Vec<u8> {
    let mut m = format!("0:{}:", data.len()).into_bytes();
    m.extend_from_slice(data);
    m
}

pub fn resize_msg(cols: u16, rows: u16) -> Vec<u8> {
    format!("1:{cols}:{rows}:").into_bytes()
}

pub const PING_MSG: &[u8] = b"2";

/// The first message: `user:ticket\n`.
pub fn auth_msg(user: &str, ticket: &str) -> Vec<u8> {
    format!("{user}:{ticket}\n").into_bytes()
}

/// `vncwebsocket` path for a termproxy port and ticket.
pub fn websocket_path(node: &str, port: u16, ticket: &str) -> Option<String> {
    valid_node(node).then(|| {
        format!(
            "/api2/json/nodes/{node}/vncwebsocket?port={port}&vncticket={}",
            enc(ticket)
        )
    })
}

// ------------------------------------------------------------------ websocket framing

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

/// One client frame (always masked, always final).
pub fn encode_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | (opcode & 0x0f));
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// Collects bytes from the socket and cuts them into frames.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

/// Larger messages are a protocol error here (terminal output comes in small chunks).
const MAX_FRAME: u64 = 16 * 1024 * 1024;

impl FrameReader {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        let b = &self.buf;
        if b.len() < 2 {
            return Ok(None);
        }
        let fin = b[0] & 0x80 != 0;
        let opcode = b[0] & 0x0f;
        let masked = b[1] & 0x80 != 0;
        let (len, mut at) = match b[1] & 0x7f {
            126 => {
                if b.len() < 4 {
                    return Ok(None);
                }
                (u16::from_be_bytes([b[2], b[3]]) as u64, 4)
            }
            127 => {
                if b.len() < 10 {
                    return Ok(None);
                }
                let mut l = [0u8; 8];
                l.copy_from_slice(&b[2..10]);
                (u64::from_be_bytes(l), 10)
            }
            n => (n as u64, 2),
        };
        if len > MAX_FRAME {
            return Err("websocket frame too large".into());
        }
        let mask = if masked {
            if b.len() < at + 4 {
                return Ok(None);
            }
            let m = [b[at], b[at + 1], b[at + 2], b[at + 3]];
            at += 4;
            Some(m)
        } else {
            None
        };
        let end = at + len as usize;
        if b.len() < end {
            return Ok(None);
        }
        let mut payload = b[at..end].to_vec();
        if let Some(m) = mask {
            for (i, x) in payload.iter_mut().enumerate() {
                *x ^= m[i % 4];
            }
        }
        self.buf.drain(..end);
        Ok(Some(Frame {
            fin,
            opcode,
            payload,
        }))
    }
}

/// SHA-1 (only for `Sec-WebSocket-Accept`).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Expected `Sec-WebSocket-Accept` for a key.
pub fn accept_key(key: &str) -> String {
    let mut s = key.as_bytes().to_vec();
    s.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    B64.encode(sha1(&s))
}

/// Checks the handshake answer: status 101 and the right accept key.
pub fn check_handshake(response: &str, key: &str) -> Result<(), String> {
    let mut lines = response.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if code != "101" {
        return Err(match code {
            "401" => t!("Anmeldung fehlgeschlagen: {}", status.trim()),
            "403" => t!("keine Berechtigung").to_string(),
            _ => status.trim().to_string(),
        });
    }
    let accept = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept"))
        .map(|(_, v)| v.trim().to_string());
    if accept.as_deref() != Some(accept_key(key).as_str()) {
        return Err("websocket handshake: bad accept key".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ session

type Tls = StreamOwned<ClientConnection, TcpStream>;

fn mask() -> [u8; 4] {
    let mut m = [0u8; 4];
    rand::rngs::OsRng.fill_bytes(&mut m);
    m
}

fn send(tls: &mut Tls, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    tls.write_all(&encode_frame(opcode, payload, mask()))?;
    tls.flush()
}

fn timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

pub fn spawn(
    params: ShellParams,
    cols: u16,
    rows: u16,
    events: Sink<TermEvent>,
) -> mpsc::UnboundedSender<TermCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let r = run(&params, cols, rows, rx, &events);
        events.send(TermEvent::Closed(r.err()));
    });
    tx
}

/// Asks for a termproxy and returns (port, ticket, user).
fn termproxy(c: &mut Client, node: &str) -> Result<(u16, String, String), String> {
    let v = c
        .request(Method::Post, &format!("/nodes/{node}/termproxy"), &[])
        .map_err(|e| match e {
            Error::Http(403, _) => t!(
                "Keine Berechtigung für die Node-Shell – das Konto oder Token braucht Sys.Console auf /nodes/{}",
                node
            ),
            other => other.to_string(),
        })?;
    let port = v["port"]
        .as_u64()
        .or_else(|| v["port"].as_str().and_then(|p| p.parse().ok()))
        .and_then(|p| u16::try_from(p).ok())
        .ok_or_else(|| t!("termproxy: keine Port-Angabe").to_string())?;
    let ticket = v["ticket"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| t!("termproxy: kein Ticket").to_string())?
        .to_string();
    let user = v["user"].as_str().unwrap_or_default().to_string();
    Ok((port, ticket, user))
}

fn connect_tls(conn: &Conn) -> Result<Tls, String> {
    let host = conn
        .host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let addr = (host, conn.port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| t!("Adresse nicht gefunden").to_string())?;
    let sock =
        TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    let _ = sock.set_nodelay(true);
    sock.set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinVerifier {
        pin: conn.pin.clone(),
        seen: Arc::new(Mutex::new(None)),
        algs: provider.signature_verification_algorithms,
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let tls = ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    Ok(StreamOwned::new(tls, sock))
}

fn run(
    p: &ShellParams,
    cols: u16,
    rows: u16,
    mut cmds: mpsc::UnboundedReceiver<TermCmd>,
    events: &Sink<TermEvent>,
) -> Result<(), String> {
    if !valid_node(&p.node) {
        return Err(t!("ungültiger Node-Name").into());
    }
    if p.conn.pin.is_none() {
        return Err(t!("Zertifikat noch nicht bestätigt").into());
    }
    events.send(TermEvent::Status(t!("Öffne Shell auf {}…", p.node)));
    let mut c = Client::new(&p.conn);
    let (port, ticket, user) = termproxy(&mut c, &p.node)?;
    let user = if user.is_empty() {
        match &p.conn.auth {
            super::super::Auth::Token { id, .. } => id.clone(),
            super::super::Auth::Password { user, .. } => user.clone(),
        }
    } else {
        user
    };
    let path = websocket_path(&p.node, port, &ticket).ok_or("node")?;
    let mut tls = connect_tls(&p.conn)?;

    // --- handshake
    let mut raw_key = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut raw_key);
    let key = B64.encode(raw_key);
    let host = if p.conn.host.contains(':') && !p.conn.host.starts_with('[') {
        format!("[{}]:{}", p.conn.host, p.conn.port)
    } else {
        format!("{}:{}", p.conn.host, p.conn.port)
    };
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Aurora\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: binary\r\nOrigin: https://{host}\r\n"
    );
    for (k, v) in c.auth_headers(Method::Get) {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    tls.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    tls.flush().map_err(|e| e.to_string())?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            return Err("websocket handshake: header too long".into());
        }
        match tls.read(&mut byte) {
            Ok(0) => return Err(t!("Verbindung geschlossen").into()),
            Ok(_) => head.push(byte[0]),
            Err(e) => return Err(e.to_string()),
        }
    }
    check_handshake(&String::from_utf8_lossy(&head), &key)?;

    // --- termproxy login
    send(&mut tls, OP_BINARY, &auth_msg(&user, &ticket)).map_err(|e| e.to_string())?;
    let mut reader = FrameReader::default();
    let mut buf = vec![0u8; 32 * 1024];
    let mut authed = false;
    let mut message: Vec<u8> = Vec::new();
    tls.get_ref()
        .set_read_timeout(Some(Duration::from_millis(40)))
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut last_ping = Instant::now();
    loop {
        // Input from the UI.
        loop {
            match cmds.try_recv() {
                Ok(TermCmd::Input(data)) if authed => {
                    send(&mut tls, OP_BINARY, &input_msg(&data)).map_err(|e| e.to_string())?;
                }
                Ok(TermCmd::Resize { cols, rows }) if authed => {
                    send(&mut tls, OP_BINARY, &resize_msg(cols, rows))
                        .map_err(|e| e.to_string())?;
                }
                Ok(TermCmd::Close) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    let _ = send(&mut tls, OP_CLOSE, &1000u16.to_be_bytes());
                    return Ok(());
                }
                Ok(_) => {}
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }
        if authed && last_ping.elapsed() > Duration::from_secs(25) {
            send(&mut tls, OP_BINARY, PING_MSG).map_err(|e| e.to_string())?;
            last_ping = Instant::now();
        }
        if !authed && started.elapsed() > Duration::from_secs(15) {
            return Err(t!("Zeitüberschreitung").into());
        }
        // Output from the node.
        match tls.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => reader.push(&buf[..n]),
            Err(e) if timeout(&e) => continue,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        while let Some(f) = reader.next_frame()? {
            match f.opcode {
                OP_PING => send(&mut tls, OP_PONG, &f.payload).map_err(|e| e.to_string())?,
                OP_PONG => {}
                OP_CLOSE => return Ok(()),
                OP_TEXT | OP_BINARY | OP_CONT => {
                    message.extend_from_slice(&f.payload);
                    if !f.fin {
                        continue;
                    }
                    let data = std::mem::take(&mut message);
                    if authed {
                        events.send(TermEvent::Output(data));
                    } else if let Some(rest) = data.strip_prefix(b"OK") {
                        authed = true;
                        events.send(TermEvent::Connected);
                        send(&mut tls, OP_BINARY, &resize_msg(cols, rows))
                            .map_err(|e| e.to_string())?;
                        if !rest.is_empty() {
                            events.send(TermEvent::Output(rest.to_vec()));
                        }
                    } else {
                        return Err(t!(
                            "termproxy hat die Anmeldung abgelehnt: {}",
                            String::from_utf8_lossy(&data).trim()
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termproxy_messages() {
        assert_eq!(input_msg(b"ls\r"), b"0:3:ls\r");
        assert_eq!(input_msg("ä".as_bytes()), "0:2:ä".as_bytes());
        assert_eq!(resize_msg(120, 40), b"1:120:40:");
        assert_eq!(auth_msg("root@pam", "PVEVNC:abc"), b"root@pam:PVEVNC:abc\n");
        assert_eq!(
            websocket_path("pve1", 5900, "PVEVNC:66AA::x/y+z=").unwrap(),
            "/api2/json/nodes/pve1/vncwebsocket?port=5900&vncticket=PVEVNC%3A66AA%3A%3Ax%2Fy%2Bz%3D"
        );
        assert_eq!(websocket_path("../x", 5900, "t"), None);
    }

    #[test]
    fn websocket_accept() {
        // RFC 6455, section 1.3.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        let ok = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
        assert_eq!(check_handshake(ok, "dGhlIHNhbXBsZSBub25jZQ=="), Ok(()));
        assert!(check_handshake(ok, "other").is_err());
        assert!(check_handshake("HTTP/1.1 401 No ticket\r\n\r\n", "x").is_err());
        // SHA-1 of "abc".
        let h: String = sha1(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(h, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn frames_roundtrip() {
        for len in [0usize, 5, 125, 126, 300, 70_000] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let wire = encode_frame(OP_BINARY, &payload, [1, 2, 3, 4]);
            assert_eq!(wire[1] & 0x80, 0x80, "client frames are masked");
            let mut r = FrameReader::default();
            // Arrives in pieces.
            let (a, b) = wire.split_at(wire.len() / 2);
            r.push(a);
            if len > 10 {
                assert_eq!(r.next_frame().unwrap(), None);
            }
            r.push(b);
            let f = r.next_frame().unwrap().unwrap();
            assert_eq!((f.fin, f.opcode), (true, OP_BINARY));
            assert_eq!(f.payload, payload);
            assert_eq!(r.next_frame().unwrap(), None);
        }
        // Unmasked server frames, two in one read.
        let mut r = FrameReader::default();
        r.push(&[0x82, 2, b'O', b'K', 0x01, 1, b'x', 0x80, 1, b'y']);
        assert_eq!(r.next_frame().unwrap().unwrap().payload, b"OK");
        let f = r.next_frame().unwrap().unwrap();
        assert_eq!(
            (f.fin, f.opcode, f.payload.as_slice()),
            (false, OP_TEXT, &b"x"[..])
        );
        let f = r.next_frame().unwrap().unwrap();
        assert_eq!((f.fin, f.opcode), (true, OP_CONT));
        let mut r = FrameReader::default();
        r.push(&[0x82, 127, 0xff, 0, 0, 0, 0, 0, 0, 0]);
        assert!(r.next_frame().is_err());
    }
}
