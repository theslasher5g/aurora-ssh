//! Master password screen: create the vault on first start, unlock afterwards.

use egui::{RichText, Vec2};

use crate::app::App;
use crate::theme;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let first_run = !app.store.vault_exists();
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(theme::bg_deep()))
        .show(ui, |ui| {
            let r = ui.max_rect();
            patch_panel(ui.painter(), r);

            let left = r.left() + (r.width() * 0.09).max(48.0);
            let col = egui::Rect::from_min_size(
                egui::pos2(left, r.top() + r.height() * 0.22),
                Vec2::new(360.0, r.height() * 0.7),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(col).layout(egui::Layout::top_down(egui::Align::Min)));
            let ui = &mut child;
            ui.horizontal(|ui| {
                theme::logo(ui, 26.0);
                ui.label(RichText::new("aurora").font(theme::mono_medium(17.0)).color(theme::text()));
            });
            ui.add_space(40.0);
            ui.label(theme::heading(if first_run { t!("Tresor anlegen") } else { t!("Willkommen zurück") }, 30.0));
            ui.add_space(2.0);
            ui.label(
                RichText::new(if first_run {
                    t!("Ein Master-Passwort verschlüsselt Passwörter und Schlüssel. Es lässt sich nicht wiederherstellen.")
                } else {
                    t!("Master-Passwort eingeben, um den Tresor zu öffnen.")
                })
                .color(theme::text_dim()),
            );
            ui.add_space(22.0);
            let field = |ui: &mut egui::Ui, value: &mut String, hint: &str| {
                ui.add(
                    egui::TextEdit::singleline(value)
                        .password(true)
                        .hint_text(hint)
                        .font(egui::FontId::monospace(14.0))
                        .margin(egui::Margin::symmetric(12, 9))
                        .desired_width(f32::INFINITY),
                )
            };
            let pw = field(ui, &mut app.lock.password, t!("Master-Passwort"));
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let mut submit = enter && pw.lost_focus();
            if !submit && !ui.memory(|m| m.focused().is_some()) {
                pw.request_focus();
            }
            if first_run {
                let c = field(ui, &mut app.lock.confirm, t!("Wiederholen"));
                submit = enter && c.lost_focus();
                if enter && pw.lost_focus() {
                    c.request_focus();
                }
                strength_meter(ui, &app.lock.password);
            }
            if let Some(e) = &app.lock.error {
                ui.label(RichText::new(e).color(theme::red()).size(13.0));
            }
            ui.add_space(10.0);
            let label = if first_run { t!("Tresor anlegen") } else { t!("Entsperren") };
            if theme::primary_button(ui, label).clicked() {
                submit = true;
            }
            if submit {
                unlock(app, first_run);
            }
        });
}

/// Decorative rack of numbered jacks on the right half; a few LEDs are lit.
fn patch_panel(p: &egui::Painter, r: egui::Rect) {
    let area = egui::Rect::from_min_max(
        egui::pos2(r.left() + r.width() * 0.52, r.top() + 60.0),
        egui::pos2(r.right() - 50.0, r.bottom() - 60.0),
    );
    if area.width() < 200.0 {
        return;
    }
    let step = 46.0;
    let cols = (area.width() / step).floor() as usize;
    let rows = (area.height() / step).floor() as usize;
    let lit = [(1usize, 2usize), (3, 0), (4, 5), (6, 3), (2, 6)];
    let leds = [
        theme::accent(),
        theme::green(),
        theme::accent(),
        theme::accent2(),
        theme::green(),
    ];
    for row in 0..rows {
        let y = area.top() + row as f32 * step + step / 2.0;
        p.text(
            egui::pos2(area.left() - 18.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{:02}", row + 1),
            egui::FontId::monospace(9.5),
            theme::border(),
        );
        for col in 0..cols {
            let c = egui::pos2(area.left() + col as f32 * step + step / 2.0, y);
            let on = lit.iter().position(|&(lc, lr)| lc == col && lr == row);
            let ring = if on.is_some() {
                theme::text_dim().linear_multiply(0.6)
            } else {
                theme::border()
            };
            p.circle_stroke(c, 11.0, egui::Stroke::new(1.5, ring));
            p.circle_filled(c, 3.0, ring);
            if let Some(i) = on {
                theme::led(p, c + Vec2::new(14.0, -14.0), leds[i], true);
            }
        }
    }
}

fn unlock(app: &mut App, first_run: bool) {
    let pw = app.lock.password.clone();
    if first_run {
        if pw.chars().count() < 8 {
            app.lock.error = Some(t!("Mindestens 8 Zeichen.").into());
            return;
        }
        if pw != app.lock.confirm {
            app.lock.error = Some(t!("Passwörter stimmen nicht überein.").into());
            return;
        }
        match app.store.create_vault(&pw) {
            Ok(v) => {
                app.vault = Some(v);
                app.keychain = Default::default();
                app.save_library();
            }
            Err(e) => app.lock.error = Some(format!("{e:#}")),
        }
    } else {
        match app.store.unlock_vault(&pw) {
            Ok((v, kc)) => {
                app.vault = Some(v);
                app.keychain = kc;
            }
            Err(e) => app.lock.error = Some(format!("{e:#}")),
        }
    }
    if app.vault.is_some() {
        app.lock = Default::default();
    }
}

fn strength_meter(ui: &mut egui::Ui, pw: &str) {
    let classes = [
        pw.chars().any(|c| c.is_lowercase()),
        pw.chars().any(|c| c.is_uppercase()),
        pw.chars().any(|c| c.is_ascii_digit()),
        pw.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let len = pw.chars().count();
    let score = ((len.min(16) as f32 / 16.0) * 0.6 + classes as f32 / 4.0 * 0.4).clamp(0.0, 1.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 5.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(3), theme::bg_hover());
    let mut fill = rect;
    fill.set_width(rect.width() * score);
    let color = if score < 0.4 {
        theme::red()
    } else if score < 0.7 {
        theme::amber()
    } else {
        theme::green()
    };
    ui.painter()
        .rect_filled(fill, egui::CornerRadius::same(3), color);
}
