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

#[cfg(test)]
mod tests {
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
