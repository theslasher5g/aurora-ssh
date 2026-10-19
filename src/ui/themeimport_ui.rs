//! Settings → Darstellung: imported themes ("Eigene") and the import card.

use egui::{CornerRadius, RichText, Vec2};

use crate::app::App;
use crate::theme;
use crate::themeimport_convert::{palette_of, register};
use crate::themeimport_parse::{self as parse, Scheme};

#[derive(Default)]
pub struct State {
    path: String,
    paste: String,
    /// Parsed candidates waiting to be saved, with the picked one.
    draft: Vec<Scheme>,
    picked: usize,
    name: String,
    error: Option<String>,
    /// Theme being renamed: (id, new name).
    renaming: Option<(String, String)>,
    /// Theme waiting for the second click on "Löschen".
    confirm_delete: Option<String>,
    message: Option<String>,
}

fn unique_name(app: &App, wanted: &str) -> String {
    let wanted = wanted.trim();
    let wanted = if wanted.is_empty() { "Theme" } else { wanted };
    let taken = |n: &str| {
        theme::THEMES.iter().any(|t| t.name == n)
            || app.lib.settings.custom_themes.iter().any(|t| t.name == n)
    };
    if !taken(wanted) {
        return wanted.to_string();
    }
    (2..)
        .map(|i| format!("{wanted} ({i})"))
        .find(|n| !taken(n))
        .unwrap_or_default()
}

fn new_id() -> String {
    format!("custom-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// Reads a file (size checked before reading) and parses it.
fn load_file(path: &str) -> Result<Vec<Scheme>, String> {
    use std::io::Read;
    let path = path.trim().trim_matches('"');
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => directories::UserDirs::new()
            .map(|d| d.home_dir().join(rest))
            .unwrap_or_else(|| path.into()),
        None => std::path::PathBuf::from(path),
    };
    let meta = std::fs::metadata(&expanded).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(t!("Das ist keine Datei.").to_string());
    }
    if meta.len() as usize > parse::MAX_BYTES {
        return Err(t!("Die Datei ist zu groß (höchstens 256 KB).").to_string());
    }
    let mut buf = Vec::new();
    std::fs::File::open(&expanded)
        .and_then(|f| {
            f.take(parse::MAX_BYTES as u64 + 1)
                .read_to_end(&mut buf)
                .map(|_| ())
        })
        .map_err(|e| e.to_string())?;
    parse::parse_any(&String::from_utf8_lossy(&buf), &expanded.to_string_lossy())
}

fn set_draft(st: &mut State, r: Result<Vec<Scheme>, String>) {
    match r {
        Ok(v) => {
            st.name = v[0].name.clone();
            st.draft = v;
            st.picked = 0;
            st.error = None;
        }
        Err(e) => {
            st.draft.clear();
            st.error = Some(e);
        }
    }
    st.message = None;
}

fn swatches(ui: &mut egui::Ui, p: &theme::Palette) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), egui::Sense::hover());
    let w = (rect.width() / 16.0).min(28.0);
    for i in 0..16 {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.min + Vec2::new(i as f32 * w, 0.0),
                Vec2::new(w - 2.0, 16.0),
            ),
            CornerRadius::same(2),
            p.ansi[i],
        );
    }
}

fn ui_strip(ui: &mut egui::Ui, p: &theme::Palette) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), egui::Sense::hover());
    let cols = [
        p.bg_deep, p.bg_panel, p.bg_card, p.bg_hover, p.border, p.text_dim, p.text, p.accent,
        p.accent2, p.red, p.amber, p.green,
    ];
    let w = (rect.width() / cols.len() as f32).min(40.0);
    for (i, c) in cols.iter().enumerate() {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.min + Vec2::new(i as f32 * w, 0.0),
                Vec2::new(w - 2.0, 22.0),
            ),
            CornerRadius::same(3),
            *c,
        );
    }
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, text);
    });
    ui.add_space(2.0);
}

