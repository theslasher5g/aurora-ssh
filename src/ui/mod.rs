pub mod chrome;
pub mod dialogs;
pub mod editors;
pub mod hosts;
pub mod keychain;
pub mod lock;
pub mod palette;
pub mod rdp;
pub mod settings;
pub mod sftp;
pub mod snippets;

/// Case-insensitive subsequence match; returns a score (higher is better).
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text_l = text.to_lowercase();
    let query_l = query.to_lowercase();
    if let Some(pos) = text_l.find(&query_l) {
        return Some(1000 - pos as i32 - text_l.len() as i32 / 4);
    }
    let mut score = 0;
    let mut chars = text_l.chars().enumerate();
    let mut last = 0usize;
    for q in query_l.chars() {
        let (i, _) = chars.by_ref().find(|(_, c)| *c == q)?;
        score -= (i - last) as i32;
        last = i;
    }
    Some(score)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn relative_time(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} min", secs / 60),
        3600..=86399 => format!("vor {} h", secs / 3600),
        86400..=172799 => "gestern".into(),
        _ => format!("vor {} Tagen", secs / 86400),
    }
}

/// Page header shared by the list views: title + count left, search and primary action right.
/// Returns true when the primary button was clicked.
pub fn page_header(
    ui: &mut egui::Ui,
    title: &str,
    count: usize,
    search: &mut String,
    hint: &str,
    add: &str,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(crate::theme::heading(title, 24.0));
        ui.label(crate::theme::mono(count.to_string(), 13.0).color(crate::theme::text_dim()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = crate::theme::primary_button(ui, add).clicked();
            ui.add(
                egui::TextEdit::singleline(search)
                    .hint_text(hint)
                    .desired_width(260.0)
                    .margin(egui::Margin::symmetric(10, 6)),
            );
        });
    });
    ui.add_space(14.0);
    crate::theme::hairline(ui);
    ui.add_space(6.0);
    clicked
}

/// Text buttons drawn right-aligned inside a list row on hover; returns the clicked index.
pub fn row_buttons(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    labels: &[&str],
) -> Option<usize> {
    use egui::{CornerRadius, Rect, Sense, Stroke, Vec2};
    let theme = crate::theme::palette();
    let mut clicked = None;
    let mut x = rect.right() - 8.0;
    let cy = rect.center().y;
    for (i, label) in labels.iter().enumerate().rev() {
        let danger = label.starts_with("Löschen");
        let g = ui.painter().layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(12.5),
            theme.text,
        );
        let r = Rect::from_min_size(
            egui::pos2(x - g.size().x - 20.0, cy - 13.0),
            Vec2::new(g.size().x + 20.0, 26.0),
        );
        x = r.left() - 6.0;
        let b = ui.interact(r, ui.id().with((id, i)), Sense::click());
        let fill = if b.hovered() {
            theme.bg_card
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect(
            r,
            CornerRadius::same(5),
            fill,
            Stroke::new(1.0, theme.border),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(
            r.center() - g.size() / 2.0,
            g,
            if danger { theme.red } else { theme.text },
        );
        if b.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Empty list placeholder with one action.
pub fn empty_list(ui: &mut egui::Ui, title: &str, body: &str, action: &str) -> bool {
    ui.add_space(40.0);
    ui.label(crate::theme::heading(title, 18.0));
    ui.label(egui::RichText::new(body).color(crate::theme::text_dim()));
    ui.add_space(12.0);
    crate::theme::primary_button(ui, action).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matching() {
        assert!(fuzzy("web", "web-01").unwrap() > fuzzy("wb", "web-01").unwrap());
        assert!(fuzzy("xyz", "web-01").is_none());
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}
