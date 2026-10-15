//! Connection backends. Each session runs on the tokio runtime (or a worker thread
//! for serial) and talks to the UI exclusively through channels.

pub mod rdp;
pub mod serial;
pub mod sftp;
pub mod ssh;

use tokio::sync::{mpsc, oneshot};

/// Commands from the UI to an interactive terminal session.
#[derive(Debug)]
pub enum TermCmd {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}

/// Events from an interactive terminal session to the UI.
#[derive(Debug)]
pub enum TermEvent {
    Status(String),
    Connected,
    Output(Vec<u8>),
    Closed(Option<String>),
}

/// Server key verification request; answered by the user in a dialog.
pub struct HostKeyPrompt {
    /// `host:port`
    pub host: String,
    pub algorithm: String,
    pub fingerprint: String,
    /// Fingerprint stored earlier, if it differs (possible MITM).
    pub previous: Option<String>,
    pub reply: oneshot::Sender<bool>,
}

/// Sends events to the UI and wakes it up.
pub struct Sink<T> {
    tx: mpsc::UnboundedSender<T>,
    ctx: egui::Context,
}

impl<T> Clone for Sink<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            ctx: self.ctx.clone(),
        }
    }
}

impl<T> Sink<T> {
    pub fn new(ctx: egui::Context) -> (Self, mpsc::UnboundedReceiver<T>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, ctx }, rx)
    }

    pub fn send(&self, ev: T) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}
