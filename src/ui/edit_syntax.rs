//! Lightweight syntax highlighting for the built-in editor: hand-written tokenizers that
//! turn text into coloured spans. They are forgiving by design (broken input never panics,
//! unknown syntax stays plain) and only split text at ASCII bytes, so every span lies on
//! a character boundary.

use egui::Color32;

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Plain,
    Shell,
    Python,
    Json,
    Yaml,
    Toml,
    Ini,
    /// Generic `key = value` / `key value` configuration.
    Conf,
    /// nginx / Apache style configuration (directives, blocks).
    Server,
    Markdown,
    Rust,
    JavaScript,
    Html,
    Dockerfile,
    Sql,
    Systemd,
    Log,
}

pub const ALL: &[Lang] = &[
    Lang::Plain,
    Lang::Shell,
    Lang::Python,
    Lang::Json,
    Lang::Yaml,
    Lang::Toml,
    Lang::Ini,
    Lang::Conf,
    Lang::Server,
    Lang::Markdown,
    Lang::Rust,
    Lang::JavaScript,
    Lang::Html,
    Lang::Dockerfile,
    Lang::Sql,
    Lang::Systemd,
    Lang::Log,
];

impl Lang {
    pub fn label(self) -> &'static str {
        match self {
            Lang::Plain => "Text",
            Lang::Shell => "Shell",
            Lang::Python => "Python",
            Lang::Json => "JSON",
            Lang::Yaml => "YAML",
            Lang::Toml => "TOML",
            Lang::Ini => "INI",
            Lang::Conf => "Conf",
            Lang::Server => "nginx/Apache",
            Lang::Markdown => "Markdown",
            Lang::Rust => "Rust",
            Lang::JavaScript => "JavaScript",
            Lang::Html => "HTML/XML",
            Lang::Dockerfile => "Dockerfile",
            Lang::Sql => "SQL",
            Lang::Systemd => "systemd",
            Lang::Log => "Log",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Plain,
    Comment,
    Str,
    Num,
    Kw,
    Ty,
    Func,
    Key,
    Head,
    Tag,
    Attr,
    Var,
    Err,
    Warn,
    Info,
    Dim,
}

/// Colour of a token in the active theme.
pub fn color(tok: Tok) -> Color32 {
    match tok {
        Tok::Plain => theme::text(),
        Tok::Comment | Tok::Dim => theme::text_dim(),
        Tok::Str => theme::green(),
        Tok::Num => theme::amber(),
        Tok::Kw => theme::pink(),
        Tok::Ty => theme::accent2(),
        Tok::Func => theme::accent(),
        Tok::Key => theme::accent(),
        Tok::Head => theme::accent2(),
        Tok::Tag => theme::pink(),
        Tok::Attr => theme::amber(),
        Tok::Var => theme::accent2(),
        Tok::Err => theme::red(),
        Tok::Warn => theme::amber(),
        Tok::Info => theme::green(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub tok: Tok,
}

/// Picks the language from the file name (and the first line for scripts).
pub fn detect(path: &str, first_line: &str) -> Lang {
    let norm = path.replace('\\', "/").to_lowercase();
    let name = norm.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if name == "dockerfile"
        || name == "containerfile"
        || name.starts_with("dockerfile.")
        || name.ends_with(".dockerfile")
    {
        return Lang::Dockerfile;
    }
    if matches!(
        ext,
        "service"
            | "socket"
            | "timer"
            | "mount"
            | "automount"
            | "target"
            | "path"
            | "slice"
            | "scope"
            | "swap"
    ) || (norm.contains("/systemd/") && ext == "conf")
    {
        return Lang::Systemd;
    }
    if name.contains(".log") || ext == "log" || norm.contains("/var/log/") {
        return Lang::Log;
    }
    if name == "nginx.conf"
        || name == "httpd.conf"
        || name == "apache2.conf"
        || name == ".htaccess"
        || name == "ports.conf"
        || norm.contains("/nginx/")
        || norm.contains("/apache2/")
        || norm.contains("/httpd/")
    {
        return Lang::Server;
    }
    match ext {
        "sh" | "bash" | "zsh" | "ksh" | "mk" => return Lang::Shell,
        "py" | "pyw" => return Lang::Python,
        "json" | "jsonc" | "json5" | "geojson" | "ipynb" => return Lang::Json,
        "yml" | "yaml" => return Lang::Yaml,
        "toml" => return Lang::Toml,
        "ini" | "cfg" | "properties" | "editorconfig" => return Lang::Ini,
        "md" | "markdown" | "mdx" => return Lang::Markdown,
        "rs" => return Lang::Rust,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => return Lang::JavaScript,
        "html" | "htm" | "xhtml" | "xml" | "svg" | "xsl" | "xslt" | "plist" => {
            return Lang::Html;
        }
        "sql" => return Lang::Sql,
        "conf" | "cnf" | "env" | "rules" | "repo" => return Lang::Conf,
        _ => {}
    }
    match name {
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".bash_logout" | ".profile" | ".zshrc"
        | ".zprofile" | ".zshenv" | ".xinitrc" | "bashrc" | "profile" | "makefile" => {
            return Lang::Shell;
        }
        ".gitconfig" | ".gitmodules" => return Lang::Ini,
        "sshd_config" | "ssh_config" | "fstab" | "hosts" | "crontab" | "sudoers"
        | "environment" | "resolv.conf" | "exports" | "syslog" | "messages" | "dmesg" => {
            return if matches!(name, "syslog" | "messages" | "dmesg") {
                Lang::Log
            } else {
                Lang::Conf
            };
        }
        _ => {}
    }
    let fl = first_line.trim();
    if let Some(rest) = fl.strip_prefix("#!") {
        let r = rest.to_lowercase();
        if r.contains("python") {
            return Lang::Python;
        }
        if r.contains("node") || r.contains("deno") {
            return Lang::JavaScript;
        }
        if ["bash", "/sh", " sh", "zsh", "dash", "ksh", "ash"]
            .iter()
            .any(|s| r.contains(s))
            || r.ends_with("sh")
        {
            return Lang::Shell;
        }
    }
    if fl.starts_with("<?xml") || fl.to_lowercase().starts_with("<!doctype html") {
        return Lang::Html;
    }
    Lang::Plain
}

// ---------------------------------------------------------------------------- helpers

struct Out {
    spans: Vec<Span>,
}

impl Out {
    fn new() -> Self {
        Self { spans: Vec::new() }
    }

    fn push(&mut self, start: usize, end: usize, tok: Tok) {
        if end <= start || tok == Tok::Plain {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.tok == tok
            && last.end == start
        {
            last.end = end;
            return;
        }
        self.spans.push(Span { start, end, tok });
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn find_from(b: &[u8], from: usize, needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    if n.is_empty() || from > b.len() {
        return None;
    }
    b[from..]
        .windows(n.len())
        .position(|w| w == n)
        .map(|p| p + from)
}

fn eol(b: &[u8], from: usize) -> usize {
    b[from.min(b.len())..]
        .iter()
        .position(|c| *c == b'\n')
        .map(|p| p + from)
        .unwrap_or(b.len())
}

/// `(offset, line)` pairs of the text; the line has no `\n`.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut off = 0;
    text.split('\n').map(move |l| {
        let start = off;
        off += l.len() + 1;
        (start, l)
    })
}

fn leading_ws(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

// ---------------------------------------------------------------------------- generic lexer

#[derive(Default)]
struct Spec {
    line_comment: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [u8],
    triple: bool,
    multiline: &'static [u8],
    kw: &'static [&'static str],
    ty: &'static [&'static str],
    ci: bool,
    dollar: bool,
    attr_at: bool,
    rust_attr: bool,
    lifetimes: bool,
    caps_type: bool,
    /// `#` starts a comment only at the start of a word (shell).
    word_comment: bool,
    func: bool,
    /// Quoted strings followed by `:` are keys (JSON).
    json_keys: bool,
}

fn lex(text: &str, s: &Spec) -> Vec<Span> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Out::new();
    let mut i = 0;
    while i < n {
        let c = b[i];
        if let Some((open, close)) = s.block
            && b[i..].starts_with(open.as_bytes())
        {
            let end = find_from(b, i + open.len(), close)
                .map(|p| p + close.len())
                .unwrap_or(n);
            out.push(i, end, Tok::Comment);
            i = end;
            continue;
        }
        if s.line_comment.iter().any(|lc| {
            b[i..].starts_with(lc.as_bytes())
                && (!s.word_comment
                    || i == 0
                    || b[i - 1].is_ascii_whitespace()
                    || matches!(b[i - 1], b';' | b'&' | b'|' | b'('))
        }) {
            let end = eol(b, i);
            out.push(i, end, Tok::Comment);
            i = end;
            continue;
        }
        if s.rust_attr && c == b'#' && (b[i..].starts_with(b"#[") || b[i..].starts_with(b"#![")) {
            let end = b[i..]
                .iter()
                .position(|x| *x == b']' || *x == b'\n')
                .map(|p| (p + i + 1).min(n))
                .unwrap_or(n);
            out.push(i, end, Tok::Attr);
            i = end;
            continue;
        }
        if s.quotes.contains(&c) {
            if s.lifetimes
                && c == b'\''
                && i + 2 < n
                && is_ident_start(b[i + 1])
                && b[i + 2] != b'\''
            {
                let mut j = i + 1;
                while j < n && is_ident(b[j]) {
                    j += 1;
                }
                out.push(i, j, Tok::Ty);
                i = j;
                continue;
            }
            if s.triple && i + 2 < n && b[i + 1] == c && b[i + 2] == c {
                let end = find_from(
                    b,
                    i + 3,
                    std::str::from_utf8(&[c, c, c]).unwrap_or("\"\"\""),
                )
                .map(|p| p + 3)
                .unwrap_or(n);
                out.push(i, end, Tok::Str);
                i = end;
                continue;
            }
            let mut j = i + 1;
            while j < n {
                if b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if b[j] == c {
                    j += 1;
                    break;
                }
                if b[j] == b'\n' && !s.multiline.contains(&c) {
                    break;
                }
                j += 1;
            }
            let j = j.min(n);
            out.push(i, j, Tok::Str);
            i = j;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_ident(b[i - 1])) {
            let mut j = i + 1;
            while j < n
                && (is_ident(b[j]) || (b[j] == b'.' && j + 1 < n && b[j + 1].is_ascii_digit()))
            {
                j += 1;
            }
            out.push(i, j, Tok::Num);
            i = j;
            continue;
        }
        if is_ident_start(c) {
            let mut j = i + 1;
            while j < n && is_ident(b[j]) {
                j += 1;
            }
            let word = &text[i..j];
            let in_list = |list: &[&str]| {
                if s.ci {
                    list.iter().any(|k| k.eq_ignore_ascii_case(word))
                } else {
                    list.contains(&word)
                }
            };
            let next = b[j..].iter().find(|x| **x != b' ' && **x != b'\t');
            let tok = if in_list(s.kw) {
                Tok::Kw
            } else if in_list(s.ty)
                || (s.caps_type && word.len() > 1 && word.as_bytes()[0].is_ascii_uppercase())
            {
                Tok::Ty
            } else if s.func && next == Some(&b'(') {
                Tok::Func
            } else {
                Tok::Plain
            };
            out.push(i, j, tok);
            i = j;
            continue;
        }
        if c == b'$' && s.dollar && i + 1 < n {
            let mut j = i + 1;
            if b[j] == b'{' {
                j = b[j..]
                    .iter()
                    .position(|x| *x == b'}' || *x == b'\n')
                    .map(|p| (p + j + 1).min(n))
                    .unwrap_or(n);
            } else if is_ident_start(b[j]) {
                while j < n && is_ident(b[j]) {
                    j += 1;
                }
            } else if b[j].is_ascii_digit()
                || matches!(b[j], b'?' | b'#' | b'@' | b'*' | b'!' | b'$')
            {
                j += 1;
            } else {
                i += 1;
                continue;
            }
            out.push(i, j, Tok::Var);
            i = j;
            continue;
        }
        if c == b'@' && s.attr_at && i + 1 < n && is_ident_start(b[i + 1]) {
            let mut j = i + 1;
            while j < n && (is_ident(b[j]) || b[j] == b'.') {
                j += 1;
            }
            out.push(i, j, Tok::Attr);
            i = j;
            continue;
        }
        i += 1;
    }
    if s.json_keys {
        for sp in out.spans.iter_mut().filter(|sp| sp.tok == Tok::Str) {
            let next = b[sp.end.min(n)..].iter().find(|x| !x.is_ascii_whitespace());
            if next == Some(&b':') {
                sp.tok = Tok::Key;
            }
        }
    }
    out.spans
}

const SHELL_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "select", "time", "return", "exit", "break", "continue", "local", "export",
    "readonly", "declare", "typeset", "unset", "source", "alias", "set", "shift", "trap", "eval",
    "exec", "cd", "echo", "printf", "test", "read", "wait", "kill", "true", "false",
];

const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "True",
    "False", "None", "self", "cls", "match", "case",
];

const RUST_KW: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "macro_rules",
];

