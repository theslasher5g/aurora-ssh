//! Settings, split into tabs: appearance, terminal, general, security, data & updates.

use egui::{CornerRadius, RichText, Vec2};

use crate::app::{Action, App};
use crate::theme;

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    theme::page_title(ui, t!("Einstellungen"), None);
    ui.add_space(18.0);
    let tabs = [
        t!("Darstellung"),
        "Terminal",
        t!("Allgemein"),
        t!("Sicherheit"),
        t!("Daten & Updates"),
    ];
    if let Some(i) = theme::segmented(ui, &tabs, app.settings_tab) {
        app.settings_tab = i;
    }
    ui.add_space(22.0);
    egui::ScrollArea::vertical()
        .id_salt("settings-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the scroll bar next to the cards.
            ui.set_max_width((ui.available_width() - 14.0).min(860.0));
            let mut changed = false;
            match app.settings_tab {
                0 => appearance(app, ui, &mut changed),
                1 => terminal(app, ui, actions, &mut changed),
                2 => general(app, ui, actions, &mut changed),
                3 => security(app, ui, actions, &mut changed),
                _ => data(app, ui, &mut changed),
            }
            ui.add_space(40.0);
            if changed {
                app.save_library();
            }
        });
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, text);
    });
    ui.add_space(2.0);
}

fn appearance(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    let w = ui.available_width();
    let cols = ((w + 12.0) / (170.0 + 12.0)).floor().max(1.0) as usize;
    let card_w = (w - 12.0 * (cols as f32 - 1.0)) / cols as f32;
    for (dark, label) in [(true, t!("Design · Dunkel")), (false, t!("Design · Hell"))] {
        caption(ui, label);
        let themes: Vec<theme::Palette> = theme::THEMES
            .iter()
            .copied()
            .filter(|t| t.dark == dark)
            .collect();
        for row in themes.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for t in row {
                    if theme_card(ui, t, card_w, app.lib.settings.theme == t.id).clicked() {
                        app.lib.settings.theme = t.id.to_string();
                        theme::apply(ui.ctx(), t.id);
                        *changed = true;
                    }
                }
            });
            ui.add_space(12.0);
        }
        ui.add_space(12.0);
    }
    crate::ui::themeimport_ui::section(app, ui, changed);
    ui.add_space(14.0);
    caption(ui, t!("Ansicht"));
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Kompakte Hostliste"),
            Some(t!(
                "Einzeilige Einträge, damit mehr Hosts auf den Bildschirm passen"
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.compact).changed();
            },
        );
        rows.setting(
            ui,
            t!("Erreichbarkeit anzeigen"),
            Some(t!("Hosts in der Liste regelmäßig anpingen (TCP)")),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.reachability).changed();
            },
        );
    });
}

