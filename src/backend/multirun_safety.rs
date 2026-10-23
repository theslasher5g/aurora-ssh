//! Safety rules of "Run on many hosts": placeholder values, dangerous-command detection
//! for several commands in one line, and how much confirmation a run needs. Pure functions.

use std::collections::HashMap;

use anyhow::{Result, bail};

use super::runbook;

/// Placeholders whose values are secrets: masked in the displayed command and in the
/// exports, and removed from every result.
pub fn is_secret_name(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "pass",
        "pwd",
        "secret",
        "token",
        "key",
        "kennwort",
        "geheim",
        "credential",
        "apikey",
    ]
    .iter()
    .any(|w| n.contains(w))
}

/// `(command to run, command to show)`: the second has secret values replaced by `***`.
/// Values must be single-line – a newline would smuggle in a second command.
pub fn render_command(template: &str, values: &[(String, String)]) -> Result<(String, String)> {
    for (name, v) in values {
        if v.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
            bail!(
                "{}",
                t!(
                    "Der Wert von „{}“ darf keinen Zeilenumbruch enthalten",
                    name
                )
            );
        }
    }
    let real: HashMap<String, String> = values.iter().cloned().collect();
    let shown: HashMap<String, String> = values
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                if is_secret_name(k) && !v.is_empty() {
                    "***".to_string()
                } else {
                    v.clone()
                },
            )
        })
        .collect();
    Ok((
        runbook::render(template, &real),
        runbook::render(template, &shown),
    ))
}

/// Values to mask in results.
pub fn secret_values(values: &[(String, String)]) -> Vec<String> {
    values
        .iter()
        .filter(|(k, v)| is_secret_name(k) && v.chars().count() >= 3)
        .map(|(_, v)| v.clone())
        .collect()
}

/// Words that only wrap another command.
const WRAPPERS: &[&str] = &[
    "sudo", "xargs", "env", "nohup", "time", "nice", "ionice", "exec", "command", "doas",
];

