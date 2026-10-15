//! Snippet library with `{{variable}}` placeholders.

use egui::{CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::app::{Action, App};
use crate::theme;
use crate::ui::{empty_list, fuzzy, page_header, row_buttons};

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let count = app.lib.snippets.len();
    if page_header(
        ui,
        "Snippets",
        count,
        &mut app.snippet_search,
        "Suchen",
        "Neu",
    ) {
        actions.push(Action::NewSnippet);
    }

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

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if count == 0 {
            if empty_list(
                ui,
                "Noch keine Snippets",
                "Speichere Befehle, die du oft brauchst. Platzhalter wie {{dienst}} fragt Aurora beim Ausführen ab.",
                "Snippet anlegen",
            ) {
                actions.push(Action::NewSnippet);
            }
            return;
        }
        for (_, s) in list {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), Sense::click());
            let hovered = ui.rect_contains_pointer(rect);
            let p = ui.painter().with_clip_rect(rect);
            if hovered {
                p.rect_filled(rect, CornerRadius::same(6), theme::bg_hover());
            }
            p.rect_filled(
                Rect::from_min_max(Pos2::new(rect.left() + 10.0, rect.bottom() - 1.0), rect.right_bottom()),
                CornerRadius::ZERO,
                theme::border().linear_multiply(0.6),
            );
            let left = rect.left() + 12.0;
            let name_g = p.layout_no_wrap(s.name.clone(), theme::semibold(14.0), theme::text());
            let nw = name_g.size().x;
            p.galley(Pos2::new(left, rect.top() + 9.0), name_g, theme::text());
            if !s.description.is_empty() {
                p.text(
                    Pos2::new(left + nw + 10.0, rect.top() + 18.0),
                    egui::Align2::LEFT_CENTER,
                    &s.description,
                    FontId::proportional(12.5),
                    theme::text_dim(),
                );
            }
            // Script preview: prompt glyph in accent, first line in mono.
            p.text(Pos2::new(left, rect.top() + 39.0), egui::Align2::LEFT_CENTER, "$", FontId::monospace(12.5), theme::accent());
            p.text(
                Pos2::new(left + 14.0, rect.top() + 39.0),
                egui::Align2::LEFT_CENTER,
                s.script.lines().next().unwrap_or_default(),
                FontId::monospace(12.5),
                theme::text_dim(),
            );
            if hovered {
                let labels = ["Ausführen", "In allen", "Bearbeiten", "Löschen"];
                match row_buttons(ui, rect, s.id, &labels) {
                    Some(0) => actions.push(Action::RunSnippet(s.id, false)),
                    Some(1) => actions.push(Action::RunSnippet(s.id, true)),
                    Some(2) => actions.push(Action::EditSnippet(s.id)),
                    Some(3) => actions.push(Action::Confirm(
                        format!("Snippet „{}“ löschen?", s.name),
                        Box::new(Action::DeleteSnippet(s.id)),
                    )),
                    _ => {}
                }
            } else {
                // Variables and tags as small outlined labels, right aligned.
                let mut x = rect.right() - 12.0;
                let labels: Vec<(String, egui::Color32)> = s
                    .variables()
                    .into_iter()
                    .map(|v| (format!("{{{{{v}}}}}"), theme::accent()))
                    .chain(s.tags.iter().map(|t| (format!("#{t}"), theme::text_dim())))
                    .collect();
                for (label, color) in labels.into_iter().rev() {
                    let g = p.layout_no_wrap(label, FontId::monospace(10.5), color);
                    let r = Rect::from_min_size(Pos2::new(x - g.size().x - 10.0, rect.center().y - 9.0), g.size() + Vec2::new(10.0, 6.0));
                    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, color.linear_multiply(0.4)), egui::StrokeKind::Inside);
                    p.galley(r.min + Vec2::new(5.0, 3.0), g, color);
                    x = r.left() - 6.0;
                }
            }
            if resp.double_clicked() {
                actions.push(Action::RunSnippet(s.id, false));
            }
        }
    });
}
