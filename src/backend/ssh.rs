//! SSH connection setup (auth + host key verification) and the interactive shell.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::PublicKeyOrCertificate;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, decode_secret_key};
use russh::{ChannelMsg, Disconnect};
use tokio::sync::{mpsc, oneshot};

use super::{HostKeyPrompt, Sink, TermCmd, TermEvent};

/// Everything needed to (re)open an SSH connection.
#[derive(Clone)]
pub struct SshParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
    /// Previously accepted fingerprint for this `host:port`.
    pub known_fingerprint: Option<String>,
    /// Connect through this host first (ProxyJump).
    pub jump: Option<Box<SshParams>>,
}

impl SshParams {
    pub fn host_id(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub struct Client {
    host_id: String,
    known: Option<String>,
    prompts: Sink<HostKeyPrompt>,
    /// Keeps the jump host connection alive as long as this session lives.
    _via: Option<Arc<SshHandle>>,
}

impl client::Handler for Client {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let (algorithm, fingerprint) = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => (
                key.algorithm().as_str().to_string(),
                key.fingerprint(HashAlg::Sha256).to_string(),
            ),
            PublicKeyOrCertificate::Certificate(cert) => (
                format!("{} (cert)", cert.algorithm().as_str()),
                cert.public_key().fingerprint(HashAlg::Sha256).to_string(),
            ),
        };
        if self.known.as_deref() == Some(fingerprint.as_str()) {
            return Ok(true);
        }
        let (tx, rx) = oneshot::channel();
        self.prompts.send(HostKeyPrompt {
            host: self.host_id.clone(),
            algorithm,
            fingerprint,
            previous: self.known.clone(),
            reply: tx,
        });
        Ok(rx.await.unwrap_or(false))
    }
}

pub type SshHandle = client::Handle<Client>;

/// Connects and authenticates (through the jump host, if set). Tries public key, then
/// password, then keyboard-interactive.
pub fn connect(
    p: &SshParams,
    prompts: Sink<HostKeyPrompt>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SshHandle>> + Send + '_>> {
    Box::pin(async move {
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 4,
            ..Default::default()
        });
        let mut session = match &p.jump {
            None => {
                let handler = Client {
                    host_id: p.host_id(),
                    known: p.known_fingerprint.clone(),
                    prompts: prompts.clone(),
                    _via: None,
                };
                let addr = (p.host.as_str(), p.port);
                tokio::time::timeout(
                    Duration::from_secs(20),
                    client::connect(config, addr, handler),
                )
                .await
                .map_err(|_| anyhow!(t!("Zeitüberschreitung – Host nicht erreichbar")))?
                .map_err(friendly_error)?
            }
            Some(jump) => {
                let via = Arc::new(connect(jump, prompts.clone()).await.map_err(|e| {
                    anyhow!("{}", t!("Jump-Host {}: {}", jump.host, format!("{e:#}")))
                })?);
                let channel = via
                    .channel_open_direct_tcpip(p.host.clone(), p.port as u32, "127.0.0.1", 0)
                    .await
                    .map_err(|e| {
                        anyhow!(
                            "{}",
                            t!(
                                "Jump-Host {} erreicht {}:{} nicht ({})",
                                jump.host,
                                p.host,
                                p.port,
                                e
                            )
                        )
                    })?;
                let handler = Client {
                    host_id: p.host_id(),
                    known: p.known_fingerprint.clone(),
                    prompts: prompts.clone(),
                    _via: Some(via),
                };
                tokio::time::timeout(
                    Duration::from_secs(20),
                    client::connect_stream(config, channel.into_stream(), handler),
                )
                .await
                .map_err(|_| anyhow!(t!("Zeitüberschreitung über den Jump-Host")))?
                .map_err(friendly_error)?
            }
        };

        let mut offered: Vec<russh::MethodKind> = Vec::new();
        let mut key_rejected = false;
        if let Some(key_text) = p.private_key.as_deref().filter(|k| !k.trim().is_empty()) {
            let key =
                decode_secret_key(key_text, p.passphrase.as_deref().filter(|s| !s.is_empty()))
                    .map_err(|e| {
                        anyhow!(
                            "{}",
                            t!(
                                "SSH-Key konnte nicht gelesen werden – stimmt die Passphrase? ({})",
                                e
                            )
                        )
                    })?;
            let hash = session.best_supported_rsa_hash().await?.flatten();
            match session
                .authenticate_publickey(
                    &p.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await?
            {
                client::AuthResult::Success => return Ok(session),
                client::AuthResult::Failure {
                    remaining_methods, ..
                } => {
                    key_rejected = true;
                    offered = remaining_methods.to_vec();
                }
            }
        }

        if let Some(pw) = p.password.as_deref() {
            match session.authenticate_password(&p.username, pw).await? {
                client::AuthResult::Success => return Ok(session),
                client::AuthResult::Failure {
                    remaining_methods, ..
                } => offered = remaining_methods.to_vec(),
            }
            // Many servers only offer keyboard-interactive for passwords.
            let mut resp = session
                .authenticate_keyboard_interactive_start(&p.username, None)
                .await?;
            for _ in 0..5 {
                match resp {
                    KeyboardInteractiveAuthResponse::Success => return Ok(session),
                    KeyboardInteractiveAuthResponse::Failure { .. } => break,
                    KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                        let answers = prompts.iter().map(|_| pw.to_string()).collect();
                        resp = session
                            .authenticate_keyboard_interactive_respond(answers)
                            .await?;
                    }
                }
            }
        }

        bail!(auth_hint(
            &p.username,
            &p.host,
            p.password.is_some(),
            key_rejected,
            &offered
        ))
    })
}

