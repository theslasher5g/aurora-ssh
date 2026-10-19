//! Reachability probe for the host list: a plain TCP connect to each SSH/RDP host
//! (nothing is sent), repeated every minute while the list is visible.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::model::Host;

const INTERVAL: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(3);
/// Probes in flight at the same time (20 000 hosts must not open 20 000 sockets).
const PARALLEL: usize = 128;

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
    /// Counts every stored result, so views can tell when something changed.
    rev: Arc<AtomicU64>,
}

impl Reachability {
    pub fn get(&self, id: Uuid) -> Option<Reach> {
        self.results.lock().ok()?.get(&id).copied()
    }

    /// Changes whenever a probe result arrives.
    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::Relaxed)
    }

    /// Copy of all results (one lock instead of one per host).
    pub fn all(&self) -> HashMap<Uuid, Reach> {
        self.results.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// Starts a probe round if the last one is older than a minute.
    pub fn tick(&mut self, rt: &tokio::runtime::Runtime, hosts: &[Host], ctx: &egui::Context) {
        self.tick_ordered(rt, hosts, &Default::default(), ctx);
    }

    /// Like [`tick`](Self::tick); hosts in `first` are probed before the rest.
    pub fn tick_ordered(
        &mut self,
        rt: &tokio::runtime::Runtime,
        hosts: &[Host],
        first: &std::collections::HashSet<Uuid>,
        ctx: &egui::Context,
    ) {
        if self.running.load(Ordering::Relaxed) || self.last.is_some_and(|t| t.elapsed() < INTERVAL)
        {
            return;
        }
        self.last = Some(Instant::now());
        self.running.store(true, Ordering::Relaxed);
        let mut targets: Vec<(bool, Uuid, String, u16)> = hosts
            .iter()
            .filter(|h| h.protocol.is_network())
            .filter(|h| !h.hostname.trim().is_empty())
            .map(|h| {
                (
                    !first.contains(&h.id),
                    h.id,
                    h.hostname.trim().to_string(),
                    h.port,
                )
            })
            .collect();
        targets.sort_by_key(|t| t.0);
        let (results, running, rev, ctx) = (
            self.results.clone(),
            self.running.clone(),
            self.rev.clone(),
            ctx.clone(),
        );
        rt.spawn(async move {
            let gate = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
            let probes = targets.into_iter().map(|(_, id, host, port)| {
                let (gate, results, rev, ctx) =
                    (gate.clone(), results.clone(), rev.clone(), ctx.clone());
                async move {
                    let _permit = gate.acquire().await.ok();
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
                    if let Ok(mut map) = results.lock() {
                        map.insert(id, reach);
                    }
                    // Repaint every few results, not 20 000 times in a row.
                    if rev.fetch_add(1, Ordering::Relaxed) % 64 == 0 {
                        ctx.request_repaint();
                    }
                    (id, reach)
                }
            });
            let ctx2 = ctx.clone();
            futures_join(probes).await;
            running.store(false, Ordering::Relaxed);
            ctx2.request_repaint();
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
