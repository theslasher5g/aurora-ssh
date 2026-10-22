//! Log viewer source: a long-running command (`tail -F`, `journalctl -f`, `docker logs -f`)
//! on an SSH host or on this PC, streamed line by line.

use std::process::Stdio;

use anyhow::{Result, anyhow};
use russh::ChannelMsg;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

use super::container::Target;
use super::ssh::connect;
use super::{HostKeyPrompt, Sink};

pub enum LogEvent {
    Status(String),
    Connected,
    Lines(Vec<String>),
    Closed(Option<String>),
}

/// What the log tab follows; stored with the session so it can be reopened.
#[derive(Clone, Debug, PartialEq)]
pub enum LogSource {
    /// A file, followed with `tail -F`.
    File(String),
    /// Any command that keeps printing (e.g. `journalctl -f`).
    Command(String),
}

impl LogSource {
    /// `/var/log/syslog` → file, anything else → command.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if (s.starts_with('/') || s.starts_with('~')) && !s.contains(' ') {
            LogSource::File(s.to_string())
        } else {
            LogSource::Command(s.to_string())
        }
    }

    pub fn text(&self) -> &str {
        match self {
            LogSource::File(s) | LogSource::Command(s) => s,
        }
    }

    pub fn command(&self) -> String {
        match self {
            LogSource::File(p) => {
                let path = match p.strip_prefix("~/") {
                    Some(rest) => format!("\"$HOME\"/{}", super::container::sh_quote(rest)),
                    None => super::container::sh_quote(p),
                };
                format!("tail -n 500 -F {path}")
            }
            LogSource::Command(c) => c.clone(),
        }
    }
}

/// Starts following; dropping the returned sender stops it.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    target: Target,
    source: LogSource,
    events: Sink<LogEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> mpsc::UnboundedSender<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(async move {
        let r = match target {
            Target::Ssh(p) => run_ssh(p, &source, rx, &events, prompts).await,
            Target::Local => run_local(&source, rx, &events).await,
        };
        events.send(LogEvent::Closed(r.err().map(|e| format!("{e:#}"))));
    });
    tx
}

/// Collects complete lines; a partial last line waits for the rest.
#[derive(Default)]
struct Lines {
    rest: Vec<u8>,
}

impl Lines {
    fn push(&mut self, data: &[u8]) -> Vec<String> {
        self.rest.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(i) = self.rest.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.rest.drain(..=i).collect();
            out.push(clean(&line));
        }
        // Never buffer unbounded data without a newline.
        if self.rest.len() > 64 * 1024 {
            out.push(clean(&std::mem::take(&mut self.rest)));
        }
        out
    }
}

/// Drops the line break, carriage returns and escape sequences (colors).
fn clean(line: &[u8]) -> String {
    let s = String::from_utf8_lossy(line);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // CSI: ESC [ … final byte
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
            }
            '\r' | '\n' => {}
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

