//! Jump-host chains and the route finder ("Weg finden"): when a host can't be reached
//! directly, saved SSH hosts that probably sit next to it are asked whether they reach it
//! (`nc -z` / bash `/dev/tcp`, then a real `direct-tcpip` channel – what a ProxyJump uses).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use russh::Disconnect;
use uuid::Uuid;

use super::ssh::{SshParams, connect, exec};
use super::{HostKeyPrompt, Sink};
use crate::model::{Host, Library, Protocol};

/// At most this many jump hosts in front of a target.
pub const MAX_HOPS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainError {
    /// A jump host (indirectly) jumps through itself or the target.
    Cycle,
    /// More than [`MAX_HOPS`] jump hosts.
    TooLong,
    /// A jump host was deleted.
    Missing,
}

impl ChainError {
    pub fn text(&self) -> &'static str {
        match self {
            ChainError::Cycle => t!("Die Jump-Kette enthält eine Schleife"),
            ChainError::TooLong => t!("Mehr als 4 Jump-Hosts hintereinander"),
            ChainError::Missing => t!("Ein Jump-Host der Kette existiert nicht mehr"),
        }
    }
}

/// The jump hosts in front of `host`, outermost first (the one Aurora connects to
/// directly), the hop next to the target last.
pub fn jump_chain(lib: &Library, host: &Host) -> Result<Vec<Uuid>, ChainError> {
    let mut seen = HashSet::from([host.id]);
    let mut chain = Vec::new();
    let mut next = host.jump_host;
    while let Some(id) = next {
        if !seen.insert(id) {
            return Err(ChainError::Cycle);
        }
        if chain.len() == MAX_HOPS {
            return Err(ChainError::TooLong);
        }
        let hop = lib.host(id).ok_or(ChainError::Missing)?;
        chain.push(id);
        next = hop.jump_host;
    }
    chain.reverse();
    Ok(chain)
}

/// Like [`jump_chain`], but keeps the usable part of a broken chain (the hops in front
/// of the target up to the loop / limit / gap).
pub fn jump_chain_lossy(lib: &Library, host: &Host) -> Vec<Uuid> {
    let mut seen = HashSet::from([host.id]);
    let mut chain = Vec::new();
    let mut next = host.jump_host;
    while let Some(id) = next {
        if !seen.insert(id) || chain.len() == MAX_HOPS {
            break;
        }
        let Some(hop) = lib.host(id) else { break };
        chain.push(id);
        next = hop.jump_host;
    }
    chain.reverse();
    chain
}

/// "bastion-01 → db-03" for the hops of a chain.
pub fn chain_names(lib: &Library, chain: &[Uuid]) -> String {
    chain
        .iter()
        .filter_map(|id| lib.host(*id))
        .map(|h| h.display_name().to_string())
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Would `candidate` as jump host of `target` lead back to `target`?
pub fn would_loop(lib: &Library, target: Uuid, candidate: Uuid) -> bool {
    if candidate == target {
        return true;
    }
    let Some(c) = lib.host(candidate) else {
        return true;
    };
    jump_chain_lossy(lib, c).contains(&target)
}

/// Hostname characters that are safe inside a shell command.
pub fn safe_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'))
}

/// First three octets of an IPv4 address.
fn net24(addr: &str) -> Option<[u8; 3]> {
    let ip: std::net::Ipv4Addr = addr.trim().parse().ok()?;
    let o = ip.octets();
    Some([o[0], o[1], o[2]])
}

/// Parent domain (`db.lan.example` → `lan.example`), for a weak "same site" hint.
fn parent_domain(name: &str) -> Option<&str> {
    let name = name.trim();
    if name.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let (_, rest) = name.split_once('.')?;
    rest.contains('.').then_some(rest)
}

/// How much a saved host looks like a way to `target` (0 = no hint at all).
pub fn candidate_score(lib: &Library, target: &Host, c: &Host) -> u32 {
    let mut score = 0;
    let tagged = c.tags.iter().any(|t| {
        let t = t.to_lowercase();
        t.contains("bastion") || t.contains("jump")
    });
    if tagged {
        score += 4;
    }
    let name = c.display_name().to_lowercase();
    if ["bastion", "jump", "gateway", "gw"]
        .iter()
        .any(|k| name.contains(k))
    {
        score += 1;
    }
    if let (Some(a), Some(b)) = (net24(&target.hostname), net24(&c.hostname))
        && a == b
    {
        score += 3;
    }
    if let (Some(a), Some(b)) = (parent_domain(&target.hostname), parent_domain(&c.hostname))
        && a == b
    {
        score += 1;
    }
    if target.group_id.is_some() && target.group_id == c.group_id {
        score += 2;
    }
    // Already used as jump host by hosts of the same group/subnet.
    if lib.hosts.iter().any(|h| {
        h.jump_host == Some(c.id)
            && h.id != target.id
            && (h.group_id == target.group_id
                || net24(&h.hostname).is_some_and(|n| Some(n) == net24(&target.hostname)))
    }) {
        score += 2;
    }
    score
}

