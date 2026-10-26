//! "Backup-Server" page: Proxmox Backup Servers with their datastores, snapshots per
//! backup group, a read-only prune preview and the download of `.blob` files.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use egui::{CornerRadius, FontId, Pos2, RichText, Sense, Vec2};
use uuid::Uuid;

use super::{ClusterAction, act, bytes, local_time, notice};
use crate::app::{Action, App};
use crate::backend::proxmox::cluster::pbs::{
    self as pb, Datastore, Keep, Mark, PbsPayload, PbsSnapshot,
};
use crate::backend::proxmox::{Auth, Conn, Epoch, Error, Session};
use crate::model::PbsServer;
use crate::theme;
use crate::ui::icons::{self, Icon};

#[derive(Default)]
pub struct PbsLive {
    pub loading: bool,
    pub error: Option<Error>,
    pub stores: Option<Vec<Datastore>>,
    pub keep: HashMap<String, Keep>,
    pub session: Option<Session>,
    pub at: Option<Instant>,
}

pub struct PbsEditor {
    pub server: PbsServer,
    pub is_new: bool,
    pub secret: String,
    pub testing: bool,
    pub result: Option<Result<String, String>>,
    pub confirm_delete: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TrustTarget {
    Editor,
    Server(Uuid),
}

pub struct PbsTrust {
    pub target: TrustTarget,
    pub host: String,
    pub fingerprint: String,
}

/// Snapshots of one datastore.
pub struct SnapView {
    pub server: Uuid,
    pub store: String,
    pub list: Option<Result<Vec<PbsSnapshot>, String>>,
    pub prune: bool,
    pub keep: Keep,
    pub open: HashSet<(String, String)>,
    pub downloading: Option<String>,
}

/// A downloaded blob: shown as text, saved on request.
pub struct BlobView {
    pub name: String,
    pub data: Vec<u8>,
    pub saved: Option<String>,
}

#[derive(Default)]
pub struct PbsState {
    pub live: HashMap<Uuid, PbsLive>,
    pub editor: Option<PbsEditor>,
    pub trust: Option<PbsTrust>,
    pub snaps: Option<SnapView>,
    pub blob: Option<BlobView>,
    counter: Epoch,
    base: Epoch,
    gens: HashMap<Uuid, Epoch>,
}

impl PbsState {
    pub fn lock(&mut self) {
        self.live.clear();
        self.editor = None;
        self.trust = None;
        self.snaps = None;
        self.blob = None;
        self.counter += 1;
        self.base = self.counter;
        self.gens.clear();
    }

    pub(crate) fn epoch(&self, server: Uuid) -> Epoch {
        self.gens.get(&server).copied().unwrap_or(self.base)
    }

