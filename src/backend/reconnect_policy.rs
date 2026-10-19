//! Automatic reconnect: which session endings count as "unexpected", the exponential
//! backoff and the detection of a sleeping laptop. Pure logic with an injected clock
//! (seconds as `f64`) so it can be tested without waiting.

use crate::model::ReconnectMode;

/// Waits before attempt 1, 2, 3 … (seconds); every further attempt waits the last value.
pub const BACKOFF_SECS: [u64; 6] = [1, 2, 4, 8, 15, 30];
/// A wall clock that runs this much faster than the monotonic clock means the machine slept.
const SLEEP_DRIFT_SECS: f64 = 15.0;
/// The UI ticks at least every few seconds; a longer gap means the process was frozen.
const SLEEP_GAP_SECS: f64 = 45.0;

/// Seconds to wait before attempt `attempt` (1-based).
pub fn wait_before(attempt: u32) -> u64 {
    let i = (attempt.max(1) as usize - 1).min(BACKOFF_SECS.len() - 1);
    BACKOFF_SECS[i]
}

/// Is the global switch (and the host's override) on?
pub fn enabled(global: bool, host: ReconnectMode) -> bool {
    match host {
        ReconnectMode::Default => global,
        ReconnectMode::On => true,
        ReconnectMode::Off => false,
    }
}

/// What kind of session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    /// Interactive SSH shell (not a one-shot command): the backend tells if the link dropped.
    Ssh,
    /// Telnet: only a read/write error counts as a drop (a plain EOF can be a logout).
    Telnet,
    /// Serial, local, replay, Proxmox consoles, SSH commands …: never reconnected automatically.
    Other,
}

/// Facts about a session that just ended.
#[derive(Clone, Copy, Debug)]
pub struct Ended {
    pub proto: Proto,
    /// The session was up before it ended.
    pub was_connected: bool,
    /// An automatic attempt was running (so `was_connected` is false for a failed attempt).
    pub retrying: bool,
    /// The backend reported a lost connection (read error, keepalive timeout, reset).
    pub dropped: bool,
    /// The session ended with an error text.
    pub had_error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Start (or continue) automatic reconnecting.
    Retry,
    /// The user or the server ended the session on purpose.
    Clean,
    /// Never connected: authentication, host key or a typo – retrying would not help.
    NeverConnected,
    /// An attempt failed for a reason retrying does not fix (login, host key).
    Fatal,
    /// Automatic reconnect is off, or not supported for this session type.
    Off,
}

/// Decides whether an ended session is reconnected.
pub fn decide(on: bool, e: Ended) -> Verdict {
    if !on || e.proto == Proto::Other {
        return Verdict::Off;
    }
    let lost = match e.proto {
        Proto::Ssh => e.dropped,
        _ => e.had_error,
    };
    if e.retrying {
        return if lost { Verdict::Retry } else { Verdict::Fatal };
    }
    if !e.was_connected {
        return Verdict::NeverConnected;
    }
    if lost { Verdict::Retry } else { Verdict::Clean }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Waiting { until: f64 },
    Trying,
}

/// What to do now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Poll {
    /// Seconds until the next attempt.
    Wait(f64),
    /// Start an attempt now.
    Attempt,
    /// The time budget is used up.
    GiveUp,
    /// An attempt is running.
    Busy,
}

/// Backoff state of one session.
#[derive(Clone, Debug)]
pub struct Retry {
    lost_at: f64,
    /// 1-based number of the next (or running) attempt.
    attempt: u32,
    phase: Phase,
    max_total: f64,
}

impl Retry {
    pub fn new(now: f64, max_minutes: u32) -> Self {
        Self {
            lost_at: now,
            attempt: 1,
            phase: Phase::Waiting {
                until: now + wait_before(1) as f64,
            },
            max_total: max_minutes.max(1) as f64 * 60.0,
        }
    }

