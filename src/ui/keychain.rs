//! Keys & passwords: identities (user + password and/or private key) as a list.

use egui::{CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::app::{Action, App};
use crate::theme;
use crate::ui::{empty_list, fuzzy, page_header, row_buttons};

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let count = app.keychain.identities.len();
    if page_header(
        ui,
        t!("Schlüssel"),
        count,
        &mut app.keychain_search,
        t!("Suchen"),
        t!("Neu"),
    ) {
        actions.push(Action::NewIdentity);
    }

    egui::ScrollArea::vertical().id_salt("keychain-page").auto_shrink([false, false]).show(ui, |ui| {
            // Room for the scroll bar next to the cards.
            ui.set_max_width(ui.available_width() - 14.0);
        let q = app.keychain_search.clone();
        let idents: Vec<_> = app
            .keychain
            .identities
            .iter()
            .filter(|i| fuzzy(&q, &format!("{} {}", i.label, i.username)).is_some())
            .cloned()
            .collect();
        if count == 0 {
            if empty_list(
                ui,
                t!("Noch keine Schlüssel"),
                t!("Speichere Passwörter und SSH-Keys einmal und verwende sie für beliebig viele Hosts."),
                t!("Schlüssel anlegen"),
            ) {
                actions.push(Action::NewIdentity);
            }
            return;
        }
        let mut rows: Vec<(crate::model::Identity, usize)> = Vec::new();
        for ident in idents {
            let used_by = app.lib.hosts.iter().filter(|h| h.identity_id == Some(ident.id)).count()
                + app.lib.groups.iter().filter(|g| g.default_identity == Some(ident.id)).count();
            rows.push((ident, used_by));
        }
        let mut toast = None;
        theme::card(ui, theme::bg_deep(), |ui| {
            for (i, (ident, used_by)) in rows.into_iter().enumerate() {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 68.0), Sense::click());
                let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                if hovered {
                    p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
                }
                if i > 0 {
                    p.line_segment(
                        [Pos2::new(rect.left() + 70.0, rect.top()), rect.right_top()],
                        Stroke::new(1.0, theme::line()),
                    );
                }
                let cy = rect.center().y;
                let kind = match (ident.private_key.is_empty(), ident.password.is_empty()) {
                    (false, false) => "K+P",
                    (false, true) => "KEY",
                    (true, false) => "PW",
                    (true, true) => "USR",
                };
                let badge = Rect::from_center_size(Pos2::new(rect.left() + 37.0, cy), Vec2::splat(38.0));
                p.rect_filled(badge, CornerRadius::same(10), theme::accent_soft());
                p.text(badge.center(), egui::Align2::CENTER_CENTER, kind, theme::bold(12.0), theme::accent());
                let title = if ident.label.is_empty() { t!("(ohne Namen)") } else { &ident.label };
                p.text(Pos2::new(rect.left() + 72.0, cy - 9.0), egui::Align2::LEFT_CENTER, title, theme::semibold(15.0), theme::text());
                p.text(
                    Pos2::new(rect.left() + 72.0, cy + 11.0),
                    egui::Align2::LEFT_CENTER,
                    if ident.username.is_empty() { "—" } else { &ident.username },
                    FontId::monospace(12.5),
                    theme::text_dim(),
                );
                // Matched by the German key, displayed translated.
                let mut keys = vec!["Bearbeiten", "Löschen"];
                if !ident.private_key.is_empty() {
                    keys.push("Public Key kopieren");
                }
                let labels: Vec<&str> = keys.iter().map(|k| crate::i18n::tr(k)).collect();
                if hovered {
                    match row_buttons(ui, rect, ident.id, &labels).map(|i| keys[i]) {
                        Some("Public Key kopieren") => match crate::keygen::public_info(&ident.private_key, &ident.passphrase) {
                            Ok((pubkey, _)) => {
                                ui.ctx().copy_text(pubkey);
                                toast = Some((t!("Public Key kopiert").to_string(), theme::accent()));
                            }
                            Err(e) => toast = Some((t!("Key ungültig: {}", e), theme::red())),
                        },
                        Some("Bearbeiten") => actions.push(Action::EditIdentity(ident.id)),
                        Some("Löschen") => actions.push(Action::Confirm(
                            t!("Schlüssel „{}“ löschen?", ident.label),
                            Box::new(Action::DeleteIdentity(ident.id)),
                        )),
                        _ => {}
                    }
                } else {
                    let usage = match used_by {
                        0 => t!("nicht verwendet").to_string(),
                        1 => t!("1 Host").to_string(),
                        n => t!("{} Hosts", n),
                    };
                    p.text(rect.right_center() - Vec2::new(18.0, 0.0), egui::Align2::RIGHT_CENTER, usage, FontId::proportional(13.0), theme::text_dim());
                }
                if resp.double_clicked() {
                    actions.push(Action::EditIdentity(ident.id));
                }
            }
        });
        if let Some((text, color)) = toast {
            app.toast(text, color);
        }
    });
}
