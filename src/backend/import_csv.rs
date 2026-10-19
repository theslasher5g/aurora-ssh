//! Generic CSV import: `name, host, port, protocol, user, group, tags, description`
//! (+ optional `password`). Header detection, `,` `;` tab delimiters, quotes, UTF-8 BOM.

use super::import_model::*;

/// RFC 4180 style reader: quoted fields, `""` escapes, line breaks inside quotes.
pub fn read_records(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            c if c == delim => {
                row.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                was_quoted = false;
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            c => field.push(c),
        }
    }
    if any || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    // Blank lines carry no record.
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    rows
}

/// The delimiter that occurs most often (outside quotes) in the first line.
pub fn detect_delimiter(text: &str) -> char {
    let mut counts = [(',', 0usize), (';', 0), ('\t', 0)];
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '\n' if !quoted => break,
            c if !quoted => {
                if let Some(e) = counts.iter_mut().find(|(d, _)| *d == c) {
                    e.1 += 1;
                }
            }
            _ => {}
        }
    }
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map_or(',', |(d, _)| *d)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Col {
    Name,
    Host,
    Port,
    Protocol,
    User,
    Group,
    Tags,
    Description,
    Password,
}

fn col_of(h: &str) -> Option<Col> {
    let h: String = h
        .trim()
        .trim_start_matches('\u{feff}')
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    Some(match h.as_str() {
        "name" | "label" | "title" | "bezeichnung" | "displayname" => Col::Name,
        "host" | "hostname" | "address" | "adresse" | "ip" | "server" | "computer"
        | "computername" => Col::Host,
        "port" => Col::Port,
        "protocol" | "protokoll" | "type" | "typ" | "connectiontype" => Col::Protocol,
        "user" | "username" | "benutzer" | "benutzername" | "login" => Col::User,
        "group" | "gruppe" | "folder" | "ordner" | "path" | "pfad" => Col::Group,
        "tags" | "tag" | "keywords" | "labels" => Col::Tags,
        "description" | "beschreibung" | "notes" | "notizen" | "comment" | "kommentar" => {
            Col::Description
        }
        "password" | "passwort" | "pass" | "pwd" => Col::Password,
        _ => return None,
    })
}

const DEFAULT_ORDER: [Col; 8] = [
    Col::Name,
    Col::Host,
    Col::Port,
    Col::Protocol,
    Col::User,
    Col::Group,
    Col::Tags,
    Col::Description,
];

