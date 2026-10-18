//! Proxmox VE REST API: cluster resources, node status, power actions and guest IPs.
//!
//! Requests are blocking (`ureq`) and run on tokio's blocking pool; results go back to
//! the UI through a [`Sink`]. Proxmox uses self-signed certificates, so TLS is checked
//! against a pinned SHA-256 fingerprint of the server certificate (trust on first use)
//! instead of a CA store: an unknown certificate is reported with its fingerprint for the
//! user to confirm, a changed one is refused.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, TcpConnector,
    Transport, TransportAdapter,
};
use uuid::Uuid;

use super::Sink;

#[path = "proxmox_ops.rs"]
mod ops;
pub use ops::*;

#[path = "proxmox_guest_ops.rs"]
mod guest_ops;
pub use guest_ops::*;
/// Cluster-wide views (tasks, storage, alerts), node shells and Proxmox Backup Server.
#[path = "proxmox_cluster.rs"]
pub mod cluster;

// ------------------------------------------------------------------ data

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Qemu,
    Lxc,
}

impl Kind {
    /// Path segment in the API (`qemu` / `lxc`).
    pub fn api(self) -> &'static str {
        match self {
            Kind::Qemu => "qemu",
            Kind::Lxc => "lxc",
        }
    }
}

/// A VM or container from `/cluster/resources`.
#[derive(Clone, Debug, PartialEq)]
pub struct Guest {
    pub vmid: u32,
    pub name: String,
    pub node: String,
    pub kind: Kind,
    /// `running`, `stopped`, `paused`, …
    pub status: String,
    /// CPU load as a fraction of `maxcpu` (0.0–1.0).
    pub cpu: f32,
    pub maxcpu: u32,
    pub mem: u64,
    pub maxmem: u64,
    pub disk: u64,
    pub maxdisk: u64,
    pub uptime: u64,
    pub tags: Vec<String>,
    pub template: bool,
}

