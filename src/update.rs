//! Update check against the GitHub releases of this project, plus a one-click update:
//! the matching asset is downloaded, verified against the release's SHA256SUMS file and
//! installed the way Aurora was installed (Windows installer, .deb, or portable folder).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::backend::Sink;

pub const REPO: &str = "theslasher5g/ssh-client";

pub type Version = (u32, u32, u32);

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn version_text(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

#[derive(Debug)]
pub enum UpdateEvent {
    /// `Ok(None)`: already up to date.
    Checked(Result<Option<Release>, String>),
    Progress(f32),
    /// Installed; `true` = Aurora should quit now (installer takes over / restarts it).
    Installed(Result<bool, String>),
}

/// How this copy of Aurora was installed, which decides how to update it.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallKind {
    WindowsInstaller,
    WindowsPortable,
    LinuxDeb,
    LinuxPortable(PathBuf),
}

impl InstallKind {
    pub fn detect() -> InstallKind {
        let exe = std::env::current_exe().unwrap_or_default();
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        if cfg!(windows) {
            if dir.join("unins000.exe").exists() {
                InstallKind::WindowsInstaller
            } else {
                InstallKind::WindowsPortable
            }
        } else if exe.starts_with("/usr") {
            InstallKind::LinuxDeb
        } else {
            InstallKind::LinuxPortable(dir)
        }
    }

    /// Release asset to download, by file-name suffix.
    fn asset_suffix(&self) -> Option<&'static str> {
        match self {
            InstallKind::WindowsInstaller => Some("-windows-x64-setup.exe"),
            InstallKind::WindowsPortable => None,
            InstallKind::LinuxDeb => Some("_amd64.deb"),
            InstallKind::LinuxPortable(_) => Some("-linux-x86_64.tar.gz"),
        }
    }

    pub fn can_self_update(&self) -> bool {
        self.asset_suffix().is_some()
    }
}

/// `v0.2.0`, `0.2.0`, `v0.1.0-windows` → (0, 2, 0).
pub fn parse_version(tag: &str) -> Option<Version> {
    let core = tag.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

fn agent() -> ureq::Agent {
    agent_with(10)
}

fn agent_with(max_redirects: u32) -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(max_redirects)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .user_agent(concat!("Aurora/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(Duration::from_secs(120)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

pub fn check(sink: Sink<UpdateEvent>) {
    std::thread::spawn(move || {
        let r = latest().map_err(|e| format!("{e:#}"));
        sink.send(UpdateEvent::Checked(r));
    });
}

/// Newest published release that has a download for this system, if newer than this build.
fn latest() -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=15");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("GitHub nicht erreichbar")?;
    if !resp.status().is_success() {
        bail!("GitHub antwortet mit HTTP {}", resp.status());
    }
    let json: serde_json::Value = serde_json::from_str(&resp.body_mut().read_to_string()?)?;
    let list = json
        .as_array()
        .ok_or_else(|| anyhow!("unerwartete Antwort von GitHub"))?;
    Ok(pick(list, &InstallKind::detect()).filter(|r| r.version > current()))
}

/// Highest stable release that fits this install (any release if we cannot self-update).
fn pick(list: &[serde_json::Value], kind: &InstallKind) -> Option<Release> {
    list.iter()
        .filter(|r| {
            !r.get("draft").and_then(|v| v.as_bool()).unwrap_or(false)
                && !r
                    .get("prerelease")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
        })
        .filter_map(parse_release)
        .filter(|r| match kind.asset_suffix() {
            Some(sfx) => r.assets.iter().any(|a| a.name.ends_with(sfx)),
            None => true,
        })
        .max_by_key(|r| r.version)
}

fn parse_release(json: &serde_json::Value) -> Option<Release> {
    let tag = json.get("tag_name")?.as_str()?;
    let assets = json
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some(Asset {
                name: a.get("name")?.as_str()?.to_string(),
                url: a.get("browser_download_url")?.as_str()?.to_string(),
            })
        })
        .collect();
    Some(Release {
        version: parse_version(tag)?,
        page: json
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        assets,
    })
}

pub fn install(release: Release, sink: Sink<UpdateEvent>) {
    std::thread::spawn(move || {
        let r = install_blocking(&release, &sink).map_err(|e| format!("{e:#}"));
        sink.send(UpdateEvent::Installed(r));
    });
}

