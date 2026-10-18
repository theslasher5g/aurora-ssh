//! Certificate radar: fetches the TLS certificates of the saved hosts and reads subject,
//! SANs, issuer and validity (minimal DER parser, no X.509 dependency).
//!
//! The TLS connection uses a verifier that accepts EVERY certificate and only records the
//! chain – it is for inspection only and never carries any data.

use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Sink;

/// Checked when a host has no port scan result yet.
pub const DEFAULT_PORTS: &[u16] = &[443, 8443, 8006, 9443, 636, 993, 995, 465, 587];

/// Ports that never speak TLS directly (skipped from port scan results).
const NOT_TLS: &[u16] = &[
    21, 22, 23, 53, 80, 110, 111, 139, 143, 445, 1883, 2049, 2375, 3306, 3389, 5432, 5900, 6379,
    8080, 11211, 25565, 27017,
];

/// SMTP ports that need `STARTTLS` first.
const STARTTLS_SMTP: &[u16] = &[25, 587];

/// Days left below which a certificate is shown red / amber.
pub const RED_DAYS: i64 = 7;
pub const AMBER_DAYS: i64 = 30;

/// One certificate as seen on `address:port`.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(default)]
pub struct CertInfo {
    pub host_id: Option<Uuid>,
    pub host_name: String,
    pub address: String,
    pub port: u16,
    pub subject: String,
    pub sans: Vec<String>,
    pub issuer: String,
    pub not_before: Option<DateTime<Utc>>,
    pub not_after: Option<DateTime<Utc>>,
    pub self_signed: bool,
    /// The address the host is saved under appears in SAN/CN.
    pub name_ok: bool,
    /// Certificates the server sent (leaf + intermediates).
    pub chain_len: usize,
    pub checked: Option<DateTime<Utc>>,
}

impl CertInfo {
    /// Whole days until `not_after` (negative = expired).
    pub fn days_left(&self, now: DateTime<Utc>) -> i64 {
        match self.not_after {
            Some(t) => days_between(now, t),
            None => i64::MIN,
        }
    }
}

/// Whole days from `now` to `then`, rounded down (−1 as soon as `then` has passed).
pub fn days_between(now: DateTime<Utc>, then: DateTime<Utc>) -> i64 {
    (then - now).num_seconds().div_euclid(86_400)
}

/// Severity for the list colours: 2 red (expired or < 7 d), 1 amber (< 30 d), 0 ok.
pub fn severity(days: i64) -> u8 {
    if days < RED_DAYS {
        2
    } else if days < AMBER_DAYS {
        1
    } else {
        0
    }
}

/// Ports to check for a host: TLS candidates from the last port scan, else the defaults.
pub fn ports_for(scanned: Option<&Vec<u16>>) -> Vec<u16> {
    match scanned.filter(|p| !p.is_empty()) {
        Some(ports) => ports
            .iter()
            .copied()
            .filter(|p| !NOT_TLS.contains(p))
            .collect(),
        None => DEFAULT_PORTS.to_vec(),
    }
}

// ---------------------------------------------------------------- DER

/// Minimal DER reader: tag, content.
struct Der<'a> {
    data: &'a [u8],
}

impl<'a> Der<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn peek_tag(&self) -> Option<u8> {
        self.data.first().copied()
    }

    /// Next element: (tag, content, whole element).
    fn next(&mut self) -> Result<(u8, &'a [u8], &'a [u8])> {
        let d = self.data;
        if d.len() < 2 {
            bail!("DER: truncated");
        }
        let tag = d[0];
        if tag & 0x1f == 0x1f {
            bail!("DER: long tags unsupported");
        }
        let (len, hdr) = match d[1] {
            l if l < 0x80 => (l as usize, 2),
            0x80 => bail!("DER: indefinite length"),
            l => {
                let n = (l & 0x7f) as usize;
                if n > 4 || d.len() < 2 + n {
                    bail!("DER: bad length");
                }
                let mut len = 0usize;
                for b in &d[2..2 + n] {
                    len = (len << 8) | *b as usize;
                }
                (len, 2 + n)
            }
        };
        let end = hdr
            .checked_add(len)
            .ok_or_else(|| anyhow!("DER: overflow"))?;
        if d.len() < end {
            bail!("DER: truncated content");
        }
        self.data = &d[end..];
        Ok((tag, &d[hdr..end], &d[..end]))
    }

    fn expect(&mut self, tag: u8) -> Result<&'a [u8]> {
        let (t, content, _) = self.next()?;
        if t != tag {
            bail!("DER: expected tag {tag:#x}, got {t:#x}");
        }
        Ok(content)
    }
}

