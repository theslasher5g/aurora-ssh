//! "Netz durchsuchen": finds machines with an open SSH/RDP port in a small IPv4 range.
//! A plain TCP connect per address and port; on SSH ports the server's version banner
//! (sent by the server first) is read as a hint which system it is.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

use super::Sink;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);
const BANNER_TIMEOUT: Duration = Duration::from_millis(900);
/// Largest range accepted: a /22.
const MAX_ADDRESSES: usize = 1024;

#[derive(Clone, Debug)]
pub struct Found {
    pub ip: Ipv4Addr,
    pub ports: Vec<u16>,
    /// e.g. `SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13`
    pub banner: Option<String>,
}

pub enum ScanEvent {
    Progress(usize, usize),
    Found(Found),
    Done,
}

/// The local /24 of the interface used for the default route, e.g. `192.168.0.0/24`.
/// No packet is sent: connecting a UDP socket only picks the source address.
pub fn local_subnet() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
            let o = ip.octets();
            Some(format!("{}.{}.{}.0/24", o[0], o[1], o[2]))
        }
        _ => None,
    }
}

/// `192.168.0.0/24`, `10.0.0.5`, `10.0.0.10-50` → address list (at most a /22).
pub fn parse_range(input: &str) -> Result<Vec<Ipv4Addr>> {
    let s = input.trim();
    let list: Vec<Ipv4Addr> = if let Some((base, bits)) = s.split_once('/') {
        let base: Ipv4Addr = base
            .trim()
            .parse()
            .map_err(|_| anyhow!("Ungültige Adresse"))?;
        let bits: u32 = bits
            .trim()
            .parse()
            .map_err(|_| anyhow!("Ungültige Maske"))?;
        if !(22..=32).contains(&bits) {
            bail!("Bitte höchstens ein /22-Netz (1024 Adressen)");
        }
        let mask = if bits == 32 {
            u32::MAX
        } else {
            !(u32::MAX >> bits)
        };
        let net = u32::from(base) & mask;
        let size = 1u32 << (32 - bits);
        // Skip network and broadcast address for real subnets.
        let (first, last) = if size >= 4 { (1, size - 1) } else { (0, size) };
        (first..last).map(|i| Ipv4Addr::from(net + i)).collect()
    } else if let Some((a, b)) = s.split_once('-') {
        let start: Ipv4Addr = a.trim().parse().map_err(|_| anyhow!("Ungültige Adresse"))?;
        let end_last: u8 = b.trim().parse().map_err(|_| anyhow!("Ungültiges Ende"))?;
        let o = start.octets();
        if end_last < o[3] {
            bail!("Ende liegt vor dem Anfang");
        }
        (o[3]..=end_last)
            .map(|l| Ipv4Addr::new(o[0], o[1], o[2], l))
            .collect()
    } else {
        vec![s.parse().map_err(|_| anyhow!("Ungültige Adresse"))?]
    };
    if list.len() > MAX_ADDRESSES {
        bail!("Bitte höchstens 1024 Adressen");
    }
    Ok(list)
}

pub fn parse_ports(input: &str) -> Result<Vec<u16>> {
    let ports: Vec<u16> = input
        .split([',', ' '])
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            p.trim()
                .parse::<u16>()
                .map_err(|_| anyhow!("Ungültiger Port: {p}"))
        })
        .collect::<Result<_>>()?;
    if ports.is_empty() || ports.len() > 8 {
        bail!("1 bis 8 Ports angeben");
    }
    Ok(ports)
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    ips: Vec<Ipv4Addr>,
    ports: Vec<u16>,
    sink: Sink<ScanEvent>,
) {
    rt.spawn(async move {
        let total = ips.len();
        let limit = Arc::new(Semaphore::new(128));
        let mut tasks = Vec::with_capacity(total);
        for ip in ips {
            let (limit, ports, sink) = (limit.clone(), ports.clone(), sink.clone());
            tasks.push(tokio::spawn(async move {
                let _permit = limit.acquire().await;
                if sink.is_closed() {
                    return;
                }
                let mut open = Vec::new();
                let mut banner = None;
                for &port in &ports {
                    let addr = SocketAddr::new(IpAddr::V4(ip), port);
                    let Ok(Ok(mut stream)) =
                        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await
                    else {
                        continue;
                    };
                    open.push(port);
                    if banner.is_none() && port != 3389 {
                        banner = read_banner(&mut stream).await;
                    }
                }
                if !open.is_empty() {
                    sink.send(ScanEvent::Found(Found {
                        ip,
                        ports: open,
                        banner,
                    }));
                }
            }));
        }
        let mut done = 0;
        for t in tasks {
            let _ = t.await;
            done += 1;
            if done % 8 == 0 || done == total {
                sink.send(ScanEvent::Progress(done, total));
            }
        }
        sink.send(ScanEvent::Done);
    });
}

async fn read_banner(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 255];
    let n = tokio::time::timeout(BANNER_TIMEOUT, stream.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let line = text.lines().next()?.trim();
    line.starts_with("SSH-").then(|| {
        line.chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ')
            .take(80)
            .collect()
    })
}

/// "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13" → "OpenSSH 9.6 · Ubuntu"
pub fn banner_hint(banner: &str) -> String {
    let rest = banner.splitn(3, '-').nth(2).unwrap_or(banner);
    let mut parts = rest.split_whitespace();
    let server = parts.next().unwrap_or_default();
    let server = match server.split_once('_') {
        Some((name, ver)) => {
            let ver: String = ver
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            format!("{name} {ver}")
        }
        None => server.to_string(),
    };
    let os = parts
        .next()
        .map(|o| o.split(['-', '_']).next().unwrap_or(o).to_string());
    match os {
        Some(os) if !os.is_empty() => format!("{server} · {os}"),
        _ => server,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("192.168.0.0/24").unwrap().len(), 254);
        assert_eq!(
            parse_range("10.0.0.7").unwrap(),
            vec![Ipv4Addr::new(10, 0, 0, 7)]
        );
        assert_eq!(parse_range("10.0.0.10-12").unwrap().len(), 3);
        assert!(parse_range("10.0.0.0/16").is_err());
        assert_eq!(parse_ports("22, 3389").unwrap(), vec![22, 3389]);
        assert!(parse_ports("ssh").is_err());
        assert_eq!(
            banner_hint("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13"),
            "OpenSSH 9.6 · Ubuntu"
        );
        assert_eq!(banner_hint("SSH-2.0-dropbear_2022.83"), "dropbear 2022.83");
    }
}