/// One possible jump host, with its own connection parameters (incl. its own chain).
pub struct Candidate {
    pub id: Uuid,
    pub params: SshParams,
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub id: Uuid,
    /// A `direct-tcpip` channel to the target opened through this host.
    pub ok: bool,
    /// Time to open that channel (≈ round trip candidate → target plus here → candidate).
    pub ms: Option<u32>,
    /// What went wrong, or a remark.
    pub note: String,
}

#[derive(Debug)]
pub enum RouteEvent {
    Probe(Probe),
    Done,
}

const PER_CANDIDATE: Duration = Duration::from_secs(25);

/// Probes all candidates concurrently.
pub fn spawn_finder(
    rt: &tokio::runtime::Runtime,
    target: String,
    port: u16,
    candidates: Vec<Candidate>,
    sink: Sink<RouteEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let mut tasks = Vec::new();
        for c in candidates {
            let (target, sink, prompts) = (target.clone(), sink.clone(), prompts.clone());
            tasks.push(tokio::spawn(async move {
                let id = c.id;
                let probe = match tokio::time::timeout(
                    PER_CANDIDATE,
                    probe(&c.params, &target, port, prompts),
                )
                .await
                {
                    Ok(Ok(p)) => Probe { id, ..p },
                    Ok(Err(e)) => Probe {
                        id,
                        ok: false,
                        ms: None,
                        note: format!("{e:#}"),
                    },
                    Err(_) => Probe {
                        id,
                        ok: false,
                        ms: None,
                        note: t!("Zeitüberschreitung").into(),
                    },
                };
                if !sink.is_closed() {
                    sink.send(RouteEvent::Probe(probe));
                }
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
        sink.send(RouteEvent::Done);
    });
}

/// Shell probe: `nc -z`, else bash `/dev/tcp`. Exit 0 = port open, 1 = closed,
/// 3 = neither tool exists.
pub fn probe_command(target: &str, port: u16) -> String {
    format!(
        "sh -c 'if command -v nc >/dev/null 2>&1; then nc -z -w2 {target} {port}; \
         elif command -v bash >/dev/null 2>&1; then timeout 3 bash -c \"exec 3<>/dev/tcp/{target}/{port}\" 2>/dev/null; \
         else exit 3; fi'"
    )
}

async fn probe(
    params: &SshParams,
    target: &str,
    port: u16,
    prompts: Sink<HostKeyPrompt>,
) -> Result<Probe> {
    if !safe_host(target) {
        return Err(anyhow!(t!("Ungültiger Hostname")));
    }
    let handle = connect(params, prompts).await?;
    // 1. Ask the shell (tells "port closed" apart from "forwarding disabled").
    let shell = match tokio::time::timeout(
        Duration::from_secs(8),
        exec(&handle, &probe_command(target, port)),
    )
    .await
    {
        Ok(Ok((0, _))) => Some(true),
        Ok(Ok((3, _))) => None,
        Ok(Ok(_)) => Some(false),
        _ => None,
    };
    // 2. What a ProxyJump really does: a direct-tcpip channel.
    let start = Instant::now();
    let channel = tokio::time::timeout(
        Duration::from_secs(6),
        handle.channel_open_direct_tcpip(target.to_string(), port as u32, "127.0.0.1", 0),
    )
    .await;
    let ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    let result = match channel {
        Ok(Ok(ch)) => {
            let _ = ch.close().await;
            Probe {
                id: Uuid::nil(),
                ok: true,
                ms: Some(ms),
                note: String::new(),
            }
        }
        Ok(Err(e)) => Probe {
            id: Uuid::nil(),
            ok: false,
            ms: None,
            note: if shell == Some(true) {
                t!("erreicht das Ziel, erlaubt aber keine Weiterleitung (AllowTcpForwarding)")
                    .to_string()
            } else if shell == Some(false) {
                t!("erreicht das Ziel nicht").to_string()
            } else {
                format!("{e}")
            },
        },
        Err(_) => Probe {
            id: Uuid::nil(),
            ok: false,
            ms: None,
            note: t!("erreicht das Ziel nicht").to_string(),
        },
    };
    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "en")
        .await;
    Ok(result)
}

/// Best first: working routes by latency, then the rest.
pub fn rank(probes: &mut [Probe]) {
    probes.sort_by_key(|p| (!p.ok, p.ms.unwrap_or(u32::MAX)));
}

/// Does a connection error mean "can't get there from here" (worth looking for a route)?
pub fn looks_unreachable(err: &str) -> bool {
    [
        t!("Zeitüberschreitung – Host nicht erreichbar"),
        t!("Zeitüberschreitung – Host antwortet nicht (aus, Firewall oder falscher Port?)"),
        t!("Netzwerk nicht erreichbar – bist du im richtigen Netz / VPN?"),
        t!("Hostname nicht gefunden – Tippfehler oder fehlt ein DNS-Eintrag?"),
    ]
    .iter()
    .any(|m| err.contains(m))
}

/// SSH hosts worth asking, best guesses first: never the target itself or hosts that
/// jump through it; without any hint all other SSH hosts are tried.
pub fn candidates_for(
    lib: &Library,
    target: &Host,
    reachable: impl Fn(Uuid) -> Option<bool>,
) -> Vec<Uuid> {
    let mut scored: Vec<(u32, Uuid)> = lib
        .hosts
        .iter()
        .filter(|c| c.protocol == Protocol::Ssh && c.id != target.id)
        .filter(|c| !c.hostname.trim().is_empty())
        .filter(|c| !would_loop(lib, target.id, c.id))
        // Known to be down (and not behind its own jump host): no use asking.
        .filter(|c| c.jump_host.is_some() || reachable(c.id) != Some(false))
        .map(|c| (candidate_score(lib, target, c), c.id))
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    if scored.iter().any(|(s, _)| *s > 0) {
        scored.retain(|(s, _)| *s > 0);
    }
    scored.into_iter().take(8).map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, addr: &str) -> Host {
        Host {
            label: name.into(),
            hostname: addr.into(),
            ..Default::default()
        }
    }

    #[test]
    fn chains_and_cycles() {
        let mut lib = Library::default();
        let a = host("a", "10.0.0.1");
        let mut b = host("b", "10.0.1.1");
        let mut c = host("c", "10.0.2.1");
        b.jump_host = Some(a.id);
        c.jump_host = Some(b.id);
        let (aid, bid, cid) = (a.id, b.id, c.id);
        lib.hosts = vec![a, b, c.clone()];
        assert_eq!(jump_chain(&lib, &c), Ok(vec![aid, bid]));
        assert_eq!(chain_names(&lib, &[aid, bid]), "a → b");
        assert!(would_loop(&lib, aid, cid));
        assert!(!would_loop(&lib, cid, aid));
        // a → c closes the loop.
        lib.hosts[0].jump_host = Some(cid);
        let c = lib.host(cid).unwrap().clone();
        assert_eq!(jump_chain(&lib, &c), Err(ChainError::Cycle));
        assert_eq!(jump_chain_lossy(&lib, &c), vec![aid, bid]);
        // Self reference.
        let mut s = host("s", "x");
        s.jump_host = Some(s.id);
        assert_eq!(jump_chain(&lib, &s), Err(ChainError::Cycle));
        assert!(jump_chain_lossy(&lib, &s).is_empty());
    }

    #[test]
    fn chain_limit() {
        let mut lib = Library::default();
        let mut prev: Option<Uuid> = None;
        for i in 0..6 {
            let mut h = host(&format!("h{i}"), "x");
            h.jump_host = prev;
            prev = Some(h.id);
            lib.hosts.push(h);
        }
        let last = lib.hosts[5].clone();
        assert_eq!(jump_chain(&lib, &last), Err(ChainError::TooLong));
        assert_eq!(jump_chain_lossy(&lib, &last).len(), MAX_HOPS);
        let fourth = lib.hosts[4].clone();
        assert_eq!(jump_chain(&lib, &fourth).unwrap().len(), 4);
    }

    #[test]
    fn scores() {
        let mut lib = Library::default();
        let target = host("db-03", "192.168.7.30");
        let mut bastion = host("bastion-01", "203.0.113.5");
        bastion.tags = vec!["bastion".into()];
        let neighbour = host("web", "192.168.7.10");
        let other = host("far", "10.1.1.1");
        let (bid, nid, oid) = (bastion.id, neighbour.id, other.id);
        lib.hosts = vec![target.clone(), bastion, neighbour, other];
        assert!(candidate_score(&lib, &target, lib.host(bid).unwrap()) >= 4);
        assert_eq!(candidate_score(&lib, &target, lib.host(nid).unwrap()), 3);
        assert_eq!(candidate_score(&lib, &target, lib.host(oid).unwrap()), 0);
        let c = candidates_for(&lib, &target, |_| None);
        assert_eq!(c, vec![bid, nid]);
        // Known-down hosts are skipped.
        let c = candidates_for(&lib, &target, |id| Some(id != bid));
        assert_eq!(c, vec![nid]);
    }

    #[test]
    fn ranking_and_hosts() {
        let mk = |ok, ms| Probe {
            id: Uuid::new_v4(),
            ok,
            ms,
            note: String::new(),
        };
        let mut v = vec![mk(false, None), mk(true, Some(80)), mk(true, Some(12))];
        rank(&mut v);
        assert_eq!(v[0].ms, Some(12));
        assert!(!v[2].ok);
        assert!(safe_host("db-03.lan"));
        assert!(safe_host("fe80::1%eth0"));
        assert!(!safe_host("a;rm -rf /"));
        assert!(!safe_host("$(x)"));
        assert!(probe_command("10.0.0.5", 22).contains("nc -z -w2 10.0.0.5 22"));
    }
}
