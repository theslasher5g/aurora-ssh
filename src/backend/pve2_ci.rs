//! Cloud-init settings of a VM beyond the first address: `ipconfig0..31`, DNS,
//! user, password, SSH keys, package upgrade and custom snippets. Parsing of the VM
//! config, validation with readable messages and the minimal `POST …/config` form
//! (only what changed, `delete` for what was cleared). The password is never part of
//! a log line, a `Debug` output or the stored model: see [`Secret`].

use std::fmt;
use std::net::IpAddr;

use serde_json::Value;

use uuid::Uuid;

use super::super::{
    Client, Conn, Epoch, Error, GuestId, Kind, Session, Sink, enc, has_cloudinit, valid_cidr,
    valid_ciuser, valid_node, valid_pubkey, valid_storage, valid_vmid,
};

/// Highest `ipconfigN` Proxmox knows.
pub const MAX_NICS: usize = 32;

/// A secret that never prints and wipes itself on drop (best effort).
#[derive(Clone, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl PartialEq for Secret {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Best effort: overwrite the bytes before the buffer is freed.
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|b| *b = 0);
        std::hint::black_box(&bytes);
    }
}

// ------------------------------------------------------------------ model

#[derive(Clone, Debug, Default, PartialEq)]
pub enum V4 {
    #[default]
    None,
    Dhcp,
    Manual,
    Static {
        cidr: String,
        gw: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum V6 {
    #[default]
    None,
    Auto,
    Dhcp,
    Manual,
    Static {
        cidr: String,
        gw: String,
    },
}

/// `ipconfigN`: IPv4 and IPv6 part of one interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nic {
    pub v4: V4,
    pub v6: V6,
}

impl Nic {
    pub fn is_empty(&self) -> bool {
        self.v4 == V4::None && self.v6 == V6::None
    }

