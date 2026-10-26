//! Reads the CSV list of a dynamic folder from a file or an `https://` address: size cap,
//! timeouts, https only (redirects too), nothing logged that could hold a secret.

use std::io::Read;
use std::time::Duration;

use crate::nav2_dyn::{Location, MAX_BYTES, parse_location};

/// Downloads or reads the list text. Blocking: call from a worker thread.
pub fn load_list(location: &str) -> Result<String, String> {
    match parse_location(location)? {
        Location::File(p) => read_file(&p),
        Location::Https(u) => download(&u),
    }
}

fn read_file(path: &str) -> Result<String, String> {
    let path = std::path::Path::new(path);
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(t!("Das ist keine Datei.").to_string());
    }
    if meta.len() > MAX_BYTES {
        return Err(t!("Die Liste ist zu groß (höchstens 5 MB).").to_string());
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut buf).map(|_| ()))
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_BYTES {
        return Err(t!("Die Liste ist zu groß (höchstens 5 MB).").to_string());
    }
    Ok(crate::backend::import_model::decode_text(&buf))
}

fn download(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .user_agent(concat!("Aurora/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let mut url = url.to_string();
    for _ in 0..4 {
        let mut resp = agent.get(&url).call().map_err(|e| {
            t!(
                "Download fehlgeschlagen: {}",
                host_only(&url, &e.to_string())
            )
        })?;
        let status = resp.status();
        if status.is_redirection() {
            let next = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            // Only on to another https address (relative ones are resolved here).
            let next = if next.starts_with('/') {
                let origin: String = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                format!("{origin}{next}")
            } else {
                next
            };
            if !next.to_ascii_lowercase().starts_with("https://") {
                return Err(t!("Unsichere Weiterleitung abgelehnt").to_string());
            }
            parse_location(&next)?;
            url = next;
            continue;
        }
        if !status.is_success() {
            return Err(t!("Download fehlgeschlagen: HTTP {}", status.as_u16()));
        }
        if resp.body().content_length().is_some_and(|n| n > MAX_BYTES) {
            return Err(t!("Die Liste ist zu groß (höchstens 5 MB).").to_string());
        }
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_BYTES + 1)
            .read_to_vec()
            .map_err(|e| t!("Download fehlgeschlagen: {}", e))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(t!("Die Liste ist zu groß (höchstens 5 MB).").to_string());
        }
        return Ok(crate::backend::import_model::decode_text(&bytes));
    }
    Err(t!("Zu viele Weiterleitungen").to_string())
}

/// Error text without the URL (it may carry a path or query the user considers private).
fn host_only(url: &str, err: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let short: String = err.chars().take(120).collect();
    format!("{host}: {}", short.replace(url, "…"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_file_and_refuses_bad_sources() {
        let dir = std::env::temp_dir().join(format!("aurora-dyn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("list.csv");
        std::fs::write(&f, "name,host\na,10.0.0.1\n").unwrap();
        let text = load_list(f.to_str().unwrap()).unwrap();
        assert!(text.contains("10.0.0.1"));
        assert!(
            load_list(dir.to_str().unwrap()).is_err(),
            "a folder is no list"
        );
        assert!(load_list(dir.join("nope.csv").to_str().unwrap()).is_err());
        assert!(load_list("http://example.org/x.csv").is_err());
        assert!(load_list("https://u:p@example.org/x.csv").is_err());
        // Too big.
        let big = dir.join("big.csv");
        std::fs::write(&big, vec![b'a'; (MAX_BYTES + 10) as usize]).unwrap();
        assert!(load_list(big.to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn error_text_hides_the_path() {
        let s = host_only(
            "https://example.org/secret/list.csv?x=1",
            "boom https://example.org/secret/list.csv?x=1",
        );
        assert!(s.starts_with("example.org: "));
        assert!(!s.contains("secret"));
    }
}
