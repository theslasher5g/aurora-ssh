//! Log viewer tab: a followed file or command with filter, levels, pause and highlighting.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Margin, RichText, Vec2};

use crate::app::{App, ConnState, LogTab, SideLog, TabKind};
use crate::theme;
use crate::ui::icons::Icon;

/// Lines kept per log tab; older ones are dropped.
pub const MAX_LINES: usize = 20_000;

/// 3 error, 2 warning, 1 normal, 0 debug/trace.
pub fn line_level(line: &str) -> u8 {
    let low = line.to_ascii_lowercase();
    let word = |w: &str| {
        low.match_indices(w).any(|(i, _)| {
            let before = low[..i].chars().next_back();
            let after = low[i + w.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_alphanumeric())
                && !after.is_some_and(|c| c.is_ascii_alphabetic())
        })
    };
    if [
        "error",
        "err",
        "fatal",
        "crit",
        "critical",
        "panic",
        "emerg",
        "alert",
        "exception",
        "failed",
        "failure",
        "traceback",
    ]
    .iter()
    .any(|w| word(w))
    {
        3
    } else if ["warn", "warning", "deprecated"].iter().any(|w| word(w)) {
        2
    } else if ["debug", "trace", "dbg"].iter().any(|w| word(w)) {
        0
    } else {
        1
    }
}

/// Filter terms: `a | b` matches either, case-insensitive.
fn terms(filter: &str) -> Vec<String> {
    filter
        .split('|')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn matches(line: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let low = line.to_lowercase();
    terms.iter().any(|t| low.contains(t.as_str()))
}

/// Indices of the lines to show.
fn visible(t: &LogTab) -> Vec<usize> {
    let terms = terms(&t.filter);
    let min_level = match t.level {
        2 => 3,
        1 => 2,
        _ => 0,
    };
    let end = if t.paused {
        t.lines.len().saturating_sub(t.paused_at)
    } else {
        t.lines.len()
    };
    (0..end)
        .filter(|&i| {
            let l = &t.lines[i];
            (min_level == 0 || line_level(l) >= min_level)
                && (!t.only_matches || matches(l, &terms))
        })
        .collect()
}

fn level_color(level: u8) -> Color32 {
    match level {
        3 => theme::red(),
        2 => theme::amber(),
        0 => theme::text_dim(),
        _ => theme::text(),
    }
}

/// The line with filter hits marked.
fn layout(line: &str, terms: &[String], font: &FontId, wrap: Option<f32>) -> LayoutJob {
    let color = level_color(line_level(line));
    let mut job = LayoutJob::default();
    if let Some(w) = wrap {
        job.wrap.max_width = w;
    }
    let plain = TextFormat::simple(font.clone(), color);
    let hit = TextFormat {
        background: theme::accent().gamma_multiply(0.35),
        ..TextFormat::simple(font.clone(), theme::text())
    };
    // Find hits on the lowercase copy; only ASCII lowercasing keeps byte offsets aligned.
    let low = line.to_ascii_lowercase();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for t in terms {
        if !t.is_ascii() {
            continue;
        }
        let mut from = 0;
        while let Some(i) = low[from..].find(t.as_str()) {
            spans.push((from + i, from + i + t.len()));
            from += i + t.len().max(1);
        }
    }
    spans.sort();
    let mut pos = 0;
    for (a, b) in spans {
        if a < pos {
            continue;
        }
        job.append(&line[pos..a], 0.0, plain.clone());
        job.append(&line[a..b], 0.0, hit.clone());
        pos = b;
    }
    job.append(&line[pos..], 0.0, plain);
    job
}

/// Pause, clear, copy, wrap and reconnect buttons. Returns (copy, reconnect).
fn actions(ui: &mut egui::Ui, t: &mut LogTab) -> (bool, bool) {
    let (mut copy, mut reconnect) = (false, false);
    let closed = matches!(t.state, ConnState::Closed(_));
    let (icon, tip) = if t.paused {
        (Icon::Play, t!("Weiterlaufen lassen"))
    } else {
        (Icon::Pause, t!("Anhalten (neue Zeilen werden gesammelt)"))
    };
    if theme::icon_btn(ui, icon, tip).clicked() {
        t.paused = !t.paused;
        t.paused_at = 0;
    }
    if theme::icon_btn(ui, Icon::Trash, t!("Leeren")).clicked() {
        t.lines.clear();
        t.dropped = 0;
        t.paused_at = 0;
    }
    if theme::icon_btn(ui, Icon::Copy, t!("Angezeigte Zeilen kopieren")).clicked() {
        copy = true;
    }
    if theme::icon_toggle(ui, Icon::Wrap, t!("Lange Zeilen umbrechen"), t.wrap).clicked() {
        t.wrap = !t.wrap;
    }
    if closed && theme::icon_btn(ui, Icon::Refresh, t!("Neu verbinden")).clicked() {
        reconnect = true;
    }
    (copy, reconnect)
}

pub fn show(app: &mut App, ui: &mut egui::Ui, tab_id: u64) {
    let index = app.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
    let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab_id) else {
        return;
    };
    let (title, subtitle) = (tab.title.clone(), tab.subtitle.clone());
    let TabKind::Log(t) = &mut tab.kind else {
        return;
    };
    let (mut reconnect, mut copy) = (false, false);
    crate::ui::chrome::session_header(
        ui,
        egui::Id::new(("log-bar", tab_id)),
        index,
        &title,
        &subtitle,
        crate::ui::chrome::state_pair(&t.state),
        |ui| (copy, reconnect) = actions(ui, t),
    );
    tools(ui, egui::Id::new(("log-tools", tab_id)), t, false);
    content(ui, egui::Id::new(("log-body", tab_id)), t, copy);
    if reconnect {
        let ctx = ui.ctx().clone();
        app.reconnect(tab_id, &ctx);
    }
}