    /// Number of the attempt that is waiting or running.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self.phase, Phase::Waiting { .. })
    }

    /// Seconds until the next attempt while waiting.
    pub fn remaining(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Waiting { until } => (until - now).max(0.0),
            Phase::Trying => 0.0,
        }
    }

    /// Seconds since the connection was lost.
    pub fn down_for(&self, now: f64) -> f64 {
        (now - self.lost_at).max(0.0)
    }

    pub fn poll(&mut self, now: f64) -> Poll {
        match self.phase {
            Phase::Trying => Poll::Busy,
            Phase::Waiting { until } => {
                if now - self.lost_at >= self.max_total {
                    Poll::GiveUp
                } else if now >= until {
                    self.phase = Phase::Trying;
                    Poll::Attempt
                } else {
                    Poll::Wait(until - now)
                }
            }
        }
    }

    /// The running attempt failed with a retryable error: schedule the next one.
    pub fn attempt_failed(&mut self, now: f64) {
        self.attempt += 1;
        self.phase = Phase::Waiting {
            until: now + wait_before(self.attempt) as f64,
        };
    }

    /// "Jetzt verbinden": skip the rest of the wait.
    pub fn retry_now(&mut self, now: f64) {
        if self.is_waiting() {
            self.phase = Phase::Waiting { until: now };
        }
    }

    /// The machine woke up or the network came back: try at once with a fresh time budget.
    pub fn wake(&mut self, now: f64) {
        self.lost_at = now;
        self.retry_now(now);
    }
}

/// Notices that the machine slept (or the process was frozen) between two UI ticks.
#[derive(Clone, Copy, Debug)]
pub struct SleepDetector {
    last_mono: f64,
    last_wall: f64,
}

impl SleepDetector {
    pub fn new(mono: f64, wall: f64) -> Self {
        Self {
            last_mono: mono,
            last_wall: wall,
        }
    }

    /// `mono`: monotonic seconds, `wall`: wall clock seconds. True after a wake-up.
    pub fn tick(&mut self, mono: f64, wall: f64) -> bool {
        let d_mono = mono - self.last_mono;
        let d_wall = wall - self.last_wall;
        self.last_mono = mono;
        self.last_wall = wall;
        d_wall - d_mono > SLEEP_DRIFT_SECS || d_mono > SLEEP_GAP_SECS
    }
}

