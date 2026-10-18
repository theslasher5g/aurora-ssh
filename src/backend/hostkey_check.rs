//! Second opinions on a server key Aurora has not seen before (or that changed): SSHFP
//! records in DNS and – for hosts behind a jump host – what the jump host sees
//! (`ssh-keyscan` run there). Runs while the host key dialog is open.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::Digest as _;

use super::ssh::{SshHandle, exec};

/// Result of one second source.
#[derive(Clone, Debug, PartialEq)]
pub enum Finding {
    Pending,
    /// The source knows this exact key.
    Match(String),
    /// The source knows a different key for this host and algorithm.
    Mismatch(String),
    /// The source has nothing to say (no records, tool missing, …).
    Nothing(String),
}

/// What the dialog shows; updated by the background checks.
#[derive(Clone, Debug, Default)]
pub struct CheckState {
    /// SSHFP lookup (`None` = not attempted).
    pub dns: Option<Finding>,
    /// Jump host name and its view of the key.
    pub jump: Option<(String, Finding)>,
}

pub type SharedChecks = Arc<Mutex<CheckState>>;

/// Overall result of the second sources.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Checking,
    Confirmed(String),
    Contradicts(String),
    NoSource,
}

pub fn verdict(s: &CheckState) -> Verdict {
    let jump_name = s.jump.as_ref().map(|(n, _)| n.clone()).unwrap_or_default();
    let sources = [
        (s.dns.as_ref(), t!("DNS (SSHFP)").to_string()),
        (
            s.jump.as_ref().map(|(_, f)| f),
            t!("Jump-Host {}", jump_name),
        ),
    ];
    if let Some((_, name)) = sources
        .iter()
        .find(|(f, _)| matches!(f, Some(Finding::Mismatch(_))))
    {
        return Verdict::Contradicts(name.clone());
    }
    let confirmed: Vec<String> = sources
        .iter()
        .filter(|(f, _)| matches!(f, Some(Finding::Match(_))))
        .map(|(_, n)| n.clone())
        .collect();
    if !confirmed.is_empty() {
        return Verdict::Confirmed(confirmed.join(" + "));
    }
    if sources
        .iter()
        .any(|(f, _)| matches!(f, Some(Finding::Pending)))
    {
        return Verdict::Checking;
    }
    Verdict::NoSource
}

/// `SHA256:…` (OpenSSH style) of a public key blob.
pub fn fingerprint(blob: &[u8]) -> String {
    format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(sha2::Sha256::digest(blob))
    )
}

/// Starts the checks for the server key `blob` (`algorithm` as in `ssh-ed25519`).
pub fn start(
    hostname: String,
    port: u16,
    algorithm: String,
    blob: Vec<u8>,
    via: Option<(String, Arc<SshHandle>)>,
    ctx: egui::Context,
) -> SharedChecks {
    let state = Arc::new(Mutex::new(CheckState {
        dns: Some(Finding::Pending),
        jump: via.as_ref().map(|(n, _)| (n.clone(), Finding::Pending)),
    }));
    {
        let (state, ctx, host, alg, blob) = (
            state.clone(),
            ctx.clone(),
            hostname.clone(),
            algorithm.clone(),
            blob.clone(),
        );
        tokio::spawn(async move {
            let f = dns_check(&host, &alg, &blob).await;
            if let Ok(mut s) = state.lock() {
                s.dns = Some(f);
            }
            ctx.request_repaint();
        });
    }
    if let Some((name, handle)) = via {
        let state = state.clone();
        tokio::spawn(async move {
            let f = jump_check(&handle, &name, &hostname, port, &algorithm, &blob).await;
            if let Ok(mut s) = state.lock() {
                s.jump = Some((name, f));
            }
            ctx.request_repaint();
        });
    }
    state
}

// ------------------------------------------------------------------ SSHFP (RFC 4255)

/// One SSHFP record.
#[derive(Clone, Debug, PartialEq)]
pub struct Sshfp {
    /// 1 RSA, 2 DSA, 3 ECDSA, 4 Ed25519, 6 Ed448
    pub algorithm: u8,
    /// 1 SHA-1, 2 SHA-256
    pub fp_type: u8,
    pub fingerprint: Vec<u8>,
}