pub fn parse(text: &str) -> Result<ImportDoc, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delim = detect_delimiter(text);
    let mut rows = read_records(text, delim);
    if rows.is_empty() {
        return Err(t!("Die CSV-Datei ist leer.").into());
    }
    // Header: at least two recognised column names (e.g. name+host) in the first row.
    let known: Vec<Option<Col>> = rows[0].iter().map(|h| col_of(h)).collect();
    let has_header = known.iter().flatten().count() >= 2
        || (known.iter().flatten().count() == 1 && known.contains(&Some(Col::Host)));
    let cols: Vec<Option<Col>> = if has_header {
        rows.remove(0);
        known
    } else {
        DEFAULT_ORDER.iter().map(|c| Some(*c)).collect()
    };
    if !cols.contains(&Some(Col::Host)) {
        return Err(t!("In der CSV-Datei fehlt die Spalte \"host\".").into());
    }
    let mut doc = ImportDoc::new(Source::Csv);
    let mut with_pw = 0usize;
    for (n, r) in rows.iter().enumerate() {
        let line = n + 1 + usize::from(has_header);
        let get = |c: Col| -> String {
            cols.iter()
                .position(|x| *x == Some(c))
                .and_then(|i| r.get(i))
                .map_or(String::new(), |s| s.trim().to_string())
        };
        let host = get(Col::Host);
        let name = get(Col::Name);
        if host.is_empty() {
            let label = if name.is_empty() {
                format!("#{line}")
            } else {
                name
            };
            doc.skip(&label, t!("Kein Hostname"));
            continue;
        }
        let proto = get(Col::Protocol);
        let kind = Kind::from_text(&proto);
        let host = if kind == Kind::Web {
            host_of_url(&host)
        } else {
            host
        };
        let password = get(Col::Password);
        if !password.is_empty() {
            with_pw += 1;
        }
        doc.push(ImportItem {
            name: clean_name(if name.is_empty() { &host } else { &name }),
            host,
            port: parse_port(&get(Col::Port)),
            kind: Some(kind),
            raw_type: proto,
            user: get(Col::User),
            password,
            group: limit_depth(split_path(&get(Col::Group))),
            tags: split_tags(&get(Col::Tags)),
            notes: get(Col::Description),
        });
    }
    if with_pw > 0 {
        doc.warnings.push(t!(
            "{} Zeilen enthalten ein Passwort im Klartext. Es wird nur importiert, wenn du es unten ausdrücklich erlaubst (Ablage im Tresor).",
            with_pw
        ));
    }
    if !has_header {
        doc.warnings.push(
            t!("Keine Kopfzeile erkannt: Spaltenreihenfolge name, host, port, protocol, user, group, tags, description angenommen.")
                .to_string(),
        );
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_delimiters_and_newlines() {
        let r = read_records("a,\"b,c\",\"d\"\"e\"\n\"x\ny\",z,\r\n\r\nlast", ',');
        assert_eq!(r[0], ["a", "b,c", "d\"e"]);
        assert_eq!(r[1], ["x\ny", "z", ""]);
        assert_eq!(r[2], ["last"]);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn delimiter_detection() {
        assert_eq!(detect_delimiter("name;host;port\n1,2,3"), ';');
        assert_eq!(detect_delimiter("name\thost"), '\t');
        assert_eq!(detect_delimiter("\"a;b\",c,d"), ',');
        assert_eq!(detect_delimiter("single"), ',');
    }

    #[test]
    fn parses_with_header_and_bom() {
        let csv = "\u{feff}Name;Host;Port;Protocol;User;Group;Tags;Description;Password\n\
                   web01;10.0.0.1;2222;ssh;root;Kunde/Wien;prod, web;\"Haupt;server\";geheim\n\
                   rdp1;rdp.local;;RDP;;Kunde\\Wien;;;\n\
                   ;;;;;;;;\n\
                   ohne;;;;;;;;\n";
        let d = parse(csv).unwrap();
        assert_eq!(d.items.len(), 2);
        let a = &d.items[0];
        assert_eq!(a.name, "web01");
        assert_eq!(a.port, Some(2222));
        assert_eq!(a.group, ["Kunde", "Wien"]);
        assert_eq!(a.tags, ["prod", "web"]);
        assert_eq!(a.notes, "Haupt;server");
        assert_eq!(a.password, "geheim");
        assert_eq!(d.items[1].kind(), Kind::Rdp);
        assert_eq!(d.items[1].port, None);
        assert_eq!(d.skipped_total, 2);
        assert!(
            d.warnings
                .iter()
                .any(|w| w.contains("Klartext") || w.contains("plain"))
        );
    }

    #[test]
    fn headerless_uses_default_order() {
        let d = parse("srv1,10.1.1.1,22,ssh,me,G,t1,desc\n").unwrap();
        assert_eq!(d.items.len(), 1);
        assert_eq!(d.items[0].host, "10.1.1.1");
        assert_eq!(d.items[0].user, "me");
        assert_eq!(d.items[0].group, ["G"]);
    }

    #[test]
    fn missing_host_column_is_an_error() {
        assert!(parse("name,port\na,22\n").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn host_only_column_works() {
        let d = parse("host\nsrv1\nsrv2\n").unwrap();
        assert_eq!(d.items.len(), 2);
        assert_eq!(d.items[0].name, "srv1");
    }
}