impl Guest {
    pub fn running(&self) -> bool {
        self.status == "running"
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub online: bool,
    pub cpu: f32,
    pub maxcpu: u32,
    pub mem: u64,
    pub maxmem: u64,
    pub uptime: u64,
}

/// Addresses of a guest.
#[derive(Clone, Debug, PartialEq)]
pub enum Ips {
    Found(Vec<String>),
    /// QEMU guest without a running guest agent.
    NoAgent,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub guests: Vec<Guest>,
    pub nodes: Vec<Node>,
    /// Looked up in this round (vmid → addresses).
    pub ips: HashMap<u32, Ips>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Start,
    Shutdown,
    Stop,
    Reboot,
    /// Continue a paused (suspended) VM.
    Resume,
}

impl Power {
    pub fn api(self) -> &'static str {
        match self {
            Power::Start => "start",
            Power::Shutdown => "shutdown",
            Power::Stop => "stop",
            Power::Reboot => "reboot",
            Power::Resume => "resume",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Auth {
    /// `user@realm!tokenid` and the token secret.
    Token { id: String, secret: String },
    /// `user@realm` and the password (ticket login).
    Password { user: String, password: String },
}

/// Ticket session from a password login.
#[derive(Clone, Debug)]
pub struct Session {
    pub ticket: String,
    pub csrf: String,
    pub at: std::time::Instant,
}

/// Everything a request needs; cheap to clone into a worker.
#[derive(Clone, Debug)]
pub struct Conn {
    pub host: String,
    pub port: u16,
    pub auth: Auth,
    /// Pinned certificate fingerprint (`AB:CD:…`), `None` = not trusted yet.
    pub pin: Option<String>,
    pub session: Option<Session>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Certificate not pinned yet: its fingerprint, for the user to confirm.
    Untrusted(String),
    /// The certificate differs from the pinned one.
    PinChanged {
        pinned: String,
        got: String,
    },
    Auth(String),
    Http(u16, String),
    Net(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Untrusted(_) => write!(f, "{}", t!("Zertifikat noch nicht bestätigt")),
            Error::PinChanged { .. } => write!(
                f,
                "{}",
                t!(
                    "Das Zertifikat des Servers hat sich geändert – Verbindung verweigert. Prüfe den Server und setze den Pin im Server-Editor zurück, wenn die Änderung gewollt war."
                )
            ),
            Error::Auth(m) => write!(f, "{}", t!("Anmeldung fehlgeschlagen: {}", m)),
            Error::Http(code, m) => write!(f, "HTTP {code}: {m}"),
            Error::Net(m) => write!(f, "{m}"),
        }
    }
}

pub enum PveEvent {
    Fetched {
        server: Uuid,
        epoch: Epoch,
        result: Result<Snapshot, Error>,
        session: Option<Session>,
    },
    PowerDone {
        server: Uuid,
        epoch: Epoch,
        vmid: u32,
        power: Power,
        result: Result<(), Error>,
        session: Option<Session>,
    },
    /// "Verbindung testen" in the editor: version string or the error.
    Tested(Result<String, Error>),
    /// Result or progress of a job about one guest (snapshots, backups, tasks, …).
    Guest {
        server: Uuid,
        epoch: Epoch,
        vmid: u32,
        session: Option<Session>,
        payload: GuestPayload,
    },
}

// ------------------------------------------------------------------ TLS pinning

/// `AB:CD:…` SHA-256 fingerprint of a DER certificate (as shown by Proxmox).
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Upper-case hex without separators, for comparing pins typed in different styles.
fn normalize_pin(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

#[derive(Debug, PartialEq)]
pub enum PinCheck {
    Match,
    Unpinned,
    Mismatch,
}

pub fn check_pin(pin: Option<&str>, fingerprint: &str) -> PinCheck {
    match pin.map(normalize_pin).filter(|p| !p.is_empty()) {
        None => PinCheck::Unpinned,
        Some(p) if p == normalize_pin(fingerprint) => PinCheck::Match,
        Some(_) => PinCheck::Mismatch,
    }
}

/// Accepts exactly the pinned certificate; remembers what the server presented so the
/// caller can show it. Handshake signatures are still verified normally.
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
        _ocsp_response: &[u8],
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

/// ureq connector that wraps the TCP stream in rustls with the [`PinVerifier`].
#[derive(Debug)]
struct PinnedTls {
    config: Arc<ClientConfig>,
}

struct TlsTransport {
    buffers: LazyBuffers,
    stream: StreamOwned<ClientConnection, TransportAdapter>,
}

impl std::fmt::Debug for TlsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinnedTlsTransport")
    }
}

impl<In: Transport> Connector<In> for PinnedTls {
    type Out = Either<In, TlsTransport>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        let Some(transport) = chained else {
            return Ok(None);
        };
        if !details.needs_tls() || transport.is_tls() {
            return Ok(Some(Either::A(transport)));
        }
        let host = details
            .uri
            .host()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let name = ServerName::try_from(host).map_err(|_| ureq::Error::Tls("invalid host name"))?;
        let mut conn = ClientConnection::new(self.config.clone(), name)?;
        let mut sock = TransportAdapter::new(transport.boxed());
        sock.set_timeout(details.timeout);
        conn.complete_io(&mut sock)?;
        Ok(Some(Either::B(TlsTransport {
            buffers: LazyBuffers::new(
                details.config.input_buffer_size(),
                details.config.output_buffer_size(),
            ),
            stream: StreamOwned { conn, sock },
        })))
    }
}

