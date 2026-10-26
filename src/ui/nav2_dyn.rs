//! Dynamic folders in the app: background loading of CSV lists, keeping Proxmox folders in
//! sync with the guests of their server, the editor section of the group sheet, the status
//! line and the small "generated" badge. The data and the sync diff are in `nav2_dyn`,
//! the file/URL reading in `backend::dyn_source`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::App;
use crate::backend::Sink;
use crate::backend::proxmox::Ips;
use crate::model::{Host, Library};
use crate::nav2_dyn::{self, DynFolder, DynSource, Gen, SyncStats};
use crate::theme;

/// A Proxmox folder looks at its server this often (one API call) even if the Proxmox page
/// is not open.
const PVE_POLL: Duration = Duration::from_secs(300);
/// How often the Proxmox folders compare their server's data with the hosts.
const TICK: Duration = Duration::from_secs(1);

pub enum DynEvent {
    Loaded {
        folder: Uuid,
        result: Result<(Vec<Gen>, u32), String>,
    },
}

/// What the group editor edits (a copy; applied on save).
#[derive(Clone)]
pub struct Draft {
    pub group: Uuid,
    pub enabled: bool,
    /// 0 Proxmox, 1 rule, 2 list.
    pub kind: usize,
    pub server: Option<Uuid>,
    pub query: String,
    pub location: String,
    pub prune: bool,
    pub on_start: bool,
}

impl Draft {
    fn from(group: Uuid, f: Option<&DynFolder>) -> Draft {
        let mut d = Draft {
            group,
            enabled: f.is_some(),
            kind: 0,
            server: None,
            query: String::new(),
            location: String::new(),
            prune: f.is_some_and(|f| f.prune),
            on_start: f.is_none_or(|f| f.on_start),
        };
        match f.map(|f| &f.source) {
            Some(DynSource::Proxmox { server }) => d.server = Some(*server),
            Some(DynSource::Rule { query }) => {
                d.kind = 1;
                d.query = query.clone();
            }
            Some(DynSource::List { location }) => {
                d.kind = 2;
                d.location = location.clone();
            }
            None => {}
        }
        d
    }

    /// The folder this draft describes, or why it is not usable.
    pub fn to_folder(&self) -> Result<DynFolder, String> {
        let source = match self.kind {
            0 => DynSource::Proxmox {
                server: self
                    .server
                    .ok_or_else(|| t!("Wähle einen Proxmox-Server").to_string())?,
            },
            1 => {
                if crate::nav_query::Query::parse(&self.query).is_empty() {
                    return Err(t!("Die Suchregel ist leer").to_string());
                }
                DynSource::Rule {
                    query: self.query.trim().to_string(),
                }
            }
            _ => {
                nav2_dyn::parse_location(&self.location)?;
                DynSource::List {
                    location: self.location.trim().to_string(),
                }
            }
        };
        Ok(DynFolder {
            source,
            status: Default::default(),
            prune: self.prune,
            on_start: self.on_start,
        })
    }
}

#[derive(Default)]
pub struct DynState {
    io: Option<(Sink<DynEvent>, UnboundedReceiver<DynEvent>)>,
    /// Lists being downloaded or read.
    pub busy: HashSet<Uuid>,
    /// Proxmox folders: signature of the guests the hosts were last synced with.
    sigs: HashMap<Uuid, u64>,
    /// Folders that must sync as soon as their server answers (manual refresh, start).
    force: HashSet<Uuid>,
    started: bool,
    last_tick: Option<Instant>,
    pub draft: Option<Draft>,
}

impl DynState {
    fn sink(&mut self, ctx: &egui::Context) -> Sink<DynEvent> {
        self.io
            .get_or_insert_with(|| Sink::new(ctx.clone()))
            .0
            .clone()
    }
}

// ------------------------------------------------------------------ sync

/// Applies the entries of a source to the folder's hosts and records the outcome.
fn apply_gens(app: &mut App, folder: Uuid, gens: Vec<Gen>, skipped: u32) {
    let Some(f) = app.lib.nav.dynamic.get(&folder).cloned() else {
        return;
    };
    let st: SyncStats = nav2_dyn::sync(&mut app.lib.hosts, folder, &gens, f.prune);
    let changed = st.added + st.removed + st.updated > 0;
    if let Some(m) = app.lib.nav.dynamic.get_mut(&folder) {
        m.status = nav2_dyn::DynStatus {
            last: Some(chrono::Utc::now()),
            added: st.added,
            removed: st.removed,
            updated: st.updated,
            total: st.total,
            skipped: skipped + st.skipped,
            error: String::new(),
        };
    }
    if changed {
        app.save_library();
    } else {
        // Only the status changed: no need to rebuild the host index.
        app.nav.save_due = Some(Instant::now() + Duration::from_millis(800));
    }
}

