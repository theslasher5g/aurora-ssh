//! Keys & passwords: identities (user + password and/or private key) as a list.

use egui::{CornerRadius, FontId, Pos2, Rect, Sense, Vec2};

use crate::app::{Action, App};
use crate::theme;
use crate::ui::{empty_list, fuzzy, page_header, row_buttons};

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let count = app.keychain.identities.len();
    if page_header(
        ui,
        "Schlüssel",
        count,
        &mut app.keychain_search,
        "Suchen",
        "Neu",
    ) {
        actions.push(Action::NewIdentity);
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
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
                "Noch keine Schlüssel",
                "Speichere Passwörter und SSH-Keys einmal und verwende sie für beliebig viele Hosts.",
                "Schlüssel anlegen",
            ) {
                actions.push(Action::NewIdentity);
            }
            return;
        }
        for ident in idents {
            let used_by = app.lib.hosts.iter().filter(|h| h.identity_id == Some(ident.id)).count()
                + app.lib.groups.iter().filter(|g| g.default_identity == Some(ident.id)).count();
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 50.0), Sense::click());
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
            let cy = rect.center().y;
            let kind = match (ident.private_key.is_empty(), ident.password.is_empty()) {
                (false, false) => "KEY+PW",
                (false, true) => "KEY",
                (true, false) => "PW",
                (true, true) => "USER",
            };
            p.text(Pos2::new(rect.left() + 12.0, cy), egui::Align2::LEFT_CENTER, kind, FontId::monospace(10.5), theme::accent());
            let title = if ident.label.is_empty() { "(ohne Namen)" } else { &ident.label };
            p.text(Pos2::new(rect.left() + 84.0, cy - 8.0), egui::Align2::LEFT_CENTER, title, theme::semibold(14.0), theme::text());
            p.text(
                Pos2::new(rect.left() + 84.0, cy + 10.0),
                egui::Align2::LEFT_CENTER,
                if ident.username.is_empty() { "—" } else { &ident.username },
                FontId::monospace(12.0),
                theme::text_dim(),
            );
            let mut labels = vec!["Bearbeiten", "Löschen"];
            if !ident.private_key.is_empty() {
                labels.insert(0, "Public Key kopieren");
            }
            if hovered {
                match row_buttons(ui, rect, ident.id, &labels).map(|i| labels[i]) {
                    Some("Public Key kopieren") => match crate::keygen::public_info(&ident.private_key, &ident.passphrase) {
                        Ok((pubkey, _)) => {
                            ui.ctx().copy_text(pubkey);
                            app.toast("Public Key kopiert", theme::accent());
                        }
                        Err(e) => app.toast(format!("Key ungültig: {e}"), theme::red()),
                    },
                    Some("Bearbeiten") => actions.push(Action::EditIdentity(ident.id)),
                    Some("Löschen") => actions.push(Action::Confirm(
                        format!("Schlüssel „{}“ löschen?", ident.label),
                        Box::new(Action::DeleteIdentity(ident.id)),
                    )),
                    _ => {}
                }
            } else {
                let usage = match used_by {
                    0 => "nicht verwendet".to_string(),
                    1 => "1 Host".to_string(),
                    n => format!("{n} Hosts"),
                };
                p.text(rect.right_center() - Vec2::new(14.0, 0.0), egui::Align2::RIGHT_CENTER, usage, FontId::monospace(11.5), theme::text_dim());
            }
            if resp.double_clicked() {
                actions.push(Action::EditIdentity(ident.id));
            }
        }
    });
}
