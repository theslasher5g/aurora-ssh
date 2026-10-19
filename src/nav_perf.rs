//! Optional frame timing for large libraries: `AURORA_PERF=1` prints, every two seconds,
//! how long the app's `ui()` call took (average / 95th percentile / worst, in ms) to stderr.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Stats {
    samples: Vec<f32>,
    since: Instant,
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("AURORA_PERF").is_some())
}

fn stats() -> &'static Mutex<Stats> {
    static S: OnceLock<Mutex<Stats>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Stats {
            samples: Vec::new(),
            since: Instant::now(),
        })
    })
}

/// Measures from creation to drop; a no-op unless `AURORA_PERF` is set.
pub struct Frame(Option<Instant>);

pub fn frame() -> Frame {
    Frame(enabled().then(Instant::now))
}

impl Drop for Frame {
    fn drop(&mut self) {
        let Some(start) = self.0 else { return };
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        let Ok(mut s) = stats().lock() else { return };
        s.samples.push(ms);
        if s.since.elapsed() >= Duration::from_secs(2) {
            if let Some(line) = summary(&mut s.samples) {
                eprintln!("perf: {line}");
            }
            s.samples.clear();
            s.since = Instant::now();
        }
    }
}

/// "frames=N avg=… p95=… max=…" of the collected samples.
pub fn summary(samples: &mut [f32]) -> Option<String> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len();
    let avg = samples.iter().sum::<f32>() / n as f32;
    let p95 = samples[((n as f32 * 0.95) as usize).min(n - 1)];
    Some(format!(
        "frames={n} avg={avg:.2}ms p95={p95:.2}ms max={:.2}ms",
        samples[n - 1]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_of_samples() {
        assert!(summary(&mut []).is_none());
        let mut v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let s = summary(&mut v).unwrap();
        assert!(s.contains("frames=100"));
        assert!(s.contains("max=100.00ms"));
        assert!(s.contains("p95=96.00ms"));
    }
}