/// Explains a failed login in words, with the next step.
fn auth_hint(
    user: &str,
    host: &str,
    tried_password: bool,
    key_rejected: bool,
    offered: &[russh::MethodKind],
) -> String {
    use russh::MethodKind;
    let password_allowed = offered.is_empty()
        || offered
            .iter()
            .any(|m| matches!(m, MethodKind::Password | MethodKind::KeyboardInteractive));
    let key_allowed = offered.is_empty() || offered.contains(&MethodKind::PublicKey);
    if tried_password && !password_allowed {
        t!(
            "{} erlaubt keine Anmeldung per Passwort – nur per SSH-Key. Hinterlege einen Key in der Identität des Hosts.",
            host
        )
    } else if key_rejected && !tried_password {
        t!(
            "Der SSH-Key wurde von {} für „{}“ nicht angenommen – ist er in ~/.ssh/authorized_keys eingetragen? (Rechtsklick → SSH-Key einrichten)",
            host,
            user
        )
    } else if !key_allowed && !tried_password {
        t!("{} erwartet ein Passwort für „{}“", host, user)
    } else {
        t!(
            "Anmeldung als „{}“ an {} abgelehnt – Benutzername oder Passwort falsch? (root-Login ist auf vielen Servern gesperrt)",
            user,
            host
        )
    }
}

/// Turns low-level connection errors into short, actionable messages.
pub fn friendly_error(e: anyhow::Error) -> anyhow::Error {
    let raw = format!("{e:#}");
    anyhow!(friendly_text(&raw).unwrap_or(raw))
}

pub fn friendly_text(raw: &str) -> Option<String> {
    let has = |s: &str| raw.contains(s);
    let msg = if has("UnknownKey") {
        t!("Server-Key wurde abgelehnt")
    } else if has("Connection refused") || has("os error 111") || has("os error 10061") {
        t!("Verbindung abgelehnt – läuft der SSH-Dienst auf diesem Port?")
    } else if has("failed to lookup")
        || has("Name or service not known")
        || has("No such host")
        || has("nodename nor servname")
        || has("os error 11001")
    {
        t!("Hostname nicht gefunden – Tippfehler oder fehlt ein DNS-Eintrag?")
    } else if has("Network is unreachable") || has("No route to host") || has("os error 10065") {
        t!("Netzwerk nicht erreichbar – bist du im richtigen Netz / VPN?")
    } else if has("Connection reset") || has("os error 104") || has("os error 10054") {
        t!("Verbindung vom Server zurückgesetzt – evtl. Firewall oder fail2ban")
    } else if has("timed out") || has("os error 110") || has("os error 10060") {
        t!("Zeitüberschreitung – Host antwortet nicht (aus, Firewall oder falscher Port?)")
    } else if has("NoCommonKexAlgo") || has("NoCommonCipher") || has("NoCommonKeyAlgo") {
        t!("Server und Aurora haben keine gemeinsamen Verfahren – sehr alter SSH-Server?")
    } else if has("Disconnect") && has("Too many authentication failures") {
        t!("Zu viele Anmeldeversuche – der Server hat die Verbindung getrennt")
    } else if has("No such file") {
        t!("Datei oder Ordner existiert nicht")
    } else if has("Permission denied") {
        t!("Keine Berechtigung")
    } else {
        return None;
    };
    Some(msg.to_string())
}

