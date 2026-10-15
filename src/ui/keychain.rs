//! Keychain view: identities (user + password and/or private key).

use egui::{RichText, Vec2};

use crate::app::{Action, App};
use crate::theme;
use crate::ui::fuzzy;

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Keychain").size(26.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::primary_button(ui, "➕ Identität").clicked() {
                actions.push(Action::NewIdentity);
            }
            ui.add(
                egui::TextEdit::singleline(&mut app.keychain_search)
                    .hint_text("🔍 Suchen…")
                    .desired_width(220.0),
            );
        });
    });
    ui.label(
        RichText::new("Passwörter und SSH-Keys, verschlüsselt mit deinem Master-Passwort. Hosts und Gruppen verweisen auf Identitäten.")
            .color(theme::TEXT_DIM),
    );
    ui.add_space(12.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let q = app.keychain_search.clone();
            let idents: Vec<_> = app
                .keychain
                .identities
                .iter()
                .filter(|i| fuzzy(&q, &format!("{} {}", i.label, i.username)).is_some())
                .cloned()
                .collect();
            if idents.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("🔑").size(40.0));
                    ui.label(
                        RichText::new("Noch keine Identitäten")
                            .size(16.0)
                            .color(theme::TEXT_DIM),
                    );
                });
            }
            for ident in idents {
                let used_by = app
                    .lib
                    .hosts
                    .iter()
                    .filter(|h| h.identity_id == Some(ident.id))
                    .count()
                    + app
                        .lib
                        .groups
                        .iter()
                        .filter(|g| g.default_identity == Some(ident.id))
                        .count();
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let color = theme::rgb(theme::hash_color(&ident.label));
                        theme::avatar(
                            ui,
                            40.0,
                            color,
                            if ident.private_key.is_empty() {
                                "🔒"
                            } else {
                                "🔑"
                            },
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let title = if ident.label.is_empty() {
                                "(ohne Namen)"
                            } else {
                                &ident.label
                            };
                            ui.label(RichText::new(title).strong().size(15.0));
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "👤 {}",
                                        if ident.username.is_empty() {
                                            "—"
                                        } else {
                                            &ident.username
                                        }
                                    ))
                                    .color(theme::TEXT_DIM),
                                );
                                theme::chip(ui, ident.kind(), theme::VIOLET);
                                if used_by > 0 {
                                    theme::chip(ui, &format!("{used_by}× verwendet"), theme::TEAL);
                                }
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("🗑").on_hover_text("Löschen").clicked() {
                                actions.push(Action::Confirm(
                                    format!("Identität „{}“ löschen?", ident.label),
                                    Box::new(Action::DeleteIdentity(ident.id)),
                                ));
                            }
                            if ui.button("✏ Bearbeiten").clicked() {
                                actions.push(Action::EditIdentity(ident.id));
                            }
                            if !ident.private_key.is_empty()
                                && ui
                                    .button("📋 Public Key")
                                    .on_hover_text("authorized_keys-Zeile kopieren")
                                    .clicked()
                            {
                                match crate::keygen::public_info(
                                    &ident.private_key,
                                    &ident.passphrase,
                                ) {
                                    Ok((pubkey, _)) => {
                                        ui.ctx().copy_text(pubkey);
                                        app.toast("Public Key kopiert", theme::TEAL);
                                    }
                                    Err(e) => app.toast(format!("Key ungültig: {e}"), theme::RED),
                                }
                            }
                        });
                    });
                });
                ui.add_space(8.0);
            }
            ui.add_space(Vec2::splat(10.0).y);
        });
}