/// The "Eigene" picker section and the import card.
pub fn section(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    // A file dropped on the settings page fills the path field.
    let dropped: Option<String> = ui.ctx().input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_string_lossy().into_owned())
            .find(|p| !p.is_empty())
    });
    if let Some(p) = dropped {
        let r = load_file(&p);
        app.themes_ui.path = p;
        set_draft(&mut app.themes_ui, r);
    }

    let w = ui.available_width();
    let cols = ((w + 12.0) / (170.0 + 12.0)).floor().max(1.0) as usize;
    let card_w = (w - 12.0 * (cols as f32 - 1.0)) / cols as f32;
    let customs: Vec<Scheme> = app.lib.settings.custom_themes.clone();
    if !customs.is_empty() {
        caption(ui, t!("Design · Eigene"));
        let pals: Vec<theme::Palette> = customs.iter().map(palette_of).collect();
        for row in pals.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for p in row {
                    if super::settings::theme_card(ui, p, card_w, app.lib.settings.theme == p.id)
                        .clicked()
                    {
                        app.lib.settings.theme = p.id.to_string();
                        theme::apply(ui.ctx(), p.id);
                        *changed = true;
                    }
                }
            });
            ui.add_space(12.0);
        }
        ui.add_space(12.0);
    }

    caption(ui, t!("Eigene Themes"));
    let family = theme::term_font(ui.ctx(), &app.lib.settings.term_font);
    let size = app.lib.settings.font_size;
    let mut save_all = false;
    let mut save_one = false;
    theme::settings_card(ui, |ui, rows| {
        rows.full(ui, |ui| {
            ui.label(
                RichText::new(t!(
                    "Farbschemas aus iTerm2 (.itermcolors), Windows Terminal (JSON), base16 (YAML/JSON), Alacritty (TOML) und Kitty/Ghostty importieren. Datei hierher ziehen, Pfad eingeben oder den Text einfügen."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let btn_w = theme::btn_width(ui, theme::Btn::Small, None, t!("Datei laden"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.themes_ui.path)
                        .hint_text("~/Downloads/Dracula.itermcolors")
                        .desired_width((ui.available_width() - btn_w - 14.0).max(120.0)),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (theme::btn(ui, theme::Btn::Small, None, t!("Datei laden")).clicked() || enter)
                    && !app.themes_ui.path.trim().is_empty()
                {
                    let res = load_file(&app.themes_ui.path);
                    set_draft(&mut app.themes_ui, res);
                }
            });
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::multiline(&mut app.themes_ui.paste)
                    .hint_text(t!("…oder das Farbschema hier einfügen"))
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            if theme::btn(ui, theme::Btn::Small, None, t!("Text einlesen")).clicked() {
                let res = parse::parse_any(&app.themes_ui.paste, "");
                set_draft(&mut app.themes_ui, res);
            }
            if let Some(e) = &app.themes_ui.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).size(12.5).color(theme::red()));
            }
            if let Some(m) = &app.themes_ui.message {
                ui.add_space(6.0);
                ui.label(RichText::new(m).size(12.5).color(theme::green()));
            }
        });
        if !app.themes_ui.draft.is_empty() {
            rows.full(ui, |ui| {
                let st = &mut app.themes_ui;
                if st.draft.len() > 1 {
                    ui.label(
                        RichText::new(t!("{} Farbschemas in der Datei", st.draft.len()))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                    egui::ComboBox::from_id_salt("theme-import-pick")
                        .selected_text(st.draft[st.picked].name.clone())
                        .show_ui(ui, |ui| {
                            for (i, s) in st.draft.iter().enumerate() {
                                if ui.selectable_value(&mut st.picked, i, &s.name).clicked() {
                                    st.name = s.name.clone();
                                }
                            }
                        });
                    ui.add_space(6.0);
                }
                let mut scheme = st.draft[st.picked].clone();
                scheme.id = "preview".into();
                let pal = palette_of(&scheme);
                egui::Frame::NONE
                    .corner_radius(CornerRadius::same(8))
                    .show(ui, |ui| {
                        super::settings::terminal_preview(ui, &pal, family.clone(), size);
                    });
                ui.add_space(6.0);
                swatches(ui, &pal);
                ui.add_space(4.0);
                ui_strip(ui, &pal);
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(t!("Name"));
                    ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(200.0));
                    if theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Speichern")).clicked() {
                        save_one = true;
                    }
                    if st.draft.len() > 1
                        && theme::btn(
                            ui,
                            theme::Btn::Small,
                            None,
                            &t!("Alle {} speichern", st.draft.len()),
                        )
                        .clicked()
                    {
                        save_all = true;
                    }
                    if theme::btn(ui, theme::Btn::Small, None, t!("Verwerfen")).clicked() {
                        st.draft.clear();
                    }
                });
            });
        }
    });

    if save_one || save_all {
        let picked = app.themes_ui.picked;
        let list: Vec<(usize, Scheme)> = app
            .themes_ui
            .draft
            .iter()
            .cloned()
            .enumerate()
            .filter(|(i, _)| save_all || *i == picked)
            .collect();
        let mut last = None;
        for (i, mut s) in list {
            let wanted = if !save_all && i == picked {
                app.themes_ui.name.clone()
            } else {
                s.name.clone()
            };
            s.name = unique_name(app, &wanted);
            s.id = new_id();
            last = Some(s.id.clone());
            app.lib.settings.custom_themes.push(s);
        }
        register(&app.lib.settings.custom_themes);
        if let Some(id) = last {
            app.lib.settings.theme = id.clone();
            theme::apply(ui.ctx(), &id);
        }
        app.themes_ui.draft.clear();
        app.themes_ui.paste.clear();
        app.themes_ui.path.clear();
        app.themes_ui.message = Some(t!("Theme gespeichert und aktiviert.").to_string());
        *changed = true;
    }

    // The list of imported themes.
    if !customs.is_empty() {
        ui.add_space(14.0);
        caption(ui, t!("Importierte Themes"));
        let mut action: Option<(String, &'static str)> = None;
        theme::settings_card(ui, |ui, rows| {
            for s in &customs {
                let pal = palette_of(s);
                rows.full(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::new(54.0, 18.0), egui::Sense::hover());
                        for (i, c) in [pal.term_bg, pal.ansi[1], pal.ansi[2], pal.ansi[4]]
                            .into_iter()
                            .enumerate()
                        {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    r.min + Vec2::new(i as f32 * 13.0, 0.0),
                                    Vec2::new(12.0, 18.0),
                                ),
                                CornerRadius::same(2),
                                c,
                            );
                        }
                        let renaming = app
                            .themes_ui
                            .renaming
                            .as_mut()
                            .filter(|(id, _)| *id == s.id);
                        if let Some((_, name)) = renaming {
                            let r = ui.add(egui::TextEdit::singleline(name).desired_width(170.0));
                            let enter =
                                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if theme::btn(ui, theme::Btn::Small, None, t!("OK")).clicked() || enter
                            {
                                action = Some((s.id.clone(), "rename"));
                            }
                        } else {
                            ui.label(RichText::new(&s.name).font(theme::semibold(13.5)));
                            if theme::btn(ui, theme::Btn::Link, None, t!("Umbenennen")).clicked() {
                                app.themes_ui.renaming = Some((s.id.clone(), s.name.clone()));
                            }
                        }
                        if theme::btn(ui, theme::Btn::Link, None, t!("Exportieren")).clicked() {
                            action = Some((s.id.clone(), "export"));
                        }
                        if theme::btn(ui, theme::Btn::Link, None, t!("JSON kopieren")).clicked() {
                            action = Some((s.id.clone(), "copy"));
                        }
                        let confirm =
                            app.themes_ui.confirm_delete.as_deref() == Some(s.id.as_str());
                        let label = if confirm {
                            t!("Wirklich löschen?")
                        } else {
                            t!("Löschen")
                        };
                        if theme::btn(ui, theme::Btn::Link, None, label).clicked() {
                            if confirm {
                                action = Some((s.id.clone(), "delete"));
                            } else {
                                app.themes_ui.confirm_delete = Some(s.id.clone());
                            }
                        }
                    });
                });
            }
        });
        if let Some((id, what)) = action {
            apply_action(app, ui.ctx(), &id, what, changed);
        }
    }
}

