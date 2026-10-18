//! Aurora as a small SSH certificate authority: the keychain holds a CA key, and each
//! SSH login can use a fresh Ed25519 key with a short-lived user certificate signed by it
//! (the server trusts the CA via `TrustedUserCAKeys`, no `authorized_keys` entry needed).

use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use rand::RngCore;
use russh::keys::ssh_key::certificate::{Builder, CertType};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{Certificate, PrivateKey, PublicKey};
use russh::keys::{HashAlg, decode_secret_key};

use crate::model::{CertSettings, Host, Keychain, SshCa};

/// Where the setup commands put the CA public key on the server.
pub const CA_PATH: &str = "/etc/ssh/aurora_ca.pub";

/// Validity choices in the host editor (hours).
pub const VALIDITY_HOURS: [u32; 3] = [1, 8, 24];

/// Certificates start this long in the past, so a server clock that runs a little
/// behind still accepts them.
const CLOCK_SKEW_SECS: u64 = 5 * 60;

/// Extensions of every user certificate: terminal, port forwarding, `~/.ssh/rc`.
/// No agent or X11 forwarding.
pub const EXTENSIONS: [&str; 3] = ["permit-port-forwarding", "permit-pty", "permit-user-rc"];

/// What one SSH connection needs to log in with a certificate.
#[derive(Clone)]
pub struct CertLogin {
    /// The CA private key (OpenSSH format).
    pub ca_key: String,
    pub principals: Vec<String>,
    pub validity_secs: u64,
    /// Filled in by the connection: accepted or why not.
    pub outcome: Arc<Mutex<Option<CertOutcome>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CertOutcome {
    /// Logged in; the certificate expires at this Unix time.
    Accepted { valid_before: u64, serial: u64 },
    /// Server said no (or signing failed); Aurora fell back to the normal identity.
    Rejected(String),
}

/// A new CA key pair (Ed25519).
pub fn create_ca() -> Result<SshCa> {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    let comment = if host.trim().is_empty() {
        "aurora-ca".to_string()
    } else {
        format!("aurora-ca@{}", host.trim())
    };
    let (private_key, public_key) = crate::keygen::generate_ed25519(&comment)?;
    Ok(SshCa {
        private_key,
        public_key,
        created: Some(chrono::Utc::now()),
    })
}

/// `SHA256:…` of the CA public key.
pub fn ca_fingerprint(ca: &SshCa) -> Option<String> {
    PublicKey::from_openssh(ca.public_key.trim())
        .ok()
        .map(|k| k.fingerprint(HashAlg::Sha256).to_string())
}

/// Principals typed in the host editor (comma or space separated); empty = `user`.
pub fn principals(text: &str, user: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in text.split([',', ' ', ';']).map(str::trim) {
        if !p.is_empty() && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    if out.is_empty() && !user.trim().is_empty() {
        out.push(user.trim().to_string());
    }
    out
}

/// The certificate login for `host`, if it is switched on and the keychain has a CA.
pub fn login_for(host: &Host, user: &str, keychain: &Keychain) -> Option<Box<CertLogin>> {
    let s: &CertSettings = &host.cert;
    let ca = keychain.ca.as_ref()?;
    if !s.enabled || ca.private_key.trim().is_empty() {
        return None;
    }
    let hours = if s.hours == 0 { 8 } else { s.hours.min(24 * 7) };
    Some(Box::new(CertLogin {
        ca_key: ca.private_key.clone(),
        principals: principals(&s.principals, user),
        validity_secs: hours as u64 * 3600,
        outcome: Arc::new(Mutex::new(None)),
    }))
}

/// A fresh Ed25519 key that only lives in memory for one connection.
pub fn ephemeral_key() -> Result<PrivateKey> {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)),
        "aurora-ephemeral",
    )
    .map_err(|e| anyhow!("{e}"))
}

