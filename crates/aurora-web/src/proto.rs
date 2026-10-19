//! Shared, pure logic of the Aurora web helper: URL validation, origin rules, certificate
//! pinning decisions, the auto-login script and the line protocol between Aurora and the
//! helper. This file is compiled into the helper (`aurora-web`) AND into Aurora itself
//! (`src/backend/web.rs`, via `#[path]`), so both sides always agree.
//!
//! Nothing in here touches the network, the file system or a window, so it is fully
//! unit-testable.

use std::io::{self, BufRead, Read};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Longest accepted protocol line (credentials + URL fit easily).
pub const MAX_LINE: usize = 64 * 1024;
/// Longest accepted URL.
pub const MAX_URL: usize = 2048;

// ---------------------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    TooLong,
    /// Anything but http/https (javascript:, file:, data:, ftp: …).
    Scheme,
    NoHost,
    /// `user:pass@host` – credentials never belong in a URL.
    UserInfo,
    BadHost,
    BadPort,
    /// Whitespace, control characters or backslashes.
    BadChar,
}

impl UrlError {
    pub fn message(&self) -> &'static str {
        match self {
            UrlError::Empty => "Keine Adresse angegeben",
            UrlError::TooLong => "Die Adresse ist zu lang",
            UrlError::Scheme => "Nur http:// und https:// sind erlaubt",
            UrlError::NoHost => "Die Adresse enthält keinen Host",
            UrlError::UserInfo => "Benutzer/Passwort gehören nicht in die Adresse",
            UrlError::BadHost => "Ungültiger Host-Name",
            UrlError::BadPort => "Ungültiger Port",
            UrlError::BadChar => "Die Adresse enthält ungültige Zeichen",
        }
    }
}

/// A validated http(s) URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebUrl {
    pub https: bool,
    /// Lower case, without the brackets of an IPv6 literal.
    pub host: String,
    pub port: u16,
    /// Path, query and fragment; always starts with `/`.
    pub rest: String,
}

impl WebUrl {
    pub fn scheme(&self) -> &'static str {
        if self.https { "https" } else { "http" }
    }

    pub fn default_port(&self) -> u16 {
        if self.https { 443 } else { 80 }
    }

    fn host_for_url(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    /// `https://host[:port]` (port only when not the default).
    pub fn origin(&self) -> String {
        if self.port == self.default_port() {
            format!("{}://{}", self.scheme(), self.host_for_url())
        } else {
            format!("{}://{}:{}", self.scheme(), self.host_for_url(), self.port)
        }
    }

    /// Normalised form of the whole URL.
    pub fn to_url(&self) -> String {
        format!("{}{}", self.origin(), self.rest)
    }

    pub fn same_origin(&self, other: &WebUrl) -> bool {
        self.https == other.https && self.host == other.host && self.port == other.port
    }
}

fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.contains(':') {
        // IPv6 literal.
        return host.parse::<std::net::Ipv6Addr>().is_ok();
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// Parses and validates `input`. Without `://` the scheme defaults to https.
pub fn parse_url(input: &str) -> Result<WebUrl, UrlError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(UrlError::Empty);
    }
    if input.len() > MAX_URL {
        return Err(UrlError::TooLong);
    }
    if input
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\' || !c.is_ascii())
    {
        return Err(UrlError::BadChar);
    }
    let (https, after) = match input.split_once("://") {
        Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
            "https" => (true, rest),
            "http" => (false, rest),
            _ => return Err(UrlError::Scheme),
        },
        None => (true, input),
    };
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(end);
    if authority.is_empty() {
        return Err(UrlError::NoHost);
    }
    if authority.contains('@') {
        return Err(UrlError::UserInfo);
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, tail) = v6.split_once(']').ok_or(UrlError::BadHost)?;
        let port = match tail {
            "" => None,
            t => Some(t.strip_prefix(':').ok_or(UrlError::BadHost)?),
        };
        (h, port)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let host = host.to_ascii_lowercase();
    if !valid_host(&host) {
        return Err(UrlError::BadHost);
    }
    let port = match port {
        None => {
            if https {
                443
            } else {
                80
            }
        }
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(UrlError::BadPort);
            }
            match p.parse::<u16>() {
                Ok(n) if n > 0 => n,
                _ => return Err(UrlError::BadPort),
            }
        }
    };
    let rest = if rest.is_empty() {
        "/".to_string()
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    };
    Ok(WebUrl {
        https,
        host,
        port,
        rest,
    })
}

