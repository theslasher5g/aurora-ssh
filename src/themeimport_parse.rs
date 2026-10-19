//! Reads terminal colour schemes from the formats people share: iTerm2 `.itermcolors`
//! (plist XML), Windows Terminal JSON, base16 (YAML and JSON), Alacritty TOML and
//! Kitty/Ghostty style `key value` files. Tolerant (comments, trailing commas, odd spacing),
//! size limited, no external parser crates and no unsafe code.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Largest input accepted (bytes).
pub const MAX_BYTES: usize = 256 * 1024;

pub type Rgb = [u8; 3];

/// A terminal colour scheme. This is also what is stored for imported themes.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Scheme {
    /// Stable id (`custom-xxxxxxxx`), assigned when the theme is saved.
    pub id: String,
    pub name: String,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Option<Rgb>,
    pub selection: Option<Rgb>,
    /// The 16 ANSI colours: 0-7 normal, 8-15 bright.
    pub ansi: [Rgb; 16],
}

impl Default for Scheme {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            fg: [0xe5, 0xe5, 0xe5],
            bg: [0x10, 0x10, 0x10],
            cursor: None,
            selection: None,
            ansi: XTERM,
        }
    }
}

/// Fallback for ANSI colours a file does not define.
pub const XTERM: [Rgb; 16] = [
    [0x00, 0x00, 0x00],
    [0xcd, 0x00, 0x00],
    [0x00, 0xcd, 0x00],
    [0xcd, 0xcd, 0x00],
    [0x00, 0x00, 0xee],
    [0xcd, 0x00, 0xcd],
    [0x00, 0xcd, 0xcd],
    [0xe5, 0xe5, 0xe5],
    [0x7f, 0x7f, 0x7f],
    [0xff, 0x00, 0x00],
    [0x00, 0xff, 0x00],
    [0xff, 0xff, 0x00],
    [0x5c, 0x5c, 0xff],
    [0xff, 0x00, 0xff],
    [0x00, 0xff, 0xff],
    [0xff, 0xff, 0xff],
];

/// Names of the ANSI slots in a Windows Terminal scheme, in order.
const WT_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "purple",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightPurple",
    "brightCyan",
    "brightWhite",
];

/// `#rrggbb`, `rrggbb`, `0xrrggbb`, `#rgb`, `rgb:rr/gg/bb`, with optional quotes.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_matches(['"', '\'']).trim();
    if let Some(rest) = s.strip_prefix("rgb:") {
        let mut it = rest.split('/');
        let mut c = [0u8; 3];
        for slot in &mut c {
            let p = it.next()?;
            if p.is_empty() || p.len() > 4 {
                return None;
            }
            let v = u32::from_str_radix(p, 16).ok()?;
            let max = (1u32 << (4 * p.len())) - 1;
            *slot = ((v * 255 + max / 2) / max) as u8;
        }
        return it.next().is_none().then_some(c);
    }
    let h = s
        .strip_prefix('#')
        .or_else(|| s.strip_prefix("0x"))
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 | 8 => {
            let v = u32::from_str_radix(&h[..6], 16).ok()?;
            Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
        }
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).map(|v| v * 17);
            Some([d(0).ok()?, d(1).ok()?, d(2).ok()?])
        }
        _ => None,
    }
}

/// `#rrggbb`
pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Reads schemes from any supported format. `hint` (a file name or empty) names schemes
/// that carry no name themselves.
pub fn parse_any(text: &str, hint: &str) -> Result<Vec<Scheme>, String> {
    if text.len() > MAX_BYTES {
        return Err(t!("Die Datei ist zu groß (höchstens 256 KB).").to_string());
    }
    let text = text.trim_start_matches('\u{feff}');
    let t = text.trim_start();
    if t.is_empty() {
        return Err(t!("Nichts zum Importieren – die Eingabe ist leer.").to_string());
    }
    let stem = file_stem(hint);
    let mut out = if t.starts_with("<?xml") || t.starts_with("<plist") || t.starts_with("<!DOCTYPE")
    {
        vec![parse_iterm(t, &stem)?]
    } else if looks_like_json(t) {
        parse_json(t, &stem)?
    } else if t.contains("[colors") || t.contains("colors.primary") {
        vec![parse_alacritty(t, &stem)?]
    } else if has_base16(t) {
        vec![parse_base16_yaml(t, &stem)?]
    } else {
        vec![parse_keyvalue(t, &stem)?]
    };
    for s in &mut out {
        if s.name.trim().is_empty() {
            s.name = if stem.is_empty() {
                t!("Importiertes Theme").to_string()
            } else {
                stem.clone()
            };
        }
        s.name = s.name.trim().chars().take(60).collect();
    }
    Ok(out)
}

