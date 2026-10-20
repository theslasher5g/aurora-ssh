//! Devolutions Remote Desktop Manager export: JSON ("Export → JSON") and the XML `.rdm`
//! export (`<ArrayOfConnection><Connection>…`). Best effort: the layout differs between
//! RDM versions, so several field names are tried, also one level deep.

use serde_json::{Map, Value};

use super::import_model::*;
use super::import_royal::{jget, jtags};
use super::import_xml::{self, Elem};

fn to_value(e: &Elem, depth: usize) -> Value {
    if e.children.is_empty() || depth > 16 {
        return Value::String(e.text.trim().to_string());
    }
    let mut m = Map::new();
    for (k, v) in &e.attrs {
        m.insert(k.clone(), Value::String(v.clone()));
    }
    for c in &e.children {
        m.insert(c.name.clone(), to_value(c, depth + 1));
    }
    Value::Object(m)
}

/// Looks up a field at the top level, then inside nested objects.
fn find(o: &Value, keys: &[&str]) -> Option<String> {
    if let Some(v) = jget(o, keys) {
        return Some(v);
    }
    o.as_object()?
        .values()
        .filter(|v| v.is_object())
        .find_map(|v| jget(v, keys))
}

pub fn parse_json(text: &str) -> Result<ImportDoc, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("JSON: {e}"))?;
    from_value(&v)
}

pub fn parse_xml(text: &str) -> Result<ImportDoc, String> {
    let root = import_xml::parse(text).map_err(|e| format!("XML: {e}"))?;
    let list: Vec<Value> = if root.name.eq_ignore_ascii_case("Connection") {
        vec![to_value(&root, 0)]
    } else {
        root.children
            .iter()
            .filter(|c| c.name.eq_ignore_ascii_case("Connection"))
            .map(|c| to_value(c, 0))
            .collect()
    };
    if list.is_empty() {
        return Err(t!("Keine Verbindungen gefunden (<Connection> fehlt).").into());
    }
    from_value(&Value::Array(list))
}

