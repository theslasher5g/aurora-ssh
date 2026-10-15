//! Reachability probe for the host list: a plain TCP connect to each SSH/RDP host
//! (nothing is sent), repeated every minute while the list is visible.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::model::{Host, Protocol};

const INTERVAL: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Reachable, connect time in milliseconds.
    Up(u32),
    Down,
}

#[derive(Default)]
pub struct Reachability {
    results: Arc<Mutex<HashMap<Uuid, Reach>>>,
    running: Arc<AtomicBool>,
    last: Option<Instant>,
}

impl Reachability {
    pub fn get(&self, id: Uuid) -> Option<Reach> {
        self.results.lock().ok()?.get(&id).copied()
    }

    /// Starts a probe round if the last one is older than a minute.
    pub fn tick(&mut self, rt: &tokio::runtime::Runtime, hosts: &[Host], ctx: &egui::Context) {
        if self.running.load(Ordering::Relaxed) || self.last.is_some_and(|t| t.elapsed() < INTERVAL)
        {
            return;
        }
        self.last = Some(Instant::now());
        self.running.store(true, Ordering::Relaxed);
        let targets: Vec<(Uuid, String, u16)> = hosts
            .iter()
            .filter(|h| matches!(h.protocol, Protocol::Ssh | Protocol::Rdp))
            .filter(|h| !h.hostname.trim().is_empty())
            .map(|h| (h.id, h.hostname.trim().to_string(), h.port))
            .collect();
        let (results, running, ctx) = (self.results.clone(), self.running.clone(), ctx.clone());
        rt.spawn(async move {
            let probes = targets.into_iter().map(|(id, host, port)| async move {
                let start = Instant::now();
                let ok = tokio::time::timeout(
                    TIMEOUT,
                    tokio::net::TcpStream::connect((host.as_str(), port)),
                )
                .await
                .is_ok_and(|r| r.is_ok());
                let reach = if ok {
                    Reach::Up(start.elapsed().as_millis().min(u32::MAX as u128) as u32)
                } else {
                    Reach::Down
                };
                (id, reach)
            });
            let all = futures_join(probes).await;
            if let Ok(mut map) = results.lock() {
                map.clear();
                map.extend(all);
            }
            running.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
}

/// Runs all probes concurrently (spawned tasks, so slow hosts don't block each other).
async fn futures_join<F>(futs: impl Iterator<Item = F>) -> Vec<(Uuid, Reach)>
where
    F: std::future::Future<Output = (Uuid, Reach)> + Send + 'static,
{
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(r) = h.await {
            out.push(r);
        }
    }
    out
}
