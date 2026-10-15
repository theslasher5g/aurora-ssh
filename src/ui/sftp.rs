//! Dual-pane SFTP browser (local ⇄ remote) with transfers and drag & drop upload.

use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Color32, CornerRadius, Margin, RichText, Vec2};

use crate::app::{App, ConnState, FsDialog, LocalPane, SftpTab, TabKind, Transfer};
use crate::backend::sftp::{Entry, SftpCmd, SftpEvent, join, parent};
use crate::theme;
use crate::ui::human_size;

pub fn refresh_local(pane: &mut LocalPane) {
    pane.edit = pane.path.display().to_string();
    pane.selected = None;
    match std::fs::read_dir(&pane.path) {
        Ok(rd) => {
            let mut entries: Vec<Entry> = rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let meta = e.metadata().ok();
                    let ft = e.file_type().ok();
                    let is_link = ft.is_some_and(|f| f.is_symlink());
                    Entry {
                        name: e.file_name().to_string_lossy().to_string(),
                        is_dir: e.path().is_dir(),
                        is_link,
                        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                        modified: meta.as_ref().and_then(|m| m.modified().ok()),
                    }
                })
                .collect();
            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            pane.entries = entries;
            pane.error = None;
        }
        Err(e) => {
            pane.entries.clear();
            pane.error = Some(e.to_string());
        }
    }
}

pub fn handle_event(t: &mut SftpTab, ev: SftpEvent, toasts: &mut Vec<(String, Color32)>) {
    match ev {
        SftpEvent::Status(s) => t.state = ConnState::Connecting(s),
        SftpEvent::Connected { home } => {
            t.state = ConnState::Connected;
            let path = if t.remote_path.is_empty() {
                home
            } else {
                t.remote_path.clone()
            };
            let _ = t.tx.send(SftpCmd::List(path));
        }
        SftpEvent::Listing { path, entries } => {
            if t.remote_path != path {
                t.remote_selected = None;
            }
            t.remote_edit = path.clone();
            t.remote_path = path;
            t.remote_entries = entries;
        }
        SftpEvent::Progress {
            id,
            name,
            done,
            total,
        } => {
            if let Some(tr) = t.transfers.iter_mut().find(|x| x.id == id) {
                tr.name = name;
                tr.done = done;
                tr.total = total;
            }
        }
        SftpEvent::TransferDone { id, error } => {
            let mut refresh_local_pane = false;
            if let Some(tr) = t.transfers.iter_mut().find(|x| x.id == id) {
                tr.finished = true;
                if let Some(e) = &error {
                    toasts.push((format!("Übertragung fehlgeschlagen: {e}"), theme::red()));
                } else {
                    toasts.push((format!("✔ {} übertragen", tr.name), theme::green()));
                    refresh_local_pane = !tr.upload;
                }
                tr.error = error;
            }
            if refresh_local_pane {
                refresh_local(&mut t.local);
            }
        }
        SftpEvent::Changed => {
            let _ = t.tx.send(SftpCmd::List(t.remote_path.clone()));
        }
        SftpEvent::Error(e) => toasts.push((e, theme::red())),
        SftpEvent::Closed(err) => {
            if let Some(e) = &err {
                toasts.push((format!("SFTP: {e}"), theme::red()));
            }
            t.state = ConnState::Closed(err);
        }
    }
}

enum PaneAction {
    Open(String),
    Up,
    Refresh,
    Navigate(String),
    Transfer(String),
    Mkdir,
    Rename(String),
    Delete(String, bool),
}