fn from_value(v: &Value) -> Result<ImportDoc, String> {
    let list: &Vec<Value> = match v {
        Value::Array(a) => a,
        Value::Object(m) => m
            .iter()
            .find(|(k, v)| {
                v.is_array()
                    && ["connections", "items", "entries", "data"]
                        .iter()
                        .any(|n| k.eq_ignore_ascii_case(n))
            })
            .and_then(|(_, v)| v.as_array())
            .ok_or_else(|| t!("Das ist kein Remote-Desktop-Manager-Export.").to_string())?,
        _ => return Err(t!("Das ist kein Remote-Desktop-Manager-Export.").into()),
    };
    let mut doc = ImportDoc::new(Source::Rdm);
    let mut creds = 0usize;
    for o in list {
        if !o.is_object() {
            continue;
        }
        let name = jget(o, &["Name"]).unwrap_or_default();
        let ty = jget(o, &["ConnectionType", "Type", "Kind"]).unwrap_or_default();
        let lty = ty.to_ascii_lowercase();
        if lty == "group" || lty == "folder" {
            continue;
        }
        if lty.contains("credential") || lty.contains("password") || lty.contains("vault") {
            creds += 1;
            doc.skip(
                &name,
                t!("Anmeldedaten-Eintrag (Passwörter werden nicht importiert)"),
            );
            continue;
        }
        let kind = Kind::from_text(&ty);
        if kind == Kind::Other {
            doc.skip(&name, t!("Nicht unterstützter Typ: {}", ty));
            continue;
        }
        let mut host = find(
            o,
            &["Host", "HostName", "Computer", "Server", "ComputerName"],
        )
        .unwrap_or_default();
        let mut web_url = String::new();
        if host.is_empty() {
            let u = find(o, &["Url", "URL", "Uri"]).unwrap_or_default();
            if kind == Kind::Web && u.contains("://") {
                web_url = u.clone();
            }
            host = host_of_url(&u);
        } else if kind == Kind::Web {
            if host.contains("://") {
                web_url = host.clone();
            }
            host = host_of_url(&host);
        }
        if host.is_empty() {
            doc.skip(&name, t!("Kein Hostname"));
            continue;
        }
        if find(o, &["Password", "SafePassword", "PasswordItem"]).is_some() {
            doc.passwords_not_imported += 1;
        }
        let group = jget(o, &["Group", "Folder"]).unwrap_or_default();
        doc.push(ImportItem {
            name: clean_name(if name.is_empty() { &host } else { &name }),
            host,
            url: web_url,
            port: find(o, &["Port", "HostPort"])
                .as_deref()
                .and_then(parse_port),
            kind: Some(kind),
            raw_type: ty,
            user: find(o, &["UserName", "Username", "User", "Login"]).unwrap_or_default(),
            password: String::new(),
            group: limit_depth(split_path(&group)),
            tags: {
                let mut t = jtags(o, &["Keywords", "Tags"]);
                // RDM separates keywords by spaces when there is no other delimiter.
                if t.len() == 1 && t[0].contains(' ') {
                    t = t[0].split_whitespace().map(clean_name).collect();
                }
                t
            },
            notes: jget(o, &["Description"]).unwrap_or_default(),
        });
    }
    if creds > 0 {
        doc.warnings.push(t!(
            "{} Anmeldedaten-Einträge übersprungen; Passwörter werden aus RDM-Exporten nicht übernommen.",
            creds
        ));
    }
    if doc.passwords_not_imported > 0 {
        doc.warnings.push(t!(
            "{} Einträge hatten ein Passwort, das nicht importiert wird.",
            doc.passwords_not_imported
        ));
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
  {"ConnectionType":"Group","Name":"Kunde","Group":"Kunde"},
  {"ConnectionType":"RDPConfigured","Name":"ts-01","Group":"Kunde\\Standort Nord","Host":"ts-01.kunde.local","Port":3389,
   "Description":"Terminalserver","Keywords":"prod;windows","UserName":"kunde\\svc"},
  {"ConnectionType":"SSHShell","Name":"linux-01","Group":"Kunde\\Standort Nord\\Labor","Host":"10.4.4.4","HostPort":"2200","Username":"ops","Password":"x"},
  {"ConnectionType":"VNC","Name":"vnc-1","Host":"10.4.4.9"},
  {"ConnectionType":"Telnet","Name":"sw","Group":"Netz","Host":"10.9.9.9"},
  {"ConnectionType":"WebBrowser","Name":"wiki","Group":"Netz","Url":"https://wiki.kunde.local/"},
  {"ConnectionType":"Credential","Name":"Domain admin","Group":"Kunde"},
  {"ConnectionType":"Document","Name":"Notizen"}
]"#;

    #[test]
    fn parses_json_export() {
        let d = parse_json(JSON).unwrap();
        assert_eq!(d.items.len(), 5);
        assert_eq!(d.items[0].group, ["Kunde", "Standort Nord"]);
        assert_eq!(d.items[0].tags, ["prod", "windows"]);
        assert_eq!(d.items[0].user, "kunde\\svc");
        assert_eq!(d.items[1].group.len(), 3);
        assert_eq!(d.items[1].port, Some(2200));
        assert_eq!(d.items[1].user, "ops");
        assert!(d.items[1].password.is_empty());
        assert_eq!(d.items[2].group.len(), 0);
        assert_eq!(d.items[4].kind(), Kind::Web);
        assert_eq!(d.items[4].host, "wiki.kunde.local");
        assert_eq!(d.skipped_total, 2);
        assert!(d.skipped[0].reason.contains("Anmeldedaten"));
        assert_eq!(d.passwords_not_imported, 1);
    }

    #[test]
    fn parses_wrapped_object() {
        let d = parse_json(r#"{"Connections":[{"ConnectionType":"VNC","Name":"a","Host":"h"}]}"#)
            .unwrap();
        assert_eq!(d.items.len(), 1);
        assert!(parse_json("{\"x\":[]}").is_err());
    }

    #[test]
    fn parses_xml_export() {
        let x = r#"<?xml version="1.0" encoding="utf-8"?>
<ArrayOfConnection>
 <Connection>
  <ConnectionType>RDPConfigured</ConnectionType>
  <Name>srv &amp; co</Name>
  <Group>Firma\Berlin</Group>
  <Url>srv.firma.de</Url>
  <Description>Mail</Description>
  <Keywords>mail</Keywords>
 </Connection>
 <Connection><ConnectionType>Credential</ConnectionType><Name>cred</Name></Connection>
</ArrayOfConnection>"#;
        let d = parse_xml(x).unwrap();
        assert_eq!(d.items.len(), 1);
        assert_eq!(d.items[0].name, "srv & co");
        assert_eq!(d.items[0].group, ["Firma", "Berlin"]);
        assert_eq!(d.items[0].host, "srv.firma.de");
        assert_eq!(d.skipped_total, 1);
    }
}
