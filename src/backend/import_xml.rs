//! Small, strict XML reader for the importer. No DTD processing: a `<!DOCTYPE>` is
//! skipped, and anything but the five predefined entities and numeric character
//! references is rejected, so entity expansion ("billion laughs") and external entities
//! cannot happen. Depth and node count are limited.

pub const MAX_XML_DEPTH: usize = 64;
pub const MAX_XML_NODES: usize = 1_500_000;

#[derive(Debug, Default, Clone)]
pub struct Elem {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Elem>,
    pub text: String,
}

impl Elem {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// First child with this name (case-insensitive).
    #[cfg(test)]
    pub fn child(&self, name: &str) -> Option<&Elem> {
        self.children
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Text of the child with this name, trimmed.
    #[cfg(test)]
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }
}

fn decode_entities(s: &str) -> Result<String, String> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find(';').filter(|e| *e <= 10) else {
            return Err("ungültige Entität".into());
        };
        let ent = &after[..end];
        match ent {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code =
                    if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                        u32::from_str_radix(h, 16).ok()
                    } else if let Some(d) = ent.strip_prefix('#') {
                        d.parse::<u32>().ok()
                    } else {
                        None
                    };
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => return Err(format!("nicht unterstützte Entität &{ent};")),
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Parses a document and returns its root element.
pub fn parse(src: &str) -> Result<Elem, String> {
    let b = src.as_bytes();
    let mut pos = 0usize;
    let mut stack: Vec<Elem> = Vec::new();
    let mut root: Option<Elem> = None;
    let mut nodes = 0usize;

    while pos < b.len() {
        if b[pos] != b'<' {
            let end = src[pos..].find('<').map_or(b.len(), |e| pos + e);
            if let Some(top) = stack.last_mut() {
                let t = &src[pos..end];
                if !t.trim().is_empty() && top.text.len() < 1 << 20 {
                    top.text.push_str(&decode_entities(t)?);
                }
            }
            pos = end;
            continue;
        }
        let rest = &src[pos..];
        if rest.starts_with("<!--") {
            let e = rest.find("-->").ok_or("Kommentar nicht beendet")?;
            pos += e + 3;
        } else if rest.starts_with("<![CDATA[") {
            let e = rest.find("]]>").ok_or("CDATA nicht beendet")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&rest[9..e]);
            }
            pos += e + 3;
        } else if rest.starts_with("<?") {
            let e = rest
                .find("?>")
                .ok_or("Verarbeitungsanweisung nicht beendet")?;
            pos += e + 2;
        } else if rest.starts_with("<!") {
            // DOCTYPE: skipped, with a possible internal subset in [...]. Entities declared
            // there are never expanded (undefined entities are an error above).
            let mut depth = 0i32;
            let mut i = 2;
            let rb = rest.as_bytes();
            while i < rb.len() {
                match rb[i] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
                i += 1;
            }
            if i >= rb.len() {
                return Err("DOCTYPE nicht beendet".into());
            }
            pos += i + 1;
        } else if rest.starts_with("</") {
            let e = rest.find('>').ok_or("Endtag nicht beendet")?;
            let name = rest[2..e].trim();
            let el = stack.pop().ok_or("Unerwartetes Endtag")?;
            if el.name != name {
                return Err(format!("Endtag </{name}> passt nicht zu <{}>", el.name));
            }
            match stack.last_mut() {
                Some(p) => p.children.push(el),
                None => {
                    if root.is_some() {
                        return Err("Mehrere Wurzelelemente".into());
                    }
                    root = Some(el);
                }
            }
            pos += e + 1;
        } else {
            // start tag
            let mut i = 1;
            let name_start = i;
            while pos + i < b.len()
                && !b[pos + i].is_ascii_whitespace()
                && b[pos + i] != b'>'
                && b[pos + i] != b'/'
            {
                i += 1;
            }
            let name = src[pos + name_start..pos + i].to_string();
            if name.is_empty() {
                return Err("Leerer Elementname".into());
            }
            let mut el = Elem {
                name,
                ..Default::default()
            };
            let mut self_close = false;
            loop {
                while pos + i < b.len() && b[pos + i].is_ascii_whitespace() {
                    i += 1;
                }
                if pos + i >= b.len() {
                    return Err("Starttag nicht beendet".into());
                }
                match b[pos + i] {
                    b'>' => {
                        i += 1;
                        break;
                    }
                    b'/' => {
                        self_close = true;
                        i += 1;
                    }
                    _ => {
                        let ks = i;
                        while pos + i < b.len()
                            && !matches!(b[pos + i], b'=' | b'>' | b'/')
                            && !b[pos + i].is_ascii_whitespace()
                        {
                            i += 1;
                        }
                        let key = src[pos + ks..pos + i].to_string();
                        while pos + i < b.len() && b[pos + i].is_ascii_whitespace() {
                            i += 1;
                        }
                        if pos + i >= b.len() || b[pos + i] != b'=' {
                            return Err(format!("Attribut {key} ohne Wert"));
                        }
                        i += 1;
                        while pos + i < b.len() && b[pos + i].is_ascii_whitespace() {
                            i += 1;
                        }
                        let q = *b.get(pos + i).ok_or("Attribut nicht beendet")?;
                        if q != b'"' && q != b'\'' {
                            return Err(format!("Attribut {key} ohne Anführungszeichen"));
                        }
                        i += 1;
                        let vs = i;
                        while pos + i < b.len() && b[pos + i] != q {
                            i += 1;
                        }
                        if pos + i >= b.len() {
                            return Err("Attribut nicht beendet".into());
                        }
                        let val = decode_entities(&src[pos + vs..pos + i])?;
                        i += 1;
                        el.attrs.push((key, val));
                    }
                }
            }
            pos += i;
            nodes += 1;
            if nodes > MAX_XML_NODES {
                return Err("Zu viele Elemente".into());
            }
            if self_close {
                match stack.last_mut() {
                    Some(p) => p.children.push(el),
                    None => {
                        if root.is_some() {
                            return Err("Mehrere Wurzelelemente".into());
                        }
                        root = Some(el);
                    }
                }
            } else {
                if stack.len() >= MAX_XML_DEPTH {
                    return Err("XML zu tief verschachtelt".into());
                }
                stack.push(el);
            }
        }
    }
    if !stack.is_empty() {
        return Err("Dokument nicht vollständig (Endtag fehlt)".into());
    }
    root.ok_or_else(|| "Kein Wurzelelement".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested() {
        let r = parse(
            "<?xml version=\"1.0\"?><!-- c --><a x=\"1&amp;2\" y='z'><b>t&lt;1</b><c/><d><![CDATA[<raw>]]></d></a>",
        )
        .unwrap();
        assert_eq!(r.attr("x"), Some("1&2"));
        assert_eq!(r.attr("y"), Some("z"));
        assert_eq!(r.child_text("b"), Some("t<1"));
        assert!(r.child("c").is_some());
        assert_eq!(r.child_text("d"), Some("<raw>"));
    }

    #[test]
    fn entity_bomb_is_rejected() {
        let x = "<?xml version=\"1.0\"?><!DOCTYPE l [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]><l>&b;</l>";
        assert!(parse(x).is_err());
    }

    #[test]
    fn external_entity_is_rejected() {
        let x = "<!DOCTYPE f [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><f>&x;</f>";
        assert!(parse(x).is_err());
    }

    #[test]
    fn numeric_refs_and_errors() {
        assert_eq!(parse("<a>&#65;&#x42;</a>").unwrap().text, "AB");
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a>").is_err());
    }

    #[test]
    fn depth_limit() {
        let deep = "<a>".repeat(200) + &"</a>".repeat(200);
        assert!(parse(&deep).is_err());
    }
}
