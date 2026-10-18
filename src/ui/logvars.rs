//! Placeholders in log commands: `journalctl -u {{service}} -n {{zeilen=200}}` asks for the
//! values when the log is started, remembers the last ones per variable and offers them as
//! one-click suggestions, so one saved command covers every service.

use std::collections::HashMap;

use egui::RichText;
use uuid::Uuid;

use crate::app::{Action, App};
use crate::theme;

/// How many recent values are kept per variable.
const HISTORY: usize = 8;
/// Longest value accepted for one placeholder.
const MAX_VALUE: usize = 256;

/// One `{{name}}` or `{{name=default}}` in a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Placeholder {
    pub name: String,
    pub default: String,
}

/// The distinct placeholders of `cmd` in order of appearance (first default wins).
pub fn placeholders(cmd: &str) -> Vec<Placeholder> {
    let mut out: Vec<Placeholder> = Vec::new();
    let mut rest = cmd;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let (name, default) = match after[..end].split_once('=') {
            Some((n, d)) => (n.trim(), d.trim()),
            None => (after[..end].trim(), ""),
        };
        if !name.is_empty() && !out.iter().any(|p| p.name == name) {
            out.push(Placeholder {
                name: name.to_string(),
                default: default.to_string(),
            });
        }
        rest = &after[end + 2..];
    }
    out
}

/// A value that is safe to put into the command line: one line, no control characters, no
/// further placeholders.
pub fn clean_value(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace("{{", "")
        .replace("}}", "")
        .trim()
        .chars()
        .take(MAX_VALUE)
        .collect()
}