fn terminal(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>, changed: &mut bool) {
    caption(ui, "Terminal");
    let family = theme::term_font(ui.ctx(), &app.lib.settings.term_font);
    let size = app.lib.settings.font_size;
    theme::settings_card(ui, |ui, rows| {
        terminal_preview(ui, &theme::palette(), family, size);
        rows.setting(
            ui,
            t!("Schrift"),
            Some(t!(
                "Pro Host abweichend einstellbar: Host bearbeiten → Terminal-Profil"
            )),
            |ui| {
                let current = if app.lib.settings.term_font.is_empty() {
                    theme::DEFAULT_TERM_FONT.to_string()
                } else {
                    app.lib.settings.term_font.clone()
                };
                egui::ComboBox::from_id_salt("term-font")
                    .selected_text(&current)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for f in theme::term_fonts() {
                            let value = if f == theme::DEFAULT_TERM_FONT {
                                String::new()
                            } else {
                                f.clone()
                            };
                            *changed |= ui
                                .selectable_value(&mut app.lib.settings.term_font, value, f)
                                .changed();
                        }
                    });
            },
        );
        rows.setting(ui, t!("Schriftgröße"), None, |ui| {
            // Right to left: value box, then the slider with an accent-filled track.
            ui.spacing_mut().item_spacing.x = 16.0;
            *changed |= ui
                .add(
                    egui::DragValue::new(&mut app.lib.settings.font_size)
                        .range(9.0..=28.0)
                        .speed(0.1)
                        .fixed_decimals(1),
                )
                .changed();
            ui.scope(|ui| {
                let v = ui.visuals_mut();
                v.selection.bg_fill = theme::accent();
                v.widgets.inactive.bg_fill = egui::Color32::WHITE;
                v.widgets.hovered.bg_fill = egui::Color32::WHITE;
                v.widgets.active.bg_fill = egui::Color32::WHITE;
                v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, theme::line());
                ui.spacing_mut().slider_width = 220.0;
                *changed |= ui
                    .add(
                        egui::Slider::new(&mut app.lib.settings.font_size, 9.0..=28.0)
                            .step_by(0.5)
                            .show_value(false)
                            .trailing_fill(true),
                    )
                    .changed();
            });
        });
        rows.setting(
            ui,
            "Scrollback",
            Some(t!("Wie viele Zeilen pro Terminal im Speicher bleiben")),
            |ui| {
                let lines = |n: usize| t!("{} Zeilen", n);
                let mut options = vec![1_000, 5_000, 10_000, 50_000, 100_000];
                if !options.contains(&app.lib.settings.scrollback) {
                    options.push(app.lib.settings.scrollback);
                    options.sort_unstable();
                }
                egui::ComboBox::from_id_salt("scrollback")
                    .selected_text(lines(app.lib.settings.scrollback))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for n in options {
                            *changed |= ui
                                .selectable_value(&mut app.lib.settings.scrollback, n, lines(n))
                                .changed();
                        }
                    });
            },
        );
        rows.setting(
            ui,
            t!("Kopieren bei Auswahl"),
            Some(t!("Markierter Text landet sofort in der Zwischenablage")),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.copy_on_select).changed();
            },
        );
    });
    ui.add_space(22.0);
    log_commands(app, ui, changed);
    ui.add_space(22.0);
    recordings(app, ui, actions, changed);
    ui.add_space(22.0);
    caption(ui, t!("Gedächtnis & Schutz"));
    crate::ui::history::settings_card(app, ui, changed);
}

/// The user's own log viewer sources.
fn log_commands(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    caption(ui, t!("Eigene Log-Befehle"));
    let hosts: Vec<(uuid::Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == crate::model::Protocol::Ssh)
        .map(|h| (h.id, h.display_name().to_string()))
        .collect();
    let mut remove = None;
    theme::settings_card(ui, |ui, rows| {
        rows.full(ui, |ui| {
            ui.label(
                RichText::new(t!(
                    "Erscheinen in der Log-Leiste einer SSH-Session (Ctrl+Shift+L) und unter „Log ansehen“. Eine Datei wird mit tail -F verfolgt, alles andere als Befehl ausgeführt. Mit {{variable}} oder {{variable=Standard}} fragt Aurora beim Start nach dem Wert, z. B. journalctl -u {{dienst}} -n {{zeilen=200}} – ein Befehl für alle Dienste."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
        });
        for (i, c) in app.lib.log_commands.iter_mut().enumerate() {
            rows.full(ui, |ui| {
                ui.horizontal(|ui| {
                    *changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut c.name)
                                .hint_text(t!("Name"))
                                .desired_width(150.0),
                        )
                        .changed();
                    let host_label = c
                        .host
                        .and_then(|h| hosts.iter().find(|(id, _)| *id == h))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| t!("Alle Hosts").to_string());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_btn(ui, crate::ui::icons::Icon::Trash, t!("Löschen"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        egui::ComboBox::from_id_salt(("log-cmd-host", c.id))
                            .selected_text(host_label)
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                *changed |= ui
                                    .selectable_value(&mut c.host, None, t!("Alle Hosts"))
                                    .changed();
                                for (id, name) in &hosts {
                                    *changed |=
                                        ui.selectable_value(&mut c.host, Some(*id), name).changed();
                                }
                            });
                        *changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut c.command)
                                    .hint_text("journalctl -f -u nginx")
                                    .font(egui::FontId::monospace(13.0))
                                    .desired_width(ui.available_width()),
                            )
                            .changed();
                    });
                });
            });
        }
        rows.full(ui, |ui| {
            if theme::btn(
                ui,
                theme::Btn::Small,
                Some(crate::ui::icons::Icon::Plus),
                t!("Neuer Log-Befehl"),
            )
            .clicked()
            {
                app.lib
                    .log_commands
                    .push(crate::model::LogCommand::default());
                *changed = true;
            }
        });
    });
    if let Some(i) = remove {
        app.lib.log_commands.remove(i);
        *changed = true;
    }
}