    fn reset(&mut self, server: Uuid) {
        self.counter += 1;
        self.gens.insert(server, self.counter);
        self.live.remove(&server);
        if self.snaps.as_ref().is_some_and(|s| s.server == server) {
            self.snaps = None;
            self.blob = None;
        }
    }
}

#[derive(Clone, Debug)]
pub enum PbsAction {
    New,
    Edit(Uuid),
    Delete(Uuid),
    OpenStore(Uuid, String),
    Download(Uuid, String, Box<PbsSnapshot>, String),
}

// ------------------------------------------------------------------ backend glue

fn conn_with(s: &PbsServer, secret: String, session: Option<Session>) -> Conn {
    let user = s.user.trim().to_string();
    Conn {
        host: s.host.trim().to_string(),
        port: s.port,
        auth: if s.token {
            Auth::Token { id: user, secret }
        } else {
            Auth::Password {
                user,
                password: secret,
            }
        },
        pin: Some(s.tls_pin.trim().to_string()).filter(|p| !p.is_empty()),
        session,
    }
}

pub(crate) fn conn(app: &App, s: &PbsServer) -> Conn {
    conn_with(
        s,
        app.keychain
            .pbs_secrets
            .get(&s.id)
            .cloned()
            .unwrap_or_default(),
        app.pve
            .cluster
            .pbs
            .live
            .get(&s.id)
            .and_then(|l| l.session.clone()),
    )
}

pub(crate) fn server(app: &App, id: Uuid) -> Option<PbsServer> {
    app.lib.pbs_servers.iter().find(|s| s.id == id).cloned()
}

fn refresh(app: &mut App, id: Uuid) {
    let Some(s) = server(app, id).filter(|s| !s.host.trim().is_empty()) else {
        return;
    };
    let c = conn(app, &s);
    let st = &mut app.pve.cluster.pbs;
    let epoch = st.epoch(id);
    let live = st.live.entry(id).or_default();
    if live.loading
        || matches!(
            live.error,
            Some(Error::Untrusted(_) | Error::PinChanged { .. })
        )
    {
        return;
    }
    live.loading = true;
    live.at = Some(Instant::now());
    pb::spawn_stores(&app.rt, id, epoch, c, app.pve.cluster.sink.clone());
}

pub fn refresh_all(app: &mut App) {
    let ids: Vec<Uuid> = app.lib.pbs_servers.iter().map(|s| s.id).collect();
    for id in ids {
        refresh(app, id);
    }
}

pub(crate) fn load_snapshots(app: &mut App) {
    let Some(v) = app.pve.cluster.pbs.snaps.as_ref() else {
        return;
    };
    let Some(s) = server(app, v.server) else {
        return;
    };
    let (id, store) = (v.server, v.store.clone());
    let c = conn(app, &s);
    pb::spawn_snapshots(
        &app.rt,
        id,
        app.pve.cluster.pbs.epoch(id),
        c,
        store,
        app.pve.cluster.sink.clone(),
    );
}

pub fn on_payload(
    app: &mut App,
    server: Uuid,
    epoch: Epoch,
    session: Option<Session>,
    payload: PbsPayload,
) {
    let st = &mut app.pve.cluster.pbs;
    if st.epoch(server) != epoch || !app.lib.pbs_servers.iter().any(|s| s.id == server) {
        return;
    }
    let live = st.live.entry(server).or_default();
    if session.is_some() {
        live.session = session;
    }
    match payload {
        PbsPayload::Stores { result, keep } => {
            live.loading = false;
            match result {
                Ok(stores) => {
                    live.stores = Some(stores);
                    live.keep = keep;
                    live.error = None;
                }
                Err(e) => {
                    if let Error::Untrusted(fp) = &e
                        && st.trust.is_none()
                    {
                        let host = app
                            .lib
                            .pbs_servers
                            .iter()
                            .find(|s| s.id == server)
                            .map(|s| format!("{}:{}", s.host.trim(), s.port))
                            .unwrap_or_default();
                        st.trust = Some(PbsTrust {
                            target: TrustTarget::Server(server),
                            host,
                            fingerprint: fp.clone(),
                        });
                    }
                    let live = st.live.entry(server).or_default();
                    if matches!(e, Error::PinChanged { .. }) {
                        live.stores = None;
                        live.session = None;
                    }
                    live.error = Some(e);
                }
            }
        }
        PbsPayload::Snapshots { store, result } => {
            if let Some(v) = st
                .snaps
                .as_mut()
                .filter(|v| v.server == server && v.store == store)
            {
                let first = v.list.is_none();
                v.list = Some(result.map_err(|e| e.to_string()));
                // Open the first group right away.
                if first
                    && let Some(Ok(list)) = &v.list
                    && let Some(s) = list.first()
                {
                    v.open.insert((s.btype.clone(), s.bid.clone()));
                }
            }
        }
        PbsPayload::Download { name, result } => {
            if let Some(v) = st.snaps.as_mut() {
                v.downloading = None;
            }
            match result {
                Ok(data) => {
                    st.blob = Some(BlobView {
                        name,
                        data,
                        saved: None,
                    })
                }
                Err(e) => app.toast(t!("Download fehlgeschlagen: {}", e), theme::red()),
            }
        }
    }
}

pub fn on_tested(app: &mut App, result: Result<String, Error>) {
    let st = &mut app.pve.cluster.pbs;
    let Some(ed) = st.editor.as_mut() else {
        return;
    };
    ed.testing = false;
    match result {
        Ok(v) => ed.result = Some(Ok(v)),
        Err(Error::Untrusted(fp)) => {
            ed.result = None;
            st.trust = Some(PbsTrust {
                target: TrustTarget::Editor,
                host: format!("{}:{}", ed.server.host.trim(), ed.server.port),
                fingerprint: fp,
            });
        }
        Err(e) => ed.result = Some(Err(e.to_string())),
    }
}

pub fn apply(app: &mut App, action: PbsAction, _ctx: &egui::Context) {
    match action {
        PbsAction::New => {
            app.pve.cluster.pbs.editor = Some(PbsEditor {
                server: PbsServer {
                    user: "root@pam!aurora".into(),
                    ..Default::default()
                },
                is_new: true,
                secret: String::new(),
                testing: false,
                result: None,
                confirm_delete: false,
            });
        }
        PbsAction::Edit(id) => {
            if let Some(s) = server(app, id) {
                app.pve.cluster.pbs.editor = Some(PbsEditor {
                    secret: app
                        .keychain
                        .pbs_secrets
                        .get(&id)
                        .cloned()
                        .unwrap_or_default(),
                    server: s,
                    is_new: false,
                    testing: false,
                    result: None,
                    confirm_delete: false,
                });
            }
        }
        PbsAction::Delete(id) => {
            app.lib.pbs_servers.retain(|s| s.id != id);
            app.pve.cluster.pbs.reset(id);
            if app.keychain.pbs_secrets.remove(&id).is_some() {
                app.save_keychain();
            }
            app.save_library();
        }
        PbsAction::OpenStore(id, store) => {
            if !pb::valid_datastore(&store) {
                return;
            }
            let keep = app
                .pve
                .cluster
                .pbs
                .live
                .get(&id)
                .and_then(|l| l.keep.get(&store).copied())
                .unwrap_or_default();
            app.pve.cluster.pbs.snaps = Some(SnapView {
                server: id,
                store,
                list: None,
                prune: false,
                keep,
                open: HashSet::new(),
                downloading: None,
            });
            load_snapshots(app);
        }
        PbsAction::Download(id, store, snap, file) => {
            let Some(s) = server(app, id) else {
                return;
            };
            if pb::download_path(&store, &snap, &file).is_none() {
                app.toast(t!("ungültiger Dateiname"), theme::red());
                return;
            }
            if let Some(v) = app.pve.cluster.pbs.snaps.as_mut() {
                v.downloading = Some(file.clone());
            }
            let c = conn(app, &s);
            pb::spawn_download(
                &app.rt,
                id,
                app.pve.cluster.pbs.epoch(id),
                c,
                store,
                *snap,
                file,
                app.pve.cluster.sink.clone(),
            );
        }
    }
}

fn pbs(a: PbsAction) -> Action {
    act(ClusterAction::Pbs(a))
}

// ------------------------------------------------------------------ page

pub fn page(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    // Datastore numbers every minute while visible.
    let due: Vec<Uuid> = app
        .lib
        .pbs_servers
        .iter()
        .filter(|s| {
            app.pve
                .cluster
                .pbs
                .live
                .get(&s.id)
                .is_none_or(|l| !l.loading && l.at.is_none_or(|t| t.elapsed().as_secs() >= 60))
        })
        .map(|s| s.id)
        .collect();
    for id in due {
        refresh(app, id);
    }
    theme::toolbar(
        ui,
        "pbs-head",
        |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "Proxmox Backup Server: Datastores, Snapshots, Dateien, Prune, Verify und Garbage Collection."
                    ))
                    .color(theme::text_dim()),
                )
                .truncate(),
            );
        },
        |ui, _| {
            if theme::btn(ui, theme::Btn::Small, Some(Icon::Plus), t!("Backup-Server"))
                .on_hover_text(t!("Proxmox Backup Server hinzufügen"))
                .clicked()
            {
                actions.push(pbs(PbsAction::New));
            }
        },
    );
    ui.add_space(14.0);
    if app.lib.pbs_servers.is_empty() {
        theme::card(ui, theme::bg_deep(), |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.set_max_width(560.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                    ui.painter()
                        .rect_filled(r, CornerRadius::same(11), theme::accent_soft());
                    icons::paint(ui.painter(), r.shrink(10.0), Icon::Download, theme::accent());
                    ui.add_space(8.0);
                    ui.label(theme::heading(t!("Noch kein Backup-Server"), 20.0));
                    ui.label(
                        RichText::new(t!(
                            "Verbinde einen Proxmox Backup Server (Port 8007), um Datastores, Snapshots, Verify-Status und eine Prune-Vorschau zu sehen."
                        ))
                        .color(theme::text_dim()),
                    );
                    ui.add_space(8.0);
                    if theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("Backup-Server hinzufügen"))
                        .clicked()
                    {
                        actions.push(pbs(PbsAction::New));
                    }
                });
        });
        return;
    }
    let servers = super::super::pve2::visible_pbs(app);
    if servers.is_empty() {
        ui.label(
            RichText::new(t!("Keine Backup-Server für diese Auswahl")).color(theme::text_dim()),
        );
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("pbs-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            for (i, s) in servers.iter().enumerate() {
                if i > 0 {
                    ui.add_space(22.0);
                }
                server_section(app, ui, s, actions);
            }
            ui.add_space(40.0);
        });
}

