//! Snippet library with `{{variable}}` placeholders.

use egui::{CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::app::{Action, App};
use crate::theme;
use crate::ui::{empty_list, fuzzy, page_header, row_buttons};

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.ops.page == 1 {
        return crate::ui::runbooks::page(app, ui, actions);
    }
    let count = app.lib.snippets.len();
    if page_header(
        ui,
        "Snippets",
        count,
        &mut app.snippet_search,
        t!("Suchen"),
        t!("Neu"),
    ) {
        actions.push(Action::NewSnippet);
    }
    crate::ui::runbooks::switch(app, ui);

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

    egui::ScrollArea::vertical().id_salt("snippets-page").auto_shrink([false, false]).show(ui, |ui| {
            // Room for the scroll bar next to the cards.
            ui.set_max_width(ui.available_width() - 14.0);
        if count == 0 {
            if empty_list(
                ui,
                t!("Noch keine Snippets"),
                t!("Speichere Befehle, die du oft brauchst. Platzhalter wie {{dienst}} fragt Aurora beim Ausführen ab."),
                t!("Snippet anlegen"),
            ) {
                actions.push(Action::NewSnippet);
            }
            return;
        }
        if list.is_empty() {
            if empty_list(
                ui,
                t!("Kein Snippet gefunden"),
                t!("Leg eins an – oder markiere Text im Terminal und wähle „Als Snippet speichern“."),
                t!("Snippet anlegen"),
            ) {
                actions.push(Action::NewSnippet);
            }
            return;
        }
        theme::card(ui, theme::bg_deep(), |ui| {
            for (i, (_, s)) in list.into_iter().enumerate() {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 68.0), Sense::click());
                let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                if hovered {
                    p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
                }
                if i > 0 {
                    p.line_segment(
                        [
                            Pos2::new(rect.left() + 18.0, rect.top()),
                            rect.right_top(),
                        ],
                        Stroke::new(1.0, theme::line()),
                    );
                }
                let left = rect.left() + 18.0;
                let y1 = rect.top() + 23.0;
                let y2 = rect.top() + 46.0;
                let name_g = p.layout_no_wrap(s.name.clone(), theme::semibold(15.0), theme::text());
                let mut nw = name_g.size().x;
                p.galley(Pos2::new(left, y1 - name_g.size().y / 2.0), name_g, theme::text());
                if s.shell == crate::model::ShellKind::PowerShell {
                    // Small "PS" tag after the name.
                    let g = p.layout_no_wrap("PS".into(), theme::bold(10.5), theme::accent2());
                    let r = Rect::from_min_size(
                        Pos2::new(left + nw + 8.0, y1 - 9.0),
                        Vec2::new(g.size().x + 12.0, 18.0),
                    );
                    p.rect_filled(r, CornerRadius::same(5), theme::soft(theme::accent2()));
                    p.galley(r.center() - g.size() / 2.0, g, theme::accent2());
                    nw += r.width() + 8.0;
                }
                if !s.description.is_empty() {
                    p.text(
                        Pos2::new(left + nw + 10.0, y1),
                        egui::Align2::LEFT_CENTER,
                        &s.description,
                        FontId::proportional(13.0),
                        theme::text_dim(),
                    );
                }
                // Script preview: prompt glyph in accent, first line in mono.
                p.text(
                    Pos2::new(left, y2),
                    egui::Align2::LEFT_CENTER,
                    if s.shell == crate::model::ShellKind::PowerShell {
                        ">"
                    } else {
                        "$"
                    },
                    theme::mono_medium(13.0),
                    theme::accent(),
                );
                p.text(
                    Pos2::new(left + 16.0, y2),
                    egui::Align2::LEFT_CENTER,
                    s.script.lines().next().unwrap_or_default(),
                    FontId::monospace(13.0),
                    theme::text_dim(),
                );
                if hovered {
                    let labels = [t!("Ausführen"), t!("In allen"), t!("Bearbeiten"), t!("Löschen")];
                    match row_buttons(ui, rect, s.id, &labels) {
                        Some(0) => actions.push(Action::RunSnippet(s.id, false)),
                        Some(1) => actions.push(Action::RunSnippet(s.id, true)),
                        Some(2) => actions.push(Action::EditSnippet(s.id)),
                        Some(3) => actions.push(Action::Confirm(
                            t!("Snippet „{}“ löschen?", s.name),
                            Box::new(Action::DeleteSnippet(s.id)),
                        )),
                        _ => {}
                    }
                } else {
                    // Variables as accent chips, tags as grey ones, right aligned.
                    let mut x = rect.right() - 18.0;
                    let labels: Vec<(String, bool)> = s
                        .variables()
                        .into_iter()
                        .map(|v| (format!("{{{{{v}}}}}"), true))
                        .chain(s.tags.iter().map(|t| (format!("#{t}"), false)))
                        .collect();
                    for (label, var) in labels.into_iter().rev() {
                        let color = if var { theme::accent() } else { theme::text_dim() };
                        let g = p.layout_no_wrap(label, FontId::monospace(12.0), color);
                        let r = Rect::from_min_size(
                            Pos2::new(x - g.size().x - 18.0, rect.center().y - 11.0),
                            Vec2::new(g.size().x + 18.0, 22.0),
                        );
                        p.rect_filled(
                            r,
                            CornerRadius::same(7),
                            if var { theme::accent_soft() } else { theme::fill() },
                        );
                        p.galley(r.center() - g.size() / 2.0, g, color);
                        x = r.left() - 6.0;
                    }
                }
                if resp.double_clicked() {
                    actions.push(Action::RunSnippet(s.id, false));
                }
            }
        });
    });
}
