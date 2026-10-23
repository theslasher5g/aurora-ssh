//! WS-Management (SOAP) messages of WinRM: building the requests (create shell, run a
//! command, receive output, terminate, delete shell) and reading the answers, plus a small
//! tolerant XML reader – the answers of a Windows host are regular, so a full XML library
//! is not needed. Pure functions, no I/O.

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

const URI_CMD: &str = "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/cmd";
const ACTION_CREATE: &str = "http://schemas.xmlsoap.org/ws/2004/09/transfer/Create";
const ACTION_DELETE: &str = "http://schemas.xmlsoap.org/ws/2004/09/transfer/Delete";
const ACTION_COMMAND: &str = "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/Command";
const ACTION_RECEIVE: &str = "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/Receive";
const ACTION_SIGNAL: &str = "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/Signal";
const SIGNAL_TERMINATE: &str =
    "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/signal/terminate";
const STATE_DONE: &str =
    "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/CommandState/Done";

/// WSMan fault code for "no output within the operation timeout" – ask again.
pub const FAULT_TIMEOUT: u32 = 2150858793;

/// Largest command line Windows accepts when no `cmd.exe` is involved.
pub const MAX_COMMAND_LINE: usize = 30_000;

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn message_id() -> String {
    format!("uuid:{}", uuid::Uuid::new_v4().to_string().to_uppercase())
}

/// A complete SOAP envelope. `options` become `<w:Option>` entries.
fn envelope(
    url: &str,
    action: &str,
    shell: Option<&str>,
    options: &[(&str, &str)],
    timeout_secs: u32,
    body: &str,
) -> String {
    let mut h = String::new();
    h.push_str(concat!(
        "<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" ",
        "xmlns:a=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" ",
        "xmlns:w=\"http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd\" ",
        "xmlns:p=\"http://schemas.microsoft.com/wbem/wsman/1/wsman.xsd\" ",
        "xmlns:rsp=\"http://schemas.microsoft.com/wbem/wsman/1/windows/shell\">",
        "<s:Header>"
    ));
    h.push_str(&format!("<a:To>{}</a:To>", xml_escape(url)));
    h.push_str(&format!(
        "<w:ResourceURI s:mustUnderstand=\"true\">{URI_CMD}</w:ResourceURI>"
    ));
    h.push_str(
        "<a:ReplyTo><a:Address s:mustUnderstand=\"true\">http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</a:Address></a:ReplyTo>",
    );
    h.push_str(&format!(
        "<a:Action s:mustUnderstand=\"true\">{action}</a:Action>"
    ));
    h.push_str("<w:MaxEnvelopeSize s:mustUnderstand=\"true\">512000</w:MaxEnvelopeSize>");
    h.push_str(&format!("<a:MessageID>{}</a:MessageID>", message_id()));
    h.push_str("<w:Locale xml:lang=\"en-US\" s:mustUnderstand=\"false\"/>");
    h.push_str("<p:DataLocale xml:lang=\"en-US\" s:mustUnderstand=\"false\"/>");
    h.push_str(&format!(
        "<w:OperationTimeout>PT{timeout_secs}S</w:OperationTimeout>"
    ));
    if let Some(id) = shell {
        h.push_str(&format!(
            "<w:SelectorSet><w:Selector Name=\"ShellId\">{}</w:Selector></w:SelectorSet>",
            xml_escape(id)
        ));
    }
    if !options.is_empty() {
        h.push_str("<w:OptionSet>");
        for (k, v) in options {
            h.push_str(&format!(
                "<w:Option Name=\"{}\">{}</w:Option>",
                xml_escape(k),
                xml_escape(v)
            ));
        }
        h.push_str("</w:OptionSet>");
    }
    h.push_str("</s:Header><s:Body>");
    h.push_str(body);
    h.push_str("</s:Body></s:Envelope>");
    h
}

pub fn create_shell(url: &str) -> String {
    envelope(
        url,
        ACTION_CREATE,
        None,
        &[("WINRS_NOPROFILE", "TRUE"), ("WINRS_CODEPAGE", "65001")],
        60,
        "<rsp:Shell><rsp:InputStreams>stdin</rsp:InputStreams><rsp:OutputStreams>stdout stderr</rsp:OutputStreams></rsp:Shell>",
    )
}

