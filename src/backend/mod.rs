//! Connection backends. Each session runs on the tokio runtime (or a worker thread
//! for serial) and talks to the UI exclusively through channels.

pub mod certradar;
pub mod container;
pub mod containers_parse;
pub mod containers_run;
pub mod diag;
pub mod hostkey_check;
pub mod inventory_collect;
pub mod inventory_detail;
pub mod inventory_doc;
pub mod inventory_export;
pub mod inventory_info;
pub mod inventory_refresh;
pub mod inventory_rules;
pub mod local;
pub mod logs;
pub mod portscan;
pub mod proxmox;
pub mod pwmanager_cli;
pub mod pwmanager_proton;
pub mod pwmanager_run;
#[cfg(all(test, unix))]
mod pwmanager_tests;
pub mod pwmanager_types;
pub mod rdp;
pub mod rdp_embed;
pub mod reach;
pub mod reconnect_policy;
pub mod record;
pub mod route;
pub mod runbook;
pub mod scan;
pub mod serial;
pub mod sftp;
pub mod sftp_edit;
pub mod ssh;
pub mod sshca;
pub mod telnet;
pub mod tunnel;
pub mod vnc;
pub mod wol;
pub mod xfer;

use tokio::sync::{mpsc, oneshot};

/// Commands from the UI to an interactive terminal session.
#[derive(Debug)]
pub enum TermCmd {
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Upload local files/folders over SFTP on the same connection. `dir` is the
    /// shell's working directory if known (may start with `~`), else the home folder.
    Upload {
        files: Vec<std::path::PathBuf>,
        dir: Option<String>,
    },
    Close,
}

/// Events from an interactive terminal session to the UI.
#[derive(Debug)]
pub enum TermEvent {
    Status(String),
    Connected,
    Output(Vec<u8>),
    UploadProgress {
        name: String,
        done: u64,
        total: u64,
    },
    /// Upload finished: target directory, or the error.
    UploadDone(Result<String, String>),
    /// OS, uptime, disk and updates of the server, collected after login.
    Facts(crate::model::HostFacts),
    Closed(Option<String>),
    /// The link broke (read error, keepalive timeout, reset) – sent right before `Closed`.
    Dropped,
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
    /// Second opinions (SSHFP, jump host), filled in while the dialog is open.
    pub checks: Option<hostkey_check::SharedChecks>,
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

    pub fn ctx(&self) -> egui::Context {
        self.ctx.clone()
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}
pub mod import_csv;
pub mod import_model;
pub mod import_mremote;
pub mod import_parse;
pub mod import_plan;
pub mod import_rdm;
pub mod import_royal;
pub mod import_xml;
