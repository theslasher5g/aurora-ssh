//! What a guest console should open: the SSH address and cloud-init user of a VM, its
//! serial ports and whether the API may open a terminal on it. The UI turns this into
//! an SSH session, an API serial console (termproxy), `qm terminal` on the node or the
//! noVNC console in the browser – see [`console_route`].

use std::net::IpAddr;

use serde_json::Value;
use uuid::Uuid;

use super::{
    Client, Conn, Epoch, Error, GuestId, GuestPayload, Kind, PveEvent, Sink, agent_candidates,
    config_candidates, host_ip, rank_ips, valid_node,
};

/// Everything the console decision needs about one VM.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConsoleInfo {
    /// Best SSH address (guest agent first, then the cloud-init config).
    pub ip: Option<IpAddr>,
    /// `ciuser` of the cloud-init config.
    pub ciuser: Option<String>,
    /// First serial port (`serial0` … `serial3`).
    pub serial: Option<String>,
    /// `VM.Console` on the guest: `None` = not checked / unknown.
    pub may_console: Option<bool>,
}

/// How a console is opened.
#[derive(Clone, Debug, PartialEq)]
pub enum ConsoleRoute {
    /// SSH to the guest; `user` prefills the login sheet.
    Login { ip: String, user: String },
    /// Terminal over the API (termproxy): the container, or a VM's serial port.
    ApiTerm { serial: Option<String> },
    /// `pct enter` / `qm terminal` over the node's SSH host.
    NodeTerm { serial: Option<String> },
    /// No address, no serial port: Proxmox' noVNC console in the browser.
    Browser,
}

/// Picks the console for a guest. `info` is only used for VMs; `node_ssh` says whether
/// the server has an SSH host for its nodes.
pub fn console_route(kind: Kind, info: &ConsoleInfo, node_ssh: bool) -> ConsoleRoute {
    if kind == Kind::Lxc {
        return if node_ssh {
            ConsoleRoute::NodeTerm { serial: None }
        } else {
            ConsoleRoute::ApiTerm { serial: None }
        };
    }
    if let Some(ip) = info.ip {
        return ConsoleRoute::Login {
            ip: ip.to_string(),
            user: info.ciuser.clone().unwrap_or_else(|| "root".into()),
        };
    }
    match &info.serial {
        // The API would refuse the terminal: `qm terminal` over SSH still works.
        Some(s) if info.may_console == Some(false) && node_ssh => ConsoleRoute::NodeTerm {
            serial: Some(s.clone()),
        },
        Some(s) => ConsoleRoute::ApiTerm {
            serial: Some(s.clone()),
        },
        None => ConsoleRoute::Browser,
    }
}

/// `serial0` … `serial3`.
pub fn valid_serial(s: &str) -> bool {
    s.strip_prefix("serial")
        .is_some_and(|n| n.len() == 1 && ('0'..='3').contains(&n.chars().next().unwrap_or('x')))
}

/// First serial port of a VM config (`serial0: socket`).
pub fn parse_serial(config: &Value) -> Option<String> {
    let obj = config.as_object()?;
    (0..4).map(|n| format!("serial{n}")).find(|k| {
        obj.get(k)
            .and_then(Value::as_str)
            .is_some_and(|v| !v.trim().is_empty())
    })
}

/// `ciuser` of a VM config, if it is a plausible user name.
pub fn parse_ciuser(config: &Value) -> Option<String> {
    let u = config.get("ciuser")?.as_str()?.trim();
    super::valid_ciuser(u).then(|| u.to_string())
}

/// `GET /access/permissions?path=/vms/{vmid}` → whether `VM.Console` is granted.
pub fn parse_may_console(data: &Value, vmid: u32) -> Option<bool> {
    let obj = data.as_object()?;
    let path = format!("/vms/{vmid}");
    Some(
        obj.get(&path)
            .and_then(Value::as_object)
            .is_some_and(|p| p.get("VM.Console").is_some_and(|v| !v.is_null())),
    )
}

/// A plain SSH user name for the login sheet (no spaces, `@` or shell characters).
pub fn valid_login_user(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 64
        && !u.starts_with('-')
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$'))
}

