//! Local port forwarding ("tunnel"): listens on 127.0.0.1:<local_port> and forwards every
//! connection through the SSH server to <remote_host>:<remote_port>.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::ssh::{SshParams, connect};
use super::{HostKeyPrompt, Sink};
use crate::model::Tunnel;

pub enum TunnelEvent {
    Up(Uuid),
    /// Number of open forwarded connections changed by +1 / -1.
    Conns(Uuid, i32),
    Down(Uuid, Option<String>),
}

/// Starts the tunnel; dropping the returned sender stops it.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    params: SshParams,
    tunnel: Tunnel,
    events: Sink<TunnelEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> mpsc::UnboundedSender<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(async move {
        let id = tunnel.id;
        let r = run(params, tunnel, rx, &events, prompts).await;
        events.send(TunnelEvent::Down(id, r.err().map(|e| format!("{e:#}"))));
    });
    tx
}

async fn run(
    params: SshParams,
    t: Tunnel,
    mut stop: mpsc::UnboundedReceiver<()>,
    events: &Sink<TunnelEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> Result<()> {
    // Bind first: a taken port is the most common problem and needs no login to report.
    let listener = TcpListener::bind(("127.0.0.1", t.local_port))
        .await
        .map_err(|e| anyhow!("{}", t!("Port {} ist schon belegt ({})", t.local_port, e)))?;
    let handle = Arc::new(connect(&params, prompts).await?);
    events.send(TunnelEvent::Up(t.id));
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = stop.recv() => break,
            _ = tick.tick() => {
                if handle.is_closed() {
                    return Err(anyhow!(t!("SSH-Verbindung getrennt")));
                }
            }
            acc = listener.accept() => {
                let (mut sock, peer) = acc?;
                let (handle, events) = (handle.clone(), events.clone());
                let (host, port, id) = (t.remote_host.clone(), t.remote_port, t.id);
                tokio::spawn(async move {
                    match handle
                        .channel_open_direct_tcpip(host, port as u32, "127.0.0.1", peer.port() as u32)
                        .await
                    {
                        Ok(ch) => {
                            events.send(TunnelEvent::Conns(id, 1));
                            let mut stream = ch.into_stream();
                            let _ = tokio::io::copy_bidirectional(&mut sock, &mut stream).await;
                            events.send(TunnelEvent::Conns(id, -1));
                        }
                        Err(e) => log::warn!("tunnel channel: {e}"),
                    }
                });
            }
        }
    }
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await;
    Ok(())
}
