//! Royal TS / Royal TSX "Royal JSON" export: nested `Objects` with folders, connections
//! (`RoyalSSHConnection`, `RoyalRDSConnection`, `RoyalVNCConnection`,
//! `RoyalTerminalConnection`, `RoyalWebConnection` …) and `RoyalCredential` objects.
//! Credentials are only read for their user names; passwords are never imported.

use std::collections::HashMap;

use serde_json::Value;

use super::import_model::*;

/// Text value of the first matching key (case-insensitive); numbers and bools are
/// converted.
pub fn jget(o: &Value, keys: &[&str]) -> Option<String> {
    let m = o.as_object()?;
    for k in keys {
        for (name, v) in m {
            if name.eq_ignore_ascii_case(k) {
                match v {
                    Value::String(s) if !s.trim().is_empty() => return Some(s.trim().to_string()),
                    Value::Number(n) => return Some(n.to_string()),
                    Value::Bool(b) => return Some(b.to_string()),
                    _ => {}
                }
            }
        }
    }
    None
}

/// Tags given as `"a, b"`, `["a","b"]` or `[{"Name":"a"}]`.
pub fn jtags(o: &Value, keys: &[&str]) -> Vec<String> {
    let Some(m) = o.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for k in keys {
        for (name, v) in m {
            if !name.eq_ignore_ascii_case(k) {
                continue;
            }
            match v {
                Value::String(s) => out.extend(split_tags(s)),
                Value::Array(a) => {
                    for e in a {
                        match e {
                            Value::String(s) => out.extend(split_tags(s)),
                            Value::Object(_) => {
                                if let Some(n) = jget(e, &["Name", "Title", "Value"]) {
                                    out.extend(split_tags(&n));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out.dedup();
    out.truncate(30);
    out
}

fn objects(o: &Value) -> Option<&Vec<Value>> {
    let m = o.as_object()?;
    m.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Objects"))
        .and_then(|(_, v)| v.as_array())
}

pub fn parse(text: &str) -> Result<ImportDoc, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("JSON: {e}"))?;
    let top: Vec<&Value> = match &v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => match objects(&v) {
            Some(a) => a.iter().collect(),
            None => return Err(t!("Das ist kein Royal-TS-Export (\"Objects\" fehlt).").into()),
        },
        _ => return Err(t!("Das ist kein Royal-TS-Export (\"Objects\" fehlt).").into()),
    };
    let mut doc = ImportDoc::new(Source::RoyalTs);
    // Pass 1: credential objects → user names (by id and by name).
    let mut creds: HashMap<String, String> = HashMap::new();
    let mut cred_count = 0usize;
    collect_creds(&top, &mut creds, &mut cred_count, 0);
    // Pass 2: folders and connections.
    let mut path = Vec::new();
    walk(&top, &mut path, &creds, &mut doc, 0);
    if cred_count > 0 {
        doc.warnings.push(t!(
            "{} Anmeldedaten-Objekte nicht importiert: Royal TS exportiert keine Passwörter im Klartext. Benutzernamen wurden den Verbindungen zugeordnet.",
            cred_count
        ));
    }
    Ok(doc)
}

fn collect_creds(list: &[&Value], out: &mut HashMap<String, String>, n: &mut usize, depth: usize) {
    if depth > 64 {
        return;
    }
    for o in list {
        let ty = jget(o, &["Type"]).unwrap_or_default();
        if ty.to_ascii_lowercase().contains("credential") {
            *n += 1;
            let user = jget(o, &["UserName", "Username", "User"]).unwrap_or_default();
            if let Some(id) = jget(o, &["ID", "Id", "Uid"]) {
                out.insert(id, user.clone());
            }
            if let Some(name) = jget(o, &["Name"]) {
                out.insert(name, user);
            }
        }
        if let Some(children) = objects(o) {
            let c: Vec<&Value> = children.iter().collect();
            collect_creds(&c, out, n, depth + 1);
        }
    }
}

fn walk(
    list: &[&Value],
    path: &mut Vec<String>,
    creds: &HashMap<String, String>,
    doc: &mut ImportDoc,
    depth: usize,
) {
    if depth > 64 {
        return;
    }
    for o in list {
        let ty = jget(o, &["Type"]).unwrap_or_default();
        let lty = ty.to_ascii_lowercase();
        let name = jget(o, &["Name", "DisplayName"]).unwrap_or_default();
        if lty.contains("folder") || (lty.is_empty() && objects(o).is_some()) {
            let nm = clean_name(&name);
            let pushed = !nm.is_empty();
            if pushed {
                path.push(nm);
            }
            if let Some(children) = objects(o) {
                let c: Vec<&Value> = children.iter().collect();
                walk(&c, path, creds, doc, depth + 1);
            }
            if pushed {
                path.pop();
            }
            continue;
        }
        if lty.contains("credential") {
            continue;
        }
        if lty.is_empty() {
            continue;
        }
        if !lty.contains("connection") {
            // Tasks, key sequences, dashboards …
            doc.skip(&name, t!("Kein Verbindungsobjekt ({})", ty));
            continue;
        }
        // Terminal connections carry the protocol in a property.
        let mut kind_text = ty.clone();
        if lty.contains("terminal")
            && let Some(t) = jget(o, &["TerminalConnectionType", "ConnectionType", "Protocol"])
        {
            kind_text = t;
        }
        let kind = Kind::from_text(&kind_text);
        if kind == Kind::Other {
            doc.skip(&name, t!("Nicht unterstützter Typ: {}", ty));
            continue;
        }
        let url = jget(o, &["URL", "URI", "Uri", "Url"]);
        let mut host = jget(
            o,
            &["ComputerName", "HostName", "Host", "Computer", "Address"],
        )
        .unwrap_or_default();
        if host.is_empty()
            && let Some(u) = &url
        {
            host = host_of_url(u);
        }
        if host.is_empty() {
            doc.skip(&name, t!("Kein Hostname"));
            continue;
        }
        let mut user = jget(o, &["CredentialUsername", "UserName", "Username"]).unwrap_or_default();
        if user.is_empty() {
            for key in ["CredentialId", "CredentialID", "CredentialName"] {
                if let Some(r) = jget(o, &[key]) {
                    let r = r.rsplit(['/', '\\']).next().unwrap_or("").to_string();
                    if let Some(u) = creds.get(&r).filter(|u| !u.is_empty()) {
                        user = u.clone();
                        break;
                    }
                }
            }
        }
        let tags = jtags(o, &["Tags", "Tag"]);
        doc.push(ImportItem {
            name: clean_name(if name.is_empty() { &host } else { &name }),
            host,
            port: jget(o, &["Port"]).as_deref().and_then(parse_port),
            kind: Some(kind),
            raw_type: ty,
            user,
            password: String::new(),
            group: limit_depth(path.clone()),
            tags,
            notes: jget(o, &["Description", "Notes"]).unwrap_or_default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const FIXTURE: &str = r#"{
  "Objects": [
    { "Type": "RoyalFolder", "Name": "Kunden", "Objects": [
        { "Type": "RoyalFolder", "Name": "Müller GmbH", "Objects": [
            { "Type": "RoyalSSHConnection", "Name": "db01", "ComputerName": "db01.mueller.local", "Port": 22,
              "CredentialId": "c-1", "Description": "Datenbank", "Tags": "prod, db" },
            { "Type": "RoyalRDSConnection", "Name": "ts01", "ComputerName": "10.2.0.7", "Port": 3390,
              "CredentialName": "Admin", "Tags": [ "windows", {"Name":"prod"} ] },
            { "Type": "RoyalWebConnection", "Name": "Portal", "URL": "https://portal.mueller.local/login" },
            { "Type": "RoyalFileTransferConnection", "Name": "ftp", "ComputerName": "ftp.mueller.local" }
        ] }
    ] },
    { "Type": "RoyalVNCConnection", "Name": "kiosk", "ComputerName": "192.168.5.5" },
    { "Type": "RoyalTerminalConnection", "Name": "switch", "ComputerName": "sw1", "TerminalConnectionType": "Telnet" },
    { "Type": "RoyalCredential", "Name": "Admin", "ID": "c-1", "UserName": "mueller\\admin", "Password": "geheim" },
    { "Type": "RoyalTask", "Name": "Backup" }
  ]
}"#;

    #[test]
    fn parses_royal_json() {
        let d = parse(FIXTURE).unwrap();
        assert_eq!(d.items.len(), 5);
        let db = &d.items[0];
        assert_eq!(db.group, ["Kunden", "Müller GmbH"]);
        assert_eq!(db.user, "mueller\\admin");
        assert_eq!(db.tags, ["prod", "db"]);
        assert_eq!(db.notes, "Datenbank");
        let ts = &d.items[1];
        assert_eq!(ts.kind(), Kind::Rdp);
        assert_eq!(ts.port, Some(3390));
        assert_eq!(ts.user, "mueller\\admin");
        assert_eq!(ts.tags, ["windows", "prod"]);
        assert_eq!(d.items[2].kind(), Kind::Web);
        assert_eq!(d.items[2].host, "portal.mueller.local");
        assert_eq!(d.items[3].kind(), Kind::Vnc);
        assert_eq!(d.items[4].kind(), Kind::Telnet);
        // file transfer + task are reported, the password is never read
        assert_eq!(d.skipped_total, 2);
        assert!(d.items.iter().all(|i| i.password.is_empty()));
        assert!(!d.warnings.is_empty());
    }

    #[test]
    fn rejects_other_json() {
        assert!(parse("{\"a\":1}").is_err());
        assert!(parse("not json").is_err());
    }
}
