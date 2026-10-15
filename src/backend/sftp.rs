//! SFTP worker: directory listings and recursive up/downloads with progress.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Result, anyhow};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::ssh::{SshParams, connect};
use super::{HostKeyPrompt, Sink};

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug)]
pub enum SftpCmd {
    List(String),
    Download {
        remote: String,
        local_dir: PathBuf,
        id: u64,
    },
    Upload {
        local: PathBuf,
        remote_dir: String,
        id: u64,
    },
    Mkdir(String),
    Remove {
        path: String,
        is_dir: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    Close,
}

#[derive(Debug)]
pub enum SftpEvent {
    Status(String),
    Connected {
        home: String,
        session: Session,
    },
    Listing {
        path: String,
        entries: Vec<Entry>,
    },
    Progress {
        id: u64,
        name: String,
        done: u64,
        total: u64,
    },
    TransferDone {
        id: u64,
        error: Option<String>,
    },
    /// The current directory changed on the server; the UI should refresh.
    Changed,
    Error(String),
    Closed(Option<String>),
}

/// Shared handle to an open SFTP session; used for server ⇄ server copies.
#[derive(Clone)]
pub struct Session(pub Arc<SftpSession>);

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session")
    }
}

/// Accepts Windows-style input (`C:\Users`, `C:/x`) and turns it into the path form
/// Win32-OpenSSH expects (`/C:/Users`). POSIX paths pass through unchanged.
pub fn normalize_remote(input: &str) -> String {
    let s = input.trim().replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        format!("/{s}")
    } else {
        s
    }
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => trimmed[..i].into(),
    }
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    params: SshParams,
    events: Sink<SftpEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> mpsc::UnboundedSender<SftpCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(async move {
        let result = run(&params, rx, &events, prompts).await;
        events.send(SftpEvent::Closed(result.err().map(|e| format!("{e:#}"))));
    });
    tx
}

async fn run(
    p: &SshParams,
    mut cmds: mpsc::UnboundedReceiver<SftpCmd>,
    events: &Sink<SftpEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> Result<()> {
    events.send(SftpEvent::Status(t!("Verbinde mit {}…", p.host_id())));
    let handle = connect(p, prompts).await?;
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = Arc::new(SftpSession::new(channel.into_stream()).await?);
    let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
    events.send(SftpEvent::Connected {
        home: home.clone(),
        session: Session(sftp.clone()),
    });

    while let Some(cmd) = cmds.recv().await {
        let res: Result<()> = match cmd {
            SftpCmd::Close => break,
            SftpCmd::List(path) => list(&sftp, &path, events).await,
            SftpCmd::Mkdir(path) => sftp.create_dir(path).await.map_err(Into::into).map(|_| {
                events.send(SftpEvent::Changed);
            }),
            SftpCmd::Rename { from, to } => {
                sftp.rename(from, to).await.map_err(Into::into).map(|_| {
                    events.send(SftpEvent::Changed);
                })
            }
            SftpCmd::Remove { path, is_dir } => {
                let r = if is_dir {
                    remove_recursive(&sftp, &path).await
                } else {
                    sftp.remove_file(path).await.map_err(Into::into)
                };
                r.map(|_| events.send(SftpEvent::Changed))
            }
            SftpCmd::Download {
                remote,
                local_dir,
                id,
            } => {
                let (sftp, events) = (sftp.clone(), events.clone());
                tokio::spawn(async move {
                    let r = download(&sftp, &remote, &local_dir, id, &events).await;
                    events.send(SftpEvent::TransferDone {
                        id,
                        error: r.err().map(|e| format!("{e:#}")),
                    });
                });
                Ok(())
            }
            SftpCmd::Upload {
                local,
                remote_dir,
                id,
            } => {
                let (sftp, events) = (sftp.clone(), events.clone());
                tokio::spawn(async move {
                    let r = upload(&sftp, &local, &remote_dir, id, &events).await;
                    events.send(SftpEvent::TransferDone {
                        id,
                        error: r.err().map(|e| format!("{e:#}")),
                    });
                    events.send(SftpEvent::Changed);
                });
                Ok(())
            }
        };
        if let Err(e) = res {
            events.send(SftpEvent::Error(format!("{e:#}")));
        }
    }
    let _ = sftp.close().await;
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await;
    Ok(())
}

async fn list(sftp: &SftpSession, path: &str, events: &Sink<SftpEvent>) -> Result<()> {
    let path = sftp.canonicalize(path).await?;
    let mut entries: Vec<Entry> = sftp
        .read_dir(path.clone())
        .await?
        .map(|e| {
            let meta = e.metadata();
            let ft = e.file_type();
            Entry {
                name: e.file_name(),
                is_dir: ft.is_dir(),
                is_link: ft.is_symlink(),
                size: meta.size.unwrap_or(0),
                modified: meta.modified().ok(),
            }
        })
        .filter(|e| e.name != "." && e.name != "..")
        .collect();
    // Symlinks to directories should be navigable.
    for e in entries.iter_mut().filter(|e| e.is_link) {
        if let Ok(m) = sftp.metadata(join(&path, &e.name)).await {
            e.is_dir = m.is_dir();
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    events.send(SftpEvent::Listing { path, entries });
    Ok(())
}

async fn remove_recursive(sftp: &SftpSession, path: &str) -> Result<()> {
    let mut stack = vec![(path.to_string(), false)];
    while let Some((dir, visited)) = stack.pop() {
        if visited {
            sftp.remove_dir(dir).await?;
            continue;
        }
        stack.push((dir.clone(), true));
        for e in sftp.read_dir(dir.clone()).await? {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let full = join(&dir, &name);
            if e.file_type().is_dir() {
                stack.push((full, false));
            } else {
                sftp.remove_file(full).await?;
            }
        }
    }
    Ok(())
}

const CHUNK: usize = 256 * 1024;

/// A remote file name that is safe to use as a single local path component.
/// Names come from the server, so `..`, separators or drive prefixes would let a
/// malicious host write outside the chosen download folder.
pub fn safe_local_name(name: &str) -> Option<&str> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':', '\0'])
        || name.chars().any(char::is_control);
    (!bad).then_some(name)
}

