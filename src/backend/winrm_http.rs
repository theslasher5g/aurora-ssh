//! The HTTP side of WinRM: one blocking HTTP/1.1 connection (plain or TLS pinned to a
//! certificate fingerprint, trust on first use) with Basic or NTLM authentication. NTLM
//! authenticates the *connection*, so the whole exchange stays on one TCP connection.
//! Response sizes and header sizes are bounded; every read has a timeout.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

use super::proxmox::{PinCheck, check_pin, fingerprint};
use super::winrm_ntlm as ntlm;
use super::winrm_types::WinrmAuth;

const MAX_HEAD: usize = 64 * 1024;
/// Largest answer body we accept (a Receive answer is a few hundred KiB at most).
pub const MAX_BODY: usize = 8 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longer than the WS-Management operation timeout (10 s) used for Receive.
const IO_TIMEOUT: Duration = Duration::from_secs(45);

/// Where and how to talk.
#[derive(Clone)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub https: bool,
    pub auth: WinrmAuth,
    pub user: String,
    pub domain: String,
    pub password: String,
    /// Pinned certificate fingerprint (HTTPS); `None` = not trusted yet.
    pub pin: Option<String>,
}

impl Target {
    pub fn url(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!(
            "{}://{host}:{}/wsman",
            if self.https { "https" } else { "http" },
            self.port
        )
    }
}