fn set_error(app: &mut App, folder: Uuid, err: String) {
    if let Some(m) = app.lib.nav.dynamic.get_mut(&folder)
        && m.status.error != err
    {
        m.status.error = err;
        app.nav.save_due = Some(Instant::now() + Duration::from_millis(800));
    }
}

/// "Jetzt abgleichen": asks the source again.
pub fn refresh(app: &mut App, ctx: &egui::Context, folder: Uuid) {
    let Some(f) = app.lib.nav.dynamic.get(&folder).cloned() else {
        return;
    };
    match f.source {
        DynSource::Proxmox { server } => {
            if !app.lib.pve_servers.iter().any(|s| s.id == server) {
                set_error(
                    app,
                    folder,
                    t!("Der Proxmox-Server existiert nicht mehr").to_string(),
                );
                return;
            }
            app.nav.dynst.force.insert(folder);
            crate::ui::proxmox::refresh(app, server);
        }
        DynSource::List { location } => {
            if app.nav.dynst.busy.contains(&folder) {
                return;
            }
            app.nav.dynst.busy.insert(folder);
            let sink = app.nav.dynst.sink(ctx);
            app.rt.spawn_blocking(move || {
                let result = crate::backend::dyn_source::load_list(&location)
                    .and_then(|text| nav2_dyn::parse_list(&text));
                sink.send(DynEvent::Loaded { folder, result });
            });
        }
        DynSource::Rule { .. } => {
            // Nothing to fetch: the folder lists the current matches; stamp the status.
            let total = rule_matches(app, folder);
            if let Some(m) = app.lib.nav.dynamic.get_mut(&folder) {
                m.status = nav2_dyn::DynStatus {
                    last: Some(chrono::Utc::now()),
                    total,
                    ..Default::default()
                };
            }
            app.nav.save_due = Some(Instant::now() + Duration::from_millis(800));
        }
    }
}

/// Hosts a rule folder lists right now (from the navigation cache).
pub fn rule_matches(app: &App, folder: Uuid) -> u32 {
    let Some(c) = app.nav.cache.as_ref() else {
        return 0;
    };
    let Some(&n) = c.idx.by_id.get(&folder) else {
        return 0;
    };
    c.shown.hosts[n]
        .iter()
        .filter(|&&e| c.idx.entries[e as usize].node != Some(n))
        .count() as u32
}

