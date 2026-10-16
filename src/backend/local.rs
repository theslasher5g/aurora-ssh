//! Programs on this PC in a pseudo terminal (ConPTY on Windows), e.g. `docker exec -it`
//! or `kubectl exec -it`. Blocking I/O on worker threads, like the serial backend.

use std::io::{Read, Write};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::mpsc;

use super::{Sink, TermCmd, TermEvent};

/// Starts `argv` in a pseudo terminal; returns the command sender.
pub fn spawn(
    argv: Vec<String>,
    cols: u16,
    rows: u16,
    events: Sink<TermEvent>,
) -> mpsc::UnboundedSender<TermCmd> {
    let (tx, rx) = mpsc::unbounded_channel::<TermCmd>();
    std::thread::spawn(move || {
        let r = run(argv, cols, rows, rx, &events);
        events.send(TermEvent::Closed(r.err().map(|e| format!("{e:#}"))));
    });
    tx
}

/// Full path of `program` from PATH (with PATHEXT on Windows). Only PATH is searched:
/// Windows would otherwise also run a same-named file from the current directory.
pub fn find_program(program: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(program);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
            .collect()
    } else {
        Vec::new()
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| {
            let plain = dir.join(program);
            if plain.is_file() {
                return Some(plain);
            }
            exts.iter()
                .map(|e| dir.join(format!("{program}{e}")))
                .find(|c| c.is_file())
        })
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn run(
    argv: Vec<String>,
    cols: u16,
    rows: u16,
    mut cmds: mpsc::UnboundedReceiver<TermCmd>,
    events: &Sink<TermEvent>,
) -> anyhow::Result<()> {
    let Some(program) = argv.first() else {
        anyhow::bail!("no command");
    };
    events.send(TermEvent::Status(t!("Starte {}…", program)));
    let exe = find_program(program).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            t!(
                "{} wurde nicht gefunden. Ist es installiert und im PATH?",
                program
            )
        )
    })?;
    let pair = native_pty_system().openpty(size(cols, rows))?;
    let mut cmd = CommandBuilder::new(exe);
    cmd.args(&argv[1..]);
    cmd.env("TERM", "xterm-256color");
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        cmd.cwd(home);
    }
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| {
        anyhow::anyhow!(
            "{}",
            t!(
                "{} konnte nicht gestartet werden ({}). Ist es installiert und im PATH?",
                program,
                e
            )
        )
    })?;
    // The slave end belongs to the child now; keeping it open would hide EOF.
    drop(pair.slave);
    events.send(TermEvent::Connected);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let out = events.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.send(TermEvent::Output(buf[..n].to_vec())),
            }
        }
    });
    let mut killer = child.clone_killer();
    // The command loop ends when the UI closes the session; a reader that hit EOF
    // (program exited) ends it through `Close` from the waiter below.
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _ = reader_thread.join();
        let _ = done_tx.send(());
    });
    loop {
        if done_rx.try_recv().is_ok() {
            break;
        }
        match cmds.try_recv() {
            Ok(TermCmd::Input(bytes)) => {
                writer.write_all(&bytes)?;
                writer.flush()?;
            }
            Ok(TermCmd::Resize { cols, rows }) => {
                let _ = pair.master.resize(size(cols, rows));
            }
            Ok(TermCmd::Upload { .. }) => {}
            Ok(TermCmd::Close) | Err(mpsc::error::TryRecvError::Disconnected) => {
                let _ = killer.kill();
                break;
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{}", t!("Beendet mit Code {}", status.exit_code()))
    }
}