fn apply_action(app: &mut App, ctx: &egui::Context, id: &str, what: &str, changed: &mut bool) {
    let Some(pos) = app
        .lib
        .settings
        .custom_themes
        .iter()
        .position(|s| s.id == id)
    else {
        return;
    };
    match what {
        "rename" => {
            if let Some((_, name)) = app.themes_ui.renaming.take()
                && !name.trim().is_empty()
            {
                let others: Vec<String> = app
                    .lib
                    .settings
                    .custom_themes
                    .iter()
                    .filter(|s| s.id != id)
                    .map(|s| s.name.clone())
                    .collect();
                let mut n = name.trim().to_string();
                let mut i = 2;
                while others.contains(&n) || theme::THEMES.iter().any(|t| t.name == n) {
                    n = format!("{} ({i})", name.trim());
                    i += 1;
                }
                app.lib.settings.custom_themes[pos].name = n;
                register(&app.lib.settings.custom_themes);
                *changed = true;
            }
        }
        "delete" => {
            app.lib.settings.custom_themes.remove(pos);
            app.themes_ui.confirm_delete = None;
            for h in &mut app.lib.hosts {
                if h.term.theme == id {
                    h.term.theme.clear();
                }
            }
            register(&app.lib.settings.custom_themes);
            if app.lib.settings.theme == id {
                app.lib.settings.theme = theme::THEMES[0].id.to_string();
                theme::apply(ctx, theme::THEMES[0].id);
            }
            *changed = true;
        }
        "export" | "copy" => {
            let s = &app.lib.settings.custom_themes[pos];
            let json = parse::to_windows_terminal(s);
            if what == "copy" {
                let ok = arboard::Clipboard::new()
                    .and_then(|mut c| c.set_text(json))
                    .is_ok();
                app.themes_ui.message = Some(if ok {
                    t!("JSON in die Zwischenablage kopiert.").to_string()
                } else {
                    t!("Zwischenablage nicht verfügbar.").to_string()
                });
            } else {
                let dir = directories::UserDirs::new()
                    .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
                    .unwrap_or_else(std::env::temp_dir);
                let safe: String = s
                    .name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let path = dir.join(format!("{safe}.json"));
                app.themes_ui.message = Some(match std::fs::write(&path, json) {
                    Ok(()) => t!("Exportiert nach {}", path.display()).to_string(),
                    Err(e) => format!("{}: {e}", path.display()),
                });
            }
        }
        _ => {}
    }
}