fn server_section(app: &mut App, ui: &mut egui::Ui, s: &PbsServer, actions: &mut Vec<Action>) {
    let live = app.pve.cluster.pbs.live.get(&s.id);
    let error = live.and_then(|l| l.error.clone());
    let loading = live.is_some_and(|l| l.loading);
    let stores = live.and_then(|l| l.stores.clone());
    let cust = super::super::pve2::cust_of(app, s.group);
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            match (&error, &stores) {
                (Some(_), _) => theme::led(ui.painter(), r.center(), theme::red(), true),
                (None, Some(_)) => theme::led(ui.painter(), r.center(), theme::green(), true),
                _ => theme::led(ui.painter(), r.center(), theme::off_color(), false),
            }
            super::super::pve2::customer_tag(ui, &cust);
            ui.label(RichText::new(s.display_name()).font(theme::semibold(16.0)));
            let addr = if s.port == 8007 {
                s.host.trim().to_string()
            } else {
                format!("{}:{}", s.host.trim(), s.port)
            };
            ui.label(theme::mono(addr, 12.0).color(theme::text_dim()));
            if loading && stores.is_none() {
                ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::btn(ui, theme::Btn::Link, None, t!("Bearbeiten")).clicked() {
                    actions.push(pbs(PbsAction::Edit(s.id)));
                }
            });
        })
        .response;
    let _ = resp;
    if let Some(e) = &error {
        ui.add_space(6.0);
        notice(ui, theme::red(), &e.to_string());
        if let Error::Untrusted(fp) = e {
            ui.add_space(4.0);
            if theme::btn(ui, theme::Btn::Small, None, t!("Zertifikat prüfen…")).clicked() {
                app.pve.cluster.pbs.trust = Some(PbsTrust {
                    target: TrustTarget::Server(s.id),
                    host: format!("{}:{}", s.host.trim(), s.port),
                    fingerprint: fp.clone(),
                });
            }
        }
    }
    let Some(stores) = stores else {
        return;
    };
    ui.add_space(8.0);
    if stores.is_empty() {
        ui.label(RichText::new(t!("Keine Datastores")).color(theme::text_dim()));
        return;
    }
    theme::card(ui, theme::bg_deep(), |ui| {
        for (i, d) in stores.iter().enumerate() {
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
            let p = ui.painter();
            if i > 0 {
                p.line_segment(
                    [rect.left_top() + Vec2::new(18.0, 0.0), rect.right_top()],
                    egui::Stroke::new(1.0, theme::line()),
                );
            }
            if resp.hovered() {
                p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
            }
            let cy = rect.center().y;
            let x = rect.left() + 18.0;
            icons::paint(
                p,
                egui::Rect::from_center_size(Pos2::new(x + 8.0, cy), Vec2::splat(16.0)),
                Icon::Download,
                theme::text_dim(),
            );
            // Columns: name | bar + numbers | "Snapshots ›"; the bar shrinks and the
            // texts truncate on narrow windows.
            let link = p.layout_no_wrap(
                t!("Snapshots ›").to_string(),
                theme::medium(13.0),
                theme::accent(),
            );
            let link_left = rect.right() - 18.0 - link.size().x;
            p.galley(
                Pos2::new(link_left, cy - link.size().y / 2.0),
                link,
                theme::accent(),
            );
            let bx = rect.left() + rect.width() * 0.4;
            let name_w = bx - (x + 28.0) - 12.0;
            let g = theme::truncated(p, &d.name, theme::semibold(14.0), theme::text(), name_w);
            p.galley(
                Pos2::new(x + 28.0, cy - 8.0 - g.size().y / 2.0),
                g,
                theme::text(),
            );
            let comment = if d.comment.is_empty() {
                t!("Datastore")
            } else {
                d.comment.as_str()
            };
            let g = theme::truncated(
                p,
                comment,
                FontId::proportional(12.0),
                theme::text_dim(),
                name_w,
            );
            p.galley(
                Pos2::new(x + 28.0, cy + 10.0 - g.size().y / 2.0),
                g,
                theme::text_dim(),
            );
            let room = link_left - 16.0 - bx;
            if let Some(e) = &d.error {
                let g = theme::truncated(p, e, FontId::proportional(12.5), theme::amber(), room);
                p.galley(Pos2::new(bx, cy - g.size().y / 2.0), g, theme::amber());
            } else if d.total > 0 && room > 40.0 {
                let text = format!(
                    "{} %  ·  {} / {}",
                    (d.usage() * 100.0).round(),
                    bytes(d.used),
                    bytes(d.total)
                );
                let color = if d.usage() > crate::backend::proxmox::cluster::STORAGE_WARN {
                    super::level_color(d.usage())
                } else {
                    theme::text_dim()
                };
                let tw = p
                    .layout_no_wrap(text.clone(), FontId::proportional(12.5), color)
                    .size()
                    .x;
                let bar_w = (room - tw - 14.0).min(200.0);
                let tx = if bar_w >= 40.0 {
                    let bar =
                        egui::Rect::from_min_size(Pos2::new(bx, cy - 4.0), Vec2::new(bar_w, 8.0));
                    super::paint_bar(p, bar, d.usage());
                    bx + bar_w + 14.0
                } else {
                    bx
                };
                let g = theme::truncated(
                    p,
                    text,
                    FontId::proportional(12.5),
                    color,
                    link_left - 16.0 - tx,
                );
                p.galley(Pos2::new(tx, cy - g.size().y / 2.0), g, color);
            }
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                actions.push(pbs(PbsAction::OpenStore(s.id, d.name.clone())));
            }
        }
    });
}