/// Signs `subject` as an OpenSSH user certificate valid from `now - 5 min` to
/// `now + validity_secs`.
pub fn sign_user_cert(
    ca: &PrivateKey,
    subject: &PublicKey,
    principals: &[String],
    key_id: &str,
    validity_secs: u64,
    now: u64,
) -> Result<Certificate> {
    if principals.is_empty() {
        // An empty list would make the certificate valid for every user.
        anyhow::bail!(t!(
            "Zertifikat ohne Principal (Benutzer) – nicht ausgestellt"
        ));
    }
    let mut nonce = vec![0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut b = Builder::new(
        nonce,
        subject.key_data().clone(),
        now.saturating_sub(CLOCK_SKEW_SECS),
        now + validity_secs,
    )
    .map_err(|e| anyhow!("{e}"))?;
    let e = |e: russh::keys::ssh_key::Error| anyhow!("{e}");
    b.serial(rand::rngs::OsRng.next_u64()).map_err(e)?;
    b.cert_type(CertType::User).map_err(e)?;
    b.key_id(key_id).map_err(e)?;
    for p in principals {
        b.valid_principal(p.as_str()).map_err(e)?;
    }
    for ext in EXTENSIONS {
        b.extension(ext, "").map_err(e)?;
    }
    b.comment(key_id).map_err(e)?;
    b.sign(ca).map_err(e)
}

/// Key ID written into the certificate (shows up in the server's auth log).
pub fn key_id(user: &str, host: &str, now: u64) -> String {
    let when = chrono::DateTime::from_timestamp(now as i64, 0)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| now.to_string());
    format!("aurora:{user}@{host}:{when}")
}

/// Ephemeral key + certificate for one login.
pub fn issue(login: &CertLogin, user: &str, host: &str) -> Result<(PrivateKey, Certificate)> {
    let ca = decode_secret_key(&login.ca_key, None).map_err(|e| anyhow!("{e}"))?;
    let key = ephemeral_key()?;
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let cert = sign_user_cert(
        &ca,
        key.public_key(),
        &login.principals,
        &key_id(user, host, now),
        login.validity_secs,
        now,
    )?;
    Ok((key, cert))
}

/// One shell line that installs the CA public key on a server and tells sshd to trust
/// it (drop-in file where `sshd_config.d` is included, else the top of `sshd_config`,
/// before any `Match` block), checks the config and reloads sshd. Needs sudo.
pub fn setup_command(ca_public: &str) -> Option<String> {
    let key = ca_public.trim();
    let safe = key.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '@' | '.' | '_' | '-' | ' ')
    });
    if !safe || !key.starts_with("ssh-") {
        return None;
    }
    let line = format!("TrustedUserCAKeys {CA_PATH}");
    Some(format!(
        "sudo sh -c \"printf '%s\\n' '{key}' > {CA_PATH} && chmod 644 {CA_PATH} && \
         if grep -qs '^{line}' /etc/ssh/sshd_config /etc/ssh/sshd_config.d/*.conf; then :; \
         elif grep -qsi '^Include /etc/ssh/sshd_config.d' /etc/ssh/sshd_config; \
         then echo '{line}' > /etc/ssh/sshd_config.d/50-aurora-ca.conf; \
         else sed -i '1i {line}' /etc/ssh/sshd_config; fi && sshd -t && \
         (systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || service ssh reload) && \
         echo Aurora-CA OK\""
    ))
}

