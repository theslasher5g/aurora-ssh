//! Snippet library with `{{variable}}` placeholders.

use egui::{CornerRadius, Margin, RichText};

use crate::app::{Action, App};
use crate::theme;
use crate::ui::fuzzy;

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Snippets").size(26.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::primary_button(ui, "➕ Snippet").clicked() {
                actions.push(Action::NewSnippet);
            }
            ui.add(
                egui::TextEdit::singleline(&mut app.snippet_search)
                    .hint_text("🔍 Suchen…")
                    .desired_width(220.0),
            );
        });
    });
    ui.add_space(12.0);

    let q = app.snippet_search.clone();
    let mut list: Vec<_> = app
        .lib
        .snippets
        .iter()
        .filter_map(|s| {
            fuzzy(
                &q,
                &format!(
                    "{} {} {} {}",
                    s.name,
                    s.description,
                    s.script,
                    s.tags.join(" ")
                ),
            )
            .map(|sc| (sc, s.clone()))
        })
        .collect();
    list.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (_, s) in list {
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚡").color(theme::amber()).size(18.0));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(&s.name).strong().size(15.0));
                            if !s.description.is_empty() {
                                ui.label(
                                    RichText::new(&s.description)
                                        .color(theme::text_dim())
                                        .size(12.5),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("🗑").clicked() {
                                actions.push(Action::Confirm(
                                    format!("Snippet „{}“ löschen?", s.name),
                                    Box::new(Action::DeleteSnippet(s.id)),
                                ));
                            }
                            if ui.button("✏").clicked() {
                                actions.push(Action::EditSnippet(s.id));
                            }
                            if ui.button("📋").on_hover_text("Kopieren").clicked() {
                                ui.ctx().copy_text(s.script.clone());
                            }
                            if ui
                                .button("▶▶ Alle")
                                .on_hover_text("In allen verbundenen Terminals ausführen")
                                .clicked()
                            {
                                actions.push(Action::RunSnippet(s.id, true));
                            }
                            if ui
                                .button("▶ Ausführen")
                                .on_hover_text("Im zuletzt aktiven Terminal ausführen")
                                .clicked()
                            {
                                actions.push(Action::RunSnippet(s.id, false));
                            }
                        });
                    });
                    egui::Frame::NONE
                        .fill(theme::bg_deep())
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(&s.script)
                                    .monospace()
                                    .size(12.5)
                                    .color(theme::teal()),
                            );
                        });
                    let vars = s.variables();
                    if !s.tags.is_empty() || !vars.is_empty() {
                        ui.horizontal(|ui| {
                            for t in &s.tags {
                                theme::chip(ui, &format!("#{t}"), theme::rgb(theme::hash_color(t)));
                            }
                            for v in &vars {
                                theme::chip(ui, &format!("{{{{{v}}}}}"), theme::violet());
                            }
                        });
                    }
                });
                ui.add_space(8.0);
            }
        });
}