impl Transport for TlsTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let output = &self.buffers.output()[..amount];
        self.stream.write_all(output)?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let input = self.buffers.input_append_buf();
        let n = self.stream.read(input)?;
        self.buffers.input_appended(n);
        Ok(n > 0)
    }

    fn is_open(&mut self) -> bool {
        self.stream.get_mut().get_mut().is_open()
    }

    fn is_tls(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------------ client

struct Client {
    agent: ureq::Agent,
    base: String,
    seen: Arc<Mutex<Option<String>>>,
    pin: Option<String>,
    auth: Auth,
    session: Option<Session>,
    /// Proxmox Backup Server: other header / cookie names (see `cluster::pbs`).
    pbs: bool,
}

/// Tickets are valid for two hours; renew a bit earlier.
const TICKET_MAX_AGE: Duration = Duration::from_secs(90 * 60);

impl Client {
    fn new(conn: &Conn) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let seen = Arc::new(Mutex::new(None));
        let verifier = PinVerifier {
            pin: conn.pin.clone(),
            seen: seen.clone(),
            algs: provider.signature_verification_algorithms,
        };
        let tls = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            // Proxmox lives in the LAN; a system proxy would only get in the way.
            .proxy(None)
            .user_agent("Aurora")
            .build();
        let connector = ().chain(TcpConnector::default()).chain(PinnedTls {
            config: Arc::new(tls),
        });
        let host = if conn.host.contains(':') && !conn.host.starts_with('[') {
            format!("[{}]", conn.host)
        } else {
            conn.host.clone()
        };
        Self {
            agent: ureq::Agent::with_parts(config, connector, DefaultResolver::default()),
            base: format!("https://{host}:{}/api2/json", conn.port),
            seen,
            pin: conn.pin.clone(),
            auth: conn.auth.clone(),
            session: conn
                .session
                .clone()
                .filter(|s| s.at.elapsed() < TICKET_MAX_AGE),
            pbs: false,
        }
    }

    /// Token header, or ticket cookie (+ CSRF token for writes).
    fn auth_headers(&self, method: Method) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();
        match (&self.auth, &self.session) {
            (Auth::Token { id, secret }, _) if self.pbs => {
                headers.push(("Authorization", format!("PBSAPIToken={id}:{secret}")));
            }
            (Auth::Token { id, secret }, _) => {
                headers.push(("Authorization", format!("PVEAPIToken={id}={secret}")));
            }
            (Auth::Password { .. }, Some(s)) => {
                let cookie = if self.pbs {
                    "PBSAuthCookie"
                } else {
                    "PVEAuthCookie"
                };
                headers.push(("Cookie", format!("{cookie}={}", s.ticket)));
                if method != Method::Get {
                    headers.push(("CSRFPreventionToken", s.csrf.clone()));
                }
            }
            (Auth::Password { .. }, None) => {}
        }
        headers
    }

    /// Turns a transport error into a pin error when the certificate was the reason.
    fn net_error(&self, e: ureq::Error) -> Error {
        let seen = self.seen.lock().ok().and_then(|s| s.clone());
        if let Some(fp) = seen {
            match check_pin(self.pin.as_deref(), &fp) {
                PinCheck::Unpinned => return Error::Untrusted(fp),
                PinCheck::Mismatch => {
                    return Error::PinChanged {
                        pinned: self.pin.clone().unwrap_or_default(),
                        got: fp,
                    };
                }
                PinCheck::Match => {}
            }
        }
        Error::Net(match e {
            ureq::Error::Io(io) => io.to_string(),
            other => other.to_string(),
        })
    }

    fn login(&mut self) -> Result<(), Error> {
        let Auth::Password { user, password } = &self.auth else {
            return Ok(());
        };
        let user = if user.contains('@') {
            user.clone()
        } else {
            format!("{user}@pam")
        };
        let mut resp = self
            .agent
            .post(format!("{}/access/ticket", self.base))
            .send_form([("username", user.as_str()), ("password", password.as_str())])
            .map_err(|e| self.net_error(e))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        if status == 401 || status == 403 {
            return Err(Error::Auth(t!("Benutzer oder Passwort falsch").into()));
        }
        if status != 200 {
            return Err(Error::Http(status, short(&body)));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| Error::Net(e.to_string()))?;
        let d = &v["data"];
        if d.get("NeedTFA")
            .is_some_and(|x| x.as_u64() == Some(1) || x.as_bool() == Some(true))
        {
            return Err(Error::Auth(
                t!("Zwei-Faktor-Anmeldung wird nicht unterstützt – nutze ein API-Token").into(),
            ));
        }
        let (Some(ticket), Some(csrf)) = (d["ticket"].as_str(), d["CSRFPreventionToken"].as_str())
        else {
            return Err(Error::Auth(t!("Benutzer oder Passwort falsch").into()));
        };
        self.session = Some(Session {
            ticket: ticket.to_string(),
            csrf: csrf.to_string(),
            at: std::time::Instant::now(),
        });
        Ok(())
    }

    /// GET or POST `path` (below `/api2/json`) and return the `data` member.
    fn call(&mut self, post: bool, path: &str) -> Result<Value, Error> {
        let method = if post { Method::Post } else { Method::Get };
        self.request(method, path, &[])
    }

    /// Any request with optional form parameters (POST only); returns the `data` member.
    fn request(
        &mut self,
        method: Method,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<Value, Error> {
        if matches!(self.auth, Auth::Password { .. }) && self.session.is_none() {
            self.login()?;
        }
        for attempt in 0..2 {
            let (status, body) = self.send(method, path, form)?;
            if status == 401 && attempt == 0 && matches!(self.auth, Auth::Password { .. }) {
                // Ticket expired: log in again once.
                self.session = None;
                self.login()?;
                continue;
            }
            return match status {
                200..=299 => {
                    let v: Value =
                        serde_json::from_str(&body).map_err(|e| Error::Net(e.to_string()))?;
                    Ok(v.get("data").cloned().unwrap_or(Value::Null))
                }
                401 => Err(Error::Auth(match self.auth {
                    Auth::Token { .. } => t!("API-Token ungültig").to_string(),
                    Auth::Password { .. } => t!("Benutzer oder Passwort falsch").to_string(),
                })),
                403 => Err(Error::Http(403, t!("keine Berechtigung").into())),
                s => Err(Error::Http(s, short(&body))),
            };
        }
        Err(Error::Auth(t!("Benutzer oder Passwort falsch").into()))
    }

    fn send(
        &self,
        method: Method,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<(u16, String), Error> {
        let url = format!("{}{path}", self.base);
        let headers = self.auth_headers(method);
        let result = match method {
            Method::Post | Method::Put => {
                let mut req = if method == Method::Post {
                    self.agent.post(&url)
                } else {
                    self.agent.put(&url)
                };
                for (k, v) in &headers {
                    req = req.header(*k, v);
                }
                if form.is_empty() {
                    req.send_empty()
                } else {
                    req.send_form(form.iter().map(|(k, v)| (*k, v.as_str())))
                }
            }
            Method::Get | Method::Delete => {
                let mut req = if method == Method::Get {
                    self.agent.get(&url)
                } else {
                    self.agent.delete(&url)
                };
                for (k, v) in &headers {
                    req = req.header(*k, v);
                }
                req.call()
            }
        };
        let mut resp = result.map_err(|e| self.net_error(e))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        Ok((status, body))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Get,
    Post,
    Put,
    Delete,
}

/// First line of an error body, shortened.
fn short(body: &str) -> String {
    let text = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
                .or_else(|| v.get("errors").map(|e| e.to_string()))
        })
        .unwrap_or_else(|| body.to_string());
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

/// Node names go into URL paths and shell commands: only DNS-style characters.
pub fn valid_node(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

// ------------------------------------------------------------------ parsing

fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse().unwrap_or(0.0),
        Value::Bool(b) => *b as u8 as f64,
        _ => 0.0,
    }
}

