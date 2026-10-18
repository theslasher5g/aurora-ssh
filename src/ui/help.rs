//! Help page: all keyboard shortcuts and mouse gestures, grouped, with a filter.

use egui::RichText;

use crate::app::App;
use crate::theme;

/// A key cap or a mouse gesture.
enum K {
    Key(&'static str),
    Gesture(&'static str),
    Or,
    Slash,
}

use K::{Gesture as G, Key, Or, Slash};

/// (what it does, keys) pairs of one group.
type Entries = &'static [(&'static str, &'static [K])];

const GROUPS: &[(&str, Entries)] = &[
    (
        "Allgemein",
        &[
            (
                "Suchen, Befehle, mit user@host verbinden",
                &[Key("Ctrl"), Key("K")],
            ),
            ("Startseite", &[Key("Ctrl"), Key("0")]),
            ("Vollbild an/aus", &[Key("F11")]),
            ("Diese Übersicht", &[Key("F1"), Or, Key("?")]),
        ],
    ),
    (
        "Sitzungen",
        &[
            ("Zu Sitzung 1 … 9 wechseln", &[Key("Ctrl"), Key("1 … 9")]),
            ("Nächste Sitzung", &[Key("Ctrl"), Key("Tab")]),
            ("Sitzung schließen", &[Key("Ctrl"), Key("Shift"), Key("W")]),
            (
                "Sitzung per Mittelklick auf den Port schließen",
                &[G("Mittelklick auf Port")],
            ),
            ("Getrennte Sitzung neu verbinden", &[Key("Enter")]),
        ],
    ),
    (
        "Terminal",
        &[
            ("Lokales Terminal", &[Key("Ctrl"), Key("Shift"), Key("T")]),
            ("Im Terminal suchen", &[Key("Ctrl"), Key("Shift"), Key("F")]),
            (
                "Logs neben dem Terminal",
                &[Key("Ctrl"), Key("Shift"), Key("L")],
            ),
            ("Schriftgröße ändern", &[Key("Ctrl"), G("Mausrad")]),
            (
                "Kopieren / Einfügen",
                &[Key("Ctrl"), Key("Shift"), Key("C"), Slash, Key("V")],
            ),
        ],
    ),
    (
        "Geteilte Ansicht",
        &[
            ("Fenster maximieren", &[G("Doppelklick auf Kopfzeile")]),
            ("Sitzung neben eine andere legen", &[G("Port ziehen")]),
            ("Dateien hochladen", &[G("Auf das Terminal ziehen")]),
        ],
    ),
];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    theme::page_header(
        ui,
        "help",
        |ui| theme::page_title(ui, t!("Tastenkürzel"), None),
        |_| {},
        |ui, bar| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // The hint is the first to go when room runs out.
            if !bar.compact {
                theme::kbd(ui, "?");
                ui.label(
                    RichText::new(t!("oder"))
                        .size(13.0)
                        .color(theme::text_dim()),
                );
                theme::kbd(ui, "F1");
                ui.label(
                    RichText::new(t!("Öffnen mit"))
                        .size(13.0)
                        .color(theme::text_dim()),
                );
                ui.add_space(10.0);
            }
            let width = bar.flex(160.0, 240.0);
            theme::search_field(
                ui,
                &mut app.help_filter,
                t!("Kürzel suchen"),
                width,
                egui::Id::new("help-search"),
            );
        },
    );
    ui.add_space(22.0);
    let filter = app.help_filter.trim().to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("help-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the scroll bar next to the cards.
            ui.set_max_width(ui.available_width() - 14.0);
            let w = ui.available_width();
            let two = w > 760.0;
            let col_w = if two { (w - 20.0) / 2.0 } else { w };
            let groups: Vec<_> = GROUPS
                .iter()
                .map(|(name, items)| {
                    let items: Vec<_> = items
                        .iter()
                        .filter(|(what, _)| {
                            filter.is_empty()
                                || crate::i18n::tr(what).to_lowercase().contains(&filter)
                                || crate::i18n::tr(name).to_lowercase().contains(&filter)
                        })
                        .collect();
                    (*name, items)
                })
                .filter(|(_, items)| !items.is_empty())
                .collect();
            if groups.is_empty() {
                ui.label(RichText::new(t!("Kein Kürzel gefunden")).color(theme::text_dim()));
            }
            for pair in groups.chunks(if two { 2 } else { 1 }) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 20.0;
                    for (name, items) in pair {
                        ui.vertical(|ui| {
                            ui.set_width(col_w);
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                theme::section_title(ui, crate::i18n::tr(name));
                            });
                            ui.add_space(2.0);
                            theme::settings_card(ui, |ui, rows| {
                                for (what, keys) in items {
                                    rows.setting(ui, crate::i18n::tr(what), None, |ui| {
                                        ui.spacing_mut().item_spacing.x = 5.0;
                                        for k in keys.iter().rev() {
                                            match k {
                                                Key(k) => {
                                                    theme::kbd(ui, k);
                                                }
                                                G(g) => {
                                                    theme::gesture(ui, crate::i18n::tr(g));
                                                }
                                                Or => {
                                                    ui.label(
                                                        RichText::new(t!("oder"))
                                                            .size(12.5)
                                                            .color(theme::text_dim()),
                                                    );
                                                }
                                                Slash => {
                                                    ui.label(
                                                        RichText::new("/")
                                                            .size(12.5)
                                                            .color(theme::text_dim()),
                                                    );
                                                }
                                            }
                                        }
                                    });
                                }
                            });
                        });
                    }
                });
                ui.add_space(26.0);
            }
        });
}