pub fn show(app: &mut App, ui: &mut egui::Ui, tab_id: u64) {
    let accent = app.accent();
    let mut next_id = app.next_id;
    let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab_id) else {
        return;
    };
    let TabKind::Sftp(t) = &mut tab.kind else {
        return;
    };
    let mut reconnect = false;

    // Drag & drop from the OS uploads into the current remote directory.
    let dropped: Vec<PathBuf> = ui.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .collect()
    });
    if matches!(t.state, ConnState::Connected) {
        for path in dropped {
            next_id += 1;
            start_upload(t, path, next_id);
        }
    }

    egui::Panel::bottom(egui::Id::new(("sftp-transfers", tab_id)))
        .resizable(true)
        .default_size(130.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(Margin::same(12)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::section_title(ui, "Übertragungen");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Fertige entfernen").clicked() {
                        t.transfers.retain(|x| !x.finished);
                    }
                });
            });
            egui::ScrollArea::vertical().show(ui, |ui| {
                if t.transfers.is_empty() {
                    ui.label(
                        RichText::new("Dateien hierher ziehen oder ⬆/⬇ verwenden.")
                            .color(theme::text_dim()),
                    );
                }
                for tr in t.transfers.iter().rev() {
                    ui.horizontal(|ui| {
                        ui.label(if tr.upload { "⬆" } else { "⬇" });
                        ui.allocate_ui_with_layout(
                            Vec2::new(220.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.add(egui::Label::new(&tr.name).truncate()),
                        );
                        let frac = if tr.total > 0 {
                            tr.done as f32 / tr.total as f32
                        } else if tr.finished {
                            1.0
                        } else {
                            0.0
                        };
                        let (color, text) = match (&tr.error, tr.finished) {
                            (Some(e), _) => (theme::red(), e.clone()),
                            (None, true) => (theme::green(), "fertig".into()),
                            (None, false) => (
                                accent,
                                format!("{} / {}", human_size(tr.done), human_size(tr.total)),
                            ),
                        };
                        ui.add(
                            egui::ProgressBar::new(frac)
                                .fill(color)
                                .desired_width(220.0)
                                .desired_height(8.0),
                        );
                        ui.label(RichText::new(text).size(11.5).color(theme::text_dim()));
                    });
                }
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_deep())
                .inner_margin(Margin::same(12)),
        )
        .show(ui, |ui| {
            if let ConnState::Connecting(s) | ConnState::Closed(Some(s)) = &t.state {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⏺").color(crate::app::state_color(&t.state)));
                    ui.label(s.as_str());
                    if matches!(t.state, ConnState::Closed(_))
                        && ui.button("⟳ Neu verbinden").clicked()
                    {
                        reconnect = true;
                    }
                });
            } else if matches!(t.state, ConnState::Closed(None))
                && ui.button("⟳ Neu verbinden").clicked()
            {
                reconnect = true;
            }

            let remote_title = format!("🌐 {}", t.params.host_id());
            let mut local_act = None;
            let mut remote_act = None;
            ui.columns(2, |cols| {
                local_act = pane(
                    &mut cols[0],
                    "local",
                    "💻 Lokal",
                    &mut t.local.edit,
                    &t.local.entries,
                    t.local.selected.as_deref(),
                    t.local.error.as_deref(),
                    "⬆ Hochladen",
                    accent,
                );
                remote_act = pane(
                    &mut cols[1],
                    "remote",
                    &remote_title,
                    &mut t.remote_edit,
                    &t.remote_entries,
                    t.remote_selected.as_deref(),
                    None,
                    "⬇ Herunterladen",
                    accent,
                );
            });

            if let Some(a) = local_act {
                match a {
                    PaneAction::Open(name) => t.local.selected = Some(name),
                    PaneAction::Up => {
                        if let Some(p) = t.local.path.parent() {
                            t.local.path = p.to_path_buf();
                            refresh_local(&mut t.local);
                        }
                    }
                    PaneAction::Refresh => refresh_local(&mut t.local),
                    PaneAction::Navigate(name) => {
                        let p = if name.starts_with('/')
                            || name.contains(":\\")
                            || name.starts_with('\\')
                        {
                            PathBuf::from(name)
                        } else {
                            t.local.path.join(name)
                        };
                        if p.is_dir() {
                            t.local.path = p;
                        }
                        refresh_local(&mut t.local);
                    }
                    PaneAction::Transfer(name) => {
                        next_id += 1;
                        start_upload(t, t.local.path.join(name), next_id);
                    }
                    PaneAction::Mkdir => {
                        t.dialog = Some(FsDialog::Mkdir {
                            remote: false,
                            name: String::new(),
                        })
                    }
                    PaneAction::Rename(name) => {
                        t.dialog = Some(FsDialog::Rename {
                            remote: false,
                            to: name.clone(),
                            from: name,
                        })
                    }
                    PaneAction::Delete(name, is_dir) => {
                        t.dialog = Some(FsDialog::Delete {
                            remote: false,
                            name,
                            is_dir,
                        })
                    }
                }
            }
            if let Some(a) = remote_act {
                match a {
                    PaneAction::Open(name) => t.remote_selected = Some(name),
                    PaneAction::Up => {
                        let _ = t.tx.send(SftpCmd::List(parent(&t.remote_path)));
                    }
                    PaneAction::Refresh => {
                        let _ = t.tx.send(SftpCmd::List(t.remote_path.clone()));
                    }
                    PaneAction::Navigate(name) => {
                        let p = if name.starts_with('/') {
                            name
                        } else {
                            join(&t.remote_path, &name)
                        };
                        let _ = t.tx.send(SftpCmd::List(p));
                    }
                    PaneAction::Transfer(name) => {
                        next_id += 1;
                        t.transfers.push(Transfer {
                            id: next_id,
                            name: name.clone(),
                            upload: false,
                            done: 0,
                            total: 0,
                            finished: false,
                            error: None,
                        });
                        let _ = t.tx.send(SftpCmd::Download {
                            remote: join(&t.remote_path, &name),
                            local_dir: t.local.path.clone(),
                            id: next_id,
                        });
                    }
                    PaneAction::Mkdir => {
                        t.dialog = Some(FsDialog::Mkdir {
                            remote: true,
                            name: String::new(),
                        })
                    }
                    PaneAction::Rename(name) => {
                        t.dialog = Some(FsDialog::Rename {
                            remote: true,
                            to: name.clone(),
                            from: name,
                        })
                    }
                    PaneAction::Delete(name, is_dir) => {
                        t.dialog = Some(FsDialog::Delete {
                            remote: true,
                            name,
                            is_dir,
                        })
                    }
                }
            }
        });

    fs_dialog(ui.ctx(), t);
    if reconnect {
        let (sink, rx) = crate::backend::Sink::new(ui.ctx().clone());
        t.tx =
            crate::backend::sftp::spawn(&app.rt, t.params.clone(), sink, app.prompts_sink.clone());
        t.rx = rx;
        t.state = ConnState::Connecting("Verbinde…".into());
    }
    app.next_id = next_id;
}