/// `data` of `/cluster/resources?type=vm`.
pub fn parse_resources(data: &Value) -> Vec<Guest> {
    let mut out: Vec<Guest> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let kind = match r["type"].as_str()? {
                "qemu" => Kind::Qemu,
                "lxc" => Kind::Lxc,
                _ => return None,
            };
            let vmid = num(&r["vmid"]) as u32;
            if vmid == 0 {
                return None;
            }
            let tags = r["tags"]
                .as_str()
                .unwrap_or_default()
                .split([';', ',', ' '])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            Some(Guest {
                vmid,
                name: r["name"].as_str().unwrap_or_default().to_string(),
                node: r["node"].as_str().unwrap_or_default().to_string(),
                kind,
                status: r["status"].as_str().unwrap_or("unknown").to_string(),
                cpu: num(&r["cpu"]) as f32,
                maxcpu: num(&r["maxcpu"]) as u32,
                mem: num(&r["mem"]) as u64,
                maxmem: num(&r["maxmem"]) as u64,
                disk: num(&r["disk"]) as u64,
                maxdisk: num(&r["maxdisk"]) as u64,
                uptime: num(&r["uptime"]) as u64,
                tags,
                template: num(&r["template"]) != 0.0,
            })
        })
        .collect();
    out.sort_by(|a, b| a.node.cmp(&b.node).then(a.vmid.cmp(&b.vmid)));
    out
}