/// `cmd` with every placeholder replaced; unknown or empty ones fall back to their default.
pub fn render(cmd: &str, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut rest = cmd;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push_str(&rest[..start]);
        let (name, default) = match after[..end].split_once('=') {
            Some((n, d)) => (n.trim(), d.trim()),
            None => (after[..end].trim(), ""),
        };
        let value = values
            .get(name)
            .map(|v| clean_value(v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| default.to_string());
        out.push_str(&value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Where the log goes once the values are in.
#[derive(Clone, Debug)]
pub enum Target {
    /// The log panel next to a terminal tab.
    Side(u64),
    /// A log tab for this host (`None` = this PC).
    Tab(Option<Uuid>),
}

/// The "values for the placeholders" dialog.
pub struct LogVarPrompt {
    pub command: String,
    pub target: Target,
    pub values: Vec<(Placeholder, String)>,
}

impl LogVarPrompt {
    /// `None` when the command has no placeholders (start it right away).
    pub fn new(
        command: &str,
        target: Target,
        history: &HashMap<String, Vec<String>>,
    ) -> Option<Self> {
        let ph = placeholders(command);
        if ph.is_empty() {
            return None;
        }
        let values = ph
            .into_iter()
            .map(|p| {
                // The last used value, else the default.
                let v = history
                    .get(&p.name)
                    .and_then(|h| h.first().cloned())
                    .unwrap_or_else(|| p.default.clone());
                (p, v)
            })
            .collect();
        Some(Self {
            command: command.to_string(),
            target,
            values,
        })
    }

    fn map(&self) -> HashMap<String, String> {
        self.values
            .iter()
            .map(|(p, v)| (p.name.clone(), v.clone()))
            .collect()
    }

    /// A placeholder without a default is still empty.
    pub fn incomplete(&self) -> bool {
        self.values
            .iter()
            .any(|(p, v)| p.default.is_empty() && clean_value(v).is_empty())
    }

    pub fn rendered(&self) -> String {
        render(&self.command, &self.map())
    }
}

/// Moves `value` to the front of the variable's recent values.
pub fn remember(history: &mut HashMap<String, Vec<String>>, name: &str, value: &str) {
    let value = clean_value(value);
    if value.is_empty() {
        return;
    }
    let list = history.entry(name.to_string()).or_default();
    list.retain(|v| *v != value);
    list.insert(0, value);
    list.truncate(HISTORY);
}

/// Draws the dialog and starts the log once confirmed.
pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some(mut p) = app.log_vars.take() else {
        return;
    };
    let (mut submit, mut cancel) = (false, false);
    let missing = p.incomplete();
    egui::Modal::new(egui::Id::new("log-vars"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            // Read before the text fields: a focused field consumes Enter.
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            ui.set_width(460.0_f32.min(ui.available_width()));
            ui.label(theme::heading(t!("Werte für den Log-Befehl"), 20.0));
            ui.label(
                RichText::new(t!(
                    "Platzhalter ausfüllen – die letzten Werte merkt sich Aurora"
                ))
                .color(theme::text_dim()),
            );
            ui.add_space(8.0);
            for (i, (ph, value)) in p.values.iter_mut().enumerate() {
                ui.label(theme::mono(format!("{{{{{}}}}}", ph.name), 12.5).color(theme::accent2()));
                let r = ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text(ph.default.clone())
                        .desired_width(f32::INFINITY),
                );
                if i == 0 && !ui.memory(|m| m.focused().is_some()) {
                    r.request_focus();
                }
                if let Some(recent) = app.lib.log_var_history.get(&ph.name) {
                    ui.horizontal_wrapped(|ui| {
                        for v in recent.iter().take(6) {
                            if ui
                                .selectable_label(value == v, theme::mono(v, 12.0))
                                .clicked()
                            {
                                *value = v.clone();
                            }
                        }
                    });
                }
                ui.add_space(6.0);
            }
            egui::Frame::NONE
                .fill(theme::bg_deep())
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(p.rendered())
                                .monospace()
                                .color(theme::accent()),
                        )
                        .wrap(),
                    );
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(!missing, |ui| theme::primary_button(ui, t!("▶ Starten")))
                        .inner
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        cancel = true;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    if cancel {
        return;
    }
    if !submit || p.incomplete() {
        app.log_vars = Some(p);
        return;
    }
    for (ph, v) in &p.values {
        remember(&mut app.lib.log_var_history, &ph.name, v);
    }
    app.save_library();
    let cmd = p.rendered();
    match p.target {
        Target::Side(tab) => app.open_side_log(tab, &cmd, ctx),
        Target::Tab(host) => actions.push(Action::OpenLogNow(host, cmd)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn finds_placeholders_with_defaults() {
        let p = placeholders("journalctl -u {{service}} -n {{ zeilen = 200 }} # {{service}}");
        assert_eq!(
            p,
            vec![
                Placeholder {
                    name: "service".into(),
                    default: String::new()
                },
                Placeholder {
                    name: "zeilen".into(),
                    default: "200".into()
                },
            ]
        );
        assert!(placeholders("tail -F /var/log/syslog").is_empty());
        assert!(placeholders("broken {{ open").is_empty());
    }

    #[test]
    fn renders_values_and_defaults() {
        let cmd = "journalctl -u {{service}} -n {{zeilen=200}}";
        assert_eq!(
            render(cmd, &vals(&[("service", "nginx")])),
            "journalctl -u nginx -n 200"
        );
        assert_eq!(
            render(cmd, &vals(&[("service", "ssh"), ("zeilen", "50")])),
            "journalctl -u ssh -n 50"
        );
        // An empty value falls back to the default.
        assert_eq!(
            render(cmd, &vals(&[("service", "x"), ("zeilen", "  ")])),
            "journalctl -u x -n 200"
        );
    }

    #[test]
    fn values_cannot_inject_lines_or_placeholders() {
        assert_eq!(clean_value("nginx\nrm -rf /"), "nginxrm -rf /");
        assert_eq!(clean_value("a{{b}}c"), "abc");
        assert_eq!(clean_value("\u{1b}[31mx"), "[31mx");
        assert_eq!(clean_value(&"x".repeat(1000)).len(), MAX_VALUE);
        let out = render("cat {{f}}", &vals(&[("f", "a\r\nb")]));
        assert!(!out.contains('\n') && !out.contains('\r'));
    }

    #[test]
    fn history_keeps_recent_unique_values() {
        let mut h = HashMap::new();
        for v in ["a", "b", "c", "a", "", "  "] {
            remember(&mut h, "service", v);
        }
        assert_eq!(h["service"], vec!["a", "c", "b"]);
        for i in 0..20 {
            remember(&mut h, "n", &i.to_string());
        }
        assert_eq!(h["n"].len(), HISTORY);
        assert_eq!(h["n"][0], "19");
    }

    #[test]
    fn prompt_prefills_last_value_or_default() {
        let mut h = HashMap::new();
        remember(&mut h, "service", "sshd");
        let p = LogVarPrompt::new("x {{service}} {{n=5}}", Target::Tab(None), &h).unwrap();
        assert_eq!(p.rendered(), "x sshd 5");
        assert!(LogVarPrompt::new("tail -F f", Target::Tab(None), &h).is_none());
        let empty =
            LogVarPrompt::new("x {{service}} {{n=5}}", Target::Tab(None), &HashMap::new()).unwrap();
        assert!(empty.incomplete());
    }
}
