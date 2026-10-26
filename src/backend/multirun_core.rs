//! "Run on many hosts": runs one command on a set of hosts in parallel (bounded
//! concurrency, per-host timeout, cancel) over SSH – or PowerShell over WinRM for Windows
//! hosts without SSH – and collects a result per host. Output is cleaned and capped, known
//! secret values are masked, nothing is logged. Pure helpers are tested below.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use russh::ChannelMsg;

use super::inventory_info::clean_output;
use super::ssh::{SshParams, connect};
use super::winrm_client::{RunError, open_trusted};
use super::winrm_http::Target;
use super::winrm_soap::powershell_args;
use super::{HostKeyPrompt, Sink};

/// Output kept per host (the rest is cut and marked).
pub const MAX_OUTPUT: usize = super::inventory_info::MAX_OUTPUT;
pub const MAX_CONCURRENCY: usize = 32;
pub const DEFAULT_CONCURRENCY: usize = 8;
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Pending,
    Running,
    Ok,
    Failed,
    Timeout,
    Unreachable,
    Skipped,
}

impl Status {
    /// Stable English word for exports.
    pub fn key(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Ok => "ok",
            Status::Failed => "failed",
            Status::Timeout => "timeout",
            Status::Unreachable => "unreachable",
            Status::Skipped => "skipped",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Pending => t!("wartet"),
            Status::Running => t!("läuft"),
            Status::Ok => t!("ok"),
            Status::Failed => t!("fehlgeschlagen"),
            Status::Timeout => t!("Zeitüberschreitung"),
            Status::Unreachable => t!("nicht erreichbar"),
            Status::Skipped => t!("übersprungen"),
        }
    }

    pub fn finished(self) -> bool {
        !matches!(self, Status::Pending | Status::Running)
    }

    /// Worth running again.
    pub fn retryable(self) -> bool {
        matches!(self, Status::Failed | Status::Timeout | Status::Unreachable)
    }
}

#[derive(Clone, Debug)]
pub struct HostResult {
    pub name: String,
    pub address: String,
    /// `PROD`, `TEST` … or empty.
    pub env: String,
    pub status: Status,
    pub exit: Option<i32>,
    pub took: Duration,
    /// Cleaned output (stdout and stderr together), capped at [`MAX_OUTPUT`].
    pub output: String,
    pub truncated: bool,
    /// Why it failed or was skipped.
    pub error: String,
    /// `ssh` or `winrm`.
    pub via: &'static str,
}

impl HostResult {
    pub fn pending(job: &Job) -> Self {
        HostResult {
            name: job.name.clone(),
            address: job.address.clone(),
            env: job.env.clone(),
            status: Status::Pending,
            exit: None,
            took: Duration::ZERO,
            output: String::new(),
            truncated: false,
            error: String::new(),
            via: match job.transport {
                Transport::Ssh(_) => "ssh",
                Transport::Winrm(_) => "winrm",
                Transport::None => "",
            },
        }
    }
}

pub enum Transport {
    Ssh(Box<SshParams>),
    Winrm(Box<Target>),
    /// Not reachable by any command channel (see `Job::skip`).
    None,
}

pub struct Job {
    pub name: String,
    pub address: String,
    pub env: String,
    pub transport: Transport,
    /// Do not run; report as skipped with this reason.
    pub skip: Option<String>,
    /// SSH host with Windows: run the command as PowerShell through `powershell.exe`.
    pub ssh_powershell: bool,
}

#[derive(Clone)]
pub struct Options {
    /// The command with all placeholders filled in.
    pub command: String,
    /// Run through `sudo -n` (SSH, POSIX hosts).
    pub sudo: bool,
    pub concurrency: usize,
    pub timeout: Duration,
    /// Values that must never appear in results.
    pub secrets: Vec<String>,
}

pub enum Event {
    Started(usize),
    Finished(usize, Box<HostResult>),
    Done,
}

