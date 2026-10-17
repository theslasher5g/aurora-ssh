//! Port scan of one host: TCP connects from this PC to a list of ports, or the listening
//! ports as the server itself sees them (`ss -tln` over SSH, which also shows services that
//! only listen on localhost and can be reached through a tunnel).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

use super::ssh::{SshParams, connect, exec};
use super::{HostKeyPrompt, Sink};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const BANNER_TIMEOUT: Duration = Duration::from_millis(700);

/// Well-known ports with a short service name; the "Bekannte Ports" preset scans these.
pub const KNOWN: &[(u16, &str)] = &[
    (21, "FTP"),
    (22, "SSH"),
    (23, "Telnet"),
    (25, "SMTP"),
    (53, "DNS"),
    (80, "HTTP"),
    (110, "POP3"),
    (111, "rpcbind"),
    (139, "NetBIOS"),
    (143, "IMAP"),
    (443, "HTTPS"),
    (445, "SMB"),
    (465, "SMTPS"),
    (587, "Submission"),
    (631, "IPP"),
    (873, "rsync"),
    (993, "IMAPS"),
    (995, "POP3S"),
    (1433, "MS SQL"),
    (1521, "Oracle"),
    (1883, "MQTT"),
    (2049, "NFS"),
    (2375, "Docker"),
    (2376, "Docker TLS"),
    (3000, "Grafana / Web"),
    (3128, "Squid"),
    (3306, "MySQL"),
    (3389, "RDP"),
    (5000, "Web"),
    (5432, "PostgreSQL"),
    (5672, "AMQP"),
    (5900, "VNC"),
    (5985, "WinRM"),
    (6379, "Redis"),
    (6443, "Kubernetes API"),
    (8000, "Web"),
    (8006, "Proxmox"),
    (8080, "HTTP alt"),
    (8081, "Web"),
    (8086, "InfluxDB"),
    (8123, "Home Assistant"),
    (8443, "HTTPS alt"),
    (8888, "Web"),
    (9000, "Portainer / Web"),
    (9090, "Prometheus / Cockpit"),
    (9100, "Node Exporter"),
    (9200, "Elasticsearch"),
    (9443, "HTTPS alt"),
    (10050, "Zabbix Agent"),
    (11211, "Memcached"),
    (15672, "RabbitMQ UI"),
    (25565, "Minecraft"),
    (27017, "MongoDB"),
    (32400, "Plex"),
];

/// Service name of a port, `""` if unknown.
pub fn service(port: u16) -> &'static str {
    KNOWN
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, s)| *s)
        .unwrap_or("")
}

/// Ports that usually speak HTTP(S): (port, https).
pub fn web_scheme(port: u16) -> Option<&'static str> {
    match port {
        443 | 8006 | 8443 | 9443 => Some("https"),
        80 | 3000 | 5000 | 8000 | 8080 | 8081 | 8086 | 8123 | 8888 | 9000 | 9090 | 15672
        | 32400 => Some("http"),
        _ => None,
    }
}

