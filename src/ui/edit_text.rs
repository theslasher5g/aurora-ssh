//! Pure text helpers of the built-in editor: search and replace, indentation, line
//! numbers, path resolution. Offsets are bytes unless a name says `char`.

use std::ops::Range;

use super::edit_syntax::Lang;

/// Fast non-cryptographic hash of the text (change detection, layout cache).
pub fn fast_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let b = s.as_bytes();
    let mut chunks = b.chunks_exact(8);
    for c in &mut chunks {
        let v = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        h ^= h >> 29;
    }
    for &x in chunks.remainder() {
        h = (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (b.len() as u64).rotate_left(32)
}

pub fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

pub fn byte_to_char(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().count()
}

// ---------------------------------------------------------------------------- search

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// All non-overlapping matches of `query` (byte ranges), at most `cap`.
pub fn find_all(text: &str, query: &str, case_sensitive: bool, cap: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    if case_sensitive {
        let mut from = 0;
        while let Some(p) = text[from..].find(query) {
            let s = from + p;
            out.push(s..s + query.len());
            from = s + query.len();
            if out.len() >= cap {
                break;
            }
        }
        return out;
    }
    let q: Vec<char> = query.chars().map(fold).collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i + q.len() <= chars.len() {
        if fold(chars[i].1) == q[0] && (1..q.len()).all(|k| fold(chars[i + k].1) == q[k]) {
            let start = chars[i].0;
            let end = chars.get(i + q.len()).map(|c| c.0).unwrap_or(text.len());
            out.push(start..end);
            i += q.len();
            if out.len() >= cap {
                break;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Index of the match to jump to from the byte offset `from`: the first one starting at or
/// after it (forward) or the last one ending at or before it (backward), wrapping around.
pub fn pick_match(matches: &[Range<usize>], from: usize, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if forward {
        matches.iter().position(|m| m.start >= from).or(Some(0))
    } else {
        matches
            .iter()
            .rposition(|m| m.end <= from)
            .or(Some(matches.len() - 1))
    }
}

/// Replaces every match; returns the new text and the number of replacements.
pub fn replace_all(text: &str, query: &str, with: &str, case_sensitive: bool) -> (String, usize) {
    let ms = find_all(text, query, case_sensitive, usize::MAX);
    if ms.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in &ms {
        out.push_str(&text[last..m.start]);
        out.push_str(with);
        last = m.end;
    }
    out.push_str(&text[last..]);
    (out, ms.len())
}

// ---------------------------------------------------------------------------- lines

/// 1-based line and column (in characters) of a byte offset.
pub fn line_col(text: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(text.len());
    let before = &text[..byte];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    (line, before[start..].chars().count() + 1)
}

/// Byte offset of the start of the 1-based `line` (clamped to the last line).
pub fn line_start(text: &str, line: usize) -> usize {
    let mut start = 0;
    for _ in 1..line.max(1) {
        match text[start..].find('\n') {
            Some(p) => start += p + 1,
            None => break,
        }
    }
    start
}

pub fn line_count(text: &str) -> usize {
    text.matches('\n').count() + 1
}

/// "42" or "42:7" → (line, column).
pub fn parse_goto(input: &str) -> Option<(usize, usize)> {
    let t = input.trim();
    let (l, c) = match t.split_once([':', ',', ' ']) {
        Some((l, c)) => (l.trim(), c.trim()),
        None => (t, ""),
    };
    let line: usize = l.parse().ok()?;
    let col: usize = if c.is_empty() { 1 } else { c.parse().ok()? };
    (line >= 1).then_some((line, col.max(1)))
}

/// Byte offset for a 1-based line and column.
pub fn goto_offset(text: &str, line: usize, col: usize) -> usize {
    let start = line_start(text, line);
    let end = text[start..]
        .find('\n')
        .map(|p| start + p)
        .unwrap_or(text.len());
    let line_text = &text[start..end];
    start + char_to_byte(line_text, col.saturating_sub(1))
}

// ---------------------------------------------------------------------------- indentation

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Indent {
    Tabs,
    Spaces(usize),
}

impl Indent {
    pub fn unit(self) -> String {
        match self {
            Indent::Tabs => "\t".into(),
            Indent::Spaces(n) => " ".repeat(n),
        }
    }

    pub fn label(self) -> String {
        match self {
            Indent::Tabs => t!("Tabs").into(),
            Indent::Spaces(n) => t!("{} Leerzeichen", n),
        }
    }
}

/// Guesses the indentation style of a file from its indented lines.
pub fn detect_indent(text: &str, lang: Lang, name: &str) -> Indent {
    if name.eq_ignore_ascii_case("makefile") || name.ends_with(".mk") {
        return Indent::Tabs;
    }
    let (mut tabs, mut spaces) = (0usize, 0usize);
    let mut steps = [0usize; 9];
    let mut prev = 0usize;
    for line in text.lines().take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with("  ") {
            spaces += 1;
            let n = line.len() - line.trim_start_matches(' ').len();
            if n > prev && n - prev <= 8 {
                steps[n - prev] += 1;
            } else if prev == 0 && n <= 8 {
                steps[n] += 1;
            }
        }
        prev = if line.starts_with('\t') {
            0
        } else {
            line.len() - line.trim_start_matches(' ').len()
        };
    }
    if tabs > spaces {
        return Indent::Tabs;
    }
    let best = (2..=8)
        .max_by_key(|n| (steps[*n], std::cmp::Reverse(*n)))
        .filter(|n| steps[*n] > 0);
    match best {
        Some(n) => Indent::Spaces(n),
        None => match lang {
            Lang::Yaml | Lang::Json | Lang::JavaScript | Lang::Html => Indent::Spaces(2),
            _ => Indent::Spaces(4),
        },
    }
}

/// Result of an edit on the text: new content and selection (char indices).
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub sel: (usize, usize),
}

/// Tab key: inserts the indent at a plain cursor (to the next tab stop), or indents every
/// line touched by the selection.
pub fn tab_key(text: &str, sel: (usize, usize), indent: Indent) -> Edit {
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let (ba, bb) = (char_to_byte(text, a), char_to_byte(text, b));
    let multi = text[ba..bb].contains('\n');
    if a == b || (!multi && a != b && false) {
        // Plain cursor.
        let (_, col) = line_col(text, ba);
        let ins = match indent {
            Indent::Tabs => "\t".to_string(),
            Indent::Spaces(n) => " ".repeat(n - (col - 1) % n),
        };
        let mut out = String::with_capacity(text.len() + ins.len());
        out.push_str(&text[..ba]);
        out.push_str(&ins);
        out.push_str(&text[ba..]);
        let c = a + ins.chars().count();
        return Edit {
            text: out,
            sel: (c, c),
        };
    }
    if !multi {
        // A selection inside one line is replaced by the indent (like most editors).
        let ins = indent.unit();
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..ba]);
        out.push_str(&ins);
        out.push_str(&text[bb..]);
        let c = a + ins.chars().count();
        return Edit {
            text: out,
            sel: (c, c),
        };
    }
    shift_lines(text, (a, b), indent, true)
}

/// Shift+Tab: removes one indent level from every line touched by the selection.
pub fn backtab_key(text: &str, sel: (usize, usize), indent: Indent) -> Edit {
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    shift_lines(text, (a, b), indent, false)
}

fn shift_lines(text: &str, (a, b): (usize, usize), indent: Indent, add: bool) -> Edit {
    let (ba, bb) = (char_to_byte(text, a), char_to_byte(text, b));
    let first = text[..ba].rfind('\n').map(|p| p + 1).unwrap_or(0);
    // A selection ending at the very start of a line does not include that line.
    let end_probe = if bb > ba && text[..bb].ends_with('\n') {
        bb - 1
    } else {
        bb
    };
    let last_end = text[end_probe..]
        .find('\n')
        .map(|p| end_probe + p)
        .unwrap_or(text.len());
    let block = &text[first..last_end];
    let unit = indent.unit();
    let mut out_block = String::with_capacity(block.len() + 16);
    let (mut new_a, mut new_b) = (a as isize, b as isize);
    let mut pos = first;
    for (i, line) in block.split('\n').enumerate() {
        if i > 0 {
            out_block.push('\n');
        }
        let line_char_start = byte_to_char(text, pos);
        let delta: isize;
        if add {
            if line.is_empty() {
                out_block.push_str(line);
                delta = 0;
            } else {
                out_block.push_str(&unit);
                out_block.push_str(line);
                delta = unit.chars().count() as isize;
            }
        } else {
            let remove = match indent {
                _ if line.starts_with('\t') => 1,
                Indent::Tabs => 0,
                Indent::Spaces(n) => line.chars().take(n).take_while(|c| *c == ' ').count(),
            };
            out_block.push_str(&line[remove..]);
            delta = -(remove as isize);
        }
        // Adjust the selection ends that lie in or after this line.
        let line_len = line.chars().count() as isize;
        let ls = line_char_start as isize;
        let adjust = |p: isize| -> isize {
            if p < ls {
                0
            } else if p <= ls + line_len {
                if add { delta } else { delta.max(-(p - ls)) }
            } else {
                delta
            }
        };
        new_a += adjust(a as isize);
        new_b += adjust(b as isize);
        pos += line.len() + 1;
    }
    let mut out = String::with_capacity(text.len() + 16);
    out.push_str(&text[..first]);
    out.push_str(&out_block);
    out.push_str(&text[last_end..]);
    Edit {
        text: out,
        sel: (new_a.max(0) as usize, new_b.max(0) as usize),
    }
}

/// Enter key: the new line starts with the indentation of the current one (one level
/// deeper after an opening bracket, or a colon in Python/YAML).
pub fn newline_with_indent(line_before_cursor: &str, lang: Lang, indent: Indent) -> String {
    let ws: String = line_before_cursor
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let trimmed = line_before_cursor.trim_end();
    let deeper = trimmed.ends_with(['{', '[', '('])
        || (matches!(lang, Lang::Python | Lang::Yaml) && trimmed.ends_with(':'));
    let mut s = String::from("\n");
    s.push_str(&ws);
    if deeper {
        s.push_str(&indent.unit());
    }
    s
}

// ---------------------------------------------------------------------------- paths

/// The SFTP path for what the user typed in "Datei öffnen…". `cwd` is the terminal's folder
/// if known (absolute, or starting with `~`). Paths relative to the home folder stay
/// relative (SFTP resolves them against it).
pub fn resolve_remote_path(input: &str, cwd: Option<&str>) -> Option<String> {
    let p = input.trim();
    if p.is_empty() || p.contains('\0') {
        return None;
    }
    let p = crate::backend::sftp::normalize_remote(p);
    if p == "~" || p == "." {
        return None;
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return (!rest.is_empty()).then(|| rest.to_string());
    }
    if p.starts_with('/') {
        return Some(p);
    }
    // Relative to the terminal's folder.
    let rel = p.strip_prefix("./").unwrap_or(&p);
    match cwd.map(str::trim).filter(|c| !c.is_empty()) {
        Some("~") | None => Some(rel.to_string()),
        Some(c) => match c.strip_prefix("~/") {
            Some(home_rel) => Some(format!("{}/{rel}", home_rel.trim_end_matches('/'))),
            None => Some(crate::backend::sftp::join(c, rel)),
        },
    }
}

// ---------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_changes_with_content() {
        assert_eq!(fast_hash("abc"), fast_hash("abc"));
        assert_ne!(fast_hash("abc"), fast_hash("abd"));
        assert_ne!(fast_hash("abcdefgh1"), fast_hash("abcdefgh2"));
        assert_ne!(fast_hash(""), fast_hash(" "));
        assert_ne!(fast_hash("a"), fast_hash("a\0"));
    }

    #[test]
    fn find_case_modes() {
        let t = "Foo foo FOO fOo";
        assert_eq!(find_all(t, "foo", true, 100), vec![4..7]);
        assert_eq!(
            find_all(t, "foo", false, 100),
            vec![0..3, 4..7, 8..11, 12..15]
        );
        assert!(find_all(t, "", false, 10).is_empty());
        assert_eq!(find_all(t, "foo", false, 2).len(), 2);
        // Non-ASCII stays on character boundaries.
        let u = "Äpfel äpfel ÄPFEL";
        let m = find_all(u, "äpfel", false, 10);
        assert_eq!(m.len(), 3);
        for r in &m {
            assert!(u.is_char_boundary(r.start) && u.is_char_boundary(r.end));
        }
        assert_eq!(&u[m[2].clone()], "ÄPFEL");
    }

    #[test]
    fn next_and_previous_match_wrap() {
        let ms = vec![2..4, 10..12, 20..22];
        assert_eq!(pick_match(&ms, 0, true), Some(0));
        assert_eq!(pick_match(&ms, 4, true), Some(1));
        assert_eq!(pick_match(&ms, 22, true), Some(0), "wraps to the first");
        assert_eq!(pick_match(&ms, 20, false), Some(1));
        assert_eq!(pick_match(&ms, 12, false), Some(1));
        assert_eq!(pick_match(&ms, 3, false), Some(2), "wraps to the last");
        assert_eq!(pick_match(&[], 0, true), None);
    }

    #[test]
    fn replace_everything() {
        assert_eq!(replace_all("a-b-a", "a", "xx", true), ("xx-b-xx".into(), 2));
        assert_eq!(replace_all("A a", "a", "b", false), ("b b".into(), 2));
        assert_eq!(replace_all("A a", "a", "b", true), ("A b".into(), 1));
        assert_eq!(replace_all("abc", "x", "y", true), ("abc".into(), 0));
        assert_eq!(replace_all("aaa", "aa", "b", true), ("ba".into(), 1));
        assert_eq!(replace_all("a.b", ".", "", true), ("ab".into(), 1));
    }

    #[test]
    fn lines_and_columns() {
        let t = "one\ntwo üx\nthree";
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 4), (2, 1));
        assert_eq!(line_col(t, t.find('x').unwrap()), (2, 6));
        assert_eq!(line_col(t, t.len()), (3, 6));
        assert_eq!(line_start(t, 2), 4);
        assert_eq!(line_start(t, 99), 12);
        assert_eq!(line_count(t), 3);
        assert_eq!(goto_offset(t, 2, 5), 4 + "two ".len());
        assert_eq!(goto_offset(t, 2, 99), 4 + "two üx".len());
        assert_eq!(parse_goto("12"), Some((12, 1)));
        assert_eq!(parse_goto(" 3:7 "), Some((3, 7)));
        assert_eq!(parse_goto("0"), None);
        assert_eq!(parse_goto("x"), None);
        assert_eq!(parse_goto(""), None);
    }

    #[test]
    fn indent_detection() {
        assert_eq!(
            detect_indent("a\n\tb\n\tc\n", Lang::Plain, "x"),
            Indent::Tabs
        );
        assert_eq!(
            detect_indent("a:\n  b:\n    c: 1\n  d: 2\n", Lang::Yaml, "x.yml"),
            Indent::Spaces(2)
        );
        assert_eq!(
            detect_indent(
                "def f():\n    x = 1\n    if x:\n        y\n",
                Lang::Python,
                "x.py"
            ),
            Indent::Spaces(4)
        );
        assert_eq!(
            detect_indent("plain\n", Lang::Plain, "x"),
            Indent::Spaces(4)
        );
        assert_eq!(detect_indent("{}", Lang::Json, "x.json"), Indent::Spaces(2));
        assert_eq!(
            detect_indent("x\n    y\n", Lang::Plain, "Makefile"),
            Indent::Tabs
        );
    }

    #[test]
    fn tab_inserts_to_the_next_stop() {
        let e = tab_key("ab", (2, 2), Indent::Spaces(4));
        assert_eq!(
            e,
            Edit {
                text: "ab  ".into(),
                sel: (4, 4)
            }
        );
        let e = tab_key("", (0, 0), Indent::Spaces(4));
        assert_eq!(e.text, "    ");
        let e = tab_key("x", (1, 1), Indent::Tabs);
        assert_eq!(
            e,
            Edit {
                text: "x\t".into(),
                sel: (2, 2)
            }
        );
    }

    #[test]
    fn tab_indents_selected_lines() {
        let e = tab_key("a\nb\nc", (0, 5), Indent::Spaces(2));
        assert_eq!(e.text, "  a\n  b\n  c");
        assert_eq!(e.sel, (2, 11));
        // Empty lines stay empty; the line after the selection is untouched.
        let e = tab_key("a\n\nb\nc", (0, 4), Indent::Spaces(2));
        assert_eq!(e.text, "  a\n\n  b\nc");
        // Selection ending at the start of a line does not include it.
        let e = tab_key("a\nb\nc", (0, 2), Indent::Spaces(2));
        assert_eq!(e.text, "  a\nb\nc");
        // Single-line selection is replaced.
        let e = tab_key("hello", (1, 3), Indent::Spaces(2));
        assert_eq!(e.text, "h  lo");
    }

    #[test]
    fn backtab_dedents() {
        let e = backtab_key("    a\n  b\nc", (0, 12), Indent::Spaces(4));
        assert_eq!(e.text, "a\nb\nc");
        let e = backtab_key("\ta\n\t\tb", (0, 6), Indent::Spaces(4));
        assert_eq!(e.text, "a\n\tb");
        // Cursor only: current line, cursor moves left with the text.
        let e = backtab_key("    abc", (6, 6), Indent::Spaces(4));
        assert_eq!(e.text, "abc");
        assert_eq!(e.sel, (2, 2));
        // Nothing to remove.
        let e = backtab_key("abc", (1, 1), Indent::Spaces(4));
        assert_eq!(e.text, "abc");
        assert_eq!(e.sel, (1, 1));
    }

    #[test]
    fn enter_keeps_and_adds_indent() {
        let u = Indent::Spaces(4);
        assert_eq!(newline_with_indent("    foo", Lang::Plain, u), "\n    ");
        assert_eq!(newline_with_indent("fn x() {", Lang::Rust, u), "\n    ");
        assert_eq!(newline_with_indent("  if x:", Lang::Python, u), "\n      ");
        assert_eq!(newline_with_indent("  key:", Lang::Yaml, u), "\n      ");
        assert_eq!(newline_with_indent("Note:", Lang::Plain, u), "\n");
        assert_eq!(
            newline_with_indent("\tx", Lang::Plain, Indent::Tabs),
            "\n\t"
        );
    }

    #[test]
    fn open_path_resolution() {
        let r = resolve_remote_path;
        assert_eq!(r("/etc/hosts", None).as_deref(), Some("/etc/hosts"));
        assert_eq!(r("/etc/hosts", Some("/var")).as_deref(), Some("/etc/hosts"));
        assert_eq!(r("~/notes.txt", Some("/var")).as_deref(), Some("notes.txt"));
        assert_eq!(
            r("a.txt", Some("/var/www")).as_deref(),
            Some("/var/www/a.txt")
        );
        assert_eq!(
            r("./a.txt", Some("/var/www/")).as_deref(),
            Some("/var/www/a.txt")
        );
        assert_eq!(r("a.txt", Some("~/src")).as_deref(), Some("src/a.txt"));
        assert_eq!(r("a.txt", Some("~")).as_deref(), Some("a.txt"));
        assert_eq!(r("a.txt", None).as_deref(), Some("a.txt"));
        assert_eq!(r("  ", None), None);
        assert_eq!(r("~", None), None);
        assert_eq!(
            r("C:\\Users\\a.txt", None).as_deref(),
            Some("/C:/Users/a.txt")
        );
    }
}
