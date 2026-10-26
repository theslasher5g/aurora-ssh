//! Live host statistics: one persistent exec channel per watched host runs a small shell
//! loop that prints `/proc` counters every few seconds; this module parses the blocks,
//! turns counter deltas into rates and keeps a bounded history. Hosts without `/proc`
//! (BSD, Windows OpenSSH) report "unsupported". The script is a constant; only the
//! interval (a clamped integer) is put into it.

use std::collections::VecDeque;

use russh::ChannelMsg;
use uuid::Uuid;

use super::inventory_collect::sh_wrap;
use super::ssh::{SshParams, connect};
use super::{HostKeyPrompt, Sink};

pub const MIN_INTERVAL: u32 = 2;
pub const MAX_INTERVAL: u32 = 10;
pub const DEFAULT_INTERVAL: u32 = 3;
/// How much history is kept.
pub const WINDOW_SECS: u32 = 600;

/// The loop that runs on the host. Interface names of bridges and containers are left
/// out of the network sum so traffic is not counted twice.
pub fn script(interval: u32) -> String {
    let interval = interval.clamp(MIN_INTERVAL, MAX_INTERVAL);
    sh_wrap(&format!(
        r#"export LC_ALL=C
if [ ! -r /proc/stat ] || [ ! -r /proc/meminfo ]; then echo '@@unsupported'; exit 0; fi
n=0
while :; do
echo '@@s'
cut -d' ' -f1 /proc/uptime | sed 's/^/up /'
head -n1 /proc/stat
grep -E '^(MemTotal|MemAvailable|MemFree|Buffers|Cached|SwapTotal|SwapFree):' /proc/meminfo
cut -d' ' -f1-3 /proc/loadavg | sed 's/^/load /'
awk 'NR>2{{sub(":"," ");if($1!~/^(lo|veth|docker|br-|virbr|cni|flannel|cali|tun|tap)/){{rx+=$2;tx+=$10}}}}END{{print "net",rx+0,tx+0}}' /proc/net/dev 2>/dev/null
if [ $((n % 10)) -eq 0 ]; then df -Pk 2>/dev/null | awk 'NR>1 && $1 ~ /^\//{{print "disk",$2,$3,$6}}'; fi
echo '@@e'
n=$((n+1))
sleep {interval}
done"#
    ))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Disk {
    pub mount: String,
    pub total_kb: u64,
    pub used_kb: u64,
}

impl Disk {
    pub fn pct(&self) -> f32 {
        if self.total_kb == 0 {
            0.0
        } else {
            (self.used_kb as f64 * 100.0 / self.total_kb as f64) as f32
        }
    }
}

/// One parsed block of raw counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    /// Seconds since boot of the host (the clock for rates).
    pub up: f64,
    /// `(idle, total)` jiffies.
    pub cpu: Option<(u64, u64)>,
    pub mem_total_kb: u64,
    pub mem_avail_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub load: [f32; 3],
    /// `(rx, tx)` bytes of the real interfaces.
    pub net: Option<(u64, u64)>,
    /// Only every few blocks.
    pub disks: Option<Vec<Disk>>,
}

/// Parses the lines between `@@s` and `@@e`.
pub fn parse_block(text: &str) -> Option<Raw> {
    let mut r = Raw::default();
    let (mut free, mut buffers, mut cached, mut avail) = (0u64, 0u64, 0u64, None);
    let mut disks: Vec<Disk> = Vec::new();
    let mut have_disks = false;
    let mut seen = 0;
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        let nums = || line.split_whitespace().skip(1);
        match key {
            "up" => {
                r.up = it.next()?.parse().ok()?;
                seen += 1;
            }
            "cpu" => {
                let v: Vec<u64> = nums().filter_map(|x| x.parse().ok()).collect();
                if v.len() >= 4 {
                    // guest time is already part of user/nice.
                    let total: u64 = v.iter().take(8).sum();
                    let idle = v[3] + v.get(4).copied().unwrap_or(0);
                    r.cpu = Some((idle, total));
                    seen += 1;
                }
            }
            "MemTotal:" => r.mem_total_kb = it.next()?.parse().ok()?,
            "MemAvailable:" => avail = it.next()?.parse().ok(),
            "MemFree:" => free = it.next()?.parse().ok()?,
            "Buffers:" => buffers = it.next()?.parse().ok()?,
            "Cached:" => cached = it.next()?.parse().ok()?,
            "SwapTotal:" => r.swap_total_kb = it.next()?.parse().ok()?,
            "SwapFree:" => r.swap_free_kb = it.next()?.parse().ok()?,
            "load" => {
                for (i, x) in it.take(3).enumerate() {
                    r.load[i] = x.parse().unwrap_or(0.0);
                }
            }
            "net" => {
                let v: Vec<u64> = nums().filter_map(|x| x.parse().ok()).collect();
                if v.len() == 2 {
                    r.net = Some((v[0], v[1]));
                }
            }
            "disk" => {
                have_disks = true;
                let total: u64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                let used: u64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                let mount: String = it.collect::<Vec<_>>().join(" ");
                // Bind mounts show the same numbers more than once.
                if total > 0
                    && !mount.is_empty()
                    && !disks
                        .iter()
                        .any(|d| d.total_kb == total && d.used_kb == used)
                {
                    disks.push(Disk {
                        mount: mount.chars().take(60).collect(),
                        total_kb: total,
                        used_kb: used,
                    });
                }
            }
            _ => {}
        }
    }
    if seen < 2 || r.mem_total_kb == 0 {
        return None;
    }
    r.mem_avail_kb = avail.unwrap_or(free + buffers + cached);
    if have_disks {
        disks.truncate(12);
        r.disks = Some(disks);
    }
    Some(r)
}