/// Called every frame: results of list loads, the start-up sync and the Proxmox folders.
pub fn pump(app: &mut App, ctx: &egui::Context) {
    if app.vault.is_none() {
        return;
    }
    let mut events = Vec::new();
    if let Some((_, rx)) = app.nav.dynst.io.as_mut() {
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    for e in events {
        match e {
            DynEvent::Loaded { folder, result } => {
                app.nav.dynst.busy.remove(&folder);
                match result {
                    Ok((gens, skipped)) => apply_gens(app, folder, gens, skipped),
                    Err(e) => set_error(app, folder, e),
                }
            }
        }
    }
    if app.lib.nav.dynamic.is_empty() && !app.lib.hosts.iter().any(|h| h.dynamic.is_some()) {
        return;
    }
    if !app.nav.dynst.started {
        app.nav.dynst.started = true;
        heal(&mut app.lib);
        let start: Vec<Uuid> = app
            .lib
            .nav
            .dynamic
            .iter()
            .filter(|(_, f)| f.on_start && f.source.stores_hosts())
            .map(|(g, _)| *g)
            .collect();
        for g in start {
            refresh(app, ctx, g);
        }
    }
    if app.nav.dynst.last_tick.is_some_and(|t| t.elapsed() < TICK) {
        ctx.request_repaint_after(TICK);
        return;
    }
    app.nav.dynst.last_tick = Some(Instant::now());
    pve_tick(app, ctx);
}

/// Proxmox folders: sync when the server's guests (or their addresses) changed.
fn pve_tick(app: &mut App, ctx: &egui::Context) {
    let folders: Vec<(Uuid, Uuid)> = app
        .lib
        .nav
        .dynamic
        .iter()
        .filter_map(|(g, f)| match f.source {
            DynSource::Proxmox { server } => Some((*g, server)),
            _ => None,
        })
        .collect();
    if folders.is_empty() {
        return;
    }
    ctx.request_repaint_after(TICK);
    let mut todo: Vec<(Uuid, Vec<Gen>, u64)> = Vec::new();
    let mut errors: Vec<(Uuid, String)> = Vec::new();
    let mut poll: Vec<Uuid> = Vec::new();
    for (g, server) in &folders {
        let Some(live) = app.pve.live.get(server) else {
            // Never loaded in this run: ask once (the start-up sync does this too).
            poll.push(*server);
            continue;
        };
        if let Some(snap) = &live.snap {
            let ip = |vmid: u32| {
                live.ips.get(&vmid).and_then(|(i, _)| match i {
                    Ips::Found(v) => v.first().cloned(),
                    Ips::NoAgent => None,
                })
            };
            let gens = nav2_dyn::from_proxmox(snap, &ip);
            let sig = nav2_dyn::proxmox_signature(&gens);
            let forced = app.nav.dynst.force.contains(g) && !live.loading;
            if forced || app.nav.dynst.sigs.get(g) != Some(&sig) {
                todo.push((*g, gens, sig));
            }
            // Working again: clear an old error.
            if live.error.is_none()
                && app
                    .lib
                    .nav
                    .dynamic
                    .get(g)
                    .is_some_and(|f| !f.status.error.is_empty())
            {
                errors.push((*g, String::new()));
            }
        }
        if let Some(e) = &live.error
            && !live.loading
        {
            errors.push((*g, e.to_string()));
        }
        if !live.loading
            && live.last.is_none_or(|t| t.elapsed() >= PVE_POLL)
            && !matches!(
                live.error,
                Some(
                    crate::backend::proxmox::Error::Untrusted(_)
                        | crate::backend::proxmox::Error::PinChanged { .. }
                )
            )
        {
            poll.push(*server);
        }
    }
    for (g, gens, sig) in todo {
        app.nav.dynst.force.remove(&g);
        app.nav.dynst.sigs.insert(g, sig);
        apply_gens(app, g, gens, 0);
    }
    for (g, e) in errors {
        set_error(app, g, e);
    }
    poll.sort();
    poll.dedup();
    for s in poll {
        crate::ui::proxmox::refresh(app, s);
    }
}

/// Generated hosts whose folder is gone (or no longer dynamic) become normal hosts.
pub fn heal(lib: &mut Library) {
    let ok: HashSet<Uuid> = lib
        .nav
        .dynamic
        .iter()
        .filter(|(g, f)| f.source.stores_hosts() && lib.groups.iter().any(|x| x.id == **g))
        .map(|(g, _)| *g)
        .collect();
    for h in &mut lib.hosts {
        if h.dynamic.as_ref().is_some_and(|d| !ok.contains(&d.folder)) {
            h.dynamic = None;
        }
    }
}

/// Deleting a dynamic folder deletes the entries it generated (they are not user data).
pub fn before_delete_group(lib: &mut Library, group: Uuid) {
    lib.hosts
        .retain(|h| !h.dynamic.as_ref().is_some_and(|d| d.folder == group));
}

/// May hosts be dropped into / created inside this group by hand?
pub fn accepts_hosts(lib: &Library, group: Uuid) -> bool {
    !lib.nav.dynamic.contains_key(&group)
}

/// Generated hosts stay where the source put them.
pub fn movable(h: &Host) -> bool {
    h.dynamic.is_none()
}

// ------------------------------------------------------------------ editor

pub struct EditorOut {
    /// "Jetzt abgleichen" was clicked (apply the draft first).
    pub refresh: bool,
    /// The draft can be saved.
    pub valid: bool,
}

fn status_line(f: &DynFolder, busy: bool) -> (String, Color32) {
    if busy {
        return (t!("Wird geladen …").to_string(), theme::text_dim());
    }
    if !f.status.error.is_empty() {
        return (t!("Fehler: {}", f.status.error.as_str()), theme::red());
    }
    let Some(t) = f.status.last else {
        return (t!("Noch nicht abgeglichen").to_string(), theme::text_dim());
    };
    let when = crate::ui::relative_time(t);
    let s = &f.status;
    let text = match f.source {
        DynSource::Rule { .. } => t!("Zuletzt {} · {} Treffer", when, s.total),
        _ => {
            let mut text = t!(
                "Zuletzt {} · {} neu · {} entfernt · {} geändert · {} Einträge",
                when,
                s.added,
                s.removed,
                s.updated,
                s.total
            );
            if s.skipped > 0 {
                text.push_str(&t!(" · {} übersprungen", s.skipped));
            }
            text
        }
    };
    (text, theme::text_dim())
}

/// The "Dynamischer Ordner" part of the group editor.
pub fn editor_section(
    ui: &mut egui::Ui,
    st: &mut DynState,
    group: Uuid,
    is_new: bool,
    lib: &Library,
) -> EditorOut {
    if st.draft.as_ref().is_none_or(|d| d.group != group) {
        st.draft = Some(Draft::from(group, lib.nav.dynamic.get(&group)));
    }
    let busy = st.busy.contains(&group);
    let Some(d) = st.draft.as_mut() else {
        return EditorOut {
            refresh: false,
            valid: true,
        };
    };
    let mut refresh = false;
    theme::form_caption(ui, t!("Dynamischer Ordner"));
    theme::form(ui, 140.0, |ui, f| {
        f.switch(ui, t!("Hosts aus einer Quelle"), &mut d.enabled);
        if !d.enabled {
            return;
        }
        f.field(ui, t!("Quelle"), |ui| {
            let labels = [t!("Proxmox"), t!("Suchregel"), t!("Liste")];
            if let Some(i) = theme::segmented(ui, &labels, d.kind) {
                d.kind = i;
            }
        });
        match d.kind {
            0 => f.field(ui, t!("Server"), |ui| {
                let sel = d
                    .server
                    .and_then(|id| lib.pve_servers.iter().find(|s| s.id == id))
                    .map(|s| s.display_name().to_string())
                    .unwrap_or_else(|| t!("— wählen —").to_string());
                egui::ComboBox::from_id_salt("dyn-pve")
                    .selected_text(sel)
                    .width((ui.available_width() - 14.0).max(140.0))
                    .show_ui(ui, |ui| {
                        for s in &lib.pve_servers {
                            ui.selectable_value(&mut d.server, Some(s.id), s.display_name());
                        }
                    });
            }),
            1 => f.field(ui, t!("Suchregel"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.query)
                        .hint_text(RichText::new("type:switch customer/*").color(theme::text_dim()))
                        .desired_width(f32::INFINITY),
                );
            }),
            _ => f.field(ui, t!("Datei oder URL"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.location)
                        .hint_text(
                            RichText::new("https://…/liste.csv  oder  /pfad/liste.csv")
                                .color(theme::text_dim()),
                        )
                        .desired_width(f32::INFINITY),
                );
            }),
        }
        if d.kind == 2 {
            f.switch(ui, t!("Beim Start abgleichen"), &mut d.on_start);
        }
        if d.kind != 1 {
            f.switch(ui, t!("Verschwundene Einträge löschen"), &mut d.prune);
        }
    });
    let mut valid = true;
    if d.enabled {
        let hint = match d.kind {
            0 => t!(
                "Alle VMs und Container des Servers erscheinen als Hosts in dieser Gruppe und werden beim Aktualisieren abgeglichen. Name, Adresse und Protokoll kommen aus Proxmox; Tags, Notizen und Zugangsdaten kannst du ergänzen. Beim Wechsel der Quelle werden die erzeugten Einträge ersetzt."
            ),
            1 => t!(
                "Eine Smart-Gruppe: sie zeigt alle Hosts, auf die die Suche passt (z. B. type:switch customer/*), ohne sie zu kopieren."
            ),
            _ => t!(
                "CSV mit den Spalten name, host, port, protocol, user, group, tags, description (Kopfzeile optional). Nur https:// oder ein Dateipfad, höchstens 5 MB, keine Zugangsdaten in der Adresse. Beim Wechsel der Quelle werden die erzeugten Einträge ersetzt."
            ),
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.add(
                egui::Label::new(RichText::new(hint).size(12.0).color(theme::text_dim())).wrap(),
            );
        });
        if let Err(e) = d.to_folder() {
            valid = false;
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(e).size(12.5).color(theme::red()));
            });
        }
        if let Some(f) = lib.nav.dynamic.get(&group) {
            ui.add_space(6.0);
            let (text, color) = status_line(f, busy);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(text).size(12.5).color(color));
            });
        }
        if !is_new && d.kind != 1 {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let ok = valid && !busy;
                ui.add_enabled_ui(ok, |ui| {
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Jetzt abgleichen")).clicked() {
                        refresh = true;
                    }
                });
            });
        }
    }
    EditorOut { refresh, valid }
}