/// SSHFP algorithm number of an SSH key algorithm.
pub fn sshfp_algorithm(ssh_alg: &str) -> Option<u8> {
    match ssh_alg {
        "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512" => Some(1),
        "ssh-dss" => Some(2),
        a if a.starts_with("ecdsa-sha2-") => Some(3),
        "ssh-ed25519" => Some(4),
        "ssh-ed448" => Some(6),
        _ => None,
    }
}

fn algorithm_name(n: u8) -> &'static str {
    match n {
        1 => "RSA",
        2 => "DSA",
        3 => "ECDSA",
        4 => "Ed25519",
        6 => "Ed448",
        _ => "?",
    }
}

/// RDATA of an SSHFP record: algorithm, fingerprint type, fingerprint.
pub fn parse_sshfp_rdata(rdata: &[u8]) -> Option<Sshfp> {
    let (&algorithm, rest) = rdata.split_first()?;
    let (&fp_type, fp) = rest.split_first()?;
    if fp.is_empty() {
        return None;
    }
    Some(Sshfp {
        algorithm,
        fp_type,
        fingerprint: fp.to_vec(),
    })
}

/// A record in text form: `4 2 0f3a…` (as `dig +short` prints it) or a line of zone
/// file / `nslookup` output that ends with these three fields.
#[cfg_attr(unix, allow(dead_code))]
pub fn parse_sshfp_text(line: &str) -> Option<Sshfp> {
    let tokens: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || c == '=')
        .filter(|t| !t.is_empty())
        .collect();
    tokens.windows(3).rev().find_map(|w| {
        let algorithm: u8 = w[0].parse().ok()?;
        let fp_type: u8 = w[1].parse().ok()?;
        let fingerprint = hex(w[2])?;
        let expected = match fp_type {
            1 => 20,
            2 => 32,
            _ => return None,
        };
        (fingerprint.len() == expected && (1..=6).contains(&algorithm)).then_some(Sshfp {
            algorithm,
            fp_type,
            fingerprint,
        })
    })
}

#[cfg_attr(unix, allow(dead_code))]
fn hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Compares the server key with the SSHFP records of the host.
pub fn compare_sshfp(records: &[Sshfp], ssh_alg: &str, blob: &[u8], dnssec: bool) -> Finding {
    let Some(alg) = sshfp_algorithm(ssh_alg) else {
        return Finding::Nothing(t!("Schlüsseltyp {} hat keine SSHFP-Nummer", ssh_alg));
    };
    let mine: Vec<&Sshfp> = records.iter().filter(|r| r.algorithm == alg).collect();
    if records.is_empty() {
        return Finding::Nothing(t!("keine SSHFP-Einträge im DNS").to_string());
    }
    if mine.is_empty() {
        return Finding::Nothing(t!(
            "SSHFP-Einträge nur für andere Schlüsseltypen, nicht für {}",
            algorithm_name(alg)
        ));
    }
    let sha1 = <sha1::Sha1 as sha1::Digest>::digest(blob);
    let sha256 = sha2::Sha256::digest(blob);
    let hit = mine.iter().find(|r| match r.fp_type {
        1 => r.fingerprint.as_slice() == sha1.as_slice(),
        2 => r.fingerprint.as_slice() == sha256.as_slice(),
        _ => false,
    });
    let trust = if dnssec {
        t!("DNSSEC-validiert")
    } else {
        t!("ohne DNSSEC – nur so sicher wie dein DNS")
    };
    match hit {
        Some(r) => Finding::Match(format!(
            "{} · {}",
            t!(
                "SSHFP {} {} passt",
                algorithm_name(alg),
                if r.fp_type == 1 { "SHA-1" } else { "SHA-256" }
            ),
            trust
        )),
        None => Finding::Mismatch(format!(
            "{} · {}",
            t!(
                "{} SSHFP-Einträge für {}, keiner passt zu diesem Schlüssel",
                mine.len(),
                algorithm_name(alg)
            ),
            trust
        )),
    }
}

