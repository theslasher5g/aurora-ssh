//! Command palette (Ctrl/Cmd+K): fuzzy search across hosts, snippets and actions.

use egui::{CornerRadius, Margin, RichText, Stroke, Vec2};

use crate::app::{Action, App, View};
use crate::model::Protocol;
use crate::theme;
use crate::ui::fuzzy;

struct Item {
    icon: String,
    title: String,
    subtitle: String,
    action: Action,
    score: i32,
}

pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    if !app.palette.open {
        return;
    }
    let q = app.palette.query.clone();
    let mut items: Vec<Item> = Vec::new();
    for h in &app.lib.hosts {
        let hay = format!("{} {} {}", h.display_name(), h.hostname, h.tags.join(" "));
        if let Some(score) = fuzzy(&q, &hay) {
            items.push(Item {
                icon: h.protocol.icon().into(),
                title: h.display_name().to_string(),
                subtitle: format!("Verbinden · {}", h.subtitle()),
                action: Action::Connect(h.id),
                score: score + 30,
            });
            if h.protocol == Protocol::Ssh
                && let Some(score) = fuzzy(&q, &format!("sftp {hay}"))
            {
                items.push(Item {
                    icon: "🗁".into(),
                    title: format!("SFTP: {}", h.display_name()),
                    subtitle: "Dateien übertragen".into(),
                    action: Action::OpenSftp(h.id),
                    score,
                });
            }
        }
    }
    for s in &app.lib.snippets {
        if let Some(score) = fuzzy(&q, &format!("{} {}", s.name, s.script)) {
            items.push(Item {
                icon: "⚡".into(),
                title: s.name.clone(),
                subtitle: s.script.lines().next().unwrap_or_default().to_string(),
                action: Action::RunSnippet(s.id, false),
                score,
            });
        }
    }
    let commands = [
        ("➕", "Neuer Host", Action::NewHost(None)),
        ("🗀", "Neue Gruppe", Action::NewGroup(None)),
        ("🔑", "Neue Identität", Action::NewIdentity),
        ("⚡", "Neues Snippet", Action::NewSnippet),
        ("🖧", "Hosts anzeigen", Action::Navigate(View::Hosts)),
        ("🔑", "Keychain anzeigen", Action::Navigate(View::Keychain)),
        ("⚡", "Snippets anzeigen", Action::Navigate(View::Snippets)),
        ("⚙", "Einstellungen", Action::Navigate(View::Settings)),
        ("🔒", "Vault sperren", Action::Lock),
    ];
    for (icon, title, action) in commands {
        if let Some(score) = fuzzy(&q, title) {
            items.push(Item {
                icon: icon.into(),
                title: title.into(),
                subtitle: "Befehl".into(),
                action,
                score: score - 10,
            });
        }
    }
    if crate::app::parse_quick(&q).is_some() && q.contains('@') {
        items.insert(
            0,
            Item {
                icon: "⚡".into(),
                title: format!("Quick Connect: {q}"),
                subtitle: "Ad-hoc SSH-Verbindung".into(),
                action: Action::QuickConnect(q.clone()),
                score: i32::MAX,
            },
        );
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.score));
    items.truncate(12);
    let n = items.len();
    if app.palette.selected >= n {
        app.palette.selected = n.saturating_sub(1);
    }

    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if up {
        app.palette.selected = app.palette.selected.saturating_sub(1);
    }
    if down && n > 0 {
        app.palette.selected = (app.palette.selected + 1).min(n - 1);
    }
    let mut chosen = None;
    if enter && n > 0 {
        chosen = Some(app.palette.selected);
    }

    let modal = egui::Modal::new(egui::Id::new("palette"))
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CARD)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(12)),
        )
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔍").size(18.0).color(theme::TEAL));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.palette.query)
                        .hint_text("Host, Snippet oder Befehl… (user@host für Quick Connect)")
                        .frame(egui::Frame::NONE)
                        .font(egui::FontId::proportional(17.0))
                        .desired_width(f32::INFINITY),
                );
                r.request_focus();
                if r.changed() {
                    app.palette.selected = 0;
                }
            });
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), egui::Sense::hover());
            theme::gradient_rect(ui.painter(), rect, theme::TEAL, theme::VIOLET, 1.0);
            ui.add_space(4.0);
            if items.is_empty() {
                ui.label(RichText::new("Keine Treffer").color(theme::TEXT_DIM));
            }
            for (i, item) in items.iter().enumerate() {
                let selected = i == app.palette.selected;
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 44.0),
                    egui::Sense::click(),
                );
                if selected || resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(10),
                        if selected {
                            theme::BG_HOVER
                        } else {
                            theme::BG_PANEL
                        },
                    );
                }
                if selected {
                    let bar = egui::Rect::from_min_size(
                        rect.left_top() + Vec2::new(0.0, 10.0),
                        Vec2::new(3.0, 24.0),
                    );
                    ui.painter()
                        .rect_filled(bar, CornerRadius::same(2), theme::TEAL);
                }
                let p = ui.painter();
                p.text(
                    rect.left_center() + Vec2::new(16.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &item.icon,
                    egui::FontId::proportional(16.0),
                    theme::TEAL,
                );
                p.text(
                    rect.left_top() + Vec2::new(44.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    &item.title,
                    egui::FontId::proportional(14.5),
                    theme::TEXT,
                );
                p.text(
                    rect.left_bottom() + Vec2::new(44.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    &item.subtitle,
                    egui::FontId::proportional(11.5),
                    theme::TEXT_DIM,
                );
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("⬆⬇ auswählen · Enter ausführen · Esc schließen")
                    .size(11.0)
                    .color(theme::TEXT_DIM),
            );
        });

    if let Some(i) = chosen {
        actions.push(items.swap_remove(i).action);
        app.palette.open = false;
    }
    if esc || modal.should_close() {
        app.palette.open = false;
    }
}