/// Starts an interactive shell session (or `command` in a terminal, e.g.
/// `docker exec -it …`); returns the command sender.
pub fn spawn_shell(
    rt: &tokio::runtime::Runtime,
    params: SshParams,
    command: Option<String>,
    cols: u16,
    rows: u16,
    events: Sink<TermEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> mpsc::UnboundedSender<TermCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(async move {
        let result = run_shell(&params, command, cols, rows, rx, &events, prompts).await;
        events.send(TermEvent::Closed(result.err().map(|e| format!("{e:#}"))));
    });
    tx
}

async fn run_shell(
    p: &SshParams,
    command: Option<String>,
    cols: u16,
    rows: u16,
    mut cmds: mpsc::UnboundedReceiver<TermCmd>,
    events: &Sink<TermEvent>,
    prompts: Sink<HostKeyPrompt>,
) -> Result<()> {
    events.send(TermEvent::Status(match &p.jump {
        Some(j) => t!("Verbinde mit {} über {}…", p.host_id(), j.host),
        None => t!("Verbinde mit {}…", p.host_id()),
    }));
    let handle = Arc::new(connect(p, prompts).await?);
    events.send(TermEvent::Status(t!("Öffne Shell…").into()));
    let channel = handle.channel_open_session().await?;
    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await?;
    match &command {
        Some(cmd) => channel.exec(false, cmd.as_str()).await?,
        None => channel.request_shell(false).await?,
    }
    events.send(TermEvent::Connected);
    if command.is_none() {
        // Facts for the host list run on a separate channel of the same connection.
        let (handle, events) = (handle.clone(), events.clone());
        tokio::spawn(async move {
            if let Some(f) = collect_facts(&handle).await {
                events.send(TermEvent::Facts(f));
            }
        });
    }

    let (mut reader, writer) = channel.split();
    let mut status = None;
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(TermCmd::Input(bytes)) => writer.data_bytes(bytes).await?,
                Some(TermCmd::Resize { cols, rows }) => {
                    writer.window_change(cols as u32, rows as u32, 0, 0).await?
                }
                Some(TermCmd::Upload { files, dir }) => {
                    let (handle, events) = (handle.clone(), events.clone());
                    tokio::spawn(async move {
                        let r = upload(&handle, files, dir, &events).await;
                        events.send(TermEvent::UploadDone(r.map_err(|e| format!("{e:#}"))));
                    });
                }
                Some(TermCmd::Close) | None => {
                    let _ = writer.close().await;
                    break;
                }
            },
            msg = reader.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    events.send(TermEvent::Output(data.to_vec()));
                }
                Some(ChannelMsg::ExitStatus { exit_status }) if exit_status != 0 && command.is_some() => {
                    status = Some(exit_status);
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
        }
    }
    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "en")
        .await;
    match status {
        Some(code) => bail!("{}", t!("Beendet mit Code {}", code)),
        None => Ok(()),
    }
}

/// Read-only system facts: os-release, hostname, uptime, root disk, pending updates.
/// Runs under `sh` so it works whatever the login shell is.
const FACTS_SCRIPT: &str = r#"sh -c '. /etc/os-release 2>/dev/null; echo "id=$ID"; echo "name=$PRETTY_NAME"; echo "host=$(hostname 2>/dev/null || cat /etc/hostname 2>/dev/null)"; echo "up=$(cut -d. -f1 /proc/uptime 2>/dev/null)"; echo "disk=$(df -P / 2>/dev/null | awk "NR==2{print \$5}")"; command -v pveversion >/dev/null 2>&1 && echo pve=1; if command -v apt-get >/dev/null 2>&1; then echo "upd=$(apt-get -s -o Debug::NoLocking=1 upgrade 2>/dev/null | grep -c "^Inst ")"; elif command -v dnf >/dev/null 2>&1; then echo "upd=$(dnf -q --cacheonly check-update 2>/dev/null | grep -c "^[a-zA-Z0-9]")"; fi'"#;

