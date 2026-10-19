//! Password-manager adapters for Bitwarden (`bw`), 1Password (`op`) and KeePassXC
//! (`keepassxc-cli`) – plus the common entry points used by the UI. Proton Pass lives in
//! `pwmanager_proton.rs`. No network code of our own: everything goes through the CLIs,
//! spawned without a shell (argument arrays), with validated references.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use super::pwmanager_proton as proton;
use super::pwmanager_run::{Out, Run, classify, has_control, program_for, run};
use super::pwmanager_types::{
    CredentialSource, PwmError, PwmItem, PwmProvider, PwmSettings, Resolved, Secret, SourceKind,
};

// ---------------------------------------------------------------- sessions

struct Stamped {
    secret: Secret,
    last: Instant,
}

#[derive(Default)]
struct Inner {
    bw: Option<Stamped>,
    kp: Option<Stamped>,
}

/// The unlock sessions (Bitwarden session key, KeePass database password), in memory only.
/// They expire after `timeout_minutes` without use and are wiped on drop / `lock_all`.
#[derive(Default)]
pub struct Sessions {
    inner: Mutex<Inner>,
}

/// Has a session last used at `last` run out at `now`? `minutes == 0` = never.
pub fn expired(last: Instant, now: Instant, minutes: u32) -> bool {
    minutes != 0 && now.saturating_duration_since(last) >= Duration::from_secs(minutes as u64 * 60)
}