async fn download(
    sftp: &SftpSession,
    remote: &str,
    local_dir: &Path,
    id: u64,
    ev: &Sink<SftpEvent>,
) -> Result<()> {
    let base = remote
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let name = safe_local_name(base)
        .ok_or_else(|| anyhow!("{}", t!("unsicherer Dateiname: {}", format!("{base:?}"))))?;
    let meta = sftp.metadata(remote.to_string()).await?;
    if !meta.is_dir() {
        return download_file(sftp, remote, &local_dir.join(name), id, ev).await;
    }
    // Directory: walk it breadth-first.
    let mut skipped = 0usize;
    let mut queue = vec![(remote.to_string(), local_dir.join(name))];
    while let Some((rdir, ldir)) = queue.pop() {
        tokio::fs::create_dir_all(&ldir).await?;
        for e in sftp.read_dir(rdir.clone()).await? {
            let n = e.file_name();
            if n == "." || n == ".." {
                continue;
            }
            let Some(local_name) = safe_local_name(&n) else {
                skipped += 1;
                continue;
            };
            let rpath = join(&rdir, &n);
            if e.file_type().is_dir() {
                queue.push((rpath, ldir.join(local_name)));
            } else if e.file_type().is_file() {
                download_file(sftp, &rpath, &ldir.join(local_name), id, ev).await?;
            }
        }
    }
    if skipped > 0 {
        return Err(anyhow!(
            "{}",
            t!("{} Einträge mit unsicheren Namen übersprungen", skipped)
        ));
    }
    Ok(())
}

async fn download_file(
    sftp: &SftpSession,
    remote: &str,
    local: &Path,
    id: u64,
    ev: &Sink<SftpEvent>,
) -> Result<()> {
    let total = sftp.metadata(remote.to_string()).await?.size.unwrap_or(0);
    let name = remote.rsplit('/').next().unwrap_or(remote).to_string();
    let mut src = sftp.open(remote.to_string()).await?;
    let mut dst = tokio::fs::File::create(local).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await?;
        done += n as u64;
        ev.send(SftpEvent::Progress {
            id,
            name: name.clone(),
            done,
            total,
        });
        if ev.is_closed() {
            return Err(anyhow!(t!("abgebrochen")));
        }
    }
    dst.flush().await?;
    Ok(())
}

pub(crate) async fn upload(
    sftp: &SftpSession,
    local: &Path,
    remote_dir: &str,
    id: u64,
    ev: &Sink<SftpEvent>,
) -> Result<()> {
    let name = local
        .file_name()
        .ok_or_else(|| anyhow!(t!("ungültiger Pfad")))?
        .to_string_lossy()
        .to_string();
    if !local.is_dir() {
        return upload_file(sftp, local, &join(remote_dir, &name), id, ev).await;
    }
    let mut queue = vec![(local.to_path_buf(), join(remote_dir, &name))];
    while let Some((ldir, rdir)) = queue.pop() {
        if !sftp.try_exists(rdir.clone()).await.unwrap_or(false) {
            sftp.create_dir(rdir.clone()).await?;
        }
        for entry in std::fs::read_dir(&ldir)? {
            let entry = entry?;
            let n = entry.file_name().to_string_lossy().to_string();
            let rpath = join(&rdir, &n);
            if entry.file_type()?.is_dir() {
                queue.push((entry.path(), rpath));
            } else {
                upload_file(sftp, &entry.path(), &rpath, id, ev).await?;
            }
        }
    }
    Ok(())
}