    /// The `ipconfigN` value; `None` for an empty interface. No validation.
    pub fn value(&self) -> Option<String> {
        let mut parts = Vec::new();
        match &self.v4 {
            V4::None => {}
            V4::Dhcp => parts.push("ip=dhcp".to_string()),
            V4::Manual => parts.push("ip=manual".to_string()),
            V4::Static { cidr, gw } => {
                parts.push(format!("ip={}", cidr.trim()));
                if !gw.trim().is_empty() {
                    parts.push(format!("gw={}", gw.trim()));
                }
            }
        }
        match &self.v6 {
            V6::None => {}
            V6::Auto => parts.push("ip6=auto".to_string()),
            V6::Dhcp => parts.push("ip6=dhcp".to_string()),
            V6::Manual => parts.push("ip6=manual".to_string()),
            V6::Static { cidr, gw } => {
                parts.push(format!("ip6={}", cidr.trim()));
                if !gw.trim().is_empty() {
                    parts.push(format!("gw6={}", gw.trim()));
                }
            }
        }
        (!parts.is_empty()).then(|| parts.join(","))
    }
}

/// Parses `ip=10.0.0.5/24,gw=10.0.0.1,ip6=auto`.
pub fn parse_ipconfig(s: &str) -> Nic {
    let (mut ip4, mut gw4, mut ip6, mut gw6) = (None, String::new(), None, String::new());
    for part in s.split(',') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        match k {
            "ip" => ip4 = Some(v.to_string()),
            "gw" => gw4 = v.to_string(),
            "ip6" => ip6 = Some(v.to_string()),
            "gw6" => gw6 = v.to_string(),
            _ => {}
        }
    }
    Nic {
        v4: match ip4.as_deref() {
            None | Some("") => V4::None,
            Some("dhcp") => V4::Dhcp,
            Some("manual") => V4::Manual,
            Some(c) => V4::Static {
                cidr: c.to_string(),
                gw: gw4,
            },
        },
        v6: match ip6.as_deref() {
            None | Some("") => V6::None,
            Some("auto") => V6::Auto,
            Some("dhcp") => V6::Dhcp,
            Some("manual") => V6::Manual,
            Some(c) => V6::Static {
                cidr: c.to_string(),
                gw: gw6,
            },
        },
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CiConfig {
    /// Index = N of `ipconfigN`; `None` = not set.
    pub nics: Vec<Option<Nic>>,
    /// Space separated addresses.
    pub nameserver: String,
    /// Space separated domains.
    pub searchdomain: String,
    pub ciuser: String,
    pub sshkeys: Vec<String>,
    /// `None` = Proxmox default.
    pub ciupgrade: Option<bool>,
    pub cicustom: String,
    /// A password is stored (Proxmox shows `**********`, never the value).
    pub has_password: bool,
}

fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().trim().to_string()
}

/// The cloud-init part of `GET …/qemu/{id}/config`.
pub fn parse_config(cfg: &Value) -> CiConfig {
    let mut nics: Vec<Option<Nic>> = Vec::new();
    for n in 0..MAX_NICS {
        if let Some(s) = cfg[format!("ipconfig{n}")].as_str() {
            nics.resize(n + 1, None);
            nics[n] = Some(parse_ipconfig(s));
        }
    }
    let sshkeys = cfg["sshkeys"]
        .as_str()
        .map(|s| {
            percent_decode(s)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    CiConfig {
        nics,
        nameserver: text(cfg, "nameserver"),
        searchdomain: text(cfg, "searchdomain"),
        ciuser: text(cfg, "ciuser"),
        sshkeys,
        ciupgrade: cfg["ciupgrade"]
            .as_bool()
            .or_else(|| cfg["ciupgrade"].as_u64().map(|n| n != 0))
            .or_else(|| cfg["ciupgrade"].as_str().map(|s| s == "1")),
        cicustom: text(cfg, "cicustom"),
        has_password: cfg["cipassword"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty()),
    }
}

// ------------------------------------------------------------------ validation

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// Which field: `ipconfig2`, `nameserver`, …
    pub field: String,
    pub text: String,
    /// A warning does not block saving.
    pub warning: bool,
}

fn err(field: impl Into<String>, text: impl Into<String>) -> Issue {
    Issue {
        field: field.into(),
        text: text.into(),
        warning: false,
    }
}

fn warn(field: impl Into<String>, text: impl Into<String>) -> Issue {
    Issue {
        field: field.into(),
        text: text.into(),
        warning: true,
    }
}

/// `10.0.0.5/24` → the address and prefix length.
fn split_cidr(c: &str) -> Option<(IpAddr, u8)> {
    let (ip, p) = c.trim().split_once('/')?;
    Some((ip.parse().ok()?, p.parse().ok()?))
}

/// Is `addr` in the network `net/prefix`?
pub fn same_subnet(net: IpAddr, prefix: u8, addr: IpAddr) -> bool {
    match (net, addr) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix.min(32))
            };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix.min(128))
            };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

fn check_static(field: &str, cidr: &str, gw: &str, v6: bool, out: &mut Vec<Issue>) {
    let cidr = cidr.trim();
    let gw = gw.trim();
    if !valid_cidr(cidr) {
        out.push(err(
            field,
            t!("Adresse muss die Form 10.0.0.5/24 haben (mit Präfix)"),
        ));
        return;
    }
    let Some((ip, prefix)) = split_cidr(cidr) else {
        return;
    };
    if ip.is_ipv6() != v6 {
        out.push(err(
            field,
            if v6 {
                t!("keine IPv6-Adresse")
            } else {
                t!("keine IPv4-Adresse")
            },
        ));
        return;
    }
    if ip.is_multicast() || ip.is_unspecified() {
        out.push(err(field, t!("ungültige Adresse")));
    }
    if gw.is_empty() {
        return;
    }
    let Ok(g) = gw.parse::<IpAddr>() else {
        out.push(err(field, t!("Gateway ist keine IP-Adresse")));
        return;
    };
    if g.is_ipv6() != v6 {
        out.push(err(field, t!("Gateway hat die falsche Adressfamilie")));
    } else if g == ip {
        out.push(err(field, t!("Gateway darf nicht die eigene Adresse sein")));
    } else if !same_subnet(ip, prefix, g) && !g.is_unspecified() {
        out.push(warn(
            field,
            t!("Gateway liegt nicht im Subnetz der Adresse"),
        ));
    }
}