/// Whether a navigation target may be shown inside the helper window: same origin as the
/// entry, or one of the explicitly allowed extra hosts (same scheme family, any port).
pub fn navigation_allowed(base: &WebUrl, extra_hosts: &[String], target: &str) -> bool {
    if target == "about:blank" {
        return true;
    }
    let Ok(t) = parse_url(target) else {
        return false;
    };
    if t.same_origin(base) {
        return true;
    }
    // Never downgrade an https entry to http on another host.
    if base.https && !t.https {
        return false;
    }
    extra_hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&t.host) && valid_host(&h.to_ascii_lowercase()))
}

/// Splits a comma/space separated host list, keeping only valid host names.
pub fn parse_extra_hosts(s: &str) -> Vec<String> {
    s.split([',', ' ', ';', '\n'])
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty() && valid_host(h))
        .take(16)
        .collect()
}

// ---------------------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------------------

/// How TLS certificate errors of the page are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// Every certificate error blocks the page.
    #[default]
    Strict,
    /// A self-signed/unknown certificate is accepted only if its SHA-256 fingerprint is
    /// the pinned one (trust on first use, confirmed by the user).
    Pinned,
    /// Ignore all certificate errors (explicit per-entry opt-in).
    InsecureAll,
}

/// `AA:BB:…` (upper case) SHA-256 of the DER certificate.
pub fn fingerprint_sha256(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let mut out = String::with_capacity(95);
    for (i, b) in digest.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// Canonical form for comparisons: lower-case hex without separators, or `None` when `s`
/// is not a SHA-256 fingerprint.
pub fn normalize_fingerprint(s: &str) -> Option<String> {
    let hex: String = s
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

pub fn fingerprints_equal(a: &str, b: &str) -> bool {
    match (normalize_fingerprint(a), normalize_fingerprint(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Decodes the first certificate of a PEM block to DER.
pub fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let mut body = String::new();
    let mut inside = false;
    for line in pem.lines() {
        let line = line.trim();
        if line.starts_with("-----BEGIN") {
            inside = true;
        } else if line.starts_with("-----END") {
            break;
        } else if inside {
            body.push_str(line);
        }
    }
    if body.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// What to do with a certificate error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsVerdict {
    /// Load the page.
    Allow,
    /// Block; the mode is strict.
    Refuse,
    /// Pinned mode, nothing pinned yet: ask the user, then answer with `TrustCert`.
    Unknown,
    /// Pinned mode and the certificate differs from the pinned one.
    Mismatch { expected: String },
}

/// `trusted` holds the pinned fingerprint(s); `seen` is the fingerprint of the leaf
/// certificate the server presented.
pub fn tls_verdict(mode: TlsMode, trusted: &[String], seen: &str) -> TlsVerdict {
    match mode {
        TlsMode::Strict => TlsVerdict::Refuse,
        TlsMode::InsecureAll => TlsVerdict::Allow,
        TlsMode::Pinned => {
            if trusted.iter().any(|t| fingerprints_equal(t, seen)) {
                TlsVerdict::Allow
            } else if let Some(first) = trusted.first() {
                TlsVerdict::Mismatch {
                    expected: first.clone(),
                }
            } else {
                TlsVerdict::Unknown
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Auto-login script
// ---------------------------------------------------------------------------------------

/// A JavaScript string literal (double quoted) that is safe in any script context: the
/// result contains no `<`, `>`, `&`, quote breaking or line-terminator characters, so
/// even `</script>` in a password cannot leave the literal.
pub fn js_string(s: &str) -> String {
    let json = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut out = String::with_capacity(json.len() + 8);
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\'' => out.push_str("\\u0027"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Credentials {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

const AUTOFILL_BODY: &str = r#"
if (window.top !== window || window.__auroraAutofill || location.origin !== C.o) return 'skip';
window.__auroraAutofill = 1;
var tries = 0;
function vis(e) {
  if (!e) return false;
  var r = e.getBoundingClientRect(), s = getComputedStyle(e);
  return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
}
function setv(e, v) {
  var d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e), 'value') ||
          Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
  e.focus();
  if (d && d.set) d.set.call(e, v); else e.value = v;
  e.dispatchEvent(new Event('input', {bubbles: true}));
  e.dispatchEvent(new Event('change', {bubbles: true}));
}
function pick(sel) {
  if (!sel) return null;
  try { var e = document.querySelector(sel); return vis(e) ? e : null; } catch (x) { return null; }
}
function attempt() {
  var p = pick(C.ps);
  if (!p) {
    var all = document.querySelectorAll('input[type=password]');
    for (var i = 0; i < all.length; i++) { if (vis(all[i])) { p = all[i]; break; } }
  }
  if (!p) return false;
  var u = pick(C.us);
  if (!u) {
    var ins = Array.prototype.slice.call(document.querySelectorAll('input'));
    for (var j = ins.indexOf(p) - 1; j >= 0; j--) {
      var t = (ins[j].getAttribute('type') || 'text').toLowerCase();
      if ((t === 'text' || t === 'email' || t === 'tel') && vis(ins[j])) { u = ins[j]; break; }
    }
  }
  if (u && C.u) setv(u, C.u);
  setv(p, C.p);
  if (C.s) {
    setTimeout(function () {
      var f = p.form, b = null;
      if (f) b = f.querySelector('button[type=submit],input[type=submit]') || f.querySelector('button');
      if (!b) b = document.querySelector('button[type=submit],input[type=submit]');
      if (b) b.click();
      else if (f && f.requestSubmit) f.requestSubmit();
      else if (f) f.submit();
    }, 250);
  }
  return true;
}
var timer = setInterval(function () {
  tries++;
  var done = false;
  try { done = attempt(); } catch (x) { done = true; }
  if (done || tries > 50) clearInterval(timer);
}, 400);
return 'armed';
"#;

/// Script that fills the login form (`user_sel`/`pass_sel` may be empty = heuristics) and
/// optionally submits it. All values are embedded as escaped JS string literals. The page
/// itself checks that it is on `origin` (so a stale load event can never leak credentials
/// into another document) and arms itself only once per document; the result is
/// `"armed"` or `"skip"`.
pub fn autofill_script(
    c: &Credentials,
    origin: &str,
    user_sel: &str,
    pass_sel: &str,
    submit: bool,
) -> String {
    format!(
        "(function(){{var C={{o:{},u:{},p:{},us:{},ps:{},s:{}}};{}}})();",
        js_string(origin),
        js_string(&c.username),
        js_string(&c.password),
        js_string(user_sel),
        js_string(pass_sel),
        if submit { "true" } else { "false" },
        AUTOFILL_BODY
    )
}

// ---------------------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------------------

/// A harmless file name for a download (no path parts, no reserved characters).
pub fn safe_download_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    out = out
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    if out.chars().count() > 120 {
        out = out.chars().take(120).collect();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if out.is_empty() {
        "download".into()
    } else if reserved {
        format!("_{out}")
    } else {
        out
    }
}

// ---------------------------------------------------------------------------------------
// Line protocol
// ---------------------------------------------------------------------------------------

/// First line from Aurora to the helper. Sensitive parts only travel over the pipe.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebStart {
    pub url: String,
    /// Window title (= name of the entry).
    pub title: String,
    /// Private data directory of this entry (cookies, local storage).
    pub profile_dir: String,
    /// Stable id of the entry (macOS data store identifier).
    pub profile_id: String,
    #[serde(default)]
    pub tls: TlsMode,
    /// Pinned SHA-256 fingerprint (`AA:BB:…`); empty = not pinned yet.
    #[serde(default)]
    pub pinned_fp: String,
    #[serde(default)]
    pub credentials: Option<Credentials>,
    #[serde(default)]
    pub user_selector: String,
    #[serde(default)]
    pub pass_selector: String,
    #[serde(default)]
    pub submit: bool,
    /// Additional hosts the page may navigate to inside the window.
    #[serde(default)]
    pub extra_hosts: Vec<String>,
    /// Downloads are saved here; empty = downloads are refused.
    #[serde(default)]
    pub download_dir: String,
}

impl std::fmt::Debug for WebStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebStart")
            .field("url", &self.url)
            .field("title", &self.title)
            .field("tls", &self.tls)
            .field("pinned_fp", &self.pinned_fp)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

/// Commands from Aurora to the helper (one JSON object per line, after `WebStart`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum WebCmd {
    /// Raise the window.
    Focus,
    Reload,
    /// Back to the entry URL.
    Home,
    /// The user confirmed this certificate (replaces the pin for the session).
    TrustCert {
        fingerprint: String,
    },
    Close,
}

/// Events from the helper to Aurora (one JSON object per line on stdout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum WebEvent {
    /// Window and web view exist.
    Ready {
        engine: String,
    },
    Title {
        title: String,
    },
    /// A page finished loading.
    Loaded {
        url: String,
    },
    /// A certificate was refused or needs a decision.
    Cert {
        fingerprint: String,
        /// Pinned fingerprint that did not match (`None` = nothing pinned yet).
        expected: Option<String>,
        /// The mode was strict: there is nothing to decide.
        strict: bool,
    },
    /// A link to another origin was handed to the system browser.
    External {
        url: String,
    },
    Download {
        name: String,
        ok: bool,
    },
    /// A navigation was refused (`url` is shortened).
    Blocked {
        url: String,
    },
    Error {
        message: String,
    },
    Closed,
}

/// Reads one line of at most `max` bytes. `Ok(None)` = end of input; a longer line is an
/// error (the rest of it is discarded).
pub fn read_bounded_line<R: BufRead>(r: &mut R, max: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r
        .by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && buf.len() > max {
        // Too long: drop the remainder of the line.
        let mut sink = Vec::new();
        loop {
            sink.clear();
            let k = r.by_ref().take(8192).read_until(b'\n', &mut sink)?;
            if k == 0 || sink.last() == Some(&b'\n') {
                break;
            }
        }
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    let s = String::from_utf8(buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not utf-8"))?;
    Ok(Some(s.trim_end_matches(['\n', '\r']).to_string()))
}

pub fn parse_start(line: &str) -> Result<WebStart, String> {
    let s: WebStart = serde_json::from_str(line).map_err(|e| format!("start: {e}"))?;
    let url = parse_url(&s.url).map_err(|e| e.message().to_string())?;
    if s.profile_dir.trim().is_empty() {
        return Err("profile_dir fehlt".into());
    }
    if s.title.len() > 300 {
        return Err("title zu lang".into());
    }
    if s.tls == TlsMode::Pinned
        && !s.pinned_fp.is_empty()
        && normalize_fingerprint(&s.pinned_fp).is_none()
    {
        return Err("pinned_fp ungültig".into());
    }
    let _ = url;
    Ok(s)
}

pub fn parse_cmd(line: &str) -> Result<WebCmd, String> {
    let cmd: WebCmd = serde_json::from_str(line).map_err(|e| format!("cmd: {e}"))?;
    if let WebCmd::TrustCert { fingerprint } = &cmd
        && normalize_fingerprint(fingerprint).is_none()
    {
        return Err("fingerprint ungültig".into());
    }
    Ok(cmd)
}

/// Shortens a URL for display/log (no query string, which may carry tokens).
pub fn short_url(url: &str) -> String {
    let cut = url.find(['?', '#']).unwrap_or(url.len());
    let mut s: String = url[..cut].chars().take(160).collect();
    if cut < url.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_accepts_http_and_https_only() {
        let u = parse_url("https://fw.lan:8443/ui/#/login").unwrap();
        assert!(u.https);
        assert_eq!(u.host, "fw.lan");
        assert_eq!(u.port, 8443);
        assert_eq!(u.rest, "/ui/#/login");
        assert_eq!(u.to_url(), "https://fw.lan:8443/ui/#/login");
        assert_eq!(
            parse_url("HTTP://NAS.local").unwrap().to_url(),
            "http://nas.local/"
        );
        // Default scheme is https.
        assert_eq!(
            parse_url("192.168.1.1:8006").unwrap().to_url(),
            "https://192.168.1.1:8006/"
        );
        assert_eq!(parse_url("pve.lan").unwrap().origin(), "https://pve.lan");
        assert_eq!(
            parse_url("https://[fe80::1]:444/x").unwrap().origin(),
            "https://[fe80::1]:444"
        );
    }

    #[test]
    fn url_rejects_dangerous_schemes_and_forms() {
        for bad in [
            "javascript:alert(1)",
            "javascript://%0aalert(1)",
            "file:///etc/passwd",
            "data:text/html,<script>1</script>",
            "ftp://host/",
            "vbscript:x",
            "view-source:https://a",
            "blob:https://a/b",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_url("https://user:pw@host/"), Err(UrlError::UserInfo));
        assert_eq!(parse_url("https://host:99999/"), Err(UrlError::BadPort));
        assert_eq!(parse_url("https://host:0/"), Err(UrlError::BadPort));
        assert_eq!(parse_url("https://ho st/"), Err(UrlError::BadChar));
        assert_eq!(parse_url("https://host\\@evil/"), Err(UrlError::BadChar));
        assert_eq!(parse_url("https://exämple.de/"), Err(UrlError::BadChar));
        assert_eq!(parse_url("https:///path"), Err(UrlError::NoHost));
        assert_eq!(parse_url(""), Err(UrlError::Empty));
        assert_eq!(parse_url("https://-bad-.com"), Err(UrlError::BadHost));
        assert_eq!(
            parse_url(&format!("https://h/{}", "a".repeat(3000))),
            Err(UrlError::TooLong)
        );
    }

    #[test]
    fn navigation_stays_on_origin() {
        let base = parse_url("https://pve.lan:8006/").unwrap();
        assert!(navigation_allowed(
            &base,
            &[],
            "https://pve.lan:8006/api2/json"
        ));
        assert!(navigation_allowed(&base, &[], "https://PVE.lan:8006/x"));
        assert!(navigation_allowed(&base, &[], "about:blank"));
        assert!(!navigation_allowed(&base, &[], "https://pve.lan:8007/"));
        assert!(!navigation_allowed(&base, &[], "https://evil.example/"));
        assert!(!navigation_allowed(&base, &[], "http://pve.lan:8006/"));
        assert!(!navigation_allowed(&base, &[], "javascript:alert(1)"));
        assert!(!navigation_allowed(&base, &[], "file:///etc/passwd"));
        assert!(!navigation_allowed(&base, &[], "data:text/html,x"));
        let extra = parse_extra_hosts("sso.example.com, bad!host, idp.lan");
        assert_eq!(extra, vec!["sso.example.com", "idp.lan"]);
        assert!(navigation_allowed(
            &base,
            &extra,
            "https://sso.example.com/login"
        ));
        assert!(!navigation_allowed(
            &base,
            &extra,
            "http://sso.example.com/login"
        ));
        assert!(!navigation_allowed(
            &base,
            &extra,
            "https://sso.example.com.evil.net/"
        ));
    }

    #[test]
    fn fingerprints() {
        let fp = fingerprint_sha256(b"hello");
        assert_eq!(fp.len(), 95);
        assert!(fp.starts_with("2C:F2:4D:BA:5F:B0"));
        assert!(fingerprints_equal(&fp, &fp.to_lowercase().replace(':', "")));
        assert!(!fingerprints_equal(&fp, "AA:BB"));
        assert!(!fingerprints_equal("", ""));
        assert_eq!(normalize_fingerprint("zz"), None);
    }

    #[test]
    fn pem_decoding() {
        let pem = "-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n";
        assert_eq!(pem_to_der(pem).unwrap(), b"hello");
        assert!(pem_to_der("nothing").is_none());
    }

    #[test]
    fn pin_verdicts() {
        let a = fingerprint_sha256(b"a");
        let b = fingerprint_sha256(b"b");
        assert_eq!(
            tls_verdict(TlsMode::Strict, std::slice::from_ref(&a), &a),
            TlsVerdict::Refuse
        );
        assert_eq!(
            tls_verdict(TlsMode::InsecureAll, &[], &b),
            TlsVerdict::Allow
        );
        assert_eq!(tls_verdict(TlsMode::Pinned, &[], &a), TlsVerdict::Unknown);
        assert_eq!(
            tls_verdict(TlsMode::Pinned, std::slice::from_ref(&a), &a.to_lowercase()),
            TlsVerdict::Allow
        );
        assert_eq!(
            tls_verdict(TlsMode::Pinned, std::slice::from_ref(&a), &b),
            TlsVerdict::Mismatch { expected: a }
        );
        // Garbage never matches.
        assert_eq!(
            tls_verdict(TlsMode::Pinned, &["x".into()], "x"),
            TlsVerdict::Mismatch {
                expected: "x".into()
            }
        );
    }

    #[test]
    fn js_string_cannot_break_out() {
        for s in [
            "plain",
            "\"; alert(1); //",
            "'; alert(1); //",
            "</script><script>alert(1)</script>",
            "<!-- <script>",
            "a\\b\\",
            "line1\nline2\r\u{2028}\u{2029}",
            "`${alert(1)}`",
            "\0",
            "ünï",
        ] {
            let lit = js_string(s);
            assert!(lit.starts_with('"') && lit.ends_with('"'));
            for forbidden in ['<', '>', '&', '\'', '\n', '\r', '\u{2028}', '\u{2029}'] {
                assert!(!lit.contains(forbidden), "{s:?} -> {lit}");
            }
            // The literal is valid JSON and round-trips.
            let back: String = serde_json::from_str(&lit).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn autofill_script_embeds_only_escaped_literals() {
        let c = Credentials {
            username: "adm\"in</script>".into(),
            password: "p'w\\\n</script><script>alert(1)//".into(),
        };
        let js = autofill_script(&c, "https://a.lan", "#user\"]", "input[name='p']", true);
        assert!(!js.contains("</script>"));
        assert!(!js.contains("<script>"));
        assert!(js.contains(&format!("p:{},", js_string(&c.password))));
        assert!(js.contains("\\u003c/script\\u003e"));
        assert!(js.starts_with("(function(){var C={o:\"https://a.lan\",u:\"adm"));
        assert!(js.contains("location.origin !== C.o"));
        assert!(js.contains(",s:true}"));
        let js2 = autofill_script(&c, "https://a.lan", "", "", false);
        assert!(js2.contains(",us:\"\",ps:\"\",s:false}"));
        // A password that looks like the script's own placeholders stays inert.
        let c2 = Credentials {
            username: "u".into(),
            password: "}; evil(); var x={".into(),
        };
        let js3 = autofill_script(&c2, "https://a.lan", "", "", false);
        assert!(js3.contains(r#"p:"}; evil(); var x={""#));
    }

    #[test]
    fn credentials_debug_is_redacted() {
        let c = Credentials {
            username: "root".into(),
            password: "hunter2".into(),
        };
        let d = format!("{c:?}");
        assert!(!d.contains("hunter2") && !d.contains("root"));
        let s = WebStart {
            url: "https://a/".into(),
            title: "t".into(),
            profile_dir: "/p".into(),
            profile_id: "x".into(),
            tls: TlsMode::Strict,
            pinned_fp: String::new(),
            credentials: Some(c),
            user_selector: String::new(),
            pass_selector: String::new(),
            submit: false,
            extra_hosts: vec![],
            download_dir: String::new(),
        };
        assert!(!format!("{s:?}").contains("hunter2"));
    }

    #[test]
    fn download_names() {
        assert_eq!(safe_download_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_download_name("C:\\x\\evil.exe"), "evil.exe");
        assert_eq!(safe_download_name("a<b>:c|d?.txt"), "abcd.txt");
        assert_eq!(safe_download_name("..."), "download");
        assert_eq!(safe_download_name(""), "download");
        assert_eq!(safe_download_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_download_name("backup.tar.gz"), "backup.tar.gz");
        assert_eq!(safe_download_name(&"x".repeat(500)).len(), 120);
    }

    #[test]
    fn messages_round_trip() {
        let cmd = WebCmd::TrustCert {
            fingerprint: fingerprint_sha256(b"x"),
        };
        let line = serde_json::to_string(&cmd).unwrap();
        assert_eq!(parse_cmd(&line).unwrap(), cmd);
        assert!(parse_cmd(r#"{"t":"trust_cert","fingerprint":"nope"}"#).is_err());
        assert!(parse_cmd(r#"{"t":"rm_rf"}"#).is_err());
        assert!(parse_cmd("garbage").is_err());
        assert_eq!(parse_cmd(r#"{"t":"reload"}"#).unwrap(), WebCmd::Reload);
        let ev = WebEvent::Cert {
            fingerprint: "AA".into(),
            expected: None,
            strict: false,
        };
        let back: WebEvent = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn start_validation() {
        let ok = r#"{"url":"https://a.lan/","title":"A","profile_dir":"/tmp/p","profile_id":"1"}"#;
        assert!(parse_start(ok).is_ok());
        let bad_url =
            r#"{"url":"javascript:alert(1)","title":"A","profile_dir":"/tmp/p","profile_id":"1"}"#;
        assert!(parse_start(bad_url).is_err());
        let no_dir = r#"{"url":"https://a/","title":"A","profile_dir":"","profile_id":"1"}"#;
        assert!(parse_start(no_dir).is_err());
        let bad_fp = r#"{"url":"https://a/","title":"A","profile_dir":"/p","profile_id":"1","tls":"pinned","pinned_fp":"zz"}"#;
        assert!(parse_start(bad_fp).is_err());
    }

    #[test]
    fn bounded_lines() {
        let mut r = io::Cursor::new(b"abc\ndef\r\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut r, 10).unwrap().as_deref(),
            Some("abc")
        );
        assert_eq!(
            read_bounded_line(&mut r, 10).unwrap().as_deref(),
            Some("def")
        );
        assert_eq!(read_bounded_line(&mut r, 10).unwrap(), None);
        let mut long = vec![b'x'; 100];
        long.extend_from_slice(b"\nok\n");
        let mut r = io::Cursor::new(long);
        assert!(read_bounded_line(&mut r, 10).is_err());
        // The next line is intact after the oversized one.
        assert_eq!(
            read_bounded_line(&mut r, 10).unwrap().as_deref(),
            Some("ok")
        );
        let mut r = io::Cursor::new(vec![0xff, b'\n']);
        assert!(read_bounded_line(&mut r, 10).is_err());
    }

    #[test]
    fn short_url_drops_query() {
        assert_eq!(short_url("https://a/b?token=secret#x"), "https://a/b…");
        assert_eq!(short_url("https://a/b"), "https://a/b");
    }
}