// ------------------------------------------------------------------ windows

pub fn windows(app: &mut App, ctx: &egui::Context) {
    snapshots_sheet(app, ctx);
    blob_sheet(app, ctx);
    editor(app, ctx);
    trust(app, ctx);
}

fn verify_pill(ui: &mut egui::Ui, v: Option<&str>) {
    match v {
        Some("ok") => theme::pill(ui, t!("verifiziert"), theme::green()),
        Some("failed") => theme::pill(ui, t!("Verify fehlgeschlagen"), theme::red()),
        Some(other) => theme::pill(ui, other, theme::amber()),
        None => theme::pill(ui, t!("nicht verifiziert"), theme::text_dim()),
    };
}

/// `>` closed, `v` open.
fn paint_chevron(p: &egui::Painter, r: egui::Rect, open: bool, color: egui::Color32) {
    let c = r.center();
    let h = r.width() / 2.0;
    let pts = if open {
        vec![
            Pos2::new(c.x - h, c.y - h / 2.0),
            Pos2::new(c.x, c.y + h / 2.0),
            Pos2::new(c.x + h, c.y - h / 2.0),
        ]
    } else {
        vec![
            Pos2::new(c.x - h / 2.0, c.y - h),
            Pos2::new(c.x + h / 2.0, c.y),
            Pos2::new(c.x - h / 2.0, c.y + h),
        ]
    };
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.6, color)));
}