/// The same steps for reading and copying by hand.
pub fn manual_steps(ca_public: &str) -> String {
    format!(
        "echo '{}' | sudo tee {CA_PATH}\n# {}\nTrustedUserCAKeys {CA_PATH}\nsudo sshd -t && sudo systemctl reload ssh   # {}",
        ca_public.trim(),
        t!("in /etc/ssh/sshd_config eintragen (vor dem ersten Match-Block):"),
        t!("oder: systemctl reload sshd")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ca() -> PrivateKey {
        let ca = create_ca().unwrap();
        decode_secret_key(&ca.private_key, None).unwrap()
    }

    #[test]
    fn certificate_is_signed_by_the_ca() {
        let ca = ca();
        let key = ephemeral_key().unwrap();
        let now = 1_800_000_000;
        let cert = sign_user_cert(
            &ca,
            key.public_key(),
            &["aurora".into(), "deploy".into()],
            &key_id("aurora", "lab", now),
            3600,
            now,
        )
        .unwrap();
        let ca_fp = ca.public_key().fingerprint(HashAlg::Sha256);
        // Signature + validity window against the trusted CA.
        cert.validate_at(now, [&ca_fp]).unwrap();
        assert!(cert.validate_at(now + 3600, [&ca_fp]).is_err());
        assert!(cert.validate_at(now - 301, [&ca_fp]).is_err());
        cert.validate_at(now - 300, [&ca_fp]).unwrap();
        // Another CA does not validate it.
        let other = ca_fingerprint(&create_ca().unwrap()).unwrap();
        assert_ne!(other, ca_fp.to_string());
        assert_eq!(cert.valid_after(), now - 300);
        assert_eq!(cert.valid_before(), now + 3600);
        assert!(cert.cert_type().is_user());
        assert_eq!(cert.valid_principals(), ["aurora", "deploy"]);
        assert_eq!(cert.key_id(), "aurora:aurora@lab:2027-01-15T08:00:00Z");
        assert!(cert.critical_options().is_empty());
        let ext: Vec<&str> = cert.extensions().keys().map(String::as_str).collect();
        assert_eq!(ext, EXTENSIONS);
        assert_eq!(cert.public_key(), key.public_key().key_data());
        assert_eq!(cert.algorithm().as_str(), "ssh-ed25519");
        // Survives the OpenSSH text form (what `ssh -i key-cert.pub` reads).
        let text = cert.to_openssh().unwrap();
        assert!(text.starts_with("ssh-ed25519-cert-v01@openssh.com "));
        let back = Certificate::from_openssh(&text).unwrap();
        back.validate_at(now, [&ca_fp]).unwrap();
    }

    #[test]
    fn no_principal_no_certificate() {
        let ca = ca();
        let key = ephemeral_key().unwrap();
        assert!(sign_user_cert(&ca, key.public_key(), &[], "x", 60, 1_800_000_000).is_err());
    }

    #[test]
    fn principals_default_to_the_user() {
        assert_eq!(principals("", "root"), ["root"]);
        assert_eq!(principals(" a, b ,a;c ", "root"), ["a", "b", "c"]);
        assert!(principals("", "").is_empty());
    }

    #[test]
    fn login_needs_switch_and_ca() {
        let mut kc = Keychain::default();
        let mut host = Host::default();
        host.cert.enabled = true;
        assert!(login_for(&host, "u", &kc).is_none());
        kc.ca = Some(create_ca().unwrap());
        let l = login_for(&host, "u", &kc).unwrap();
        assert_eq!(l.principals, ["u"]);
        assert_eq!(l.validity_secs, 8 * 3600);
        host.cert.enabled = false;
        assert!(login_for(&host, "u", &kc).is_none());
    }

    #[test]
    fn issued_certificate_matches_the_ephemeral_key() {
        let ca = create_ca().unwrap();
        let login = CertLogin {
            ca_key: ca.private_key.clone(),
            principals: vec!["aurora".into()],
            validity_secs: 3600,
            outcome: Default::default(),
        };
        let (key, cert) = issue(&login, "aurora", "127.0.0.1").unwrap();
        assert_eq!(cert.public_key(), key.public_key().key_data());
        let fp = PublicKey::from_openssh(&ca.public_key)
            .unwrap()
            .fingerprint(HashAlg::Sha256);
        cert.validate([&fp]).unwrap();
        assert!(cert.key_id().starts_with("aurora:aurora@127.0.0.1:"));
    }

    #[test]
    fn setup_command_only_for_clean_keys() {
        let ca = create_ca().unwrap();
        let cmd = setup_command(&ca.public_key).unwrap();
        assert!(
            cmd.contains(CA_PATH) && cmd.contains("TrustedUserCAKeys") && cmd.contains("sshd -t")
        );
        assert!(setup_command("ssh-ed25519 AAAA'; rm -rf /").is_none());
        assert!(setup_command("garbage").is_none());
    }
}