impl Sessions {
    fn slot(g: &mut Inner, p: PwmProvider) -> Option<&mut Option<Stamped>> {
        match p {
            PwmProvider::Bitwarden => Some(&mut g.bw),
            PwmProvider::KeePassXc => Some(&mut g.kp),
            _ => None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The live session of `p` (refreshing its idle timer), or `None` if absent/expired.
    pub fn get_at(&self, p: PwmProvider, minutes: u32, now: Instant) -> Option<Secret> {
        let mut g = self.lock();
        let slot = Self::slot(&mut g, p)?;
        match slot {
            Some(s) if expired(s.last, now, minutes) => {
                *slot = None;
                None
            }
            Some(s) => {
                s.last = now;
                Some(s.secret.clone())
            }
            None => None,
        }
    }

    pub fn get(&self, p: PwmProvider, minutes: u32) -> Option<Secret> {
        self.get_at(p, minutes, Instant::now())
    }

    pub fn set(&self, p: PwmProvider, secret: Secret) {
        let mut g = self.lock();
        if let Some(slot) = Self::slot(&mut g, p) {
            *slot = Some(Stamped {
                secret,
                last: Instant::now(),
            });
        }
    }

    pub fn clear(&self, p: PwmProvider) {
        let mut g = self.lock();
        if let Some(slot) = Self::slot(&mut g, p) {
            *slot = None;
        }
    }

    /// Forgets every session (Aurora locks).
    pub fn lock_all(&self) {
        let mut g = self.lock();
        g.bw = None;
        g.kp = None;
    }

    pub fn is_unlocked(&self, p: PwmProvider, minutes: u32) -> bool {
        self.get(p, minutes).is_some()
    }
}

// ---------------------------------------------------------------- context

/// Everything a worker thread needs.
#[derive(Clone)]
pub struct Ctx {
    pub settings: PwmSettings,
    pub sessions: Arc<Sessions>,
    pub timeout: Duration,
}

impl Ctx {
    pub fn new(settings: PwmSettings, sessions: Arc<Sessions>) -> Self {
        Self {
            settings,
            sessions,
            timeout: super::pwmanager_run::TIMEOUT,
        }
    }

    fn minutes(&self) -> u32 {
        self.settings.timeout_minutes
    }

    fn program(&self, p: PwmProvider) -> Result<PathBuf, PwmError> {
        if !self.settings.enabled(p) {
            return Err(PwmError::Disabled(p));
        }
        program_for(p, self.settings.path(p))
    }

    /// Runs the CLI of `p`; the caller looks at the exit code.
    pub fn exec(
        &self,
        p: PwmProvider,
        args: Vec<String>,
        env: Vec<(&'static str, Secret)>,
        stdin: Option<Secret>,
    ) -> Result<Out, PwmError> {
        let mut r = Run::new(self.program(p)?, args);
        r.env = env;
        r.stdin = stdin;
        r.timeout = self.timeout;
        run(r)
    }

    /// Like `exec`, a non-zero exit becomes a classified error.
    pub fn exec_ok(
        &self,
        p: PwmProvider,
        args: Vec<String>,
        env: Vec<(&'static str, Secret)>,
        stdin: Option<Secret>,
    ) -> Result<Out, PwmError> {
        let out = self.exec(p, args, env, stdin)?;
        if out.ok() {
            Ok(out)
        } else {
            Err(classify(p, &out))
        }
    }
}

// ---------------------------------------------------------------- validation

/// Item references and user-supplied strings that end up as CLI arguments.
fn plain_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !has_control(s) && !s.starts_with('-') && s.trim() == s
}

/// Is `item` an acceptable opaque reference for `p`? Rejects option injection (leading
/// `-`), control characters and, where the format has none, spaces.
pub fn validate_ref(p: PwmProvider, item: &str) -> Result<(), PwmError> {
    let ok = match p {
        PwmProvider::Bitwarden => {
            item.len() <= 64
                && !item.is_empty()
                && item.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !item.starts_with('-')
        }
        PwmProvider::OnePassword => {
            if let Some(rest) = item.strip_prefix("op://") {
                plain_ok(item, 512) && rest.contains('/') && !rest.starts_with('/')
            } else {
                !item.is_empty()
                    && item.len() <= 64
                    && item
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    && !item.starts_with('-')
            }
        }
        PwmProvider::KeePassXc => plain_ok(item, 512),
        PwmProvider::ProtonPass => proton::valid_ref(item),
    };
    if ok {
        Ok(())
    } else {
        Err(PwmError::BadReference)
    }
}

/// Search text for the CLIs: no control characters, trimmed, at most 100 characters.
pub fn clean_query(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(100)
        .collect()
}

fn account_args(ctx: &Ctx) -> Result<Vec<String>, PwmError> {
    let a = ctx.settings.op_account.trim();
    if a.is_empty() {
        return Ok(Vec::new());
    }
    if !plain_ok(a, 128) || a.contains(' ') {
        return Err(PwmError::BadConfig);
    }
    Ok(vec!["--account".into(), a.to_string()])
}

// ---------------------------------------------------------------- shared helpers

/// Turns the pieces of an item into what a connection needs.
pub fn build_resolved(
    kind: SourceKind,
    use_username: bool,
    user: Option<String>,
    password: Option<String>,
    notes: Option<String>,
    key_field: Option<String>,
) -> Result<Resolved, PwmError> {
    let mut r = Resolved::default();
    if use_username {
        r.username = user.filter(|u| !u.trim().is_empty());
    }
    match kind {
        SourceKind::Password => {
            let pw = password
                .filter(|p| !p.is_empty())
                .ok_or(PwmError::NoSecret)?;
            r.password = Some(Secret::new(pw));
        }
        SourceKind::PrivateKey => {
            let key = key_field
                .filter(|k| k.contains("PRIVATE KEY"))
                .or_else(|| notes.filter(|n| n.contains("PRIVATE KEY")))
                .ok_or(PwmError::NoSecret)?;
            let mut key = key.replace("\r\n", "\n").trim().to_string();
            key.push('\n');
            r.private_key = Some(Secret::new(key));
            r.passphrase = password.filter(|p| !p.is_empty()).map(Secret::new);
        }
    }
    Ok(r)
}

fn jstr(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for k in path {
        match cur.get(*k) {
            Some(n) => cur = n,
            None => return String::new(),
        }
    }
    cur.as_str().unwrap_or_default().to_string()
}

fn cap(mut items: Vec<PwmItem>) -> Vec<PwmItem> {
    items.sort_by_key(|i| i.name.to_lowercase());
    items.truncate(50);
    items
}

// ---------------------------------------------------------------- Bitwarden

fn bw_json(args: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    v.push("--nointeraction".into());
    v
}

#[derive(Debug, PartialEq)]
pub enum BwState {
    Unlocked,
    Locked,
    LoggedOut,
}

pub fn parse_bw_status(json: &str) -> Result<BwState, PwmError> {
    let v: Value = serde_json::from_str(json.trim()).map_err(|_| PwmError::BadOutput)?;
    match v.get("status").and_then(Value::as_str) {
        Some("unlocked") => Ok(BwState::Unlocked),
        Some("locked") => Ok(BwState::Locked),
        Some("unauthenticated") => Ok(BwState::LoggedOut),
        _ => Err(PwmError::BadOutput),
    }
}

fn bw_state(ctx: &Ctx) -> Result<BwState, PwmError> {
    let env = ctx
        .sessions
        .get(PwmProvider::Bitwarden, ctx.minutes())
        .map(|s| vec![("BW_SESSION", s)])
        .unwrap_or_default();
    let out = ctx.exec_ok(
        PwmProvider::Bitwarden,
        bw_json(&["status", "--raw"]),
        env,
        None,
    )?;
    parse_bw_status(out.stdout.expose())
}

/// Unlocks the vault with the master password; the password goes through the environment
/// of the child (`--passwordenv`), never argv. Keeps the session key in memory.
pub fn bw_unlock(ctx: &Ctx, master: Secret) -> Result<(), PwmError> {
    match bw_state(ctx)? {
        BwState::LoggedOut => return Err(PwmError::LoggedOut(PwmProvider::Bitwarden)),
        BwState::Unlocked | BwState::Locked => {}
    }
    let out = ctx.exec_ok(
        PwmProvider::Bitwarden,
        bw_json(&["unlock", "--passwordenv", "BW_PASSWORD", "--raw"]),
        vec![("BW_PASSWORD", master)],
        None,
    )?;
    let key = out.stdout.expose().trim().to_string();
    let valid = !key.is_empty()
        && key.len() <= 1024
        && key.chars().all(|c| !c.is_whitespace() && !c.is_control());
    if !valid {
        return Err(PwmError::BadOutput);
    }
    ctx.sessions.set(PwmProvider::Bitwarden, Secret::new(key));
    Ok(())
}

fn bw_run(ctx: &Ctx, args: Vec<String>) -> Result<Out, PwmError> {
    let p = PwmProvider::Bitwarden;
    let Some(session) = ctx.sessions.get(p, ctx.minutes()) else {
        return match bw_state(ctx)? {
            BwState::LoggedOut => Err(PwmError::LoggedOut(p)),
            _ => Err(PwmError::Locked(p)),
        };
    };
    ctx.exec_ok(p, args, vec![("BW_SESSION", session)], None)
        .inspect_err(|e| {
            if matches!(e, PwmError::Locked(_)) {
                ctx.sessions.clear(p);
            }
        })
}

pub fn parse_bw_folders(json: &str) -> HashMap<String, String> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else {
        return HashMap::new();
    };
    list.iter()
        .filter_map(|f| {
            let id = jstr(f, &["id"]);
            (!id.is_empty()).then(|| (id, jstr(f, &["name"])))
        })
        .collect()
}

pub fn parse_bw_items(
    json: &str,
    folders: &HashMap<String, String>,
) -> Result<Vec<PwmItem>, PwmError> {
    let v: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let list = v.as_array().ok_or(PwmError::BadOutput)?;
    let mut out = Vec::new();
    for it in list {
        let id = jstr(it, &["id"]);
        if validate_ref(PwmProvider::Bitwarden, &id).is_err() {
            continue;
        }
        // 1 login, 2 secure note (keys), 5 SSH key; skip cards and identities.
        if let Some(t) = it.get("type").and_then(Value::as_i64)
            && !matches!(t, 1 | 2 | 5)
        {
            continue;
        }
        out.push(PwmItem {
            id,
            name: jstr(it, &["name"]),
            username: jstr(it, &["login", "username"]),
            folder: folders
                .get(&jstr(it, &["folderId"]))
                .cloned()
                .unwrap_or_default(),
        });
    }
    Ok(out)
}

pub fn parse_bw_item(
    json: &str,
    kind: SourceKind,
    use_username: bool,
) -> Result<Resolved, PwmError> {
    let v: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let opt = |s: String| (!s.is_empty()).then_some(s);
    build_resolved(
        kind,
        use_username,
        opt(jstr(&v, &["login", "username"])),
        opt(jstr(&v, &["login", "password"])),
        opt(jstr(&v, &["notes"])),
        opt(jstr(&v, &["sshKey", "privateKey"])),
    )
}

fn bw_search(ctx: &Ctx, text: &str) -> Result<Vec<PwmItem>, PwmError> {
    let q = clean_query(text);
    let mut args = vec!["list".to_string(), "items".to_string()];
    if !q.is_empty() {
        args.push(format!("--search={q}"));
    }
    args.push("--nointeraction".into());
    let out = bw_run(ctx, args)?;
    let folders = bw_run(ctx, bw_json(&["list", "folders"]))
        .map(|o| parse_bw_folders(o.stdout.expose()))
        .unwrap_or_default();
    Ok(cap(parse_bw_items(out.stdout.expose(), &folders)?))
}

fn bw_fetch(
    ctx: &Ctx,
    id: &str,
    kind: SourceKind,
    use_username: bool,
) -> Result<Resolved, PwmError> {
    validate_ref(PwmProvider::Bitwarden, id)?;
    let out = bw_run(ctx, bw_json(&["get", "item", id]))?;
    parse_bw_item(out.stdout.expose(), kind, use_username)
}

// ---------------------------------------------------------------- 1Password

pub fn parse_op_items(json: &str) -> Result<Vec<PwmItem>, PwmError> {
    let v: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let list = v.as_array().ok_or(PwmError::BadOutput)?;
    Ok(list
        .iter()
        .filter_map(|it| {
            let id = jstr(it, &["id"]);
            validate_ref(PwmProvider::OnePassword, &id).ok()?;
            Some(PwmItem {
                id,
                name: jstr(it, &["title"]),
                username: jstr(it, &["additional_information"]),
                folder: jstr(it, &["vault", "name"]),
            })
        })
        .collect())
}

pub fn parse_op_item(
    json: &str,
    kind: SourceKind,
    use_username: bool,
) -> Result<Resolved, PwmError> {
    let v: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let fields = v
        .get("fields")
        .and_then(Value::as_array)
        .ok_or(PwmError::BadOutput)?;
    let (mut user, mut pw, mut notes, mut key) = (None, None, None, None);
    for f in fields {
        let purpose = jstr(f, &["purpose"]);
        let label = jstr(f, &["label"]).to_lowercase();
        let ty = jstr(f, &["type"]).to_uppercase();
        let value = jstr(f, &["value"]);
        if value.is_empty() && ty != "SSHKEY" {
            continue;
        }
        if purpose == "USERNAME" || (user.is_none() && label == "username") {
            user = Some(value);
        } else if purpose == "PASSWORD" || (pw.is_none() && label == "password") {
            pw = Some(value);
        } else if purpose == "NOTES" || label == "notesplain" || label == "notes" {
            notes = Some(value);
        } else if ty == "SSHKEY" || label == "private key" {
            let open = jstr(f, &["ssh_formats", "openssh", "value"]);
            key = Some(if open.is_empty() { value } else { open });
        }
    }
    build_resolved(kind, use_username, user, pw, notes, key)
}

fn op_search(ctx: &Ctx, text: &str) -> Result<Vec<PwmItem>, PwmError> {
    let mut args: Vec<String> = ["item", "list", "--format", "json"]
        .map(String::from)
        .to_vec();
    args.extend(account_args(ctx)?);
    let out = ctx.exec_ok(PwmProvider::OnePassword, args, Vec::new(), None)?;
    let q = clean_query(text).to_lowercase();
    let items = parse_op_items(out.stdout.expose())?
        .into_iter()
        .filter(|i| {
            q.is_empty()
                || i.name.to_lowercase().contains(&q)
                || i.username.to_lowercase().contains(&q)
                || i.folder.to_lowercase().contains(&q)
        })
        .collect();
    Ok(cap(items))
}

fn op_fetch(
    ctx: &Ctx,
    item: &str,
    kind: SourceKind,
    use_username: bool,
) -> Result<Resolved, PwmError> {
    validate_ref(PwmProvider::OnePassword, item)?;
    let p = PwmProvider::OnePassword;
    if item.starts_with("op://") {
        let mut args = vec!["read".to_string(), "--no-newline".to_string()];
        args.extend(account_args(ctx)?);
        args.push("--".into());
        args.push(item.to_string());
        let out = ctx.exec_ok(p, args, Vec::new(), None)?;
        let text = out.stdout.expose().to_string();
        return match kind {
            SourceKind::Password => build_resolved(kind, false, None, Some(text), None, None),
            SourceKind::PrivateKey => build_resolved(kind, false, None, None, None, Some(text)),
        };
    }
    let mut args: Vec<String> = ["item", "get", "--format", "json", "--reveal"]
        .map(String::from)
        .to_vec();
    args.extend(account_args(ctx)?);
    args.push("--".into());
    args.push(item.to_string());
    let out = ctx.exec_ok(p, args, Vec::new(), None)?;
    parse_op_item(out.stdout.expose(), kind, use_username)
}

// ---------------------------------------------------------------- KeePassXC

fn kp_db(ctx: &Ctx) -> Result<(Vec<String>, String), PwmError> {
    let db = ctx.settings.kp_db.trim();
    if !plain_ok(db, 1024) {
        return Err(PwmError::BadConfig);
    }
    let mut opts = vec!["-q".to_string()];
    let kf = ctx.settings.kp_keyfile.trim();
    if !kf.is_empty() {
        if !plain_ok(kf, 1024) {
            return Err(PwmError::BadConfig);
        }
        opts.push("-k".into());
        opts.push(kf.to_string());
    }
    Ok((opts, db.to_string()))
}

fn kp_exec(
    ctx: &Ctx,
    password: &Secret,
    cmd: &str,
    extra: &[&str],
    tail: &[&str],
) -> Result<Out, PwmError> {
    let (opts, db) = kp_db(ctx)?;
    let mut args = vec![cmd.to_string()];
    args.extend(opts);
    args.extend(extra.iter().map(|s| s.to_string()));
    args.push("--".into());
    args.push(db);
    args.extend(tail.iter().map(|s| s.to_string()));
    ctx.exec_ok(
        PwmProvider::KeePassXc,
        args,
        Vec::new(),
        Some(password.clone()),
    )
}

fn kp_pw(ctx: &Ctx) -> Result<Secret, PwmError> {
    ctx.sessions
        .get(PwmProvider::KeePassXc, ctx.minutes())
        .ok_or(PwmError::Locked(PwmProvider::KeePassXc))
}

/// Checks the database password (opens the database once) and keeps it in memory.
pub fn kp_unlock(ctx: &Ctx, password: Secret) -> Result<(), PwmError> {
    kp_exec(ctx, &password, "ls", &[], &[])?;
    ctx.sessions.set(PwmProvider::KeePassXc, password);
    Ok(())
}

pub fn parse_kp_search(stdout: &str) -> Vec<PwmItem> {
    stdout
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| l.starts_with('/') && l.len() > 1 && !has_control(l))
        .map(|l| {
            let path = l.trim_end_matches('/');
            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
            PwmItem {
                id: path.to_string(),
                name: name.to_string(),
                username: String::new(),
                folder: folder.trim_start_matches('/').to_string(),
            }
        })
        .collect()
}

fn kp_search(ctx: &Ctx, text: &str) -> Result<Vec<PwmItem>, PwmError> {
    let pw = kp_pw(ctx)?;
    let q = clean_query(text);
    let out = if q.is_empty() {
        kp_exec(ctx, &pw, "ls", &["-R", "-f"], &[])?
    } else {
        kp_exec(ctx, &pw, "search", &[], &[&q])?
    };
    let mut items = parse_kp_search(out.stdout.expose());
    if q.is_empty() {
        // `ls -R -f` lists groups too (trailing `/` is stripped above): keep entries only.
        items.retain(|i| !i.name.is_empty());
    }
    Ok(cap(items))
}

/// One attribute of an entry (single `-a`, so the output is the bare value).
fn kp_attr(ctx: &Ctx, pw: &Secret, entry: &str, attr: &str) -> Result<Option<String>, PwmError> {
    let out = kp_exec(ctx, pw, "show", &["-s", "-a", attr], &[entry])?;
    let text = out.stdout.expose();
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    Ok((!text.is_empty()).then(|| text.to_string()))
}

fn kp_fetch(
    ctx: &Ctx,
    entry: &str,
    kind: SourceKind,
    use_username: bool,
) -> Result<Resolved, PwmError> {
    validate_ref(PwmProvider::KeePassXc, entry)?;
    let pw = kp_pw(ctx)?;
    let user = if use_username {
        kp_attr(ctx, &pw, entry, "UserName")?
    } else {
        None
    };
    let password = kp_attr(ctx, &pw, entry, "Password")?;
    let notes = match kind {
        SourceKind::PrivateKey => kp_attr(ctx, &pw, entry, "Notes")?,
        SourceKind::Password => None,
    };
    build_resolved(kind, use_username, user, password, notes, None)
}

// ---------------------------------------------------------------- common entry points

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    NotInstalled,
    Disabled,
    Locked,
    LoggedOut,
    Ready,
    Error(PwmError),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::NotInstalled => t!("nicht gefunden").to_string(),
            Status::Disabled => t!("aus").to_string(),
            Status::Locked => t!("gesperrt").to_string(),
            Status::LoggedOut => t!("nicht angemeldet").to_string(),
            Status::Ready => t!("bereit").to_string(),
            Status::Error(_) => t!("Fehler").to_string(),
        }
    }
}