fn install_blocking(release: &Release, sink: &Sink<UpdateEvent>) -> Result<bool> {
    let kind = InstallKind::detect();
    let suffix = kind
        .asset_suffix()
        .ok_or_else(|| anyhow!("Portable Version: bitte neu herunterladen"))?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(suffix))
        .ok_or_else(|| anyhow!("Release enthält keine Datei *{suffix}"))?;
    let sums = release
        .assets
        .iter()
        .find(|a| a.name.starts_with("SHA256SUMS"))
        .ok_or_else(|| anyhow!("Release enthält keine Prüfsummen"))?;

    let sums = String::from_utf8(download(&sums.url, None)?)?;
    let expected = expected_hash(&sums, &asset.name)
        .ok_or_else(|| anyhow!("keine Prüfsumme für {}", asset.name))?;
    let data = download(&asset.url, Some(sink))?;
    let actual = hex(&Sha256::digest(&data));
    if actual != expected {
        bail!("Prüfsumme stimmt nicht – Download verworfen");
    }

    let dir = std::env::temp_dir().join(format!("aurora-update-{}", release.version_text()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(&asset.name);
    std::fs::write(&file, &data)?;

    match kind {
        InstallKind::WindowsInstaller => {
            // The installer closes Aurora, replaces it and starts it again.
            std::process::Command::new(&file)
                .args([
                    "/SILENT",
                    "/SUPPRESSMSGBOXES",
                    "/NORESTART",
                    "/CLOSEAPPLICATIONS",
                ])
                .spawn()
                .context("Installer konnte nicht gestartet werden")?;
            Ok(true)
        }
        InstallKind::LinuxDeb => {
            let status = std::process::Command::new("pkexec")
                .args(["apt-get", "install", "-y"])
                .arg(&file)
                .status()
                .context("pkexec nicht gefunden – bitte manuell installieren")?;
            if !status.success() {
                bail!("Installation abgebrochen ({status})");
            }
            Ok(false)
        }
        InstallKind::LinuxPortable(target) => {
            let status = std::process::Command::new("tar")
                .arg("-xzf")
                .arg(&file)
                .arg("-C")
                .arg(&dir)
                .status()?;
            if !status.success() {
                bail!("Entpacken fehlgeschlagen");
            }
            let unpacked = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.is_dir())
                .ok_or_else(|| anyhow!("Archiv ist leer"))?;
            let exe = std::env::current_exe()?;
            replace_file(&unpacked.join("aurora"), &exe)?;
            let helper = target.join("aurora-rdp");
            if helper.exists() || unpacked.join("aurora-rdp").exists() {
                replace_file(&unpacked.join("aurora-rdp"), &helper)?;
            }
            Ok(false)
        }
        InstallKind::WindowsPortable => unreachable!(),
    }
}

/// Atomically swaps `dst` for `src` (the running binary can be replaced on Unix).
fn replace_file(src: &Path, dst: &Path) -> Result<()> {
    let tmp = dst.with_extension("new");
    std::fs::copy(src, &tmp).with_context(|| format!("{} nicht beschreibbar", dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

/// GET that follows redirects itself, without reading the redirect bodies (some proxies
/// mangle the chunked body of GitHub's 302 to the asset CDN).
fn get_following(url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    let agent = agent_with(0);
    let mut url = url.to_string();
    for _ in 0..8 {
        let resp = agent.get(&url).call().context("Download fehlgeschlagen")?;
        let status = resp.status();
        if status.is_redirection() {
            let next = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("Weiterleitung ohne Ziel"))?;
            if !next.starts_with("https://") {
                bail!("unsichere Weiterleitung");
            }
            url = next.to_string();
            continue;
        }
        if !status.is_success() {
            bail!("Download fehlgeschlagen: HTTP {status}");
        }
        return Ok(resp);
    }
    bail!("zu viele Weiterleitungen")
}

fn download(url: &str, progress: Option<&Sink<UpdateEvent>>) -> Result<Vec<u8>> {
    let mut resp = get_following(url)?;
    let total = resp.body().content_length().unwrap_or(0);
    let mut reader = resp
        .body_mut()
        .with_config()
        .limit(512 * 1024 * 1024)
        .reader();
    let mut out = Vec::with_capacity(total as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if let Some(s) = progress
            && total > 0
        {
            s.send(UpdateEvent::Progress(out.len() as f32 / total as f32));
        }
    }
    Ok(out)
}

/// Looks up `name` in a `sha256sum`-style file (`<hash>  <name>` per line).
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        (file == name).then(|| hash.to_ascii_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Opens a URL in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let r = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        log::warn!("Browser konnte nicht geöffnet werden: {e}");
    }
}

/// Starts a fresh copy of Aurora (used after an in-place update).
pub fn restart() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("v0.1.0-windows"), Some((0, 1, 0)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("latest"), None);
        assert!(parse_version("v0.10.0").unwrap() > parse_version("v0.9.9").unwrap());
    }

    #[test]
    fn checksums() {
        let sums = "abc123  Aurora-0.2.0-windows-x64-setup.exe\nDEF456 *aurora_0.2.0_amd64.deb\n";
        assert_eq!(
            expected_hash(sums, "aurora_0.2.0_amd64.deb").as_deref(),
            Some("def456")
        );
        assert_eq!(expected_hash(sums, "other.zip"), None);
    }

    #[test]
    fn picks_release_for_platform() {
        let list = vec![
            serde_json::json!({"tag_name": "v0.3.0-windows", "html_url": "",
                "assets": [{"name": "Aurora-0.3.0-windows-x64-setup.exe", "browser_download_url": "w"}]}),
            serde_json::json!({"tag_name": "v0.2.0", "html_url": "",
                "assets": [{"name": "aurora_0.2.0_amd64.deb", "browser_download_url": "d"}]}),
            serde_json::json!({"tag_name": "v0.4.0", "prerelease": true, "html_url": "",
                "assets": [{"name": "aurora_0.4.0_amd64.deb", "browser_download_url": "d"}]}),
        ];
        assert_eq!(
            pick(&list, &InstallKind::LinuxDeb).unwrap().version,
            (0, 2, 0)
        );
        assert_eq!(
            pick(&list, &InstallKind::WindowsInstaller).unwrap().version,
            (0, 3, 0)
        );
        assert_eq!(
            pick(&list, &InstallKind::WindowsPortable).unwrap().version,
            (0, 3, 0)
        );
    }

    #[test]
    fn release_json() {
        let json = serde_json::json!({
            "tag_name": "v0.3.1",
            "html_url": "https://github.com/x/y/releases/tag/v0.3.1",
            "body": "notes",
            "assets": [{"name": "SHA256SUMS.txt", "browser_download_url": "https://u/s"}]
        });
        let r = parse_release(&json).unwrap();
        assert_eq!(r.version, (0, 3, 1));
        assert_eq!(r.assets[0].name, "SHA256SUMS.txt");
    }
}