async fn dns_check(host: &str, alg: &str, blob: &[u8]) -> Finding {
    let host = host.trim().trim_end_matches('.');
    if host.parse::<IpAddr>().is_ok() || host.starts_with('[') {
        return Finding::Nothing(t!("IP-Adresse – kein DNS-Name für SSHFP").to_string());
    }
    if host.eq_ignore_ascii_case("localhost") || !host.contains('.') {
        return Finding::Nothing(
            t!("kein voller DNS-Name (FQDN) – SSHFP übersprungen").to_string(),
        );
    }
    match lookup_sshfp(host).await {
        Ok((records, ad)) => compare_sshfp(&records, alg, blob, ad),
        Err(e) => Finding::Nothing(t!("DNS-Abfrage fehlgeschlagen: {}", e)),
    }
}

#[cfg(unix)]
async fn lookup_sshfp(name: &str) -> Result<(Vec<Sshfp>, bool)> {
    let conf = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    let server = parse_resolv_conf(&conf).ok_or_else(|| anyhow!(t!("kein DNS-Server gefunden")))?;
    query_sshfp(SocketAddr::new(server, 53), name).await
}

/// Windows: no resolv.conf – ask `nslookup` and read the records from its output.
#[cfg(not(unix))]
async fn lookup_sshfp(name: &str) -> Result<(Vec<Sshfp>, bool)> {
    let mut cmd = tokio::process::Command::new("nslookup");
    cmd.args(["-type=SSHFP", name]);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(8), cmd.output())
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))??;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok((text.lines().filter_map(parse_sshfp_text).collect(), false))
}

/// First `nameserver` of a resolv.conf.
pub fn parse_resolv_conf(text: &str) -> Option<IpAddr> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next()? == "nameserver")
            .then(|| it.next())
            .flatten()
            .and_then(|a| a.split('%').next()?.parse().ok())
    })
}

const TYPE_SSHFP: u16 = 44;

/// DNS query for the SSHFP records of `name` (recursion desired, AD bit set so a
/// validating resolver reports DNSSEC, EDNS0 for 1232-byte answers).
pub fn build_query(id: u16, name: &str) -> Result<Vec<u8>> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0120u16.to_be_bytes()); // RD + AD
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 1]); // 1 question, 1 additional (OPT)
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            bail!(t!("ungültiger DNS-Name"));
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_SSHFP.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes()); // IN
    // OPT pseudo record: root name, type 41, UDP size 1232, no flags, no data.
    q.extend_from_slice(&[0, 0, 41, 0x04, 0xd0, 0, 0, 0, 0, 0, 0]);
    Ok(q)
}

fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *msg.get(pos)? as usize;
        if len & 0xc0 == 0xc0 {
            return Some(pos + 2);
        }
        if len == 0 {
            return Some(pos + 1);
        }
        pos += 1 + len;
    }
}

fn u16_at(msg: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*msg.get(pos)?, *msg.get(pos + 1)?]))
}

/// Parsed answer: SSHFP records, AD (authentic data) flag, truncated flag.
#[derive(Debug, PartialEq)]
pub struct DnsAnswer {
    pub records: Vec<Sshfp>,
    pub authentic: bool,
    pub truncated: bool,
}

pub fn parse_response(id: u16, msg: &[u8]) -> Result<DnsAnswer> {
    let bad = || anyhow!(t!("ungültige DNS-Antwort"));
    if msg.len() < 12 || u16_at(msg, 0) != Some(id) {
        return Err(bad());
    }
    let flags = u16_at(msg, 2).ok_or_else(bad)?;
    if flags & 0x8000 == 0 {
        return Err(bad());
    }
    let rcode = flags & 0x000f;
    if rcode == 3 {
        bail!(t!("Name existiert nicht (NXDOMAIN)"));
    }
    if rcode != 0 {
        bail!("{}", t!("DNS-Fehler {}", rcode));
    }
    let qd = u16_at(msg, 4).ok_or_else(bad)?;
    let an = u16_at(msg, 6).ok_or_else(bad)?;
    let mut pos = 12;
    for _ in 0..qd {
        pos = skip_name(msg, pos).ok_or_else(bad)? + 4;
    }
    let mut records = Vec::new();
    for _ in 0..an {
        pos = skip_name(msg, pos).ok_or_else(bad)?;
        let rtype = u16_at(msg, pos).ok_or_else(bad)?;
        let len = u16_at(msg, pos + 8).ok_or_else(bad)? as usize;
        let start = pos + 10;
        let rdata = msg.get(start..start + len).ok_or_else(bad)?;
        if rtype == TYPE_SSHFP
            && let Some(r) = parse_sshfp_rdata(rdata)
        {
            records.push(r);
        }
        pos = start + len;
    }
    Ok(DnsAnswer {
        records,
        authentic: flags & 0x0020 != 0,
        truncated: flags & 0x0200 != 0,
    })
}

