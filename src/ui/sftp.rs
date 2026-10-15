//! File browser. Two panes, each either the local disk or any server, with copies in
//! every direction. Server ⇄ server copies are streamed through Aurora, so the two
//! servers never have to reach each other. Windows servers (Win32-OpenSSH) work too:
//! `C:\path` input is translated to the `/C:/path` form they expect.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{Color32, CornerRadius, Margin, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::app::{
    Action, App, ConnState, EditFile, FilePane, FsDialog, RemoteConn, SftpTab, TabKind, Transfer,
};
use crate::backend::sftp::{Entry, SftpCmd, SftpEvent, join, normalize_remote, parent};
use crate::backend::ssh::SshParams;
use crate::backend::{HostKeyPrompt, Sink};
use crate::model::{Host, Protocol};
use crate::theme;
use crate::ui::human_size;
use std::time::Instant;

// ---------------------------------------------------------------------------- sources

pub fn home_dir() -> String {
    directories::UserDirs::new()
        .map(|d| d.home_dir().display().to_string())
        .unwrap_or_else(|| "/".into())
}

/// Hosts that can be browsed: SSH hosts, and RDP hosts (Windows) via their SFTP port.
pub fn sftp_hosts(app: &App) -> Vec<(Uuid, String)> {
    app.lib
        .hosts
        .iter()
        .filter(|h| matches!(h.protocol, Protocol::Ssh | Protocol::Rdp))
        .map(|h| (h.id, h.display_name().to_string()))
        .collect()
}

/// The SSH endpoint used for file access to a host (RDP hosts use their SFTP port).
pub fn sftp_target(mut host: Host) -> Host {
    if host.protocol == Protocol::Rdp {
        host.port = host.rdp.sftp_port;
        host.protocol = Protocol::Ssh;
    }
    host
}

pub fn spawn_conn(
    rt: &tokio::runtime::Runtime,
    params: SshParams,
    label: String,
    host_id: Option<Uuid>,
    ctx: &egui::Context,
    prompts: Sink<HostKeyPrompt>,
) -> RemoteConn {
    let (sink, rx) = Sink::new(ctx.clone());
    let tx = crate::backend::sftp::spawn(rt, params.clone(), sink.clone(), prompts);
    RemoteConn {
        host_id,
        label,
        params,
        tx,
        rx,
        sink,
        state: ConnState::Connecting(t!("Verbinde…").into()),
        session: None,
    }
}

pub fn local_pane(path: String) -> FilePane {
    let mut p = FilePane {
        path,
        ..Default::default()
    };
    refresh_local(&mut p);
    p
}

/// Windows drive letters, shown above `C:\`.
fn drives() -> Vec<Entry> {
    if !cfg!(windows) {
        return Vec::new();
    }
    ('A'..='Z')
        .filter(|c| Path::new(&format!("{c}:\\")).exists())
        .map(|c| Entry {
            name: format!("{c}:"),
            is_dir: true,
            is_link: false,
            size: 0,
            modified: None,
        })
        .collect()
}

fn local_parent(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    match Path::new(path).parent() {
        Some(p) => Some(p.display().to_string()),
        None if cfg!(windows) => Some(String::new()),
        None => None,
    }
}

fn local_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        format!("{name}\\")
    } else {
        Path::new(dir).join(name).display().to_string()
    }
}