fn start_upload(t: &mut SftpTab, path: PathBuf, id: u64) {
    t.transfers.push(Transfer {
        id,
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        upload: true,
        done: 0,
        total: 0,
        finished: false,
        error: None,
    });
    let _ = t.tx.send(SftpCmd::Upload {
        local: path,
        remote_dir: t.remote_path.clone(),
        id,
    });
}

#[allow(clippy::too_many_arguments)]
fn pane(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    path_edit: &mut String,
    entries: &[Entry],
    selected: Option<&str>,
    error: Option<&str>,
    transfer_label: &str,
    accent: Color32,
) -> Option<PaneAction> {
    let mut action = None;
    theme::card_frame()
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let sel = selected.map(str::to_string);
                    ui.add_enabled_ui(sel.is_some(), |ui| {
                        if ui
                            .button(RichText::new(transfer_label).color(accent))
                            .clicked()
                            && let Some(s) = &sel
                        {
                            action = Some(PaneAction::Transfer(s.clone()));
                        }
                        if ui.button("🗑").on_hover_text("Löschen").clicked()
                            && let Some(s) = &sel
                        {
                            let is_dir = entries.iter().any(|e| &e.name == s && e.is_dir);
                            action = Some(PaneAction::Delete(s.clone(), is_dir));
                        }
                        if ui.button("✏").on_hover_text("Umbenennen").clicked()
                            && let Some(s) = &sel
                        {
                            action = Some(PaneAction::Rename(s.clone()));
                        }
                    });
                    if ui.button("🗀➕").on_hover_text("Neuer Ordner").clicked() {
                        action = Some(PaneAction::Mkdir);
                    }
                });
            });
            ui.horizontal(|ui| {
                if ui
                    .button("⬆")
                    .on_hover_text("Übergeordneter Ordner")
                    .clicked()
                {
                    action = Some(PaneAction::Up);
                }
                if ui.button("⟳").clicked() {
                    action = Some(PaneAction::Refresh);
                }
                let r = ui.add(
                    egui::TextEdit::singleline(path_edit)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    action = Some(PaneAction::Navigate(path_edit.clone()));
                }
            });
            if let Some(e) = error {
                ui.label(RichText::new(e).color(theme::red()).size(12.0));
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt(id)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for e in entries {
                        let is_sel = selected == Some(e.name.as_str());
                        let (rect, resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 26.0),
                            egui::Sense::click(),
                        );
                        if is_sel {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(6),
                                accent.linear_multiply(0.18),
                            );
                        } else if resp.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(6),
                                theme::bg_hover(),
                            );
                        }
                        let icon = if e.is_dir {
                            "🗀"
                        } else if e.is_link {
                            "🔗"
                        } else {
                            "🗋"
                        };
                        let p = ui.painter();
                        let font = egui::FontId::proportional(13.0);
                        p.text(
                            rect.left_center() + Vec2::new(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            icon,
                            font.clone(),
                            theme::text(),
                        );
                        let name_color = if e.is_dir {
                            theme::text()
                        } else {
                            theme::text_dim().linear_multiply(1.3)
                        };
                        let name_rect = egui::Rect::from_min_max(
                            rect.left_top() + Vec2::new(32.0, 0.0),
                            rect.right_bottom() - Vec2::new(170.0, 0.0),
                        );
                        p.with_clip_rect(name_rect).text(
                            name_rect.left_center(),
                            egui::Align2::LEFT_CENTER,
                            &e.name,
                            font.clone(),
                            name_color,
                        );
                        if !e.is_dir {
                            p.text(
                                rect.right_center() - Vec2::new(100.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                human_size(e.size),
                                egui::FontId::proportional(11.5),
                                theme::text_dim(),
                            );
                        }
                        if let Some(m) = e.modified {
                            p.text(
                                rect.right_center() - Vec2::new(8.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                short_date(m),
                                egui::FontId::proportional(11.5),
                                theme::text_dim(),
                            );
                        }
                        if resp.clicked() {
                            action = Some(PaneAction::Open(e.name.clone()));
                        }
                        if resp.double_clicked() {
                            action = Some(if e.is_dir {
                                PaneAction::Navigate(e.name.clone())
                            } else {
                                PaneAction::Transfer(e.name.clone())
                            });
                        }
                        resp.context_menu(|ui| {
                            if e.is_dir && ui.button("🗁 Öffnen").clicked() {
                                action = Some(PaneAction::Navigate(e.name.clone()));
                                ui.close();
                            }
                            if ui.button(transfer_label).clicked() {
                                action = Some(PaneAction::Transfer(e.name.clone()));
                                ui.close();
                            }
                            if ui.button("✏ Umbenennen").clicked() {
                                action = Some(PaneAction::Rename(e.name.clone()));
                                ui.close();
                            }
                            if ui
                                .button(RichText::new("🗑 Löschen").color(theme::red()))
                                .clicked()
                            {
                                action = Some(PaneAction::Delete(e.name.clone(), e.is_dir));
                                ui.close();
                            }
                        });
                    }
                });
        });
    action
}