async fn query_sshfp(server: SocketAddr, name: &str) -> Result<(Vec<Sshfp>, bool)> {
    use rand::Rng;
    let id: u16 = rand::thread_rng().r#gen();
    let query = build_query(id, name)?;
    let bind: SocketAddr = if server.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let sock = tokio::net::UdpSocket::bind(bind).await?;
    sock.connect(server).await?;
    let mut buf = vec![0u8; 4096];
    let mut answer = None;
    for _ in 0..2 {
        sock.send(&query).await?;
        // Ignore stray datagrams with another id until the timeout.
        let read = async {
            loop {
                let n = sock.recv(&mut buf).await?;
                if let Ok(a) = parse_response(id, &buf[..n]) {
                    return anyhow::Ok(a);
                }
                if u16_at(&buf[..n], 0) == Some(id) {
                    return parse_response(id, &buf[..n]);
                }
            }
        };
        if let Ok(r) = tokio::time::timeout(Duration::from_secs(3), read).await {
            answer = Some(r?);
            break;
        }
    }
    let answer = answer.ok_or_else(|| anyhow!(t!("DNS-Server antwortet nicht")))?;
    if !answer.truncated {
        return Ok((answer.records, answer.authentic));
    }
    // Too big for UDP: the same query over TCP.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let run = async {
        let mut tcp = tokio::net::TcpStream::connect(server).await?;
        let mut framed = (query.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(&query);
        tcp.write_all(&framed).await?;
        let len = tcp.read_u16().await? as usize;
        let mut msg = vec![0u8; len];
        tcp.read_exact(&mut msg).await?;
        parse_response(id, &msg)
    };
    let a = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .map_err(|_| anyhow!(t!("DNS-Server antwortet nicht")))??;
    Ok((a.records, a.authentic))
}

// ------------------------------------------------------------------ jump host view

/// Command that asks the jump host for the target's keys; `None` for names that do
/// not belong in a shell command.
pub fn keyscan_command(host: &str, port: u16) -> Option<String> {
    let host = host.trim();
    let ok = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'));
    ok.then(|| format!("ssh-keyscan -T 5 -p {port} -t ed25519,ecdsa,rsa {host} 2>/dev/null"))
}

/// `host keytype base64` lines of `ssh-keyscan` → (key type, key blob).
pub fn parse_keyscan(text: &str) -> Vec<(String, Vec<u8>)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let _host = it.next()?;
            let alg = it.next()?;
            let blob = STANDARD.decode(it.next()?).ok()?;
            alg.starts_with(|c: char| c.is_ascii_alphabetic())
                .then(|| (alg.to_string(), blob))
        })
        .collect()
}

/// Compares the key the client got with the keys the jump host sees.
pub fn compare_keyscan(
    entries: &[(String, Vec<u8>)],
    jump: &str,
    alg: &str,
    blob: &[u8],
) -> Finding {
    if entries.iter().any(|(_, b)| b == blob) {
        return Finding::Match(t!("{} sieht denselben Schlüssel", jump));
    }
    let base = |a: &str| match a {
        "rsa-sha2-256" | "rsa-sha2-512" => "ssh-rsa".to_string(),
        other => other.to_string(),
    };
    match entries.iter().find(|(a, _)| base(a) == base(alg)) {
        Some((_, other)) => Finding::Mismatch(t!(
            "{} sieht einen anderen Schlüssel: {}",
            jump,
            fingerprint(other)
        )),
        None if entries.is_empty() => Finding::Nothing(t!(
            "{} liefert keine Schlüssel (ssh-keyscan fehlt oder Ziel nicht erreichbar)",
            jump
        )),
        None => Finding::Nothing(t!("{} sieht keinen {}-Schlüssel", jump, alg)),
    }
}