pub fn refresh_local(pane: &mut FilePane) {
    pane.edit = pane.path.clone();
    pane.selected = None;
    if pane.path.is_empty() {
        pane.entries = drives();
        pane.error = None;
        return;
    }
    match std::fs::read_dir(&pane.path) {
        Ok(rd) => {
            let mut entries: Vec<Entry> = rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let meta = e.metadata().ok();
                    let ft = e.file_type().ok();
                    Entry {
                        name: e.file_name().to_string_lossy().to_string(),
                        is_dir: e.path().is_dir(),
                        is_link: ft.is_some_and(|f| f.is_symlink()),
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

impl FilePane {
    pub fn label(&self) -> &str {
        self.remote
            .as_ref()
            .map(|r| r.label.as_str())
            .unwrap_or(t!("Lokal"))
    }

    fn join(&self, name: &str) -> String {
        if self.remote.is_some() {
            join(&self.path, name)
        } else {
            local_join(&self.path, name)
        }
    }

    fn connected(&self) -> bool {
        self.remote
            .as_ref()
            .is_none_or(|r| matches!(r.state, ConnState::Connected))
    }

    fn refresh(&mut self) {
        match &self.remote {
            Some(r) => {
                let _ = r.tx.send(SftpCmd::List(self.path.clone()));
            }
            None => refresh_local(self),
        }
    }

    fn navigate(&mut self, target: String) {
        match &self.remote {
            Some(r) => {
                let _ = r.tx.send(SftpCmd::List(target));
            }
            None => {
                if target.is_empty() || Path::new(&target).is_dir() {
                    self.path = target;
                }
                refresh_local(self);
            }
        }
    }

    fn up(&mut self) {
        if self.remote.is_some() {
            let p = parent(&self.path);
            self.navigate(p);
        } else if let Some(p) = local_parent(&self.path) {
            self.navigate(p);
        }
    }
}

// ---------------------------------------------------------------------------- events

pub fn handle_event(
    t: &mut SftpTab,
    idx: usize,
    ev: SftpEvent,
    toasts: &mut Vec<(String, Color32)>,
) {
    let pane = &mut t.panes[idx];
    let Some(r) = pane.remote.as_mut() else {
        return;
    };
    match ev {
        SftpEvent::Status(s) => r.state = ConnState::Connecting(s),
        SftpEvent::Connected { home, session } => {
            r.state = ConnState::Connected;
            r.session = Some(session);
            let path = if pane.path.is_empty() {
                home
            } else {
                pane.path.clone()
            };
            let _ = r.tx.send(SftpCmd::List(path));
        }
        SftpEvent::Listing { path, entries } => {
            if pane.path != path {
                pane.selected = None;
            }
            pane.edit = path.clone();
            pane.path = path;
            pane.entries = entries;
            pane.error = None;
        }
        SftpEvent::Progress {
            id,
            name,
            done,
            total,
        } => {
            // `name` keeps the top-level item; folder copies report each file separately.
            if let Some(tr) = t.transfers.iter_mut().find(|x| x.id == id) {
                tr.current = name;
                tr.done = done;
                tr.total = total;
            }
        }
        SftpEvent::TransferDone { id, error } => {
            if let Some(ed) = t.edits.iter_mut().find(|e| e.transfer == id && !e.opened) {
                match &error {
                    None => ed.ready = true,
                    Some(e) => toasts.push((t!("Öffnen fehlgeschlagen: {}", e), theme::red())),
                }
                if error.is_some() {
                    t.edits.retain(|e| e.transfer != id);
                }
                t.transfers.retain(|x| x.id != id);
                return;
            }
            let mut refresh = None;
            if let Some(tr) = t.transfers.iter_mut().find(|x| x.id == id) {
                tr.finished = true;
                if let Some(e) = &error {
                    let e = crate::backend::ssh::friendly_text(e).unwrap_or_else(|| e.clone());
                    toasts.push((t!("Übertragung fehlgeschlagen: {}", e), theme::red()));
                } else {
                    toasts.push((t!("{} übertragen", tr.name), theme::green()));
                }
                tr.error = error;
                refresh = Some(tr.dst);
            }
            if let Some(d) = refresh {
                t.panes[d].refresh();
            }
        }
        SftpEvent::Changed => {
            let _ = r.tx.send(SftpCmd::List(pane.path.clone()));
        }
        SftpEvent::Error(e) => {
            pane.error = Some(crate::backend::ssh::friendly_text(&e).unwrap_or(e));
        }
        SftpEvent::Closed(err) => {
            if let Some(e) = &err {
                toasts.push((format!("{}: {e}", r.label), theme::red()));
            }
            r.state = ConnState::Closed(err);
            r.session = None;
        }
    }
}

/// Starts copying `name` from pane `from` into the current folder of the other pane.
fn start_transfer(
    t: &mut SftpTab,
    rt: &tokio::runtime::Handle,
    from: usize,
    name: &str,
    id: u64,
) -> Result<(), String> {
    let to = 1 - from;
    let src = t.panes[from].join(name);
    let dst_dir = t.panes[to].path.clone();
    let (a, b) = (&t.panes[from].remote, &t.panes[to].remote);
    match (a, b) {
        (None, None) => {
            return Err(
                t!("Beide Seiten sind lokal – wähle links oder rechts einen Server.").into(),
            );
        }
        (None, Some(r)) => {
            let _ = r.tx.send(SftpCmd::Upload {
                local: PathBuf::from(&src),
                remote_dir: dst_dir,
                id,
            });
        }
        (Some(r), None) => {
            if dst_dir.is_empty() {
                return Err(t!("Bitte zuerst ein Laufwerk öffnen.").into());
            }
            let _ = r.tx.send(SftpCmd::Download {
                remote: src,
                local_dir: PathBuf::from(dst_dir),
                id,
            });
        }
        (Some(ra), Some(rb)) => {
            let (Some(sa), Some(sb)) = (&ra.session, &rb.session) else {
                return Err(t!("Beide Server müssen verbunden sein.").into());
            };
            let (sa, sb, sink) = (sa.0.clone(), sb.0.clone(), rb.sink.clone());
            rt.spawn(async move {
                let r = crate::backend::sftp::relay(sa, src, sb, dst_dir, id, &sink).await;
                sink.send(SftpEvent::TransferDone {
                    id,
                    error: r.err().map(|e| format!("{e:#}")),
                });
                sink.send(SftpEvent::Changed);
            });
        }
    }
    t.transfers.push(Transfer {
        id,
        name: name.to_string(),
        current: String::new(),
        route: format!("{} → {}", t.panes[from].label(), t.panes[to].label()),
        dst: to,
        upload: to == 1,
        done: 0,
        total: 0,
        finished: false,
        error: None,
    });
    Ok(())
}

// ---------------------------------------------------------------------------- file tab

enum PaneAction {
    Select(String),
    Up,
    Refresh,
    Navigate(String),
    Open(String),
    Transfer(String),
    Mkdir,
    Rename(String),
    Delete(String, bool),
    Source(Option<Uuid>),
    Reconnect,
    /// Open a remote file in the local editor.
    Edit(String),
}

pub fn show(app: &mut App, ui: &mut egui::Ui, tab_id: u64) {
    let accent = app.accent();
    let editor = app.lib.settings.editor.clone();
    let hosts = sftp_hosts(app);
    let rt = app.rt.handle().clone();
    let mut next_id = app.next_id;
    let index = app.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
    let mut toasts: Vec<(String, Color32)> = Vec::new();
    let mut source_change: Option<(usize, Option<Uuid>)> = None;
    let mut reconnect = false;
    // In a split only the active pane takes files dropped from the OS.
    let takes_drops = app.active == Some(tab_id);
    let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab_id) else {
        return;
    };
    let title = tab.title.clone();
    let TabKind::Sftp(t) = &mut tab.kind else {
        return;
    };

    let closed = matches!(t.state(), ConnState::Closed(_));
    crate::ui::chrome::session_header(
        ui,
        egui::Id::new(("sftp-bar", tab_id)),
        index,
        &title,
        &format!("{}  ⇄  {}", t.panes[0].label(), t.panes[1].label()),
        crate::ui::chrome::state_pair(t.state()),
        |ui| {
            if closed && theme::icon_button(ui, "⟳", t!("Neu verbinden")).clicked() {
                reconnect = true;
            }
        },
    );

    // Files dropped from the OS go into the remote pane under the pointer (or the first one).
    let (dropped, pointer): (Vec<PathBuf>, Option<egui::Pos2>) = ui.input(|i| {
        (
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect(),
            i.pointer.hover_pos(),
        )
    });

    transfers_panel(ui, tab_id, t, accent);

    let mut acts: [Option<PaneAction>; 2] = [None, None];
    let mut pane_rects = [egui::Rect::NOTHING; 2];
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_deep())
                .inner_margin(Margin::symmetric(18, 14)),
        )
        .show(ui, |ui| {
            let mid = ui.max_rect().center().x;
            ui.painter().vline(
                mid,
                ui.max_rect().y_range(),
                egui::Stroke::new(1.0, theme::border()),
            );
            ui.spacing_mut().item_spacing.x = 36.0;
            ui.columns(2, |cols| {
                for (i, col) in cols.iter_mut().enumerate() {
                    pane_rects[i] = col.max_rect();
                    acts[i] = pane_ui(col, tab_id, i, &mut t.panes[i], &hosts, accent);
                }
            });
        });

    if !dropped.is_empty() && takes_drops {
        let target = pointer
            .and_then(|p| pane_rects.iter().position(|r| r.contains(p)))
            .filter(|i| t.panes[*i].remote.is_some())
            .or_else(|| t.panes.iter().position(|p| p.remote.is_some()));
        match target {
            Some(to) if t.panes[to].connected() => {
                for path in dropped {
                    next_id += 1;
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if let Some(r) = &t.panes[to].remote {
                        let _ = r.tx.send(SftpCmd::Upload {
                            local: path.clone(),
                            remote_dir: t.panes[to].path.clone(),
                            id: next_id,
                        });
                    }
                    t.transfers.push(Transfer {
                        id: next_id,
                        name,
                        current: String::new(),
                        route: t!("Lokal → {}", t.panes[to].label()),
                        dst: to,
                        upload: true,
                        done: 0,
                        total: 0,
                        finished: false,
                        error: None,
                    });
                }
            }
            _ => toasts.push((
                t!("Zum Hochladen zuerst einen Server verbinden").into(),
                theme::amber(),
            )),
        }
    }

    for (i, act) in acts.into_iter().enumerate() {
        let Some(act) = act else { continue };
        let pane = &mut t.panes[i];
        match act {
            PaneAction::Select(name) => pane.selected = Some(name),
            PaneAction::Up => pane.up(),
            PaneAction::Refresh => pane.refresh(),
            PaneAction::Navigate(input) => {
                let target = if pane.remote.is_some() {
                    let n = normalize_remote(&input);
                    if n.starts_with('/') {
                        n
                    } else {
                        join(&pane.path, &n)
                    }
                } else {
                    let p = PathBuf::from(input.trim());
                    if p.is_absolute() || input.trim().is_empty() {
                        input.trim().to_string()
                    } else {
                        local_join(&pane.path, input.trim())
                    }
                };
                pane.navigate(target);
            }
            PaneAction::Open(name) => {
                let target = pane.join(&name);
                pane.navigate(target);
            }
            PaneAction::Transfer(name) => {
                next_id += 1;
                if let Err(e) = start_transfer(t, &rt, i, &name, next_id) {
                    toasts.push((e, theme::amber()));
                }
            }
            PaneAction::Mkdir => {
                t.dialog = Some((
                    i,
                    FsDialog::Mkdir {
                        name: String::new(),
                    },
                ))
            }
            PaneAction::Rename(name) => {
                t.dialog = Some((
                    i,
                    FsDialog::Rename {
                        to: name.clone(),
                        from: name,
                    },
                ))
            }
            PaneAction::Delete(name, is_dir) => {
                t.dialog = Some((i, FsDialog::Delete { name, is_dir }))
            }
            PaneAction::Source(host) => source_change = Some((i, host)),
            PaneAction::Edit(name) => {
                let Some(safe) = crate::backend::sftp::safe_local_name(&name) else {
                    toasts.push((t!("Unsicherer Dateiname").into(), theme::red()));
                    continue;
                };
                let remote_path = pane.join(&name);
                if t.edits
                    .iter()
                    .any(|e| e.remote_path == remote_path && e.pane == i)
                {
                    toasts.push((
                        t!("Datei ist schon im Editor offen").into(),
                        theme::accent(),
                    ));
                    continue;
                }
                let dir = std::env::temp_dir()
                    .join("aurora-edit")
                    .join(uuid::Uuid::new_v4().simple().to_string());
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    toasts.push((t!("Temp-Ordner: {}", e), theme::red()));
                    continue;
                }
                next_id += 1;
                if let Some(r) = &pane.remote {
                    let _ = r.tx.send(SftpCmd::Download {
                        remote: remote_path.clone(),
                        local_dir: dir.clone(),
                        id: next_id,
                    });
                }
                t.edits.push(EditFile {
                    pane: i,
                    remote_path,
                    local: dir.join(safe),
                    transfer: next_id,
                    ready: false,
                    opened: false,
                    mtime: None,
                    last_check: Instant::now(),
                    uploads: 0,
                });
            }
            PaneAction::Reconnect => reconnect = true,
        }
    }

    // Open finished downloads in the editor; upload every save back.
    for ed in t.edits.iter_mut() {
        if ed.ready && !ed.opened {
            ed.opened = true;
            ed.mtime = std::fs::metadata(&ed.local).and_then(|m| m.modified()).ok();
            if let Err(e) = open_in_editor(&editor, &ed.local) {
                toasts.push((format!("Editor: {e}"), theme::red()));
            }
        }
        if ed.opened && ed.last_check.elapsed() >= std::time::Duration::from_millis(800) {
            ed.last_check = Instant::now();
            let now = std::fs::metadata(&ed.local).and_then(|m| m.modified()).ok();
            if now.is_some() && now != ed.mtime {
                ed.mtime = now;
                if let Some(r) = &t.panes[ed.pane].remote {
                    next_id += 1;
                    ed.uploads += 1;
                    let _ = r.tx.send(SftpCmd::Upload {
                        local: ed.local.clone(),
                        remote_dir: crate::backend::sftp::parent(&ed.remote_path),
                        id: next_id,
                    });
                    let name = ed
                        .remote_path
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    t.transfers.push(Transfer {
                        id: next_id,
                        name: t!("{} (gespeichert)", name),
                        current: String::new(),
                        route: "Editor → Server".into(),
                        dst: ed.pane,
                        upload: true,
                        done: 0,
                        total: 0,
                        finished: false,
                        error: None,
                    });
                }
            }
        }
    }
    if !t.edits.is_empty() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(800));
    }

    fs_dialog(ui.ctx(), t);
    app.next_id = next_id;
    for (text, c) in toasts {
        app.toast(text, c);
    }
    if reconnect {
        let ctx = ui.ctx().clone();
        app.reconnect(tab_id, &ctx);
    }
    if let Some((idx, host)) = source_change {
        let ctx = ui.ctx().clone();
        app.apply(Action::SetPaneSource(tab_id, idx, host), &ctx);
    }
}

