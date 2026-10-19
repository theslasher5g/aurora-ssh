//! Sheets of the container page: `docker inspect` key facts and `kubectl describe` text.

use egui::RichText;

use crate::app::App;
use crate::backend::containers_parse::Inspect;
use crate::theme;

pub enum Sheet {
    Loading(String),
    Inspect {
        title: String,
        info: Box<Inspect>,
        /// Show the values of the environment variables (they often hold secrets).
        reveal: bool,
    },
    Text {
        title: String,
        text: String,
    },
    Error {
        title: String,
        msg: String,
    },
}

fn fact(ui: &mut egui::Ui, label: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(label).size(12.5).color(theme::text_dim()));
        ui.label(theme::mono(value, 12.5).color(theme::text()));
    });
}

fn list(ui: &mut egui::Ui, caption: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    theme::form_caption(ui, caption);
    for i in items {
        ui.add(egui::Label::new(theme::mono(i, 12.5).color(theme::text())).wrap());
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(sheet) = app.ctr.sheet.as_mut() else {
        return;
    };
    let title = match sheet {
        Sheet::Loading(t) | Sheet::Inspect { title: t, .. } | Sheet::Text { title: t, .. } => {
            t.clone()
        }
        Sheet::Error { title, .. } => title.clone(),
    };
    let (closed_btn, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("ctr-sheet"),
        640.0,
        &title,
        |ui| match sheet {
            Sheet::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t!("Lädt…"));
                });
                None
            }
            Sheet::Error { msg, .. } => {
                ui.label(RichText::new(msg.as_str()).color(theme::red()));
                None
            }
            Sheet::Text { text, .. } => {
                ui.add(
                    egui::Label::new(theme::mono(text.as_str(), 12.0).color(theme::text()))
                        .selectable(true),
                );
                Some(text.clone())
            }
            Sheet::Inspect { info, reveal, .. } => {
                fact(ui, t!("Image"), &info.image);
                fact(ui, t!("Status"), &info.state);
                fact(ui, t!("Health"), &info.health);
                fact(ui, t!("Neustart-Richtlinie"), &info.restart_policy);
                fact(ui, t!("Befehl"), &info.command);
                fact(ui, t!("Erstellt"), &info.created);
                list(ui, t!("Mounts"), &info.mounts);
                list(ui, t!("Netzwerke"), &info.networks);
                list(ui, t!("Ports"), &info.ports);
                theme::form_caption(ui, t!("Umgebungsvariablen"));
                if info.env.is_empty() {
                    ui.label(RichText::new(t!("Keine")).color(theme::text_dim()));
                } else {
                    ui.horizontal(|ui| {
                        ui.checkbox(reveal, t!("Werte anzeigen"));
                        ui.label(
                            RichText::new(t!("Werte können Passwörter enthalten"))
                                .size(12.0)
                                .color(theme::text_dim()),
                        );
                    });
                    for (k, v) in &info.env {
                        let line = if *reveal {
                            format!("{k}={v}")
                        } else {
                            k.clone()
                        };
                        ui.add(
                            egui::Label::new(theme::mono(line, 12.5).color(theme::text())).wrap(),
                        );
                    }
                }
                None
            }
        },
        |ui, copy| {
            let closed = theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked();
            if let Some(t) = copy
                && theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(crate::ui::icons::Icon::Copy),
                    t!("Kopieren"),
                )
                .clicked()
            {
                ui.ctx().copy_text(t);
            }
            closed
        },
    );
    if closed_btn || dismissed {
        app.ctr.sheet = None;
    }
}
