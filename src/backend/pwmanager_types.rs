//! Password-manager integration: data types shared by the model (`library.json`), the
//! CLI adapters and the UI. Nothing here is a secret except `Secret` / `Resolved`, which
//! live in memory only, wipe themselves on drop and never print their content.

use serde::{Deserialize, Serialize};

/// The external password managers Aurora can read credentials from (through their CLIs).
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PwmProvider {
    #[default]
    Bitwarden,
    OnePassword,
    KeePassXc,
    ProtonPass,
}

impl PwmProvider {
    pub const ALL: [PwmProvider; 4] = [
        PwmProvider::Bitwarden,
        PwmProvider::OnePassword,
        PwmProvider::KeePassXc,
        PwmProvider::ProtonPass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PwmProvider::Bitwarden => "Bitwarden",
            PwmProvider::OnePassword => "1Password",
            PwmProvider::KeePassXc => "KeePassXC",
            PwmProvider::ProtonPass => "Proton Pass",
        }
    }

    /// Default program name looked up in `PATH`.
    pub fn program(self) -> &'static str {
        match self {
            PwmProvider::Bitwarden => "bw",
            PwmProvider::OnePassword => "op",
            PwmProvider::KeePassXc => "keepassxc-cli",
            PwmProvider::ProtonPass => "pass-cli",
        }
    }

    /// Needs a master password typed into Aurora's own dialog.
    pub fn asks_password(self) -> bool {
        matches!(self, PwmProvider::Bitwarden | PwmProvider::KeePassXc)
    }
}

/// Which secret of the manager item is used.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// The login password of the item.
    #[default]
    Password,
    /// A private key kept in the item (notes / SSH key field); the item's password, if
    /// any, is used as the passphrase.
    PrivateKey,
}

/// Where a host (or identity) takes its secret from. Only a reference: the secret itself
/// never reaches `library.json`.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct CredentialSource {
    pub provider: PwmProvider,
    /// Opaque item reference: Bitwarden item id, 1Password item id or `op://…`
    /// reference, KeePass entry path, Proton `pass://share/item`.
    pub item: String,
    /// Item name for display.
    pub label: String,
    pub kind: SourceKind,
    /// Use the item's user name when the host has none.
    pub use_username: bool,
}

impl Default for CredentialSource {
    fn default() -> Self {
        Self {
            provider: PwmProvider::Bitwarden,
            item: String::new(),
            label: String::new(),
            kind: SourceKind::Password,
            use_username: true,
        }
    }
}

/// Settings of the password-manager integration (Settings → Sicherheit).
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct PwmSettings {
    pub bw_enabled: bool,
    /// Path of `bw`; empty = from `PATH`.
    pub bw_path: String,
    pub op_enabled: bool,
    pub op_path: String,
    /// 1Password account (`--account`); empty = default.
    pub op_account: String,
    pub kp_enabled: bool,
    pub kp_path: String,
    pub kp_db: String,
    pub kp_keyfile: String,
    pub proton_enabled: bool,
    pub proton_path: String,
    /// Forget unlock sessions after this many idle minutes (0 = until Aurora locks).
    pub timeout_minutes: u32,
}

impl Default for PwmSettings {
    fn default() -> Self {
        Self {
            bw_enabled: false,
            bw_path: String::new(),
            op_enabled: false,
            op_path: String::new(),
            op_account: String::new(),
            kp_enabled: false,
            kp_path: String::new(),
            kp_db: String::new(),
            kp_keyfile: String::new(),
            proton_enabled: false,
            proton_path: String::new(),
            timeout_minutes: 15,
        }
    }
}

impl PwmSettings {
    pub fn enabled(&self, p: PwmProvider) -> bool {
        match p {
            PwmProvider::Bitwarden => self.bw_enabled,
            PwmProvider::OnePassword => self.op_enabled,
            PwmProvider::KeePassXc => self.kp_enabled,
            PwmProvider::ProtonPass => self.proton_enabled,
        }
    }

    pub fn enabled_providers(&self) -> Vec<PwmProvider> {
        PwmProvider::ALL
            .into_iter()
            .filter(|p| self.enabled(*p))
            .collect()
    }

    pub fn path(&self, p: PwmProvider) -> &str {
        match p {
            PwmProvider::Bitwarden => &self.bw_path,
            PwmProvider::OnePassword => &self.op_path,
            PwmProvider::KeePassXc => &self.kp_path,
            PwmProvider::ProtonPass => &self.proton_path,
        }
    }
}

/// A secret kept in memory only: wiped when dropped, never printed.
#[derive(Clone, Default)]
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into().into_bytes())
    }

    pub fn from_bytes(b: Vec<u8>) -> Self {
        Secret(b)
    }

    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Overwrites a buffer in a way the optimizer must not remove.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret(***, {} bytes)", self.0.len())
    }
}

/// What a manager item yielded for one connection.
#[derive(Clone, Default)]
pub struct Resolved {
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub private_key: Option<Secret>,
    pub passphrase: Option<Secret>,
}