/// Runs `command` with `args` in the shell (no `cmd.exe` in between).
pub fn command(url: &str, shell: &str, command: &str, args: &[String]) -> String {
    let mut body = format!(
        "<rsp:CommandLine><rsp:Command>{}</rsp:Command>",
        xml_escape(command)
    );
    for a in args {
        body.push_str(&format!("<rsp:Arguments>{}</rsp:Arguments>", xml_escape(a)));
    }
    body.push_str("</rsp:CommandLine>");
    envelope(
        url,
        ACTION_COMMAND,
        Some(shell),
        &[
            ("WINRS_CONSOLEMODE_STDIN", "TRUE"),
            ("WINRS_SKIP_CMD_SHELL", "TRUE"),
        ],
        60,
        &body,
    )
}

pub fn receive(url: &str, shell: &str, command_id: &str, timeout_secs: u32) -> String {
    envelope(
        url,
        ACTION_RECEIVE,
        Some(shell),
        &[],
        timeout_secs,
        &format!(
            "<rsp:Receive><rsp:DesiredStream CommandId=\"{}\">stdout stderr</rsp:DesiredStream></rsp:Receive>",
            xml_escape(command_id)
        ),
    )
}

pub fn terminate(url: &str, shell: &str, command_id: &str) -> String {
    envelope(
        url,
        ACTION_SIGNAL,
        Some(shell),
        &[],
        20,
        &format!(
            "<rsp:Signal CommandId=\"{}\"><rsp:Code>{SIGNAL_TERMINATE}</rsp:Code></rsp:Signal>",
            xml_escape(command_id)
        ),
    )
}

pub fn delete_shell(url: &str, shell: &str) -> String {
    envelope(url, ACTION_DELETE, Some(shell), &[], 20, "")
}

/// Arguments for `powershell.exe` that run `script` (UTF-16LE, base64): no quoting
/// problems whatever the script contains. Output is forced to UTF-8 without progress bars.
pub fn powershell_args(script: &str) -> Result<Vec<String>> {
    let full = format!(
        "$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=New-Object Text.UTF8Encoding $false;{script}"
    );
    let utf16: Vec<u8> = full.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = B64.encode(utf16);
    if encoded.len() > MAX_COMMAND_LINE {
        bail!(
            "{}",
            t!("Der Befehl ist zu lang für WinRM (höchstens etwa 10 000 Zeichen)")
        );
    }
    Ok(vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-EncodedCommand".into(),
        encoded,
    ])
}

// ---------------------------------------------------------------------------
// Reading answers

/// One XML element (names without namespace prefix).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Elem {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// Text directly inside the element (entities decoded, children skipped).
    pub text: String,
    pub children: Vec<Elem>,
}

impl Elem {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// First element with this local name, depth first (including `self`).
    pub fn find(&self, name: &str) -> Option<&Elem> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }

    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Elem>) {
        if self.name == name {
            out.push(self);
        }
        for c in &self.children {
            c.find_all(name, out);
        }
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..end];
        let ch = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, n)| n)
}

/// Parses `<name a="1" b='2'/>` attributes.
fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = &s[start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' || name.is_empty() {
            break;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let Some(&q) = b.get(i).filter(|q| **q == b'"' || **q == b'\'') else {
            break;
        };
        i += 1;
        let vstart = i;
        while i < b.len() && b[i] != q {
            i += 1;
        }
        out.push((local(name).to_string(), decode_entities(&s[vstart..i])));
        i += 1;
    }
    out
}

/// Parses a document into its root element.
pub fn parse(xml: &str) -> Result<Elem> {
    let mut stack: Vec<Elem> = vec![Elem::default()];
    let mut i = 0;
    let b = xml.as_bytes();
    while i < b.len() {
        if b[i] != b'<' {
            let end = xml[i..].find('<').map_or(b.len(), |e| i + e);
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&decode_entities(&xml[i..end]));
            }
            i = end;
            continue;
        }
        let rest = &xml[i..];
        if rest.starts_with("<!--") {
            i += rest
                .find("-->")
                .ok_or_else(|| anyhow!("unterminated comment"))?
                + 3;
        } else if rest.starts_with("<![CDATA[") {
            let end = rest
                .find("]]>")
                .ok_or_else(|| anyhow!("unterminated CDATA"))?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&rest[9..end]);
            }
            i += end + 3;
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            i += rest
                .find('>')
                .ok_or_else(|| anyhow!("unterminated declaration"))?
                + 1;
        } else if let Some(close) = rest.strip_prefix("</") {
            let end = close.find('>').ok_or_else(|| anyhow!("unterminated tag"))?;
            let name = local(close[..end].trim()).to_string();
            if stack.len() < 2 {
                bail!("unbalanced XML");
            }
            let done = stack.pop().unwrap_or_default();
            if done.name != name {
                bail!("mismatched XML tag {name}");
            }
            if let Some(top) = stack.last_mut() {
                top.children.push(done);
            }
            i += 2 + end + 1;
        } else {
            // Start tag; a '>' inside a quoted attribute value is legal.
            let mut j = 1;
            let mut quote = 0u8;
            while j < rest.len() {
                let c = rest.as_bytes()[j];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                j += 1;
            }
            if j >= rest.len() {
                bail!("unterminated tag");
            }
            let inner = &rest[1..j];
            let (inner, selfclose) = match inner.strip_suffix('/') {
                Some(x) => (x, true),
                None => (inner, false),
            };
            let (name, attrs) = match inner.find(|c: char| c.is_whitespace()) {
                Some(p) => (&inner[..p], parse_attrs(&inner[p..])),
                None => (inner, Vec::new()),
            };
            let el = Elem {
                name: local(name).to_string(),
                attrs,
                ..Default::default()
            };
            if selfclose {
                if let Some(top) = stack.last_mut() {
                    top.children.push(el);
                }
            } else {
                if stack.len() > 64 {
                    bail!("XML nested too deeply");
                }
                stack.push(el);
            }
            i += j + 1;
        }
    }
    if stack.len() != 1 {
        bail!("unbalanced XML");
    }
    let root = stack.pop().unwrap_or_default();
    root.children
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty XML document"))
}