/// `data` of `/nodes`.
pub fn parse_nodes(data: &Value) -> Vec<Node> {
    let mut out: Vec<Node> = data
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|n| {
            Some(Node {
                name: n["node"].as_str()?.to_string(),
                online: n["status"].as_str() == Some("online"),
                cpu: num(&n["cpu"]) as f32,
                maxcpu: num(&n["maxcpu"]) as u32,
                mem: num(&n["mem"]) as u64,
                maxmem: num(&n["maxmem"]) as u64,
                uptime: num(&n["uptime"]) as u64,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn ranked(cands: &[IpCandidate], near: Option<std::net::IpAddr>) -> Vec<String> {
    rank_ips(cands, near)
        .iter()
        .map(|ip| ip.to_string())
        .collect()
}

/// `data` of `qemu/{vmid}/agent/network-get-interfaces` (`{"result": [...]}`): usable
/// addresses, best first.
#[cfg(test)]
pub fn parse_agent_ips(data: &Value) -> Vec<String> {
    ranked(&agent_candidates(data), None)
}

/// `data` of `lxc/{vmid}/interfaces` (`inet`/`inet6` with prefix, or `ip-addresses`).
#[cfg(test)]
pub fn parse_lxc_ips(data: &Value) -> Vec<String> {
    ranked(&lxc_candidates(data), None)
}

// ------------------------------------------------------------------ console

/// What a console opens (SSH, API terminal, node terminal or browser).
#[path = "proxmox_console.rs"]
mod console;
pub use console::*;

// ------------------------------------------------------------------ jobs

/// Fetches guests and nodes, plus the addresses of the listed running guests.
pub fn spawn_fetch(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    ips_for: Vec<(String, Kind, u32)>,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let near = host_ip(&conn.host, conn.port);
        let mut c = Client::new(&conn);
        let result = (|| {
            let guests = parse_resources(&c.call(false, "/cluster/resources?type=vm")?);
            let nodes = parse_nodes(&c.call(false, "/nodes")?);
            let mut ips = HashMap::new();
            for (node, kind, vmid) in ips_for {
                let running = guests.iter().any(|g| g.vmid == vmid && g.running());
                if !running || !valid_node(&node) {
                    continue;
                }
                let path = match kind {
                    Kind::Qemu => format!("/nodes/{node}/qemu/{vmid}/agent/network-get-interfaces"),
                    Kind::Lxc => format!("/nodes/{node}/lxc/{vmid}/interfaces"),
                };
                match (kind, c.call(false, &path)) {
                    (Kind::Qemu, Ok(d)) => {
                        ips.insert(vmid, Ips::Found(ranked(&agent_candidates(&d), near)));
                    }
                    (Kind::Lxc, Ok(d)) => {
                        ips.insert(vmid, Ips::Found(ranked(&lxc_candidates(&d), near)));
                    }
                    // No (running) guest agent: Proxmox answers with a 500.
                    (Kind::Qemu, Err(Error::Http(..))) => {
                        ips.insert(vmid, Ips::NoAgent);
                    }
                    (_, Err(e @ (Error::Untrusted(_) | Error::PinChanged { .. }))) => {
                        return Err(e);
                    }
                    _ => {}
                }
            }
            Ok(Snapshot { guests, nodes, ips })
        })();
        sink.send(PveEvent::Fetched {
            server,
            epoch,
            result,
            session: c.session.clone(),
        });
    });
}

pub fn spawn_power(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: (String, Kind, u32),
    power: Power,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let (node, kind, vmid) = guest;
        let result = if valid_node(&node) {
            c.call(
                true,
                &format!("/nodes/{node}/{}/{vmid}/status/{}", kind.api(), power.api()),
            )
            .map(|_| ())
        } else {
            Err(Error::Net(t!("ungültiger Node-Name").into()))
        };
        sink.send(PveEvent::PowerDone {
            server,
            epoch,
            vmid,
            power,
            result,
            session: c.session.clone(),
        });
    });
}