/// Is `p` usable right now? (Installed? Unlocked?) Does not fail for a disabled provider.
pub fn status(ctx: &Ctx, p: PwmProvider) -> Status {
    if !ctx.settings.enabled(p) {
        return Status::Disabled;
    }
    let r: Result<Status, PwmError> = (|| match p {
        PwmProvider::Bitwarden => Ok(match bw_state(ctx)? {
            BwState::Unlocked | BwState::Locked if ctx.sessions.is_unlocked(p, ctx.minutes()) => {
                Status::Ready
            }
            BwState::Unlocked | BwState::Locked => Status::Locked,
            BwState::LoggedOut => Status::LoggedOut,
        }),
        PwmProvider::OnePassword => {
            let mut args: Vec<String> = ["whoami", "--format", "json"].map(String::from).to_vec();
            args.extend(account_args(ctx)?);
            let out = ctx.exec(p, args, Vec::new(), None)?;
            if out.ok() {
                Ok(Status::Ready)
            } else {
                match classify(p, &out) {
                    PwmError::Locked(_) | PwmError::LoggedOut(_) => Ok(Status::Locked),
                    e => Err(e),
                }
            }
        }
        PwmProvider::KeePassXc => {
            ctx.program(p)?;
            kp_db(ctx)?;
            Ok(if ctx.sessions.is_unlocked(p, ctx.minutes()) {
                Status::Ready
            } else {
                Status::Locked
            })
        }
        PwmProvider::ProtonPass => match proton::status(ctx) {
            Ok(()) => Ok(Status::Ready),
            Err(PwmError::LoggedOut(_)) => Ok(Status::LoggedOut),
            Err(e) => Err(e),
        },
    })();
    match r {
        Ok(s) => s,
        Err(PwmError::NotInstalled(_)) => Status::NotInstalled,
        Err(PwmError::LoggedOut(_)) => Status::LoggedOut,
        Err(PwmError::Locked(_)) => Status::Locked,
        Err(e) => Status::Error(e),
    }
}