/// A SOAP fault.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fault {
    pub reason: String,
    /// `Code` attribute of `WSManFault`.
    pub code: Option<u32>,
    pub message: String,
}

impl Fault {
    pub fn is_timeout(&self) -> bool {
        self.code == Some(FAULT_TIMEOUT)
    }

    pub fn text(&self) -> String {
        let m = if self.message.trim().is_empty() {
            self.reason.trim()
        } else {
            self.message.trim()
        };
        match self.code {
            Some(c) => format!("{m} (WSMan {c})"),
            None => m.to_string(),
        }
    }
}

pub fn fault(root: &Elem) -> Option<Fault> {
    let f = root.find("Fault")?;
    let wsman = root.find("WSManFault");
    Some(Fault {
        reason: f
            .find("Reason")
            .and_then(|r| r.find("Text"))
            .map(|t| t.text.trim().to_string())
            .unwrap_or_default(),
        code: wsman
            .and_then(|w| w.attr("Code"))
            .and_then(|c| c.trim().parse().ok()),
        message: wsman
            .and_then(|w| w.find("Message"))
            .map(|m| collapse(&m.text))
            .unwrap_or_default(),
    })
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses an answer; a fault becomes an error (the caller may look at it first with
/// [`parse`] + [`fault`]).
pub fn parse_response(xml: &str) -> Result<Elem> {
    let root = parse(xml)?;
    if let Some(f) = fault(&root) {
        bail!("{}", f.text());
    }
    Ok(root)
}

pub fn shell_id(root: &Elem) -> Result<String> {
    let mut sel = Vec::new();
    root.find_all("Selector", &mut sel);
    sel.iter()
        .find(|s| s.attr("Name") == Some("ShellId"))
        .map(|s| s.text.trim().to_string())
        .or_else(|| root.find("ShellId").map(|s| s.text.trim().to_string()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("{}", t!("Die Antwort des Servers enthält keine Shell-ID")))
}

pub fn command_id(root: &Elem) -> Result<String> {
    root.find("CommandId")
        .map(|c| c.text.trim().to_string())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| anyhow!("{}", t!("Die Antwort des Servers enthält keine Befehls-ID")))
}

/// What one `Receive` answer delivered.
#[derive(Debug, Default, PartialEq)]
pub struct Received {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub done: bool,
    pub exit: Option<i32>,
}

pub fn received(root: &Elem) -> Received {
    let mut out = Received::default();
    let mut streams = Vec::new();
    root.find_all("Stream", &mut streams);
    for s in streams {
        let data = B64
            .decode(s.text.split_whitespace().collect::<String>())
            .unwrap_or_default();
        match s.attr("Name") {
            Some("stdout") => out.stdout.extend_from_slice(&data),
            Some("stderr") => out.stderr.extend_from_slice(&data),
            _ => {}
        }
    }
    if let Some(st) = root.find("CommandState") {
        out.done = st.attr("State") == Some(STATE_DONE);
        out.exit = st.find("ExitCode").and_then(|c| c.text.trim().parse().ok());
    }
    out
}

/// PowerShell reports errors on stderr as CLIXML (`#< CLIXML` + `<Objs>`); this returns
/// the error text. Anything else is returned unchanged.
pub fn clixml_to_text(s: &str) -> String {
    let Some(rest) = s.trim_start().strip_prefix("#< CLIXML") else {
        return s.to_string();
    };
    let Ok(root) = parse(rest.trim_start()) else {
        return s.to_string();
    };
    let mut parts = Vec::new();
    root.find_all("S", &mut parts);
    let mut out = String::new();
    for p in parts {
        if matches!(p.attr("S"), Some("Error" | "error")) {
            out.push_str(&unescape_clixml(&p.text));
        }
    }
    if out.trim().is_empty() {
        // Progress and information records only: nothing to say.
        return String::new();
    }
    out
}

/// `_x000D__x000A_` and friends.
fn unescape_clixml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("_x") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let code = tail
            .get(2..6)
            .filter(|h| h.chars().all(|c| c.is_ascii_hexdigit()))
            .filter(|_| tail.get(6..7) == Some("_"))
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32);
        match code {
            Some('\r') => {}
            Some(c) => out.push(c),
            None => {
                out.push_str("_x");
                rest = &tail[2..];
                continue;
            }
        }
        rest = &tail[7..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE_RESPONSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xml:lang="en-US" xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:x="http://schemas.xmlsoap.org/ws/2004/09/transfer" xmlns:w="http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd" xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell">
<s:Header><a:Action>http://schemas.xmlsoap.org/ws/2004/09/transfer/CreateResponse</a:Action></s:Header>
<s:Body><x:ResourceCreated><a:Address>http://host:5985/wsman</a:Address><a:ReferenceParameters><w:ResourceURI>http://schemas.microsoft.com/wbem/wsman/1/windows/shell/cmd</w:ResourceURI><w:SelectorSet><w:Selector Name="ShellId">3C1A5F2B-4C2E-4C3C-9F6C-0D4B8F1B2A11</w:Selector></w:SelectorSet></a:ReferenceParameters></x:ResourceCreated>
<rsp:Shell><rsp:ShellId>3C1A5F2B-4C2E-4C3C-9F6C-0D4B8F1B2A11</rsp:ShellId><rsp:Owner>CORP\admin</rsp:Owner></rsp:Shell></s:Body></s:Envelope>"#;

    const COMMAND_RESPONSE: &str = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell"><s:Header/><s:Body><rsp:CommandResponse><rsp:CommandId>AB12CD34-0000-1111-2222-333344445555</rsp:CommandId></rsp:CommandResponse></s:Body></s:Envelope>"#;

    // "hello\r\n" and "oops" in base64.
    const RECEIVE_RESPONSE: &str = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell"><s:Body><rsp:ReceiveResponse><rsp:Stream Name="stdout" CommandId="AB12">aGVsbG8NCg==</rsp:Stream><rsp:Stream Name="stderr" CommandId="AB12">b29wcw==</rsp:Stream><rsp:Stream Name="stdout" CommandId="AB12" End="true"/><rsp:CommandState CommandId="AB12" State="http://schemas.microsoft.com/wbem/wsman/1/windows/shell/CommandState/Done"><rsp:ExitCode>3</rsp:ExitCode></rsp:CommandState></rsp:ReceiveResponse></s:Body></s:Envelope>"#;

    const TIMEOUT_FAULT: &str = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:f="http://schemas.microsoft.com/wbem/wsman/1/wsmanfault"><s:Body><s:Fault><s:Code><s:Value>s:Receiver</s:Value><s:Subcode><s:Value>w:TimedOut</s:Value></s:Subcode></s:Code><s:Reason><s:Text xml:lang="en-US">The WS-Management service cannot complete the operation within the time specified in OperationTimeout.</s:Text></s:Reason><s:Detail><f:WSManFault Code="2150858793" Machine="srv.corp"><f:Message>The WS-Management service cannot complete the operation within the time specified in OperationTimeout.  </f:Message></f:WSManFault></s:Detail></s:Fault></s:Body></s:Envelope>"#;

    #[test]
    fn reads_shell_and_command_ids() {
        let r = parse_response(CREATE_RESPONSE).unwrap();
        assert_eq!(
            shell_id(&r).unwrap(),
            "3C1A5F2B-4C2E-4C3C-9F6C-0D4B8F1B2A11"
        );
        let c = parse_response(COMMAND_RESPONSE).unwrap();
        assert_eq!(
            command_id(&c).unwrap(),
            "AB12CD34-0000-1111-2222-333344445555"
        );
        assert!(shell_id(&c).is_err());
    }

    #[test]
    fn reads_output_and_exit_code() {
        let r = received(&parse_response(RECEIVE_RESPONSE).unwrap());
        assert_eq!(r.stdout, b"hello\r\n");
        assert_eq!(r.stderr, b"oops");
        assert!(r.done);
        assert_eq!(r.exit, Some(3));
        let running = r#"<a><b:ReceiveResponse xmlns:b="x"><b:Stream Name="stdout">YQ==</b:Stream><b:CommandState State="http://schemas.microsoft.com/wbem/wsman/1/windows/shell/CommandState/Running"/></b:ReceiveResponse></a>"#;
        let r = received(&parse(running).unwrap());
        assert_eq!(r.stdout, b"a");
        assert!(!r.done && r.exit.is_none());
    }

    #[test]
    fn faults() {
        let root = parse(TIMEOUT_FAULT).unwrap();
        let f = fault(&root).unwrap();
        assert!(f.is_timeout());
        assert!(f.text().contains("OperationTimeout"));
        assert!(parse_response(TIMEOUT_FAULT).is_err());
        assert!(fault(&parse(COMMAND_RESPONSE).unwrap()).is_none());
    }

    #[test]
    fn xml_reader_edge_cases() {
        let r = parse(
            "<?xml version=\"1.0\"?><!-- c --><a x='1 > 0' y=\"&lt;&amp;\"><b>t &amp; &#65;&#x42;</b><![CDATA[<raw>]]><c/></a>",
        )
        .unwrap();
        assert_eq!(r.attr("x"), Some("1 > 0"));
        assert_eq!(r.attr("y"), Some("<&"));
        assert_eq!(r.find("b").unwrap().text, "t & AB");
        assert_eq!(r.text, "<raw>");
        assert!(r.find("c").is_some());
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a>").is_err());
        assert!(parse("").is_err());
        let deep = "<a>".repeat(100);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn requests_are_well_formed() {
        let url = "http://srv:5985/wsman";
        for xml in [
            create_shell(url),
            command(
                url,
                "SHELL1",
                "powershell.exe",
                &["-NoProfile".into(), "a<b&c".into()],
            ),
            receive(url, "SHELL1", "CMD1", 20),
            terminate(url, "SHELL1", "CMD1"),
            delete_shell(url, "SHELL1"),
        ] {
            let root = parse(&xml).unwrap_or_else(|e| panic!("{e}: {xml}"));
            assert_eq!(root.name, "Envelope");
            assert_eq!(root.find("To").unwrap().text, url);
        }
        let x = command(url, "S", "powershell.exe", &["a<b&c".into()]);
        let root = parse(&x).unwrap();
        assert_eq!(root.find("Arguments").unwrap().text, "a<b&c");
        assert_eq!(root.find("Command").unwrap().text, "powershell.exe");
        let sel = root.find("Selector").unwrap();
        assert_eq!(sel.attr("Name"), Some("ShellId"));
        let rx = parse(&receive(url, "S", "C", 20)).unwrap();
        assert_eq!(
            rx.find("DesiredStream").unwrap().attr("CommandId"),
            Some("C")
        );
        assert_eq!(rx.find("OperationTimeout").unwrap().text, "PT20S");
        // Ids are escaped, not trusted.
        let evil = parse(&delete_shell(url, "x</w:Selector><evil/>")).unwrap();
        assert!(evil.find("evil").is_none());
    }

    #[test]
    fn powershell_arguments() {
        let a = powershell_args("Get-Date").unwrap();
        assert_eq!(a[2], "-EncodedCommand");
        let bytes = B64.decode(&a[3]).unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16(&units).unwrap();
        assert!(text.ends_with("Get-Date") && text.contains("OutputEncoding"));
        assert!(powershell_args(&"x".repeat(20_000)).is_err());
        // Non-ASCII survives.
        let a = powershell_args("'Größe €'").unwrap();
        let bytes = B64.decode(&a[3]).unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert!(String::from_utf16(&units).unwrap().contains("Größe €"));
    }

    #[test]
    fn clixml_errors() {
        let s = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\" xmlns=\"http://schemas.microsoft.com/powershell/2004/04\"><S S=\"Error\">Get-Foo : The term is not recognized_x000D__x000A_</S><S S=\"Error\">At line:1_x000D__x000A_</S></Objs>";
        assert_eq!(
            clixml_to_text(s),
            "Get-Foo : The term is not recognized\nAt line:1\n"
        );
        let progress = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\" xmlns=\"x\"><Obj S=\"progress\"><TN><T>System.Management.Automation.PSCustomObject</T></TN></Obj></Objs>";
        assert_eq!(clixml_to_text(progress), "");
        assert_eq!(clixml_to_text("plain"), "plain");
        assert_eq!(unescape_clixml("a_x0041_b_xZZZZ_"), "aAb_xZZZZ_");
    }
}
