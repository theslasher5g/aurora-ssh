//! Shell integration for the command blocks: copyable snippets for bash, zsh and fish
//! (OSC 133 prompt marks and OSC 7 working directory) and the optional automatic setup
//! after login. Aurora never touches rc files: the automatic setup only runs a one-liner
//! in the running shell, hidden from the screen, and is lost when the session ends.

use std::time::{Duration, Instant};

use egui::RichText;

use crate::app::App;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    pub const ALL: [Shell; 3] = [Shell::Bash, Shell::Zsh, Shell::Fish];

    pub fn name(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
        }
    }

    /// File the snippet is meant for.
    pub fn rc_file(self) -> &'static str {
        match self {
            Shell::Bash => "~/.bashrc",
            Shell::Zsh => "~/.zshrc",
            Shell::Fish => "~/.config/fish/config.fish",
        }
    }

    /// Snippet for the rc file.
    pub fn snippet(self) -> &'static str {
        match self {
            Shell::Bash => {
                "# Aurora: Befehlsblöcke (OSC 133) und Arbeitsordner (OSC 7)
__aurora_prompt() {
  local e=$?
  printf '\\033]133;D;%s\\007\\033]7;file://%s%s\\007\\033]133;A\\007' \"$e\" \"$HOSTNAME\" \"${PWD// /%20}\"
}
PROMPT_COMMAND=\"__aurora_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"
PS1=\"${PS1}\\[\\033]133;B\\007\\]\"
PS0=\"\\033]133;C\\007\"
"
            }
            Shell::Zsh => {
                "# Aurora: Befehlsblöcke (OSC 133) und Arbeitsordner (OSC 7)
__aurora_precmd() {
  printf '\\033]133;D;%s\\007\\033]7;file://%s%s\\007\\033]133;A\\007' \"$?\" \"$HOST\" \"${PWD// /%20}\"
}
__aurora_preexec() { printf '\\033]133;C\\007'; }
precmd_functions=(__aurora_precmd $precmd_functions)
preexec_functions+=(__aurora_preexec)
PS1=\"${PS1}%{\"$'\\033]133;B\\007'\"%}\"
"
            }
            Shell::Fish => {
                "# Aurora: Befehlsblöcke (OSC 133) und Arbeitsordner (OSC 7)
function __aurora_postexec --on-event fish_postexec
    set -g __aurora_status $status
end
function __aurora_prompt --on-event fish_prompt
    printf '\\033]133;D;%s\\007\\033]7;file://%s%s\\007\\033]133;A\\007' \"$__aurora_status\" $hostname (string replace -a ' ' %20 -- $PWD)
end
function __aurora_preexec --on-event fish_preexec
    printf '\\033]133;C\\007'
end
functions -q __aurora_orig_prompt; or functions -c fish_prompt __aurora_orig_prompt
function fish_prompt
    __aurora_orig_prompt
    printf '\\033]133;B\\007'
end
"
            }
        }
    }

    /// The same as one line for the automatic setup. The leading space keeps it out of the
    /// history (`HISTCONTROL=ignorespace`); the last command prints the marker the
    /// automatic setup waits for. Static text: nothing from the host goes into it.
    pub fn one_liner(self) -> String {
        let body = match self {
            Shell::Bash => {
                " [ -n \"$__AURORA_SI\" ] || { __AURORA_SI=1; __aurora_prompt() { local e=$?; printf '\\033]133;D;%s\\007\\033]7;file://%s%s\\007\\033]133;A\\007' \"$e\" \"$HOSTNAME\" \"${PWD// /%20}\"; }; PROMPT_COMMAND=\"__aurora_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"; PS1=\"${PS1}\\[\\033]133;B\\007\\]\"; PS0=\"\\033]133;C\\007\"; }; echo \"AUR\"\"SH-DONE\""
            }
            Shell::Zsh => {
                " [[ -n \"$__AURORA_SI\" ]] || { __AURORA_SI=1; __aurora_precmd() { printf '\\033]133;D;%s\\007\\033]7;file://%s%s\\007\\033]133;A\\007' \"$?\" \"$HOST\" \"${PWD// /%20}\"; }; __aurora_preexec() { printf '\\033]133;C\\007'; }; precmd_functions=(__aurora_precmd $precmd_functions); preexec_functions+=(__aurora_preexec); PS1=\"${PS1}%{\"$'\\033]133;B\\007'\"%}\"; }; echo \"AUR\"\"SH-DONE\""
            }
            Shell::Fish => {
                " set -q __AURORA_SI; or begin; set -g __AURORA_SI 1; function __aurora_postexec --on-event fish_postexec; set -g __aurora_status $status; end; function __aurora_prompt --on-event fish_prompt; printf '\\033]133;D;%s\\007\\033]7;file://%s%s\\007\\033]133;A\\007' \"$__aurora_status\" $hostname (string replace -a ' ' %20 -- $PWD); end; function __aurora_preexec --on-event fish_preexec; printf '\\033]133;C\\007'; end; functions -c fish_prompt __aurora_orig_prompt; function fish_prompt; __aurora_orig_prompt; printf '\\033]133;B\\007'; end; end; echo \"AUR\"\"SH-DONE\""
            }
        };
        body.to_string()
    }
}