/// Filter, level and follow controls; `narrow` stacks them for the side panel.
fn tools(ui: &mut egui::Ui, id: egui::Id, t: &mut LogTab, narrow: bool) {
    egui::Panel::top(id)
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(Margin::symmetric(if narrow { 12 } else { 14 }, 8)),
        )
        .show_separator_line(false)
        .show(ui, |ui| {
            let filter = |ui: &mut egui::Ui, t: &mut LogTab, w: f32| {
                ui.add(
                    egui::TextEdit::singleline(&mut t.filter)
                        .hint_text(t!("Filter – z.B. nginx | timeout"))
                        .desired_width(w),
                );
            };
            let levels = |ui: &mut egui::Ui, t: &mut LogTab| {
                for (i, label) in [t!("Alle"), t!("Warnungen"), t!("Fehler")]
                    .into_iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut t.level, i as u8, label);
                }
            };
            if narrow {
                filter(ui, t, f32::INFINITY);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    levels(ui, t);
                    ui.checkbox(&mut t.only_matches, t!("nur Treffer"))
                        .on_hover_text(t!("Aus: alle Zeilen zeigen und Treffer nur markieren"));
                    ui.checkbox(&mut t.follow, t!("Ende folgen"));
                });
            } else {
                ui.horizontal(|ui| {
                    filter(ui, t, 260.0);
                    ui.checkbox(&mut t.only_matches, t!("nur Treffer"))
                        .on_hover_text(t!("Aus: alle Zeilen zeigen und Treffer nur markieren"));
                    ui.separator();
                    levels(ui, t);
                    ui.separator();
                    ui.checkbox(&mut t.follow, t!("Ende folgen"));
                });
            }
        });
}