/// Applies the draft of the group editor (on save, or before a manual refresh).
pub fn commit(app: &mut App, ctx: &egui::Context, group: Uuid, refresh_now: bool) {
    let Some(d) = app.nav.dynst.draft.clone().filter(|d| d.group == group) else {
        return;
    };
    if !d.enabled {
        if app.lib.nav.dynamic.remove(&group).is_some() {
            // The entries stay as normal hosts (heal clears their marks).
            heal(&mut app.lib);
            app.save_library();
        }
        return;
    }
    let Ok(mut f) = d.to_folder() else { return };
    let old = app.lib.nav.dynamic.get(&group).cloned();
    let source_changed = old.as_ref().is_none_or(|o| o.source != f.source);
    if let Some(o) = &old
        && !source_changed
    {
        f.status = o.status.clone();
    }
    if source_changed {
        // The old entries belong to another source: replace them.
        before_delete_group(&mut app.lib, group);
        app.nav.dynst.sigs.remove(&group);
    }
    if old.as_ref() != Some(&f) {
        app.lib.nav.dynamic.insert(group, f);
        app.save_library();
    }
    if source_changed || refresh_now {
        refresh(app, ctx, group);
    }
}

/// Called after the group editor was shown: save/cancel/refresh of the dynamic part.
pub fn after_editor(
    app: &mut App,
    ctx: &egui::Context,
    group: Option<Uuid>,
    saved: bool,
    cancelled: bool,
    refresh_now: bool,
) {
    let Some(group) = group else { return };
    if cancelled {
        app.nav.dynst.draft = None;
    } else if saved {
        commit(app, ctx, group, false);
        app.nav.dynst.draft = None;
    } else if refresh_now {
        commit(app, ctx, group, true);
    }
}