async fn collect_facts(handle: &SshHandle) -> Option<crate::model::HostFacts> {
    let mut facts = match exec(handle, FACTS_SCRIPT).await {
        Ok((_, out)) => crate::model::HostFacts::parse(&out),
        Err(_) => None,
    };
    if facts.is_none() {
        // Windows OpenSSH runs commands in cmd.exe.
        if let Ok((_, out)) = exec(handle, "ver & hostname").await {
            facts = crate::model::HostFacts::parse_windows(&out);
        }
    }
    let mut f = facts?;
    f.collected = Some(chrono::Utc::now());
    Some(f)
}

/// Runs one command on its own channel; returns (exit status, stdout), output capped at 64 KiB.
pub async fn exec(handle: &SshHandle, cmd: &str) -> Result<(u32, String)> {
    let run = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        let mut out = Vec::new();
        let mut status = 0;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() < 64 * 1024 {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => status = exit_status,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok((status, String::from_utf8_lossy(&out).into_owned()))
    };
    tokio::time::timeout(Duration::from_secs(25), run)
        .await
        .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
}

/// Puts `public` into `~/.ssh/authorized_keys` (password login), then proves that a
/// login with the matching private key works.
pub async fn install_key(
    p: SshParams,
    public: String,
    private: String,
    prompts: Sink<HostKeyPrompt>,
) -> Result<()> {
    let line = public.trim();
    // The key goes into a shell command: only the characters a public key line uses.
    let safe = line.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '@' | '.' | '_' | '-' | ' ')
    });
    if !safe || !line.starts_with("ssh-") {
        bail!(t!("Ungültiger öffentlicher Schlüssel"));
    }
    let handle = connect(&p, prompts.clone()).await?;
    let cmd = format!(
        "sh -c 'umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && (grep -qxF \"{line}\" ~/.ssh/authorized_keys || echo \"{line}\" >> ~/.ssh/authorized_keys)'"
    );
    let (status, _) = exec(&handle, &cmd).await?;
    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "en")
        .await;
    if status != 0 {
        bail!(
            "{}",
            t!(
                "Server hat den Schlüssel nicht übernommen (nur Linux/Unix-Server, Exit {})",
                status
            )
        );
    }
    // Verify: key only, no password fallback.
    let check = SshParams {
        password: None,
        private_key: Some(private),
        passphrase: None,
        ..p
    };
    let handle = connect(&check, prompts).await.map_err(|e| {
        anyhow!(
            "{}",
            t!(
                "Schlüssel eingetragen, Anmeldung damit klappt aber nicht: {}",
                format!("{e:#}")
            )
        )
    })?;
    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "en")
        .await;
    Ok(())
}

/// Files dropped onto a terminal: a second channel on the same connection runs SFTP.
async fn upload(
    handle: &SshHandle,
    files: Vec<std::path::PathBuf>,
    dir: Option<String>,
    events: &Sink<TermEvent>,
) -> Result<String> {
    use super::sftp::{SftpEvent, join, upload};
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
    let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
    let target = match dir.as_deref() {
        None | Some("~") => home.clone(),
        Some(d) if d.starts_with("~/") => join(&home, &d[2..]),
        Some(d) if d.starts_with('/') => d.to_string(),
        Some(d) => join(&home, d),
    };
    // A prompt title can be stale or abbreviated; fall back to home.
    let target = if sftp.try_exists(target.clone()).await.unwrap_or(false) {
        target
    } else {
        home
    };

    let (progress, mut rx) = Sink::<SftpEvent>::new(events.ctx());
    let fwd = events.clone();
    let forward = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let SftpEvent::Progress {
                name, done, total, ..
            } = ev
            {
                fwd.send(TermEvent::UploadProgress { name, done, total });
            }
        }
    });
    let mut result = Ok(());
    for f in &files {
        result = upload(&sftp, f, &target, 0, &progress).await;
        if result.is_err() {
            break;
        }
    }
    drop(progress);
    let _ = forward.await;
    let _ = sftp.close().await;
    result.map(|_| target)
}