fn short_date(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%d.%m.%y %H:%M").to_string()
}

fn fs_dialog(ctx: &egui::Context, t: &mut SftpTab) {
    let Some(dialog) = t.dialog.as_mut() else {
        return;
    };
    let mut done: Option<bool> = None;
    egui::Modal::new(egui::Id::new("fs-dialog")).show(ctx, |ui| {
        ui.set_width(380.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        match dialog {
            FsDialog::Mkdir { name, .. } => {
                ui.label(RichText::new("Neuer Ordner").size(17.0).strong());
                ui.text_edit_singleline(name).request_focus();
            }
            FsDialog::Rename { to, from, .. } => {
                ui.label(
                    RichText::new(format!("„{from}“ umbenennen"))
                        .size(17.0)
                        .strong(),
                );
                ui.text_edit_singleline(to).request_focus();
            }
            FsDialog::Delete { name, is_dir, .. } => {
                ui.label(RichText::new("Löschen?").size(17.0).strong());
                ui.label(if *is_dir {
                    format!("Ordner „{name}“ inklusive Inhalt löschen?")
                } else {
                    format!("Datei „{name}“ löschen?")
                });
            }
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "OK").clicked() || enter {
                    done = Some(true);
                }
                if ui.button("Abbrechen").clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    done = Some(false);
                }
            });
        });
    });
    let Some(ok) = done else { return };
    let Some(dialog) = t.dialog.take() else {
        return;
    };
    if !ok {
        return;
    }
    let mut local_changed = false;
    let mut err = None;
    match dialog {
        FsDialog::Mkdir { remote: true, name } if !name.is_empty() => {
            let _ = t.tx.send(SftpCmd::Mkdir(join(&t.remote_path, &name)));
        }
        FsDialog::Mkdir {
            remote: false,
            name,
        } if !name.is_empty() => {
            err = std::fs::create_dir(t.local.path.join(name)).err();
            local_changed = true;
        }
        FsDialog::Rename {
            remote: true,
            from,
            to,
        } if !to.is_empty() && from != to => {
            let _ = t.tx.send(SftpCmd::Rename {
                from: join(&t.remote_path, &from),
                to: join(&t.remote_path, &to),
            });
        }
        FsDialog::Rename {
            remote: false,
            from,
            to,
        } if !to.is_empty() && from != to => {
            err = std::fs::rename(t.local.path.join(from), t.local.path.join(to)).err();
            local_changed = true;
        }
        FsDialog::Delete {
            remote: true,
            name,
            is_dir,
        } => {
            let _ = t.tx.send(SftpCmd::Remove {
                path: join(&t.remote_path, &name),
                is_dir,
            });
        }
        FsDialog::Delete {
            remote: false,
            name,
            is_dir,
        } => {
            let p = t.local.path.join(name);
            err = if is_dir {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            }
            .err();
            local_changed = true;
        }
        _ => {}
    }
    if local_changed {
        refresh_local(&mut t.local);
        if let Some(e) = err {
            t.local.error = Some(e.to_string());
        }
    }
}