/// The lines with a status line at the bottom.
fn content(ui: &mut egui::Ui, id: egui::Id, t: &mut LogTab, copy: bool) {
    let shown = visible(t);
    if copy {
        let text: Vec<&str> = shown.iter().map(|&i| t.lines[i].as_str()).collect();
        ui.ctx().copy_text(text.join("\n"));
    }
    let terms = terms(&t.filter);
    let font = FontId::monospace(12.5);
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::palette().term_bg)
                .inner_margin(Margin::symmetric(12, 8)),
        )
        .show(ui, |ui| {
            // Status line at the bottom.
            egui::Panel::bottom(id.with("status"))
                .frame(egui::Frame::NONE.inner_margin(Margin::symmetric(0, 4)))
                .show_separator_line(false)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let total = t.lines.len();
                        let mut text = t!("{} Zeilen", total);
                        if shown.len() != total {
                            text = t!("{} von {} Zeilen", shown.len(), total);
                        }
                        if t.dropped > 0 {
                            text.push_str(&t!(" · {} ältere verworfen", t.dropped));
                        }
                        ui.label(theme::mono(text, 11.5).color(theme::text_dim()));
                        if t.paused && t.paused_at > 0 {
                            ui.label(
                                RichText::new(t!("⏸ {} neue Zeilen warten", t.paused_at))
                                    .size(11.5)
                                    .color(theme::amber()),
                            );
                        }
                    });
                });
            if shown.is_empty() {
                ui.add_space(20.0);
                ui.label(
                    RichText::new(match &t.state {
                        ConnState::Connecting(s) => s.clone(),
                        ConnState::Closed(Some(e)) if t.lines.is_empty() => e.clone(),
                        _ if t.lines.is_empty() => t!("Noch keine Ausgabe").to_string(),
                        _ => t!("Keine Zeile passt zum Filter").to_string(),
                    })
                    .color(theme::text_dim()),
                );
                return;
            }
            let stick = t.follow && !t.paused;
            let area = egui::ScrollArea::both()
                .id_salt(id.with("scroll"))
                .auto_shrink([false, false])
                .stick_to_bottom(stick);
            if t.wrap {
                // Wrapped rows differ in height: draw the last lines only.
                let from = shown.len().saturating_sub(3000);
                area.show(ui, |ui| {
                    let w = ui.available_width();
                    for &i in &shown[from..] {
                        ui.label(layout(&t.lines[i], &terms, &font, Some(w)));
                    }
                });
            } else {
                area.show_rows(ui, row_h, shown.len(), |ui, range| {
                    for &i in &shown[range] {
                        ui.add(egui::Label::new(layout(&t.lines[i], &terms, &font, None)).extend());
                    }
                });
            }
        });
}

/// What the side panel asks the app to do.
pub enum SideAction {
    Close,
    Reconnect,
    /// Follow this source on the session's host.
    Start(String),
    /// Save the source as an own log command: (name, command).
    Save(String, String),
    Delete(uuid::Uuid),
    /// Back to the source list (stops the current log).
    Back,
}