/// One point of the history (rates already computed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Host uptime in seconds when taken.
    pub t: f64,
    pub cpu: f32,
    pub mem: f32,
    pub swap: f32,
    pub load1: f32,
    /// Bytes per second.
    pub rx: f64,
    pub tx: f64,
}

/// Bounded history of one host.
#[derive(Clone, Debug)]
pub struct History {
    pub points: VecDeque<Point>,
    cap: usize,
    last: Option<Raw>,
    pub mem_total_kb: u64,
    pub swap_total_kb: u64,
    pub load: [f32; 3],
    pub disks: Vec<Disk>,
    /// Whether the host reports network counters at all.
    pub has_net: bool,
}

impl History {
    pub fn new(interval: u32) -> Self {
        let cap = (WINDOW_SECS / interval.max(1)) as usize + 2;
        Self {
            points: VecDeque::with_capacity(cap),
            cap,
            last: None,
            mem_total_kb: 0,
            swap_total_kb: 0,
            load: [0.0; 3],
            disks: Vec::new(),
            has_net: false,
        }
    }

    pub fn latest(&self) -> Option<&Point> {
        self.points.back()
    }

    /// Adds a block; the first one only sets the baseline for the counters.
    pub fn push(&mut self, raw: Raw) {
        self.mem_total_kb = raw.mem_total_kb;
        self.swap_total_kb = raw.swap_total_kb;
        self.load = raw.load;
        if let Some(d) = &raw.disks {
            self.disks = d.clone();
        }
        self.has_net |= raw.net.is_some();
        let mem = if raw.mem_total_kb > 0 {
            (raw.mem_total_kb.saturating_sub(raw.mem_avail_kb) as f64 * 100.0
                / raw.mem_total_kb as f64) as f32
        } else {
            0.0
        };
        let swap = if raw.swap_total_kb > 0 {
            (raw.swap_total_kb.saturating_sub(raw.swap_free_kb) as f64 * 100.0
                / raw.swap_total_kb as f64) as f32
        } else {
            0.0
        };
        if let Some(prev) = &self.last {
            let dt = raw.up - prev.up;
            // A reboot or a stale block: start over.
            if dt > 0.0 && dt < 120.0 {
                let cpu = match (prev.cpu, raw.cpu) {
                    (Some((pi, pt)), Some((i, t))) if t > pt && i >= pi => {
                        (100.0 - (i - pi) as f64 * 100.0 / (t - pt) as f64).clamp(0.0, 100.0) as f32
                    }
                    _ => self.latest().map_or(0.0, |p| p.cpu),
                };
                let (rx, tx) = match (prev.net, raw.net) {
                    (Some((pr, pt)), Some((r, t))) if r >= pr && t >= pt => {
                        ((r - pr) as f64 / dt, (t - pt) as f64 / dt)
                    }
                    _ => (0.0, 0.0),
                };
                if self.points.len() >= self.cap {
                    self.points.pop_front();
                }
                self.points.push_back(Point {
                    t: raw.up,
                    cpu,
                    mem,
                    swap,
                    load1: raw.load[0],
                    rx,
                    tx,
                });
            } else if dt < 0.0 {
                self.points.clear();
            }
        }
        self.last = Some(raw);
    }

    pub fn series(&self, f: impl Fn(&Point) -> f32) -> Vec<f32> {
        self.points.iter().map(f).collect()
    }