impl Resolved {
    /// Fills the fields of `other` that are still empty here.
    pub fn merge(&mut self, other: Resolved) {
        if self.username.is_none() {
            self.username = other.username.clone();
        }
        if self.password.is_none() {
            self.password = other.password.clone();
        }
        if self.private_key.is_none() {
            self.private_key = other.private_key.clone();
        }
        if self.passphrase.is_none() {
            self.passphrase = other.passphrase.clone();
        }
    }
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("username", &self.username.as_ref().map(|_| "…"))
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("private_key", &self.private_key.as_ref().map(|_| "***"))
            .field("passphrase", &self.passphrase.as_ref().map(|_| "***"))
            .finish()
    }
}

/// One search hit in the item picker (no secret).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PwmItem {
    /// Opaque reference (see `CredentialSource::item`).
    pub id: String,
    pub name: String,
    pub username: String,
    pub folder: String,
}

/// Why a manager request failed. The texts are fixed messages – raw CLI output (which may
/// contain secrets) is never shown.
#[derive(Clone, Debug, PartialEq)]
pub enum PwmError {
    NotInstalled(PwmProvider),
    Disabled(PwmProvider),
    /// Needs unlocking (master password / app unlock).
    Locked(PwmProvider),
    /// Not logged in at all.
    LoggedOut(PwmProvider),
    WrongPassword,
    NotFound,
    BadReference,
    BadConfig,
    Timeout,
    TooLarge,
    /// The CLI answered something unexpected.
    BadOutput,
    /// The item has no password / private key.
    NoSecret,
    Failed(Option<i32>),
}

impl PwmError {
    pub fn message(&self) -> String {
        match self {
            PwmError::NotInstalled(p) => {
                t!("{}-CLI ({}) nicht gefunden", p.label(), p.program()).to_string()
            }
            PwmError::Disabled(p) => t!(
                "{} ist nicht aktiviert (Einstellungen → Sicherheit → Passwort-Manager)",
                p.label()
            )
            .to_string(),
            PwmError::Locked(p) => t!("{} ist gesperrt", p.label()).to_string(),
            PwmError::LoggedOut(p) => match p {
                PwmProvider::Bitwarden => {
                    t!("Bitwarden ist nicht angemeldet – im Terminal „bw login“ ausführen")
                        .to_string()
                }
                PwmProvider::ProtonPass => {
                    t!("Proton Pass ist nicht angemeldet – im Terminal „pass-cli login“ ausführen")
                        .to_string()
                }
                _ => t!(
                    "{} ist nicht angemeldet – App entsperren bzw. im Terminal anmelden",
                    p.label()
                )
                .to_string(),
            },
            PwmError::WrongPassword => t!("Falsches Passwort").to_string(),
            PwmError::NotFound => t!("Eintrag nicht gefunden").to_string(),
            PwmError::BadReference => t!("Ungültige Eintrags-Referenz").to_string(),
            PwmError::BadConfig => t!("Ungültige Einstellung (Pfad/Konto)").to_string(),
            PwmError::Timeout => t!("Zeitüberschreitung (20 s)").to_string(),
            PwmError::TooLarge => t!("Antwort zu groß").to_string(),
            PwmError::BadOutput => t!("Unerwartete Antwort des Programms").to_string(),
            PwmError::NoSecret => {
                t!("Der Eintrag enthält kein Passwort bzw. keinen Schlüssel").to_string()
            }
            PwmError::Failed(Some(c)) => t!("Programm fehlgeschlagen (Code {})", c).to_string(),
            PwmError::Failed(None) => t!("Programm fehlgeschlagen").to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_secrets() {
        let s = Secret::new("hunter2-geheim");
        assert!(!format!("{s:?}").contains("hunter2"));
        let r = Resolved {
            username: Some("alice".into()),
            password: Some(Secret::new("hunter2-geheim")),
            private_key: Some(Secret::new("-----BEGIN KEY-----")),
            passphrase: Some(Secret::new("pass-phrase")),
        };
        let d = format!("{r:?}");
        for needle in ["hunter2", "BEGIN", "pass-phrase", "alice"] {
            assert!(!d.contains(needle), "{needle} leaked: {d}");
        }
    }

    #[test]
    fn wipe_zeroes() {
        let mut v = b"secret".to_vec();
        wipe(&mut v);
        assert_eq!(v, vec![0u8; 6]);
    }

    #[test]
    fn source_roundtrip_and_defaults() {
        let src: CredentialSource =
            serde_json::from_str(r#"{"provider":"keepassxc","item":"/A/B"}"#).unwrap();
        assert_eq!(src.provider, PwmProvider::KeePassXc);
        assert!(src.use_username);
        assert_eq!(src.kind, SourceKind::Password);
        let st: PwmSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(st.timeout_minutes, 15);
        assert!(st.enabled_providers().is_empty());
    }
}