/// Splits a line into the commands it chains (`;`, `&&`, `||`, `|`, `&`, line breaks).
fn segments(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let (mut sq, mut dq) = (false, false);
    while let Some(c) = chars.next() {
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            ';' | '\n' | '|' | '&' if !sq && !dq => {
                if matches!(c, '|' | '&') && chars.peek() == Some(&c) {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Why this command (line) deserves a second look, if it does. Looks at every command a
/// line chains and sees through `sudo`, `xargs` & co.
pub fn dangerous(cmd: &str, powershell: bool) -> Option<&'static str> {
    if powershell && let Some(w) = dangerous_powershell(cmd) {
        return Some(w);
    }
    for seg in segments(cmd) {
        let mut words: Vec<&str> = seg.split_whitespace().collect();
        while let Some(&wrapper) = words.first().filter(|w| WRAPPERS.contains(w)) {
            words.remove(0);
            // Options and `VAR=value` assignments of the wrapper.
            while let Some(&w) = words.first() {
                if w.contains('=') && wrapper == "env" {
                    words.remove(0);
                } else if w.starts_with('-') {
                    words.remove(0);
                    let takes_arg = matches!(wrapper, "sudo" | "doas")
                        && matches!(
                            w,
                            "-u" | "-g" | "-U" | "-p" | "-C" | "-D" | "-R" | "-T" | "-h"
                        );
                    if takes_arg && !words.is_empty() {
                        words.remove(0);
                    }
                } else {
                    break;
                }
            }
        }
        if words.is_empty() {
            continue;
        }
        if let Some(w) = crate::model::dangerous_command(&words.join(" ")) {
            return Some(w);
        }
        // `bash -c '…'`, `sh -c "…"`: look inside as well.
        if matches!(words[0], "sh" | "bash" | "zsh" | "dash")
            && let Some(pos) = seg.find(" -c ")
        {
            let inner = seg[pos + 4..].trim().trim_matches(['\'', '"']);
            if let Some(w) = dangerous(inner, false) {
                return Some(w);
            }
        }
    }
    None
}

fn dangerous_powershell(cmd: &str) -> Option<&'static str> {
    let l = cmd.to_lowercase();
    let has = |w: &str| l.contains(w);
    if (has("remove-item") || has("rm ") || has("del ") || has("rd "))
        && (has("-recurse") || has("/s"))
    {
        return Some(t!("löscht rekursiv"));
    }
    if has("stop-computer") || has("restart-computer") || has("shutdown") {
        return Some(t!("fährt den Server herunter oder startet ihn neu"));
    }
    if has("format-volume") || has("clear-disk") || has("diskpart") || has("remove-partition") {
        return Some(t!("formatiert oder löscht Datenträger"));
    }
    if has("stop-service") || has("restart-service") || has("sc stop") || has("net stop") {
        return Some(t!("stoppt oder startet Dienste"));
    }
    if has("remove-vm") || has("remove-adcomputer") || has("remove-aduser") {
        return Some(t!("entfernt Objekte"));
    }
    if has("drop database") || has("drop table") || has("truncate table") {
        return Some(t!("löscht Datenbankinhalte"));
    }
    None
}

/// How much confirmation a run needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// The ordinary confirmation (hosts and command).
    Plain,
    /// PROD hosts are included: an extra confirmation.
    Prod,
    /// A dangerous command on PROD hosts: the user types a word.
    TypeToConfirm,
}

pub fn gate(prod_hosts: usize, dangerous: Option<&str>) -> Gate {
    match (prod_hosts > 0, dangerous.is_some()) {
        (true, true) => Gate::TypeToConfirm,
        (true, false) => Gate::Prod,
        _ => Gate::Plain,
    }
}

/// The word to type for [`Gate::TypeToConfirm`].
pub const CONFIRM_WORD: &str = "PROD";

#[cfg(test)]
mod tests {
    use super::*;

    fn v(k: &str, x: &str) -> (String, String) {
        (k.into(), x.into())
    }

    #[test]
    fn placeholders_and_secrets() {
        let (real, shown) = render_command(
            "mysql -p{{db_password}} -e 'use {{db}}'",
            &[v("db_password", "hunter2"), v("db", "shop")],
        )
        .unwrap();
        assert_eq!(real, "mysql -phunter2 -e 'use shop'");
        assert_eq!(shown, "mysql -p*** -e 'use shop'");
        assert_eq!(
            secret_values(&[
                v("db_password", "hunter2"),
                v("db", "shop"),
                v("token", "ab")
            ]),
            vec!["hunter2".to_string()]
        );
        // A newline would add a second command.
        assert!(render_command("echo {{x}}", &[v("x", "a\nrm -rf /")]).is_err());
        assert!(render_command("echo {{x}}", &[v("x", "a\rb")]).is_err());
        // Unknown placeholders stay visible.
        assert_eq!(render_command("echo {{x}}", &[]).unwrap().0, "echo {{x}}");
        assert!(
            is_secret_name("API_KEY") && is_secret_name("Passwort") && !is_secret_name("service")
        );
    }

    #[test]
    fn dangerous_commands_in_chains() {
        assert!(dangerous("uptime", false).is_none());
        assert!(dangerous("df -h; free -m", false).is_none());
        assert!(dangerous("rm -rf /var/cache/x", false).is_some());
        assert!(dangerous("uptime && rm -rf /tmp/x", false).is_some());
        assert!(dangerous("echo ok; sudo reboot", false).is_some());
        assert!(dangerous("ls | xargs rm -rf", false).is_some());
        assert!(dangerous("sudo -u root systemctl restart nginx", false).is_some());
        assert!(dangerous("env A=1 shutdown -h now", false).is_some());
        assert!(dangerous("bash -c 'rm -rf /x'", false).is_some());
        assert!(dangerous("sh -c \"echo hi\"", false).is_none());
        // Separators inside quotes do not split.
        assert!(dangerous("echo 'a; rm -rf b'", false).is_none());
        assert!(dangerous("systemctl status nginx", false).is_none());
        assert!(dangerous("Get-Service | Where Status -eq Running", true).is_none());
        assert!(dangerous("Restart-Computer -Force", true).is_some());
        assert!(dangerous("Remove-Item C:\\x -Recurse", true).is_some());
        assert!(dangerous("Stop-Service Spooler", true).is_some());
        assert!(dangerous("Format-Volume -DriveLetter D", true).is_some());
    }

    #[test]
    fn gates() {
        assert_eq!(gate(0, None), Gate::Plain);
        assert_eq!(gate(0, Some("x")), Gate::Plain);
        assert_eq!(gate(2, None), Gate::Prod);
        assert_eq!(gate(2, Some("x")), Gate::TypeToConfirm);
    }
}