/// `{…}` or an array of objects; a leading `[colors.primary]` is TOML.
fn looks_like_json(t: &str) -> bool {
    t.starts_with('{')
        || t.strip_prefix('[').is_some_and(|r| {
            matches!(
                r.trim_start().chars().next(),
                Some('{' | '[' | ']' | '"' | '0'..='9')
            )
        })
}

fn file_stem(hint: &str) -> String {
    let name = hint.rsplit(['/', '\\']).next().unwrap_or(hint);
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    stem.trim().to_string()
}

fn has_base16(t: &str) -> bool {
    t.lines().any(|l| {
        let l = l.trim_start();
        l.get(..6).is_some_and(|k| k.eq_ignore_ascii_case("base00"))
            && l.get(6..).is_some_and(|r| !r.trim_start().is_empty())
    })
}

/// Collects what a format provides and builds the scheme.
#[derive(Default)]
struct Collect {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    cursor: Option<Rgb>,
    selection: Option<Rgb>,
    ansi: [Option<Rgb>; 16],
    name: String,
}

impl Collect {
    fn finish(self) -> Result<Scheme, String> {
        let bg = self
            .bg
            .ok_or_else(|| t!("Keine Hintergrundfarbe gefunden.").to_string())?;
        let fg = self
            .fg
            .ok_or_else(|| t!("Keine Vordergrundfarbe gefunden.").to_string())?;
        let found = self.ansi.iter().filter(|c| c.is_some()).count();
        if found < 8 {
            return Err(t!(
                "Nur {} von 16 Terminalfarben gefunden – das sieht nicht nach einem Farbschema aus.",
                found
            )
            .to_string());
        }
        let mut ansi = XTERM;
        for i in 0..16 {
            ansi[i] = match self.ansi[i] {
                Some(c) => c,
                // A missing bright colour is the normal one.
                None if i >= 8 && self.ansi[i - 8].is_some() => {
                    self.ansi[i - 8].unwrap_or(XTERM[i])
                }
                None => XTERM[i],
            };
        }
        Ok(Scheme {
            id: String::new(),
            name: self.name,
            fg,
            bg,
            cursor: self.cursor,
            selection: self.selection,
            ansi,
        })
    }
}

// ---------------------------------------------------------------- iTerm2 (plist XML)

#[derive(Debug, PartialEq)]
enum Tok {
    Open(String),
    Close(String),
    Text(String),
}

