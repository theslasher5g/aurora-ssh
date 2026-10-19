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
        // Big inventories (megabytes) are written compactly: several times faster.
        let json = if lib.hosts.len() > 2000 {
            serde_json::to_string(lib)?
        } else {
            serde_json::to_string_pretty(lib)?
        };
        write_atomic(&self.library_path(), json.as_bytes())
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

/// A file encrypted with the vault key (no KDF of its own).
#[derive(Serialize, Deserialize)]
struct BlobFile {
    version: u32,
    nonce: String,
    ciphertext: String,
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

    /// Writes `plain` encrypted with the vault key to `name` next to the vault (data that
    /// is too large or too busy for the keychain, e.g. the command history).
    pub fn save_blob(&self, name: &str, plain: &[u8]) -> Result<()> {
        let mut nonce = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = XChaCha20Poly1305::new((&self.key).into())
            .encrypt(XNonce::from_slice(&nonce), plain)
            .map_err(|_| anyhow!(t!("Verschlüsselung fehlgeschlagen")))?;
        let file = BlobFile {
            version: 1,
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ciphertext),
        };
        write_atomic(
            &self.blob_path(name),
            serde_json::to_string(&file)?.as_bytes(),
        )
    }

    /// Reads a blob written by `save_blob`; `None` if there is none yet.
    pub fn load_blob(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.blob_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let file: BlobFile = serde_json::from_str(&fs::read_to_string(&path)?)?;
        let nonce = B64.decode(&file.nonce)?;
        if nonce.len() != 24 {
            bail!(t!("ungültige Nonce"));
        }
        XChaCha20Poly1305::new((&self.key).into())
            .decrypt(
                XNonce::from_slice(&nonce),
                B64.decode(&file.ciphertext)?.as_ref(),
            )
            .map(Some)
            .map_err(|_| anyhow!(t!("Falsches Passwort oder beschädigte Datei")))
    }

    fn blob_path(&self, name: &str) -> PathBuf {
        self.path.with_file_name(name)
    }

    /// Changes whenever the key changes (new master password): blobs must be written again.
    pub fn key_id(&self) -> u64 {
        use sha2::Digest;
        let d = sha2::Sha256::digest(self.key);
        u64::from_le_bytes(d[..8].try_into().unwrap_or_default())
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
    /// Proxmox VE / Backup servers and their secrets from the keychain (secrets only
    /// ever travel inside this encrypted bundle, never in `library.json`).
    pub pve_servers: Vec<crate::model::PveServer>,
    pub pve_secrets: std::collections::HashMap<uuid::Uuid, String>,
    pub pbs_servers: Vec<crate::model::PbsServer>,
    pub pbs_secrets: std::collections::HashMap<uuid::Uuid, String>,
}

impl ExportBundle {
    /// Adds the Proxmox servers that are not in `lib` yet (matched by id), with their
    /// secrets; returns how many were added.
    pub fn merge_proxmox(
        &mut self,
        lib: &mut crate::model::Library,
        keychain: &mut Keychain,
    ) -> usize {
        let mut n = 0;
        for s in std::mem::take(&mut self.pve_servers) {
            if lib.pve_servers.iter().any(|x| x.id == s.id) {
                continue;
            }
            if let Some(secret) = self.pve_secrets.remove(&s.id) {
                keychain.pve_secrets.insert(s.id, secret);
            }
            lib.pve_servers.push(s);
            n += 1;
        }
        for s in std::mem::take(&mut self.pbs_servers) {
            if lib.pbs_servers.iter().any(|x| x.id == s.id) {
                continue;
            }
            if let Some(secret) = self.pbs_secrets.remove(&s.id) {
                keychain.pbs_secrets.insert(s.id, secret);
            }
            lib.pbs_servers.push(s);
            n += 1;
        }
        n
    }
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

    /// Proxmox servers and their secrets survive an export / import; the library file
    /// never contains a secret.
    #[test]
    fn proxmox_export_roundtrip() {
        use crate::model::{Library, PbsServer, PveServer};
        let pve = PveServer {
            host: "pve.lan".into(),
            user: "root@pam!aurora".into(),
            notify: false,
            ..Default::default()
        };
        let pbs = PbsServer {
            host: "pbs.lan".into(),
            user: "backup@pbs!ro".into(),
            ..Default::default()
        };
        let mut lib = Library::default();
        lib.pve_servers.push(pve.clone());
        lib.pbs_servers.push(pbs.clone());
        let mut kc = Keychain::default();
        kc.pve_secrets.insert(pve.id, "pve-token-secret".into());
        kc.pbs_secrets.insert(pbs.id, "pbs-token-secret".into());
        let lib_json = serde_json::to_string(&lib).unwrap();
        assert!(!lib_json.contains("token-secret"));

        let bundle = ExportBundle {
            version: 1,
            pve_servers: lib.pve_servers.clone(),
            pve_secrets: kc.pve_secrets.clone(),
            pbs_servers: lib.pbs_servers.clone(),
            pbs_secrets: kc.pbs_secrets.clone(),
            ..Default::default()
        };
        let sealed = seal("export-pw-1", &serde_json::to_vec(&bundle).unwrap()).unwrap();
        assert!(!sealed.contains("token-secret"));
        let mut back: ExportBundle =
            serde_json::from_slice(&unseal("export-pw-1", &sealed).unwrap()).unwrap();

        let mut lib2 = Library::default();
        let mut kc2 = Keychain::default();
        assert_eq!(back.merge_proxmox(&mut lib2, &mut kc2), 2);
        assert_eq!(lib2.pve_servers, vec![pve.clone()]);
        assert_eq!(lib2.pbs_servers, vec![pbs.clone()]);
        assert_eq!(kc2.pve_secrets[&pve.id], "pve-token-secret");
        assert_eq!(kc2.pbs_secrets[&pbs.id], "pbs-token-secret");
        // Importing again adds nothing and keeps the existing secret.
        let mut again: ExportBundle =
            serde_json::from_slice(&unseal("export-pw-1", &sealed).unwrap()).unwrap();
        kc2.pve_secrets.insert(pve.id, "changed".into());
        assert_eq!(again.merge_proxmox(&mut lib2, &mut kc2), 0);
        assert_eq!(kc2.pve_secrets[&pve.id], "changed");
        // Bundles from older versions load without Proxmox data.
        let old: ExportBundle = serde_json::from_str(r#"{"version":1,"hosts":[]}"#).unwrap();
        assert!(old.pve_servers.is_empty() && old.pbs_secrets.is_empty());
    }

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

        // Blobs: encrypted with the vault key, unreadable after a re-key.
        vault.save_blob("blob.enc", b"history s3cret").unwrap();
        let raw = fs::read_to_string(dir.join("blob.enc")).unwrap();
        assert!(!raw.contains("s3cret"));
        let (v2, _) = store.unlock_vault("hunter2").unwrap();
        assert_eq!(
            v2.load_blob("blob.enc").unwrap().as_deref(),
            Some(&b"history s3cret"[..])
        );
        assert_eq!(v2.key_id(), vault.key_id());
        assert!(v2.load_blob("missing.enc").unwrap().is_none());
        let mut v3 = v2;
        v3.change_password("other", &kc).unwrap();
        assert!(v3.load_blob("blob.enc").is_err());
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