fn transfers_panel(ui: &mut egui::Ui, tab_id: u64, t: &mut SftpTab, accent: Color32) {
    egui::Panel::bottom(egui::Id::new(("sftp-transfers", tab_id)))
        .resizable(true)
        .default_size(120.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .inner_margin(Margin::symmetric(18, 12)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::section_title(ui, t!("Übertragungen"));
                if t.transfers.iter().any(|x| x.finished) && text_button(ui, t!("aufräumen")).clicked()
                {
                    t.transfers.retain(|x| !x.finished);
                }
            });
            ui.add_space(6.0);
            let mut close_edit = None;
            for (i, ed) in t.edits.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(
                        RichText::new("EDIT")
                            .font(egui::FontId::monospace(10.5))
                            .color(theme::accent()),
                    );
                    let name = ed.remote_path.rsplit('/').next().unwrap_or_default();
                    ui.label(theme::mono(name, 12.5));
                    ui.label(
                        RichText::new(if !ed.opened {
                            t!("wird geladen…").to_string()
                        } else if ed.uploads == 0 {
                            t!("im Editor offen – Speichern lädt automatisch hoch").to_string()
                        } else {
                            t!("{}× gespeichert und hochgeladen", ed.uploads)
                        })
                        .size(12.0)
                        .color(theme::text_dim()),
                    );
                    if text_button(ui, t!("fertig")).clicked() {
                        close_edit = Some(i);
                    }
                });
            }
            if let Some(i) = close_edit {
                let ed = t.edits.remove(i);
                if let Some(dir) = ed.local.parent() {
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                if t.transfers.is_empty() && t.edits.is_empty() {
                    ui.label(
                        RichText::new(t!(
                            "Doppelklick auf eine Server-Datei öffnet sie im Editor (Speichern lädt hoch). \
                             Kopieren-Button überträgt auf die andere Seite, auch Server ⇄ Server."
                        ))
                        .size(12.5)
                        .color(theme::text_dim()),
                    );
                }
                for tr in t.transfers.iter().rev() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(
                            RichText::new(if tr.upload { "→" } else { "←" })
                                .font(theme::mono_medium(13.0))
                                .color(theme::accent()),
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(220.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.add(egui::Label::new(theme::mono(&tr.name, 12.5)).truncate()),
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(170.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&tr.route)
                                            .size(12.0)
                                            .color(theme::text_dim()),
                                    )
                                    .truncate(),
                                )
                            },
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
                            (None, true) => (theme::green(), t!("fertig").into()),
                            (None, false) if tr.current != tr.name && !tr.current.is_empty() => (
                                accent,
                                format!(
                                    "{} · {} / {}",
                                    tr.current,
                                    human_size(tr.done),
                                    human_size(tr.total)
                                ),
                            ),
                            (None, false) => (
                                accent,
                                format!("{} / {}", human_size(tr.done), human_size(tr.total)),
                            ),
                        };
                        let (r, _) = ui.allocate_exact_size(Vec2::new(160.0, 12.0), Sense::hover());
                        let line =
                            egui::Rect::from_center_size(r.center(), Vec2::new(r.width(), 2.0));
                        ui.painter()
                            .rect_filled(line, CornerRadius::ZERO, theme::border());
                        let mut done = line;
                        done.set_width(line.width() * frac.clamp(0.0, 1.0));
                        ui.painter().rect_filled(done, CornerRadius::ZERO, color);
                        ui.label(theme::mono(&text, 11.5).color(if tr.error.is_some() {
                            theme::red()
                        } else {
                            theme::text_dim()
                        }));
                    });
                }
            });
        });
}

