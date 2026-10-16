//! Telnet terminal: plain TCP with the option negotiation a terminal needs (echo,
//! suppress-go-ahead, window size, terminal type). Optional auto-login with the stored
//! user name and password when the server asks for them.

use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::{Sink, TermCmd, TermEvent};

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const ECHO: u8 = 1;
const SGA: u8 = 3;
const TTYPE: u8 = 24;
const NAWS: u8 = 31;

#[derive(Clone, Debug)]
pub struct TelnetParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    params: TelnetParams,
    cols: u16,
    rows: u16,
    events: Sink<TermEvent>,
) -> mpsc::UnboundedSender<TermCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(async move {
        let r = run(&params, cols, rows, rx, &events).await;
        events.send(TermEvent::Closed(r.err().map(|e| format!("{e:#}"))));
    });
    tx
}

async fn run(
    p: &TelnetParams,
    cols: u16,
    rows: u16,
    mut cmds: mpsc::UnboundedReceiver<TermCmd>,
    events: &Sink<TermEvent>,
) -> Result<()> {
    events.send(TermEvent::Status(t!("Verbinde mit {}:{}…", p.host, p.port)));
    let stream = tokio::time::timeout(
        Duration::from_secs(10),
        TcpStream::connect((p.host.as_str(), p.port)),
    )
    .await
    .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
    .map_err(|e| super::ssh::friendly_error(e.into()))?;
    let _ = stream.set_nodelay(true);
    events.send(TermEvent::Connected);
    let (mut rd, mut wr) = stream.into_split();
    let mut proto = Telnet::new(cols, rows);
    let mut login = AutoLogin::new(&p.username, p.password.as_deref());
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(TermCmd::Input(bytes)) => wr.write_all(&escape(&bytes)).await?,
                Some(TermCmd::Resize { cols, rows }) => {
                    if let Some(msg) = proto.resize(cols, rows) {
                        wr.write_all(&msg).await?;
                    }
                }
                Some(TermCmd::Upload { .. }) => {}
                Some(TermCmd::Close) | None => break,
            },
            n = rd.read(&mut buf) => {
                let n = n?;
                if n == 0 {
                    break;
                }
                let (data, reply) = proto.receive(&buf[..n]);
                if !reply.is_empty() {
                    wr.write_all(&reply).await?;
                }
                if !data.is_empty() {
                    if let Some(answer) = login.feed(&data) {
                        wr.write_all(&escape(&answer)).await?;
                    }
                    events.send(TermEvent::Output(data));
                }
            }
        }
    }
    Ok(())
}

/// Doubles IAC bytes in user data.
fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        }
    }
    out
}

#[derive(PartialEq)]
enum State {
    Data,
    Iac,
    /// After WILL/WONT/DO/DONT.
    Opt(u8),
    Sub,
    SubIac,
}

/// Telnet protocol state machine: strips commands from the data stream and builds replies.
struct Telnet {
    state: State,
    sub: Vec<u8>,
    cols: u16,
    rows: u16,
    naws: bool,
    /// Options we already answered, so we never loop on the server's acknowledgements.
    answered: [Option<u8>; 256],
}

impl Telnet {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            state: State::Data,
            sub: Vec::new(),
            cols,
            rows,
            naws: false,
            answered: [None; 256],
        }
    }

    fn naws_msg(&self) -> Vec<u8> {
        let mut m = vec![IAC, SB, NAWS];
        for b in self
            .cols
            .to_be_bytes()
            .into_iter()
            .chain(self.rows.to_be_bytes())
        {
            m.push(b);
            if b == IAC {
                m.push(IAC);
            }
        }
        m.extend_from_slice(&[IAC, SE]);
        m
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.cols = cols;
        self.rows = rows;
        self.naws.then(|| self.naws_msg())
    }

    fn answer(&mut self, out: &mut Vec<u8>, verb: u8, opt: u8) {
        if self.answered[opt as usize] == Some(verb) {
            return;
        }
        self.answered[opt as usize] = Some(verb);
        out.extend_from_slice(&[IAC, verb, opt]);
    }

    /// Returns (terminal data, bytes to send back).
    fn receive(&mut self, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::with_capacity(input.len());
        let mut reply = Vec::new();
        for &b in input {
            match self.state {
                State::Data => {
                    if b == IAC {
                        self.state = State::Iac;
                    } else {
                        data.push(b);
                    }
                }
                State::Iac => {
                    self.state = match b {
                        IAC => {
                            data.push(IAC);
                            State::Data
                        }
                        WILL | WONT | DO | DONT => State::Opt(b),
                        SB => {
                            self.sub.clear();
                            State::Sub
                        }
                        _ => State::Data,
                    }
                }
                State::Opt(verb) => {
                    self.state = State::Data;
                    match (verb, b) {
                        // The server echoes and suppresses go-ahead: that's character mode.
                        (WILL, ECHO | SGA) => self.answer(&mut reply, DO, b),
                        (WILL, _) => self.answer(&mut reply, DONT, b),
                        (DO, SGA | TTYPE) => self.answer(&mut reply, WILL, b),
                        (DO, NAWS) => {
                            if self.answered[NAWS as usize] != Some(WILL) {
                                self.answer(&mut reply, WILL, NAWS);
                            }
                            self.naws = true;
                            reply.extend(self.naws_msg());
                        }
                        (DO, _) => self.answer(&mut reply, WONT, b),
                        (WONT, _) => self.answer(&mut reply, DONT, b),
                        (DONT, _) => {
                            if b == NAWS {
                                self.naws = false;
                            }
                            self.answer(&mut reply, WONT, b)
                        }
                        _ => {}
                    }
                }
                State::Sub => {
                    if b == IAC {
                        self.state = State::SubIac;
                    } else if self.sub.len() < 256 {
                        self.sub.push(b);
                    }
                }
                State::SubIac => {
                    if b == SE {
                        self.state = State::Data;
                        // TTYPE SEND → TTYPE IS xterm-256color
                        if self.sub.first() == Some(&TTYPE) && self.sub.get(1) == Some(&1) {
                            reply.extend_from_slice(&[IAC, SB, TTYPE, 0]);
                            reply.extend_from_slice(b"XTERM-256COLOR");
                            reply.extend_from_slice(&[IAC, SE]);
                        }
                    } else {
                        if b == IAC && self.sub.len() < 256 {
                            self.sub.push(IAC);
                        }
                        self.state = State::Sub;
                    }
                }
            }
        }
        (data, reply)
    }
}