const RUST_TY: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64", "bool", "char", "str",
];

const JS_KW: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "async",
    "await",
    "static",
    "get",
    "set",
    "null",
    "undefined",
    "true",
    "false",
    "interface",
    "type",
    "enum",
    "implements",
    "public",
    "private",
    "protected",
    "readonly",
    "abstract",
    "as",
    "from",
    "namespace",
    "declare",
];

const JS_TY: &[&str] = &[
    "string", "number", "boolean", "any", "never", "unknown", "object", "Array", "Promise", "Map",
    "Set",
];

const SQL_KW: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "null",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "alter",
    "drop",
    "index",
    "view",
    "primary",
    "key",
    "foreign",
    "references",
    "join",
    "inner",
    "left",
    "right",
    "outer",
    "full",
    "cross",
    "on",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "union",
    "all",
    "distinct",
    "as",
    "case",
    "when",
    "then",
    "else",
    "end",
    "in",
    "is",
    "like",
    "ilike",
    "between",
    "exists",
    "asc",
    "desc",
    "default",
    "constraint",
    "unique",
    "check",
    "begin",
    "commit",
    "rollback",
    "transaction",
    "database",
    "schema",
    "use",
    "grant",
    "revoke",
    "truncate",
    "with",
    "returning",
    "if",
    "cascade",
    "add",
    "column",
    "rename",
    "to",
    "true",
    "false",
    "explain",
    "analyze",
    "vacuum",
];