fn pane_ui(
    ui: &mut egui::Ui,
    tab_id: u64,
    idx: usize,
    pane: &mut FilePane,
    hosts: &[(Uuid, String)],
    accent: Color32,
) -> Option<PaneAction> {
    let mut action = None;
    let transfer_label = if idx == 0 {
        t!("Kopieren →")
    } else {
        t!("← Kopieren")
    };
    let current = pane.remote.as_ref().and_then(|r| r.host_id);
    let is_remote = pane.remote.is_some();
    ui.set_min_size(ui.available_size());
    // Source picker; actions next to it, or in their own row when the pane is narrow.
    let narrow = ui.available_width() < 520.0;
    let actions_row = |ui: &mut egui::Ui, action: &mut Option<PaneAction>| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let sel = pane.selected.clone();
        ui.add_enabled_ui(pane.connected() && !pane.path.is_empty(), |ui| {
            if text_button(ui, t!("+ Ordner")).clicked() {
                *action = Some(PaneAction::Mkdir);
            }
        });
        ui.add_enabled_ui(sel.is_some() && pane.connected(), |ui| {
            if text_button(ui, t!("Umbenennen")).clicked()
                && let Some(s) = &sel
            {
                *action = Some(PaneAction::Rename(s.clone()));
            }
            if text_button(ui, t!("Löschen")).clicked()
                && let Some(s) = &sel
            {
                let is_dir = pane.entries.iter().any(|e| &e.name == s && e.is_dir);
                *action = Some(PaneAction::Delete(s.clone(), is_dir));
            }
            let color = if sel.is_some() {
                accent
            } else {
                theme::text_dim()
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(transfer_label).size(12.5).color(color))
                        .frame(false),
                )
                .clicked()
                && let Some(s) = &sel
            {
                *action = Some(PaneAction::Transfer(s.clone()));
            }
        });
    };
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(("pane-src", tab_id, idx))
            .selected_text(RichText::new(pane.label()).font(theme::semibold(13.5)))
            .width(if narrow {
                ui.available_width() - 8.0
            } else {
                200.0
            })
            .show_ui(ui, |ui| {
                if ui.selectable_label(!is_remote, t!("Lokal")).clicked() && is_remote {
                    action = Some(PaneAction::Source(None));
                }
                if !hosts.is_empty() {
                    ui.separator();
                }
                for (id, name) in hosts {
                    if ui.selectable_label(current == Some(*id), name).clicked()
                        && current != Some(*id)
                    {
                        action = Some(PaneAction::Source(Some(*id)));
                    }
                }
            });
        if !narrow {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0));
                    actions_row(ui, &mut action);
                });
            });
        }
    });
    if narrow {
        ui.horizontal(|ui| actions_row(ui, &mut action));
    }
    ui.add_space(4.0);

    if let Some(r) = &pane.remote
        && !matches!(r.state, ConnState::Connected)
    {
        let (text, color) = crate::ui::chrome::state_pair(&r.state);
        ui.horizontal(|ui| {
            let (lr, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            theme::led(ui.painter(), lr.center(), color, true);
            ui.add(
                egui::Label::new(RichText::new(text).size(12.5).color(theme::text_dim()))
                    .truncate(),
            );
            if matches!(r.state, ConnState::Closed(_))
                && text_button(ui, t!("Neu verbinden")).clicked()
            {
                action = Some(PaneAction::Reconnect);
            }
        });
        return action;
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if theme::icon_button(ui, "↑", t!("Übergeordneter Ordner")).clicked() {
            action = Some(PaneAction::Up);
        }
        if theme::icon_button(ui, "⟳", t!("Aktualisieren")).clicked() {
            action = Some(PaneAction::Refresh);
        }
        ui.add_space(4.0);
        let hint = if is_remote {
            t!("/pfad oder C:\\pfad")
        } else {
            t!("Pfad")
        };
        let r = ui.add(
            egui::TextEdit::singleline(&mut pane.edit)
                .id(egui::Id::new(("pane-path", tab_id, idx)))
                .hint_text(hint)
                .font(egui::FontId::monospace(12.5))
                .margin(Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = Some(PaneAction::Navigate(pane.edit.clone()));
        }
    });
    if let Some(e) = &pane.error {
        ui.label(RichText::new(e).color(theme::red()).size(12.0));
    }
    ui.add_space(8.0);
    theme::hairline(ui);

    egui::ScrollArea::vertical()
        .id_salt(("pane-scroll", tab_id, idx))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if pane.entries.is_empty() && pane.error.is_none() {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(t!("Leerer Ordner"))
                        .size(12.5)
                        .color(theme::text_dim()),
                );
            }
            for e in &pane.entries {
                if let Some(a) = entry_row(
                    ui,
                    e,
                    pane.selected.as_deref(),
                    accent,
                    transfer_label,
                    is_remote,
                ) {
                    action = Some(a);
                }
            }
        });
    action
}