fn keep_field(ui: &mut egui::Ui, label: &str, v: &mut u32) {
    ui.label(RichText::new(label).size(12.5).color(theme::text_dim()));
    ui.add(egui::DragValue::new(v).range(0..=999));
    ui.add_space(6.0);
}

fn snapshots_sheet(app: &mut App, ctx: &egui::Context) {
    let server_name = match &app.pve.cluster.pbs.snaps {
        Some(v) => server(app, v.server)
            .map(|s| s.display_name().to_string())
            .unwrap_or_default(),
        None => return,
    };
    let Some(v) = app.pve.cluster.pbs.snaps.as_mut() else {
        return;
    };
    let mut todo: Option<PbsAction> = None;
    let mut reload = false;
    let mut p2acts: Vec<super::super::pve2::Act> = Vec::new();
    let p2 = &app.pve.p2;
    let title = format!("{} · {}", server_name, v.store);
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pbs-snapshots"),
        860.0,
        &title,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                theme::toggle(ui, &mut v.prune);
                ui.label(t!("Prune-Vorschau"));
                ui.label(
                    RichText::new(t!("nur Anzeige – es wird nichts gelöscht"))
                        .size(12.5)
                        .color(theme::text_dim()),
                );
            });
            if v.prune {
                ui.horizontal_wrapped(|ui| {
                    keep_field(ui, "keep-last", &mut v.keep.last);
                    keep_field(ui, "hourly", &mut v.keep.hourly);
                    keep_field(ui, "daily", &mut v.keep.daily);
                    keep_field(ui, "weekly", &mut v.keep.weekly);
                    keep_field(ui, "monthly", &mut v.keep.monthly);
                    keep_field(ui, "yearly", &mut v.keep.yearly);
                });
            }
            super::super::pve2::pbs_ui::store_bar(ui, p2, v.server, &v.store, &mut p2acts);
            let list = match &v.list {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(t!("lädt…")).color(theme::text_dim()));
                    });
                    return;
                }
                Some(Err(e)) => {
                    notice(ui, theme::red(), e);
                    return;
                }
                Some(Ok(list)) => list,
            };
            if list.is_empty() {
                ui.label(RichText::new(t!("Keine Snapshots")).color(theme::text_dim()));
                return;
            }
            let groups = pb::groups(list);
            let mut total_remove = 0;
            for ((btype, bid), snaps) in &groups {
                let key = (btype.clone(), bid.clone());
                let marks = v.prune.then(|| {
                    let times: Vec<(i64, bool)> =
                        snaps.iter().map(|s| (s.time, s.protected)).collect();
                    pb::prune_preview_local(&times, v.keep)
                });
                let removes = marks
                    .as_ref()
                    .map(|m| m.iter().filter(|m| **m == Mark::Remove).count())
                    .unwrap_or(0);
                total_remove += removes;
                let open = v.open.contains(&key);
                let head = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                        paint_chevron(ui.painter(), r.shrink(5.0), open, theme::text_dim());
                        let name = ui.add(
                            egui::Label::new(
                                RichText::new(format!("{btype}/{bid}")).font(theme::semibold(14.0)),
                            )
                            .sense(Sense::click()),
                        );
                        ui.label(
                            RichText::new(t!("{} Snapshots", snaps.len()))
                                .size(12.5)
                                .color(theme::text_dim()),
                        );
                        if let Some(last) = snaps.first() {
                            ui.label(
                                RichText::new(t!("zuletzt {}", local_time(last.time)))
                                    .size(12.5)
                                    .color(theme::text_dim()),
                            );
                        }
                        if removes > 0 {
                            theme::tag(
                                ui,
                                &t!("{} würden gelöscht", removes),
                                Some(theme::amber()),
                            );
                        }
                        super::super::pve2::pbs_ui::group_buttons(
                            ui,
                            v.server,
                            &v.store,
                            btype,
                            bid,
                            &mut p2acts,
                        );
                        resp.clicked() || name.clicked()
                    })
                    .inner;
                if head {
                    if open {
                        v.open.remove(&key);
                    } else {
                        v.open.insert(key.clone());
                    }
                }
                if !open {
                    continue;
                }
                theme::form(ui, 110.0, |ui, f| {
                    for (i, s) in snaps.iter().enumerate() {
                        f.full(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                ui.label(theme::mono(
                                    chrono::DateTime::from_timestamp(s.time, 0)
                                        .map(|t| {
                                            t.with_timezone(&chrono::Local)
                                                .format("%Y-%m-%d %H:%M:%S")
                                                .to_string()
                                        })
                                        .unwrap_or_default(),
                                    12.5,
                                ));
                                if let Some(size) = s.size {
                                    ui.label(
                                        RichText::new(bytes(size))
                                            .size(12.5)
                                            .color(theme::text_dim()),
                                    );
                                }
                                verify_pill(ui, s.verify.as_deref());
                                if s.protected {
                                    theme::pill(ui, t!("geschützt"), theme::accent());
                                }
                                if let Some(m) = marks.as_ref().map(|m| m[i]) {
                                    match m {
                                        Mark::Keep => {
                                            theme::tag(ui, t!("bleibt"), Some(theme::green()))
                                        }
                                        Mark::Protected => theme::tag(
                                            ui,
                                            t!("bleibt (geschützt)"),
                                            Some(theme::green()),
                                        ),
                                        Mark::Remove => {
                                            theme::tag(ui, t!("würde gelöscht"), Some(theme::red()))
                                        }
                                    };
                                }
                                if !s.comment.is_empty() {
                                    ui.label(
                                        RichText::new(&s.comment)
                                            .size(12.5)
                                            .color(theme::text_dim()),
                                    );
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let archives: Vec<&str> = s
                                    .files
                                    .iter()
                                    .filter(|f| !pb::valid_blob_name(&f.name))
                                    .map(|f| f.name.as_str())
                                    .collect();
                                if !archives.is_empty() {
                                    theme::tag(ui, &t!("{} Archive", archives.len()), None)
                                        .on_hover_text(archives.join("\n"));
                                }
                                for file in s.files.iter().filter(|f| pb::valid_blob_name(&f.name))
                                {
                                    let label =
                                        file.name.strip_suffix(".blob").unwrap_or(&file.name);
                                    let text = if file.encrypted {
                                        format!("🔒 {label}")
                                    } else {
                                        label.to_string()
                                    };
                                    if v.downloading.as_deref() == Some(&file.name) {
                                        ui.spinner();
                                    }
                                    let r = ui
                                        .add_enabled_ui(v.downloading.is_none(), |ui| {
                                            theme::btn(
                                                ui,
                                                theme::Btn::Link,
                                                Some(Icon::Download),
                                                &text,
                                            )
                                        })
                                        .inner
                                        .on_hover_text(t!("{} anzeigen und speichern", file.name));
                                    if r.clicked() {
                                        todo = Some(PbsAction::Download(
                                            v.server,
                                            v.store.clone(),
                                            Box::new((*s).clone()),
                                            file.name.clone(),
                                        ));
                                    }
                                }
                                super::super::pve2::pbs_ui::snapshot_buttons(
                                    ui,
                                    v.server,
                                    &v.store,
                                    s,
                                    &mut p2acts,
                                );
                            });
                        });
                    }
                });
                ui.add_space(6.0);
            }
            if v.prune {
                ui.add_space(4.0);
                notice(
                    ui,
                    if total_remove > 0 {
                        theme::amber()
                    } else {
                        theme::green()
                    },
                    &t!(
                        "Mit diesen Einstellungen würde ein Prune {} Snapshots entfernen. Diese Vorschau löscht nichts – mit „Prune…“ prüft der Server die Regeln und löscht nach Bestätigung.",
                        total_remove
                    ),
                );
            }
        },
        |ui, _| {
            let mut close = false;
            if theme::btn(ui, theme::Btn::Primary, None, t!("Schließen")).clicked() {
                close = true;
            }
            if theme::btn(ui, theme::Btn::Normal, Some(Icon::Refresh), t!("Neu laden")).clicked() {
                reload = true;
            }
            close
        },
    );
    for a in p2acts {
        super::super::pve2::apply(app, a, ctx);
    }
    if let Some(a) = todo {
        apply(app, a, ctx);
        return;
    }
    if reload {
        if let Some(v) = app.pve.cluster.pbs.snaps.as_mut() {
            v.list = None;
        }
        load_snapshots(app);
        return;
    }
    if (close || dismissed) && app.pve.cluster.pbs.blob.is_none() {
        app.pve.cluster.pbs.snaps = None;
    }
}

