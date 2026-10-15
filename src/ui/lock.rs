//! Master password screen: create the vault on first start, unlock afterwards.

use egui::{Margin, RichText, Vec2};

use crate::app::App;
use crate::theme;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let first_run = !app.store.vault_exists();
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(theme::BG_DEEP))
        .show(ui, |ui| {
            // Aurora glow band at the top.
            let r = ui.max_rect();
            let band = egui::Rect::from_min_size(r.min, Vec2::new(r.width(), 4.0));
            theme::gradient_rect(ui.painter(), band, theme::TEAL, theme::VIOLET, 0.0);

            ui.vertical_centered(|ui| {
                ui.add_space((r.height() * 0.18).max(30.0));
                theme::logo(ui, 72.0);
                ui.add_space(14.0);
                ui.label(RichText::new("aurora").size(34.0).strong());
                ui.label(
                    RichText::new("SSH · SFTP · RDP · Serial — dein Terminal-Cockpit")
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(28.0);

                theme::card_frame()
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(if first_run { "Vault anlegen" } else { "Vault entsperren" })
                                    .size(18.0)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(if first_run {
                                    "Wähle ein Master-Passwort. Es verschlüsselt deine Keychain (Argon2id + XChaCha20-Poly1305) und kann nicht wiederhergestellt werden."
                                } else {
                                    "Gib dein Master-Passwort ein."
                                })
                                .color(theme::TEXT_DIM)
                                .size(12.5),
                            );
                            ui.add_space(10.0);
                            let pw = ui.add(
                                egui::TextEdit::singleline(&mut app.lock.password)
                                    .password(true)
                                    .hint_text("Master-Passwort")
                                    .desired_width(f32::INFINITY),
                            );
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let mut submit = enter && pw.lost_focus();
                            if !submit && !ui.memory(|m| m.focused().is_some()) {
                                pw.request_focus();
                            }
                            if first_run {
                                let c = ui.add(
                                    egui::TextEdit::singleline(&mut app.lock.confirm)
                                        .password(true)
                                        .hint_text("Wiederholen")
                                        .desired_width(f32::INFINITY),
                                );
                                submit = enter && c.lost_focus();
                                if enter && pw.lost_focus() {
                                    c.request_focus();
                                }
                                strength_meter(ui, &app.lock.password);
                            }
                            if let Some(e) = &app.lock.error {
                                ui.label(RichText::new(e).color(theme::RED).size(12.5));
                            }
                            ui.add_space(6.0);
                            let label = if first_run { "Vault erstellen" } else { "Entsperren" };
                            if theme::primary_button(ui, label).clicked() {
                                submit = true;
                            }
                            if submit {
                                unlock(app, first_run);
                            }
                        });
                    });
            });
        });
}

fn unlock(app: &mut App, first_run: bool) {
    let pw = app.lock.password.clone();
    if first_run {
        if pw.chars().count() < 8 {
            app.lock.error = Some("Mindestens 8 Zeichen.".into());
            return;
        }
        if pw != app.lock.confirm {
            app.lock.error = Some("Passwörter stimmen nicht überein.".into());
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
        .rect_filled(rect, egui::CornerRadius::same(3), theme::BG_HOVER);
    let mut fill = rect;
    fill.set_width(rect.width() * score);
    let color = if score < 0.4 {
        theme::RED
    } else if score < 0.7 {
        theme::AMBER
    } else {
        theme::GREEN
    };
    ui.painter()
        .rect_filled(fill, egui::CornerRadius::same(3), color);
}
