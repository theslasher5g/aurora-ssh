//! "Befehlsblöcke": bookkeeping of command blocks (prompt, command, output, exit code).
//!
//! Blocks come from OSC 133 shell-integration marks (`A` prompt start, `B` command start,
//! `C` output start, `D;code` command finished). Without marks the terminal falls back to
//! the prompt heuristic and feeds the same bookkeeping through `heur_prompt`/`heur_enter`.
//! All positions are *absolute* line numbers (line 0 = the first line the terminal ever
//! showed), so they stay valid while the screen scrolls and the scrollback is trimmed.

use std::collections::VecDeque;

/// Blocks kept per terminal.
pub const MAX_BLOCKS: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub id: u64,
    /// First line of the prompt (OSC 133 `A`).
    pub prompt: usize,
    /// Where the typed command starts (`B`): line and column.
    pub cmd_pos: Option<(usize, u16)>,
    /// First line of the output (`C`).
    pub out_start: Option<usize>,
    /// Line after the last output line (`D`); `None` while the command runs.
    pub out_end: Option<usize>,
    pub exit: Option<i32>,
    pub command: String,
    /// Seconds (terminal clock) of the prompt, of the output start and of the end.
    pub t_prompt: f64,
    pub t_out: Option<f64>,
    pub t_end: Option<f64>,
    pub collapsed: bool,
    pub heuristic: bool,
    /// The start of the block was cut off by scrollback trimming.
    pub truncated: bool,
}

impl Block {
    /// A command was started (there is output or at least an output start).
    pub fn ran(&self) -> bool {
        self.out_start.is_some()
    }

    pub fn running(&self) -> bool {
        self.out_start.is_some() && self.out_end.is_none()
    }

    /// Run time of the command: output start to end.
    pub fn duration(&self) -> Option<f64> {
        Some((self.t_end? - self.t_out?).max(0.0))
    }

    /// Output lines `[start, end)`; `live_end` is used while the command still runs.
    pub fn output_lines(&self, live_end: usize) -> Option<(usize, usize)> {
        let s = self.out_start?;
        let e = self.out_end.unwrap_or(live_end).max(s);
        Some((s, e))
    }

    /// All lines of the block (prompt up to the end of the output).
    pub fn span(&self, live_end: usize) -> (usize, usize) {
        let e = self.out_end.unwrap_or(live_end).max(self.prompt + 1);
        (self.prompt, e)
    }

    pub fn failed(&self) -> bool {
        self.exit.is_some_and(|c| c != 0)
    }
}

/// `3,2 s`, `1 min 5 s`, `250 ms`.
pub fn duration_label(secs: f64) -> String {
    let decimal = |v: f64| {
        let s = format!("{v:.1}");
        if crate::i18n::is_english() {
            s
        } else {
            s.replace('.', ",")
        }
    };
    if secs < 1.0 {
        format!("{} ms", (secs * 1000.0).round() as u64)
    } else if secs < 60.0 {
        format!("{} s", decimal(secs))
    } else {
        let s = secs.round() as u64;
        if s >= 3600 {
            format!("{} h {} min", s / 3600, (s % 3600) / 60)
        } else {
            format!("{} min {} s", s / 60, s % 60)
        }
    }
}

#[derive(Default)]
pub struct Blocks {
    pub list: VecDeque<Block>,
    /// The shell sends OSC 133 marks (the heuristic stays off).
    pub osc: bool,
    next_id: u64,
}

/// Line after the last line of output when the cursor is at `(line, col)`.
fn end_excl(line: usize, col: u16) -> usize {
    if col == 0 { line } else { line + 1 }
}

impl Blocks {
    fn push(&mut self, prompt: usize, now: f64, heuristic: bool) -> &mut Block {
        self.next_id += 1;
        self.list.push_back(Block {
            id: self.next_id,
            prompt,
            t_prompt: now,
            heuristic,
            ..Block::default()
        });
        while self.list.len() > MAX_BLOCKS {
            self.list.pop_front();
        }
        self.list.back_mut().expect("just pushed")
    }

    /// The block that is still being prepared or run (the last one, not yet ended).
    fn open(&mut self, line: usize, now: f64, heuristic: bool) -> &mut Block {
        if self.list.back().is_none_or(|b| b.out_end.is_some()) {
            self.push(line, now, heuristic);
        }
        self.list.back_mut().expect("block exists")
    }