/// Log viewer docked at the right of a terminal session. `commands` are the user's own
/// sources for this host: (id, name, command).
pub fn side_panel(
    ui: &mut egui::Ui,
    id: egui::Id,
    side: &mut SideLog,
    host_name: &str,
    commands: &[(uuid::Uuid, String, String)],
) -> Option<SideAction> {
    let mut out = None;
    // Header: title, source and the log actions.
    egui::Panel::top(id.with("head"))
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(Margin {
                    left: 12,
                    right: 8,
                    top: 8,
                    bottom: 4,
                }),
        )
        .show_separator_line(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::ui::icons::show(ui, Icon::Logs, 16.0, theme::accent());
                ui.label(
                    RichText::new(t!("Logs"))
                        .font(theme::semibold(14.0))
                        .color(theme::text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_btn(ui, Icon::Close, t!("Log-Leiste schließen")).clicked() {
                        out = Some(SideAction::Close);
                    }
                    if let Some(t) = side.log.as_mut() {
                        let (copy, reconnect) = actions(ui, t);
                        side.copy = copy;
                        if reconnect {
                            out = Some(SideAction::Reconnect);
                        }
                    }
                });
            });
            if let Some(t) = &side.log {
                ui.horizontal(|ui| {
                    let (label, color) = crate::ui::chrome::state_pair(&t.state);
                    theme::led(
                        ui.painter(),
                        ui.cursor().left_center() + Vec2::new(4.0, 0.0),
                        color,
                        matches!(t.state, ConnState::Connected),
                    );
                    ui.add_space(12.0);
                    ui.add(
                        egui::Label::new(
                            theme::mono(t.source.text(), 12.0).color(theme::text_dim()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(label);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::btn(ui, theme::Btn::Link, None, t!("Quelle"))
                            .on_hover_text(t!("Andere Log-Quelle wählen"))
                            .clicked()
                        {
                            out = Some(SideAction::Back);
                        }
                    });
                });
            }
        });
    if let Some(t) = side.log.as_mut() {
        tools(ui, id.with("tools"), t, true);
        let copy = std::mem::take(&mut side.copy);
        content(ui, id.with("body"), t, copy);
        return out;
    }

    // No log yet: pick a source.
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(Margin::symmetric(12, 8)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(id.with("pick"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(t!(
                            "Datei (wird mit tail -F verfolgt) oder ein Befehl, der laufend ausgibt – läuft auf {}",
                            host_name
                        ))
                        .size(12.0)
                        .color(theme::text_dim()),
                    );
                    ui.add_space(4.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut side.input)
                            .hint_text("/var/log/syslog · journalctl -f -u nginx")
                            .font(FontId::monospace(13.0))
                            .desired_width(f32::INFINITY),
                    );
                    let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ready = !side.input.trim().is_empty();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled_ui(ready, |ui| theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Starten")))
                            .inner
                            .clicked()
                            || (entered && ready)
                        {
                            out = Some(SideAction::Start(side.input.trim().to_string()));
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut side.save_name)
                                .hint_text(t!("Name"))
                                .desired_width(ui.available_width() - 90.0),
                        );
                        if ui
                            .add_enabled_ui(ready, |ui| theme::btn(ui, theme::Btn::Small, None, t!("Speichern")))
                            .inner
                            .on_hover_text(t!("Als eigenen Log-Befehl für diesen Host speichern"))
                            .clicked()
                        {
                            let name = if side.save_name.trim().is_empty() {
                                side.input.trim().to_string()
                            } else {
                                side.save_name.trim().to_string()
                            };
                            out = Some(SideAction::Save(name, side.input.trim().to_string()));
                            side.save_name.clear();
                        }
                    });
                    ui.add_space(12.0);
                    let row = |ui: &mut egui::Ui, name: &str, cmd: &str, own: bool| {
                        let (rect, resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 40.0),
                            egui::Sense::click(),
                        );
                        let hovered = resp.hovered();
                        if hovered {
                            ui.painter()
                                .rect_filled(rect, egui::CornerRadius::same(8), theme::bg_hover());
                        }
                        let p = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)));
                        p.text(
                            rect.left_top() + Vec2::new(10.0, 6.0),
                            egui::Align2::LEFT_TOP,
                            name,
                            theme::semibold(12.5),
                            theme::text(),
                        );
                        p.text(
                            rect.left_top() + Vec2::new(10.0, 23.0),
                            egui::Align2::LEFT_TOP,
                            cmd,
                            FontId::monospace(11.0),
                            theme::text_dim(),
                        );
                        let mut delete = false;
                        if own && hovered {
                            let b = egui::Rect::from_center_size(
                                rect.right_center() - Vec2::new(18.0, 0.0),
                                Vec2::splat(26.0),
                            );
                            delete = ui
                                .put(b, |ui: &mut egui::Ui| {
                                    theme::icon_btn(ui, Icon::Trash, t!("Löschen"))
                                })
                                .clicked();
                        }
                        let start = !delete
                            && resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(t!("Klicken zum Starten"))
                                .clicked();
                        (start, delete)
                    };
                    if !commands.is_empty() {
                        theme::section_title(ui, t!("Eigene Befehle"));
                        for (cid, name, cmd) in commands {
                            let (start, delete) = row(ui, name, cmd, true);
                            if start {
                                out = Some(SideAction::Start(cmd.clone()));
                            }
                            if delete {
                                out = Some(SideAction::Delete(*cid));
                            }
                        }
                        ui.add_space(10.0);
                    }
                    theme::section_title(ui, t!("Vorlagen"));
                    for (name, cmd) in crate::model::LOG_PRESETS {
                        if row(ui, crate::i18n::tr(name), cmd, false).0 {
                            out = Some(SideAction::Start(cmd.to_string()));
                        }
                    }
                });
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_filter() {
        assert_eq!(line_level("2024-01-01 ERROR db down"), 3);
        assert_eq!(line_level("[warn] slow query"), 2);
        assert_eq!(line_level("connection failed"), 3);
        assert_eq!(line_level("user=terry logged in"), 1);
        assert_eq!(line_level("errors=0 ok"), 1);
        assert_eq!(line_level("DEBUG cache hit"), 0);
        let t = terms(" Nginx | timeout |");
        assert_eq!(t, vec!["nginx", "timeout"]);
        assert!(matches("upstream TIMEOUT", &t));
        assert!(!matches("all good", &t));
    }
}
