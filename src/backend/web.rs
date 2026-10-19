//! Web interface entries (firewall, switch, NAS, Proxmox …): the helper process, vendor
//! templates and the glue between a [`Host`] of protocol `Web` and the helper protocol.
//!
//! The page itself is shown by the `aurora-web` helper (wry: WebView2 / WKWebView /
//! WebKitGTK) in its own window with a private profile per entry. Aurora talks to it over
//! stdin/stdout JSON lines; credentials only travel through that pipe.

use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use uuid::Uuid;

use super::Sink;
use crate::model::Host;

/// URL rules, certificate pinning decisions, the auto-login script and the line protocol.
/// One source file, compiled into Aurora and into the helper, so both always agree.
#[path = "../../crates/aurora-web/src/proto.rs"]
#[allow(dead_code)]
pub mod proto;

pub use proto::{Credentials, TlsMode, WebCmd, WebEvent, WebStart};

// ------------------------------------------------------------------------------------
// Helper process
// ------------------------------------------------------------------------------------

/// Finds the helper binary: `AURORA_WEB_HELPER`, next to the executable, or a dev build.
pub fn helper_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "aurora-web.exe"
    } else {
        "aurora-web"
    };
    if let Ok(p) = std::env::var("AURORA_WEB_HELPER") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(name));
        if cfg!(target_os = "linux") {
            candidates.push(dir.join("../lib/aurora-term").join(name));
        }
    }
    // Dev builds only: never pick up a binary relative to the working directory in a
    // release build (it would receive the user's web login).
    if cfg!(debug_assertions) {
        for profile in ["release", "debug"] {
            candidates.push(
                PathBuf::from("crates/aurora-web/target")
                    .join(profile)
                    .join(name),
            );
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Private data directory (cookies, local storage, cache) of one entry.
pub fn profile_dir(host: Uuid) -> PathBuf {
    crate::store::data_dir()
        .join("web-profiles")
        .join(host.to_string())
}

/// Where the web page may save downloads (`Downloads/Aurora`).
pub fn download_dir() -> PathBuf {
    crate::ui::rdp::download_target()
}

/// Deletes the saved logins/cookies of an entry.
pub fn forget_profile(host: Uuid) {
    let dir = profile_dir(host);
    if dir.starts_with(crate::store::data_dir()) && dir.is_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Removes the saved web data of deleted entries.
pub fn forget_host_data(hosts: &[Host], ids: &[Uuid]) {
    for h in hosts {
        if h.protocol == crate::model::Protocol::Web && ids.contains(&h.id) {
            forget_profile(h.id);
        }
    }
}

pub struct WebHandle {
    stdin: Arc<Mutex<Option<BufWriter<ChildStdin>>>>,
    child: Child,
}

impl WebHandle {
    pub fn send(&self, cmd: &WebCmd) {
        let Ok(line) = serde_json::to_string(cmd) else {
            return;
        };
        let mut guard = self.stdin.lock().unwrap();
        if let Some(w) = guard.as_mut() {
            let ok = writeln!(w, "{line}").is_ok() && w.flush().is_ok();
            if !ok {
                *guard = None;
            }
        }
    }

    pub fn close(&mut self) {
        self.send(&WebCmd::Close);
        self.stdin.lock().unwrap().take();
    }

    /// The helper process is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for WebHandle {
    fn drop(&mut self) {
        self.close();
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Starts the helper and sends `start` (with any credentials) over its stdin. Events
/// arrive on `events`; the last one is always `Closed`.
pub fn spawn(start: &WebStart, events: Sink<WebEvent>) -> Result<WebHandle> {
    let helper =
        helper_path().ok_or_else(|| anyhow!(t!("Web-Komponente (aurora-web) nicht gefunden")))?;
    // Validate again here: the helper must never see a URL Aurora would not open itself.
    proto::parse_url(&start.url).map_err(|e| anyhow!(crate::i18n::tr(e.message()).to_string()))?;
    let mut cmd = Command::new(&helper);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("{} starten", helper.display()))?;
    let mut stdin = BufWriter::new(child.stdin.take().context("stdin")?);
    let stdout = child.stdout.take().context("stdout")?;

    // Credentials go over the pipe, never through argv or the environment.
    writeln!(stdin, "{}", serde_json::to_string(start)?)?;
    stdin.flush()?;

    std::thread::spawn(move || {
        let mut out = BufReader::new(stdout);
        let mut closed = false;
        loop {
            match proto::read_bounded_line(&mut out, proto::MAX_LINE) {
                Ok(Some(line)) => {
                    if let Ok(ev) = serde_json::from_str::<WebEvent>(&line) {
                        closed = matches!(ev, WebEvent::Closed);
                        events.send(ev);
                        if closed {
                            break;
                        }
                    }
                }
                Ok(None) => break,
                // Oversized or malformed line: skip it.
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue,
                Err(_) => break,
            }
        }
        if !closed {
            events.send(WebEvent::Closed);
        }
    });

    Ok(WebHandle {
        stdin: Arc::new(Mutex::new(Some(stdin))),
        child,
    })
}

/// Opens an http(s) link in the system browser. Re-validated and restricted to a safe
/// character set, because on Windows the URL passes through the shell protocol handler.
pub fn open_external(url: &str) -> bool {
    let Ok(parsed) = proto::parse_url(url) else {
        return false;
    };
    let safe = |c: char| c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c);
    let url = parsed.to_url();
    if !url.chars().all(safe) {
        return false;
    }
    #[cfg(windows)]
    let r = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = Command::new("xdg-open").arg(&url).spawn();
    r.is_ok()
}

// ------------------------------------------------------------------------------------
// Entries
// ------------------------------------------------------------------------------------

/// "host:port" the pin belongs to.
pub fn pin_scope(url: &proto::WebUrl) -> String {
    format!("{}:{}", url.host, url.port)
}

/// Brings `hostname`/`port` (reachability LED, certificate radar, search) in line with
/// the URL, normalises the URL and drops a pin that belongs to another address.
/// Returns the validation error, if any.
pub fn sync_host(h: &mut Host) -> Result<(), proto::UrlError> {
    let url = proto::parse_url(&h.web.url)?;
    h.web.url = url.to_url();
    h.hostname = url.host.clone();
    h.port = url.port;
    let scope = pin_scope(&url);
    if h.web.pinned_for != scope || h.web.tls != TlsMode::Pinned {
        h.web.pinned_fp.clear();
        h.web.pinned_at = None;
    }
    h.web.pinned_for = scope;
    h.web.extra_hosts = proto::parse_extra_hosts(&h.web.extra_hosts).join(", ");
    h.web.user_selector = h.web.user_selector.trim().chars().take(200).collect();
    h.web.pass_selector = h.web.pass_selector.trim().chars().take(200).collect();
    Ok(())
}

/// What the helper needs to open `h`. `credentials` only when auto-login is on.
pub fn start_for(h: &Host, user: &str, password: Option<&str>) -> Result<WebStart, String> {
    let url = proto::parse_url(&h.web.url).map_err(|e| crate::i18n::tr(e.message()).to_string())?;
    let credentials =
        (h.web.auto_login && (password.is_some() || !user.is_empty())).then(|| Credentials {
            username: user.to_string(),
            password: password.unwrap_or_default().to_string(),
        });
    Ok(WebStart {
        url: url.to_url(),
        title: h.display_name().chars().take(120).collect(),
        profile_dir: profile_dir(h.id).to_string_lossy().into_owned(),
        profile_id: h.id.to_string(),
        tls: h.web.tls,
        pinned_fp: if h.web.tls == TlsMode::Pinned && h.web.pinned_for == pin_scope(&url) {
            h.web.pinned_fp.clone()
        } else {
            String::new()
        },
        credentials,
        user_selector: h.web.user_selector.clone(),
        pass_selector: h.web.pass_selector.clone(),
        submit: h.web.submit,
        extra_hosts: proto::parse_extra_hosts(&h.web.extra_hosts),
        download_dir: download_dir().to_string_lossy().into_owned(),
    })
}

// ------------------------------------------------------------------------------------
// Vendor templates
// ------------------------------------------------------------------------------------

/// Prefill for the new-entry form.
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub https: bool,
    pub port: u16,
    pub path: &'static str,
    /// Tag added to the entry.
    pub tag: &'static str,
    /// CSS selectors of the login form; empty = detect automatically.
    pub user_selector: &'static str,
    pub pass_selector: &'static str,
    /// These devices ship a self-signed certificate: pin it instead of failing.
    pub self_signed: bool,
    /// Short hint shown below the template.
    pub hint: &'static str,
}

pub const TEMPLATES: &[Template] = &[
    Template {
        id: "generic",
        name: "Allgemein",
        https: true,
        port: 443,
        path: "/",
        tag: "web",
        user_selector: "",
        pass_selector: "",
        self_signed: false,
        hint: "",
    },
    Template {
        id: "pfsense",
        name: "pfSense",
        https: true,
        port: 443,
        path: "/",
        tag: "pfsense",
        user_selector: "input[name=usernamefld]",
        pass_selector: "input[name=passwordfld]",
        self_signed: true,
        hint: "Standard-Anmeldung: admin / pfsense (sofort ändern)",
    },
    Template {
        id: "opnsense",
        name: "OPNsense",
        https: true,
        port: 443,
        path: "/",
        tag: "opnsense",
        user_selector: "#usernamefld",
        pass_selector: "#passwordfld",
        self_signed: true,
        hint: "Standard-Anmeldung: root / opnsense (sofort ändern)",
    },
    Template {
        id: "fortigate",
        name: "FortiGate",
        https: true,
        port: 443,
        path: "/login",
        tag: "fortigate",
        user_selector: "#username",
        pass_selector: "#secretkey",
        self_signed: true,
        hint: "Admin-Port oft 443 oder 8443",
    },
    Template {
        id: "sophos",
        name: "Sophos Firewall",
        https: true,
        port: 4444,
        path: "/",
        tag: "sophos",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "Admin-Konsole auf Port 4444",
    },
    Template {
        id: "unifi",
        name: "UniFi",
        https: true,
        port: 443,
        path: "/",
        tag: "unifi",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "UniFi OS; ältere Controller nutzen Port 8443",
    },
    Template {
        id: "mikrotik",
        name: "MikroTik WebFig",
        https: false,
        port: 80,
        path: "/webfig/",
        tag: "mikrotik",
        user_selector: "",
        pass_selector: "",
        self_signed: false,
        hint: "WebFig ist standardmäßig unverschlüsselt (http) – besser www-ssl aktivieren",
    },
    Template {
        id: "synology",
        name: "Synology DSM",
        https: true,
        port: 5001,
        path: "/",
        tag: "synology",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "DSM: https 5001, http 5000",
    },
    Template {
        id: "truenas",
        name: "TrueNAS",
        https: true,
        port: 443,
        path: "/ui/",
        tag: "truenas",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "",
    },
    Template {
        id: "pve",
        name: "Proxmox VE",
        https: true,
        port: 8006,
        path: "/",
        tag: "proxmox",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "Weboberfläche auf Port 8006",
    },
    Template {
        id: "pbs",
        name: "Proxmox Backup Server",
        https: true,
        port: 8007,
        path: "/",
        tag: "proxmox",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "Weboberfläche auf Port 8007",
    },
    Template {
        id: "idrac",
        name: "Dell iDRAC",
        https: true,
        port: 443,
        path: "/login.html",
        tag: "idrac",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "Remote-Management-Karte (BMC)",
    },
    Template {
        id: "ilo",
        name: "HPE iLO",
        https: true,
        port: 443,
        path: "/",
        tag: "ilo",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "Remote-Management-Karte (BMC)",
    },
    Template {
        id: "ubiquiti",
        name: "Ubiquiti EdgeOS",
        https: true,
        port: 443,
        path: "/",
        tag: "ubiquiti",
        user_selector: "",
        pass_selector: "",
        self_signed: true,
        hint: "",
    },
];

pub fn template(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// Applies a template to a new entry: URL (keeping the host typed so far), TLS policy,
/// login selectors, tag and a name suggestion.
pub fn apply_template(h: &mut Host, tags: &mut String, t: &Template) {
    let host = proto::parse_url(&h.web.url)
        .map(|u| u.host)
        .ok()
        .or_else(|| {
            // A bare host typed so far ("192.168.1.1") also counts.
            let raw = h.web.url.trim();
            proto::parse_url(raw).ok().map(|u| u.host)
        })
        .unwrap_or_else(|| "192.168.1.1".to_string());
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let scheme = if t.https { "https" } else { "http" };
    let default = if t.https { 443 } else { 80 };
    h.web.url = if t.port == default {
        format!("{scheme}://{host}{}", t.path)
    } else {
        format!("{scheme}://{host}:{}{}", t.port, t.path)
    };
    h.web.template = t.id.to_string();
    h.web.user_selector = t.user_selector.to_string();
    h.web.pass_selector = t.pass_selector.to_string();
    h.web.tls = if t.self_signed {
        TlsMode::Pinned
    } else {
        TlsMode::Strict
    };
    if let Ok(u) = proto::parse_url(&h.web.url) {
        h.port = u.port;
    }
    let mut parts: Vec<String> = tags
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !parts.iter().any(|p| p == t.tag) {
        parts.push(t.tag.to_string());
    }
    *tags = parts.join(", ");
    if h.label.trim().is_empty() && t.id != "generic" {
        h.label = t.name.to_string();
    }
}

/// Which templates' tags are known (so applying another template can replace the old tag).
pub fn forget_template_tag(tags: &mut String, old_id: &str) {
    let Some(old) = template(old_id) else { return };
    let parts: Vec<&str> = tags
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != old.tag)
        .collect();
    *tags = parts.join(", ");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web_host(url: &str) -> Host {
        let mut h = Host {
            protocol: crate::model::Protocol::Web,
            ..Default::default()
        };
        h.web.url = url.into();
        h
    }

    #[test]
    fn templates_build_valid_urls() {
        for t in TEMPLATES {
            let mut h = web_host("10.0.0.1");
            let mut tags = String::new();
            apply_template(&mut h, &mut tags, t);
            let u = proto::parse_url(&h.web.url).unwrap_or_else(|_| panic!("{}", t.id));
            assert_eq!(u.host, "10.0.0.1", "{}", t.id);
            assert_eq!(u.port, t.port, "{}", t.id);
            assert_eq!(u.https, t.https, "{}", t.id);
            assert!(tags.contains(t.tag));
            // Selectors are plain CSS, never script.
            assert!(!t.user_selector.contains("javascript"));
        }
        // Unique ids.
        let mut ids: Vec<_> = TEMPLATES.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TEMPLATES.len());
    }

    #[test]
    fn template_keeps_host_and_defaults() {
        let mut h = web_host("https://fw.lan:1234/old");
        let mut tags = "prod".to_string();
        apply_template(&mut h, &mut tags, template("pve").unwrap());
        assert_eq!(h.web.url, "https://fw.lan:8006/");
        assert_eq!(h.web.tls, TlsMode::Pinned);
        assert_eq!(tags, "prod, proxmox");
        assert_eq!(h.label, "Proxmox VE");
        let mut h2 = web_host("");
        apply_template(&mut h2, &mut String::new(), template("generic").unwrap());
        assert_eq!(h2.web.url, "https://192.168.1.1/");
        assert_eq!(h2.web.tls, TlsMode::Strict);
        assert!(h2.label.is_empty());
        let mut h3 = web_host("http://sw.lan");
        apply_template(&mut h3, &mut String::new(), template("mikrotik").unwrap());
        assert_eq!(h3.web.url, "http://sw.lan/webfig/");
    }

    #[test]
    fn sync_copies_host_and_port_and_drops_stale_pin() {
        let mut h = web_host("nas.lan:5001/x");
        h.web.tls = TlsMode::Pinned;
        h.web.pinned_fp = proto::fingerprint_sha256(b"cert");
        h.web.pinned_for = "nas.lan:5001".into();
        sync_host(&mut h).unwrap();
        assert_eq!((h.hostname.as_str(), h.port), ("nas.lan", 5001));
        assert_eq!(h.web.url, "https://nas.lan:5001/x");
        assert!(!h.web.pinned_fp.is_empty(), "same address keeps the pin");
        // Another address: the pin does not carry over.
        h.web.url = "https://other.lan:5001/".into();
        sync_host(&mut h).unwrap();
        assert!(h.web.pinned_fp.is_empty());
        assert_eq!(h.web.pinned_for, "other.lan:5001");
        // Leaving pinned mode forgets it as well.
        h.web.pinned_fp = proto::fingerprint_sha256(b"cert");
        h.web.tls = TlsMode::Strict;
        sync_host(&mut h).unwrap();
        assert!(h.web.pinned_fp.is_empty());
        // Invalid URLs are reported, nothing is touched.
        h.web.url = "javascript:alert(1)".into();
        assert!(sync_host(&mut h).is_err());
        assert_eq!(h.hostname, "other.lan");
    }

    #[test]
    fn start_carries_credentials_only_with_auto_login() {
        let mut h = web_host("https://fw.lan/");
        h.label = "FW".into();
        let s = start_for(&h, "admin", Some("pw")).unwrap();
        assert!(s.credentials.is_none());
        h.web.auto_login = true;
        let s = start_for(&h, "admin", Some("pw")).unwrap();
        let c = s.credentials.unwrap();
        assert_eq!((c.username.as_str(), c.password.as_str()), ("admin", "pw"));
        assert_eq!(s.title, "FW");
        assert!(s.profile_dir.contains(&h.id.to_string()));
        // A pin for another address is not handed over.
        h.web.tls = TlsMode::Pinned;
        h.web.pinned_fp = proto::fingerprint_sha256(b"x");
        h.web.pinned_for = "elsewhere:443".into();
        assert!(start_for(&h, "", None).unwrap().pinned_fp.is_empty());
        h.web.pinned_for = "fw.lan:443".into();
        assert!(!start_for(&h, "", None).unwrap().pinned_fp.is_empty());
        h.web.url = "file:///etc/passwd".into();
        assert!(start_for(&h, "", None).is_err());
    }

    #[test]
    fn external_links_are_validated() {
        assert!(!open_external("javascript:alert(1)"));
        assert!(!open_external("file:///etc/passwd"));
        assert!(!open_external("https://a/\"; calc"));
        assert!(!open_external("https://a/ b"));
    }

    #[test]
    fn forgetting_a_template_tag() {
        let mut tags = "prod, pfsense, edge".to_string();
        forget_template_tag(&mut tags, "pfsense");
        assert_eq!(tags, "prod, edge");
    }
}
