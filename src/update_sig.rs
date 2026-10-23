//! Release signature of `SHA256SUMS.txt`: a plain Ed25519 signature (`SHA256SUMS.txt.sig`,
//! 64 raw bytes or their base64 text) made in the release workflow with
//! `openssl pkeyutl -sign -rawin`. The public key is compiled into the app
//! (`assets/release-signing-key.txt`, base64 of the 32 raw bytes).

use anyhow::{Result, bail};
use base64::Engine;

const EMBEDDED_KEY: &str = include_str!("../assets/release-signing-key.txt");

/// What the signature check of a checksum file found.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Trust {
    /// A signature exists and is valid for the embedded key.
    Verified,
    /// The release has no signature (older release / no key configured on the project side).
    NoSignature,
    /// The release has a signature but this build contains no public key to check it with.
    NoKey,
}

/// First non-comment line of the key file, decoded; `None` while no key is set.
pub fn parse_pubkey(text: &str) -> Option<[u8; 32]> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(line)
        .ok()?;
    bytes.try_into().ok()
}

pub fn embedded_key() -> Option<[u8; 32]> {
    parse_pubkey(EMBEDDED_KEY)
}

/// Accepts the raw 64-byte signature or its base64 text.
pub fn parse_signature(data: &[u8]) -> Option<[u8; 64]> {
    if let Ok(raw) = <[u8; 64]>::try_from(data) {
        return Some(raw);
    }
    let text = std::str::from_utf8(data).ok()?.trim();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    bytes.try_into().ok()
}

/// Verifies `sums` against `sig` with `key`. Any failure is an error.
pub fn verify(sums: &[u8], sig: &[u8], key: &[u8; 32]) -> Result<()> {
    let Some(sig) = parse_signature(sig) else {
        bail!(t!("Signaturdatei ist ungültig"));
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(sums, &sig)
        .map_err(|_| anyhow::anyhow!(t!("Signatur der Prüfsummen ist ungültig")))
}

/// Applies the policy: a present signature MUST verify (error = hard fail); a missing one
/// (or a build without key) is reported so the caller can ask the user.
pub fn check(sums: &[u8], sig: Option<&[u8]>, key: Option<&[u8; 32]>) -> Result<Trust> {
    match (sig, key) {
        (None, _) => Ok(Trust::NoSignature),
        (Some(_), None) => Ok(Trust::NoKey),
        (Some(sig), Some(key)) => verify(sums, sig, key).map(|()| Trust::Verified),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    // Throwaway key made with `openssl genpkey -algorithm ed25519`; the signature with
    // `openssl pkeyutl -sign -rawin -in SHA256SUMS.txt`.
    const PUB: &str = "WhEYH6y+tUHgDnASGIHdR9zUyGFTndIqxvAwSk2qVvk=";
    const SUMS: &str =
        "aaaa1111  Aurora-0.9.9-windows-x64-setup.exe\nbbbb2222  aurora_0.9.9_amd64.deb\n";
    const SIG: &str =
        "EgzC/P9pUO27RoPJKsAXPUAmvpu6GzoMehtIHA9S5YBjSschUxxqwVPAT1wmHhdWKhk81ZZMf/KyYXTBIxckCQ==";

    fn key() -> [u8; 32] {
        parse_pubkey(PUB).unwrap()
    }

    fn raw_sig() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(SIG)
            .unwrap()
    }

    #[test]
    fn valid_openssl_signature() {
        assert_eq!(
            check(SUMS.as_bytes(), Some(SIG.as_bytes()), Some(&key())).unwrap(),
            Trust::Verified
        );
        // raw 64-byte form (what the workflow publishes)
        let raw = raw_sig();
        assert_eq!(raw.len(), 64);
        assert_eq!(
            check(SUMS.as_bytes(), Some(&raw), Some(&key())).unwrap(),
            Trust::Verified
        );
    }

    #[test]
    fn tampered_file_is_rejected() {
        let bad = SUMS.replace("bbbb2222", "cccc3333");
        assert!(check(bad.as_bytes(), Some(SIG.as_bytes()), Some(&key())).is_err());
        let mut sig = raw_sig();
        sig[0] ^= 1;
        assert!(check(SUMS.as_bytes(), Some(&sig), Some(&key())).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let other = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let other_pub: [u8; 32] = other.public_key().as_ref().try_into().unwrap();
        assert!(check(SUMS.as_bytes(), Some(SIG.as_bytes()), Some(&other_pub)).is_err());
        // a signature made by the other key does not verify with ours, but with its own
        let sig = other.sign(SUMS.as_bytes());
        assert!(check(SUMS.as_bytes(), Some(sig.as_ref()), Some(&key())).is_err());
        assert_eq!(
            check(SUMS.as_bytes(), Some(sig.as_ref()), Some(&other_pub)).unwrap(),
            Trust::Verified
        );
    }

    #[test]
    fn missing_signature_or_key() {
        assert_eq!(
            check(SUMS.as_bytes(), None, Some(&key())).unwrap(),
            Trust::NoSignature
        );
        assert_eq!(
            check(SUMS.as_bytes(), None, None).unwrap(),
            Trust::NoSignature
        );
        assert_eq!(
            check(SUMS.as_bytes(), Some(SIG.as_bytes()), None).unwrap(),
            Trust::NoKey
        );
    }

    #[test]
    fn garbage_signature_is_rejected() {
        assert!(check(SUMS.as_bytes(), Some(b"nonsense"), Some(&key())).is_err());
        assert!(check(SUMS.as_bytes(), Some(b""), Some(&key())).is_err());
    }

    #[test]
    fn key_file_parsing() {
        assert_eq!(parse_pubkey("# only a comment\n\n"), None);
        assert_eq!(parse_pubkey(&format!("# c\n{PUB}\n")).unwrap(), key());
        assert_eq!(parse_pubkey("AAAA"), None);
    }

    #[test]
    fn embedded_key_is_empty_or_valid() {
        // Either no key yet, or a well-formed one - never a half-pasted line.
        let line = EMBEDDED_KEY
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'));
        if line.is_some() {
            assert!(embedded_key().is_some());
        }
    }
}