/// Typed first to ask which shell runs: prints `AURSH:<bash>|<zsh>|<fish>` with the version
/// variables of the three shells (the unset ones are empty). The marker is split in the
/// typed text so that only the answer contains it in one piece.
pub const PROBE: &str = " echo \"AUR\"\"SH:$BASH_VERSION|$ZSH_VERSION|$FISH_VERSION\"";

/// The shell named in the output of `PROBE`.
pub fn parse_probe(output: &str) -> Option<Shell> {
    let at = output.find("AURSH:")? + "AURSH:".len();
    let line = output[at..].lines().next()?;
    let mut parts = line.trim().split('|');
    let (b, z, f) = (parts.next()?, parts.next()?, parts.next()?);
    [(b, Shell::Bash), (z, Shell::Zsh), (f, Shell::Fish)]
        .into_iter()
        .find(|(v, _)| !v.trim().is_empty())
        .map(|(_, s)| s)
}

/// Erases the line that shows the prompt which is about to be replaced.
const CLEAR_LINE: &[u8] = b"\r\x1b[2K";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Off,
    Wait,
    Probe,
    Setup,
    Done,
}

/// Automatic shell integration of one terminal: waits for the prompt, asks which shell
/// runs, sends the one-liner and hides everything the shell prints meanwhile.
pub struct AutoSetup {
    stage: Stage,
    since: Instant,
    buf: Vec<u8>,
    /// Input the session should send (drained by the caller).
    pub outbox: Vec<Vec<u8>>,
}

impl AutoSetup {
    pub fn new(enabled: bool) -> Self {
        Self {
            stage: if enabled { Stage::Wait } else { Stage::Off },
            since: Instant::now(),
            buf: Vec::new(),
            outbox: Vec::new(),
        }
    }

    /// Output is currently held back.
    pub fn swallowing(&self) -> bool {
        matches!(self.stage, Stage::Probe | Stage::Setup)
    }

    /// Waiting for the first prompt.
    pub fn waiting(&self) -> bool {
        self.stage == Stage::Wait
    }

    /// Gives up (the server already speaks OSC 133, the session closed …).
    pub fn cancel(&mut self) -> Vec<u8> {
        self.stage = Stage::Done;
        std::mem::take(&mut self.buf)
    }