fn xml_tokens(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        let text = rest[..i].trim();
        if !text.is_empty() {
            out.push(Tok::Text(xml_unescape(text)));
        }
        rest = &rest[i..];
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map(|e| &rest[e + 3..]).unwrap_or("");
            continue;
        }
        let Some(end) = rest.find('>') else {
            break;
        };
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            out.push(Tok::Close(name.trim().to_string()));
        } else {
            let selfclosing = inner.ends_with('/');
            let name = inner
                .trim_end_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            out.push(Tok::Open(name.clone()));
            if selfclosing {
                out.push(Tok::Close(name));
            }
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// `key -> number` pairs of one `<dict>`; the iterator sits after its `</dict>`.
fn plist_color(toks: &mut std::iter::Peekable<std::slice::Iter<'_, Tok>>) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    let mut key = None;
    let mut depth = 1;
    while let Some(t) = toks.next() {
        match t {
            Tok::Open(n) if n == "dict" => depth += 1,
            Tok::Close(n) if n == "dict" => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Tok::Open(n) if n == "key" => {
                if let Some(Tok::Text(k)) = toks.peek() {
                    key = Some(k.clone());
                }
            }
            Tok::Open(n) if n == "real" || n == "integer" => {
                if let (Some(Tok::Text(v)), Some(k)) = (toks.peek(), key.take())
                    && let Ok(v) = v.trim().parse::<f64>()
                {
                    m.insert(k, v);
                }
            }
            _ => {}
        }
    }
    m
}

fn plist_rgb(m: &HashMap<String, f64>) -> Option<Rgb> {
    let c = |k: &str| m.get(k).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Some([
        c("Red Component")?,
        c("Green Component")?,
        c("Blue Component")?,
    ])
}

fn parse_iterm(text: &str, stem: &str) -> Result<Scheme, String> {
    let toks = xml_tokens(text);
    let mut colors: HashMap<String, Rgb> = HashMap::new();
    let mut it = toks.iter().peekable();
    let mut key: Option<String> = None;
    while let Some(t) = it.next() {
        match t {
            Tok::Open(n) if n == "key" => {
                if let Some(Tok::Text(k)) = it.peek() {
                    key = Some(k.clone());
                }
            }
            Tok::Open(n) if n == "dict" => {
                // The root dict is the first one (no key before it).
                if let Some(k) = key.take() {
                    let m = plist_color(&mut it);
                    if let Some(c) = plist_rgb(&m) {
                        colors.insert(k, c);
                    }
                }
            }
            Tok::Open(n) if n != "key" && n != "plist" => {
                key = None;
            }
            _ => {}
        }
    }
    if colors.is_empty() {
        return Err(t!("In der Datei stehen keine iTerm-Farben.").to_string());
    }
    // Newer files carry "(Dark)" / "(Light)" variants next to the plain names.
    let get = |name: &str| {
        colors
            .get(name)
            .or_else(|| colors.get(&format!("{name} (Dark)")))
            .or_else(|| colors.get(&format!("{name} (Light)")))
            .copied()
    };
    let mut c = Collect {
        fg: get("Foreground Color"),
        bg: get("Background Color"),
        cursor: get("Cursor Color"),
        selection: get("Selection Color"),
        name: stem.to_string(),
        ..Default::default()
    };
    for i in 0..16 {
        c.ansi[i] = get(&format!("Ansi {i} Color"));
    }
    c.finish()
}

// ---------------------------------------------------------------- JSON (Windows Terminal, base16)

/// Removes `//` and `/* */` comments and trailing commas (Windows Terminal's settings.json
/// allows both), leaving strings alone.
fn json_clean(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == ',' {
            // Trailing comma: next non-space char closes the container.
            let mut j = i + 1;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if !(j < b.len() && (b[j] == '}' || b[j] == ']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn parse_json(text: &str, stem: &str) -> Result<Vec<Scheme>, String> {
    let v: serde_json::Value = serde_json::from_str(&json_clean(text))
        .map_err(|e| t!("Das ist kein gültiges JSON: {}", e).to_string())?;
    let mut out = Vec::new();
    match &v {
        serde_json::Value::Array(a) => {
            for x in a {
                out.push(json_scheme(x, stem)?);
            }
        }
        serde_json::Value::Object(o) => {
            if let Some(serde_json::Value::Array(a)) = o.get("schemes") {
                for x in a {
                    out.push(json_scheme(x, stem)?);
                }
            } else {
                out.push(json_scheme(&v, stem)?);
            }
        }
        _ => return Err(t!("Unbekanntes JSON-Format.").to_string()),
    }
    if out.is_empty() {
        return Err(t!("Die Datei enthält keine Farbschemas.").to_string());
    }
    Ok(out)
}

fn json_scheme(v: &serde_json::Value, stem: &str) -> Result<Scheme, String> {
    let o = v
        .as_object()
        .ok_or_else(|| t!("Unbekanntes JSON-Format.").to_string())?;
    let s = |k: &str| o.get(k).and_then(|x| x.as_str()).and_then(parse_hex);
    let name = o
        .get("name")
        .or_else(|| o.get("scheme"))
        .or_else(|| o.get("title"))
        .and_then(|x| x.as_str())
        .unwrap_or(stem)
        .to_string();
    // base16 in JSON: flat, or under "palette".
    let flat = o.contains_key("base00");
    let nested = o.get("palette").and_then(|p| p.as_object());
    if flat || nested.is_some_and(|p| p.contains_key("base00")) {
        let src = if flat { o } else { nested.unwrap_or(o) };
        let mut m = HashMap::new();
        for (k, val) in src {
            if let (Some(i), Some(c)) = (base_index(k), val.as_str().and_then(parse_hex)) {
                m.insert(i, c);
            }
        }
        return base16_scheme(&m, name);
    }
    let mut c = Collect {
        fg: s("foreground"),
        bg: s("background"),
        cursor: s("cursorColor"),
        selection: s("selectionBackground"),
        name,
        ..Default::default()
    };
    for (i, n) in WT_NAMES.iter().enumerate() {
        c.ansi[i] = s(n);
    }
    c.finish()
}

// ---------------------------------------------------------------- base16

fn base_index(key: &str) -> Option<usize> {
    let k = key.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
    let h = k.strip_prefix("base0")?;
    if h.len() != 1 {
        return None;
    }
    usize::from_str_radix(h, 16).ok()
}

fn base16_scheme(m: &HashMap<usize, Rgb>, name: String) -> Result<Scheme, String> {
    let g = |i: usize| m.get(&i).copied();
    let need =
        |i: usize| g(i).ok_or_else(|| t!("base0{} fehlt im Schema.", format!("{i:X}")).to_string());
    // The base16-shell convention.
    let order = [0x00, 0x08, 0x0b, 0x0a, 0x0d, 0x0e, 0x0c, 0x05];
    let mut ansi = XTERM;
    for (i, b) in order.iter().enumerate() {
        ansi[i] = need(*b)?;
    }
    ansi[8] = need(0x03)?;
    for i in 1..7 {
        ansi[8 + i] = ansi[i];
    }
    ansi[15] = need(0x07)?;
    Ok(Scheme {
        id: String::new(),
        name,
        fg: need(0x05)?,
        bg: need(0x00)?,
        cursor: g(0x05),
        selection: g(0x02),
        ansi,
    })
}

fn yaml_value(v: &str) -> &str {
    let v = v.trim();
    if let Some(rest) = v.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("");
    }
    if let Some(rest) = v.strip_prefix('\'') {
        return rest.split('\'').next().unwrap_or("");
    }
    v.split(" #").next().unwrap_or(v).trim()
}

fn parse_base16_yaml(text: &str, stem: &str) -> Result<Scheme, String> {
    let mut m = HashMap::new();
    let mut name = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), yaml_value(v));
        if let Some(i) = base_index(k) {
            if let Some(c) = parse_hex(v) {
                m.insert(i, c);
            }
        } else if matches!(k, "scheme" | "name") && name.is_empty() && !v.is_empty() {
            name = v.to_string();
        }
    }
    base16_scheme(
        &m,
        if name.is_empty() {
            stem.to_string()
        } else {
            name
        },
    )
}