/// DNS names (labels separated by dots).
fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// `user=local:snippets/ci.yaml,network=…`: only the four known keys and snippet
/// volumes with plain file names.
pub fn valid_cicustom(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return true;
    }
    let mut seen = Vec::new();
    for part in s.split(',') {
        let Some((k, v)) = part.split_once('=') else {
            return false;
        };
        if !matches!(k, "user" | "network" | "meta" | "vendor") || seen.contains(&k) {
            return false;
        }
        seen.push(k);
        let Some((storage, file)) = v.split_once(':') else {
            return false;
        };
        let Some(name) = file.strip_prefix("snippets/") else {
            return false;
        };
        if !valid_storage(storage)
            || name.is_empty()
            || name.starts_with('.')
            || name.contains("..")
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        {
            return false;
        }
    }
    true
}

pub fn valid_password(p: &str) -> bool {
    !p.is_empty() && p.len() <= 256 && !p.chars().any(char::is_control)
}

/// All problems of a configuration, errors first.
pub fn validate(c: &CiConfig, password: Option<&Secret>) -> Vec<Issue> {
    let mut out = Vec::new();
    for (n, nic) in c.nics.iter().enumerate() {
        let Some(nic) = nic else { continue };
        let field = format!("ipconfig{n}");
        if nic.is_empty() {
            continue;
        }
        if let V4::Static { cidr, gw } = &nic.v4 {
            check_static(&field, cidr, gw, false, &mut out);
        }
        if let V6::Static { cidr, gw } = &nic.v6 {
            check_static(&field, cidr, gw, true, &mut out);
        }
    }
    let ns = c.nameserver.split_whitespace().collect::<Vec<_>>();
    if ns.len() > 3 {
        out.push(warn(
            "nameserver",
            t!("Mehr als 3 Nameserver werden ignoriert"),
        ));
    }
    if ns.iter().any(|a| a.parse::<IpAddr>().is_err()) {
        out.push(err("nameserver", t!("Nameserver müssen IP-Adressen sein")));
    }
    if c.searchdomain.split_whitespace().any(|d| !valid_domain(d)) {
        out.push(err(
            "searchdomain",
            t!("Suchdomäne ist kein gültiger DNS-Name"),
        ));
    }
    let user = c.ciuser.trim();
    if !user.is_empty() && !valid_ciuser(user) {
        out.push(err(
            "ciuser",
            t!("Benutzername: Kleinbuchstaben, Ziffern, _ und - (max. 32)"),
        ));
    }
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        if !valid_password(p.expose()) {
            out.push(err("cipassword", t!("Passwort enthält ungültige Zeichen")));
        } else if p.expose().chars().count() < 8 {
            out.push(warn("cipassword", t!("Kurzes Passwort (unter 8 Zeichen)")));
        }
    }
    for (i, k) in c
        .sshkeys
        .iter()
        .filter(|k| !k.trim().is_empty())
        .enumerate()
    {
        if !valid_pubkey(k) {
            out.push(err(
                "sshkeys",
                t!("SSH-Key {} ist kein öffentlicher OpenSSH-Schlüssel", i + 1),
            ));
        }
    }
    if !valid_cicustom(&c.cicustom) {
        out.push(err(
            "cicustom",
            t!("Format: user=local:snippets/datei.yaml (auch network=, meta=, vendor=)"),
        ));
    }
    out.sort_by_key(|i| i.warning);
    out
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| !i.warning)
}

// ------------------------------------------------------------------ form

