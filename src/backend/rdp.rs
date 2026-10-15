//! RDP: hands the session to the platform's native RDP client
//! (mstsc on Windows, the Windows App / Microsoft Remote Desktop on macOS,
//! FreeRDP or Remmina on Linux) with a generated connection profile.

use std::io::Write;
use std::process::{Child, Command, Stdio};

use anyhow::{Result, anyhow};

use crate::model::RdpSettings;

pub struct RdpLaunch {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub settings: RdpSettings,
}

pub struct RdpProcess {
    pub child: Option<Child>,
    pub client: String,
}

fn rdp_file(l: &RdpLaunch) -> String {
    let mut s = String::new();
    s += &format!("full address:s:{}:{}\r\n", l.host, l.port);
    if !l.username.is_empty() {
        s += &format!("username:s:{}\r\n", l.username);
    }
    if !l.settings.domain.is_empty() {
        s += &format!("domain:s:{}\r\n", l.settings.domain);
    }
    s += &format!(
        "screen mode id:i:{}\r\n",
        if l.settings.fullscreen { 2 } else { 1 }
    );
    s += &format!(
        "desktopwidth:i:{}\r\ndesktopheight:i:{}\r\n",
        l.settings.width, l.settings.height
    );
    s += &format!("redirectclipboard:i:{}\r\n", l.settings.clipboard as u8);
    s += &format!("use multimon:i:{}\r\n", l.settings.multimon as u8);
    s += "dynamic resolution:i:1\r\nsession bpp:i:32\r\nprompt for credentials:i:0\r\n";
    s
}

fn which(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(bin);
        p.is_file() || (cfg!(windows) && p.with_extension("exe").is_file())
    })
}

pub fn launch(l: &RdpLaunch) -> Result<RdpProcess> {
    if cfg!(target_os = "windows") {
        let path = std::env::temp_dir().join(format!(
            "aurora-{}.rdp",
            l.host.replace([':', '/', '\\'], "_")
        ));
        std::fs::write(&path, rdp_file(l))?;
        if let Some(pw) = &l.password {
            // Store the credential so mstsc can log in without prompting.
            let user = if l.settings.domain.is_empty() {
                l.username.clone()
            } else {
                format!("{}\\{}", l.settings.domain, l.username)
            };
            let _ = Command::new("cmdkey")
                .arg(format!("/generic:TERMSRV/{}", l.host))
                .arg(format!("/user:{user}"))
                .arg(format!("/pass:{pw}"))
                .stdout(Stdio::null())
                .status();
        }
        let child = Command::new("mstsc").arg(&path).spawn()?;
        return Ok(RdpProcess {
            child: Some(child),
            client: "mstsc".into(),
        });
    }

    if cfg!(target_os = "macos") {
        let path =
            std::env::temp_dir().join(format!("aurora-{}.rdp", l.host.replace([':', '/'], "_")));
        std::fs::write(&path, rdp_file(l))?;
        Command::new("open").arg(&path).status()?;
        return Ok(RdpProcess {
            child: None,
            client: "Windows App".into(),
        });
    }

    for bin in [
        "xfreerdp3",
        "xfreerdp",
        "sdl-freerdp3",
        "sdl-freerdp",
        "wlfreerdp",
    ] {
        if !which(bin) {
            continue;
        }
        let mut cmd = Command::new(bin);
        cmd.arg(format!("/v:{}:{}", l.host, l.port))
            .arg("/cert:tofu")
            .arg("/dynamic-resolution")
            .arg(format!("/title:Aurora · {}", l.host));
        if !l.username.is_empty() {
            cmd.arg(format!("/u:{}", l.username));
        }
        if !l.settings.domain.is_empty() {
            cmd.arg(format!("/d:{}", l.settings.domain));
        }
        if l.settings.fullscreen {
            cmd.arg("/f");
        } else {
            cmd.arg(format!("/size:{}x{}", l.settings.width, l.settings.height));
        }
        if l.settings.clipboard {
            cmd.arg("+clipboard");
        }
        if l.settings.multimon {
            cmd.arg("/multimon");
        }
        // Pass the password over stdin rather than argv so it does not show up in `ps`.
        if l.password.is_some() {
            cmd.arg("/from-stdin:force").stdin(Stdio::piped());
        }
        let mut child = cmd.spawn()?;
        if let (Some(pw), Some(mut stdin)) = (&l.password, child.stdin.take()) {
            // FreeRDP prompts for every missing field in order: user, domain, password.
            if l.username.is_empty() {
                let _ = writeln!(stdin);
            }
            if l.settings.domain.is_empty() {
                let _ = writeln!(stdin);
            }
            let _ = writeln!(stdin, "{pw}");
        }
        return Ok(RdpProcess {
            child: Some(child),
            client: bin.into(),
        });
    }

    if which("remmina") {
        let user = if l.username.is_empty() {
            String::new()
        } else {
            format!("{}@", l.username)
        };
        let child = Command::new("remmina")
            .arg("-c")
            .arg(format!("rdp://{user}{}:{}", l.host, l.port))
            .spawn()?;
        return Ok(RdpProcess {
            child: Some(child),
            client: "remmina".into(),
        });
    }

    Err(anyhow!(
        "Kein RDP-Client gefunden. Installiere FreeRDP (xfreerdp) oder Remmina."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_profile_contains_address() {
        let l = RdpLaunch {
            host: "10.0.0.5".into(),
            port: 3390,
            username: "admin".into(),
            password: None,
            settings: RdpSettings::default(),
        };
        let f = rdp_file(&l);
        assert!(f.contains("full address:s:10.0.0.5:3390"));
        assert!(f.contains("username:s:admin"));
    }
}
