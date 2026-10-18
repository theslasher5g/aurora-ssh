//! Wake-on-LAN: magic packets (from this PC or from a jump host in the target's subnet),
//! MAC lookup in the ARP/neighbour table, and "wake up and connect" (wait for the port).

use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use russh::{ChannelMsg, Disconnect};

use super::ssh::{SshHandle, SshParams, connect};
use super::{HostKeyPrompt, Sink};

pub const DEFAULT_BROADCAST: &str = "255.255.255.255";
pub const DEFAULT_PORT: u16 = 9;
/// "Aufwecken und verbinden" gives up after this long.
pub const WAIT_LIMIT: Duration = Duration::from_secs(180);
const POLL_EVERY: Duration = Duration::from_secs(2);

/// `aa:bb:cc:dd:ee:ff`, `AA-BB-…`, `aabb.ccdd.eeff`, `aabbccddeeff`, also single-digit
/// groups as BSD `arp` prints them (`a:b:c:d:e:f`).
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let s = s.trim();
    let hex = |p: &str| -> Option<u8> {
        if p.is_empty() || p.len() > 2 || !p.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u8::from_str_radix(p, 16).ok()
    };
    let mut out = [0u8; 6];
    for sep in [':', '-'] {
        let parts: Vec<&str> = s.split(sep).collect();
        if parts.len() == 6 {
            for (i, p) in parts.iter().enumerate() {
                out[i] = hex(p)?;
            }
            return Some(out);
        }
    }
    let compact: String = match s.split('.').collect::<Vec<_>>().as_slice() {
        [a, b, c] if [a, b, c].iter().all(|p| p.len() == 4) => format!("{a}{b}{c}"),
        [one] => one.to_string(),
        _ => return None,
    };
    if compact.len() != 12 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn format_mac(m: [u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// 6 × `0xFF`, then the MAC 16 times.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut p = [0xffu8; 102];
    for i in 0..16 {
        p[6 + i * 6..12 + i * 6].copy_from_slice(&mac);
    }
    p
}

/// Broadcast address and port of a host's settings (empty / 0 = defaults).
pub fn target_of(broadcast: &str, port: u16) -> (String, u16) {
    let b = broadcast.trim();
    (
        if b.is_empty() {
            DEFAULT_BROADCAST.to_string()
        } else {
            b.to_string()
        },
        if port == 0 { DEFAULT_PORT } else { port },
    )
}

/// Characters allowed for an address that goes into a shell command.
fn safe_addr(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
}

/// Sends the magic packet from this PC (three times, UDP is lossy).
pub fn send_local(mac: [u8; 6], broadcast: &str, port: u16) -> Result<()> {
    let target: IpAddr = broadcast
        .parse()
        .map_err(|_| anyhow!(t!("Ungültige Broadcast-Adresse: {}", broadcast)))?;
    let sock = if target.is_ipv4() {
        UdpSocket::bind(("0.0.0.0", 0))?
    } else {
        UdpSocket::bind(("::", 0))?
    };
    sock.set_broadcast(true)?;
    let packet = magic_packet(mac);
    for _ in 0..3 {
        sock.send_to(&packet, (target, port))?;
    }
    Ok(())
}

/// Script for a jump host (runs under `sh -s`): wakeonlan, python3, etherwake, bash.
pub fn remote_script(mac: [u8; 6], broadcast: &str, port: u16) -> Result<String> {
    if !safe_addr(broadcast) {
        bail!(t!("Ungültige Broadcast-Adresse: {}", broadcast));
    }
    let m = format_mac(mac);
    let packet = magic_packet(mac);
    let hex: String = packet.iter().map(|b| format!("{b:02x}")).collect();
    let esc: String = packet.iter().map(|b| format!("\\x{b:02x}")).collect();
    Ok(format!(
        r#"M='{m}'; B='{broadcast}'; P='{port}'
if command -v wakeonlan >/dev/null 2>&1 && wakeonlan -i "$B" -p "$P" "$M" >/dev/null 2>&1; then echo via=wakeonlan; exit 0; fi
if command -v python3 >/dev/null 2>&1 && python3 -c 'import socket,sys
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET,socket.SO_BROADCAST,1)
[s.sendto(bytes.fromhex(sys.argv[1]),(sys.argv[2],int(sys.argv[3]))) for _ in range(3)]' '{hex}' "$B" "$P"; then echo via=python3; exit 0; fi
if command -v etherwake >/dev/null 2>&1 && etherwake "$M" >/dev/null 2>&1; then echo via=etherwake; exit 0; fi
if command -v bash >/dev/null 2>&1 && bash -c 'printf "$1" > "/dev/udp/$2/$3"' _ '{esc}' "$B" "$P" 2>/dev/null; then echo via=bash; exit 0; fi
echo via=none; exit 1
"#
    ))
}

