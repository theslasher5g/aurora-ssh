//! "Container" browser (Docker/Podman containers and Kubernetes pods on this PC or an
//! SSH host) and the "Log ansehen" dialog.

use egui::{RichText, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::backend::container::Item;
use crate::model::{ContainerSettings, Host, Protocol};
use crate::theme;

pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    browser(app, ctx, actions);
    log_picker(app, ctx, actions);
}

fn host_for(app: &App, item: &Item, container: Option<&String>) -> Option<Host> {
    let b = app.containers.as_ref()?;
    let group_id = b.via.and_then(|v| app.lib.host(v)).and_then(|h| h.group_id);
    Some(Host {
        protocol: b.proto,
        group_id,
        port: 0,
        container: ContainerSettings {
            via: b.via,
            podman: b.podman,
            name: item.name.clone(),
            namespace: item.namespace.clone(),
            container: container.cloned().unwrap_or_default(),
            context: b.context.clone(),
            ..Default::default()
        },
        ..Default::default()
    })
}

fn browser(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    if app.containers.is_none() {
        return;
    }
    let ssh_hosts: Vec<(Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh)
        .map(|h| (h.id, h.display_name().to_string()))
        .collect();
    let saved: Vec<(Option<Uuid>, String, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol.is_container())
        .map(|h| {
            (
                h.container.via,
                h.container.name.clone(),
                h.container.namespace.clone(),
            )
        })
        .collect();
    let mut refresh = false;
    let mut close = false;
    // (item, container in the pod, shell = true / logs = false / save = None)
    let mut picked: Option<(Item, Option<String>, Option<bool>)> = None;
    let Some(b) = app.containers.as_mut() else {
        return;
    };
    egui::Modal::new(egui::Id::new("containers")).show(ctx, |ui| {
        ui.set_width(640.0);
        ui.horizontal(|ui| {
            ui.label(theme::heading(t!("Container"), 20.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text(t!("Schließen"))
                    .clicked()
                {
                    close = true;
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for p in [Protocol::Docker, Protocol::Kubernetes] {
                if ui
                    .selectable_label(b.proto == p, format!("{} {}", p.icon(), p.label()))
                    .clicked()
                    && b.proto != p
                {
                    b.proto = p;
                    b.items.clear();
                    refresh = true;
                }
            }
            ui.separator();
            ui.label(t!("Läuft auf"));
            let sel = b
                .via
                .and_then(|v| ssh_hosts.iter().find(|(i, _)| *i == v))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("Dieser PC").to_string());
            let before = b.via;
            egui::ComboBox::from_id_salt("ct-via")
                .selected_text(sel)
                .width(170.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut b.via, None, t!("Dieser PC"));
                    for (id, n) in &ssh_hosts {
                        ui.selectable_value(&mut b.via, Some(*id), n);
                    }
                });
            if b.via != before {
                b.items.clear();
                refresh = true;
            }
        });
        ui.horizontal(|ui| {
            if b.proto == Protocol::Docker {
                if ui.checkbox(&mut b.podman, "Podman").changed() {
                    refresh = true;
                }
            } else {
                ui.label(t!("Kontext"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut b.context)
                        .hint_text(t!("aktueller"))
                        .desired_width(140.0),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    refresh = true;
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut b.filter)
                    .hint_text(t!("Filtern"))
                    .desired_width(200.0),
            );
            if ui
                .add_enabled(!b.loading, egui::Button::new(t!("⟳ Aktualisieren")))
                .clicked()
            {
                refresh = true;
            }
            if b.loading {
                ui.spinner();
            }
        });
        ui.add_space(6.0);
        if let Some(e) = &b.error {
            ui.label(RichText::new(e).color(theme::red()));
            ui.add_space(4.0);
        }
        let f = b.filter.to_lowercase();
        let items: Vec<&Item> = b
            .items
            .iter()
            .filter(|i| {
                f.is_empty()
                    || i.name.to_lowercase().contains(&f)
                    || i.detail.to_lowercase().contains(&f)
                    || i.namespace.to_lowercase().contains(&f)
            })
            .collect();
        if items.is_empty() && !b.loading && b.error.is_none() {
            ui.label(
                RichText::new(if b.proto == Protocol::Kubernetes {
                    t!("Keine Pods gefunden")
                } else {
                    t!("Keine Container gefunden")
                })
                .color(theme::text_dim()),
            );
        }
        // Fixed height for the rows we have: the modal keeps its size from the last
        // frame, so a growing list would otherwise stay squeezed.
        let height = (items.len() as f32 * 62.0).clamp(0.0, 380.0);
        egui::ScrollArea::vertical()
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for item in items {
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            dot.center(),
                            4.0,
                            if item.running {
                                theme::green()
                            } else {
                                theme::text_dim()
                            },
                        );
                        ui.vertical(|ui| {
                            let name = if item.namespace.is_empty() {
                                item.name.clone()
                            } else {
                                format!("{}/{}", item.namespace, item.name)
                            };
                            ui.label(theme::mono(name, 13.0).color(theme::text()));
                            ui.label(
                                RichText::new(format!("{} · {}", item.detail, item.status))
                                    .size(11.5)
                                    .color(theme::text_dim()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let multi = item.containers.len() > 1;
                            let is_saved = saved.iter().any(|(v, n, ns)| {
                                *v == b.via && *n == item.name && *ns == item.namespace
                            });
                            if b.for_editor {
                                if ui.button(t!("Übernehmen")).clicked() {
                                    picked = Some((item.clone(), None, None));
                                }
                                return;
                            }
                            if ui
                                .add_enabled(!is_saved, egui::Button::new(t!("Speichern")))
                                .on_hover_text(if is_saved {
                                    t!("Schon in der Hostliste")
                                } else {
                                    t!("Als Eintrag in der Hostliste speichern")
                                })
                                .clicked()
                            {
                                picked = Some((item.clone(), None, None));
                            }
                            for (shell, label) in [(false, t!("Logs")), (true, t!("Shell"))] {
                                let enabled = !shell || item.running;
                                if multi {
                                    ui.add_enabled_ui(enabled, |ui| {
                                        ui.menu_button(label, |ui| {
                                            for c in &item.containers {
                                                if ui.button(c).clicked() {
                                                    picked = Some((
                                                        item.clone(),
                                                        Some(c.clone()),
                                                        Some(shell),
                                                    ));
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                } else if ui
                                    .add_enabled(enabled, egui::Button::new(label))
                                    .clicked()
                                {
                                    picked = Some((item.clone(), None, Some(shell)));
                                }
                            }
                        });
                    });
                    ui.separator();
                }
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
    });
    let for_editor = b.for_editor;
    if let Some((item, container, what)) = picked {
        let Some(host) = host_for(app, &item, container.as_ref()) else {
            return;
        };
        match what {
            _ if for_editor => {
                if let Some(ed) = app.host_editor.as_mut() {
                    ed.host.protocol = host.protocol;
                    ed.host.container = host.container;
                }
                close = true;
            }
            Some(shell) => {
                actions.push(Action::OpenContainer(Box::new(host), !shell));
                close = true;
            }
            None => {
                app.toast(t!("„{}“ gespeichert", host.display_name()), theme::green());
                app.lib.hosts.push(host);
                app.save_library();
            }
        }
    }
    if close {
        app.containers = None;
    } else if refresh {
        app.refresh_containers(ctx);
    }
}

fn log_picker(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some(host) = app.log_picker.as_ref().map(|p| p.host) else {
        return;
    };
    let name = host
        .and_then(|h| app.lib.host(h))
        .map(|h| h.display_name().to_string())
        .unwrap_or_else(|| t!("Dieser PC").to_string());
    let Some(p) = app.log_picker.as_mut() else {
        return;
    };
    let (mut open, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("log-picker")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(theme::heading(t!("Log ansehen"), 20.0));
        ui.label(theme::mono(&name, 13.0).color(theme::text_dim()));
        ui.add_space(8.0);
        ui.label(
            RichText::new(t!(
                "Datei (wird mit tail -F verfolgt) oder ein Befehl, der laufend ausgibt"
            ))
            .size(12.5)
            .color(theme::text_dim()),
        );
        let r = ui.add(
            egui::TextEdit::singleline(&mut p.source)
                .hint_text("/var/log/syslog · journalctl -f")
                .font(egui::FontId::monospace(13.5))
                .desired_width(f32::INFINITY),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            open = true;
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for preset in [
                "journalctl -f",
                "journalctl -f -p warning",
                "/var/log/syslog",
                "/var/log/messages",
                "/var/log/auth.log",
                "/var/log/nginx/error.log",
                "/var/log/nginx/access.log",
                "/var/log/apache2/error.log",
            ] {
                if ui
                    .selectable_label(p.source == preset, theme::mono(preset, 12.0))
                    .clicked()
                {
                    p.source = preset.to_string();
                }
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!p.source.trim().is_empty(), egui::Button::new(t!("Öffnen")))
                    .clicked()
                {
                    open = true;
                }
                if ui.button(t!("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
    });
    if cancel {
        app.log_picker = None;
    } else if open && !p.source.trim().is_empty() {
        actions.push(Action::OpenLog(host, p.source.trim().to_string()));
    }
}