// ---------------------------------------------------------------- Alacritty (TOML)

/// Flattens a TOML subset (tables, dotted keys, inline tables, strings) into
/// `table.key -> value`.
fn toml_flat(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut table = String::new();
    for raw in text.lines() {
        let line = strip_toml_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some(t) = line.strip_prefix('[') {
            table = t
                .trim_end_matches(']')
                .trim_start_matches('[')
                .trim()
                .to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().trim_matches('"');
        let prefix = if table.is_empty() {
            key.to_string()
        } else {
            format!("{table}.{key}")
        };
        let v = v.trim();
        if let Some(inner) = v.strip_prefix('{') {
            for part in inner.trim_end_matches('}').split(',') {
                if let Some((ik, iv)) = part.split_once('=') {
                    out.insert(
                        format!("{prefix}.{}", ik.trim().trim_matches('"')),
                        iv.trim().trim_matches(['"', '\'']).to_string(),
                    );
                }
            }
        } else {
            out.insert(prefix, v.trim_matches(['"', '\'']).to_string());
        }
    }
    out
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_str: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (in_str, c) {
            (None, '"' | '\'') => in_str = Some(c),
            (Some(q), c) if c == q => in_str = None,
            (None, '#') => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_alacritty(text: &str, stem: &str) -> Result<Scheme, String> {
    let m = toml_flat(text);
    let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let mut c = Collect {
        fg: g("colors.primary.foreground"),
        bg: g("colors.primary.background"),
        cursor: g("colors.cursor.cursor"),
        selection: g("colors.selection.background"),
        name: stem.to_string(),
        ..Default::default()
    };
    for (i, n) in names.iter().enumerate() {
        c.ansi[i] = g(&format!("colors.normal.{n}"));
        c.ansi[8 + i] = g(&format!("colors.bright.{n}"));
    }
    if let Some(first) = text.lines().next().and_then(|l| l.trim().strip_prefix('#'))
        && !first.trim().is_empty()
        && first.trim().len() < 50
        && !first.contains(':')
        && stem.is_empty()
    {
        c.name = first.trim().to_string();
    }
    c.finish()
}

// ---------------------------------------------------------------- Kitty / Ghostty (`key value`)

fn parse_keyvalue(text: &str, stem: &str) -> Result<Scheme, String> {
    let mut c = Collect {
        name: stem.to_string(),
        ..Default::default()
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `key = value` (Ghostty) or `key value` (Kitty).
        let (k, v) = match line.split_once('=') {
            Some((k, v)) if !k.trim().contains(char::is_whitespace) => (k.trim(), v.trim()),
            _ => match line.split_once(char::is_whitespace) {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            },
        };
        let k = k.to_ascii_lowercase().replace('-', "_");
        match k.as_str() {
            "foreground" => c.fg = parse_hex(v).or(c.fg),
            "background" => c.bg = parse_hex(v).or(c.bg),
            "cursor" | "cursor_color" => c.cursor = parse_hex(v).or(c.cursor),
            "selection_background" => c.selection = parse_hex(v).or(c.selection),
            "palette" => {
                // `palette = 3=#cdcd00`
                if let Some((n, col)) = v.split_once('=')
                    && let (Ok(n), Some(col)) = (n.trim().parse::<usize>(), parse_hex(col))
                    && n < 16
                {
                    c.ansi[n] = Some(col);
                }
            }
            _ => {
                if let Some(n) = k
                    .strip_prefix("color")
                    .and_then(|n| n.parse::<usize>().ok())
                    && n < 16
                    && let Some(col) = parse_hex(v)
                {
                    c.ansi[n] = Some(col);
                }
            }
        }
    }
    if c.bg.is_none() && c.fg.is_none() && c.ansi.iter().all(|x| x.is_none()) {
        return Err(t!("Das Format wurde nicht erkannt (iTerm, Windows Terminal, base16, Alacritty, Kitty/Ghostty).").to_string());
    }
    c.finish()
}

// ---------------------------------------------------------------- export

/// Windows Terminal scheme JSON of a scheme.
pub fn to_windows_terminal(s: &Scheme) -> String {
    let mut o = serde_json::Map::new();
    let mut put = |k: &str, v: String| {
        o.insert(k.to_string(), serde_json::Value::String(v));
    };
    put("name", s.name.clone());
    put("background", hex(s.bg));
    put("foreground", hex(s.fg));
    if let Some(c) = s.cursor {
        put("cursorColor", hex(c));
    }
    if let Some(c) = s.selection {
        put("selectionBackground", hex(c));
    }
    for (i, n) in WT_NAMES.iter().enumerate() {
        put(n, hex(s.ansi[i]));
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(o)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRACULA: [&str; 16] = [
        "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
        "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
    ];

    fn rgb(h: &str) -> Rgb {
        parse_hex(h).unwrap()
    }

    #[test]
    fn hex_forms() {
        assert_eq!(parse_hex("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("0xff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("\"#f80\""), Some([255, 136, 0]));
        assert_eq!(parse_hex("#ff8000cc"), Some([255, 128, 0]));
        assert_eq!(parse_hex("rgb:ff/80/00"), Some([255, 128, 0]));
        assert_eq!(parse_hex("rgb:f/8/0"), Some([255, 136, 0]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("red"), None);
        assert_eq!(parse_hex(""), None);
    }

    /// An iTerm2 file as the app writes it (alphabetical component order, `Color Space`).
    fn itermcolors() -> String {
        let entry = |name: &str, c: Rgb| {
            format!(
                "\t<key>{name}</key>\n\t<dict>\n\t\t<key>Alpha Component</key>\n\t\t<real>1</real>\n\t\t<key>Blue Component</key>\n\t\t<real>{}</real>\n\t\t<key>Color Space</key>\n\t\t<string>sRGB</string>\n\t\t<key>Green Component</key>\n\t\t<real>{}</real>\n\t\t<key>Red Component</key>\n\t\t<real>{}</real>\n\t</dict>\n",
                c[2] as f64 / 255.0,
                c[1] as f64 / 255.0,
                c[0] as f64 / 255.0
            )
        };
        let mut s = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
        );
        for (i, h) in DRACULA.iter().enumerate() {
            s += &entry(&format!("Ansi {i} Color"), rgb(h));
        }
        s += &entry("Background Color", rgb("#282a36"));
        s += &entry("Foreground Color", rgb("#f8f8f2"));
        s += &entry("Cursor Color", rgb("#f8f8f0"));
        s += &entry("Selection Color", rgb("#44475a"));
        s += "\t<key>Badge Color</key>\n\t<dict>\n\t\t<key>Red Component</key>\n\t\t<real>1</real>\n\t</dict>\n\t<key>Bold Color</key>\n\t<dict>\n\t\t<key>Blue Component</key>\n\t\t<real>0.5</real>\n\t</dict>\n</dict>\n</plist>\n";
        s
    }

    #[test]
    fn iterm_plist() {
        let r = parse_any(&itermcolors(), "/home/u/Dracula.itermcolors").unwrap();
        assert_eq!(r.len(), 1);
        let s = &r[0];
        assert_eq!(s.name, "Dracula");
        assert_eq!(s.bg, rgb("#282a36"));
        assert_eq!(s.fg, rgb("#f8f8f2"));
        assert_eq!(s.cursor, Some(rgb("#f8f8f0")));
        assert_eq!(s.selection, Some(rgb("#44475a")));
        for (i, h) in DRACULA.iter().enumerate() {
            assert_eq!(s.ansi[i], rgb(h), "ansi {i}");
        }
    }

    #[test]
    fn iterm_real_snippet_with_long_reals_and_integers() {
        let mut s = itermcolors();
        // iTerm writes numbers like this; a plain integer component is legal too.
        s = s.replace(
            "<key>Foreground Color</key>",
            "<key>Foreground Color</key><!-- c -->",
        );
        let snippet = "<key>Ansi 1 Color (Dark)</key><dict><key>Red Component</key><real>0.80392156862745101</real><key>Green Component</key><integer>0</integer><key>Blue Component</key><real>4.3e-1</real></dict>";
        s = s.replace("</dict>\n</plist>", &format!("{snippet}</dict>\n</plist>"));
        let r = parse_any(&s, "x.itermcolors").unwrap();
        // The plain "Ansi 1 Color" wins over the "(Dark)" variant.
        assert_eq!(r[0].ansi[1], rgb("#ff5555"));
    }

    #[test]
    fn iterm_garbage_is_an_error() {
        assert!(parse_any("<plist><dict></dict></plist>", "").is_err());
    }

    const WT: &str = r##"{
        // Campbell, the default
        "name": "Campbell",
        "cursorColor": "#FFFFFF",
        "selectionBackground": "#FFFFFF",
        "background": "#0C0C0C",
        "foreground": "#CCCCCC",
        "black": "#0C0C0C",
        "blue": "#0037DA",
        "cyan": "#3A96DD",
        "green": "#13A10E",
        "purple": "#881798",
        "red": "#C50F1F",
        "white": "#CCCCCC",
        "yellow": "#C19C00",
        "brightBlack": "#767676",
        "brightBlue": "#3B78FF",
        "brightCyan": "#61D6D6",
        "brightGreen": "#16C60C",
        "brightPurple": "#B4009E",
        "brightRed": "#E74856",
        "brightWhite": "#F2F2F2",
        "brightYellow": "#F9F1A5",
    }"##;

    #[test]
    fn windows_terminal_single_scheme_with_comments() {
        let r = parse_any(WT, "").unwrap();
        assert_eq!(r.len(), 1);
        let s = &r[0];
        assert_eq!(s.name, "Campbell");
        assert_eq!(s.bg, rgb("#0c0c0c"));
        assert_eq!(s.ansi[4], rgb("#0037da"));
        assert_eq!(s.ansi[13], rgb("#b4009e"));
        assert_eq!(s.cursor, Some([255, 255, 255]));
    }

    #[test]
    fn windows_terminal_settings_with_schemes_array() {
        let doc = format!(
            r#"{{ "$schema": "https://aka.ms/terminal-profiles-schema", "profiles": {{ "list": [] }}, "schemes": [ {WT}, {} ] }}"#,
            WT.replace("Campbell", "Second")
                .replace("#0C0C0C", "#101010")
        );
        let r = parse_any(&doc, "settings.json").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].name, "Second");
        assert_eq!(r[1].bg, rgb("#101010"));
    }

    #[test]
    fn windows_terminal_missing_bright_colours_fall_back() {
        let json = r##"{"name":"Mini","background":"#000000","foreground":"#ffffff",
          "black":"#000000","red":"#ff0000","green":"#00ff00","yellow":"#ffff00",
          "blue":"#0000ff","purple":"#ff00ff","cyan":"#00ffff","white":"#ffffff"}"##;
        let s = &parse_any(json, "").unwrap()[0];
        assert_eq!(s.ansi[9], s.ansi[1]);
        assert!(s.cursor.is_none());
    }

    const BASE16_YAML: &str = r##"# Base16 Solarized Dark
scheme: "Solarized Dark"
author: "Ethan Schoonover (modified by aramisgithub)"
base00: "002b36"
base01: "073642"
base02: "586e75"
base03: "657b83"
base04: "839496"
base05: "93a1a1"
base06: "eee8d5"
base07: "fdf6e3"
base08: "dc322f"
base09: "cb4b16"
base0A: "b58900"
base0B: "859900"
base0C: "2aa198"
base0D: "268bd2"
base0E: "6c71c4"
base0F: "d33682"
"##;

    #[test]
    fn base16_yaml_old_style() {
        let s = &parse_any(BASE16_YAML, "").unwrap()[0];
        assert_eq!(s.name, "Solarized Dark");
        assert_eq!(s.bg, rgb("#002b36"));
        assert_eq!(s.fg, rgb("#93a1a1"));
        assert_eq!(s.ansi[1], rgb("#dc322f"));
        assert_eq!(s.ansi[2], rgb("#859900"));
        assert_eq!(s.ansi[4], rgb("#268bd2"));
        assert_eq!(s.ansi[8], rgb("#657b83"));
        assert_eq!(s.ansi[15], rgb("#fdf6e3"));
        assert_eq!(s.selection, Some(rgb("#586e75")));
    }

    #[test]
    fn base16_yaml_tinted_theming_style() {
        let y = r##"system: "base16"
name: "Gruvbox dark, hard"
author: "Dawid Kurek"
variant: "dark"
palette:
  base00: "#1d2021" # bg
  base01: "#3c3836"
  base02: "#504945"
  base03: "#665c54"
  base04: "#bdae93"
  base05: "#d5c4a1"
  base06: "#ebdbb2"
  base07: "#fbf1c7"
  base08: "#fb4934"
  base09: "#fe8019"
  base0A: "#fabd2f"
  base0B: "#b8bb26"
  base0C: "#8ec07c"
  base0D: "#83a598"
  base0E: "#d3869b"
  base0F: "#d65d0e"
"##;
        let s = &parse_any(y, "").unwrap()[0];
        assert_eq!(s.name, "Gruvbox dark, hard");
        assert_eq!(s.bg, rgb("#1d2021"));
        assert_eq!(s.ansi[3], rgb("#fabd2f"));
    }

    #[test]
    fn base16_json() {
        let j = r##"{"scheme":"Tomorrow Night","author":"Chris Kempson",
          "base00":"1d1f21","base01":"282a2e","base02":"373b41","base03":"969896",
          "base04":"b4b7b4","base05":"c5c8c6","base06":"e0e0e0","base07":"ffffff",
          "base08":"cc6666","base09":"de935f","base0A":"f0c674","base0B":"b5bd68",
          "base0C":"8abeb7","base0D":"81a2be","base0E":"b294bb","base0F":"a3685a"}"##;
        let s = &parse_any(j, "").unwrap()[0];
        assert_eq!(s.name, "Tomorrow Night");
        assert_eq!(s.ansi[6], rgb("#8abeb7"));
        assert_eq!(s.fg, rgb("#c5c8c6"));
    }

    #[test]
    fn base16_incomplete_is_an_error() {
        let e = parse_any("base00: \"000000\"\nbase01: \"111111\"\n", "").unwrap_err();
        assert!(e.contains("base"), "{e}");
    }

    const ALACRITTY: &str = r##"# Colors (Tokyo Night)
[colors.primary]
background = '#1a1b26'
foreground = '#c0caf5'

[colors.cursor]
text = '#1a1b26'
cursor = '#c0caf5'

[colors.selection]
background = '#33467c'

[colors.normal]
black   = '#15161e'
red     = '#f7768e'
green   = '#9ece6a'
yellow  = '#e0af68'
blue    = '#7aa2f7'
magenta = '#bb9af7'
cyan    = '#7dcfff'
white   = '#a9b1d6'

[colors.bright]
black   = '#414868'
red     = '#f7768e'
green   = '#9ece6a'
yellow  = '#e0af68'
blue    = '#7aa2f7'
magenta = '#bb9af7'
cyan    = '#7dcfff'
white   = '#c0caf5'
"##;

    #[test]
    fn alacritty_toml() {
        let s = &parse_any(ALACRITTY, "tokyo-night.toml").unwrap()[0];
        assert_eq!(s.name, "tokyo-night");
        assert_eq!(s.bg, rgb("#1a1b26"));
        assert_eq!(s.cursor, Some(rgb("#c0caf5")));
        assert_eq!(s.selection, Some(rgb("#33467c")));
        assert_eq!(s.ansi[0], rgb("#15161e"));
        assert_eq!(s.ansi[8], rgb("#414868"));
        assert_eq!(s.ansi[15], rgb("#c0caf5"));
    }

    #[test]
    fn alacritty_inline_tables_and_0x_colours() {
        let t = r#"[colors]
primary = { background = "0x000000", foreground = "0xeaeaea" } # comment
normal = { black = "0x000000", red = "0xd54e53", green = "0xb9ca4a", yellow = "0xe6c547", blue = "0x7aa6da", magenta = "0xc397d8", cyan = "0x70c0ba", white = "0xeaeaea" }
"#;
        let s = &parse_any(t, "").unwrap()[0];
        assert_eq!(s.bg, [0, 0, 0]);
        assert_eq!(s.ansi[1], rgb("#d54e53"));
        // No bright table: bright = normal.
        assert_eq!(s.ansi[9], s.ansi[1]);
    }

    const KITTY: &str = "# vim:ft=kitty
## name: Nord
foreground            #d8dee9
background            #2e3440
selection_foreground  #000000
selection_background  #81a1c1
cursor                #d8dee9
color0   #3b4252
color1   #bf616a
color2   #a3be8c
color3   #ebcb8b
color4   #81a1c1
color5   #b48ead
color6   #88c0d0
color7   #e5e9f0
color8   #4c566a
color9   #bf616a
color10  #a3be8c
color11  #ebcb8b
color12  #81a1c1
color13  #b48ead
color14  #8fbcbb
color15  #eceff4
";

    #[test]
    fn kitty_conf() {
        let s = &parse_any(KITTY, "nord.conf").unwrap()[0];
        assert_eq!(s.bg, rgb("#2e3440"));
        assert_eq!(s.selection, Some(rgb("#81a1c1")));
        assert_eq!(s.ansi[14], rgb("#8fbcbb"));
        assert_eq!(s.name, "nord");
    }

    #[test]
    fn ghostty_conf() {
        let g = "# Ghostty theme
background = 282c34
foreground = ffffff
cursor-color = ffcc00
selection-background = 3e4451
palette = 0=#1d1f21
palette = 1=#cc6666
palette = 2=#b5bd68
palette = 3=#f0c674
palette = 4=#81a2be
palette = 5=#b294bb
palette = 6=#8abeb7
palette = 7=#c5c8c6
palette = 8=#969896
palette = 9=#cc6666
";
        let s = &parse_any(g, "mytheme").unwrap()[0];
        assert_eq!(s.bg, rgb("#282c34"));
        assert_eq!(s.cursor, Some(rgb("#ffcc00")));
        assert_eq!(s.ansi[7], rgb("#c5c8c6"));
        assert_eq!(s.ansi[9], rgb("#cc6666"));
        assert_eq!(s.ansi[10], s.ansi[2]);
    }

    #[test]
    fn limits_and_garbage() {
        assert!(parse_any("", "").is_err());
        assert!(parse_any("   \n ", "").is_err());
        assert!(parse_any("hello world", "").is_err());
        assert!(parse_any("{not json", "").is_err());
        assert!(parse_any("[1,2,3]", "").is_err());
        let big = format!("{{\"name\":\"{}\"}}", "a".repeat(MAX_BYTES));
        assert!(parse_any(&big, "").unwrap_err().contains("256"));
        // Fg/bg only: not a scheme.
        assert!(parse_any("foreground #fff\nbackground #000\n", "").is_err());
    }

    #[test]
    fn bom_and_crlf_are_fine() {
        let s = format!("\u{feff}{}", KITTY.replace('\n', "\r\n"));
        assert!(parse_any(&s, "").is_ok());
    }

    #[test]
    fn export_roundtrips_through_windows_terminal_json() {
        let a = parse_any(ALACRITTY, "Tokyo").unwrap().remove(0);
        let json = to_windows_terminal(&a);
        let b = parse_any(&json, "").unwrap().remove(0);
        assert_eq!(a, b);
    }
}