/// Lets the UI stop a run.
#[derive(Clone)]
pub struct Handle {
    flag: Arc<AtomicBool>,
    tx: tokio::sync::watch::Sender<bool>,
}

impl Handle {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        let _ = self.tx.send(true);
    }

    #[cfg(test)]
    pub fn cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Replaces every secret value in `text` by `***`.
pub fn mask_secrets(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.chars().count() >= 3) {
        out = out.replace(s.as_str(), "***");
    }
    out
}

/// The command line the SSH server gets.
pub fn ssh_line(job: &Job, opts: &Options) -> Result<String> {
    if job.ssh_powershell {
        let mut parts = vec!["powershell.exe".to_string()];
        parts.extend(powershell_args(&opts.command)?);
        return Ok(parts.join(" "));
    }
    Ok(super::diag::wrap_with(&opts.command, opts.sudo, true))
}

/// Starts the run; results arrive through `sink`.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    jobs: Vec<(usize, Job)>,
    opts: Options,
    sink: Sink<Event>,
    prompts: Sink<HostKeyPrompt>,
) -> Handle {
    let flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = Handle {
        flag: flag.clone(),
        tx,
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(
        opts.concurrency.clamp(1, MAX_CONCURRENCY),
    ));
    let opts = Arc::new(opts);
    rt.spawn(async move {
        let mut tasks = Vec::new();
        for (idx, job) in jobs {
            let (sem, opts, sink, prompts, flag, mut rx) = (
                sem.clone(),
                opts.clone(),
                sink.clone(),
                prompts.clone(),
                flag.clone(),
                rx.clone(),
            );
            tasks.push(tokio::spawn(async move {
                if let Some(reason) = job.skip.clone() {
                    let mut r = HostResult::pending(&job);
                    r.status = Status::Skipped;
                    r.error = reason;
                    sink.send(Event::Finished(idx, Box::new(r)));
                    return;
                }
                // Wait for a free slot (or the cancel).
                let permit = tokio::select! {
                    p = sem.acquire_owned() => p.ok(),
                    _ = rx.wait_for(|c| *c) => None,
                };
                let Some(_permit) = permit.filter(|_| !flag.load(Ordering::Relaxed)) else {
                    let mut r = HostResult::pending(&job);
                    r.status = Status::Skipped;
                    r.error = t!("abgebrochen").to_string();
                    sink.send(Event::Finished(idx, Box::new(r)));
                    return;
                };
                sink.send(Event::Started(idx));
                let start = Instant::now();
                let mut r = tokio::select! {
                    r = run_one(&job, &opts, flag.clone(), prompts) => r,
                    _ = rx.wait_for(|c| *c) => {
                        let mut r = HostResult::pending(&job);
                        r.status = Status::Skipped;
                        r.error = t!("abgebrochen").to_string();
                        r
                    }
                };
                r.took = start.elapsed();
                r.output = mask_secrets(&r.output, &opts.secrets);
                r.error = mask_secrets(&r.error, &opts.secrets);
                sink.send(Event::Finished(idx, Box::new(r)));
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
        sink.send(Event::Done);
    });
    handle
}

const PHASE_CONNECT: u8 = 0;
const PHASE_RUN: u8 = 1;

async fn run_one(
    job: &Job,
    opts: &Options,
    flag: Arc<AtomicBool>,
    prompts: Sink<HostKeyPrompt>,
) -> HostResult {
    let mut r = HostResult::pending(job);
    let phase = Arc::new(AtomicU8::new(PHASE_CONNECT));
    let outcome = tokio::time::timeout(opts.timeout, async {
        match &job.transport {
            Transport::Ssh(p) => ssh_exec(job, p, opts, prompts, &phase).await,
            Transport::Winrm(t) => winrm_exec(t, opts, flag, &phase).await,
            Transport::None => Err(Failure::Other(anyhow!(
                "{}",
                t!("Kein Befehlskanal (SSH oder WinRM) eingerichtet")
            ))),
        }
    })
    .await;
    match outcome {
        Err(_) => {
            if phase.load(Ordering::Relaxed) == PHASE_CONNECT {
                r.status = Status::Unreachable;
                r.error = t!("Zeitüberschreitung beim Verbinden").to_string();
            } else {
                r.status = Status::Timeout;
                r.error = t!("Befehl nach {} s abgebrochen", opts.timeout.as_secs());
            }
        }
        Ok(Err(Failure::Unreachable(e))) => {
            r.status = Status::Unreachable;
            r.error = format!("{e:#}");
        }
        Ok(Err(Failure::Timeout)) => {
            r.status = Status::Timeout;
            r.error = t!("Befehl nach {} s abgebrochen", opts.timeout.as_secs());
        }
        Ok(Err(Failure::Other(e))) => {
            r.status = Status::Failed;
            r.error = format!("{e:#}");
        }
        Ok(Ok(done)) => {
            // Mask before the cap: a secret split by the cut would stay readable.
            let masked = mask_secrets(&String::from_utf8_lossy(&done.raw), &opts.secrets);
            (r.output, r.truncated) = clean_output(masked.as_bytes());
            r.truncated |= done.cut;
            r.exit = done.exit;
            if done.exit == Some(0) {
                r.status = Status::Ok;
            } else {
                r.status = Status::Failed;
                if done.exit.is_none() {
                    r.error = t!("Kein Exit-Code (Verbindung beendet)").to_string();
                }
            }
        }
    }
    r
}

enum Failure {
    Unreachable(anyhow::Error),
    Timeout,
    Other(anyhow::Error),
}

struct Done {
    raw: Vec<u8>,
    exit: Option<i32>,
    cut: bool,
}

async fn ssh_exec(
    job: &Job,
    params: &SshParams,
    opts: &Options,
    prompts: Sink<HostKeyPrompt>,
    phase: &AtomicU8,
) -> Result<Done, Failure> {
    let line = ssh_line(job, opts).map_err(Failure::Other)?;
    let handle = connect(params, prompts)
        .await
        .map_err(Failure::Unreachable)?;
    phase.store(PHASE_RUN, Ordering::Relaxed);
    let work = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, line).await?;
        // sudo -n and PowerShell must not wait for input.
        let _ = ch.eof().await;
        let (mut out, mut cut, mut exit) = (Vec::new(), false, None);
        let cap = MAX_OUTPUT * 2;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    let room = cap.saturating_sub(out.len());
                    cut |= data.len() > room;
                    out.extend_from_slice(&data[..data.len().min(room)]);
                }
                ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status as i32),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok(Done {
            raw: out,
            exit,
            cut,
        })
    };
    let res = work.await.map_err(Failure::Other);
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await;
    res
}

