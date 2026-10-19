//! Command palette (Ctrl/Cmd+K): fuzzy search across hosts, snippets and actions.

use egui::{CornerRadius, Margin, RichText, Vec2};

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
                icon: crate::ui::hosts::protocol_tag(h.protocol).into(),
                title: h.display_name().to_string(),
                subtitle: h.subtitle(),
                action: Action::Connect(h.id),
                score: score + 30,
            });
        }
        if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp)
            && let Some(score) = fuzzy(&q, &format!("sftp dateien {} {hay}", t!("Dateien")))
        {
            items.push(Item {
                icon: "SFTP".into(),
                title: format!("SFTP: {}", h.display_name()),
                subtitle: t!("Dateien").into(),
                action: Action::OpenSftp(h.id),
                score,
            });
        }
        for t in &h.tunnels {
            if let Some(score) = fuzzy(&q, &format!("tunnel {} {hay}", t.label())) {
                let on = app.tunnels.contains_key(&t.id);
                items.push(Item {
                    icon: "TUN".into(),
                    title: if on {
                        t!("Tunnel schließen: {} ({})", t.label(), h.display_name())
                    } else {
                        t!("Tunnel öffnen: {} ({})", t.label(), h.display_name())
                    },
                    subtitle: format!("localhost:{}", t.local_port),
                    action: Action::ToggleTunnel(h.id, t.id),
                    score: score - 5,
                });
            }
        }
    }
    for s in &app.lib.snippets {
        if let Some(score) = fuzzy(&q, &format!("{} {}", s.name, s.script)) {
            items.push(Item {
                icon: "SNIP".into(),
                title: s.name.clone(),
                subtitle: s.script.lines().next().unwrap_or_default().to_string(),
                action: Action::RunSnippet(s.id, false),
                score,
            });
        }
    }
    for (title, subtitle, action, score) in crate::ui::history::palette_items(app, &q) {
        items.push(Item {
            icon: "HIST".into(),
            title,
            subtitle,
            action,
            score,
        });
    }
    for (icon, title, subtitle, action) in crate::ui::proxmox::palette_items(app) {
        if let Some(score) = fuzzy(&q, &format!("{title} proxmox {subtitle}")) {
            items.push(Item {
                icon,
                title,
                subtitle,
                action,
                score: score + 10,
            });
        }
    }
    let commands = [
        ("CMD", "Neuer Host", Action::NewHost(None)),
        ("CMD", "Neue Gruppe", Action::NewGroup(None)),
        ("CMD", "Neue Identität", Action::NewIdentity),
        ("CMD", "Neues Snippet", Action::NewSnippet),
        ("CMD", "Lokales Terminal", Action::LocalTerminal),
        ("CMD", "Datei im Editor öffnen…", Action::EditPrompt),
        (
            "CMD",
            "Verbindungen anzeigen",
            Action::Navigate(View::Hosts),
        ),
        (
            "CMD",
            "Schlüssel anzeigen",
            Action::Navigate(View::Keychain),
        ),
        ("CMD", "Snippets anzeigen", Action::Navigate(View::Snippets)),
        ("CMD", "Proxmox anzeigen", Action::Navigate(View::Proxmox)),
        ("CMD", "Einstellungen", Action::Navigate(View::Settings)),
        ("CMD", "Tresor sperren", Action::Lock),
    ];
    for t in crate::theme::THEMES {
        let title = format!("Theme: {}", t.name);
        if let Some(score) = fuzzy(&q, &title) {
            items.push(Item {
                icon: "THEME".into(),
                title,
                subtitle: String::new(),
                action: Action::SetTheme(t.id.to_string()),
                score: score - 20,
            });
        }
    }
    for (icon, title, action) in commands {
        let title = crate::i18n::tr(title);
        if let Some(score) = fuzzy(&q, title) {
            items.push(Item {
                icon: icon.into(),
                title: title.into(),
                subtitle: String::new(),
                action,
                score: score - 10,
            });
        }
    }
    // Quick connect: `user@host`, `ssh -p 22 host` rank first; a bare hostname or IP
    // is offered below matching hosts.
    let explicit = q.contains('@') || q.trim_start().starts_with("ssh ");
    let hostish = q.contains('.') || q.contains(':') || q.trim() == "localhost";
    if crate::app::parse_quick(&q).is_some() && (explicit || hostish) {
        items.push(Item {
            icon: "SSH".into(),
            title: t!("Direkt verbinden: {}", q.trim()),
            subtitle: "Enter".into(),
            action: Action::QuickConnect(q.clone()),
            score: if explicit { i32::MAX } else { 5 },
        });
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
        .frame(theme::dialog_frame().inner_margin(Margin::same(10)))
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(600.0);
            let r = ui
                .horizontal(|ui| {
                    ui.add_space(8.0);
                    crate::ui::icons::show(
                        ui,
                        crate::ui::icons::Icon::Search,
                        18.0,
                        theme::text_dim(),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut app.palette.query)
                            .hint_text(t!("Host, Snippet, Befehl oder user@host"))
                            .frame(egui::Frame::NONE)
                            .font(theme::medium(18.0))
                            .margin(egui::Margin::symmetric(6, 10))
                            .desired_width(f32::INFINITY),
                    )
                })
                .inner;
            r.request_focus();
            if r.changed() {
                app.palette.selected = 0;
            }
            theme::hairline(ui);
            ui.add_space(6.0);
            if items.is_empty() {
                ui.label(RichText::new(t!("Keine Treffer")).color(theme::text_dim()));
            }
            for (i, item) in items.iter().enumerate() {
                let selected = i == app.palette.selected;
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 42.0),
                    egui::Sense::click(),
                );
                if selected || resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(10),
                        if selected {
                            theme::accent_soft()
                        } else {
                            theme::hover()
                        },
                    );
                }
                let p = ui.painter();
                let kind = p.layout_no_wrap(
                    item.icon.clone(),
                    theme::bold(10.0),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text_dim()
                    },
                );
                let chip = egui::Rect::from_min_size(
                    rect.left_center() + Vec2::new(10.0, -10.0),
                    Vec2::new(48.0, 20.0),
                );
                p.rect_filled(
                    chip,
                    CornerRadius::same(6),
                    if selected {
                        theme::accent_soft()
                    } else {
                        theme::fill()
                    },
                );
                p.galley(chip.center() - kind.size() / 2.0, kind, theme::text_dim());
                p.text(
                    rect.left_center() + Vec2::new(70.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &item.title,
                    theme::semibold(14.5),
                    theme::text(),
                );
                p.with_clip_rect(rect).text(
                    rect.right_center() - Vec2::new(12.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    &item.subtitle,
                    egui::FontId::monospace(11.5),
                    theme::text_dim(),
                );
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.x = 5.0;
                for (keys, what) in [
                    (&["↑", "↓"][..], t!("auswählen")),
                    (&["Enter"][..], t!("ausführen")),
                    (&["Esc"][..], t!("schließen")),
                ] {
                    for k in keys {
                        theme::kbd(ui, k);
                    }
                    ui.label(RichText::new(what).size(12.0).color(theme::text_dim()));
                    ui.add_space(10.0);
                }
            });
        });

    if let Some(i) = chosen {
        actions.push(items.swap_remove(i).action);
        app.palette.open = false;
    }
    if esc || modal.should_close() {
        app.palette.open = false;
    }
}