/// Unlocks a provider that asks for a password in Aurora's dialog.
pub fn unlock(ctx: &Ctx, p: PwmProvider, password: Secret) -> Result<(), PwmError> {
    match p {
        PwmProvider::Bitwarden => bw_unlock(ctx, password),
        PwmProvider::KeePassXc => kp_unlock(ctx, password),
        _ => Err(PwmError::BadConfig),
    }
}

pub fn search(ctx: &Ctx, p: PwmProvider, text: &str) -> Result<Vec<PwmItem>, PwmError> {
    ctx.program(p)?;
    match p {
        PwmProvider::Bitwarden => bw_search(ctx, text),
        PwmProvider::OnePassword => op_search(ctx, text),
        PwmProvider::KeePassXc => kp_search(ctx, text),
        PwmProvider::ProtonPass => proton::search(ctx, text).map(cap),
    }
}

pub fn fetch(ctx: &Ctx, src: &CredentialSource) -> Result<Resolved, PwmError> {
    let p = src.provider;
    validate_ref(p, &src.item)?;
    ctx.program(p)?;
    match p {
        PwmProvider::Bitwarden => bw_fetch(ctx, &src.item, src.kind, src.use_username),
        PwmProvider::OnePassword => op_fetch(ctx, &src.item, src.kind, src.use_username),
        PwmProvider::KeePassXc => kp_fetch(ctx, &src.item, src.kind, src.use_username),
        PwmProvider::ProtonPass => proton::fetch(ctx, &src.item, src.kind, src.use_username),
    }
}