/// `name` without `.blob`, only file-name-safe characters.
fn save_name(name: &str) -> String {
    let base = name.strip_suffix(".blob").unwrap_or(name);
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim_start_matches('.').to_string();
    if clean.is_empty() {
        "download".into()
    } else {
        clean
    }
}

fn save_blob(name: &str, data: &[u8]) -> anyhow::Result<std::path::PathBuf> {
    let dir = directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
        .ok_or_else(|| anyhow::anyhow!(t!("Kein Download-Ordner gefunden")))?;
    let file = save_name(name);
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (file.clone(), String::new()),
    };
    for i in 0..1000 {
        let candidate = if i == 0 {
            dir.join(&file)
        } else {
            dir.join(format!("{stem}-{i}{ext}"))
        };
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        match opts.open(&candidate) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(data)?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(t!("Kein freier Dateiname"))
}

fn blob_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(b) = app.pve.cluster.pbs.blob.as_mut() else {
        return;
    };
    let mut save = false;
    let text = std::str::from_utf8(&b.data).ok();
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pbs-blob"),
        760.0,
        &b.name,
        |ui| {
            ui.label(
                RichText::new(bytes(b.data.len() as u64))
                    .size(12.5)
                    .color(theme::text_dim()),
            );
            egui::Frame::NONE
                .fill(theme::bg_deep())
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::both()
                        .id_salt("pbs-blob-text")
                        .max_height(440.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| match text {
                            Some(t) => {
                                ui.label(theme::mono(t, 12.0));
                            }
                            None => {
                                ui.label(
                                    RichText::new(t!("Binärdaten – nur Speichern möglich"))
                                        .color(theme::text_dim()),
                                );
                            }
                        });
                });
            if let Some(p) = &b.saved {
                ui.label(RichText::new(t!("Gespeichert: {}", p)).color(theme::green()));
            }
        },
        |ui, _| {
            let mut close = false;
            if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                close = true;
            }
            if theme::btn(
                ui,
                theme::Btn::Primary,
                Some(Icon::Download),
                t!("Speichern"),
            )
            .on_hover_text(t!("Im Download-Ordner speichern"))
            .clicked()
            {
                save = true;
            }
            close
        },
    );
    if save {
        match save_blob(&b.name, &b.data) {
            Ok(p) => b.saved = Some(p.display().to_string()),
            Err(e) => {
                let msg = t!("Speichern fehlgeschlagen: {}", format!("{e:#}"));
                app.toast(msg, theme::red());
            }
        }
        return;
    }
    if close || dismissed {
        app.pve.cluster.pbs.blob = None;
    }
}