    /// An OSC 133 mark: `kind` is `A`, `B`, `C` or `D`, `(line, col)` the cursor, `arg` the
    /// exit code of `D`. `command` reads the typed command from the `B` position up to the
    /// cursor (only called for `C`).
    pub fn osc_mark(
        &mut self,
        kind: u8,
        (line, col): (usize, u16),
        arg: Option<&str>,
        now: f64,
        command: impl FnOnce(Option<(usize, u16)>) -> String,
    ) {
        if !self.osc {
            // The shell speaks OSC 133: forget what the heuristic guessed.
            self.osc = true;
            self.list.retain(|b| !b.heuristic);
        }
        self.apply(kind, (line, col), arg, now, false, command);
    }

    fn apply(
        &mut self,
        kind: u8,
        (line, col): (usize, u16),
        arg: Option<&str>,
        now: f64,
        heuristic: bool,
        command: impl FnOnce(Option<(usize, u16)>) -> String,
    ) {
        match kind {
            b'A' => {
                // A prompt that never ran anything (empty Enter, Ctrl+C) is dropped.
                if self.list.back().is_some_and(|b| !b.ran()) {
                    self.list.pop_back();
                }
                if let Some(b) = self.list.back_mut()
                    && b.running()
                {
                    b.out_end = Some(end_excl(line, col));
                    b.t_end = Some(now);
                }
                // The prompt is above older blocks: the screen was cleared.
                while self.list.back().is_some_and(|b| b.prompt >= line) {
                    self.list.pop_back();
                }
                self.push(line, now, heuristic);
            }
            b'B' => {
                self.open(line, now, heuristic).cmd_pos = Some((line, col));
            }
            b'C' => {
                let b = self.open(line, now, heuristic);
                if b.running() {
                    return;
                }
                b.out_start = Some(if col == 0 { line } else { line + 1 });
                b.out_end = None;
                b.t_out = Some(now);
                b.command = command(b.cmd_pos);
            }
            b'D' => {
                if let Some(b) = self.list.back_mut()
                    && b.running()
                {
                    b.out_end = Some(end_excl(line, col).max(b.out_start.unwrap_or(0)));
                    b.t_end = Some(now);
                    b.exit = arg.and_then(|a| a.trim().parse().ok());
                }
            }
            _ => {}
        }
    }

    /// Heuristic: the cursor sits after a shell prompt on `line` (no OSC 133 marks).
    pub fn heur_prompt(&mut self, line: usize, now: f64) {
        if self.osc {
            return;
        }
        if self
            .list
            .back()
            .is_some_and(|b| b.prompt == line && !b.running())
        {
            return;
        }
        self.apply(b'D', (line, 0), None, now, true, |_| String::new());
        self.apply(b'A', (line, 0), None, now, true, |_| String::new());
    }

    /// Heuristic: Enter was pressed for `command` at the prompt on `prompt_line`; its output
    /// starts on `out_line`.
    pub fn heur_enter(&mut self, prompt_line: usize, out_line: usize, command: &str, now: f64) {
        if self.osc || command.trim().is_empty() {
            return;
        }
        if self.list.back().is_none_or(|b| b.ran()) {
            self.push(prompt_line, now, true);
        }
        let Some(b) = self.list.back_mut() else {
            return;
        };
        b.prompt = b.prompt.min(prompt_line);
        b.out_start = Some(out_line);
        b.out_end = None;
        b.t_out = Some(now);
        b.command = command.trim().to_string();
    }

    /// The block with the command still running, if any.
    pub fn running(&self) -> Option<&Block> {
        self.list.back().filter(|b| b.running())
    }

    /// The shell shows a prompt (nothing runs).
    pub fn at_prompt(&self) -> bool {
        self.running().is_none()
    }

    /// Forgets lines before `oldest` (scrollback was trimmed).
    pub fn trim(&mut self, oldest: usize) {
        while self
            .list
            .front()
            .is_some_and(|b| b.out_end.map_or(b.prompt < oldest, |e| e <= oldest))
        {
            self.list.pop_front();
        }
        for b in self.list.iter_mut().take_while(|b| b.prompt < oldest) {
            b.truncated = true;
        }
    }