/// After saving the host editor: what the source generated is taken from the stored host,
/// whatever the form says.
pub fn keep_generated(old: &Host, edited: &mut Host) {
    if old.dynamic.is_none() {
        return;
    }
    edited.label = old.label.clone();
    edited.hostname = old.hostname.clone();
    edited.port = old.port;
    edited.protocol = old.protocol;
    edited.group_id = old.group_id;
    edited.web.url = old.web.url.clone();
    edited.dynamic = old.dynamic.clone();
}

/// Banner at the top of the host editor for a generated entry.
pub fn locked_notice(ui: &mut egui::Ui, lib: &Library, h: &Host) {
    let Some(d) = &h.dynamic else { return };
    let folder = lib.group(d.folder).map(|g| g.name.as_str()).unwrap_or("?");
    let text = if d.gone {
        t!(
            "Erzeugt aus „{}“ – die Quelle führt diesen Eintrag nicht mehr. Tags, Notizen und Zugangsdaten bleiben bearbeitbar.",
            folder
        )
    } else {
        t!(
            "Erzeugt aus „{}“: Name, Adresse, Protokoll und Gruppe kommen aus der Quelle. Tags, Notizen und Zugangsdaten kannst du ergänzen.",
            folder
        )
    };
    egui::Frame::NONE
        .fill(theme::accent_soft())
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::text())).wrap());
        });
    ui.add_space(10.0);
}

// ------------------------------------------------------------------ badges

/// Short name of a folder's source for badges and tooltips.
pub fn source_label(f: &DynFolder) -> &'static str {
    match f.source {
        DynSource::Proxmox { .. } => "Proxmox",
        DynSource::Rule { .. } => "Regel",
        DynSource::List { .. } => "Liste",
    }
}

/// Small badge in a host row for generated entries; returns the new x position.
pub fn host_badge(
    p: &egui::Painter,
    ui: &egui::Ui,
    lib: &Library,
    h: &Host,
    x: f32,
    y: f32,
    right: f32,
) -> f32 {
    let Some(d) = &h.dynamic else { return x };
    let (text, color) = if d.gone {
        (t!("entfernt").to_string(), theme::amber())
    } else {
        (
            crate::i18n::tr(
                lib.nav
                    .dynamic
                    .get(&d.folder)
                    .map(source_label)
                    .unwrap_or("Quelle"),
            )
            .to_string(),
            theme::text_dim(),
        )
    };
    let g = p.layout_no_wrap(text, theme::semibold(10.5), color);
    let w = g.size().x + 12.0;
    if x + w > right {
        return x;
    }
    let r = Rect::from_min_size(Pos2::new(x, y - 8.0), Vec2::new(w, 16.0));
    p.rect_filled(r, CornerRadius::same(5), theme::soft(color));
    p.galley(r.center() - g.size() / 2.0, g, color);
    let tip = if d.gone {
        t!("Dieser Eintrag ist in der Quelle nicht mehr vorhanden")
    } else {
        t!(
            "Automatisch erzeugt – Name, Adresse und Protokoll kommen aus der Quelle; Tags, Notizen und Zugangsdaten kannst du ergänzen"
        )
    };
    ui.interact(r, ui.id().with(("dyn-badge", h.id)), egui::Sense::hover())
        .on_hover_text(tip);
    x + w + 6.0
}