fn editor(app: &mut App, ctx: &egui::Context) {
    let customers = super::super::pve2::group_choices(app);
    let Some(ed) = app.pve.cluster.pbs.editor.as_mut() else {
        return;
    };
    let title = if ed.is_new {
        t!("Backup-Server hinzufügen")
    } else {
        t!("Backup-Server bearbeiten")
    };
    let mut test = false;
    let mut delete = false;
    let can = !ed.server.host.trim().is_empty() && !ed.server.user.trim().is_empty();
    let testing = ed.testing;
    let confirming = ed.confirm_delete;
    let ((save, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pbs-editor"),
        560.0,
        title,
        |ui| {
            let s = &mut ed.server;
            theme::form(ui, 110.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    super::super::text(ui, &mut s.name, t!("z.B. pbs-keller"));
                });
                f.field(ui, t!("Adresse"), |ui| {
                    let w = ui.available_width() - 86.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut s.host)
                            .hint_text(RichText::new("192.168.1.20").color(theme::text_dim()))
                            .desired_width(w),
                    );
                    ui.label(RichText::new(":").color(theme::text_dim()));
                    ui.add(egui::DragValue::new(&mut s.port).range(1..=65535));
                });
                f.field(ui, t!("Kunde"), |ui| {
                    super::super::pve2::group_picker(ui, &customers, &mut s.group);
                });
            });
            theme::form_caption(ui, t!("Anmeldung"));
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                let cur = if s.token { 0 } else { 1 };
                if let Some(i) =
                    theme::segmented(ui, &[t!("API-Token"), t!("Benutzer + Passwort")], cur)
                    && s.token != (i == 0)
                {
                    s.token = i == 0;
                    ed.secret.clear();
                    if !s.token
                        && let Some((user, _)) = s.user.split_once('!')
                    {
                        s.user = user.to_string();
                    }
                }
            });
            ui.add_space(8.0);
            theme::form(ui, 110.0, |ui, f| {
                if s.token {
                    f.field(ui, t!("Token-ID"), |ui| {
                        super::super::text(ui, &mut s.user, "root@pam!aurora");
                    });
                    f.field(ui, t!("Secret"), |ui| {
                        super::super::secret(ui, &mut ed.secret, "xxxxxxxx-xxxx-…");
                    });
                } else {
                    f.field(ui, t!("Benutzer"), |ui| {
                        super::super::text(ui, &mut s.user, "root@pam");
                    });
                    f.field(ui, t!("Passwort"), |ui| {
                        super::super::secret(ui, &mut ed.secret, "");
                    });
                }
            });
            super::super::hint(
                ui,
                t!(
                    "Für die Anzeige reicht ein Token mit der Rolle DatastoreAudit (für Downloads DatastoreReader)."
                ),
            );
            theme::form_caption(ui, t!("Zertifikat"));
            theme::form(ui, 110.0, |ui, f| {
                f.full(ui, |ui| {
                    if s.tls_pin.is_empty() {
                        ui.label(
                            RichText::new(t!(
                                "noch nicht bestätigt – wird beim ersten Verbinden angezeigt"
                            ))
                            .size(12.5)
                            .color(theme::text_dim()),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                theme::mono(&s.tls_pin, 11.0).color(theme::text_dim()),
                            )
                            .wrap(),
                        );
                        ui.add_space(6.0);
                        if theme::btn(ui, theme::Btn::Small, None, t!("Pin zurücksetzen")).clicked()
                        {
                            s.tls_pin.clear();
                        }
                    }
                });
            });
            match &ed.result {
                Some(Ok(v)) => {
                    ui.add_space(10.0);
                    ui.label(RichText::new(t!("Verbunden: {}", v)).color(theme::green()));
                }
                Some(Err(e)) => {
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(RichText::new(e).color(theme::red())).wrap());
                }
                None if ed.testing => {
                    ui.add_space(10.0);
                    ui.label(RichText::new(t!("Teste…")).color(theme::text_dim()));
                }
                None => {}
            }
            if confirming {
                ui.add_space(8.0);
                notice(
                    ui,
                    theme::red(),
                    &t!(
                        "Backup-Server „{}“ entfernen? Die Backups bleiben unberührt.",
                        s.display_name()
                    ),
                );
            }
        },
        |ui, _| {
            let mut out = (false, false);
            ui.add_enabled_ui(can, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Speichern")).clicked() {
                    out.0 = true;
                }
            });
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                out.1 = true;
            }
            ui.add_enabled_ui(can && !testing, |ui| {
                if theme::btn(ui, theme::Btn::Normal, None, t!("Verbindung testen")).clicked() {
                    test = true;
                }
            });
            if !ed.is_new {
                let label = if ed.confirm_delete {
                    t!("Ja, entfernen")
                } else {
                    t!("Entfernen")
                };
                if theme::btn(ui, theme::Btn::Link, Some(Icon::Trash), label).clicked() {
                    if ed.confirm_delete {
                        delete = true;
                    } else {
                        ed.confirm_delete = true;
                    }
                }
            }
            out
        },
    );
    if test {
        ed.testing = true;
        ed.result = None;
        let c = conn_with(&ed.server, ed.secret.clone(), None);
        pb::spawn_test(&app.rt, c, app.pve.cluster.sink.clone());
        return;
    }
    if delete {
        let id = ed.server.id;
        app.pve.cluster.pbs.editor = None;
        apply(app, PbsAction::Delete(id), ctx);
        return;
    }
    if save {
        let ed = app.pve.cluster.pbs.editor.take().expect("editor");
        let mut server = ed.server;
        server.host = server.host.trim().to_string();
        server.user = server.user.trim().to_string();
        let id = server.id;
        match app.lib.pbs_servers.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                if s.host != server.host
                    || s.port != server.port
                    || s.user != server.user
                    || s.tls_pin != server.tls_pin
                {
                    app.pve.cluster.pbs.reset(id);
                }
                *s = server;
            }
            None => app.lib.pbs_servers.push(server),
        }
        if app.keychain.pbs_secrets.get(&id) != Some(&ed.secret) {
            app.keychain.pbs_secrets.insert(id, ed.secret);
            app.pve.cluster.pbs.reset(id);
            app.save_keychain();
        }
        app.save_library();
        refresh(app, id);
    } else if cancel || (dismissed && app.pve.cluster.pbs.trust.is_none()) {
        app.pve.cluster.pbs.editor = None;
    }
}