const SQL_TY: &[&str] = &[
    "int",
    "integer",
    "bigint",
    "smallint",
    "varchar",
    "char",
    "text",
    "boolean",
    "bool",
    "date",
    "timestamp",
    "datetime",
    "decimal",
    "numeric",
    "float",
    "double",
    "serial",
    "uuid",
    "json",
    "jsonb",
    "blob",
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "now",
];

fn lex_lang(lang: Lang, text: &str) -> Vec<Span> {
    match lang {
        Lang::Shell => lex(
            text,
            &Spec {
                line_comment: &["#"],
                quotes: b"'\"`",
                multiline: b"'\"`",
                kw: SHELL_KW,
                dollar: true,
                word_comment: true,
                ..Default::default()
            },
        ),
        Lang::Python => lex(
            text,
            &Spec {
                line_comment: &["#"],
                quotes: b"'\"",
                triple: true,
                kw: PY_KW,
                attr_at: true,
                caps_type: true,
                func: true,
                ..Default::default()
            },
        ),
        Lang::Rust => lex(
            text,
            &Spec {
                line_comment: &["//"],
                block: Some(("/*", "*/")),
                quotes: b"'\"",
                multiline: b"\"",
                kw: RUST_KW,
                ty: RUST_TY,
                rust_attr: true,
                lifetimes: true,
                caps_type: true,
                func: true,
                ..Default::default()
            },
        ),
        Lang::JavaScript => lex(
            text,
            &Spec {
                line_comment: &["//"],
                block: Some(("/*", "*/")),
                quotes: b"'\"`",
                multiline: b"`",
                kw: JS_KW,
                ty: JS_TY,
                attr_at: true,
                caps_type: true,
                func: true,
                ..Default::default()
            },
        ),
        Lang::Sql => lex(
            text,
            &Spec {
                line_comment: &["--"],
                block: Some(("/*", "*/")),
                quotes: b"'\"`",
                multiline: b"'",
                kw: SQL_KW,
                ty: SQL_TY,
                ci: true,
                ..Default::default()
            },
        ),
        Lang::Json => lex(
            text,
            &Spec {
                line_comment: &["//"],
                block: Some(("/*", "*/")),
                quotes: b"\"",
                kw: &["true", "false", "null"],
                json_keys: true,
                ..Default::default()
            },
        ),
        Lang::Yaml => yaml(text),
        Lang::Toml => keyvalue(text, Flavor::Toml),
        Lang::Ini => keyvalue(text, Flavor::Ini),
        Lang::Conf => keyvalue(text, Flavor::Conf),
        Lang::Server => server(text),
        Lang::Markdown => markdown(text),
        Lang::Html => html(text),
        Lang::Dockerfile => dockerfile(text),
        Lang::Systemd => systemd(text),
        Lang::Log => log(text),
        Lang::Plain => Vec::new(),
    }
}

/// Spans of the non-plain parts of `text`, sorted and without overlaps.
pub fn tokenize(lang: Lang, text: &str) -> Vec<Span> {
    let mut spans = lex_lang(lang, text);
    // Defensive: keep only well-formed spans on character boundaries, in order.
    let mut last = 0;
    spans.retain(|s| {
        let ok = s.start >= last
            && s.end > s.start
            && s.end <= text.len()
            && text.is_char_boundary(s.start)
            && text.is_char_boundary(s.end);
        if ok {
            last = s.end;
        }
        ok
    });
    spans
}

// ---------------------------------------------------------------------------- values

/// Tokens of a value (right of `=` / `:`): strings, numbers, booleans, variables, comments.
fn value_tokens(
    out: &mut Out,
    text: &str,
    from: usize,
    to: usize,
    comment_chars: &[u8],
    bools: &[&str],
) {
    let b = text.as_bytes();
    let mut i = from;
    while i < to {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < to {
                if b[j] == b'\\' && c == b'"' {
                    j += 2;
                    continue;
                }
                if b[j] == c {
                    j += 1;
                    break;
                }
                j += 1;
            }
            let j = j.min(to);
            out.push(i, j, Tok::Str);
            i = j;
        } else if comment_chars.contains(&c) && (i == from || b[i - 1].is_ascii_whitespace()) {
            out.push(i, to, Tok::Comment);
            return;
        } else if c == b'$' && i + 1 < to && (b[i + 1] == b'{' || is_ident_start(b[i + 1])) {
            let mut j = i + 1;
            if b[j] == b'{' {
                j = b[j..to]
                    .iter()
                    .position(|x| *x == b'}')
                    .map(|p| p + j + 1)
                    .unwrap_or(to);
            } else {
                while j < to && is_ident(b[j]) {
                    j += 1;
                }
            }
            out.push(i, j, Tok::Var);
            i = j;
        } else if c.is_ascii_digit() && (i == from || !is_ident(b[i - 1])) {
            let mut j = i + 1;
            while j < to && (is_ident(b[j]) || b[j] == b'.' || b[j] == b'-' || b[j] == b':') {
                j += 1;
            }
            // `10m`, `0.5`, `2024-01-01T10:00:00`, `0x1f` count as numbers; mixed words do not.
            let tok = &text[i..j];
            let numberish = tok
                .chars()
                .all(|ch| ch.is_ascii_digit() || ".-:_xXeEabcdefABCDEFTZ+".contains(ch))
                || tok
                    .trim_end_matches(|ch: char| ch.is_ascii_alphabetic())
                    .parse::<f64>()
                    .is_ok();
            if numberish {
                out.push(i, j, Tok::Num);
            }
            i = j;
        } else if is_ident_start(c) {
            let mut j = i + 1;
            while j < to && (is_ident(b[j]) || b[j] == b'.' || b[j] == b'-') {
                j += 1;
            }
            let w = &text[i..j];
            if bools.iter().any(|k| k.eq_ignore_ascii_case(w)) {
                out.push(i, j, Tok::Kw);
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

const BOOLS: &[&str] = &["true", "false", "yes", "no", "on", "off", "null", "none"];

// ---------------------------------------------------------------------------- YAML

fn yaml(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out = Out::new();
    for (off, line) in lines(text) {
        let ws = leading_ws(line);
        let rest = &line[ws..];
        if rest.is_empty() {
            continue;
        }
        let end = off + line.len();
        let mut pos = off + ws;
        if rest.starts_with('#') {
            out.push(pos, end, Tok::Comment);
            continue;
        }
        if ws == 0 && (rest.starts_with("---") || rest.starts_with("...")) {
            out.push(pos, pos + 3, Tok::Kw);
            pos += 3;
        }
        // List markers.
        while pos < end && b[pos] == b'-' && (pos + 1 == end || b[pos + 1] == b' ') {
            pos += 1;
            while pos < end && b[pos] == b' ' {
                pos += 1;
            }
        }
        // Key?
        let mut key_end = None;
        let mut j = pos;
        if j < end && (b[j] == b'"' || b[j] == b'\'') {
            let q = b[j];
            j += 1;
            while j < end && b[j] != q {
                j += 1;
            }
            j = (j + 1).min(end);
            if j < end && b[j] == b':' && (j + 1 == end || b[j + 1] == b' ') {
                key_end = Some(j);
            }
        } else {
            while j < end {
                if b[j] == b':' && (j + 1 == end || b[j + 1] == b' ') {
                    key_end = Some(j);
                    break;
                }
                if b[j] == b'#' && j > pos && b[j - 1] == b' ' {
                    break;
                }
                if matches!(b[j], b'{' | b'[') && j == pos {
                    break;
                }
                j += 1;
            }
        }
        let value_from = match key_end {
            Some(k) if k > pos => {
                out.push(pos, k, Tok::Key);
                k + 1
            }
            _ => pos,
        };
        // Anchors, aliases and tags.
        let mut v = value_from;
        while v < end {
            if matches!(b[v], b'&' | b'*' | b'!') && (v == value_from || b[v - 1] == b' ') {
                let mut k = v + 1;
                while k < end && !b[k].is_ascii_whitespace() {
                    k += 1;
                }
                out.push(v, k, Tok::Attr);
                v = k;
            } else {
                v += 1;
            }
        }
        value_tokens(&mut out, text, value_from, end, b"#", BOOLS);
    }
    // Attr and value spans were pushed out of order; sort and drop overlaps.
    sort_spans(out.spans)
}

fn sort_spans(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));
    let mut last = 0;
    spans.retain(|s| {
        if s.start >= last {
            last = s.end;
            true
        } else {
            false
        }
    });
    spans
}

// ---------------------------------------------------------------------------- INI / TOML / conf

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Toml,
    Ini,
    Conf,
}