/// Session recordings: auto-record switch, folder and the latest files.
fn recordings(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>, changed: &mut bool) {
    caption(ui, t!("Aufnahmen"));
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Alle SSH-Sessions aufzeichnen"),
            Some(t!(
                "Sonst über den Aufnahme-Knopf in der Session. Aufgezeichnet wird, was das Terminal zeigt (asciinema-Format)."
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.record_sessions).changed();
            },
        );
        let dir = crate::backend::record::dir();
        rows.setting(ui, t!("Ordner"), Some(&dir.display().to_string()), |ui| {
            if theme::btn(ui, theme::Btn::Small, None, t!("Ordner öffnen")).clicked()
                && let Err(e) = crate::backend::record::open_folder()
            {
                app.toast(e.to_string(), theme::red());
            }
        });
        let files = crate::backend::record::list();
        if files.is_empty() {
            rows.full(ui, |ui| {
                ui.label(RichText::new(t!("Noch keine Aufnahmen")).color(theme::text_dim()));
            });
        }
        for (path, size, modified) in files.into_iter().take(12) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let when: chrono::DateTime<chrono::Local> = modified.into();
            let help = format!(
                "{} · {:.1} KB",
                when.format("%d.%m.%Y %H:%M"),
                size as f64 / 1024.0
            );
            rows.setting(ui, &name, Some(&help), |ui| {
                if theme::icon_btn(ui, crate::ui::icons::Icon::Trash, t!("Löschen")).clicked() {
                    let _ = std::fs::remove_file(&path);
                }
                if theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(crate::ui::icons::Icon::Play),
                    t!("Abspielen"),
                )
                .clicked()
                {
                    actions.push(Action::PlayRecording(path.clone()));
                }
            });
        }
    });
}