const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
const OID_O: &[u8] = &[0x55, 0x04, 0x0a];
const OID_SAN: &[u8] = &[0x55, 0x1d, 0x11];

/// The parts of an X.509 certificate the radar shows.
#[derive(Debug, PartialEq, Default)]
pub struct Parsed {
    pub subject: String,
    pub issuer: String,
    pub not_before: Option<DateTime<Utc>>,
    pub not_after: Option<DateTime<Utc>>,
    /// DNS names and IP addresses.
    pub sans: Vec<String>,
    pub self_signed: bool,
}

fn der_string(tag: u8, v: &[u8]) -> String {
    match tag {
        // BMPString: UTF-16BE.
        0x1e => {
            let units: Vec<u16> = v
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(v).into_owned(),
    }
}

/// CN of a Name (falls back to O).
fn name_text(name: &[u8]) -> Result<String> {
    let (mut cn, mut org) = (None, None);
    let mut rdns = Der::new(name);
    while !rdns.is_empty() {
        let set = rdns.expect(0x31)?;
        let mut atvs = Der::new(set);
        while !atvs.is_empty() {
            let atv = atvs.expect(0x30)?;
            let mut a = Der::new(atv);
            let oid = a.expect(0x06)?;
            let (tag, value, _) = a.next()?;
            if oid == OID_CN && cn.is_none() {
                cn = Some(der_string(tag, value));
            } else if oid == OID_O && org.is_none() {
                org = Some(der_string(tag, value));
            }
        }
    }
    Ok(cn.or(org).unwrap_or_default())
}

/// UTCTime (`YYMMDDHHMMSSZ`) or GeneralizedTime (`YYYYMMDDHHMMSSZ`).
pub fn parse_time(tag: u8, v: &[u8]) -> Result<DateTime<Utc>> {
    let s = std::str::from_utf8(v)?;
    let s = s.strip_suffix('Z').ok_or_else(|| anyhow!("time: no Z"))?;
    let (year, rest) = match tag {
        0x17 => {
            let yy: i32 = s.get(..2).ok_or_else(|| anyhow!("time"))?.parse()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &s[2..])
        }
        0x18 => (s.get(..4).ok_or_else(|| anyhow!("time"))?.parse()?, &s[4..]),
        _ => bail!("time: tag {tag:#x}"),
    };
    let num = |i: usize| -> Result<u32> {
        Ok(rest
            .get(i..i + 2)
            .ok_or_else(|| anyhow!("time: short"))?
            .parse()?)
    };
    let (mo, d, h, mi) = (num(0)?, num(2)?, num(4)?, num(6)?);
    let sec = if rest.len() >= 10 { num(8)? } else { 0 };
    let dt = NaiveDate::from_ymd_opt(year, mo, d)
        .and_then(|date| date.and_hms_opt(h, mi, sec))
        .ok_or_else(|| anyhow!("time: invalid"))?;
    Ok(dt.and_utc())
}

