//! On-disk persistence. The host library is plain JSON; the keychain lives in an
//! Argon2id + XChaCha20-Poly1305 encrypted vault unlocked by a master password.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::model::{Keychain, Library};

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AURORA_DATA_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("dev", "Aurora", "AuroraTerm")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".aurora-term"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn library_path(&self) -> PathBuf {
        self.dir.join("library.json")
    }

    fn vault_path(&self) -> PathBuf {
        self.dir.join("vault.json")
    }

    /// Returns the library, or `None` on first start.
    pub fn load_library(&self) -> Result<Option<Library>> {
        let path = self.library_path();
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("{}", path.display()))?;
        Ok(Some(serde_json::from_str(&text)?))
    }

    pub fn save_library(&self, lib: &Library) -> Result<()> {
        write_atomic(
            &self.library_path(),
            serde_json::to_string_pretty(lib)?.as_bytes(),
        )
    }

    pub fn vault_exists(&self) -> bool {
        self.vault_path().exists()
    }

    pub fn create_vault(&self, password: &str) -> Result<Vault> {
        let mut salt = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let vault = Vault {
            path: self.vault_path(),
            salt,
            key: derive_key(password, &salt)?,
        };
        vault.save(&Keychain::default())?;
        Ok(vault)
    }

    pub fn unlock_vault(&self, password: &str) -> Result<(Vault, Keychain)> {
        let text = fs::read_to_string(self.vault_path())?;
        let file: VaultFile = serde_json::from_str(&text)?;
        let salt: [u8; 16] = B64
            .decode(&file.salt)?
            .try_into()
            .map_err(|_| anyhow!(t!("ungültiger Salt")))?;
        let key = derive_key(password, &salt)?;
        let nonce = B64.decode(&file.nonce)?;
        if nonce.len() != 24 {
            bail!(t!("ungültige Nonce"));
        }
        let cipher = XChaCha20Poly1305::new((&key).into());
        let plain = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                B64.decode(&file.ciphertext)?.as_ref(),
            )
            .map_err(|_| anyhow!(t!("Falsches Master-Passwort")))?;
        let keychain = serde_json::from_slice(&plain)?;
        Ok((
            Vault {
                path: self.vault_path(),
                salt,
                key,
            },
            keychain,
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    kdf: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

fn derive_key(password: &str, salt: &[u8; 16]) -> Result<[u8; 32]> {
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(key)
}

/// An unlocked vault: holds the derived key so the keychain can be re-encrypted on save.
pub struct Vault {
    path: PathBuf,
    salt: [u8; 16],
    key: [u8; 32],
}

impl Vault {
    pub fn save(&self, keychain: &Keychain) -> Result<()> {
        let mut nonce = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let plain = serde_json::to_vec(keychain)?;
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plain.as_ref())
            .map_err(|_| anyhow!(t!("Verschlüsselung fehlgeschlagen")))?;
        let file = VaultFile {
            version: 1,
            kdf: "argon2id-m65536-t3-p1".into(),
            salt: B64.encode(self.salt),
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ciphertext),
        };
        write_atomic(&self.path, serde_json::to_string_pretty(&file)?.as_bytes())
    }

    /// Re-key the vault with a new master password.
    pub fn change_password(&mut self, new_password: &str, keychain: &Keychain) -> Result<()> {
        rand::rngs::OsRng.fill_bytes(&mut self.salt);
        self.key = derive_key(new_password, &self.salt)?;
        self.save(keychain)
    }
}

/// A fresh directory only this user can access (`<tmp>/<prefix>-<random>`, mode 0700).
/// Never reuses an existing path, so nobody else can pre-create or swap its contents.
pub fn private_temp_dir(prefix: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(&dir)?;
    Ok(dir)
}

/// Everything worth moving to another PC, as written into an `.aurora` export file.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExportBundle {
    pub version: u32,
    pub hosts: Vec<crate::model::Host>,
    pub groups: Vec<crate::model::Group>,
    pub snippets: Vec<crate::model::Snippet>,
    pub identities: Vec<crate::model::Identity>,
    pub known_hosts: std::collections::HashMap<String, String>,
    pub key_groups: Vec<crate::model::KeyGroup>,
}