    /// Seconds the history spans.
    #[cfg(test)]
    pub fn span(&self) -> f64 {
        match (self.points.front(), self.points.back()) {
            (Some(a), Some(b)) => b.t - a.t,
            _ => 0.0,
        }
    }
}

// ------------------------------------------------------------------ the sampler task

pub enum Kind {
    Connected,
    Sample(Box<Raw>),
    /// No `/proc` (or no POSIX shell): the reason.
    Unsupported(String),
    /// The sampler ended; may be retried.
    Ended(String),
}

pub struct Event {
    pub host: Uuid,
    /// Generation of the watcher that sent it (stale events are dropped).
    pub gen_id: u64,
    pub kind: Kind,
}

/// What one sampler watches.
pub struct Task {
    pub host: Uuid,
    pub gen_id: u64,
    pub params: SshParams,
    pub interval: u32,
}

/// Starts the sampler for a host; it ends when `stop` becomes `true` (or the receiver
/// side is dropped), when the connection breaks or when the host is unsupported.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    task: Task,
    sink: Sink<Event>,
    prompts: Sink<HostKeyPrompt>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let Task {
        host,
        gen_id,
        params,
        interval,
    } = task;
    rt.spawn(async move {
        let send = |kind| sink.send(Event { host, gen_id, kind });
        let run = async {
            let handle = connect(&params, prompts)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let mut ch = handle
                .channel_open_session()
                .await
                .map_err(|e| e.to_string())?;
            ch.exec(true, script(interval))
                .await
                .map_err(|e| e.to_string())?;
            let _ = ch.eof().await;
            send(Kind::Connected);
            let (mut buf, mut block, mut in_block) = (String::new(), String::new(), false);
            let (mut samples, mut stderr) = (0u32, String::new());
            let mut result: Result<(), String> = Ok(());
            loop {
                let msg = tokio::select! {
                    m = ch.wait() => m,
                    _ = stop.wait_for(|s| *s) => break,
                };
                let Some(msg) = msg else { break };
                match msg {
                    ChannelMsg::Data { data } => {
                        buf.push_str(&String::from_utf8_lossy(&data));
                        // A runaway line (not our script) must not grow without bound.
                        if buf.len() > 64 * 1024 {
                            result = Err("unexpected output".into());
                            break;
                        }
                        while let Some(p) = buf.find('\n') {
                            let line: String = buf.drain(..=p).collect();
                            let line = line.trim_end();
                            match line {
                                "@@unsupported" => {
                                    send(Kind::Unsupported(
                                        t!("Kein /proc auf diesem Host (nur Linux)").to_string(),
                                    ));
                                    let _ = handle
                                        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                                        .await;
                                    return Ok(());
                                }
                                "@@s" => {
                                    block.clear();
                                    in_block = true;
                                }
                                "@@e" => {
                                    in_block = false;
                                    if let Some(raw) = parse_block(&block) {
                                        samples += 1;
                                        send(Kind::Sample(Box::new(raw)));
                                    }
                                }
                                l if in_block && block.len() < 16 * 1024 => {
                                    block.push_str(l);
                                    block.push('\n');
                                }
                                _ => {}
                            }
                        }
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        if stderr.len() < 512 {
                            stderr.push_str(&String::from_utf8_lossy(&data));
                        }
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
            if *stop.borrow() {
                return Ok(());
            }
            if samples == 0 {
                // Closed without a single block: no POSIX shell here.
                let why = stderr.lines().next().unwrap_or("").trim().to_string();
                send(Kind::Unsupported(if why.is_empty() {
                    t!("Der Host liefert keine Messwerte (keine POSIX-Shell?)").to_string()
                } else {
                    why
                }));
                return Ok(());
            }
            result
        };
        let r: Result<(), String> = run.await;
        if let Err(e) = r {
            send(Kind::Ended(e));
        } else if !*stop.borrow() {
            send(Kind::Ended(t!("Verbindung beendet").to_string()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "up 1000.50\ncpu  4705 150 1120 16250 520 0 30 0 0 0\nMemTotal:       16000000 kB\nMemFree:         2000000 kB\nMemAvailable:    8000000 kB\nBuffers:          500000 kB\nCached:          3000000 kB\nSwapTotal:       2000000 kB\nSwapFree:        1500000 kB\nload 0.52 0.40 0.30\nnet 1000000 500000\ndisk 100000000 70000000 /\ndisk 100000000 70000000 /bind\ndisk 5000000 1000000 /boot\n";

    #[test]
    fn script_is_one_safe_argument() {
        let s = script(3);
        assert!(s.starts_with("sh -c '"));
        assert!(s.contains("sleep 3"));
        // Out-of-range intervals are clamped, never put in as typed.
        assert!(script(0).contains("sleep 2") && script(99999).contains("sleep 10"));
        // The wrapper quotes every inner quote.
        assert!(!s[7..s.len() - 1].replace("'\\''", "").contains('\''));
    }

    #[test]
    fn parses_a_block() {
        let r = parse_block(BLOCK).unwrap();
        assert_eq!(r.up, 1000.5);
        // idle + iowait, and the sum of the first eight columns.
        assert_eq!(r.cpu, Some((16770, 22775)));
        assert_eq!(r.mem_total_kb, 16_000_000);
        assert_eq!(r.mem_avail_kb, 8_000_000);
        assert_eq!(r.swap_total_kb, 2_000_000);
        assert_eq!(r.load, [0.52, 0.40, 0.30]);
        assert_eq!(r.net, Some((1_000_000, 500_000)));
        let d = r.disks.unwrap();
        assert_eq!(d.len(), 2, "the bind mount is the same file system");
        assert_eq!(d[0].mount, "/");
        assert!((d[0].pct() - 70.0).abs() < 0.01);
    }

    #[test]
    fn old_kernels_without_memavailable() {
        let text = BLOCK.replace("MemAvailable:    8000000 kB\n", "");
        let r = parse_block(&text).unwrap();
        assert_eq!(r.mem_avail_kb, 2_000_000 + 500_000 + 3_000_000);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_block("").is_none());
        assert!(parse_block("hello\nworld\n").is_none());
        assert!(parse_block("up 5\n").is_none());
        assert!(parse_block("up x\ncpu 1 2 3 4\nMemTotal: 5 kB\n").is_none());
    }

    fn raw(up: f64, idle: u64, total: u64, rx: u64, tx: u64) -> Raw {
        Raw {
            up,
            cpu: Some((idle, total)),
            mem_total_kb: 1000,
            mem_avail_kb: 250,
            net: Some((rx, tx)),
            ..Default::default()
        }
    }

    #[test]
    fn rates_from_counter_deltas() {
        let mut h = History::new(3);
        h.push(raw(100.0, 1000, 2000, 0, 0));
        assert!(h.points.is_empty(), "the first block is only the baseline");
        h.push(raw(103.0, 1750, 3000, 3000, 6000));
        let p = h.latest().unwrap();
        // 750 of 1000 jiffies idle -> 25 % busy.
        assert!((p.cpu - 25.0).abs() < 0.01, "{}", p.cpu);
        assert!((p.mem - 75.0).abs() < 0.01);
        assert!((p.rx - 1000.0).abs() < 0.01 && (p.tx - 2000.0).abs() < 0.01);
        assert!(h.has_net);
        assert!((h.span() - 3.0).abs() < 1e-9 || h.span() == 0.0);
    }

    #[test]
    fn counter_resets_and_reboots() {
        let mut h = History::new(3);
        h.push(raw(100.0, 1000, 2000, 5000, 5000));
        h.push(raw(103.0, 1100, 2100, 8000, 8000));
        assert_eq!(h.points.len(), 1);
        // Counters went backwards (interface re-created): rate 0, cpu from the new deltas.
        h.push(raw(106.0, 1200, 2200, 10, 10));
        let p = h.latest().unwrap();
        assert_eq!((p.rx, p.tx), (0.0, 0.0));
        // The host rebooted: uptime restarts, the history is dropped.
        h.push(raw(5.0, 10, 20, 0, 0));
        assert!(h.points.is_empty());
        h.push(raw(8.0, 20, 40, 100, 100));
        assert_eq!(h.points.len(), 1);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new(3);
        let mut up = 0.0;
        for i in 0..1000u64 {
            up += 3.0;
            h.push(raw(up, i * 10, i * 20, i, i));
        }
        assert_eq!(h.points.len(), (WINDOW_SECS / 3) as usize + 2);
        assert!(h.span() <= WINDOW_SECS as f64 + 6.0);
        assert_eq!(h.series(|p| p.cpu).len(), h.points.len());
    }

    #[test]
    fn fully_idle_or_busy_stays_in_range() {
        let mut h = History::new(3);
        h.push(raw(0.0, 0, 0, 0, 0));
        h.push(raw(3.0, 500, 500, 0, 0));
        assert_eq!(h.latest().unwrap().cpu, 0.0);
        h.push(raw(6.0, 500, 1000, 0, 0));
        assert_eq!(h.latest().unwrap().cpu, 100.0);
    }
}