/// Banner text: "Verbindung verloren – neuer Versuch in 4 s (Versuch 3)".
pub fn banner_text(r: &Retry, remaining: f64) -> String {
    if r.is_waiting() {
        t!(
            "Verbindung verloren – neuer Versuch in {} s (Versuch {})",
            remaining.ceil().max(1.0) as u64,
            r.attempt()
        )
        .to_string()
    } else {
        t!(
            "Verbindung verloren – verbinde neu… (Versuch {})",
            r.attempt()
        )
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(proto: Proto) -> Ended {
        Ended {
            proto,
            was_connected: true,
            retrying: false,
            dropped: false,
            had_error: false,
        }
    }

    #[test]
    fn backoff_sequence() {
        let w: Vec<u64> = (1..=9).map(wait_before).collect();
        assert_eq!(w, [1, 2, 4, 8, 15, 30, 30, 30, 30]);
        assert_eq!(wait_before(0), 1);
    }

    #[test]
    fn switch_and_override() {
        assert!(enabled(true, ReconnectMode::Default));
        assert!(!enabled(false, ReconnectMode::Default));
        assert!(enabled(false, ReconnectMode::On));
        assert!(!enabled(true, ReconnectMode::Off));
    }

    #[test]
    fn unexpected_ssh_drop_retries() {
        let e = Ended {
            dropped: true,
            had_error: true,
            ..ended(Proto::Ssh)
        };
        assert_eq!(decide(true, e), Verdict::Retry);
    }

    #[test]
    fn clean_exit_does_not_retry() {
        assert_eq!(decide(true, ended(Proto::Ssh)), Verdict::Clean);
        // A non-zero exit of the remote shell is an ordinary end too.
        let e = Ended {
            had_error: true,
            ..ended(Proto::Ssh)
        };
        assert_eq!(decide(true, e), Verdict::Clean);
    }

    #[test]
    fn never_connected_does_not_retry() {
        let e = Ended {
            was_connected: false,
            dropped: true,
            ..ended(Proto::Ssh)
        };
        assert_eq!(decide(true, e), Verdict::NeverConnected);
    }

    #[test]
    fn failed_attempt_distinguishes_network_from_login() {
        let net = Ended {
            was_connected: false,
            retrying: true,
            dropped: true,
            had_error: true,
            ..ended(Proto::Ssh)
        };
        assert_eq!(decide(true, net), Verdict::Retry);
        let auth = Ended {
            dropped: false,
            ..net
        };
        assert_eq!(decide(true, auth), Verdict::Fatal);
    }

    #[test]
    fn off_and_unsupported() {
        let e = Ended {
            dropped: true,
            ..ended(Proto::Ssh)
        };
        assert_eq!(decide(false, e), Verdict::Off);
        assert_eq!(decide(true, ended(Proto::Other)), Verdict::Off);
    }

    #[test]
    fn telnet_only_errors_count() {
        assert_eq!(decide(true, ended(Proto::Telnet)), Verdict::Clean);
        let e = Ended {
            had_error: true,
            ..ended(Proto::Telnet)
        };
        assert_eq!(decide(true, e), Verdict::Retry);
    }

    #[test]
    fn state_machine_walks_the_backoff() {
        let mut r = Retry::new(100.0, 10);
        assert_eq!(r.attempt(), 1);
        assert!(matches!(r.poll(100.2), Poll::Wait(w) if (w - 0.8).abs() < 1e-9));
        assert_eq!(r.poll(101.0), Poll::Attempt);
        assert_eq!(r.poll(101.5), Poll::Busy);
        r.attempt_failed(102.0);
        assert_eq!(r.attempt(), 2);
        assert!(matches!(r.poll(102.0), Poll::Wait(w) if (w - 2.0).abs() < 1e-9));
        assert_eq!(r.poll(104.0), Poll::Attempt);
        r.attempt_failed(104.0);
        r.attempt_failed(104.0);
        r.attempt_failed(104.0);
        r.attempt_failed(104.0);
        r.attempt_failed(104.0);
        assert_eq!(r.attempt(), 7);
        assert!(matches!(r.poll(104.0), Poll::Wait(w) if (w - 30.0).abs() < 1e-9));
    }

    #[test]
    fn gives_up_after_the_budget() {
        let mut r = Retry::new(0.0, 1);
        r.attempt_failed(59.0);
        assert!(matches!(r.poll(59.5), Poll::Wait(_)));
        assert_eq!(r.poll(60.0), Poll::GiveUp);
        assert_eq!(r.down_for(75.0), 75.0);
    }

    #[test]
    fn retry_now_and_wake() {
        let mut r = Retry::new(0.0, 10);
        r.attempt_failed(1.0);
        r.attempt_failed(3.0);
        r.attempt_failed(7.0);
        assert!(matches!(r.poll(7.5), Poll::Wait(_)));
        r.retry_now(7.5);
        assert_eq!(r.poll(7.5), Poll::Attempt);
        // While an attempt runs, "now" does nothing.
        r.retry_now(8.0);
        assert_eq!(r.poll(8.0), Poll::Busy);
        // Waking up after a long sleep: no give-up, immediate attempt.
        let mut r = Retry::new(0.0, 1);
        r.attempt_failed(30.0);
        r.wake(7200.0);
        assert_eq!(r.poll(7200.0), Poll::Attempt);
    }

    #[test]
    fn sleep_detection() {
        let mut d = SleepDetector::new(0.0, 1000.0);
        assert!(!d.tick(2.0, 1002.0));
        assert!(!d.tick(5.0, 1005.1));
        // Wall clock jumped 600 s while the monotonic clock stood still (Linux suspend).
        assert!(d.tick(7.0, 1607.0));
        // A frozen process: monotonic time jumped too.
        assert!(d.tick(80.0, 1680.0));
        assert!(!d.tick(82.0, 1682.0));
    }

    #[test]
    fn banner_mentions_seconds_and_attempt() {
        let mut r = Retry::new(0.0, 10);
        r.attempt_failed(0.0);
        r.attempt_failed(0.0);
        let s = banner_text(&r, 3.2);
        assert!(s.contains("4 s") && s.contains('3'), "{s}");
    }
}