async fn run_ssh(
    p: super::ssh::SshParams,
    source: &LogSource,
    mut stop: mpsc::UnboundedReceiver<()>,
    events: &Sink<LogEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> Result<()> {
    events.send(LogEvent::Status(t!("Verbinde mit {}…", p.host_id())));
    let handle = connect(&p, prompts).await?;
    let mut ch = handle.channel_open_session().await?;
    ch.exec(
        true,
        // Under `sh`, so a fish/csh login shell does not choke on the POSIX syntax.
        super::inventory_collect::sh_wrap(&format!(
            "export TERM=${{TERM:-xterm-256color}} COLUMNS=${{COLUMNS:-200}}; {} 2>&1",
            source.command()
        )),
    )
    .await?;
    events.send(LogEvent::Connected);
    let mut lines = Lines::default();
    let mut status = None;
    loop {
        tokio::select! {
            _ = stop.recv() => break,
            msg = ch.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let l = lines.push(&data);
                    if !l.is_empty() {
                        events.send(LogEvent::Lines(l));
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
        }
    }
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await;
    match status {
        Some(code) if code != 0 => Err(anyhow!("{}", t!("Beendet mit Code {}", code))),
        _ => Ok(()),
    }
}

async fn run_local(
    source: &LogSource,
    mut stop: mpsc::UnboundedReceiver<()>,
    events: &Sink<LogEvent>,
) -> Result<()> {
    // Local sources are container logs (argv joined with `shell_line`) or a file.
    let mut cmd = match source {
        LogSource::File(path) => {
            return follow_file(path, stop, events).await;
        }
        LogSource::Command(line) => {
            let argv = super::container::split_args(line);
            let Some(prog) = argv.first() else {
                return Err(anyhow!("no command"));
            };
            let exe = super::local::find_program(prog).ok_or_else(|| {
                anyhow!(
                    "{}",
                    t!(
                        "{} wurde nicht gefunden. Ist es installiert und im PATH?",
                        prog
                    )
                )
            })?;
            let mut c = tokio::process::Command::new(exe);
            c.args(&argv[1..]);
            c
        }
    };
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| anyhow!("{e}"))?;
    events.send(LogEvent::Connected);
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    fn pump<R: AsyncRead + Unpin + Send + 'static>(r: R, tx: mpsc::UnboundedSender<String>) {
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if tx.send(clean(l.as_bytes())).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(o) = child.stdout.take() {
        pump(o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        pump(e, tx.clone());
    }
    drop(tx);
    loop {
        tokio::select! {
            _ = stop.recv() => {
                let _ = child.kill().await;
                return Ok(());
            }
            line = rx.recv() => match line {
                Some(l) => {
                    let mut batch = vec![l];
                    while let Ok(more) = rx.try_recv() {
                        batch.push(more);
                        if batch.len() >= 2000 {
                            break;
                        }
                    }
                    events.send(LogEvent::Lines(batch));
                }
                None => break,
            },
        }
    }
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{}",
            t!("Beendet mit Code {}", status.code().unwrap_or(-1))
        ))
    }
}

/// `tail -F` for a local file: last 500 lines, then whatever gets appended.
async fn follow_file(
    path: &str,
    mut stop: mpsc::UnboundedReceiver<()>,
    events: &Sink<LogEvent>,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut f = tokio::fs::File::open(path).await?;
    events.send(LogEvent::Connected);
    let len = f.metadata().await?.len();
    let start = len.saturating_sub(256 * 1024);
    f.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).await?;
    let mut lines = Lines::default();
    let mut first = lines.push(&buf);
    if start > 0 && !first.is_empty() {
        first.remove(0); // cut in the middle of a line
    }
    let skip = first.len().saturating_sub(500);
    events.send(LogEvent::Lines(first.split_off(skip)));
    let mut pos = len;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = stop.recv() => return Ok(()),
            _ = tick.tick() => {
                let now = tokio::fs::metadata(path).await.map(|m| m.len()).unwrap_or(pos);
                if now < pos {
                    // Rotated or truncated: start over.
                    f = tokio::fs::File::open(path).await?;
                    pos = 0;
                }
                if now > pos {
                    f.seek(std::io::SeekFrom::Start(pos)).await?;
                    let mut chunk = Vec::new();
                    (&mut f).take(now - pos).read_to_end(&mut chunk).await?;
                    pos += chunk.len() as u64;
                    let l = lines.push(&chunk);
                    if !l.is_empty() {
                        events.send(LogEvent::Lines(l));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_sources() {
        let mut l = Lines::default();
        assert_eq!(l.push(b"one\r\ntw"), vec!["one"]);
        assert_eq!(l.push(b"o\n\x1b[31mred\x1b[0m\n"), vec!["two", "red"]);
        assert_eq!(
            LogSource::parse("/var/log/my app.log"),
            LogSource::Command("/var/log/my app.log".into())
        );
        assert_eq!(
            LogSource::parse(" /var/log/syslog ").command(),
            "tail -n 500 -F /var/log/syslog"
        );
        assert_eq!(
            LogSource::parse("~/app.log").command(),
            "tail -n 500 -F \"$HOME\"/app.log"
        );
        assert_eq!(
            LogSource::parse("journalctl -f"),
            LogSource::Command("journalctl -f".into())
        );
    }
}