/// Why a connection could not be made.
#[derive(Debug)]
pub enum ConnectError {
    /// The certificate is not pinned yet: its fingerprint, for the user to confirm.
    Untrusted(String),
    /// The certificate differs from the pinned one.
    PinChanged {
        pinned: String,
        got: String,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Untrusted(fp) => {
                write!(f, "{}", t!("Zertifikat noch nicht bestätigt: {}", fp))
            }
            ConnectError::PinChanged { got, .. } => {
                write!(f, "{}", t!("Das Zertifikat hat sich geändert: {}", got))
            }
            ConnectError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<anyhow::Error> for ConnectError {
    fn from(e: anyhow::Error) -> Self {
        ConnectError::Other(e)
    }
}

// ------------------------------------------------------------------ TLS pin

/// Accepts exactly the pinned certificate and remembers what the server presented.
#[derive(Debug)]
struct PinVerifier {
    pin: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(fp.clone());
        }
        match check_pin(self.pin.as_deref(), &fp) {
            PinCheck::Match => Ok(ServerCertVerified::assertion()),
            PinCheck::Unpinned => Err(rustls::Error::General("certificate not trusted yet".into())),
            PinCheck::Mismatch => Err(rustls::Error::General(
                "certificate does not match the pin".into(),
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

fn tcp(host: &str, port: u16) -> Result<TcpStream> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .with_context(|| t!("Host {} nicht gefunden", host))?
        .collect();
    let mut last = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_read_timeout(Some(IO_TIMEOUT))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => anyhow!(
            "{}",
            t!("Verbindung zu {}:{} fehlgeschlagen: {}", host, port, e)
        ),
        None => anyhow!("{}", t!("Host {} nicht gefunden", host)),
    })
}

fn connect_stream(t: &Target) -> Result<Stream, ConnectError> {
    let sock = tcp(&t.host, t.port)?;
    if !t.https {
        return Ok(Stream::Plain(sock));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let seen = Arc::new(Mutex::new(None));
    let verifier = PinVerifier {
        pin: t.pin.clone(),
        seen: seen.clone(),
        algs: provider.signature_verification_algorithms,
    };
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| anyhow!("{e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = ServerName::try_from(t.host.trim_matches(['[', ']']).to_string())
        .map_err(|_| anyhow!("invalid host name"))?;
    let conn = ClientConnection::new(Arc::new(cfg), name).map_err(|e| anyhow!("{e}"))?;
    let mut tls = StreamOwned::new(conn, sock);
    // Complete the handshake now so a pin problem shows up here and not on first use.
    while tls.conn.is_handshaking() {
        if let Err(e) = tls.conn.complete_io(&mut tls.sock) {
            let fp = seen.lock().ok().and_then(|s| s.clone());
            if let Some(fp) = fp {
                match check_pin(t.pin.as_deref(), &fp) {
                    PinCheck::Unpinned => return Err(ConnectError::Untrusted(fp)),
                    PinCheck::Mismatch => {
                        return Err(ConnectError::PinChanged {
                            pinned: t.pin.clone().unwrap_or_default(),
                            got: fp,
                        });
                    }
                    PinCheck::Match => {}
                }
            }
            return Err(ConnectError::Other(anyhow!(
                "{}",
                t!("TLS-Verbindung fehlgeschlagen: {}", e)
            )));
        }
    }
    Ok(Stream::Tls(Box::new(tls)))
}

// ------------------------------------------------------------------ HTTP

#[derive(Debug, Default)]
pub struct Response {
    pub status: u16,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers
            .iter()
            .filter(move |(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn closes(&self) -> bool {
        self.header_all("connection")
            .any(|v| v.eq_ignore_ascii_case("close"))
    }
}

fn read_until_head_end<R: Read>(r: &mut R) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(p + 4);
            buf.truncate(p);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            bail!(
                "{}",
                t!("Die Antwort des Servers ist ungültig (Kopfzeilen zu lang)")
            );
        }
        let n = r.read(&mut chunk)?;
        if n == 0 {
            bail!("{}", t!("Der Server hat die Verbindung beendet"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Reads one complete response (status line, headers, body with Content-Length or chunked).
pub fn read_response<R: Read>(r: &mut R) -> Result<Response> {
    let (head, mut rest) = read_until_head_end(r)?;
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|_| status_line.starts_with("HTTP/1."))
        .ok_or_else(|| anyhow!("{}", t!("Die Antwort des Servers ist kein HTTP")))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let mut resp = Response {
        status,
        headers,
        body: Vec::new(),
    };
    let chunked = resp
        .header_all("transfer-encoding")
        .any(|v| v.to_ascii_lowercase().contains("chunked"));
    let length: Option<usize> = resp
        .header_all("content-length")
        .next()
        .and_then(|v| v.parse().ok());
    if status == 204 || status == 304 || (100..200).contains(&status) {
        return Ok(resp);
    }
    if chunked {
        resp.body = read_chunked(r, &mut rest)?;
    } else if let Some(n) = length {
        if n > MAX_BODY {
            bail!("{}", t!("Die Antwort des Servers ist zu groß"));
        }
        while rest.len() < n {
            let mut chunk = [0u8; 8192];
            let got = r.read(&mut chunk)?;
            if got == 0 {
                bail!("{}", t!("Der Server hat die Verbindung beendet"));
            }
            rest.extend_from_slice(&chunk[..got]);
        }
        rest.truncate(n);
        resp.body = rest;
    } else {
        // No length: until the server closes.
        let mut chunk = [0u8; 8192];
        loop {
            let got = r.read(&mut chunk)?;
            if got == 0 {
                break;
            }
            rest.extend_from_slice(&chunk[..got]);
            if rest.len() > MAX_BODY {
                bail!("{}", t!("Die Antwort des Servers ist zu groß"));
            }
        }
        resp.body = rest;
    }
    Ok(resp)
}

fn read_chunked<R: Read>(r: &mut R, pending: &mut Vec<u8>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = std::mem::take(pending);
    let mut fill = |buf: &mut Vec<u8>| -> Result<()> {
        let mut chunk = [0u8; 8192];
        let n = r.read(&mut chunk)?;
        if n == 0 {
            bail!("{}", t!("Der Server hat die Verbindung beendet"));
        }
        buf.extend_from_slice(&chunk[..n]);
        Ok(())
    };
    loop {
        // Chunk size line.
        let line_end = loop {
            if let Some(p) = buf.windows(2).position(|w| w == b"\r\n") {
                break p;
            }
            if buf.len() > 1024 {
                bail!(
                    "{}",
                    t!("Die Antwort des Servers ist ungültig (Chunk-Kopf)")
                );
            }
            fill(&mut buf)?;
        };
        let size_text = String::from_utf8_lossy(&buf[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| {
                anyhow!(
                    "{}",
                    t!("Die Antwort des Servers ist ungültig (Chunk-Größe)")
                )
            })?;
        buf.drain(..line_end + 2);
        if size == 0 {
            // Trailer: read up to the final empty line.
            loop {
                if buf.starts_with(b"\r\n") {
                    return Ok(out);
                }
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let _ = p;
                    return Ok(out);
                }
                if buf.len() > MAX_HEAD {
                    return Ok(out);
                }
                fill(&mut buf)?;
            }
        }
        if out.len() + size > MAX_BODY {
            bail!("{}", t!("Die Antwort des Servers ist zu groß"));
        }
        while buf.len() < size + 2 {
            fill(&mut buf)?;
        }
        out.extend_from_slice(&buf[..size]);
        buf.drain(..size + 2);
    }
}

/// The NTLM token in a `WWW-Authenticate` header value (`Negotiate <base64>`), also when
/// the server wrapped it in SPNEGO.
pub fn ntlm_token(value: &str) -> Option<Vec<u8>> {
    let (scheme, token) = value.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("negotiate") && !scheme.eq_ignore_ascii_case("ntlm") {
        return None;
    }
    let raw = B64.decode(token.trim()).ok()?;
    let at = raw.windows(8).position(|w| w == b"NTLMSSP\0")?;
    Some(raw[at..].to_vec())
}

/// An open, authenticated WinRM connection.
pub struct Http {
    target: Target,
    stream: Stream,
    authed: bool,
    open: bool,
}

impl Http {
    pub fn connect(target: &Target) -> Result<Http, ConnectError> {
        if target.auth == WinrmAuth::Basic && !target.https {
            return Err(ConnectError::Other(anyhow!(
                "{}",
                t!("Basic-Anmeldung wird nur über HTTPS angeboten")
            )));
        }
        Ok(Http {
            stream: connect_stream(target)?,
            target: target.clone(),
            authed: false,
            open: true,
        })
    }

    fn send(&mut self, auth: Option<&str>, body: &[u8]) -> Result<Response> {
        let mut req = format!(
            "POST /wsman HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: Aurora\r\nContent-Type: application/soap+xml;charset=UTF-8\r\nConnection: Keep-Alive\r\nContent-Length: {}\r\n",
            self.target.host,
            self.target.port,
            body.len()
        );
        if let Some(a) = auth {
            req.push_str("Authorization: ");
            req.push_str(a);
            req.push_str("\r\n");
        }
        req.push_str("\r\n");
        self.stream.write_all(req.as_bytes())?;
        self.stream.write_all(body)?;
        self.stream.flush()?;
        let resp = read_response(&mut self.stream)?;
        if resp.closes() {
            self.open = false;
        }
        Ok(resp)
    }

    fn denied(resp: &Response) -> anyhow::Error {
        let offered: Vec<&str> = resp.header_all("www-authenticate").collect();
        anyhow!(
            "{}",
            t!(
                "Anmeldung abgelehnt (HTTP 401). Prüfe Benutzer, Passwort und Domäne; angeboten: {}",
                if offered.is_empty() {
                    "-".to_string()
                } else {
                    offered.join(", ")
                }
            )
        )
    }

    /// NTLM handshake on this connection; the SOAP request goes with the last message.
    fn ntlm_post(&mut self, body: &[u8]) -> Result<Response> {
        let t = self.target.clone();
        let neg = ntlm::negotiate_message();
        let r1 = self.send(Some(&format!("Negotiate {}", B64.encode(&neg))), &[])?;
        if r1.status != 401 {
            // Some servers do not insist on authentication at all.
            if (200..300).contains(&r1.status) {
                self.authed = true;
                return self.send(None, body);
            }
            bail!(
                "{}",
                t!("Unerwartete Antwort des Servers (HTTP {})", r1.status)
            );
        }
        let challenge = r1
            .header_all("www-authenticate")
            .find_map(ntlm_token)
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    t!(
                        "Der Server bietet kein NTLM an (angeboten: {})",
                        r1.header_all("www-authenticate")
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                )
            })?;
        if !self.open {
            bail!(
                "{}",
                t!("Der Server beendet die Verbindung während der Anmeldung")
            );
        }
        let ch = ntlm::parse_challenge(&challenge)?;
        let mut cc = [0u8; 8];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut cc);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let auth = ntlm::authenticate_message(
            &neg,
            &ch,
            &ntlm::AuthInput {
                user: &t.user,
                domain: &t.domain,
                password: &t.password,
                workstation: "",
                client_challenge: cc,
                now_filetime: ntlm::filetime(now),
            },
        );
        let r2 = self.send(Some(&format!("Negotiate {}", B64.encode(auth))), body)?;
        if r2.status == 401 {
            return Err(Self::denied(&r2));
        }
        self.authed = true;
        Ok(r2)
    }

    /// Sends a SOAP request and returns the answer body (status 200 or a SOAP fault 500).
    pub fn soap(&mut self, body: &str) -> Result<String> {
        let resp = match self.target.auth {
            WinrmAuth::Basic => {
                let cred = B64.encode(format!("{}:{}", self.target.user, self.target.password));
                self.send(Some(&format!("Basic {cred}")), body.as_bytes())?
            }
            WinrmAuth::Ntlm if self.authed => self.send(None, body.as_bytes())?,
            WinrmAuth::Ntlm => self.ntlm_post(body.as_bytes())?,
        };
        match resp.status {
            200 | 500 => Ok(String::from_utf8_lossy(&resp.body).into_owned()),
            401 => Err(Self::denied(&resp)),
            404 => bail!("{}", t!("Auf diesem Port läuft kein WinRM (HTTP 404)")),
            s => bail!(
                "{}",
                t!(
                    "Unerwartete Antwort des Servers (HTTP {}): {}",
                    s,
                    String::from_utf8_lossy(&resp.body)
                        .chars()
                        .take(200)
                        .collect::<String>()
                )
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn content_length_and_headers() {
        let raw = b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Negotiate abc\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhelloEXTRA";
        let r = read_response(&mut Cursor::new(&raw[..])).unwrap();
        assert_eq!(r.status, 401);
        assert_eq!(r.body, b"hello");
        assert_eq!(r.header_all("www-authenticate").count(), 2);
        assert!(r.closes());
    }

    #[test]
    fn chunked_bodies() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nX-Trailer: 1\r\n\r\n";
        let r = read_response(&mut Cursor::new(&raw[..])).unwrap();
        assert_eq!(r.body, b"hello world");
        // A chunk split over many reads.
        struct Slow<'a>(&'a [u8]);
        impl Read for Slow<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(3).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let r = read_response(&mut Slow(raw)).unwrap();
        assert_eq!(r.body, b"hello world");
    }

    #[test]
    fn bad_responses() {
        assert!(read_response(&mut Cursor::new(&b"SSH-2.0-x\r\n\r\n"[..])).is_err());
        assert!(
            read_response(&mut Cursor::new(
                &b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"[..]
            ))
            .is_err()
        );
        let huge = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(read_response(&mut Cursor::new(huge.as_bytes())).is_err());
        let endless = vec![b'a'; MAX_HEAD + 10];
        assert!(read_response(&mut Cursor::new(endless)).is_err());
        let bad_chunk = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nabc";
        assert!(read_response(&mut Cursor::new(&bad_chunk[..])).is_err());
    }

    #[test]
    fn ntlm_tokens() {
        let msg = ntlm::negotiate_message();
        let plain = format!("Negotiate {}", B64.encode(&msg));
        assert_eq!(ntlm_token(&plain).unwrap(), msg);
        // Wrapped in some SPNEGO-like prefix.
        let mut wrapped = vec![0xa1, 0x81, 0x00, 0x30, 0x10];
        wrapped.extend_from_slice(&msg);
        let w = format!("Negotiate {}", B64.encode(&wrapped));
        assert_eq!(ntlm_token(&w).unwrap(), msg);
        assert!(ntlm_token("Basic realm=\"x\"").is_none());
        assert!(ntlm_token("Negotiate").is_none());
        assert!(ntlm_token("Negotiate !!!").is_none());
    }

    #[test]
    fn urls() {
        let mut t = Target {
            host: "srv".into(),
            port: 5985,
            https: false,
            auth: WinrmAuth::Ntlm,
            user: String::new(),
            domain: String::new(),
            password: String::new(),
            pin: None,
        };
        assert_eq!(t.url(), "http://srv:5985/wsman");
        t.host = "fe80::1".into();
        t.https = true;
        t.port = 5986;
        assert_eq!(t.url(), "https://[fe80::1]:5986/wsman");
        t.auth = WinrmAuth::Basic;
        t.https = false;
        assert!(Http::connect(&t).is_err());
    }
}