/// Encrypts with a password of its own (same Argon2id + XChaCha20-Poly1305 scheme as the
/// vault, fresh salt and nonce).
pub fn seal(password: &str, plain: &[u8]) -> Result<String> {
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let key = derive_key(password, &salt)?;
    let ciphertext = XChaCha20Poly1305::new((&key).into())
        .encrypt(XNonce::from_slice(&nonce), plain)
        .map_err(|_| anyhow!(t!("Verschlüsselung fehlgeschlagen")))?;
    let file = VaultFile {
        version: 1,
        kdf: "argon2id-m65536-t3-p1".into(),
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    };
    Ok(serde_json::to_string_pretty(&file)?)
}

pub fn unseal(password: &str, text: &str) -> Result<Vec<u8>> {
    let file: VaultFile =
        serde_json::from_str(text).map_err(|_| anyhow!(t!("Keine Aurora-Exportdatei")))?;
    let salt: [u8; 16] = B64
        .decode(&file.salt)?
        .try_into()
        .map_err(|_| anyhow!(t!("ungültiger Salt")))?;
    let nonce = B64.decode(&file.nonce)?;
    if nonce.len() != 24 {
        bail!(t!("ungültige Nonce"));
    }
    let key = derive_key(password, &salt)?;
    XChaCha20Poly1305::new((&key).into())
        .decrypt(
            XNonce::from_slice(&nonce),
            B64.decode(&file.ciphertext)?.as_ref(),
        )
        .map_err(|_| anyhow!(t!("Falsches Passwort oder beschädigte Datei")))
}

pub fn write_export(path: &Path, text: &str) -> Result<()> {
    write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_roundtrip() {
        let sealed = super::seal("pw-123456", b"hello").unwrap();
        assert_eq!(super::unseal("pw-123456", &sealed).unwrap(), b"hello");
        assert!(super::unseal("wrong", &sealed).is_err());
    }

    use super::*;
    use crate::model::Identity;

    #[test]
    fn vault_roundtrip_and_wrong_password() {
        let dir = std::env::temp_dir().join(format!("aurora-test-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir.clone());
        let vault = store.create_vault("hunter2").unwrap();
        let mut kc = Keychain::default();
        let mut id = Identity::new();
        id.password = "s3cret".into();
        kc.identities.push(id);
        vault.save(&kc).unwrap();

        let raw = fs::read_to_string(dir.join("vault.json")).unwrap();
        assert!(!raw.contains("s3cret"));

        let (_, loaded) = store.unlock_vault("hunter2").unwrap();
        assert_eq!(loaded.identities[0].password, "s3cret");
        assert!(store.unlock_vault("wrong").is_err());
        fs::remove_dir_all(dir).ok();
    }

    /// Keychains written before key groups existed load unchanged, without groups.
    #[test]
    fn old_keychain_loads_without_groups() {
        let id = uuid::Uuid::new_v4();
        let old = format!(
            r#"{{"identities":[{{"id":"{id}","label":"deploy","username":"root",
                "password":"pw","private_key":"","passphrase":""}}],
                "pve_secrets":{{}}}}"#
        );
        let kc: Keychain = serde_json::from_str(&old).unwrap();
        assert!(kc.groups.is_empty());
        let ident = &kc.identities[0];
        assert_eq!((ident.id, ident.label.as_str()), (id, "deploy"));
        assert_eq!(
            (ident.username.as_str(), ident.password.as_str()),
            ("root", "pw")
        );
        assert_eq!(ident.group_id, None);
        let grouped = kc.grouped();
        assert_eq!(grouped.len(), 1);
        assert!(grouped[0].0.is_none());
        assert_eq!(grouped[0].1.len(), 1);

        // Round trip with a group; a dangling group id counts as ungrouped.
        let mut kc = kc;
        let g = crate::model::KeyGroup {
            name: "Prod".into(),
            ..Default::default()
        };
        kc.identities[0].group_id = Some(g.id);
        kc.groups.push(g.clone());
        let mut stray = Identity::new();
        stray.group_id = Some(uuid::Uuid::new_v4());
        kc.identities.push(stray);
        let back: Keychain = serde_json::from_str(&serde_json::to_string(&kc).unwrap()).unwrap();
        assert_eq!(back.groups, vec![g.clone()]);
        let grouped = back.grouped();
        assert_eq!(grouped[0].0.map(|x| x.id), Some(g.id));
        assert_eq!(grouped[0].1[0].label, "deploy");
        assert_eq!(grouped[1].1.len(), 1);
    }
}