fn parse_sans(ext_value: &[u8]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut outer = Der::new(ext_value);
    let names = outer.expect(0x30)?;
    let mut d = Der::new(names);
    while !d.is_empty() {
        let (tag, v, _) = d.next()?;
        match tag {
            0x82 => out.push(String::from_utf8_lossy(v).into_owned()),
            0x87 if v.len() == 4 => {
                out.push(IpAddr::from([v[0], v[1], v[2], v[3]]).to_string());
            }
            0x87 if v.len() == 16 => {
                let mut a = [0u8; 16];
                a.copy_from_slice(v);
                out.push(IpAddr::from(a).to_string());
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Parses a DER certificate.
pub fn parse_cert(der: &[u8]) -> Result<Parsed> {
    let mut top = Der::new(der);
    let cert = top.expect(0x30)?;
    let mut c = Der::new(cert);
    let tbs = c.expect(0x30)?;
    let mut t = Der::new(tbs);
    if t.peek_tag() == Some(0xa0) {
        t.next()?; // version
    }
    t.expect(0x02)?; // serial
    t.expect(0x30)?; // signature algorithm
    let (_, issuer, issuer_raw) = t.next()?;
    let validity = t.expect(0x30)?;
    let (_, subject, subject_raw) = t.next()?;
    t.expect(0x30)?; // public key
    let mut p = Parsed {
        subject: name_text(subject)?,
        issuer: name_text(issuer)?,
        self_signed: issuer_raw == subject_raw,
        ..Default::default()
    };
    let mut v = Der::new(validity);
    let (tag, nb, _) = v.next()?;
    p.not_before = Some(parse_time(tag, nb)?);
    let (tag, na, _) = v.next()?;
    p.not_after = Some(parse_time(tag, na)?);
    while !t.is_empty() {
        let (tag, content, _) = t.next()?;
        if tag != 0xa3 {
            continue;
        }
        let mut wrap = Der::new(content);
        let exts = wrap.expect(0x30)?;
        let mut e = Der::new(exts);
        while !e.is_empty() {
            let ext = e.expect(0x30)?;
            let mut x = Der::new(ext);
            let oid = x.expect(0x06)?;
            if x.peek_tag() == Some(0x01) {
                x.next()?; // critical
            }
            let value = x.expect(0x04)?;
            if oid == OID_SAN {
                p.sans = parse_sans(value)?;
            }
        }
    }
    Ok(p)
}

/// Does `host` match a certificate name (`*.example.com` covers one label)?
pub fn name_matches(pattern: &str, host: &str) -> bool {
    let (pattern, host) = (
        pattern.trim().trim_end_matches('.').to_lowercase(),
        host.trim().trim_end_matches('.').to_lowercase(),
    );
    if let (Ok(a), Ok(b)) = (pattern.parse::<IpAddr>(), host.parse::<IpAddr>()) {
        return a == b;
    }
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .split_once('.')
            .is_some_and(|(first, rest)| !first.is_empty() && rest == suffix),
        None => pattern == host,
    }
}

/// The host name is covered by the SANs (or the CN when there are no SANs).
pub fn covers(p: &Parsed, host: &str) -> bool {
    if p.sans.is_empty() {
        name_matches(&p.subject, host)
    } else {
        p.sans.iter().any(|s| name_matches(s, host))
    }
}

// ---------------------------------------------------------------- TLS

/// Accepts any certificate and keeps the chain. Inspection only!
#[derive(Debug)]
struct Recorder {
    chain: Mutex<Vec<Vec<u8>>>,
    schemes: Vec<rustls::SignatureScheme>,
}

impl rustls::client::danger::ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if let Ok(mut c) = self.chain.lock() {
            *c = std::iter::once(end_entity)
                .chain(intermediates)
                .map(|d| d.as_ref().to_vec())
                .collect();
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.clone()
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// SMTP: greeting, EHLO, STARTTLS.
fn smtp_starttls(sock: &mut TcpStream) -> Result<()> {
    let mut reader = BufReader::new(sock.try_clone()?);
    let read_reply = |reader: &mut BufReader<TcpStream>| -> Result<String> {
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                bail!("SMTP: closed");
            }
            // "250-…" continues, "250 …" ends the reply.
            if line.len() < 4 || line.as_bytes()[3] != b'-' {
                return Ok(line);
            }
        }
    };
    if !read_reply(&mut reader)?.starts_with("220") {
        bail!("SMTP: no greeting");
    }
    sock.write_all(b"EHLO aurora\r\n")?;
    if !read_reply(&mut reader)?.starts_with("250") {
        bail!("SMTP: EHLO refused");
    }
    sock.write_all(b"STARTTLS\r\n")?;
    if !read_reply(&mut reader)?.starts_with("220") {
        bail!("SMTP: no STARTTLS");
    }
    Ok(())
}

/// TLS handshake with `address:port`; returns the DER chain the server sent.
pub fn fetch_chain(address: &str, port: u16) -> Result<Vec<Vec<u8>>> {
    let addr = (address, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("no address"))?;
    let mut sock = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    sock.set_read_timeout(Some(IO_TIMEOUT))?;
    sock.set_write_timeout(Some(IO_TIMEOUT))?;
    if STARTTLS_SMTP.contains(&port) {
        smtp_starttls(&mut sock)?;
    }
    let provider = rustls::crypto::ring::default_provider();
    let recorder = Arc::new(Recorder {
        chain: Mutex::new(Vec::new()),
        schemes: provider
            .signature_verification_algorithms
            .supported_schemes(),
    });
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    let name =
        rustls::pki_types::ServerName::try_from(address.trim_matches(['[', ']']).to_string())
            .map_err(|e| anyhow!("{e}"))?;
    let mut conn = rustls::ClientConnection::new(Arc::new(config), name)?;
    let mut result = Ok(());
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut sock) {
            result = Err(e);
            break;
        }
    }
    let chain = recorder.chain.lock().map(|c| c.clone()).unwrap_or_default();
    if chain.is_empty() {
        result?;
        bail!("no certificate");
    }
    // Polite close; errors don't matter.
    conn.send_close_notify();
    let _ = conn.write_tls(&mut sock);
    Ok(chain)
}

