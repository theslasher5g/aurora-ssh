//! Migration importer: picks the right parser for a file.

use std::path::Path;

use super::import_model::*;
use super::{import_csv, import_mremote, import_rdm, import_royal};

/// Reads and parses `path`; the format is recognised from the content (the extension is
/// only a hint).
pub fn parse_file(path: &Path) -> Result<ImportDoc, String> {
    let bytes = read_limited(path)?;
    parse_bytes(&bytes, &path.to_string_lossy())
}

pub fn parse_bytes(bytes: &[u8], name: &str) -> Result<ImportDoc, String> {
    let lname = name.to_ascii_lowercase();
    if bytes.starts_with(b"PK\x03\x04") {
        return Err(if lname.ends_with(".rtsz") || lname.ends_with(".rtsx") {
            t!("Royal-TS-Dokumente (.rtsz/.rtsx) sind Archive und können nicht gelesen werden. In Royal TS: Datei → Importieren & Exportieren → Exportieren → \"Royal JSON\" wählen und die .json-Datei hier öffnen.").to_string()
        } else {
            t!("ZIP-Archive werden nicht gelesen. Bitte die Exportdatei (.json, .xml, .csv) entpacken und diese öffnen.").to_string()
        });
    }
    let text = decode_text(bytes);
    let t = text.trim_start();
    if t.starts_with('<') {
        let head: String = t.chars().take(4000).collect();
        return if head.contains("<Connections") || head.contains(":Connections") {
            import_mremote::parse(&text)
        } else if head.contains("<ArrayOfConnection") || head.contains("<Connection") {
            import_rdm::parse_xml(&text)
        } else {
            Err(t!("Unbekanntes XML-Format. Unterstützt: mRemoteNG confCons.xml und Remote Desktop Manager .rdm.").to_string())
        };
    }
    if t.starts_with('{') || t.starts_with('[') {
        let looks_royal = {
            let head: String = t.chars().take(4000).collect();
            head.contains("\"Objects\"") || head.contains("\"Royal")
        };
        return if looks_royal {
            import_royal::parse(&text)
        } else {
            import_rdm::parse_json(&text)
        };
    }
    import_csv::parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        let m = parse_bytes(
            br#"<Connections><Node Name="a" Type="Connection" Hostname="h"/></Connections>"#,
            "confCons.xml",
        )
        .unwrap();
        assert_eq!(m.source, Some(Source::MRemoteNg));
        let r = parse_bytes(
            br#"{"Objects":[{"Type":"RoyalSSHConnection","Name":"a","ComputerName":"h"}]}"#,
            "x.json",
        )
        .unwrap();
        assert_eq!(r.source, Some(Source::RoyalTs));
        let d = parse_bytes(
            br#"[{"ConnectionType":"SSHShell","Name":"a","Host":"h"}]"#,
            "x.json",
        )
        .unwrap();
        assert_eq!(d.source, Some(Source::Rdm));
        let c = parse_bytes(b"name,host\na,h\n", "x.csv").unwrap();
        assert_eq!(c.source, Some(Source::Csv));
        assert!(parse_bytes(b"PK\x03\x04zzz", "doc.rtsz").is_err());
        assert!(parse_bytes(b"<foo/>", "x.xml").is_err());
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        for b in [
            &b"{"[..],
            b"<",
            b"\xff\xfe\x00",
            b"[1,2",
            b"<Connections><Node",
        ] {
            let _ = parse_bytes(b, "x");
        }
    }
}