fn entry_row(
    ui: &mut egui::Ui,
    e: &Entry,
    selected: Option<&str>,
    accent: Color32,
    transfer_label: &str,
    remote: bool,
) -> Option<PaneAction> {
    let mut action = None;
    let is_sel = selected == Some(e.name.as_str());
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    let p = ui.painter();
    if is_sel {
        p.rect_filled(rect, CornerRadius::ZERO, accent.linear_multiply(0.10));
        p.rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            CornerRadius::ZERO,
            accent,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::ZERO, theme::bg_hover());
    }
    // Glyph: filled block for folders, outline for files, arrow for links.
    let g = egui::Rect::from_center_size(
        rect.left_center() + Vec2::new(16.0, 0.0),
        Vec2::new(10.0, 8.0),
    );
    if e.is_dir {
        p.rect_filled(
            g,
            CornerRadius::same(1),
            theme::accent2().linear_multiply(0.8),
        );
    } else if e.is_link {
        p.text(
            g.center(),
            egui::Align2::CENTER_CENTER,
            "↗",
            egui::FontId::monospace(12.0),
            theme::text_dim(),
        );
    } else {
        p.rect_stroke(
            g,
            CornerRadius::same(1),
            egui::Stroke::new(1.0, theme::text_dim()),
            egui::StrokeKind::Inside,
        );
    }
    let font = if e.is_dir {
        theme::semibold(13.0)
    } else {
        egui::FontId::proportional(13.0)
    };
    let name_color = if e.is_dir || is_sel {
        theme::text()
    } else {
        theme::text_dim().linear_multiply(1.3)
    };
    let name_rect = egui::Rect::from_min_max(
        rect.left_top() + Vec2::new(32.0, 0.0),
        rect.right_bottom() - Vec2::new(190.0, 0.0),
    );
    p.with_clip_rect(name_rect).text(
        name_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        &e.name,
        font,
        name_color,
    );
    let meta = egui::FontId::monospace(11.5);
    if !e.is_dir {
        p.text(
            rect.right_center() - Vec2::new(116.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            human_size(e.size),
            meta.clone(),
            theme::text_dim(),
        );
    }
    if let Some(m) = e.modified {
        p.text(
            rect.right_center() - Vec2::new(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            short_date(m),
            meta,
            theme::text_dim(),
        );
    }
    if resp.clicked() {
        action = Some(PaneAction::Select(e.name.clone()));
    }
    if resp.double_clicked() {
        action = Some(if e.is_dir {
            PaneAction::Open(e.name.clone())
        } else if remote {
            // Remote files open in the local editor; copying is in the menu.
            PaneAction::Edit(e.name.clone())
        } else {
            PaneAction::Transfer(e.name.clone())
        });
    }
    resp.context_menu(|ui| {
        if remote && !e.is_dir && ui.button(t!("Bearbeiten (eigener Editor)")).clicked() {
            action = Some(PaneAction::Edit(e.name.clone()));
            ui.close();
        }
        if e.is_dir && ui.button(t!("Öffnen")).clicked() {
            action = Some(PaneAction::Open(e.name.clone()));
            ui.close();
        }
        if ui.button(transfer_label).clicked() {
            action = Some(PaneAction::Transfer(e.name.clone()));
            ui.close();
        }
        if ui.button(t!("Umbenennen")).clicked() {
            action = Some(PaneAction::Rename(e.name.clone()));
            ui.close();
        }
        if ui
            .button(RichText::new(t!("Löschen")).color(theme::red()))
            .clicked()
        {
            action = Some(PaneAction::Delete(e.name.clone(), e.is_dir));
            ui.close();
        }
    });
    action
}

/// Opens a local file in the configured editor command, or the system default app.
/// The path is passed as a single argument (no shell), so odd file names are harmless.
fn open_in_editor(editor: &str, path: &std::path::Path) -> std::io::Result<()> {
    let editor = editor.trim();
    if !editor.is_empty() {
        let mut parts = editor.split_whitespace();
        let program = parts.next().unwrap_or(editor);
        std::process::Command::new(program)
            .args(parts)
            .arg(path)
            .spawn()?;
        return Ok(());
    }
    #[cfg(windows)]
    std::process::Command::new("explorer").arg(path).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(path).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

fn text_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let color = if ui.is_enabled() {
        theme::text_dim()
    } else {
        theme::text_dim().linear_multiply(0.45)
    };
    ui.add(egui::Button::new(RichText::new(label).size(12.5).color(color)).frame(false))
}

fn short_date(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%d.%m.%y %H:%M").to_string()
}

fn fs_dialog(ctx: &egui::Context, t: &mut SftpTab) {
    let Some((_, dialog)) = t.dialog.as_mut() else {
        return;
    };
    let mut done: Option<bool> = None;
    egui::Modal::new(egui::Id::new("fs-dialog")).show(ctx, |ui| {
        ui.set_width(380.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        match dialog {
            FsDialog::Mkdir { name } => {
                ui.label(theme::heading(t!("Neuer Ordner"), 18.0));
                ui.text_edit_singleline(name).request_focus();
            }
            FsDialog::Rename { to, from } => {
                ui.label(theme::heading(t!("„{}“ umbenennen", from), 18.0));
                ui.text_edit_singleline(to).request_focus();
            }
            FsDialog::Delete { name, is_dir } => {
                ui.label(theme::heading(t!("Löschen?"), 18.0));
                ui.label(if *is_dir {
                    t!("Ordner „{}“ inklusive Inhalt löschen?", name)
                } else {
                    t!("Datei „{}“ löschen?", name)
                });
            }
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "OK").clicked() || enter {
                    done = Some(true);
                }
                if ui.button(t!("Abbrechen")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    done = Some(false);
                }
            });
        });
    });
    let Some(ok) = done else { return };
    let Some((idx, dialog)) = t.dialog.take() else {
        return;
    };
    if !ok {
        return;
    }
    let pane = &mut t.panes[idx];
    if let Some(r) = &pane.remote {
        let cmd = match dialog {
            FsDialog::Mkdir { name } if !name.is_empty() => {
                Some(SftpCmd::Mkdir(join(&pane.path, &name)))
            }
            FsDialog::Rename { from, to } if !to.is_empty() && from != to => {
                Some(SftpCmd::Rename {
                    from: join(&pane.path, &from),
                    to: join(&pane.path, &to),
                })
            }
            FsDialog::Delete { name, is_dir } => Some(SftpCmd::Remove {
                path: join(&pane.path, &name),
                is_dir,
            }),
            _ => None,
        };
        if let Some(c) = cmd {
            let _ = r.tx.send(c);
        }
        return;
    }
    let dir = pane.path.clone();
    let err = match dialog {
        FsDialog::Mkdir { name } if !name.is_empty() => {
            std::fs::create_dir(local_join(&dir, &name)).err()
        }
        FsDialog::Rename { from, to } if !to.is_empty() && from != to => {
            std::fs::rename(local_join(&dir, &from), local_join(&dir, &to)).err()
        }
        FsDialog::Delete { name, is_dir } => {
            let p = local_join(&dir, &name);
            if is_dir {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            }
            .err()
        }
        _ => None,
    };
    refresh_local(pane);
    if let Some(e) = err {
        pane.error = Some(e.to_string());
    }
}

