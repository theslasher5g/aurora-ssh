//! WinRM client: opens a remote shell and runs PowerShell scripts in it (blocking; call it
//! from `spawn_blocking`). Certificates are pinned like SSH host keys: an unknown or
//! changed certificate is shown to the user (the same dialog as for SSH and RDP) before
//! anything is sent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

use super::winrm_http::{ConnectError, Http, Target};
use super::winrm_soap as soap;
use super::{HostKeyPrompt, Sink};

/// Seconds the server may wait for output before it answers "nothing yet".
const RECEIVE_WAIT: u32 = 10;

#[derive(Debug)]
pub struct ExecOut {
    pub stdout: Vec<u8>,
    /// Error stream as plain text (CLIXML decoded).
    pub stderr: String,
    pub exit: i32,
    /// More output than the cap came in; the rest was dropped.
    pub truncated: bool,
}

#[derive(Debug)]
pub enum RunError {
    Timeout,
    Cancelled,
    Other(anyhow::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Timeout => write!(f, "{}", t!("Zeitüberschreitung")),
            RunError::Cancelled => write!(f, "{}", t!("abgebrochen")),
            RunError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<anyhow::Error> for RunError {
    fn from(e: anyhow::Error) -> Self {
        RunError::Other(e)
    }
}

pub struct Session {
    http: Http,
    url: String,
    shell: String,
}

impl Session {
    /// Connects, authenticates and creates the remote shell.
    pub fn open(target: &Target) -> Result<Session, ConnectError> {
        let mut http = Http::connect(target)?;
        let url = target.url();
        let answer = http.soap(&soap::create_shell(&url))?;
        let root = soap::parse_response(&answer)?;
        let shell = soap::shell_id(&root)?;
        Ok(Session { http, url, shell })
    }

    /// Runs a PowerShell script; `cap` bounds the stored stdout.
    pub fn run_script(
        &mut self,
        script: &str,
        timeout: Duration,
        cap: usize,
        cancel: &AtomicBool,
    ) -> Result<ExecOut, RunError> {
        let args = soap::powershell_args(script)?;
        let answer = self.http.soap(&soap::command(
            &self.url,
            &self.shell,
            "powershell.exe",
            &args,
        ))?;
        let id = soap::command_id(&soap::parse_response(&answer)?)?;
        let deadline = Instant::now() + timeout;
        let mut out = ExecOut {
            stdout: Vec::new(),
            stderr: String::new(),
            exit: 0,
            truncated: false,
        };
        let mut err_raw: Vec<u8> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                self.terminate(&id);
                return Err(RunError::Cancelled);
            }
            if Instant::now() >= deadline {
                self.terminate(&id);
                return Err(RunError::Timeout);
            }
            let answer =
                self.http
                    .soap(&soap::receive(&self.url, &self.shell, &id, RECEIVE_WAIT))?;
            let root = soap::parse(&answer).map_err(RunError::Other)?;
            if let Some(f) = soap::fault(&root) {
                if f.is_timeout() {
                    continue;
                }
                return Err(RunError::Other(anyhow!("{}", f.text())));
            }
            let got = soap::received(&root);
            for (data, dst) in [(&got.stdout, &mut out.stdout), (&got.stderr, &mut err_raw)] {
                let room = cap.saturating_sub(dst.len());
                if data.len() > room {
                    out.truncated = true;
                }
                dst.extend_from_slice(&data[..data.len().min(room)]);
            }
            if got.done {
                out.exit = got.exit.unwrap_or(0);
                break;
            }
        }
        out.stderr = soap::clixml_to_text(&String::from_utf8_lossy(&err_raw));
        Ok(out)
    }

    /// Stops a running command (best effort).
    fn terminate(&mut self, id: &str) {
        let _ = self.http.soap(&soap::terminate(&self.url, &self.shell, id));
    }

    /// Deletes the remote shell (best effort).
    pub fn close(mut self) {
        let _ = self.http.soap(&soap::delete_shell(&self.url, &self.shell));
    }
}

/// Like [`Session::open`], but asks the user about an unknown or changed certificate
/// first (`prompts`; `None` = never ask, fail instead). Returns the session and the
/// certificate fingerprint that was accepted (to remember it).
pub fn open_trusted(
    target: &Target,
    prompts: Option<&Sink<HostKeyPrompt>>,
) -> Result<(Session, Option<String>)> {
    let mut target = target.clone();
    let mut accepted = None;
    for _ in 0..3 {
        match Session::open(&target) {
            Ok(s) => return Ok((s, accepted)),
            Err(ConnectError::Other(e)) => return Err(e),
            Err(e @ (ConnectError::Untrusted(_) | ConnectError::PinChanged { .. })) => {
                let Some(prompts) = prompts else {
                    bail!("{e}");
                };
                let (fp, previous) = match e {
                    ConnectError::Untrusted(fp) => (fp, None),
                    ConnectError::PinChanged { pinned, got } => (got, Some(pinned)),
                    ConnectError::Other(_) => unreachable!(),
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                prompts.send(HostKeyPrompt {
                    host: super::winrm_types::pin_key(&target.host, target.port),
                    algorithm: "WinRM TLS (SHA-256)".into(),
                    fingerprint: fp.clone(),
                    previous,
                    reply: tx,
                    checks: None,
                });
                if !rx.blocking_recv().unwrap_or(false) {
                    bail!("{}", t!("Zertifikat nicht bestätigt"));
                }
                target.pin = Some(fp.clone());
                accepted = Some(fp);
            }
        }
    }
    bail!("{}", t!("Das Zertifikat konnte nicht bestätigt werden"))
}