fn keyvalue(text: &str, flavor: Flavor) -> Vec<Span> {
    let b = text.as_bytes();
    let comment_chars: &[u8] = if flavor == Flavor::Ini { b"#;" } else { b"#" };
    let mut out = Out::new();
    for (off, line) in lines(text) {
        let ws = leading_ws(line);
        let rest = line[ws..].trim_end();
        if rest.is_empty() {
            continue;
        }
        let start = off + ws;
        let end = start + rest.len();
        if comment_chars.contains(&rest.as_bytes()[0]) {
            out.push(start, end, Tok::Comment);
            continue;
        }
        if rest.starts_with('[') {
            let close = if rest.starts_with("[[") { "]]" } else { "]" };
            let e = rest
                .find(close)
                .map(|p| p + close.len())
                .unwrap_or(rest.len());
            out.push(start, start + e, Tok::Head);
            value_tokens(&mut out, text, start + e, end, comment_chars, BOOLS);
            continue;
        }
        // key / value split.
        let mut sep = None;
        let mut q = None;
        for (k, ch) in rest.bytes().enumerate() {
            match (q, ch) {
                (Some(c), x) if x == c => q = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') if k == 0 => q = Some(ch),
                (None, b'=') => {
                    sep = Some(k);
                    break;
                }
                (None, b':') if flavor == Flavor::Ini => {
                    sep = Some(k);
                    break;
                }
                (None, b' ' | b'\t') if flavor == Flavor::Conf => {
                    sep = Some(k);
                    break;
                }
                _ => {}
            }
        }
        match sep {
            Some(k) => {
                let key_len = rest[..k].trim_end().len();
                out.push(start, start + key_len, Tok::Key);
                let mut v = start + k + 1;
                while v < end && (b[v] == b' ' || b[v] == b'\t') {
                    v += 1;
                }
                value_tokens(&mut out, text, v, end, comment_chars, BOOLS);
            }
            None if flavor == Flavor::Conf => out.push(start, end, Tok::Key),
            None => {}
        }
    }
    out.spans
}

// ---------------------------------------------------------------------------- nginx / Apache

fn server(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Out::new();
    let mut i = 0;
    // Start of a statement: the next word is a directive.
    let mut stmt = true;
    let apache = !b.contains(&b';') && !b.contains(&b'{');
    while i < n {
        let c = b[i];
        match c {
            b'\n' => {
                // Apache has no `;` – a line that is not continued ends the statement.
                if apache && is_apache_line_end(b, i) {
                    stmt = true;
                }
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' if i == 0 || b[i - 1].is_ascii_whitespace() => {
                let e = eol(b, i);
                out.push(i, e, Tok::Comment);
                i = e;
            }
            b'"' | b'\'' => {
                let mut j = i + 1;
                while j < n && b[j] != c && b[j] != b'\n' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let j = (j + 1).min(n);
                out.push(i, j, Tok::Str);
                stmt = false;
                i = j;
            }
            b'<' if stmt => {
                // Apache section: <VirtualHost *:80> / </VirtualHost>
                let e = b[i..]
                    .iter()
                    .position(|x| *x == b'>' || *x == b'\n')
                    .map(|p| p + i + 1)
                    .unwrap_or(n);
                out.push(i, e, Tok::Tag);
                i = e;
            }
            b'{' | b'}' | b';' => {
                stmt = true;
                i += 1;
            }
            b'$' if i + 1 < n && (is_ident_start(b[i + 1]) || b[i + 1] == b'{') => {
                let mut j = i + 1;
                if b[j] == b'{' {
                    j = b[j..]
                        .iter()
                        .position(|x| *x == b'}')
                        .map(|p| p + j + 1)
                        .unwrap_or(n);
                } else {
                    while j < n && is_ident(b[j]) {
                        j += 1;
                    }
                }
                out.push(i, j, Tok::Var);
                stmt = false;
                i = j;
            }
            _ if c.is_ascii_digit() && (i == 0 || !is_ident(b[i - 1])) => {
                let mut j = i + 1;
                while j < n && (is_ident(b[j]) || b[j] == b'.') {
                    j += 1;
                }
                let w = &text[i..j];
                let unit = w.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.');
                if unit.is_empty()
                    || matches!(
                        unit,
                        "k" | "m"
                            | "g"
                            | "K"
                            | "M"
                            | "G"
                            | "s"
                            | "ms"
                            | "h"
                            | "d"
                            | "w"
                            | "y"
                            | "r"
                    )
                {
                    out.push(i, j, Tok::Num);
                }
                stmt = false;
                i = j;
            }
            _ if is_ident_start(c) => {
                let mut j = i + 1;
                while j < n && (is_ident(b[j]) || matches!(b[j], b'-' | b'.' | b'/' | b':')) {
                    j += 1;
                }
                let w = &text[i..j];
                if stmt {
                    out.push(i, j, Tok::Kw);
                    stmt = false;
                } else if matches!(
                    w.to_ascii_lowercase().as_str(),
                    "on" | "off" | "yes" | "no" | "true" | "false" | "all" | "none" | "any"
                ) {
                    out.push(i, j, Tok::Ty);
                }
                i = j;
            }
            _ => {
                stmt = false;
                i += 1;
            }
        }
    }
    out.spans
}

