//! SSH key helpers for the keychain.

use anyhow::{Result, anyhow};
use rand::RngCore;
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{LineEnding, PrivateKey};
use russh::keys::{HashAlg, decode_secret_key};

/// Generates a fresh Ed25519 key pair; returns (private OpenSSH PEM, public authorized_keys line).
pub fn generate_ed25519(comment: &str) -> Result<(String, String)> {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let key = PrivateKey::new(
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)),
        comment,
    )
    .map_err(|e| anyhow!("{e}"))?;
    let private = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| anyhow!("{e}"))?
        .to_string();
    let public = key.public_key().to_openssh().map_err(|e| anyhow!("{e}"))?;
    Ok((private, public))
}

/// Derives (authorized_keys line, SHA256 fingerprint) from a private key.
pub fn public_info(private: &str, passphrase: &str) -> Result<(String, String)> {
    let key = decode_secret_key(private, (!passphrase.is_empty()).then_some(passphrase))
        .map_err(|e| anyhow!("{e}"))?;
    let public = key.public_key().to_openssh().map_err(|e| anyhow!("{e}"))?;
    Ok((
        public,
        key.public_key().fingerprint(HashAlg::Sha256).to_string(),
    ))
}

/// What the keys page shows and searches for a private key.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyMeta {
    /// "Ed25519", "RSA 4096", "ECDSA P-256", …
    pub kind: String,
    pub comment: String,
    /// `SHA256:…`
    pub fingerprint: String,
}

/// Type, comment and fingerprint without the passphrase: the OpenSSH format keeps the
/// public half in clear text (the comment of an encrypted key stays hidden). `None` for
/// text that is no key, or a legacy PEM key with a passphrase.
pub fn key_meta(private: &str) -> Option<KeyMeta> {
    let key = match PrivateKey::from_openssh(private.trim()) {
        Ok(k) => k,
        Err(_) => decode_secret_key(private, None).ok()?,
    };
    let public = key.public_key();
    let algo = public.algorithm();
    let kind = match public.key_data().rsa() {
        Some(rsa) => format!("RSA {}", rsa.key_size()),
        None => match algo.as_str() {
            "ssh-ed25519" => "Ed25519".to_string(),
            "ecdsa-sha2-nistp256" => "ECDSA P-256".to_string(),
            "ecdsa-sha2-nistp384" => "ECDSA P-384".to_string(),
            "ecdsa-sha2-nistp521" => "ECDSA P-521".to_string(),
            "ssh-dss" => "DSA".to_string(),
            other => other.to_string(),
        },
    };
    Some(KeyMeta {
        kind,
        comment: key.comment().as_str_lossy().to_string(),
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn meta_without_passphrase() {
        let (private, _) = super::generate_ed25519("ops@aurora").unwrap();
        let meta = super::key_meta(&private).unwrap();
        assert_eq!(meta.kind, "Ed25519");
        assert_eq!(meta.comment, "ops@aurora");
        assert_eq!(
            meta.fingerprint,
            super::public_info(&private, "").unwrap().1
        );
        assert!(super::key_meta("not a key").is_none());
    }

    #[test]
    fn generated_key_roundtrips() {
        let (private, public) = super::generate_ed25519("test@aurora").unwrap();
        assert!(private.contains("OPENSSH PRIVATE KEY"));
        assert!(public.starts_with("ssh-ed25519 "));
        let (public2, fp) = super::public_info(&private, "").unwrap();
        assert_eq!(public, public2);
        assert!(fp.starts_with("SHA256:"));
    }
}