fn norm_list(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn keys_of(c: &CiConfig) -> Vec<String> {
    let mut v: Vec<String> = c
        .sshkeys
        .iter()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    v.dedup();
    v
}

/// What to send: only the settings that differ from `old`; cleared ones go into
/// `delete`. `Err` lists validation errors (the password is validated here but sent
/// by [`CiApply`], so it never sits in a plain form).
pub fn build_form(
    old: &CiConfig,
    new: &CiConfig,
    password: Option<&Secret>,
) -> Result<Vec<(String, String)>, Vec<Issue>> {
    let issues = validate(new, password);
    if has_errors(&issues) {
        return Err(issues.into_iter().filter(|i| !i.warning).collect());
    }
    let mut form: Vec<(String, String)> = Vec::new();
    let mut delete: Vec<String> = Vec::new();
    let mut set = |key: &str, new_v: Option<String>, old_v: Option<String>| {
        if new_v != old_v {
            match new_v {
                Some(v) => form.push((key.to_string(), v)),
                None => delete.push(key.to_string()),
            }
        }
    };
    for n in 0..MAX_NICS {
        let get = |c: &CiConfig| c.nics.get(n).and_then(|x| x.as_ref()).and_then(Nic::value);
        set(&format!("ipconfig{n}"), get(new), get(old));
    }
    let some = |s: &str| Some(norm_list(s)).filter(|s| !s.is_empty());
    set("nameserver", some(&new.nameserver), some(&old.nameserver));
    set(
        "searchdomain",
        some(&new.searchdomain),
        some(&old.searchdomain),
    );
    let one = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    set("ciuser", one(&new.ciuser), one(&old.ciuser));
    set("cicustom", one(&new.cicustom), one(&old.cicustom));
    let (nk, ok) = (keys_of(new), keys_of(old));
    set(
        "sshkeys",
        (!nk.is_empty()).then(|| enc(&nk.join("\n"))),
        (!ok.is_empty()).then(|| enc(&ok.join("\n"))),
    );
    if new.ciupgrade != old.ciupgrade {
        match new.ciupgrade {
            Some(b) => form.push(("ciupgrade".into(), if b { "1" } else { "0" }.into())),
            None => delete.push("ciupgrade".into()),
        }
    }
    if !delete.is_empty() {
        form.push(("delete".into(), delete.join(",")));
    }
    Ok(form)
}

/// What the task sends: the changed settings, an optional new password and whether the
/// cloud-init drive is regenerated afterwards. `Debug` shows keys only.
#[derive(Clone, Default, PartialEq)]
pub struct CiApply {
    pub form: Vec<(String, String)>,
    pub password: Option<Secret>,
    pub regenerate: bool,
}

impl fmt::Debug for CiApply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CiApply")
            .field(
                "keys",
                &self
                    .form
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("regenerate", &self.regenerate)
            .finish()
    }
}

impl CiApply {
    /// Validates and builds; `None` password = keep the current one.
    pub fn plan(
        old: &CiConfig,
        new: &CiConfig,
        password: Option<Secret>,
        regenerate: bool,
    ) -> Result<CiApply, Vec<Issue>> {
        let password = password.filter(|p| !p.is_empty());
        let form = build_form(old, new, password.as_ref())?;
        Ok(CiApply {
            form,
            password,
            regenerate,
        })
    }

    pub fn is_noop(&self) -> bool {
        self.form.is_empty() && self.password.is_none() && !self.regenerate
    }

    /// The `POST …/config` form including the password (built at the last moment).
    pub fn config_form(&self) -> Vec<(String, String)> {
        let mut f = self.form.clone();
        if let Some(p) = &self.password {
            f.push(("cipassword".to_string(), p.expose().to_string()));
        }
        f
    }
}

/// Suggested gateway for a static address: the first host of its /24 (IPv4) – only a
/// hint the user can overwrite.
pub fn suggest_gateway(cidr: &str) -> Option<String> {
    match split_cidr(cidr)? {
        (IpAddr::V4(a), p) if (8..=30).contains(&p) => {
            let mask = u32::MAX << (32 - p);
            let first = (u32::from(a) & mask) + 1;
            (first != u32::from(a)).then(|| std::net::Ipv4Addr::from(first).to_string())
        }
        _ => None,
    }
}

// ------------------------------------------------------------------ loading

/// The VM config as far as cloud-init goes.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub config: CiConfig,
    /// The VM has a cloud-init drive (otherwise the settings have no effect).
    pub drive: bool,
}

#[derive(Clone, Debug)]
pub enum CiEvent {
    Loaded {
        server: Uuid,
        epoch: Epoch,
        guest: GuestId,
        session: Option<Session>,
        result: Result<Loaded, Error>,
    },
}

/// `/nodes/{node}/qemu/{vmid}/config`, `None` for names unfit for a URL.
pub fn config_path(g: &GuestId) -> Option<String> {
    (g.kind == Kind::Qemu && valid_node(&g.node) && valid_vmid(g.vmid))
        .then(|| format!("/nodes/{}/qemu/{}/config", g.node, g.vmid))
}