/// `22, 80, 8000-8100` → port list (sorted, unique, at most 65535 entries).
pub fn parse_ports(input: &str) -> Result<Vec<u16>> {
    let mut out = Vec::new();
    for part in input
        .split([',', ' ', ';'])
        .filter(|p| !p.trim().is_empty())
    {
        let part = part.trim();
        let bad = || anyhow!("{}", t!("Ungültiger Port: {}", part));
        if let Some((a, b)) = part.split_once('-') {
            let a: u16 = a.trim().parse().map_err(|_| bad())?;
            let b: u16 = b.trim().parse().map_err(|_| bad())?;
            if a == 0 || b < a {
                return Err(bad());
            }
            out.extend(a..=b);
        } else {
            let p: u16 = part.parse().map_err(|_| bad())?;
            if p == 0 {
                return Err(bad());
            }
            out.push(p);
        }
    }
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        bail!(t!("Keine Ports angegeben"));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenPort {
    pub port: u16,
    /// Connect time (TCP scan) or `None` (server list).
    pub ms: Option<u32>,
    /// First line the service sent, e.g. `SSH-2.0-OpenSSH_9.6`.
    pub banner: Option<String>,
    /// Server list: the address it listens on (`0.0.0.0`, `127.0.0.1`, `[::]` …).
    pub bind: Option<String>,
    /// Server list: program name if `ss -p` could tell (only for own processes).
    pub process: Option<String>,
}

impl OpenPort {
    /// Listens only on loopback: reachable through a tunnel, not from outside.
    pub fn local_only(&self) -> bool {
        self.bind.as_deref().is_some_and(|b| {
            let b = b.trim_start_matches('[').trim_end_matches(']');
            b.starts_with("127.") || b == "::1" || b == "localhost"
        })
    }
}

pub enum PortEvent {
    Progress(usize, usize),
    Open(OpenPort),
    Error(String),
    Done,
}

/// TCP connects from this PC; dropping the receiver stops the scan.
pub fn spawn_tcp(
    rt: &tokio::runtime::Runtime,
    host: String,
    ports: Vec<u16>,
    sink: Sink<PortEvent>,
) {
    rt.spawn(async move {
        let addr = match tokio::net::lookup_host((host.as_str(), 0)).await {
            Ok(mut a) => match a.next() {
                Some(a) => a.ip(),
                None => {
                    sink.send(PortEvent::Error(t!("{} nicht gefunden", host)));
                    return;
                }
            },
            Err(e) => {
                sink.send(PortEvent::Error(format!("{host}: {e}")));
                return;
            }
        };
        let total = ports.len();
        let limit = Arc::new(Semaphore::new(200));
        let mut tasks = Vec::with_capacity(total);
        for port in ports {
            let (limit, sink) = (limit.clone(), sink.clone());
            tasks.push(tokio::spawn(async move {
                let _permit = limit.acquire().await;
                if sink.is_closed() {
                    return;
                }
                let start = Instant::now();
                let target = SocketAddr::new(addr, port);
                let Ok(Ok(mut stream)) =
                    tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await
                else {
                    return;
                };
                let ms = start.elapsed().as_millis() as u32;
                // Only read what the service sends by itself (SSH, FTP, SMTP …); never send.
                let banner = read_banner(&mut stream).await;
                sink.send(PortEvent::Open(OpenPort {
                    port,
                    ms: Some(ms),
                    banner,
                    bind: None,
                    process: None,
                }));
            }));
        }
        let mut done = 0;
        for t in tasks {
            let _ = t.await;
            done += 1;
            if done % 64 == 0 || done == total {
                sink.send(PortEvent::Progress(done, total));
            }
            if sink.is_closed() {
                return;
            }
        }
        sink.send(PortEvent::Done);
    });
}

async fn read_banner(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 200];
    let n = tokio::time::timeout(BANNER_TIMEOUT, stream.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let line: String = text
        .lines()
        .next()?
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(80)
        .collect();
    (!line.trim().is_empty()).then(|| line.trim().to_string())
}

/// Listening TCP ports as the server sees them (`ss`, else `netstat`).
pub fn spawn_listening(
    rt: &tokio::runtime::Runtime,
    params: SshParams,
    sink: Sink<PortEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        match listening(&params, prompts).await {
            Ok(list) => {
                for p in list {
                    sink.send(PortEvent::Open(p));
                }
                sink.send(PortEvent::Done);
            }
            Err(e) => sink.send(PortEvent::Error(format!("{e:#}"))),
        }
    });
}

async fn listening(p: &SshParams, prompts: Sink<HostKeyPrompt>) -> Result<Vec<OpenPort>> {
    let handle = connect(p, prompts).await?;
    let (_, out) = exec(
        &handle,
        "sh -c 'ss -Htlnp 2>/dev/null || ss -tlnp 2>/dev/null || netstat -tln 2>/dev/null || cat /proc/net/tcp /proc/net/tcp6 2>/dev/null'",
    )
    .await?;
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await;
    let list = parse_listening(&out);
    if list.is_empty() && out.trim().is_empty() {
        bail!(t!("Weder ss noch netstat auf dem Server gefunden"));
    }
    Ok(list)
}