async fn jump_check(
    handle: &SshHandle,
    jump: &str,
    host: &str,
    port: u16,
    alg: &str,
    blob: &[u8],
) -> Finding {
    let Some(cmd) = keyscan_command(host, port) else {
        return Finding::Nothing(t!("ungültiger Hostname").to_string());
    };
    match exec(handle, &cmd).await {
        Ok((_, out)) => compare_keyscan(&parse_keyscan(&out), jump, alg, blob),
        Err(e) => Finding::Nothing(t!("ssh-keyscan auf {} fehlgeschlagen: {}", jump, e)),
    }
}

/// Command that prints the fingerprint of the server's own key file, to run on the
/// server console (e.g. Proxmox noVNC) and compare by eye.
pub fn console_fingerprint_command(alg: &str) -> String {
    let file = match alg {
        a if a.starts_with("ecdsa") => "ssh_host_ecdsa_key",
        "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512" => "ssh_host_rsa_key",
        _ => "ssh_host_ed25519_key",
    };
    format!("ssh-keygen -lf /etc/ssh/{file}.pub")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed25519_blob() -> (String, Vec<u8>) {
        let (_, public) = crate::keygen::generate_ed25519("t").unwrap();
        let mut it = public.split_whitespace();
        let alg = it.next().unwrap().to_string();
        (alg, STANDARD.decode(it.next().unwrap()).unwrap())
    }

    #[test]
    fn sshfp_sha256_and_sha1_compare() {
        let (alg, blob) = ed25519_blob();
        let sha256 = sha2::Sha256::digest(&blob).to_vec();
        let sha1 = <sha1::Sha1 as sha1::Digest>::digest(&blob).to_vec();
        let rec = |t: u8, fp: Vec<u8>| Sshfp {
            algorithm: 4,
            fp_type: t,
            fingerprint: fp,
        };
        assert!(matches!(
            compare_sshfp(&[rec(2, sha256.clone())], &alg, &blob, true),
            Finding::Match(_)
        ));
        assert!(matches!(
            compare_sshfp(&[rec(1, sha1.clone())], &alg, &blob, false),
            Finding::Match(_)
        ));
        let mut wrong = sha256.clone();
        wrong[0] ^= 1;
        assert!(matches!(
            compare_sshfp(&[rec(2, wrong)], &alg, &blob, false),
            Finding::Mismatch(_)
        ));
        // Records for RSA only say nothing about an Ed25519 key.
        let rsa = Sshfp {
            algorithm: 1,
            fp_type: 2,
            fingerprint: vec![0; 32],
        };
        assert!(matches!(
            compare_sshfp(&[rsa], &alg, &blob, false),
            Finding::Nothing(_)
        ));
        assert!(matches!(
            compare_sshfp(&[], &alg, &blob, false),
            Finding::Nothing(_)
        ));
    }

    #[test]
    fn sshfp_text_forms() {
        let h = "a".repeat(64);
        let r = parse_sshfp_text(&format!("4 2 {h}")).unwrap();
        assert_eq!((r.algorithm, r.fp_type, r.fingerprint.len()), (4, 2, 32));
        let s1 = "0123456789abcdef0123456789ABCDEF01234567";
        let r = parse_sshfp_text(&format!("host.example.com. 300 IN SSHFP 1 1 {s1}")).unwrap();
        assert_eq!((r.algorithm, r.fp_type, r.fingerprint[0]), (1, 1, 0x01));
        // Wrong length for the type, unknown type, garbage.
        assert!(parse_sshfp_text(&format!("4 2 {s1}")).is_none());
        assert!(parse_sshfp_text(&format!("4 3 {h}")).is_none());
        assert!(parse_sshfp_text("Server: 127.0.0.53").is_none());
    }

    #[test]
    fn dns_wire_roundtrip() {
        let q = build_query(0x1234, "web.example.com").unwrap();
        assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x20]);
        // Fake answer: copy the question, set QR/RA/AD, one SSHFP answer via pointer.
        let mut a = q[..q.len() - 11].to_vec();
        a[2] = 0x81;
        a[3] = 0xa0;
        a[7] = 1; // ANCOUNT
        a[11] = 0; // ARCOUNT
        let mut rdata = vec![4u8, 2];
        rdata.extend_from_slice(&[0xab; 32]);
        a.extend_from_slice(&[0xc0, 12, 0, 44, 0, 1, 0, 0, 1, 0x2c]);
        a.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        a.extend_from_slice(&rdata);
        let r = parse_response(0x1234, &a).unwrap();
        assert!(r.authentic && !r.truncated);
        assert_eq!(r.records.len(), 1);
        assert_eq!(r.records[0].fingerprint, vec![0xab; 32]);
        assert!(parse_response(0x9999, &a).is_err());
        a[3] = 0x83; // NXDOMAIN
        assert!(parse_response(0x1234, &a).is_err());
        assert!(build_query(1, "bad..name").is_err());
    }

    #[test]
    fn resolv_conf() {
        let t = "# x\nsearch lan\nnameserver 10.0.0.1\nnameserver 1.1.1.1\n";
        assert_eq!(parse_resolv_conf(t), Some("10.0.0.1".parse().unwrap()));
        assert_eq!(
            parse_resolv_conf("nameserver fe80::1%eth0"),
            Some("fe80::1".parse().unwrap())
        );
        assert_eq!(parse_resolv_conf("search lan"), None);
    }

    #[test]
    fn keyscan_output() {
        let (alg, blob) = ed25519_blob();
        let b64 = STANDARD.encode(&blob);
        let out = format!(
            "# 10.0.0.5:22 SSH-2.0-OpenSSH_9.6\n10.0.0.5 {alg} {b64}\n\
             10.0.0.5 ecdsa-sha2-nistp256 AAAAE2VjZHNh\n\n"
        );
        let entries = parse_keyscan(&out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], (alg.clone(), blob.clone()));
        assert!(matches!(
            compare_keyscan(&entries, "jump", &alg, &blob),
            Finding::Match(_)
        ));
        let (_, other) = ed25519_blob();
        assert!(matches!(
            compare_keyscan(&entries, "jump", &alg, &other),
            Finding::Mismatch(_)
        ));
        assert!(matches!(
            compare_keyscan(&[], "jump", &alg, &blob),
            Finding::Nothing(_)
        ));
        assert!(matches!(
            compare_keyscan(&entries[1..], "jump", &alg, &blob),
            Finding::Nothing(_)
        ));
    }

    #[test]
    fn keyscan_command_rejects_shell_text() {
        assert_eq!(
            keyscan_command("db.lan", 2201).unwrap(),
            "ssh-keyscan -T 5 -p 2201 -t ed25519,ecdsa,rsa db.lan 2>/dev/null"
        );
        assert!(keyscan_command("a;rm -rf /", 22).is_none());
        assert!(keyscan_command("-oProxyCommand=x", 22).is_none());
    }

    #[test]
    fn verdicts() {
        let mut s = CheckState {
            dns: Some(Finding::Pending),
            jump: None,
        };
        assert_eq!(verdict(&s), Verdict::Checking);
        s.dns = Some(Finding::Nothing(String::new()));
        assert_eq!(verdict(&s), Verdict::NoSource);
        s.jump = Some(("j".into(), Finding::Match(String::new())));
        assert!(matches!(verdict(&s), Verdict::Confirmed(_)));
        s.dns = Some(Finding::Mismatch(String::new()));
        assert!(matches!(verdict(&s), Verdict::Contradicts(_)));
    }

    #[test]
    fn fingerprint_matches_ssh_key() {
        let (_, public) = crate::keygen::generate_ed25519("t").unwrap();
        let k = russh::keys::ssh_key::PublicKey::from_openssh(&public).unwrap();
        let blob = k.to_bytes().unwrap();
        assert_eq!(
            fingerprint(&blob),
            k.fingerprint(russh::keys::HashAlg::Sha256).to_string()
        );
    }
}