pub fn spawn_load(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    sink: Sink<CiEvent>,
) {
    rt.spawn_blocking(move || {
        let mut c = Client::new(&conn);
        let result = match config_path(&guest) {
            Some(p) => c.call(false, &p).map(|cfg| Loaded {
                config: parse_config(&cfg),
                drive: has_cloudinit(&cfg),
            }),
            None => Err(Error::Net(t!("ungültige Eingabe").into())),
        };
        sink.send(CiEvent::Loaded {
            server,
            epoch,
            guest,
            session: c.session.clone(),
            result,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl user@host";

    #[test]
    fn ipconfig_roundtrip() {
        for s in [
            "ip=dhcp",
            "ip=10.0.0.5/24,gw=10.0.0.1",
            "ip=10.0.0.5/24,gw=10.0.0.1,ip6=auto",
            "ip=dhcp,ip6=fd00::5/64,gw6=fd00::1",
            "ip6=dhcp",
        ] {
            assert_eq!(parse_ipconfig(s).value().as_deref(), Some(s), "{s}");
        }
        assert!(parse_ipconfig("").is_empty());
        assert_eq!(parse_ipconfig("ip=manual").v4, V4::Manual);
    }

    #[test]
    fn parses_vm_config() {
        let cfg = json!({
            "ipconfig0": "ip=192.168.1.5/24,gw=192.168.1.1",
            "ipconfig2": "ip=dhcp",
            "nameserver": "1.1.1.1 9.9.9.9", "searchdomain": "lan.example",
            "ciuser": "debian", "cipassword": "**********",
            "sshkeys": enc(&format!("{KEY}\n{KEY}2")),
            "ciupgrade": 1, "cicustom": "user=local:snippets/a.yaml",
        });
        let c = parse_config(&cfg);
        assert_eq!(c.nics.len(), 3);
        assert!(c.nics[1].is_none() && c.nics[2].as_ref().unwrap().v4 == V4::Dhcp);
        assert_eq!(c.sshkeys.len(), 2);
        assert!(c.sshkeys[0].starts_with("ssh-ed25519 AAAA"));
        assert_eq!(c.ciupgrade, Some(true));
        assert!(c.has_password);
        assert_eq!(c.ciuser, "debian");
        let empty = parse_config(&json!({"memory": 512}));
        assert_eq!(empty, CiConfig::default());
    }

    #[test]
    fn validation_messages() {
        let c = CiConfig {
            nics: vec![
                Some(Nic {
                    v4: V4::Static {
                        cidr: "10.0.0.5".into(),
                        gw: "".into(),
                    },
                    v6: V6::None,
                }),
                Some(Nic {
                    v4: V4::Static {
                        cidr: "10.0.0.5/24".into(),
                        gw: "10.0.1.1".into(),
                    },
                    v6: V6::Static {
                        cidr: "fd00::5/64".into(),
                        gw: "10.0.0.1".into(),
                    },
                }),
                Some(Nic {
                    v4: V4::Static {
                        cidr: "10.0.0.5/24".into(),
                        gw: "10.0.0.5".into(),
                    },
                    v6: V6::None,
                }),
            ],
            nameserver: "1.1.1.1 nope".into(),
            searchdomain: "ok.example bad_".into(),
            ciuser: "Root".into(),
            sshkeys: vec!["ssh-rsa short".into()],
            cicustom: "user=local:../x".into(),
            ..Default::default()
        };
        let issues = validate(&c, Some(&Secret::new("ab\ncd")));
        let fields: Vec<&str> = issues.iter().map(|i| i.field.as_str()).collect();
        for f in [
            "ipconfig0",
            "ipconfig1",
            "ipconfig2",
            "nameserver",
            "ciuser",
            "sshkeys",
            "cicustom",
            "cipassword",
        ] {
            assert!(fields.contains(&f), "{f} in {fields:?}");
        }
        // 10.0.1.1 outside 10.0.0.0/24 is only a warning, the v6 gateway family is an error.
        let g1: Vec<_> = issues.iter().filter(|i| i.field == "ipconfig1").collect();
        assert!(g1.iter().any(|i| i.warning) && g1.iter().any(|i| !i.warning));
        assert!(has_errors(&issues));
        assert!(build_form(&CiConfig::default(), &c, None).is_err());
    }

    #[test]
    fn minimal_form_and_delete() {
        let old = parse_config(&json!({
            "ipconfig0": "ip=10.0.0.5/24,gw=10.0.0.1", "nameserver": "1.1.1.1",
            "ciuser": "debian", "ciupgrade": 1,
        }));
        let same = build_form(&old, &old.clone(), None).unwrap();
        assert!(same.is_empty());

        let mut new = old.clone();
        new.nics.resize(2, None);
        new.nics[1] = Some(Nic {
            v4: V4::Dhcp,
            v6: V6::Auto,
        });
        new.nameserver = String::new();
        new.searchdomain = " lan.example  ".into();
        new.sshkeys = vec![KEY.into(), format!("  {KEY}  ")];
        new.ciupgrade = None;
        new.cicustom = "user=local:snippets/a.yaml,vendor=nas:snippets/v.yml".into();
        let pw = Secret::new("hunter22hunter");
        let form = build_form(&old, &new, Some(&pw)).unwrap();
        let get = |k: &str| form.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("ipconfig1"), Some("ip=dhcp,ip6=auto"));
        assert_eq!(get("ipconfig0"), None);
        assert_eq!(get("searchdomain"), Some("lan.example"));
        assert_eq!(
            get("cipassword"),
            None,
            "the password is not part of the plain form"
        );
        assert!(get("sshkeys").unwrap().contains("%20user%40host"));
        assert!(
            !get("sshkeys").unwrap().contains("%0A"),
            "duplicate key removed"
        );
        assert_eq!(get("delete"), Some("nameserver,ciupgrade"));
        assert!(get("cicustom").unwrap().contains("vendor="));
    }

    #[test]
    fn apply_adds_password_late_and_hides_it() {
        let old = CiConfig::default();
        let mut new = old.clone();
        new.ciuser = "debian".into();
        let a = CiApply::plan(&old, &new, Some(Secret::new("hunter22hunter")), true).unwrap();
        assert!(!a.is_noop());
        assert!(!format!("{a:?}").contains("hunter22"));
        assert!(!format!("{:?}", a.form).contains("hunter22"));
        let f = a.config_form();
        assert!(f.contains(&("cipassword".to_string(), "hunter22hunter".to_string())));
        assert!(f.contains(&("ciuser".to_string(), "debian".to_string())));
        assert!(a.regenerate);
        let none = CiApply::plan(&old, &old, Some(Secret::new("")), false).unwrap();
        assert!(none.is_noop());
        assert!(CiApply::plan(&old, &old, Some(Secret::new("bad\npw")), false).is_err());
    }

    #[test]
    fn secret_never_prints() {
        let s = Secret::new("topsecret");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert!(!format!("{:?}", Some(&s)).contains("topsecret"));
    }

    #[test]
    fn cicustom_and_gateway_hint() {
        assert!(valid_cicustom(""));
        assert!(valid_cicustom(
            "user=local:snippets/ci-user.yaml,meta=local:snippets/m.yml"
        ));
        for bad in [
            "user=local:snippets/../etc",
            "user=local:iso/x.yaml",
            "foo=local:snippets/x",
            "user=local:snippets/x,user=local:snippets/y",
            "user=lo cal:snippets/x",
            "user",
        ] {
            assert!(!valid_cicustom(bad), "{bad}");
        }
        assert_eq!(
            suggest_gateway("192.168.7.40/24").as_deref(),
            Some("192.168.7.1")
        );
        assert_eq!(suggest_gateway("192.168.7.1/24"), None);
        assert_eq!(suggest_gateway("fd00::5/64"), None);
        assert!(same_subnet(
            "10.0.0.5".parse().unwrap(),
            24,
            "10.0.0.200".parse().unwrap()
        ));
        assert!(!same_subnet(
            "10.0.0.5".parse().unwrap(),
            24,
            "10.0.1.1".parse().unwrap()
        ));
    }
}