/// Resolves several sources (host, jump hosts, identity) for one connection.
pub fn resolve_all(
    ctx: &Ctx,
    wants: &[(Uuid, CredentialSource)],
) -> Result<Vec<(Uuid, Resolved)>, PwmError> {
    let mut out: Vec<(Uuid, Resolved)> = Vec::new();
    for (key, src) in wants {
        let r = fetch(ctx, src)?;
        match out.iter_mut().find(|(k, _)| k == key) {
            Some((_, existing)) => existing.merge(r),
            None => out.push((*key, r)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_validation_rejects_injection() {
        use PwmProvider::*;
        for ok in [
            (Bitwarden, "0b3d9e22-aaaa-4bbb-8ccc-0123456789ab"),
            (OnePassword, "abcd1234efgh5678ijkl9012mn"),
            (OnePassword, "op://Private/aurora lab/password"),
            (KeePassXc, "/Servers/aurora lab"),
            (ProtonPass, "pass://SHARE1/ITEM1"),
        ] {
            assert!(validate_ref(ok.0, ok.1).is_ok(), "{ok:?}");
        }
        for bad in [
            (Bitwarden, "--session"),
            (Bitwarden, "abc def"),
            (Bitwarden, "abc\n"),
            (Bitwarden, ""),
            (Bitwarden, "a;rm"),
            (OnePassword, "-x"),
            (OnePassword, "op://"),
            (OnePassword, "op://vault"),
            (OnePassword, "op://v/i\nx"),
            (OnePassword, "a b"),
            (KeePassXc, "-k"),
            (KeePassXc, "/a\u{0}b"),
            (KeePassXc, " /lead"),
            (KeePassXc, ""),
            (ProtonPass, "pass://a/b/c"),
            (ProtonPass, "--output"),
        ] {
            assert_eq!(
                validate_ref(bad.0, bad.1),
                Err(PwmError::BadReference),
                "{bad:?}"
            );
        }
        assert_eq!(clean_query(" a\nb\u{7}c "), "abc");
        assert_eq!(clean_query(&"x".repeat(500)).len(), 100);
    }

    #[test]
    fn session_timeout_logic() {
        let t0 = Instant::now();
        let m = |s: u64| t0 + Duration::from_secs(s);
        assert!(!expired(t0, m(14 * 60), 15));
        assert!(expired(t0, m(15 * 60), 15));
        assert!(!expired(t0, m(10_000), 0));

        let s = Sessions::default();
        s.set(PwmProvider::Bitwarden, Secret::new("k"));
        // Use refreshes the idle timer.
        let now = Instant::now();
        assert!(
            s.get_at(
                PwmProvider::Bitwarden,
                15,
                now + Duration::from_secs(14 * 60)
            )
            .is_some()
        );
        assert!(
            s.get_at(
                PwmProvider::Bitwarden,
                15,
                now + Duration::from_secs(14 * 60 + 5 * 60)
            )
            .is_some()
        );
        // Long idle: gone for good.
        assert!(
            s.get_at(
                PwmProvider::Bitwarden,
                15,
                now + Duration::from_secs(3 * 3600)
            )
            .is_none()
        );
        assert!(s.get(PwmProvider::Bitwarden, 15).is_none());
        s.set(PwmProvider::KeePassXc, Secret::new("pw"));
        s.lock_all();
        assert!(s.get(PwmProvider::KeePassXc, 0).is_none());
        // Providers without a session slot never hold one.
        s.set(PwmProvider::OnePassword, Secret::new("x"));
        assert!(s.get(PwmProvider::OnePassword, 0).is_none());
    }

    #[test]
    fn parses_bitwarden() {
        assert_eq!(
            parse_bw_status(r#"{"status":"locked","lastSync":null}"#),
            Ok(BwState::Locked)
        );
        assert_eq!(
            parse_bw_status(r#"{"status":"unauthenticated"}"#),
            Ok(BwState::LoggedOut)
        );
        assert_eq!(parse_bw_status("x"), Err(PwmError::BadOutput));
        let folders = parse_bw_folders(r#"[{"id":"f1","name":"Lab"},{"id":null,"name":"No"}]"#);
        let items = parse_bw_items(
            r#"[{"id":"aaaa-1","type":1,"name":"aurora-lab","folderId":"f1",
                 "login":{"username":"aurora","password":"x","uris":[{"uri":"ssh://h"}]}},
                {"id":"bbbb-2","type":3,"name":"card"},
                {"id":"--bad","type":1,"name":"evil"}]"#,
            &folders,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].username.as_str(), items[0].folder.as_str()),
            ("aurora", "Lab")
        );
        let r = parse_bw_item(
            r#"{"login":{"username":"aurora","password":"pw1"},"notes":"n"}"#,
            SourceKind::Password,
            true,
        )
        .unwrap();
        assert_eq!(r.username.as_deref(), Some("aurora"));
        assert_eq!(r.password.unwrap().expose(), "pw1");
        let key = r#"{"login":{"password":"phrase"},"notes":"-----BEGIN OPENSSH PRIVATE KEY-----\nA\n-----END OPENSSH PRIVATE KEY-----"}"#;
        let r = parse_bw_item(key, SourceKind::PrivateKey, false).unwrap();
        assert!(r.private_key.unwrap().expose().ends_with("KEY-----\n"));
        assert_eq!(r.passphrase.unwrap().expose(), "phrase");
        assert_eq!(
            parse_bw_item(r#"{"login":{}}"#, SourceKind::Password, true).err(),
            Some(PwmError::NoSecret)
        );
    }

    #[test]
    fn parses_onepassword_and_keepass() {
        let items = parse_op_items(
            r#"[{"id":"abc123","title":"aurora-lab","vault":{"id":"v","name":"Private"},
                 "category":"LOGIN","additional_information":"aurora"}]"#,
        )
        .unwrap();
        assert_eq!(items[0].folder, "Private");
        assert_eq!(items[0].username, "aurora");
        let r = parse_op_item(
            r#"{"fields":[{"id":"username","purpose":"USERNAME","label":"username","value":"aurora"},
                 {"id":"password","purpose":"PASSWORD","label":"password","value":"pw2"},
                 {"id":"notesPlain","purpose":"NOTES","label":"notesPlain","value":"hi"}]}"#,
            SourceKind::Password,
            true,
        )
        .unwrap();
        assert_eq!(r.username.as_deref(), Some("aurora"));
        assert_eq!(r.password.unwrap().expose(), "pw2");
        let k = parse_op_item(
            r#"{"fields":[{"id":"k","type":"SSHKEY","label":"private key","value":"x",
                "ssh_formats":{"openssh":{"value":"-----BEGIN OPENSSH PRIVATE KEY-----\nQ\n-----END OPENSSH PRIVATE KEY-----"}}}]}"#,
            SourceKind::PrivateKey,
            false,
        )
        .unwrap();
        assert!(
            k.private_key
                .unwrap()
                .expose()
                .contains("OPENSSH PRIVATE KEY")
        );

        let kp = parse_kp_search("Could not x\n/Servers/aurora lab\r\n/Top\n\n");
        assert_eq!(kp.len(), 2);
        assert_eq!(
            (kp[0].name.as_str(), kp[0].folder.as_str()),
            ("aurora lab", "Servers")
        );
        assert_eq!(kp[1].folder, "");
    }
}