    pub fn get(&self, id: u64) -> Option<&Block> {
        self.list.iter().find(|b| b.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Block> {
        self.list.iter_mut().find(|b| b.id == id)
    }

    /// The block that contains `line` (a block with a command only).
    pub fn at_line(&self, line: usize, live_end: usize) -> Option<&Block> {
        self.list.iter().rev().find(|b| {
            let (s, e) = b.span(live_end);
            b.ran() && line >= s && line < e
        })
    }

    /// Start of the previous block before line `before`.
    pub fn prev_start(&self, before: usize) -> Option<usize> {
        self.list
            .iter()
            .filter(|b| b.ran() && b.prompt < before)
            .map(|b| b.prompt)
            .next_back()
    }

    /// Start of the next block after line `after`.
    pub fn next_start(&self, after: usize) -> Option<usize> {
        self.list
            .iter()
            .filter(|b| b.ran() && b.prompt > after)
            .map(|b| b.prompt)
            .next()
    }

    /// The newest block that finished with a non-zero exit code.
    pub fn last_failed(&self) -> Option<&Block> {
        self.list.iter().rev().find(|b| b.failed())
    }

    /// The newest block that has finished.
    pub fn last_done(&self) -> Option<&Block> {
        self.list.iter().rev().find(|b| b.ran() && !b.running())
    }
}

/// Command text from the lines between the `B` position and the cursor. `lines` are
/// `(text, wrapped)` for the lines `from.0 ..= last`; the first line starts at `from.1`.
pub fn join_command(lines: &[(String, bool)], from_col: u16) -> String {
    let mut out = String::new();
    for (i, (text, _)) in lines.iter().enumerate() {
        let part: String = if i == 0 {
            text.chars().skip(from_col as usize).collect()
        } else {
            text.clone()
        };
        out.push_str(&part);
        let wrapped = lines.get(i).is_some_and(|l| l.1);
        if i + 1 < lines.len() && !wrapped {
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// Joins output lines; rows that were wrapped by the terminal are joined without a break.
/// Trailing blank lines are dropped.
pub fn join_output(lines: &[(String, bool)]) -> String {
    let mut out = String::new();
    for (text, wrapped) in lines {
        out.push_str(text.trim_end());
        if !wrapped {
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// A command that is safe to type again: one line, no control characters.
pub fn rerunnable(command: &str) -> Option<String> {
    let c = command.trim();
    (!c.is_empty() && !c.chars().any(|ch| ch.is_control())).then(|| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(b: &mut Blocks, kind: u8, line: usize, col: u16, arg: Option<&str>, t: f64) {
        b.osc_mark(kind, (line, col), arg, t, |_| "cmd".into());
    }

    #[test]
    fn osc_cycle() {
        let mut b = Blocks::default();
        mark(&mut b, b'A', 0, 0, None, 0.0);
        mark(&mut b, b'B', 0, 2, None, 0.0);
        mark(&mut b, b'C', 1, 0, None, 1.0);
        assert!(b.running().is_some());
        mark(&mut b, b'D', 4, 0, Some("0"), 4.2);
        mark(&mut b, b'A', 4, 0, None, 4.2);
        mark(&mut b, b'B', 4, 2, None, 4.2);
        mark(&mut b, b'C', 5, 0, None, 5.0);
        mark(&mut b, b'D', 6, 3, Some("2"), 5.5);
        let first = &b.list[0];
        assert_eq!(first.output_lines(99), Some((1, 4)));
        assert_eq!(first.exit, Some(0));
        assert_eq!(first.command, "cmd");
        assert!((first.duration().unwrap() - 3.2).abs() < 1e-9);
        // The partial last line counts as output.
        assert_eq!(b.list[1].output_lines(99), Some((5, 7)));
        assert!(b.list[1].failed());
        assert_eq!(b.last_failed().unwrap().id, b.list[1].id);
        assert_eq!(b.prev_start(5), Some(4));
        assert_eq!(b.prev_start(4), Some(0));
        assert_eq!(b.prev_start(0), None);
        assert_eq!(b.next_start(0), Some(4));
        assert_eq!(b.next_start(4), None);
    }

    #[test]
    fn empty_prompts_and_clear_screen() {
        let mut b = Blocks::default();
        mark(&mut b, b'A', 0, 0, None, 0.0);
        // Empty Enter: the next prompt replaces the unused one.
        mark(&mut b, b'A', 1, 0, None, 1.0);
        assert_eq!(b.list.len(), 1);
        assert_eq!(b.list[0].prompt, 1);
        mark(&mut b, b'C', 2, 0, None, 2.0);
        mark(&mut b, b'D', 3, 0, Some("0"), 2.1);
        mark(&mut b, b'A', 3, 0, None, 2.1);
        mark(&mut b, b'C', 4, 0, None, 3.0);
        mark(&mut b, b'D', 4, 0, Some("0"), 3.1);
        // "clear": the next prompt is at the top again.
        mark(&mut b, b'A', 0, 0, None, 3.1);
        assert_eq!(b.list.len(), 1);
        assert_eq!(b.list[0].prompt, 0);
        // D without a running command is ignored.
        mark(&mut b, b'D', 0, 0, Some("1"), 3.2);
        assert!(b.list[0].exit.is_none());
    }

    #[test]
    fn missing_end_is_closed_by_next_prompt() {
        let mut b = Blocks::default();
        mark(&mut b, b'A', 0, 0, None, 0.0);
        mark(&mut b, b'C', 1, 0, None, 0.5);
        mark(&mut b, b'A', 5, 0, None, 1.0);
        assert_eq!(b.list[0].out_end, Some(5));
        assert_eq!(b.list[0].exit, None);
    }

    #[test]
    fn trimming_keeps_what_is_left() {
        let mut b = Blocks::default();
        for i in 0..5usize {
            let l = i * 10;
            mark(&mut b, b'A', l, 0, None, 0.0);
            mark(&mut b, b'C', l + 1, 0, None, 0.0);
            mark(&mut b, b'D', l + 9, 0, Some("0"), 0.1);
        }
        assert_eq!(b.list.len(), 5);
        // Lines before 25 are gone: blocks 0 and 1 end before it, block 2 is cut.
        b.trim(25);
        assert_eq!(b.list.len(), 3);
        assert!(b.list[0].truncated);
        assert_eq!(b.list[0].prompt, 20);
        assert!(!b.list[1].truncated);
        b.trim(1000);
        assert!(b.list.is_empty());
    }

    #[test]
    fn heuristic_blocks_and_osc_takeover() {
        let mut b = Blocks::default();
        b.heur_prompt(0, 0.0);
        b.heur_prompt(0, 0.1);
        assert_eq!(b.list.len(), 1);
        b.heur_enter(0, 1, "ls -l", 1.0);
        assert!(b.running().is_some());
        b.heur_prompt(4, 2.0);
        assert_eq!(b.list.len(), 2);
        assert_eq!(b.list[0].output_lines(9), Some((1, 4)));
        assert_eq!(b.list[0].command, "ls -l");
        assert!(b.list[0].heuristic && b.list[0].exit.is_none());
        // The shell starts to send marks: heuristic blocks are dropped.
        mark(&mut b, b'A', 4, 0, None, 3.0);
        assert!(b.osc);
        assert!(b.list.iter().all(|x| !x.heuristic));
        b.heur_prompt(9, 4.0);
        assert_eq!(b.list.len(), 1);
    }

    #[test]
    fn text_helpers() {
        assert_eq!(duration_label(0.25), "250 ms");
        assert_eq!(duration_label(3.24).replace(',', "."), "3.2 s");
        assert_eq!(duration_label(75.0), "1 min 15 s");
        assert_eq!(duration_label(3700.0), "1 h 1 min");
        let lines = vec![
            ("user@vm:~$ echo hi".to_string(), false),
            ("second".to_string(), false),
        ];
        assert_eq!(join_command(&lines, 10), "echo hi\nsecond");
        let wrapped = vec![("abc".to_string(), true), ("def  ".to_string(), false)];
        assert_eq!(join_output(&wrapped), "abcdef");
        assert_eq!(rerunnable(" ls -l "), Some("ls -l".into()));
        assert_eq!(rerunnable("a\nb"), None);
        assert_eq!(rerunnable("a\x1b[2Jb"), None);
        assert_eq!(rerunnable("  "), None);
    }
}
