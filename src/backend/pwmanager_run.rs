//! Safe child-process runner for the password-manager CLIs: no shell, scrubbed
//! environment, bounded output, hard timeout. stderr is captured only to classify errors
//! and is never shown (it may contain secrets).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::pwmanager_types::{PwmError, PwmProvider, Secret};

/// Default per-command timeout.
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// Largest accepted stdout (item lists of big vaults).
pub const MAX_OUT: usize = 8 * 1024 * 1024;
const MAX_ERR: usize = 8 * 1024;

/// Environment variables passed on to the CLIs (everything else is dropped).
const ENV_ALLOW: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "BITWARDENCLI_APPDATA_DIR",
    "APPDATA",
    "LOCALAPPDATA",
    "USERPROFILE",
    "SYSTEMROOT",
    "SystemRoot",
    "COMSPEC",
    "PATHEXT",
];

pub struct Run {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Extra environment (secrets such as `BW_SESSION`).
    pub env: Vec<(&'static str, Secret)>,
    /// Written to stdin, then closed.
    pub stdin: Option<Secret>,
    pub timeout: Duration,
    pub max_out: usize,
}

impl Run {
    pub fn new(program: PathBuf, args: Vec<String>) -> Self {
        Self {
            program,
            args,
            env: Vec::new(),
            stdin: None,
            timeout: TIMEOUT,
            max_out: MAX_OUT,
        }
    }
}

pub struct Out {
    pub code: Option<i32>,
    pub stdout: Secret,
    /// Lower-cased stderr, only for `classify`.
    pub stderr: String,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// The program to run for `p`: the configured path, else `PATH`.
pub fn program_for(p: PwmProvider, configured: &str) -> Result<PathBuf, PwmError> {
    let configured = configured.trim();
    if !configured.is_empty() {
        if has_control(configured) || configured.starts_with('-') {
            return Err(PwmError::BadConfig);
        }
        let path = Path::new(configured);
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(PwmError::NotInstalled(p))
        };
    }
    super::local::find_program(p.program()).ok_or(PwmError::NotInstalled(p))
}

pub fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

pub fn run(r: Run) -> Result<Out, PwmError> {
    let mut cmd = Command::new(&r.program);
    cmd.args(&r.args);
    cmd.env_clear();
    for k in ENV_ALLOW {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in &r.env {
        cmd.env(k, v.expose());
    }
    cmd.stdin(if r.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|_| PwmError::Failed(None))?;

    if let (Some(secret), Some(mut stdin)) = (r.stdin.as_ref(), child.stdin.take()) {
        let data = secret.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(data.as_bytes());
            let _ = stdin.write_all(b"\n");
        });
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let max = r.max_out;
    let t_out = std::thread::spawn(move || read_capped(stdout.as_mut(), max));
    let t_err = std::thread::spawn(move || read_capped(stderr.as_mut(), MAX_ERR));

    let deadline = Instant::now() + r.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = t_out.join();
                    let _ = t_err.join();
                    return Err(PwmError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => return Err(PwmError::Failed(None)),
        }
    };
    let (out, truncated) = t_out.join().unwrap_or_default();
    let (err, _) = t_err.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err).to_lowercase();
    if truncated {
        return Err(PwmError::TooLarge);
    }
    Ok(Out {
        code: status.code(),
        stdout: Secret::from_bytes(out),
        stderr,
    })
}