/// Apache has no `;`: a line that is not continued ends the statement.
fn is_apache_line_end(b: &[u8], nl: usize) -> bool {
    let start = b[..nl]
        .iter()
        .rposition(|x| *x == b'\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let line = &b[start..nl];
    let t = line.iter().rev().find(|x| !x.is_ascii_whitespace());
    !matches!(t, Some(b'\\') | Some(b','))
}

// ---------------------------------------------------------------------------- Markdown

fn markdown(text: &str) -> Vec<Span> {
    let mut out = Out::new();
    let mut fence: Option<char> = None;
    for (off, line) in lines(text) {
        let ws = leading_ws(line);
        let rest = &line[ws..];
        let start = off + ws;
        let end = off + line.len();
        if let Some(f) = fence {
            out.push(off, end, Tok::Str);
            if rest.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            continue;
        }
        if rest.starts_with("```") || rest.starts_with("~~~") {
            fence = rest.chars().next();
            out.push(start, end, Tok::Dim);
            continue;
        }
        if rest.starts_with('#') {
            let hashes = rest.chars().take_while(|c| *c == '#').count();
            if hashes <= 6 && rest[hashes..].starts_with(' ') {
                out.push(start, end, Tok::Head);
                continue;
            }
        }
        if rest.starts_with('>') {
            out.push(start, end, Tok::Comment);
            continue;
        }
        if rest.len() >= 3
            && rest.chars().all(|c| matches!(c, '-' | '*' | '_' | ' '))
            && rest.chars().filter(|c| *c != ' ').count() >= 3
        {
            out.push(start, end, Tok::Dim);
            continue;
        }
        // List marker.
        let mut body = start;
        let marker = rest
            .strip_prefix("- ")
            .or_else(|| rest.strip_prefix("* "))
            .or_else(|| rest.strip_prefix("+ "))
            .map(|_| 1)
            .or_else(|| {
                let digits = rest.chars().take_while(char::is_ascii_digit).count();
                (digits > 0 && rest[digits..].starts_with(". ")).then_some(digits + 1)
            });
        if let Some(m) = marker {
            out.push(start, start + m, Tok::Kw);
            body = start + m;
        }
        inline_markdown(&mut out, text, body, end);
    }
    out.spans
}

fn inline_markdown(out: &mut Out, text: &str, from: usize, to: usize) {
    let b = text.as_bytes();
    let mut i = from;
    while i < to {
        match b[i] {
            b'`' => {
                let close = b[i + 1..to].iter().position(|x| *x == b'`');
                if let Some(p) = close {
                    out.push(i, i + 1 + p + 1, Tok::Str);
                    i += p + 2;
                    continue;
                }
                i += 1;
            }
            b'*' | b'_' if i + 1 < to && b[i + 1] == b[i] => {
                let m = [b[i], b[i]];
                let m = std::str::from_utf8(&m).unwrap_or("**");
                if let Some(p) = find_from(&b[..to], i + 2, m) {
                    out.push(i, p + 2, Tok::Ty);
                    i = p + 2;
                    continue;
                }
                i += 2;
            }
            b'[' => {
                // [text](url)
                if let Some(p) = b[i + 1..to].iter().position(|x| *x == b']')
                    && i + 1 + p + 1 < to
                    && b[i + 1 + p + 1] == b'('
                    && let Some(q) = b[i + 1 + p + 2..to].iter().position(|x| *x == b')')
                {
                    let text_end = i + 1 + p + 1;
                    out.push(i, text_end, Tok::Attr);
                    out.push(text_end, text_end + 2 + q + 1, Tok::Dim);
                    i = text_end + 2 + q + 1;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
}

// ---------------------------------------------------------------------------- HTML / XML

fn html(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Out::new();
    let mut i = 0;
    while i < n {
        match b[i] {
            b'<' if b[i..].starts_with(b"<!--") => {
                let e = find_from(b, i + 4, "-->").map(|p| p + 3).unwrap_or(n);
                out.push(i, e, Tok::Comment);
                i = e;
            }
            b'<' if i + 1 < n && (b[i + 1] == b'!' || b[i + 1] == b'?') => {
                let e = b[i..]
                    .iter()
                    .position(|x| *x == b'>')
                    .map(|p| p + i + 1)
                    .unwrap_or(n);
                out.push(i, e, Tok::Kw);
                i = e;
            }
            b'<' if i + 1 < n
                && (is_ident_start(b[i + 1])
                    || (b[i + 1] == b'/' && i + 2 < n && is_ident_start(b[i + 2]))) =>
            {
                let tag_start = i;
                let mut j = i + 1;
                if b[j] == b'/' {
                    j += 1;
                }
                let name_start = j;
                while j < n && (is_ident(b[j]) || matches!(b[j], b':' | b'-' | b'.')) {
                    j += 1;
                }
                let name = text[name_start..j].to_ascii_lowercase();
                out.push(i, j, Tok::Tag);
                // Attributes.
                while j < n && b[j] != b'>' {
                    match b[j] {
                        b'"' | b'\'' => {
                            let q = b[j];
                            let mut k = j + 1;
                            while k < n && b[k] != q {
                                k += 1;
                            }
                            let k = (k + 1).min(n);
                            out.push(j, k, Tok::Str);
                            j = k;
                        }
                        c if is_ident_start(c) || c == b'@' || c == b':' => {
                            let mut k = j + 1;
                            while k < n && (is_ident(b[k]) || matches!(b[k], b':' | b'-' | b'.')) {
                                k += 1;
                            }
                            out.push(j, k, Tok::Attr);
                            j = k;
                        }
                        b'/' | b'?' => {
                            out.push(j, j + 1, Tok::Tag);
                            j += 1;
                        }
                        _ => j += 1,
                    }
                }
                if j < n {
                    out.push(j, j + 1, Tok::Tag);
                    j += 1;
                }
                i = j;
                // Raw text elements: nothing inside is tokenized as markup.
                let closing = b[tag_start + 1] == b'/';
                let self_closed = i >= 2 && b[i - 2] == b'/';
                if !closing && !self_closed && matches!(name.as_str(), "script" | "style") {
                    let close = format!("</{name}");
                    let lower = text[i..].to_ascii_lowercase();
                    i = lower.find(&close).map(|p| p + i).unwrap_or(n);
                }
            }
            b'&' => {
                let e = b[i + 1..n.min(i + 12)]
                    .iter()
                    .position(|x| *x == b';')
                    .map(|p| i + p + 2);
                match e {
                    Some(e)
                        if b[i + 1..e - 1]
                            .iter()
                            .all(|c| c.is_ascii_alphanumeric() || *c == b'#')
                            && e - i > 2 =>
                    {
                        out.push(i, e, Tok::Num);
                        i = e;
                    }
                    _ => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    out.spans
}

// ---------------------------------------------------------------------------- Dockerfile

const DOCKER_KW: &[&str] = &[
    "FROM",
    "RUN",
    "CMD",
    "LABEL",
    "EXPOSE",
    "ENV",
    "ADD",
    "COPY",
    "ENTRYPOINT",
    "VOLUME",
    "USER",
    "WORKDIR",
    "ARG",
    "ONBUILD",
    "STOPSIGNAL",
    "HEALTHCHECK",
    "SHELL",
    "MAINTAINER",
];

fn dockerfile(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out = Out::new();
    let mut continued = false;
    for (off, line) in lines(text) {
        let ws = leading_ws(line);
        let rest = line[ws..].trim_end();
        let start = off + ws;
        let end = start + rest.len();
        if rest.is_empty() {
            continue;
        }
        if rest.starts_with('#') {
            out.push(start, end, Tok::Comment);
            continue;
        }
        let mut pos = start;
        if !continued {
            let mut word_end = start;
            while word_end < end && b[word_end].is_ascii_alphabetic() {
                word_end += 1;
            }
            if DOCKER_KW
                .iter()
                .any(|k| k.eq_ignore_ascii_case(&text[start..word_end]))
            {
                out.push(start, word_end, Tok::Kw);
                pos = word_end;
                // ONBUILD RUN …
                let after = text[pos..end].trim_start();
                let skip = text[pos..end].len() - after.len();
                let mut w = pos + skip;
                while w < end && b[w].is_ascii_alphabetic() {
                    w += 1;
                }
                if text[start..word_end].eq_ignore_ascii_case("onbuild")
                    && DOCKER_KW
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(&text[pos + skip..w]))
                {
                    out.push(pos + skip, w, Tok::Kw);
                    pos = w;
                }
            }
        }
        // `AS` in FROM, flags and variables.
        let mut i = pos;
        while i < end {
            let c = b[i];
            if c == b'"' || c == b'\'' {
                let mut j = i + 1;
                while j < end && b[j] != c {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let j = (j + 1).min(end);
                out.push(i, j, Tok::Str);
                i = j;
            } else if c == b'$' && i + 1 < end && (b[i + 1] == b'{' || is_ident_start(b[i + 1])) {
                let mut j = i + 1;
                if b[j] == b'{' {
                    j = b[j..end]
                        .iter()
                        .position(|x| *x == b'}')
                        .map(|p| p + j + 1)
                        .unwrap_or(end);
                } else {
                    while j < end && is_ident(b[j]) {
                        j += 1;
                    }
                }
                out.push(i, j, Tok::Var);
                i = j;
            } else if c == b'-' && i + 1 < end && b[i + 1] == b'-' && (i == 0 || b[i - 1] == b' ') {
                let mut j = i + 2;
                while j < end && !b[j].is_ascii_whitespace() && b[j] != b'=' {
                    j += 1;
                }
                out.push(i, j, Tok::Attr);
                i = j;
            } else if is_ident_start(c) {
                let mut j = i + 1;
                while j < end && is_ident(b[j]) {
                    j += 1;
                }
                if text[i..j].eq_ignore_ascii_case("as") && i > pos {
                    out.push(i, j, Tok::Kw);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        continued = rest.ends_with('\\');
    }
    out.spans
}

// ---------------------------------------------------------------------------- systemd

fn systemd(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out = Out::new();
    for (off, line) in lines(text) {
        let ws = leading_ws(line);
        let rest = line[ws..].trim_end();
        if rest.is_empty() {
            continue;
        }
        let start = off + ws;
        let end = start + rest.len();
        if rest.starts_with('#') || rest.starts_with(';') {
            out.push(start, end, Tok::Comment);
        } else if rest.starts_with('[') {
            out.push(start, end, Tok::Head);
        } else if let Some(eq) = rest.find('=') {
            out.push(start, start + eq, Tok::Key);
            let mut i = start + eq + 1;
            while i < end {
                let c = b[i];
                if c == b'"' || c == b'\'' {
                    let mut j = i + 1;
                    while j < end && b[j] != c {
                        if b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    let j = (j + 1).min(end);
                    out.push(i, j, Tok::Str);
                    i = j;
                } else if c == b'%' && i + 1 < end && b[i + 1].is_ascii_alphabetic() {
                    out.push(i, i + 2, Tok::Var);
                    i += 2;
                } else if c == b'$' && i + 1 < end && (b[i + 1] == b'{' || is_ident_start(b[i + 1]))
                {
                    let mut j = i + 1;
                    if b[j] == b'{' {
                        j = b[j..end]
                            .iter()
                            .position(|x| *x == b'}')
                            .map(|p| p + j + 1)
                            .unwrap_or(end);
                    } else {
                        while j < end && is_ident(b[j]) {
                            j += 1;
                        }
                    }
                    out.push(i, j, Tok::Var);
                    i = j;
                } else if c.is_ascii_digit() && (i == 0 || !is_ident(b[i - 1])) {
                    let mut j = i + 1;
                    while j < end && is_ident(b[j]) {
                        j += 1;
                    }
                    if text[i..j]
                        .trim_end_matches(|ch: char| ch.is_ascii_alphabetic())
                        .chars()
                        .all(|ch| ch.is_ascii_digit())
                    {
                        out.push(i, j, Tok::Num);
                    }
                    i = j;
                } else if is_ident_start(c) {
                    let mut j = i + 1;
                    while j < end && is_ident(b[j]) {
                        j += 1;
                    }
                    if matches!(
                        text[i..j].to_ascii_lowercase().as_str(),
                        "yes" | "no" | "true" | "false" | "on" | "off"
                    ) {
                        out.push(i, j, Tok::Kw);
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
    }
    out.spans
}

// ---------------------------------------------------------------------------- logs

fn level_of(word: &str) -> Option<Tok> {
    Some(match word.to_ascii_uppercase().as_str() {
        "ERROR" | "ERR" | "FATAL" | "CRIT" | "CRITICAL" | "SEVERE" | "PANIC" | "ALERT"
        | "EMERG" | "EMERGENCY" => Tok::Err,
        "WARN" | "WARNING" => Tok::Warn,
        "INFO" | "NOTICE" => Tok::Info,
        "DEBUG" | "TRACE" | "VERBOSE" => Tok::Dim,
        _ => return None,
    })
}

fn log(text: &str) -> Vec<Span> {
    let mut out = Out::new();
    const MONTHS: &[&str] = &[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    for (off, line) in lines(text) {
        let b = line.as_bytes();
        let n = b.len();
        if n == 0 {
            continue;
        }
        // Timestamp.
        let mut pos = 0;
        if b[0].is_ascii_digit() {
            while pos < n
                && (b[pos].is_ascii_digit()
                    || matches!(
                        b[pos],
                        b'-' | b':' | b'.' | b',' | b'/' | b'+' | b'T' | b'Z' | b' '
                    ))
            {
                pos += 1;
            }
            while pos > 0 && b[pos - 1] == b' ' {
                pos -= 1;
            }
        } else if n > 15 && MONTHS.iter().any(|m| line.starts_with(m)) && b[3] == b' ' {
            // Syslog: "Oct 18 12:00:01"
            pos = 15.min(n);
            if !line.is_char_boundary(pos) {
                pos = 0;
            }
        }
        out.push(off, off + pos, Tok::Dim);
        // First level word, or error-ish words.
        let mut i = pos;
        let mut level_done = false;
        while i < n {
            if is_ident_start(b[i]) {
                let mut j = i + 1;
                while j < n && is_ident(b[j]) {
                    j += 1;
                }
                let w = &line[i..j];
                if !level_done && let Some(tok) = level_of(w) {
                    level_done = true;
                    match tok {
                        Tok::Err | Tok::Warn => {
                            out.push(off + i, off + n, tok);
                            break;
                        }
                        _ => out.push(off + i, off + j, tok),
                    }
                } else if !level_done
                    && matches!(
                        w.to_ascii_lowercase().as_str(),
                        "failed"
                            | "failure"
                            | "error"
                            | "denied"
                            | "refused"
                            | "fatal"
                            | "panic"
                            | "segfault"
                            | "timeout"
                    )
                {
                    out.push(off + i, off + j, Tok::Err);
                } else if !level_done
                    && matches!(w.to_ascii_lowercase().as_str(), "warning" | "deprecated")
                {
                    out.push(off + i, off + j, Tok::Warn);
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    out.spans
}

// ---------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(lang: Lang, text: &str) -> Vec<(String, Tok)> {
        tokenize(lang, text)
            .into_iter()
            .map(|s| (text[s.start..s.end].to_string(), s.tok))
            .collect()
    }

    fn has(v: &[(String, Tok)], text: &str, tok: Tok) -> bool {
        v.iter().any(|(t, k)| t == text && *k == tok)
    }

    #[test]
    fn detection() {
        assert_eq!(detect("/home/a/run.sh", ""), Lang::Shell);
        assert_eq!(detect("/home/a/.bashrc", ""), Lang::Shell);
        assert_eq!(detect("/x/script", "#!/usr/bin/env python3"), Lang::Python);
        assert_eq!(detect("/x/script", "#!/bin/bash"), Lang::Shell);
        assert_eq!(detect("/x/a.json", ""), Lang::Json);
        assert_eq!(detect("/x/a.YML", ""), Lang::Yaml);
        assert_eq!(detect("/x/Cargo.toml", ""), Lang::Toml);
        assert_eq!(detect("/etc/nginx/nginx.conf", ""), Lang::Server);
        assert_eq!(detect("/etc/nginx/sites-enabled/default", ""), Lang::Server);
        assert_eq!(detect("/etc/apache2/apache2.conf", ""), Lang::Server);
        assert_eq!(detect("/etc/ssh/sshd_config", ""), Lang::Conf);
        assert_eq!(detect("/etc/systemd/system/a.service", ""), Lang::Systemd);
        assert_eq!(detect("/srv/Dockerfile", ""), Lang::Dockerfile);
        assert_eq!(detect("/srv/Dockerfile.dev", ""), Lang::Dockerfile);
        assert_eq!(detect("/var/log/syslog", ""), Lang::Log);
        assert_eq!(detect("/var/log/nginx/access.log.1", ""), Lang::Log);
        assert_eq!(detect("/x/a.rs", ""), Lang::Rust);
        assert_eq!(detect("/x/a.ts", ""), Lang::JavaScript);
        assert_eq!(detect("/x/index.html", ""), Lang::Html);
        assert_eq!(detect("/x/q.sql", ""), Lang::Sql);
        assert_eq!(detect("/x/README.md", ""), Lang::Markdown);
        assert_eq!(detect("/x/notes", "hello"), Lang::Plain);
        assert_eq!(detect("C:\\x\\a.py", ""), Lang::Python);
    }

    #[test]
    fn shell_tokens() {
        let v = toks(
            Lang::Shell,
            "#!/bin/sh\nif [ -n \"$HOME\" ]; then echo 'hi' # c\nfi\nx=$(ls) ${A:-b}\n",
        );
        assert!(has(&v, "#!/bin/sh", Tok::Comment));
        assert!(has(&v, "if", Tok::Kw));
        assert!(has(&v, "\"$HOME\"", Tok::Str));
        assert!(has(&v, "'hi'", Tok::Str));
        assert!(has(&v, "# c", Tok::Comment));
        assert!(has(&v, "${A:-b}", Tok::Var));
        // `#` inside a word is not a comment.
        let v = toks(Lang::Shell, "echo a#b\n");
        assert!(!v.iter().any(|(_, k)| *k == Tok::Comment));
    }

    #[test]
    fn python_tokens() {
        let v = toks(
            Lang::Python,
            "@app.route('/')\ndef f(x):\n    \"\"\"doc\n    more\"\"\"\n    return x + 1  # n\n",
        );
        assert!(has(&v, "@app.route", Tok::Attr));
        assert!(has(&v, "def", Tok::Kw));
        assert!(has(&v, "f", Tok::Func));
        assert!(
            v.iter()
                .any(|(t, k)| t.starts_with("\"\"\"doc") && *k == Tok::Str)
        );
        assert!(has(&v, "return", Tok::Kw));
        assert!(has(&v, "1", Tok::Num));
        assert!(has(&v, "# n", Tok::Comment));
    }

    #[test]
    fn json_keys_and_values() {
        let v = toks(Lang::Json, "{\"a\": [1, 2.5, true, null], \"b\": \"x\"}");
        assert!(has(&v, "\"a\"", Tok::Key));
        assert!(has(&v, "\"b\"", Tok::Key));
        assert!(has(&v, "\"x\"", Tok::Str));
        assert!(has(&v, "2.5", Tok::Num));
        assert!(has(&v, "true", Tok::Kw));
        assert!(has(&v, "null", Tok::Kw));
    }

    #[test]
    fn yaml_tokens() {
        let v = toks(
            Lang::Yaml,
            "---\n# comment\nname: web\nports:\n  - 80\n  - \"443\"\nenabled: true\nref: &a value\n",
        );
        assert!(has(&v, "---", Tok::Kw));
        assert!(has(&v, "# comment", Tok::Comment));
        assert!(has(&v, "name", Tok::Key));
        assert!(has(&v, "ports", Tok::Key));
        assert!(has(&v, "80", Tok::Num));
        assert!(has(&v, "\"443\"", Tok::Str));
        assert!(has(&v, "true", Tok::Kw));
        assert!(has(&v, "&a", Tok::Attr));
    }

    #[test]
    fn toml_ini_conf() {
        let v = toks(
            Lang::Toml,
            "[package]\nname = \"x\" # n\nversion = 1\nok = true\n",
        );
        assert!(has(&v, "[package]", Tok::Head));
        assert!(has(&v, "name", Tok::Key));
        assert!(has(&v, "\"x\"", Tok::Str));
        assert!(has(&v, "# n", Tok::Comment));
        assert!(has(&v, "1", Tok::Num));
        assert!(has(&v, "true", Tok::Kw));
        let v = toks(Lang::Ini, "; c\n[s]\nk: v\n");
        assert!(has(&v, "; c", Tok::Comment));
        assert!(has(&v, "k", Tok::Key));
        let v = toks(Lang::Conf, "# x\nPermitRootLogin no\nPort 22\n");
        assert!(has(&v, "PermitRootLogin", Tok::Key));
        assert!(has(&v, "# x", Tok::Comment));
    }

    #[test]
    fn server_config() {
        let v = toks(
            Lang::Server,
            "server {\n    listen 80;\n    server_name example.com;\n    location / { proxy_pass http://127.0.0.1:8080; }\n    # c\n    client_max_body_size 10m;\n    gzip on;\n    set $x \"a\";\n}\n",
        );
        assert!(has(&v, "server", Tok::Kw));
        assert!(has(&v, "listen", Tok::Kw));
        assert!(has(&v, "80", Tok::Num));
        assert!(has(&v, "location", Tok::Kw));
        assert!(has(&v, "proxy_pass", Tok::Kw));
        assert!(has(&v, "# c", Tok::Comment));
        assert!(has(&v, "10m", Tok::Num));
        assert!(has(&v, "on", Tok::Ty));
        assert!(has(&v, "$x", Tok::Var));
        let v = toks(
            Lang::Server,
            "<VirtualHost *:80>\n  ServerName a.b\n  DocumentRoot \"/var/www\"\n</VirtualHost>\n",
        );
        assert!(has(&v, "<VirtualHost *:80>", Tok::Tag));
        assert!(has(&v, "ServerName", Tok::Kw));
        assert!(has(&v, "DocumentRoot", Tok::Kw));
        assert!(has(&v, "\"/var/www\"", Tok::Str));
        assert!(has(&v, "</VirtualHost>", Tok::Tag));
    }

    #[test]
    fn markdown_tokens() {
        let v = toks(
            Lang::Markdown,
            "# Title\ntext `code` and **bold** [l](http://x)\n- item\n> quote\n```sh\nls\n```\n",
        );
        assert!(has(&v, "# Title", Tok::Head));
        assert!(has(&v, "`code`", Tok::Str));
        assert!(has(&v, "**bold**", Tok::Ty));
        assert!(has(&v, "[l]", Tok::Attr));
        assert!(has(&v, "-", Tok::Kw));
        assert!(has(&v, "> quote", Tok::Comment));
        assert!(has(&v, "ls", Tok::Str));
    }

    #[test]
    fn rust_tokens() {
        let v = toks(
            Lang::Rust,
            "#[derive(Debug)]\nfn main(x: &'a str) -> Result<u8, String> { let c = 'x'; /* b */ println!(\"hi\"); // end\n }",
        );
        assert!(has(&v, "#[derive(Debug)]", Tok::Attr));
        assert!(has(&v, "fn", Tok::Kw));
        assert!(has(&v, "main", Tok::Func));
        assert!(has(&v, "'a", Tok::Ty));
        assert!(has(&v, "Result", Tok::Ty));
        assert!(has(&v, "u8", Tok::Ty));
        assert!(has(&v, "'x'", Tok::Str));
        assert!(has(&v, "/* b */", Tok::Comment));
        assert!(has(&v, "\"hi\"", Tok::Str));
        assert!(has(&v, "// end", Tok::Comment));
    }

    #[test]
    fn javascript_tokens() {
        let v = toks(
            Lang::JavaScript,
            "const a = `x\ny`; function f() { return 1 } // c\n",
        );
        assert!(has(&v, "const", Tok::Kw));
        assert!(has(&v, "`x\ny`", Tok::Str));
        assert!(has(&v, "function", Tok::Kw));
        assert!(has(&v, "// c", Tok::Comment));
    }

    #[test]
    fn html_tokens() {
        let v = toks(
            Lang::Html,
            "<!DOCTYPE html>\n<!-- c -->\n<a href=\"x\" class=y>t &amp; u</a><script>if (a<b) {}</script>",
        );
        assert!(has(&v, "<!DOCTYPE html>", Tok::Kw));
        assert!(has(&v, "<!-- c -->", Tok::Comment));
        assert!(has(&v, "<a", Tok::Tag));
        assert!(has(&v, "href", Tok::Attr));
        assert!(has(&v, "\"x\"", Tok::Str));
        assert!(has(&v, "&amp;", Tok::Num));
        assert!(
            v.iter()
                .any(|(t, k)| t.starts_with("</a") && *k == Tok::Tag)
        );
        // script body is not tokenized as tags.
        assert!(!has(&v, "<b", Tok::Tag));
    }

    #[test]
    fn dockerfile_tokens() {
        let v = toks(
            Lang::Dockerfile,
            "FROM rust:1 AS build\n# c\nRUN cargo build \\\n  --release\nENV A=$B\nCMD [\"a\"]\n",
        );
        assert!(has(&v, "FROM", Tok::Kw));
        assert!(has(&v, "AS", Tok::Kw));
        assert!(has(&v, "# c", Tok::Comment));
        assert!(has(&v, "RUN", Tok::Kw));
        assert!(has(&v, "ENV", Tok::Kw));
        assert!(has(&v, "$B", Tok::Var));
        assert!(has(&v, "CMD", Tok::Kw));
        assert!(has(&v, "\"a\"", Tok::Str));
    }

    #[test]
    fn sql_tokens() {
        let v = toks(
            Lang::Sql,
            "SELECT id, 'x' FROM t -- c\nWHERE a = 1 /* b */;",
        );
        assert!(has(&v, "SELECT", Tok::Kw));
        assert!(has(&v, "FROM", Tok::Kw));
        assert!(has(&v, "'x'", Tok::Str));
        assert!(has(&v, "-- c", Tok::Comment));
        assert!(has(&v, "1", Tok::Num));
        assert!(has(&v, "/* b */", Tok::Comment));
    }

    #[test]
    fn systemd_tokens() {
        let v = toks(
            Lang::Systemd,
            "[Unit]\n# c\nDescription=Web %n\n[Service]\nExecStart=/bin/x --a \"b\" $X\nRestart=on-failure\nTimeoutSec=30\nRemainAfterExit=yes\n",
        );
        assert!(has(&v, "[Unit]", Tok::Head));
        assert!(has(&v, "# c", Tok::Comment));
        assert!(has(&v, "Description", Tok::Key));
        assert!(has(&v, "%n", Tok::Var));
        assert!(has(&v, "\"b\"", Tok::Str));
        assert!(has(&v, "$X", Tok::Var));
        assert!(has(&v, "30", Tok::Num));
        assert!(has(&v, "yes", Tok::Kw));
    }

    #[test]
    fn log_levels() {
        let text = "2024-05-01 12:00:01,123 ERROR something broke\n2024-05-01T12:00:02Z WARN careful now\nOct 18 12:00:01 host sshd[1]: Failed password for root\nINFO started\nDEBUG x\n";
        let v = toks(Lang::Log, text);
        assert!(
            v.iter()
                .any(|(t, k)| t.starts_with("ERROR something") && *k == Tok::Err)
        );
        assert!(
            v.iter()
                .any(|(t, k)| t.starts_with("WARN careful") && *k == Tok::Warn)
        );
        assert!(has(&v, "Failed", Tok::Err));
        assert!(has(&v, "INFO", Tok::Info));
        assert!(has(&v, "DEBUG", Tok::Dim));
        assert!(
            v.iter()
                .any(|(t, k)| t.starts_with("2024-05-01 12:00:01") && *k == Tok::Dim)
        );
    }

    #[test]
    fn spans_are_valid_for_odd_input() {
        // Non-ASCII, unterminated constructs and empty input never panic.
        let samples = [
            "",
            "\"unterminated",
            "'",
            "/* open",
            "日本語 = \"ü\" # ö\n",
            "<a href=\"x",
            "```\nopen fence",
            "key: 'x\nnext: ü",
            "[sec",
            "$",
            "${",
            "a\u{0}b",
            "\r\n\r\n",
        ];
        for lang in ALL {
            for s in samples {
                let spans = tokenize(*lang, s);
                let mut last = 0;
                for sp in spans {
                    assert!(
                        sp.start >= last && sp.end > sp.start && sp.end <= s.len(),
                        "{lang:?} {s:?}"
                    );
                    assert!(s.is_char_boundary(sp.start) && s.is_char_boundary(sp.end));
                    last = sp.end;
                }
            }
        }
    }
}