async fn upload_file(
    sftp: &SftpSession,
    local: &Path,
    remote: &str,
    id: u64,
    ev: &Sink<SftpEvent>,
) -> Result<()> {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut src = tokio::fs::File::open(local).await?;
    let total = src.metadata().await?.len();
    let mut dst = sftp.create(remote.to_string()).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await?;
        done += n as u64;
        ev.send(SftpEvent::Progress {
            id,
            name: name.clone(),
            done,
            total,
        });
        if ev.is_closed() {
            return Err(anyhow!(t!("abgebrochen")));
        }
    }
    dst.shutdown().await?;
    Ok(())
}

/// Copies a file or directory from one server to another, streaming through Aurora,
/// so the two servers never need to reach each other.
pub async fn relay(
    src: Arc<SftpSession>,
    src_path: String,
    dst: Arc<SftpSession>,
    dst_dir: String,
    id: u64,
    ev: &Sink<SftpEvent>,
) -> Result<()> {
    let base = src_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let safe = |n: &str| !(n.is_empty() || n == "." || n == ".." || n.contains(['/', '\\', '\0']));
    if !safe(&base) {
        return Err(anyhow!(
            "{}",
            t!("unsicherer Dateiname: {}", format!("{base:?}"))
        ));
    }
    let meta = src.metadata(src_path.clone()).await?;
    if !meta.is_dir() {
        return relay_file(&src, &src_path, &dst, &join(&dst_dir, &base), id, ev).await;
    }
    let mut skipped = 0usize;
    let mut queue = vec![(src_path, join(&dst_dir, &base))];
    while let Some((sdir, ddir)) = queue.pop() {
        if !dst.try_exists(ddir.clone()).await.unwrap_or(false) {
            dst.create_dir(ddir.clone()).await?;
        }
        for e in src.read_dir(sdir.clone()).await? {
            let n = e.file_name();
            if n == "." || n == ".." {
                continue;
            }
            if !safe(&n) {
                skipped += 1;
                continue;
            }
            let (sp, dp) = (join(&sdir, &n), join(&ddir, &n));
            if e.file_type().is_dir() {
                queue.push((sp, dp));
            } else if e.file_type().is_file() {
                relay_file(&src, &sp, &dst, &dp, id, ev).await?;
            }
        }
    }
    if skipped > 0 {
        return Err(anyhow!(
            "{}",
            t!("{} Einträge mit unsicheren Namen übersprungen", skipped)
        ));
    }
    Ok(())
}

async fn relay_file(
    src: &SftpSession,
    src_path: &str,
    dst: &SftpSession,
    dst_path: &str,
    id: u64,
    ev: &Sink<SftpEvent>,
) -> Result<()> {
    let total = src.metadata(src_path.to_string()).await?.size.unwrap_or(0);
    let name = src_path.rsplit('/').next().unwrap_or(src_path).to_string();
    let mut from = src.open(src_path.to_string()).await?;
    let mut to = dst.create(dst_path.to_string()).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n]).await?;
        done += n as u64;
        ev.send(SftpEvent::Progress {
            id,
            name: name.clone(),
            done,
            total,
        });
        if ev.is_closed() {
            return Err(anyhow!(t!("abgebrochen")));
        }
    }
    to.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers() {
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/home/u", "a.txt"), "/home/u/a.txt");
        assert_eq!(parent("/home/u"), "/home");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(normalize_remote("C:\\Users\\me"), "/C:/Users/me");
        assert_eq!(normalize_remote("d:/data"), "/d:/data");
        assert_eq!(normalize_remote(" /home/u "), "/home/u");
        assert_eq!(parent("/C:/Users"), "/C:");
        assert_eq!(parent("/C:"), "/");
        assert_eq!(safe_local_name("report.txt"), Some("report.txt"));
        for bad in ["..", ".", "", "../x", "a/b", "a\\b", "C:evil", "x\ny"] {
            assert_eq!(safe_local_name(bad), None, "{bad:?}");
        }
    }
}