/// Reads up to `max` bytes, drains the rest; `true` if more was sent.
fn read_capped<R: Read>(r: Option<&mut R>, max: usize) -> (Vec<u8>, bool) {
    let Some(r) = r else {
        return (Vec::new(), false);
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < max {
                    let take = n.min(max - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
        }
    }
    super::pwmanager_types::wipe(&mut chunk);
    (buf, truncated)
}

/// Maps a failed CLI call to a fixed error. `stderr` is already lower-case.
pub fn classify(p: PwmProvider, out: &Out) -> PwmError {
    let e = out.stderr.as_str();
    let has = |needles: &[&str]| needles.iter().any(|n| e.contains(n));
    if has(&[
        "invalid master password",
        "invalid credentials",
        "invalid password",
        "wrong password",
        "incorrect password",
    ]) {
        return PwmError::WrongPassword;
    }
    if has(&[
        "not logged in",
        "unauthenticated",
        "you are not logged in",
        "no active session",
        "not authenticated",
    ]) {
        return PwmError::LoggedOut(p);
    }
    if has(&[
        "vault is locked",
        "is locked",
        "not signed in",
        "no accounts configured",
        "sign in",
        "session expired",
        "unauthorized",
        "authorization",
    ]) {
        return PwmError::Locked(p);
    }
    if has(&[
        "not found",
        "no item",
        "isn't an item",
        "could not be found",
        "could not find",
        "no entry",
        "does not exist",
    ]) {
        return PwmError::NotFound;
    }
    PwmError::Failed(out.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(stderr: &str) -> Out {
        Out {
            code: Some(1),
            stdout: Secret::default(),
            stderr: stderr.to_lowercase(),
        }
    }

    #[test]
    fn error_mapping() {
        use PwmProvider::*;
        assert_eq!(
            classify(Bitwarden, &out("Invalid master password.")),
            PwmError::WrongPassword
        );
        assert_eq!(
            classify(Bitwarden, &out("Vault is locked.")),
            PwmError::Locked(Bitwarden)
        );
        assert_eq!(
            classify(Bitwarden, &out("You are not logged in.")),
            PwmError::LoggedOut(Bitwarden)
        );
        assert_eq!(
            classify(OnePassword, &out("[ERROR] account is not signed in")),
            PwmError::Locked(OnePassword)
        );
        assert_eq!(
            classify(
                KeePassXc,
                &out("Error while reading the database: Invalid credentials were provided")
            ),
            PwmError::WrongPassword
        );
        assert_eq!(
            classify(KeePassXc, &out("Could not find entry with path /x.")),
            PwmError::NotFound
        );
        assert_eq!(
            classify(Bitwarden, &out("weird")),
            PwmError::Failed(Some(1))
        );
        // The raw text never ends up in the message.
        assert!(
            !classify(Bitwarden, &out("secret-xyz"))
                .message()
                .contains("xyz")
        );
    }

    #[test]
    fn missing_program() {
        let r = program_for(PwmProvider::Bitwarden, "/nonexistent/bw");
        assert_eq!(r, Err(PwmError::NotInstalled(PwmProvider::Bitwarden)));
        assert_eq!(
            program_for(PwmProvider::Bitwarden, "-evil"),
            Err(PwmError::BadConfig)
        );
    }

    #[cfg(unix)]
    #[test]
    fn env_scrubbed_output_capped_timeout() {
        let sh = PathBuf::from("/bin/sh");
        // SAFETY of test: only reads the environment of the child.
        let mut r = Run::new(
            sh.clone(),
            vec!["-c".into(), "echo \"x${AURORA_TEST_LEAK}x\"".into()],
        );
        unsafe { std::env::set_var("AURORA_TEST_LEAK", "leaked") };
        let o = run(Run { ..r }).unwrap();
        assert_eq!(o.stdout.expose().trim(), "xx");
        // stdin and env secrets arrive.
        r = Run::new(
            sh.clone(),
            vec!["-c".into(), "read l; echo \"$l-$MYSEC\"".into()],
        );
        r.stdin = Some(Secret::new("in"));
        r.env.push(("MYSEC", Secret::new("env")));
        assert_eq!(run(r).unwrap().stdout.expose().trim(), "in-env");
        // Output limit.
        let mut r = Run::new(sh.clone(), vec!["-c".into(), "yes | head -c 100000".into()]);
        r.max_out = 1000;
        assert_eq!(run(r).err(), Some(PwmError::TooLarge));
        // Timeout.
        let mut r = Run::new(sh, vec!["-c".into(), "sleep 5".into()]);
        r.timeout = Duration::from_millis(200);
        assert_eq!(run(r).err(), Some(PwmError::Timeout));
    }
}
