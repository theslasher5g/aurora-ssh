pub mod chrome;
pub mod containers;
pub mod dialogs;
pub mod editors;
pub mod help;
pub mod hosts;
pub mod icons;
pub mod keychain;
pub mod lock;
pub mod logview;
pub mod palette;
pub mod rdp;
pub mod settings;
pub mod sftp;
pub mod snippets;
pub mod vnc;

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

/// `12 s`, `3 min 5 s`, `1 h 20 min`.
pub fn short_duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

pub fn relative_time(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => t!("gerade eben").into(),
        60..=3599 => t!("vor {} min", secs / 60),
        3600..=86399 => t!("vor {} h", secs / 3600),
        86400..=172799 => t!("gestern").into(),
        _ => t!("vor {} Tagen", secs / 86400),
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
    use crate::theme;
    let mut clicked = false;
    ui.horizontal(|ui| {
        theme::page_title(ui, title, Some(count));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            clicked = theme::btn(ui, theme::Btn::Primary, Some(icons::Icon::Plus), add).clicked();
            theme::search_field(
                ui,
                search,
                hint,
                260.0,
                ui.id().with(("page-search", title)),
            );
        });
    });
    ui.add_space(22.0);
    clicked
}

/// Buttons drawn right-aligned inside a list row on hover; returns the clicked index.
pub fn row_buttons(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    labels: &[&str],
) -> Option<usize> {
    use crate::theme;
    let mut clicked = None;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("row-buttons", id))
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                rect.right_bottom() - egui::vec2(16.0, 0.0),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 6.0;
    for (i, label) in labels.iter().enumerate().rev() {
        // The first action is the main one.
        let kind = if i == 0 {
            theme::Btn::SmallPrimary
        } else {
            theme::Btn::Small
        };
        if theme::btn(&mut child, kind, None, label).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Empty list placeholder with one action.
pub fn empty_list(ui: &mut egui::Ui, title: &str, body: &str, action: &str) -> bool {
    use crate::theme;
    let mut clicked = false;
    theme::card(ui, theme::bg_deep(), |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(24, 24))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(theme::heading(title, 18.0));
                ui.label(egui::RichText::new(body).color(theme::text_dim()));
                ui.add_space(10.0);
                clicked =
                    theme::btn(ui, theme::Btn::Primary, Some(icons::Icon::Plus), action).clicked();
            });
    });
    clicked
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