/// Parses `ss -tln(p)` or `netstat -tln` output; one entry per port (an outside address
/// wins over loopback).
pub fn parse_listening(out: &str) -> Vec<OpenPort> {
    let mut list: Vec<OpenPort> = Vec::new();
    for line in out.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // ss: State Recv-Q Send-Q Local Peer [Process]; netstat: Proto Recv-Q Send-Q Local Foreign State
        // /proc/net/tcp: `sl: local remote st …`, hex address, state 0A = LISTEN.
        if cols.first().is_some_and(|c| c.ends_with(':')) && cols.get(3) == Some(&"0A") {
            if let Some((bind, port)) = proc_addr(cols[1]) {
                add(
                    &mut list,
                    OpenPort {
                        port,
                        ms: None,
                        banner: None,
                        bind: Some(bind),
                        process: None,
                    },
                );
            }
            continue;
        }
        let ss = cols.first() == Some(&"LISTEN") && cols.len() >= 4;
        let netstat =
            cols.first().is_some_and(|c| c.starts_with("tcp")) && cols.get(5) == Some(&"LISTEN");
        if !ss && !netstat {
            continue;
        }
        let local = cols[3];
        let Some((bind, port)) = local.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        let bind = bind.split('%').next().unwrap_or(bind).to_string();
        let process = line.split("users:((\"").nth(1).and_then(|r| {
            r.split('"')
                .next()
                .filter(|n| !n.is_empty())
                .map(|n| n.chars().take(32).collect())
        });
        let entry = OpenPort {
            port,
            ms: None,
            banner: None,
            bind: Some(bind),
            process,
        };
        add(&mut list, entry);
    }
    list.sort_by_key(|e| e.port);
    list
}

/// One entry per port; an outside address wins over loopback.
fn add(list: &mut Vec<OpenPort>, entry: OpenPort) {
    match list.iter_mut().find(|e| e.port == entry.port) {
        Some(e) => {
            if e.local_only() && !entry.local_only() {
                e.bind = entry.bind;
            }
            if e.process.is_none() {
                e.process = entry.process;
            }
        }
        None => list.push(entry),
    }
}

/// `0100007F:1F90` → ("127.0.0.1", 8080); IPv6 as four little-endian 32-bit words.
fn proc_addr(s: &str) -> Option<(String, u16)> {
    let (addr, port) = s.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let words: Vec<u32> = (0..addr.len() / 8)
        .map(|i| u32::from_str_radix(&addr[i * 8..i * 8 + 8], 16))
        .collect::<Result<_, _>>()
        .ok()?;
    let ip = match words.as_slice() {
        [w] => std::net::IpAddr::from(w.to_le_bytes()),
        [a, b, c, d] => {
            let mut bytes = [0u8; 16];
            for (i, w) in [a, b, c, d].iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            std::net::IpAddr::from(bytes)
        }
        _ => return None,
    };
    Some((ip.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_and_listening() {
        assert_eq!(
            parse_ports("22, 80 8000-8002").unwrap(),
            vec![22, 80, 8000, 8001, 8002]
        );
        assert!(parse_ports("0").is_err());
        assert!(parse_ports("90-80").is_err());
        assert!(parse_ports("").is_err());
        assert_eq!(parse_ports("1-65535").unwrap().len(), 65535);

        let ss = "LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:*\n\
                  LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n\
                  LISTEN 0 128 [::]:22 [::]:*\n\
                  LISTEN 0 511 127.0.0.1:6379 0.0.0.0:*\n\
                  LISTEN 0 511 [::1]:6379 [::]:*\n";
        let l = parse_listening(ss);
        assert_eq!(
            l.iter().map(|p| p.port).collect::<Vec<_>>(),
            vec![22, 53, 6379]
        );
        assert!(!l[0].local_only());
        assert_eq!(l[0].process.as_deref(), Some("sshd"));
        assert!(l[1].local_only());
        assert!(l[2].local_only());

        let netstat = "Active Internet connections (only servers)\n\
                       Proto Recv-Q Send-Q Local Address Foreign Address State\n\
                       tcp 0 0 0.0.0.0:80 0.0.0.0:* LISTEN\n\
                       tcp6 0 0 :::443 :::* LISTEN\n";
        let l = parse_listening(netstat);
        assert_eq!(l.iter().map(|p| p.port).collect::<Vec<_>>(), vec![80, 443]);
        assert_eq!(service(22), "SSH");

        let proc = "  sl  local_address rem_address   st\n   0: 0100007F:1F90 00000000:0000 0A 0\n   1: 00000000:08AE 00000000:0000 0A 0\n   2: 0100007F:9C40 0100007F:1F90 01 0\n   0: 00000000000000000000000001000000:18EB 00000000000000000000000000000000:0000 0A 0\n";
        let l = parse_listening(proc);
        assert_eq!(
            l.iter().map(|p| p.port).collect::<Vec<_>>(),
            vec![2222, 6379, 8080]
        );
        assert_eq!(l[0].bind.as_deref(), Some("0.0.0.0"));
        assert!(l[1].local_only(), "{:?}", l[1].bind);
        assert!(l[2].local_only());
        assert_eq!(web_scheme(8006), Some("https"));
    }
}