/// Runs `cmd` with `input` on stdin; returns (exit status, stdout).
pub async fn exec_stdin(handle: &SshHandle, cmd: &str, input: &str) -> Result<(u32, String)> {
    let run = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        ch.data(input.as_bytes()).await?;
        ch.eof().await?;
        let mut out = Vec::new();
        let mut status = 0;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } if out.len() < 64 * 1024 => out.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => status = exit_status,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok((status, String::from_utf8_lossy(&out).into_owned()))
    };
    tokio::time::timeout(Duration::from_secs(25), run)
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
}

/// Sends the packet from the jump host; returns the tool that worked.
async fn send_remote(
    handle: &SshHandle,
    mac: [u8; 6],
    broadcast: &str,
    port: u16,
) -> Result<String> {
    let script = remote_script(mac, broadcast, port)?;
    let (status, out) = exec_stdin(handle, "sh -s", &script).await?;
    let tool = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("via="))
        .unwrap_or("")
        .to_string();
    if status != 0 || tool.is_empty() || tool == "none" {
        bail!(t!(
            "Der Jump-Host kann kein Magic Packet senden (weder wakeonlan, python3, etherwake noch bash)"
        ));
    }
    Ok(tool)
}

/// Finds the MAC of `ip` in neighbour-table output: `ip neigh`, `/proc/net/arp`,
/// `arp -n`, Windows `arp -a` and BSD/macOS `arp -a`.
pub fn parse_neigh(out: &str, ip: &str) -> Option<[u8; 6]> {
    out.lines().find_map(|line| {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let has_ip = tokens
            .iter()
            .any(|t| t.trim_matches(|c| c == '(' || c == ')') == ip);
        if !has_ip {
            return None;
        }
        tokens
            .iter()
            .filter(|t| t.contains(':') || t.contains('-'))
            .filter_map(|t| parse_mac(t))
            .find(|m| *m != [0; 6] && *m != [0xff; 6])
    })
}

async fn run_local(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let o = tokio::time::timeout(Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// First address of `host` (a literal IP stays as it is).
async fn resolve(host: &str, port: u16) -> Result<IpAddr> {
    if let Ok(ip) = host.trim().trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(ip);
    }
    let mut addrs = tokio::net::lookup_host((host.trim(), port))
        .await
        .map_err(|e| super::ssh::friendly_error(e.into()))?;
    let all: Vec<_> = addrs.by_ref().collect();
    all.iter()
        .find(|a| a.is_ipv4())
        .or(all.first())
        .map(|a| a.ip())
        .ok_or_else(|| {
            anyhow!(t!(
                "Hostname nicht gefunden – Tippfehler oder fehlt ein DNS-Eintrag?"
            ))
        })
}

/// MAC of `host` from this PC's neighbour table (after a connection attempt fills it).
pub async fn lookup_local(host: &str, port: u16) -> Result<[u8; 6]> {
    let ip = resolve(host, port).await?;
    if ip.is_loopback() {
        bail!(t!("Loopback-Adressen haben keine MAC-Adresse"));
    }
    let _ = tokio::time::timeout(
        Duration::from_millis(1200),
        tokio::net::TcpStream::connect((ip, port)),
    )
    .await;
    let ip_s = ip.to_string();
    let tries: [(&str, Vec<&str>); 4] = [
        ("ip", vec!["neigh", "show", ip_s.as_str()]),
        ("cat", vec!["/proc/net/arp"]),
        ("arp", vec!["-a", ip_s.as_str()]),
        ("arp", vec!["-an"]),
    ];
    for (program, args) in tries {
        if cfg!(windows) && program != "arp" {
            continue;
        }
        if let Some(out) = run_local(program, &args).await
            && let Some(mac) = parse_neigh(&out, &ip_s)
        {
            return Ok(mac);
        }
    }
    bail!(t!(
        "MAC von {} nicht in der ARP-Tabelle – nur Geräte im selben Netz sind sichtbar",
        ip_s
    ))
}

/// MAC of `host` from the neighbour table of a jump host in its subnet.
pub async fn lookup_via(
    params: SshParams,
    host: String,
    prompts: Sink<HostKeyPrompt>,
) -> Result<[u8; 6]> {
    if !safe_addr(&host) {
        bail!(t!("Ungültiger Hostname"));
    }
    let handle = connect(&params, prompts).await?;
    let script = format!(
        r#"H='{host}'
IP=$(getent ahostsv4 "$H" 2>/dev/null | awk 'NR==1{{print $1}}'); [ -n "$IP" ] || IP="$H"
ping -c1 -W1 "$IP" >/dev/null 2>&1
echo "ip=$IP"
ip neigh show "$IP" 2>/dev/null; cat /proc/net/arp 2>/dev/null; arp -an 2>/dev/null
"#
    );
    let (_, out) = exec_stdin(&handle, "sh -s", &script).await?;
    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "en")
        .await;
    let ip = out
        .lines()
        .find_map(|l| l.strip_prefix("ip="))
        .unwrap_or(&host)
        .trim()
        .to_string();
    parse_neigh(&out, &ip)
        .ok_or_else(|| anyhow!(t!("MAC von {} nicht in der ARP-Tabelle des Jump-Hosts", ip)))
}