fn general(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>, changed: &mut bool) {
    caption(ui, t!("Allgemein"));
    theme::settings_card(ui, |ui, rows| {
        rows.setting(ui, "Sprache / Language", None, |ui| {
            let codes = ["", "de", "en"];
            let cur = codes
                .iter()
                .position(|c| *c == app.lib.settings.language)
                .unwrap_or(0);
            if let Some(i) = theme::segmented(ui, &["System", "Deutsch", "English"], cur) {
                app.lib.settings.language = codes[i].to_string();
                crate::i18n::set_language(codes[i]);
                *changed = true;
            }
        });
        rows.setting(
            ui,
            t!("Sessions wiederherstellen"),
            Some(t!("beim Start wieder öffnen")),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.restore_sessions).changed();
            },
        );
        crate::ui::reconnect_auto::settings_rows(app, ui, rows, changed);
        rows.setting(
            ui,
            "Editor",
            Some(t!(
                "Programm zum Bearbeiten von Server-Dateien; leer = Standard-App des Systems"
            )),
            |ui| {
                *changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut app.lib.settings.editor)
                            .hint_text(t!("Standard-App (z.B. code, notepad++)"))
                            .desired_width(240.0),
                    )
                    .changed();
            },
        );
    });
    ui.add_space(22.0);
    caption(ui, t!("Hilfe"));
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Einführung"),
            Some(t!("Die kurze Tour vom ersten Start")),
            |ui| {
                if theme::btn(ui, theme::Btn::Small, None, t!("Einführung zeigen")).clicked() {
                    app.lib.settings.onboarded = false;
                    app.tour_step = 0;
                    *changed = true;
                }
            },
        );
        rows.setting(
            ui,
            t!("Tastenkürzel"),
            Some(t!("Öffnen mit F1 oder ?")),
            |ui| {
                if theme::btn(ui, theme::Btn::Small, None, t!("Anzeigen")).clicked() {
                    actions.push(Action::Navigate(crate::app::View::Help));
                }
            },
        );
        rows.setting(
            ui,
            t!("SSH-Konfiguration"),
            Some(t!("Hosts aus ~/.ssh/config übernehmen")),
            |ui| {
                if theme::btn(ui, theme::Btn::Small, None, t!("Importieren")).clicked() {
                    actions.push(Action::ImportSshConfig);
                }
            },
        );
    });
    ui.add_space(22.0);
    crate::ui::inventory_page::settings_card(app, ui, changed);
}

fn security(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>, changed: &mut bool) {
    caption(ui, t!("Sicherheit"));
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Automatisch sperren"),
            Some(t!("ohne Eingabe – offene Sessions laufen weiter")),
            |ui| {
                let m = app.lib.settings.auto_lock_minutes;
                let text = |m: u32| {
                    if m == 0 {
                        t!("nie").to_string()
                    } else {
                        t!("nach {} min", m)
                    }
                };
                egui::ComboBox::from_id_salt("autolock")
                    .selected_text(text(m))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for v in [0, 5, 15, 30, 60, 120] {
                            *changed |= ui
                                .selectable_value(
                                    &mut app.lib.settings.auto_lock_minutes,
                                    v,
                                    text(v),
                                )
                                .changed();
                        }
                    });
            },
        );
        rows.setting(
            ui,
            t!("Gefährliche Befehle"),
            Some(t!("auf allen Hosts nachfragen (sonst nur bei PROD)")),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.warn_everywhere).changed();
            },
        );
    });
    ui.add_space(22.0);
    crate::ui::pwmanager_ui::settings_card(app, ui, changed);
    ui.add_space(22.0);
    caption(ui, t!("Master-Passwort"));
    theme::settings_card(ui, |ui, rows| {
        rows.full(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add(
                    egui::TextEdit::singleline(&mut app.pw_change.new)
                        .password(true)
                        .hint_text(t!("Neu"))
                        .desired_width(200.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut app.pw_change.confirm)
                        .password(true)
                        .hint_text(t!("Wiederholen"))
                        .desired_width(200.0),
                );
                if theme::btn(ui, theme::Btn::Normal, None, t!("Ändern")).clicked() {
                    if app.pw_change.new.chars().count() < 8 {
                        app.toast(t!("Mindestens 8 Zeichen"), theme::amber());
                    } else if app.pw_change.new != app.pw_change.confirm {
                        app.toast(t!("Passwörter stimmen nicht überein"), theme::amber());
                    } else if let Some(v) = app.vault.as_mut() {
                        match v.change_password(&app.pw_change.new, &app.keychain) {
                            Ok(()) => {
                                app.pw_change = Default::default();
                                app.toast(t!("Master-Passwort geändert"), theme::green());
                            }
                            Err(e) => app.toast(format!("{e:#}"), theme::red()),
                        }
                    }
                }
            });
        });
    });
    ui.add_space(22.0);
    known_hosts(app, ui, actions);
}

