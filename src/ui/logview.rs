//! Log viewer tab: a followed file or command with filter, levels, pause and highlighting.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Margin, RichText};

use crate::app::{App, ConnState, LogTab, TabKind};
use crate::theme;

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

pub fn show(app: &mut App, ui: &mut egui::Ui, tab_id: u64) {
    let index = app.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
    let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab_id) else {
        return;
    };
    let (title, subtitle) = (tab.title.clone(), tab.subtitle.clone());
    let TabKind::Log(t) = &mut tab.kind else {
        return;
    };
    let mut reconnect = false;
    let mut copy = false;
    let closed = matches!(t.state, ConnState::Closed(_));
    crate::ui::chrome::session_header(
        ui,
        egui::Id::new(("log-bar", tab_id)),
        index,
        &title,
        &subtitle,
        crate::ui::chrome::state_pair(&t.state),
        |ui| {
            let (glyph, tip) = if t.paused {
                ("▶", t!("Weiterlaufen lassen"))
            } else {
                ("⏸", t!("Anhalten (neue Zeilen werden gesammelt)"))
            };
            if theme::icon_button(ui, glyph, tip).clicked() {
                t.paused = !t.paused;
                t.paused_at = 0;
            }
            if theme::icon_button(ui, "🗑", t!("Leeren")).clicked() {
                t.lines.clear();
                t.dropped = 0;
                t.paused_at = 0;
            }
            if theme::icon_button(ui, "📋", t!("Angezeigte Zeilen kopieren")).clicked() {
                copy = true;
            }
            if theme::icon_button(ui, "↩", t!("Lange Zeilen umbrechen")).clicked() {
                t.wrap = !t.wrap;
            }
            if closed && theme::icon_button(ui, "⟳", t!("Neu verbinden")).clicked() {
                reconnect = true;
            }
        },
    );

    egui::Panel::top(egui::Id::new(("log-tools", tab_id)))
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(Margin::symmetric(14, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut t.filter)
                        .hint_text(t!("Filter – z.B. nginx | timeout"))
                        .desired_width(260.0),
                );
                ui.checkbox(&mut t.only_matches, t!("nur Treffer"))
                    .on_hover_text(t!("Aus: alle Zeilen zeigen und Treffer nur markieren"));
                ui.separator();
                for (i, label) in [t!("Alle"), t!("Warnungen"), t!("Fehler")]
                    .into_iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut t.level, i as u8, label);
                }
                ui.separator();
                ui.checkbox(&mut t.follow, t!("Ende folgen"));
            });
        });

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
            egui::Panel::bottom(egui::Id::new(("log-status", tab_id)))
                .frame(egui::Frame::NONE.inner_margin(Margin::symmetric(0, 4)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
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
                        _ if t.lines.is_empty() => t!("Noch keine Ausgabe").to_string(),
                        _ => t!("Keine Zeile passt zum Filter").to_string(),
                    })
                    .color(theme::text_dim()),
                );
                return;
            }
            let stick = t.follow && !t.paused;
            let area = egui::ScrollArea::both()
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
    if reconnect {
        let ctx = ui.ctx().clone();
        app.reconnect(tab_id, &ctx);
    }
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