/// What to wake and, optionally, what to wait for.
pub struct WakeJob {
    pub mac: [u8; 6],
    pub broadcast: String,
    pub port: u16,
    /// Send (and poll) from this jump host instead of this PC.
    pub via: Option<SshParams>,
    /// Wait until this `host:port` accepts connections.
    pub wait_for: Option<(String, u16)>,
}

#[derive(Debug)]
pub enum WakeEvent {
    /// Packet sent (`""` = from this PC, else the tool on the jump host).
    Sent(String),
    /// Still waiting (another poll failed).
    Waiting,
    Ready,
    Failed(String),
}

/// Sends the packet and waits for the port; `cancel` stops the wait.
pub fn spawn_wake(
    rt: &tokio::runtime::Runtime,
    job: WakeJob,
    sink: Sink<WakeEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    rt.spawn(async move {
        if let Err(e) = wake(job, &sink, prompts, &stop).await {
            sink.send(WakeEvent::Failed(format!("{e:#}")));
        }
    });
    cancel
}

async fn wake(
    job: WakeJob,
    sink: &Sink<WakeEvent>,
    prompts: Sink<HostKeyPrompt>,
    cancel: &AtomicBool,
) -> Result<()> {
    let via = match &job.via {
        Some(p) => Some(
            connect(p, prompts)
                .await
                .map_err(|e| anyhow!("{}", t!("Jump-Host {}: {}", p.host, format!("{e:#}"))))?,
        ),
        None => None,
    };
    match &via {
        Some(h) => {
            let tool = send_remote(h, job.mac, &job.broadcast, job.port).await?;
            sink.send(WakeEvent::Sent(tool));
        }
        None => {
            let (mac, b, port) = (job.mac, job.broadcast.clone(), job.port);
            tokio::task::spawn_blocking(move || send_local(mac, &b, port)).await??;
            sink.send(WakeEvent::Sent(String::new()));
        }
    }
    let Some((host, port)) = job.wait_for else {
        return Ok(());
    };
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || sink.is_closed() {
            return Ok(());
        }
        let round = Instant::now();
        let up = match &via {
            // Through the jump host: can it open a channel to the target yet?
            Some(h) => matches!(
                tokio::time::timeout(
                    Duration::from_millis(1800),
                    h.channel_open_direct_tcpip(host.clone(), port as u32, "127.0.0.1", 0),
                )
                .await,
                Ok(Ok(_))
            ),
            None => matches!(
                tokio::time::timeout(
                    Duration::from_millis(1800),
                    tokio::net::TcpStream::connect((host.as_str(), port)),
                )
                .await,
                Ok(Ok(_))
            ),
        };
        if up {
            sink.send(WakeEvent::Ready);
            break;
        }
        let waited = start.elapsed();
        if waited >= WAIT_LIMIT {
            bail!(t!(
                "{} ist nach 3 Minuten noch nicht erreichbar – ist Wake-on-LAN im BIOS/der Netzwerkkarte aktiv?",
                host
            ));
        }
        sink.send(WakeEvent::Waiting);
        if let Some(rest) = POLL_EVERY.checked_sub(round.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
    if let Some(h) = via {
        let _ = h.disconnect(Disconnect::ByApplication, "bye", "en").await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e];

    #[test]
    fn mac_formats() {
        for s in [
            "00:1a:2b:3c:4d:5e",
            "00-1A-2B-3C-4D-5E",
            "001a.2b3c.4d5e",
            "001A2B3C4D5E",
            " 0:1a:2b:3c:4d:5e ",
        ] {
            assert_eq!(parse_mac(s), Some(MAC), "{s}");
        }
        for s in [
            "",
            "00:1a:2b:3c:4d",
            "00:1a:2b:3c:4d:5e:6f",
            "zz:1a:2b:3c:4d:5e",
            "001:a:2b:3c:4d:5e",
            "1.2.3.4",
        ] {
            assert_eq!(parse_mac(s), None, "{s}");
        }
        assert_eq!(format_mac(MAC), "00:1a:2b:3c:4d:5e");
    }

    #[test]
    fn packet() {
        let p = magic_packet(MAC);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xff; 6]);
        for i in 0..16 {
            assert_eq!(&p[6 + i * 6..12 + i * 6], &MAC);
        }
        assert_eq!(target_of("", 0), ("255.255.255.255".to_string(), 9));
        assert_eq!(
            target_of(" 192.168.1.255 ", 7),
            ("192.168.1.255".to_string(), 7)
        );
    }

    #[test]
    fn neighbour_tables() {
        let ip_neigh = "192.168.1.10 dev eth0 lladdr 00:1a:2b:3c:4d:5e REACHABLE\n\
                        192.168.1.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff STALE\n\
                        192.168.1.99 dev eth0  FAILED\n";
        assert_eq!(parse_neigh(ip_neigh, "192.168.1.10"), Some(MAC));
        assert_eq!(
            parse_neigh(ip_neigh, "192.168.1.1"),
            Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
        assert_eq!(parse_neigh(ip_neigh, "192.168.1.99"), None);
        assert_eq!(parse_neigh(ip_neigh, "192.168.1.100"), None);

        let proc_arp = "IP address       HW type     Flags       HW address            Mask     Device\n\
                        192.168.1.10     0x1         0x2         00:1a:2b:3c:4d:5e     *        eth0\n\
                        192.168.1.20     0x1         0x0         00:00:00:00:00:00     *        eth0\n";
        assert_eq!(parse_neigh(proc_arp, "192.168.1.10"), Some(MAC));
        assert_eq!(parse_neigh(proc_arp, "192.168.1.20"), None);

        let windows = "Interface: 192.168.1.5 --- 0x7\n  \
                       Internet Address      Physical Address      Type\n  \
                       192.168.1.10          00-1a-2b-3c-4d-5e     dynamic\n  \
                       192.168.1.255         ff-ff-ff-ff-ff-ff     static\n";
        assert_eq!(parse_neigh(windows, "192.168.1.10"), Some(MAC));
        assert_eq!(parse_neigh(windows, "192.168.1.255"), None);

        let macos = "? (192.168.1.10) at 0:1a:2b:3c:4d:5e on en0 ifscope [ethernet]\n\
                     ? (192.168.1.11) at (incomplete) on en0 ifscope [ethernet]\n";
        assert_eq!(parse_neigh(macos, "192.168.1.10"), Some(MAC));
        assert_eq!(parse_neigh(macos, "192.168.1.11"), None);

        let arp_n = "Address                  HWtype  HWaddress           Flags Mask            Iface\n\
                     192.168.1.10             ether   00:1a:2b:3c:4d:5e   C                     eth0\n";
        assert_eq!(parse_neigh(arp_n, "192.168.1.10"), Some(MAC));
    }

    #[test]
    fn script_is_safe() {
        let s = remote_script(MAC, "192.168.1.255", 9).unwrap();
        assert!(s.contains("M='00:1a:2b:3c:4d:5e'"));
        assert!(s.contains(&"ff".repeat(6)));
        assert!(s.contains("\\xff\\xff"));
        assert!(remote_script(MAC, "1.2.3.4'; rm -rf ~", 9).is_err());
    }

    /// The packet really leaves through a UDP socket (unicast to localhost).
    #[test]
    fn sends_udp() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let port = rx.local_addr().unwrap().port();
        send_local(MAC, "127.0.0.1", port).unwrap();
        let mut buf = [0u8; 200];
        let (n, _) = rx.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], &magic_packet(MAC)[..]);
    }
}