/// Types the stored user name / password once when the matching prompt shows up.
struct AutoLogin {
    user: Option<String>,
    password: Option<String>,
    tail: String,
}

impl AutoLogin {
    fn new(user: &str, password: Option<&str>) -> Self {
        let user = (!user.trim().is_empty()).then(|| user.trim().to_string());
        Self {
            // Without a user name the password is only typed if asked for directly.
            password: password.filter(|p| !p.is_empty()).map(str::to_string),
            user,
            tail: String::new(),
        }
    }

    fn feed(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if self.user.is_none() && self.password.is_none() {
            return None;
        }
        self.tail.push_str(&String::from_utf8_lossy(data));
        if self.tail.len() > 200 {
            let cut = self.tail.len() - 200;
            let cut = (cut..self.tail.len())
                .find(|i| self.tail.is_char_boundary(*i))
                .unwrap_or(0);
            self.tail.drain(..cut);
        }
        let line = self
            .tail
            .rsplit(['\n', '\r'])
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_lowercase();
        let asks = |words: &[&str]| words.iter().any(|w| line.ends_with(w));
        if asks(&[
            "login:",
            "username:",
            "user name:",
            "user:",
            "benutzername:",
        ]) && let Some(u) = self.user.take()
        {
            self.tail.clear();
            return Some(format!("{u}\r\n").into_bytes());
        }
        if asks(&["password:", "passwort:", "kennwort:"])
            && let Some(p) = self.password.take()
        {
            self.user = None;
            self.tail.clear();
            return Some(format!("{p}\r\n").into_bytes());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation() {
        let mut t = Telnet::new(100, 30);
        let (data, reply) = t.receive(&[b'h', IAC, DO, NAWS, IAC, WILL, ECHO, b'i', IAC, IAC]);
        assert_eq!(data, vec![b'h', b'i', IAC]);
        assert_eq!(
            reply,
            vec![
                IAC, WILL, NAWS, IAC, SB, NAWS, 0, 100, 0, 30, IAC, SE, IAC, DO, ECHO
            ]
        );
        // Repeated WILL ECHO is not answered again (no negotiation loop).
        let (_, reply) = t.receive(&[IAC, WILL, ECHO]);
        assert!(reply.is_empty());
        let (_, reply) = t.receive(&[IAC, SB, TTYPE, 1, IAC, SE]);
        assert!(reply.starts_with(&[IAC, SB, TTYPE, 0, b'X']));
        assert_eq!(t.resize(120, 40).unwrap()[3..7], [0, 120, 0, 40]);
        // Split across reads.
        let (d1, _) = t.receive(&[b'a', IAC]);
        let (d2, _) = t.receive(&[DO, 99, b'b']);
        assert_eq!((d1, d2), (vec![b'a'], vec![b'b']));
        assert_eq!(escape(&[1, IAC, 2]), vec![1, IAC, IAC, 2]);
    }

    #[test]
    fn auto_login() {
        let mut l = AutoLogin::new("admin", Some("pw"));
        assert_eq!(l.feed(b"Welcome\r\n"), None);
        assert_eq!(l.feed(b"router login: "), Some(b"admin\r\n".to_vec()));
        assert_eq!(l.feed(b"Password: "), Some(b"pw\r\n".to_vec()));
        assert_eq!(l.feed(b"login: "), None);
    }
}
