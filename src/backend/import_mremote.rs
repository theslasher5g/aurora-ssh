//! mRemoteNG `confCons.xml`: `<Connections>` with nested `<Node Type="Container|Connection">`.
//! Passwords in the file are encrypted by mRemoteNG; they are never decrypted here.

use super::import_model::*;
use super::import_xml::{self, Elem};

fn truthy(s: Option<&str>) -> bool {
    s.is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

#[derive(Clone, Default)]
struct Inherit {
    user: String,
    domain: String,
}

pub fn parse(text: &str) -> Result<ImportDoc, String> {
    let root = import_xml::parse(text).map_err(|e| format!("XML: {e}"))?;
    if !root.name.eq_ignore_ascii_case("Connections") {
        return Err(
            t!("Das ist keine mRemoteNG-Datei (Wurzelelement <Connections> fehlt).").into(),
        );
    }
    let mut doc = ImportDoc::new(Source::MRemoteNg);
    let has_nodes = root.children.iter().any(|c| c.name == "Node");
    if truthy(root.attr("FullFileEncryption")) || (!has_nodes && !root.text.trim().is_empty()) {
        doc.encrypted = true;
        doc.warnings.push(
            t!("Die Datei ist vollständig verschlüsselt (FullFileEncryption). In mRemoteNG unter Extras → Optionen → Sicherheit die Verschlüsselung der gesamten Datei ausschalten oder per Datei → Exportieren ohne Verschlüsselung speichern.")
                .to_string(),
        );
        return Ok(doc);
    }
    if root.attr("EncryptionEngine").is_some() || root.attr("Protected").is_some() {
        doc.warnings.push(
            t!("mRemoteNG verschlüsselt die Passwörter in dieser Datei; sie werden nicht entschlüsselt und nicht importiert.")
                .to_string(),
        );
    }
    let mut path = Vec::new();
    for n in &root.children {
        walk(n, &mut path, &Inherit::default(), &mut doc, 0);
    }
    if doc.passwords_not_imported > 0 {
        doc.warnings.push(t!(
            "{} Einträge hatten ein Passwort; Passwörter werden bei mRemoteNG nicht importiert.",
            doc.passwords_not_imported
        ));
    }
    Ok(doc)
}

fn walk(n: &Elem, path: &mut Vec<String>, inh: &Inherit, doc: &mut ImportDoc, depth: usize) {
    if n.name != "Node" || depth > 64 {
        return;
    }
    let name = n.attr("Name").unwrap_or("");
    let ty = n.attr("Type").unwrap_or("Connection");
    let own = Inherit {
        user: n.attr("Username").unwrap_or("").to_string(),
        domain: n.attr("Domain").unwrap_or("").to_string(),
    };
    if ty.eq_ignore_ascii_case("Container") {
        let nm = clean_name(name);
        let pushed = !nm.is_empty();
        if pushed {
            path.push(nm);
        }
        // Children may inherit user / domain from the folder (which may inherit itself).
        let eff = Inherit {
            user: if own.user.is_empty() || truthy(n.attr("InheritUsername")) {
                inh.user.clone()
            } else {
                own.user.clone()
            },
            domain: if own.domain.is_empty() || truthy(n.attr("InheritDomain")) {
                inh.domain.clone()
            } else {
                own.domain.clone()
            },
        };
        for c in &n.children {
            walk(c, path, &eff, doc, depth + 1);
        }
        if pushed {
            path.pop();
        }
        return;
    }
    let (mut user, mut domain) = (own.user.clone(), own.domain.clone());
    if truthy(n.attr("InheritUsername")) {
        user = inh.user.clone();
    }
    if truthy(n.attr("InheritDomain")) {
        domain = inh.domain.clone();
    }
    let raw = n.attr("Protocol").unwrap_or("SSH2").to_string();
    let kind = Kind::from_text(&raw);
    let host = n.attr("Hostname").unwrap_or("").trim().to_string();
    if host.is_empty() {
        doc.skip(name, t!("Kein Hostname"));
        return;
    }
    if !n.attr("Password").unwrap_or("").is_empty() {
        doc.passwords_not_imported += 1;
    }
    if !domain.is_empty() && !user.is_empty() && kind == Kind::Rdp {
        user = format!("{domain}\\{user}");
    }
    let host = if kind == Kind::Web {
        host_of_url(&host)
    } else {
        host
    };
    let notes = n
        .attr("Descr")
        .or_else(|| n.attr("Description"))
        .unwrap_or("")
        .trim()
        .to_string();
    doc.push(ImportItem {
        name: clean_name(if name.is_empty() { &host } else { name }),
        host,
        port: n.attr("Port").and_then(parse_port),
        kind: Some(kind),
        raw_type: raw,
        user: user.trim().to_string(),
        password: String::new(),
        group: limit_depth(path.clone()),
        tags: Vec::new(),
        notes,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Connections Name="Connections" Export="false" EncryptionEngine="AES" BlockCipherMode="GCM" KdfIterations="1000" FullFileEncryption="false" Protected="GiUis20DIbnYzWPcdaQKfjE2H5jh//L5v4RGrJMGNXuIq2CttB/d/BxaBP2LwRhY" ConfVersion="2.6">
  <Node Name="Kunde A" Type="Container" Expanded="True" Username="admin" Domain="KA" Descr="" Panel="General">
    <Node Name="Standort Wien" Type="Container" Expanded="False">
      <Node Name="web01" Type="Connection" Descr="Webserver &amp; Proxy" Hostname="10.1.0.5" Protocol="SSH2" Port="2222" Username="root" Domain="" Password="AbCdEf==" Panel="General" InheritUsername="false" />
      <Node Name="rdp01" Type="Connection" Descr="" Hostname="rdp01.ka.local" Protocol="RDP" Port="3389" Username="" Domain="" Password="" InheritUsername="true" InheritDomain="true" />
    </Node>
    <Node Name="wiki" Type="Connection" Hostname="https://wiki.ka.local/start" Protocol="HTTPS" Port="443" />
  </Node>
  <Node Name="vnc-lab" Type="Connection" Hostname="192.168.0.9" Protocol="VNC" Port="5900" Username="" />
  <Node Name="leer" Type="Connection" Hostname="" Protocol="Telnet" />
</Connections>"#;

    #[test]
    fn parses_folders_and_hosts() {
        let d = parse(FIXTURE).unwrap();
        assert_eq!(d.items.len(), 4);
        let w = &d.items[0];
        assert_eq!(w.name, "web01");
        assert_eq!(w.group, ["Kunde A", "Standort Wien"]);
        assert_eq!(w.port, Some(2222));
        assert_eq!(w.notes, "Webserver & Proxy");
        assert_eq!(w.kind(), Kind::Ssh);
        assert!(w.password.is_empty());
        let r = &d.items[1];
        assert_eq!(r.user, "KA\\admin");
        assert_eq!(r.kind(), Kind::Rdp);
        assert_eq!(d.items[2].kind(), Kind::Web);
        assert_eq!(d.items[2].host, "wiki.ka.local");
        assert!(d.items[3].group.is_empty());
        assert_eq!(d.skipped_total, 1);
        assert_eq!(d.passwords_not_imported, 1);
        assert!(!d.encrypted);
        assert!(!d.warnings.is_empty());
    }

    #[test]
    fn full_file_encryption_is_reported() {
        let x = r#"<Connections Name="Connections" EncryptionEngine="AES" FullFileEncryption="true">c29tZWNpcGhlcg==</Connections>"#;
        let d = parse(x).unwrap();
        assert!(d.encrypted);
        assert!(d.items.is_empty());
    }

    #[test]
    fn wrong_root_is_an_error() {
        assert!(parse("<foo/>").is_err());
    }
}