fn trust(app: &mut App, ctx: &egui::Context) {
    let Some(t) = app.pve.cluster.pbs.trust.as_ref() else {
        return;
    };
    let mut answer = None;
    egui::Modal::new(egui::Id::new("pbs-trust"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(theme::heading(t!("Zertifikat prüfen"), 20.0));
            ui.add(
                egui::Label::new(
                    RichText::new(t!("Der Backup-Server nutzt ein selbst signiertes Zertifikat. Vergleiche den Fingerprint mit dem im Dashboard des Backup-Servers („Fingerprint anzeigen“), bevor du ihm vertraust."))
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
            ui.add_space(8.0);
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::mono(&t.host, 13.0).color(theme::text()));
                ui.label(RichText::new("SHA-256").size(12.5).color(theme::text_dim()));
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(theme::mono(&t.fingerprint, 12.5).color(theme::accent()))
                        .wrap(),
                );
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if theme::btn(ui, theme::Btn::Primary, None, t!("Vertrauen")).clicked() {
                        answer = Some(true);
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    let Some(yes) = answer else {
        return;
    };
    let Some(t) = app.pve.cluster.pbs.trust.take() else {
        return;
    };
    if !yes {
        return;
    }
    match t.target {
        TrustTarget::Editor => {
            if let Some(ed) = app.pve.cluster.pbs.editor.as_mut() {
                ed.server.tls_pin = t.fingerprint;
                ed.testing = true;
                let c = conn_with(&ed.server, ed.secret.clone(), None);
                pb::spawn_test(&app.rt, c, app.pve.cluster.sink.clone());
            }
        }
        TrustTarget::Server(id) => {
            if let Some(s) = app.lib.pbs_servers.iter_mut().find(|s| s.id == id) {
                s.tls_pin = t.fingerprint.clone();
                if let Some(ed) = app
                    .pve
                    .cluster
                    .pbs
                    .editor
                    .as_mut()
                    .filter(|e| e.server.id == id)
                {
                    ed.server.tls_pin = t.fingerprint;
                }
                app.save_library();
                if let Some(l) = app.pve.cluster.pbs.live.get_mut(&id) {
                    l.error = None;
                }
                refresh(app, id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_names() {
        assert_eq!(save_name("qemu-server.conf.blob"), "qemu-server.conf");
        assert_eq!(save_name("../../etc/passwd.blob"), "_.._etc_passwd");
        assert_eq!(save_name(".blob"), "download");
    }
}