/// Line run over the node's SSH host: `pct enter` / `qm terminal`, hopping to the
/// guest's node when the SSH host is another cluster member. `None` for a bad node name
/// or serial port.
pub fn node_console_line(
    kind: Kind,
    vmid: u32,
    node: &str,
    serial: Option<&str>,
) -> Option<String> {
    if !valid_node(node) || serial.is_some_and(|s| !valid_serial(s)) {
        return None;
    }
    let cmd = match (kind, serial) {
        (Kind::Lxc, _) => format!("pct enter {vmid}"),
        (Kind::Qemu, Some(s)) => format!("qm terminal {vmid} -iface {s}"),
        (Kind::Qemu, None) => format!("qm terminal {vmid}"),
    };
    Some(format!(
        "if [ \"$(hostname)\" = '{node}' ]; then {cmd}; else ssh -t root@{node} {cmd}; fi"
    ))
}

/// Looks up address, cloud-init user, serial port and console permission of a VM.
pub fn spawn_console_info(
    rt: &tokio::runtime::Runtime,
    server: Uuid,
    epoch: Epoch,
    conn: Conn,
    guest: GuestId,
    sink: Sink<PveEvent>,
) {
    rt.spawn_blocking(move || {
        let near = host_ip(&conn.host, conn.port);
        let mut c = Client::new(&conn);
        let result = (|| {
            if !valid_node(&guest.node) {
                return Err(Error::Net(t!("ungültiger Node-Name").into()));
            }
            let base = format!("/nodes/{}/{}/{}", guest.node, guest.kind.api(), guest.vmid);
            let fatal = |e: &Error| {
                matches!(
                    e,
                    Error::Untrusted(_) | Error::PinChanged { .. } | Error::Auth(_)
                )
            };
            // No permission for the config: still try the agent.
            let config = match c.call(false, &format!("{base}/config")) {
                Ok(v) => v,
                Err(e) if fatal(&e) => return Err(e),
                Err(e) => {
                    log::info!(
                        "Proxmox: Konfiguration von {} nicht lesbar: {e}",
                        guest.vmid
                    );
                    Value::Null
                }
            };
            let mut info = ConsoleInfo {
                ciuser: parse_ciuser(&config),
                serial: parse_serial(&config),
                ..Default::default()
            };
            // Stopped guest or no agent: Proxmox answers with an HTTP error.
            let live = match c.call(false, &format!("{base}/agent/network-get-interfaces")) {
                Ok(d) => agent_candidates(&d),
                Err(e) if fatal(&e) => return Err(e),
                Err(_) => Vec::new(),
            };
            info.ip = rank_ips(&live, near).first().copied().or_else(|| {
                rank_ips(&config_candidates(&config, guest.kind), near)
                    .first()
                    .copied()
            });
            if info.ip.is_none() && info.serial.is_some() {
                let path = format!("/access/permissions?path=/vms/{}", guest.vmid);
                info.may_console = c
                    .call(false, &path)
                    .ok()
                    .and_then(|d| parse_may_console(&d, guest.vmid));
            }
            Ok(info)
        })();
        sink.send(PveEvent::Guest {
            server,
            epoch,
            vmid: guest.vmid,
            session: c.session.clone(),
            payload: GuestPayload::Console(result),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ip(s: &str) -> Option<IpAddr> {
        s.parse().ok()
    }

    #[test]
    fn routes() {
        let none = ConsoleInfo::default();
        // Containers: node SSH host if there is one, the API otherwise.
        assert_eq!(
            console_route(Kind::Lxc, &none, true),
            ConsoleRoute::NodeTerm { serial: None }
        );
        assert_eq!(
            console_route(Kind::Lxc, &none, false),
            ConsoleRoute::ApiTerm { serial: None }
        );
        // VM with address: SSH, as the cloud-init user or root.
        let with_ip = ConsoleInfo {
            ip: ip("10.0.0.7"),
            ciuser: Some("debian".into()),
            serial: Some("serial0".into()),
            may_console: None,
        };
        assert_eq!(
            console_route(Kind::Qemu, &with_ip, false),
            ConsoleRoute::Login {
                ip: "10.0.0.7".into(),
                user: "debian".into()
            }
        );
        let root = ConsoleInfo {
            ciuser: None,
            ..with_ip.clone()
        };
        assert_eq!(
            console_route(Kind::Qemu, &root, true),
            ConsoleRoute::Login {
                ip: "10.0.0.7".into(),
                user: "root".into()
            }
        );
        // No address, serial port: API terminal; qm terminal only without VM.Console.
        let serial = ConsoleInfo {
            ip: None,
            ..with_ip.clone()
        };
        let api = ConsoleRoute::ApiTerm {
            serial: Some("serial0".into()),
        };
        assert_eq!(console_route(Kind::Qemu, &serial, true), api);
        assert_eq!(console_route(Kind::Qemu, &serial, false), api);
        let denied = ConsoleInfo {
            may_console: Some(false),
            ..serial.clone()
        };
        assert_eq!(
            console_route(Kind::Qemu, &denied, true),
            ConsoleRoute::NodeTerm {
                serial: Some("serial0".into())
            }
        );
        assert_eq!(console_route(Kind::Qemu, &denied, false), api);
        // Neither: browser.
        assert_eq!(
            console_route(Kind::Qemu, &none, true),
            ConsoleRoute::Browser
        );
        assert_eq!(
            console_route(Kind::Qemu, &none, false),
            ConsoleRoute::Browser
        );
    }

    #[test]
    fn config_parsing() {
        let cfg = json!({
            "ciuser": "ubuntu",
            "serial1": "socket",
            "serial3": "/dev/ttyS0",
            "vga": "serial1",
            "ipconfig0": "ip=dhcp"
        });
        assert_eq!(parse_ciuser(&cfg).as_deref(), Some("ubuntu"));
        assert_eq!(parse_serial(&cfg).as_deref(), Some("serial1"));
        assert_eq!(parse_serial(&json!({"serial0": ""})), None);
        assert_eq!(parse_serial(&json!({"serial4": "socket"})), None);
        assert_eq!(parse_serial(&json!({})), None);
        assert_eq!(parse_serial(&Value::Null), None);
        assert_eq!(parse_ciuser(&json!({"ciuser": "a b"})), None);
        assert_eq!(
            parse_ciuser(&json!({"ciuser": " admin "})).as_deref(),
            Some("admin")
        );
        assert_eq!(parse_ciuser(&json!({})), None);
        assert!(valid_serial("serial0") && valid_serial("serial3"));
        assert!(!valid_serial("serial4") && !valid_serial("serial") && !valid_serial("serial00"));
        assert!(!valid_serial("socket"));
    }

    #[test]
    fn permissions() {
        let ok = json!({"/vms/100": {"VM.Console": 1, "VM.Audit": 1}});
        assert_eq!(parse_may_console(&ok, 100), Some(true));
        assert_eq!(parse_may_console(&ok, 101), Some(false));
        assert_eq!(
            parse_may_console(&json!({"/vms/100": {"VM.Audit": 1}}), 100),
            Some(false)
        );
        assert_eq!(parse_may_console(&Value::Null, 100), None);
    }

    #[test]
    fn login_users() {
        assert!(valid_login_user("root") && valid_login_user("first.last"));
        assert!(valid_login_user("svc_1-x") && valid_login_user("Admin"));
        assert!(!valid_login_user("") && !valid_login_user("-oProxy"));
        assert!(!valid_login_user("a b") && !valid_login_user("a@b") && !valid_login_user("a;b"));
    }

    #[test]
    fn node_lines() {
        assert_eq!(
            node_console_line(Kind::Lxc, 101, "pve1", None).unwrap(),
            "if [ \"$(hostname)\" = 'pve1' ]; then pct enter 101; else ssh -t root@pve1 pct enter 101; fi"
        );
        let l = node_console_line(Kind::Qemu, 100, "pve2", Some("serial1")).unwrap();
        assert!(l.contains("qm terminal 100 -iface serial1") && l.contains("root@pve2"));
        assert!(
            node_console_line(Kind::Qemu, 100, "pve2", None)
                .unwrap()
                .contains("qm terminal 100;")
        );
        assert_eq!(node_console_line(Kind::Lxc, 1, "x; rm -rf /", None), None);
        assert_eq!(node_console_line(Kind::Qemu, 1, "pve1", Some("x;y")), None);
    }
}