/// Logs in and reads `/version`.
pub fn spawn_test(rt: &tokio::runtime::Runtime, conn: Conn, sink: Sink<PveEvent>) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = c
            .call(false, "/version")
            .map(|d| format!("Proxmox VE {}", d["version"].as_str().unwrap_or("?").trim()));
        sink.send(PveEvent::Tested(result));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cluster_resources() {
        let data = json!([
            {"id":"qemu/100","type":"qemu","vmid":100,"name":"web","node":"pve1","status":"running",
             "cpu":0.125,"maxcpu":4,"mem":1073741824u64,"maxmem":4294967296u64,"disk":0,"maxdisk":34359738368u64,
             "uptime":3600,"tags":"prod;web","template":0},
            {"id":"lxc/101","type":"lxc","vmid":101,"name":"dns","node":"pve1","status":"stopped",
             "cpu":0,"maxcpu":1,"mem":0,"maxmem":536870912,"uptime":0},
            {"id":"qemu/9000","type":"qemu","vmid":9000,"name":"debian-tpl","node":"pve2","status":"stopped","template":1},
            {"id":"storage/pve1/local","type":"storage","node":"pve1"}
        ]);
        let g = parse_resources(&data);
        assert_eq!(g.len(), 3);
        assert_eq!(
            (g[0].vmid, g[0].kind, g[0].name.as_str()),
            (100, Kind::Qemu, "web")
        );
        assert!(g[0].running());
        assert_eq!(g[0].tags, vec!["prod", "web"]);
        assert_eq!(g[0].maxmem, 4294967296);
        assert!((g[0].cpu - 0.125).abs() < 1e-6);
        assert_eq!((g[1].kind, g[1].status.as_str()), (Kind::Lxc, "stopped"));
        assert!(g[1].tags.is_empty() && !g[1].template);
        assert!(g[2].template);
        assert_eq!(g[2].node, "pve2");

        let n = parse_nodes(&json!([
            {"node":"pve2","status":"offline"},
            {"node":"pve1","status":"online","cpu":0.05,"maxcpu":8,"mem":2,"maxmem":4,"uptime":99}
        ]));
        assert_eq!(n[0].name, "pve1");
        assert!(n[0].online && !n[1].online);
    }

    #[test]
    fn pin_logic() {
        let fp = fingerprint(b"certificate bytes");
        assert_eq!(fp.len(), 32 * 3 - 1);
        assert_eq!(check_pin(None, &fp), PinCheck::Unpinned);
        assert_eq!(check_pin(Some(""), &fp), PinCheck::Unpinned);
        assert_eq!(check_pin(Some(&fp), &fp), PinCheck::Match);
        // Lower case / without colons is the same pin.
        let loose = fp.replace(':', "").to_lowercase();
        assert_eq!(check_pin(Some(&loose), &fp), PinCheck::Match);
        assert_eq!(
            check_pin(Some(&fingerprint(b"other")), &fp),
            PinCheck::Mismatch
        );
    }

    #[test]
    fn pin_verifier_accepts_only_the_pin() {
        let der = CertificateDer::from(b"fake der".to_vec());
        let name = ServerName::try_from("10.0.0.1").unwrap();
        let verify = |pin: Option<String>| {
            let seen = Arc::new(Mutex::new(None));
            let v = PinVerifier {
                pin,
                seen: seen.clone(),
                algs: rustls::crypto::ring::default_provider().signature_verification_algorithms,
            };
            let ok = v
                .verify_server_cert(&der, &[], &name, &[], UnixTime::now())
                .is_ok();
            let seen = seen.lock().unwrap().clone();
            (ok, seen)
        };
        let fp = fingerprint(b"fake der");
        assert_eq!(verify(Some(fp.clone())), (true, Some(fp.clone())));
        assert_eq!(verify(None), (false, Some(fp.clone())));
        assert_eq!(verify(Some(fingerprint(b"x"))), (false, Some(fp)));
    }

    #[test]
    fn guest_ips() {
        let agent = json!({"result":[
            {"name":"lo","ip-addresses":[{"ip-address":"127.0.0.1","ip-address-type":"ipv4"}]},
            {"name":"ens18","hardware-address":"bc:24:11:00:00:01","ip-addresses":[
                {"ip-address":"fe80::be24:11ff:fe00:1","ip-address-type":"ipv6","prefix":64},
                {"ip-address":"2001:db8::10","ip-address-type":"ipv6","prefix":64},
                {"ip-address":"192.168.1.50","ip-address-type":"ipv4","prefix":24}
            ]}
        ]});
        assert_eq!(
            parse_agent_ips(&agent),
            vec!["192.168.1.50", "2001:db8::10"]
        );
        let lxc = json!([
            {"name":"lo","inet":"127.0.0.1/8","inet6":"::1/128","hwaddr":"00:00:00:00:00:00"},
            {"name":"eth0","inet":"10.0.0.21/24","inet6":"fe80::1/64","hwaddr":"bc:24:11:00:00:02"}
        ]);
        assert_eq!(parse_lxc_ips(&lxc), vec!["10.0.0.21"]);
        assert!(parse_lxc_ips(&json!(null)).is_empty());
    }
}