/// One `host:port` to check.
#[derive(Clone, Debug)]
pub struct Target {
    pub host_id: Option<Uuid>,
    pub name: String,
    pub address: String,
    pub port: u16,
}

#[derive(Debug)]
pub enum RadarEvent {
    Progress(usize, usize),
    Found(CertInfo),
    Done,
}

/// Inspects one target.
pub fn inspect(t: &Target) -> Result<CertInfo> {
    let chain = fetch_chain(&t.address, t.port)?;
    let p = parse_cert(&chain[0])?;
    Ok(CertInfo {
        host_id: t.host_id,
        host_name: t.name.clone(),
        address: t.address.clone(),
        port: t.port,
        name_ok: covers(&p, &t.address),
        subject: p.subject,
        sans: p.sans,
        issuer: p.issuer,
        not_before: p.not_before,
        not_after: p.not_after,
        self_signed: p.self_signed,
        chain_len: chain.len(),
        checked: Some(Utc::now()),
    })
}

/// Checks all targets (16 at a time) on blocking threads.
pub fn spawn_scan(rt: &tokio::runtime::Runtime, targets: Vec<Target>, sink: Sink<RadarEvent>) {
    rt.spawn(async move {
        let total = targets.len();
        let sem = Arc::new(tokio::sync::Semaphore::new(16));
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for t in targets {
            let (sem, sink, done) = (sem.clone(), sink.clone(), done.clone());
            tasks.push(tokio::spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else {
                    return;
                };
                if sink.is_closed() {
                    return;
                }
                let r = tokio::task::spawn_blocking(move || inspect(&t)).await;
                if let Ok(Ok(info)) = r {
                    sink.send(RadarEvent::Found(info));
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                sink.send(RadarEvent::Progress(n, total));
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
        sink.send(RadarEvent::Done);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// `openssl req -x509 -newkey ec … -days 400 -subj "/O=Aurora Test/CN=radar.test"
    /// -addext "subjectAltName=DNS:radar.test,DNS:*.radar.test,IP:127.0.0.1"`
    const PEM: &str = "MIIB1jCCAXygAwIBAgIUH10lraExeI6nEZfwSRpx8Cj9uFkwCgYIKoZIzj0EAwIw\
KzEUMBIGA1UECgwLQXVyb3JhIFRlc3QxEzARBgNVBAMMCnJhZGFyLnRlc3QwHhcN\
MjYxMDE4MDg0NjA3WhcNMjcxMTIyMDg0NjA3WjArMRQwEgYDVQQKDAtBdXJvcmEg\
VGVzdDETMBEGA1UEAwwKcmFkYXIudGVzdDBZMBMGByqGSM49AgEGCCqGSM49AwEH\
A0IABAY4tTMUwkMEjHoOe2SuACqMvME6H0/l9/H7TEgYwrivrrdukxnV3XG2bHfC\
8NR15WtoIZfn9DmR0nmFlSDSPpmjfjB8MB0GA1UdDgQWBBSPR4Byb70hnWY15sJP\
bfxKAJjDNjAfBgNVHSMEGDAWgBSPR4Byb70hnWY15sJPbfxKAJjDNjAPBgNVHRMB\
Af8EBTADAQH/MCkGA1UdEQQiMCCCCnJhZGFyLnRlc3SCDCoucmFkYXIudGVzdIcE\
fwAAATAKBggqhkjOPQQDAgNIADBFAiA0T1YT7pJdPK5/3piCZp2Os0zvAfYtULpe\
lVvjC+lbHgIhAIcFi4a3DpvQfCNr2ymwuuS858rpQeRYUDsZ5VW6F/87";

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn parses_certificate() {
        let der = base64::engine::general_purpose::STANDARD
            .decode(PEM)
            .unwrap();
        let p = parse_cert(&der).unwrap();
        assert_eq!(p.subject, "radar.test");
        assert_eq!(p.issuer, "radar.test");
        assert!(p.self_signed);
        assert_eq!(p.not_before, Some(utc("2026-10-18T08:46:07Z")));
        assert_eq!(p.not_after, Some(utc("2027-11-22T08:46:07Z")));
        assert_eq!(p.sans, vec!["radar.test", "*.radar.test", "127.0.0.1"]);
        assert!(covers(&p, "radar.test"));
        assert!(covers(&p, "www.radar.test"));
        assert!(covers(&p, "127.0.0.1"));
        assert!(!covers(&p, "a.b.radar.test"));
        assert!(!covers(&p, "other.test"));
        // Garbage and truncation fail cleanly.
        assert!(parse_cert(&der[..40]).is_err());
        assert!(parse_cert(b"\x30\x84\xff\xff\xff\xff").is_err());
        assert!(parse_cert(&[]).is_err());
    }

    #[test]
    fn times() {
        assert_eq!(
            parse_time(0x17, b"260101120000Z").unwrap(),
            utc("2026-01-01T12:00:00Z")
        );
        assert_eq!(
            parse_time(0x17, b"991231235959Z").unwrap(),
            utc("1999-12-31T23:59:59Z")
        );
        assert_eq!(
            parse_time(0x18, b"20500101000000Z").unwrap(),
            utc("2050-01-01T00:00:00Z")
        );
        assert!(parse_time(0x17, b"261301000000Z").is_err());
        assert!(parse_time(0x17, b"2601").is_err());
        assert!(parse_time(0x05, b"260101000000Z").is_err());
    }

    #[test]
    fn day_math() {
        let now = utc("2026-10-18T12:00:00Z");
        assert_eq!(days_between(now, utc("2026-11-01T12:00:00Z")), 14);
        assert_eq!(days_between(now, utc("2026-11-01T11:59:59Z")), 13);
        assert_eq!(days_between(now, utc("2026-10-18T13:00:00Z")), 0);
        assert_eq!(days_between(now, utc("2026-10-18T11:00:00Z")), -1);
        assert_eq!(days_between(now, utc("2026-10-10T12:00:00Z")), -8);
        assert_eq!(severity(-3), 2);
        assert_eq!(severity(6), 2);
        assert_eq!(severity(7), 1);
        assert_eq!(severity(29), 1);
        assert_eq!(severity(30), 0);
        let info = CertInfo {
            not_after: Some(utc("2026-10-23T12:00:00Z")),
            ..Default::default()
        };
        assert_eq!(info.days_left(now), 5);
    }

    #[test]
    fn names_and_ports() {
        assert!(name_matches("*.Example.com", "www.example.com."));
        assert!(!name_matches("*.example.com", "example.com"));
        assert!(name_matches("::1", "0:0:0:0:0:0:0:1"));
        assert_eq!(ports_for(None), DEFAULT_PORTS.to_vec());
        assert_eq!(ports_for(Some(&vec![])), DEFAULT_PORTS.to_vec());
        assert_eq!(ports_for(Some(&vec![22, 80, 443, 8006])), vec![443, 8006]);
    }
}