/// Paints a badge for a dynamic group header; returns the new x position.
pub fn group_badge(p: &egui::Painter, f: &DynFolder, x: f32, y: f32, right: f32) -> f32 {
    let text = crate::i18n::tr(source_label(f)).to_string();
    let color = if f.status.error.is_empty() {
        theme::accent()
    } else {
        theme::red()
    };
    let g = p.layout_no_wrap(text, theme::semibold(10.5), color);
    let w = g.size().x + 12.0;
    if x + w > right {
        return x;
    }
    let r = Rect::from_min_size(Pos2::new(x, y - 8.0), Vec2::new(w, 16.0));
    p.rect_filled(r, CornerRadius::same(5), theme::soft(color));
    p.galley(r.center() - g.size() / 2.0, g, color);
    x + w + 6.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_round_trip_and_validation() {
        let g = Uuid::new_v4();
        let mut d = Draft::from(g, None);
        assert!(!d.enabled && d.on_start);
        d.kind = 0;
        assert!(d.to_folder().is_err(), "no server chosen");
        d.server = Some(Uuid::new_v4());
        assert!(d.to_folder().is_ok());
        d.kind = 1;
        d.query = "  ".into();
        assert!(d.to_folder().is_err());
        d.query = " type:switch customer/* ".into();
        let f = d.to_folder().unwrap();
        assert_eq!(
            f.source,
            DynSource::Rule {
                query: "type:switch customer/*".into()
            }
        );
        let back = Draft::from(g, Some(&f));
        assert!(back.enabled && back.kind == 1 && back.query == "type:switch customer/*");
        d.kind = 2;
        d.location = "http://x/y.csv".into();
        assert!(d.to_folder().is_err());
        d.location = "https://example.org/a.csv".into();
        assert!(d.to_folder().is_ok());
    }

    #[test]
    fn heal_clears_marks_of_missing_folders() {
        let mut lib = Library::default();
        let g = crate::model::Group {
            name: "dyn".into(),
            ..Default::default()
        };
        let gid = g.id;
        lib.groups.push(g);
        lib.nav.dynamic.insert(
            gid,
            DynFolder {
                source: DynSource::List {
                    location: "/x".into(),
                },
                ..Default::default()
            },
        );
        let mark = |folder| crate::nav2_dyn::DynMark {
            folder,
            key: "k".into(),
            ..Default::default()
        };
        lib.hosts.push(Host {
            dynamic: Some(mark(gid)),
            ..Default::default()
        });
        lib.hosts.push(Host {
            dynamic: Some(mark(Uuid::new_v4())),
            ..Default::default()
        });
        heal(&mut lib);
        assert!(lib.hosts[0].dynamic.is_some());
        assert!(lib.hosts[1].dynamic.is_none());
        // A rule folder generates nothing: marks pointing at it are cleared too.
        lib.nav.dynamic.insert(
            gid,
            DynFolder {
                source: DynSource::Rule { query: "x".into() },
                ..Default::default()
            },
        );
        heal(&mut lib);
        assert!(lib.hosts[0].dynamic.is_none());
        // Deleting a folder deletes what it generated.
        lib.hosts[0].dynamic = Some(mark(gid));
        before_delete_group(&mut lib, gid);
        assert_eq!(lib.hosts.len(), 1);
    }

    #[test]
    fn status_lines() {
        let mut f = DynFolder::default();
        assert!(status_line(&f, false).0.contains("Noch nicht"));
        f.status.last = Some(chrono::Utc::now());
        f.source = DynSource::List {
            location: "x".into(),
        };
        f.status.added = 2;
        f.status.total = 5;
        let (t, _) = status_line(&f, false);
        assert!(t.contains("2 neu") && t.contains("5 Einträge"));
        f.status.error = "boom".into();
        assert!(status_line(&f, false).0.contains("boom"));
        assert!(status_line(&f, true).0.contains("geladen"));
    }
}