    /// Call every frame. `ready`: connected, output has settled and the cursor sits at a
    /// prompt. Returns held-back output that has to be shown after all (timeout).
    pub fn tick(&mut self, ready: bool) -> Vec<u8> {
        match self.stage {
            Stage::Wait => {
                if ready {
                    self.outbox.push(format!("{PROBE}\r").into_bytes());
                    self.stage = Stage::Probe;
                    self.since = Instant::now();
                } else if self.since.elapsed() > Duration::from_secs(25) {
                    self.stage = Stage::Done;
                }
            }
            Stage::Probe | Stage::Setup if self.since.elapsed() > Duration::from_secs(6) => {
                return self.cancel();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Output from the session: returns what is to be shown.
    pub fn absorb(&mut self, data: Vec<u8>) -> Vec<u8> {
        if !self.swallowing() {
            return data;
        }
        self.buf.extend_from_slice(&data);
        if self.buf.len() > 64 << 10 {
            return self.cancel();
        }
        let text = String::from_utf8_lossy(&self.buf).into_owned();
        match self.stage {
            Stage::Probe => {
                // The answer line has to be complete.
                let Some(i) = text.find("AURSH:") else {
                    return Vec::new();
                };
                let Some(nl) = text[i..].find('\n') else {
                    return Vec::new();
                };
                let end = i + nl + 1;
                match parse_probe(&text[i..end]) {
                    Some(shell) => {
                        self.buf = self.buf.split_off(end.min(self.buf.len()));
                        self.outbox
                            .push(format!("{}\r", shell.one_liner()).into_bytes());
                        self.stage = Stage::Setup;
                        self.since = Instant::now();
                        Vec::new()
                    }
                    None => {
                        let rest = self.buf.split_off(end.min(self.buf.len()));
                        self.stage = Stage::Done;
                        self.buf.clear();
                        let mut out = CLEAR_LINE.to_vec();
                        out.extend(rest);
                        out
                    }
                }
            }
            Stage::Setup => {
                let Some(i) = text.find("AURSH-DONE") else {
                    return Vec::new();
                };
                let Some(nl) = text[i..].find('\n') else {
                    return Vec::new();
                };
                let end = (i + nl + 1).min(self.buf.len());
                let rest = self.buf.split_off(end);
                self.stage = Stage::Done;
                self.buf.clear();
                let mut out = CLEAR_LINE.to_vec();
                out.extend(rest);
                out
            }
            _ => std::mem::take(&mut self.buf),
        }
    }
}

/// Settings card "Shell-Integration": the switch for the blocks and the snippets.
pub fn settings_card(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, t!("Shell-Integration"));
    });
    ui.add_space(2.0);
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Befehlsblöcke"),
            Some(t!(
                "Markiert Befehl und Ausgabe jedes Befehls am linken Rand: Kopieren, Wiederholen, Einklappen, Ctrl+↑/↓ springt zwischen Befehlen"
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.command_blocks).changed();
            },
        );
        rows.full(ui, |ui| {
            ui.label(
                RichText::new(t!(
                    "Mit diesen Zeilen meldet die Shell Prompt, Befehl, Ausgabe und Exit-Code (OSC 133) sowie den Arbeitsordner (OSC 7). Ohne sie rät Aurora die Blöcke am Prompt. Aurora ändert keine Konfigurationsdateien: Einfügen in die Datei der Shell oder pro Host „Shell-Integration beim Start automatisch einrichten“ einschalten."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
        });
        for shell in Shell::ALL {
            rows.full(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(shell.name()).size(14.0).color(theme::text()));
                    ui.label(
                        RichText::new(shell.rc_file())
                            .font(egui::FontId::monospace(12.0))
                            .color(theme::text_dim()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::btn(
                            ui,
                            theme::Btn::Small,
                            Some(crate::ui::icons::Icon::Copy),
                            t!("Kopieren"),
                        )
                        .clicked()
                        {
                            ui.ctx().copy_text(shell.snippet().to_string());
                        }
                    });
                });
                egui::Frame::NONE
                    .fill(theme::bg_panel())
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::horizontal()
                            .id_salt(("si-snippet", shell.name()))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(shell.snippet().trim_end())
                                            .font(egui::FontId::monospace(11.5))
                                            .color(theme::text()),
                                    )
                                    .extend(),
                                );
                            });
                    });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_names_the_shell() {
        assert_eq!(
            parse_probe("AURSH:5.1.16(1)-release||\r\n"),
            Some(Shell::Bash)
        );
        assert_eq!(parse_probe("AURSH:|5.9|\r\n"), Some(Shell::Zsh));
        assert_eq!(parse_probe("AURSH:||3.7.0\r\n"), Some(Shell::Fish));
        assert_eq!(parse_probe("AURSH:||\r\n"), None);
        assert_eq!(parse_probe("nothing"), None);
        // The typed command line itself never contains the marker in one piece.
        assert!(!PROBE.contains("AURSH"));
        for s in Shell::ALL {
            assert!(!s.one_liner().contains("AURSH-DONE"));
        }
    }

    #[test]
    fn one_liners_are_single_safe_lines() {
        for s in Shell::ALL {
            let l = s.one_liner();
            assert!(
                l.starts_with(' '),
                "{s:?}: leading space keeps it out of the history"
            );
            assert!(!l.contains('\n') && !l.contains('\r'), "{s:?}");
            assert!(l.is_ascii(), "{s:?}");
            assert!(l.contains("133;A") && l.contains("133;B") && l.contains("133;C"));
            assert!(l.contains("133;D") && l.contains("]7;file://"));
            // Does not modify rc files or run anything remote-controlled.
            for bad in [
                ".bashrc",
                ".zshrc",
                "config.fish",
                ">>",
                "curl",
                "wget",
                "sudo",
            ] {
                assert!(!l.contains(bad), "{s:?} {bad}");
            }
            assert!(!s.snippet().is_empty());
        }
    }

    #[test]
    fn auto_setup_flow_hides_the_exchange() {
        let mut a = AutoSetup::new(true);
        assert!(a.waiting());
        assert!(a.tick(false).is_empty());
        assert!(a.outbox.is_empty());
        // Prompt is ready: the probe goes out and output is held back.
        assert!(a.tick(true).is_empty());
        assert_eq!(a.outbox.len(), 1);
        assert!(String::from_utf8_lossy(&a.outbox[0]).starts_with(" echo"));
        assert!(a.swallowing());
        // Echo of the typed line and the answer arrive in pieces.
        assert!(
            a.absorb(b" echo \"AUR\"\"SH:$BASH_VERSION|$Z".to_vec())
                .is_empty()
        );
        assert!(
            a.absorb(b"SH_VERSION|$FISH_VERSION\"\r\nAURSH:5.2||".to_vec())
                .is_empty()
        );
        assert_eq!(a.outbox.len(), 1);
        assert!(a.absorb(b"\r\nu@h:~$ ".to_vec()).is_empty());
        // Now the bash one-liner was queued.
        assert_eq!(a.outbox.len(), 2);
        assert!(String::from_utf8_lossy(&a.outbox[1]).contains("PROMPT_COMMAND"));
        assert!(
            a.absorb(b" [ -n \"$__AURORA_SI\" ] || {...}; echo \"AUR\"\"SH-DONE\"\r\n".to_vec())
                .is_empty()
        );
        let shown = a.absorb(b"AURSH-DONE\r\n\x1b]133;A\x07u@h:~$ ".to_vec());
        assert_eq!(shown, b"\r\x1b[2K\x1b]133;A\x07u@h:~$ ");
        assert!(!a.swallowing());
        // Afterwards output passes through untouched.
        assert_eq!(a.absorb(b"hello".to_vec()), b"hello");
    }

    #[test]
    fn unknown_shell_and_timeouts_show_everything() {
        let mut a = AutoSetup::new(true);
        a.tick(true);
        let shown = a.absorb(b"AURSH:||\r\n$ ".to_vec());
        assert_eq!(shown, b"\r\x1b[2K$ ");
        assert_eq!(a.outbox.len(), 1);
        let mut off = AutoSetup::new(false);
        assert!(!off.waiting() && !off.swallowing());
        assert!(off.tick(true).is_empty() && off.outbox.is_empty());
        // Cancel hands back what was held.
        let mut a = AutoSetup::new(true);
        a.tick(true);
        a.absorb(b"partial".to_vec());
        assert_eq!(a.cancel(), b"partial");
    }
}