fn data(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    updates(app, ui, changed);
    ui.add_space(22.0);
    caption(ui, "Export / Import");
    theme::settings_card(ui, |ui, rows| {
        rows.full(ui, |ui| {
            ui.label(
                RichText::new(t!(
                    "Hosts, Gruppen, Snippets und Schlüssel in eine verschlüsselte Datei – \
                     zum Umziehen auf einen anderen PC oder als Backup."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut app.export_path)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::monospace(12.5)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add(
                    egui::TextEdit::singleline(&mut app.export_pw)
                        .password(true)
                        .hint_text(t!("Passwort für die Datei"))
                        .desired_width(240.0),
                );
                if theme::btn(ui, theme::Btn::Normal, None, t!("Exportieren")).clicked() {
                    app.export_data();
                }
                if theme::btn(ui, theme::Btn::Normal, None, t!("Importieren")).clicked() {
                    app.import_data();
                }
            });
        });
    });
    ui.add_space(22.0);
    caption(ui, t!("Daten"));
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Datenordner"),
            Some(&app.data_dir.display().to_string()),
            |_| {},
        );
        rows.setting(
            ui,
            t!("Serielle Ports"),
            Some(&t!("{} gefunden", app.serial_ports.len())),
            |ui| {
                if theme::btn(ui, theme::Btn::Small, None, t!("Neu scannen")).clicked() {
                    app.serial_ports = crate::backend::serial::available_ports();
                    app.toast(
                        t!("{} serielle Ports", app.serial_ports.len()),
                        theme::accent(),
                    );
                }
            },
        );
    });
}

fn updates(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    caption(ui, "Updates");
    theme::settings_card(ui, |ui, rows| {
        let help = if let Some((msg, _)) = &app.update.message {
            msg.clone()
        } else if let Some(rel) = app.update.latest.as_ref().filter(|_| !app.update.installed) {
            t!("Neue Version {} verfügbar.", rel.version_text())
        } else {
            t!("Du hast die aktuelle Version.").to_string()
        };
        rows.setting(
            ui,
            &format!("Aurora {}", env!("CARGO_PKG_VERSION")),
            Some(&help),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if app.update.installed {
                    if theme::btn(ui, theme::Btn::Primary, None, t!("Neu starten")).clicked() {
                        crate::update::restart();
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else if let Some(rel) = app.update.latest.clone() {
                    let kind = crate::update::InstallKind::detect();
                    if app.update.installing {
                        ui.label(
                            theme::mono(format!("{:.0} %", app.update.progress * 100.0), 12.5)
                                .color(theme::accent()),
                        );
                        ui.spinner();
                    } else if kind.can_self_update() {
                        if theme::btn(
                            ui,
                            theme::Btn::Primary,
                            None,
                            &t!("Auf {} aktualisieren", rel.version_text()),
                        )
                        .clicked()
                        {
                            app.install_update();
                        }
                    } else if theme::btn(ui, theme::Btn::Primary, None, t!("Download-Seite öffnen"))
                        .clicked()
                    {
                        crate::update::open_url(&rel.page);
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Änderungen")).clicked() {
                        crate::update::open_url(&rel.page);
                    }
                } else if app.update.checking {
                    ui.spinner();
                } else if theme::btn(ui, theme::Btn::Normal, None, t!("Jetzt prüfen")).clicked() {
                    app.check_updates();
                }
            },
        );
        rows.setting(
            ui,
            t!("Automatisch prüfen"),
            Some(t!("Beim Start automatisch nach Updates suchen")),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.check_updates).changed();
            },
        );
    });
}

