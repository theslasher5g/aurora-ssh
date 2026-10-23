//! Per-host WinRM settings (stored in `library.json`; the credentials come from the host's
//! keychain identity, never from here).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WinrmAuth {
    /// NTLMv2 (works with local accounts and domain accounts).
    #[default]
    Ntlm,
    /// User name and password in the header – only offered over HTTPS.
    Basic,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct WinrmSettings {
    /// Collect facts and run commands on this Windows host through WinRM.
    pub enabled: bool,
    /// HTTPS (5986) instead of HTTP (5985).
    pub https: bool,
    /// 0 = the default of the transport.
    pub port: u16,
    pub auth: WinrmAuth,
}

impl Default for WinrmSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            https: true,
            port: 0,
            auth: WinrmAuth::Ntlm,
        }
    }
}

impl WinrmSettings {
    pub fn effective_port(&self) -> u16 {
        match (self.port, self.https) {
            (0, true) => 5986,
            (0, false) => 5985,
            (p, _) => p,
        }
    }
}

/// Key of the pinned certificate in the known-hosts table.
pub fn pin_key(host: &str, port: u16) -> String {
    format!("winrm://{host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_and_old_files() {
        let mut s = WinrmSettings::default();
        assert_eq!(s.effective_port(), 5986);
        s.https = false;
        assert_eq!(s.effective_port(), 5985);
        s.port = 8443;
        assert_eq!(s.effective_port(), 8443);
        // A library written before WinRM existed has no such field.
        let s: WinrmSettings = serde_json::from_str("{}").unwrap();
        assert!(!s.enabled && s.https);
        assert_eq!(pin_key("srv", 5986), "winrm://srv:5986");
    }
}