// ---------------------------------------------------------------------------- rail page

/// "Dateien" page in the rail: pick two sources, see open file tabs, one-click hosts.
pub fn page(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let hosts = sftp_hosts(app);
    ui.label(theme::heading(t!("Dateien"), 24.0));
    ui.add_space(14.0);
    theme::hairline(ui);
    ui.add_space(18.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            theme::section_title(ui, t!("Neue Ansicht"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                source_combo(ui, "files-left", &mut app.files_left, &hosts);
                ui.label(theme::mono("⇄", 16.0).color(theme::accent()));
                source_combo(ui, "files-right", &mut app.files_right, &hosts);
                if theme::primary_button(ui, t!("Öffnen")).clicked() {
                    actions.push(Action::OpenFiles(app.files_left, app.files_right));
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(t!(
                    "Jede Seite kann lokal oder ein Server sein. Server ⇄ Server wird über Aurora \
                     gestreamt – die Server müssen sich dafür nicht gegenseitig erreichen. \
                     Windows-Server mit OpenSSH funktionieren ebenfalls."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );

            let open: Vec<(u64, String, String, usize)> = app
                .tabs
                .iter()
                .filter_map(|t| match &t.kind {
                    TabKind::Sftp(s) => Some((
                        t.id,
                        t.title.clone(),
                        format!("{}  ⇄  {}", s.panes[0].label(), s.panes[1].label()),
                        s.transfers.iter().filter(|x| !x.finished).count(),
                    )),
                    _ => None,
                })
                .collect();
            if !open.is_empty() {
                ui.add_space(26.0);
                theme::section_title(ui, t!("Offen"));
                ui.add_space(4.0);
                for (id, title, route, running) in open {
                    let (rect, resp) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(4), theme::bg_hover());
                    }
                    let p = ui.painter();
                    p.text(
                        rect.left_center() + Vec2::new(12.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &title,
                        theme::semibold(14.0),
                        theme::text(),
                    );
                    p.text(
                        rect.left_center() + Vec2::new(300.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        route,
                        egui::FontId::monospace(12.0),
                        theme::text_dim(),
                    );
                    if running > 0 {
                        p.text(
                            rect.right_center() - Vec2::new(12.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            t!("{} aktiv", running),
                            egui::FontId::monospace(11.5),
                            theme::accent(),
                        );
                    }
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        app.active = Some(id);
                    }
                }
            }

            ui.add_space(26.0);
            theme::section_title(ui, "Server");
            ui.add_space(4.0);
            if hosts.is_empty() {
                ui.label(
                    RichText::new(t!("Noch keine Server. Leg einen SSH-Host an – Windows-Server mit OpenSSH gehen auch."))
                        .color(theme::text_dim()),
                );
                ui.add_space(8.0);
                if theme::primary_button(ui, t!("SSH-Host anlegen")).clicked() {
                    actions.push(Action::NewHostOf(Protocol::Ssh, None));
                }
            }
            for h in app
                .lib
                .hosts
                .iter()
                .filter(|h| matches!(h.protocol, Protocol::Ssh | Protocol::Rdp))
            {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
                if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(4), theme::bg_hover());
                }
                let p = ui.painter();
                p.text(
                    rect.left_center() + Vec2::new(12.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    h.display_name(),
                    theme::semibold(13.5),
                    theme::text(),
                );
                let port = if h.protocol == Protocol::Rdp {
                    h.rdp.sftp_port
                } else {
                    h.port
                };
                p.text(
                    rect.left_center() + Vec2::new(300.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{}:{port}", h.hostname.trim()),
                    egui::FontId::monospace(12.0),
                    theme::text_dim(),
                );
                if h.protocol == Protocol::Rdp {
                    p.text(
                        rect.left_center() + Vec2::new(560.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        "Windows",
                        egui::FontId::monospace(11.5),
                        theme::accent2(),
                    );
                }
                p.text(
                    rect.right_center() - Vec2::new(12.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    if resp.hovered() {
                        t!("Lokal ⇄ öffnen")
                    } else {
                        ""
                    },
                    egui::FontId::proportional(12.5),
                    theme::accent(),
                );
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    actions.push(Action::OpenFiles(None, Some(h.id)));
                }
                theme::hairline(ui);
            }
        });
}

fn source_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<Uuid>, hosts: &[(Uuid, String)]) {
    let label = value
        .and_then(|v| hosts.iter().find(|(id, _)| *id == v))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| t!("Lokal").into());
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .width(220.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, t!("Lokal"));
            for (hid, name) in hosts {
                ui.selectable_value(value, Some(*hid), name);
            }
        });
}