/// Clickable mini preview of a theme, drawn in the theme's own colours.
pub(crate) fn theme_card(
    ui: &mut egui::Ui,
    t: &theme::Palette,
    width: f32,
    selected: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 104.0), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), t.bg_card);
    let ring = if selected {
        egui::Stroke::new(2.0, theme::accent())
    } else if resp.hovered() {
        egui::Stroke::new(1.0, theme::text_dim())
    } else {
        egui::Stroke::new(1.0, theme::line())
    };
    // Mini window: sidebar + a panel with three coloured lines.
    let top = egui::Rect::from_min_size(
        rect.min + Vec2::new(10.0, 10.0),
        Vec2::new(rect.width() - 20.0, 58.0),
    );
    let side = egui::Rect::from_min_size(top.min, Vec2::new(26.0, top.height()));
    p.rect_filled(side, CornerRadius::same(6), t.bg_panel);
    let panel = egui::Rect::from_min_max(side.right_top() + Vec2::new(6.0, 0.0), top.max);
    p.rect_filled(
        panel,
        CornerRadius::same(6),
        egui::Color32::from_rgba_unmultiplied(127, 127, 127, 31),
    );
    for (i, (w, color)) in [(0.8f32, t.ansi[10]), (0.55, t.ansi[14]), (0.35, t.ansi[12])]
        .into_iter()
        .enumerate()
    {
        p.rect_filled(
            egui::Rect::from_min_size(
                panel.min + Vec2::new(8.0, 8.0 + i as f32 * 9.0),
                Vec2::new((panel.width() - 16.0) * w, 3.0),
            ),
            CornerRadius::same(2),
            color,
        );
    }
    let cy = rect.bottom() - 18.0;
    p.circle_filled(egui::pos2(rect.left() + 16.0, cy), 4.0, t.accent);
    p.text(
        egui::pos2(rect.left() + 28.0, cy),
        egui::Align2::LEFT_CENTER,
        t.name,
        theme::semibold(13.0),
        t.text,
    );
    p.rect_stroke(
        rect,
        CornerRadius::same(12),
        ring,
        egui::StrokeKind::Outside,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Trusted server keys (TOFU); a changed key triggers a MITM warning on connect.
fn known_hosts(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    caption(ui, t!("Vertrauenswürdige Server"));
    let mut entries: Vec<_> = app
        .lib
        .known_hosts
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    entries.sort();
    theme::settings_card(ui, |ui, rows| {
        if entries.is_empty() {
            rows.full(ui, |ui| {
                ui.label(RichText::new(t!("Noch keine")).color(theme::text_dim()));
            });
        }
        for (host, fp) in entries {
            rows.full(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(theme::mono(&host, 12.5).color(theme::text()));
                        ui.add(
                            egui::Label::new(
                                RichText::new(&fp)
                                    .monospace()
                                    .size(11.0)
                                    .color(theme::text_dim()),
                            )
                            .truncate(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_btn(ui, crate::ui::icons::Icon::Close, t!("Entfernen"))
                            .clicked()
                        {
                            actions.push(Action::ForgetKnownHost(host.clone()));
                        }
                    });
                });
            });
        }
    });
}

/// A prompt line drawn with the given terminal colours, font and size, so font and theme
/// changes can be judged before opening a session.
pub fn terminal_preview(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    family: egui::FontFamily,
    size: f32,
) {
    let font = egui::FontId::new(size, family);
    let h = (size * 1.4 + 40.0).max(56.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), h), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, CornerRadius::ZERO, p.term_bg);
    let mut job = egui::text::LayoutJob::default();
    let part = |job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32| {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };
    part(&mut job, "root@pve00", p.ansi[10]);
    part(&mut job, ":", p.term_fg);
    part(&mut job, "~", p.ansi[12]);
    part(&mut job, "# ls -la ", p.term_fg);
    let galley = painter.layout_job(job);
    let pos = egui::pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0);
    let w = galley.size().x;
    let gh = galley.size().y;
    painter.galley(pos, galley, p.term_fg);
    // Block cursor as in the terminal.
    let cw = size * 0.6;
    painter.rect_filled(
        egui::Rect::from_min_size(egui::pos2(pos.x + w, pos.y), Vec2::new(cw, gh)),
        CornerRadius::same(1),
        p.accent,
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0, theme::line()),
    );
}