async fn winrm_exec(
    target: &Target,
    opts: &Options,
    flag: Arc<AtomicBool>,
    phase: &Arc<AtomicU8>,
) -> Result<Done, Failure> {
    let (target, command, timeout, phase) = (
        target.clone(),
        opts.command.clone(),
        opts.timeout,
        phase.clone(),
    );
    tokio::task::spawn_blocking(move || {
        // Never ask in a batch run: an unconfirmed certificate is a failure.
        let (mut sess, _) = open_trusted(&target, None).map_err(Failure::Unreachable)?;
        phase.store(PHASE_RUN, Ordering::Relaxed);
        let res = sess.run_script(&command, timeout, MAX_OUTPUT * 2, &flag);
        sess.close();
        match res {
            Ok(o) => {
                let mut raw = o.stdout;
                if !o.stderr.trim().is_empty() {
                    if !raw.is_empty() && !raw.ends_with(b"\n") {
                        raw.push(b'\n');
                    }
                    raw.extend_from_slice(o.stderr.as_bytes());
                }
                Ok(Done {
                    raw,
                    exit: Some(o.exit),
                    cut: o.truncated,
                })
            }
            Err(RunError::Timeout) => Err(Failure::Timeout),
            Err(RunError::Cancelled) => Err(Failure::Other(anyhow!("{}", t!("abgebrochen")))),
            Err(RunError::Other(e)) => Err(Failure::Other(e)),
        }
    })
    .await
    .map_err(|e| Failure::Other(anyhow!("{e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(ssh_powershell: bool) -> Job {
        Job {
            name: "web".into(),
            address: "10.0.0.1".into(),
            env: "PROD".into(),
            transport: Transport::None,
            skip: None,
            ssh_powershell,
        }
    }

    fn opts(cmd: &str, sudo: bool) -> Options {
        Options {
            command: cmd.into(),
            sudo,
            concurrency: 8,
            timeout: Duration::from_secs(5),
            secrets: vec![],
        }
    }

    #[test]
    fn command_lines() {
        let l = ssh_line(&job(false), &opts("uptime", false)).unwrap();
        assert_eq!(l, "env LC_ALL=C sh -c 'uptime'");
        let l = ssh_line(&job(false), &opts("cat /etc/shadow", true)).unwrap();
        assert!(l.starts_with("sudo -n env LC_ALL=C sh -c "), "{l}");
        // A quote in the command cannot break out of the sh -c string.
        let l = ssh_line(&job(false), &opts("echo 'a'; rm x", false)).unwrap();
        assert_eq!(l, "env LC_ALL=C sh -c 'echo '\\''a'\\''; rm x'");
        let l = ssh_line(&job(true), &opts("Get-Date", false)).unwrap();
        assert!(l.starts_with("powershell.exe -NoProfile -NonInteractive -EncodedCommand "));
        assert!(!l.contains("Get-Date"), "the script travels encoded");
    }

    #[test]
    fn secrets_are_masked() {
        let s = vec!["hunter2".to_string(), "ab".to_string()];
        assert_eq!(
            mask_secrets("pw=hunter2 and hunter2 ab", &s),
            "pw=*** and *** ab"
        );
        assert_eq!(mask_secrets("nothing", &s), "nothing");
    }

    #[test]
    fn status_words() {
        assert!(Status::Failed.retryable() && Status::Timeout.retryable());
        assert!(!Status::Ok.retryable() && !Status::Skipped.retryable());
        assert!(Status::Ok.finished() && !Status::Running.finished());
        assert_eq!(Status::Unreachable.key(), "unreachable");
    }

    /// The engine itself, with hosts that cannot be reached: bounded, cancellable, every
    /// job reports exactly once.
    #[test]
    fn engine_reports_every_job_once() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ctx = egui::Context::default();
        let (sink, mut rx) = Sink::<Event>::new(ctx.clone());
        let (psink, _prx) = Sink::<HostKeyPrompt>::new(ctx);
        let mut jobs = Vec::new();
        for i in 0..6 {
            let mut j = job(false);
            if i == 0 {
                j.skip = Some("because".into());
            }
            jobs.push((i, j));
        }
        let h = spawn(&rt, jobs, opts("true", false), sink, psink);
        let mut seen = vec![0; 6];
        let mut done = false;
        let t0 = Instant::now();
        while !done && t0.elapsed() < Duration::from_secs(10) {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    Event::Finished(i, r) => {
                        seen[i] += 1;
                        if i == 0 {
                            assert_eq!(r.status, Status::Skipped);
                            assert_eq!(r.error, "because");
                        } else {
                            // No command channel configured.
                            assert_eq!(r.status, Status::Failed, "{}", r.error);
                        }
                    }
                    Event::Done => done = true,
                    Event::Started(_) => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(done && seen.iter().all(|n| *n == 1), "{seen:?}");
        assert!(!h.cancelled());
        h.cancel();
        assert!(h.cancelled());
    }
}
