//! Runtime state of the large-inventory navigation: the cached index/rows of the host
//! list (rebuilt only when something changed), expansion, scope, saved views and the
//! actions that change them. Drawing lives in `nav_list`, `nav_table` and `nav_rail`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::app::{Action, App, View};
use crate::backend::reach::Reach;
use crate::model::{Env, Host};
use crate::nav_model::{self, SavedView, ViewMode};
use crate::nav_query::{Query, Status};
use crate::nav_tree::{self, FlattenOpts, Heights, Index, Layout, Shown, Sort};

/// Expand/collapse toggles are written to disk after this much quiet (the library of a
/// big inventory is megabytes; one write per click would stall the list).
const SAVE_DELAY: Duration = Duration::from_millis(1500);
/// Host states are re-read at most this often.
const STATUS_EVERY: Duration = Duration::from_secs(1);
/// Below this many hits a search/filter opens every group that has hits.
pub const AUTO_OPEN_MAX: usize = 300;

pub const ROW_H: f32 = 66.0;
pub const ROW_H_COMPACT: f32 = 44.0;

/// The row the keyboard cursor stands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Group(Uuid),
    Host(Uuid),
}

#[derive(Clone, Debug)]
pub enum NavAction {
    /// "Only this customer" (`None` = everything).
    Scope(Option<Uuid>),
    /// Expand (true) or collapse (false) every group of the current scope.
    ExpandAll(bool),
    /// Expand the group's ancestors and the group itself and scroll to it.
    Reveal(Uuid),
    /// Show a host in the tree (clears the search) and put the cursor on it.
    RevealHost(Uuid),
    SetMode(ViewMode),
    PinGroup(Uuid, bool),
    SetBadge(Uuid, String),
    SaveView(String),
    ApplyView(Uuid),
    DeleteView(Uuid),
    MoveView(Uuid, i32),
    /// Add (true) or remove (false) a tag on these hosts.
    Tag(Vec<Uuid>, String, bool),
    Table(crate::ui::nav_table::TableOp),
}

/// Everything the list needs to draw a frame without touching all hosts.
pub struct Cache {
    pub idx: Index,
    idx_stamp: (u64, usize, usize),
    status_stamp: (u64, usize, usize, bool),
    status_at: Instant,
    /// Bumped whenever `shown` was rebuilt.
    pub epoch: u64,
    shown_key: ShownKey,
    pub shown: Shown,
    pub query: Query,
    /// Search results for free-text queries (a flat ranked list instead of the tree).
    pub flat: Option<Vec<u32>>,
    /// A search/filter with few hits opens its groups.
    pub auto_open: bool,
    pub filtering: bool,
    pub layout: Layout,
    layout_key: (u64, u64, bool, bool),
    /// Hosts in the order they are listed (Shift+click ranges).
    pub host_ids: Vec<Uuid>,
    pub recent: Vec<u32>,
    pub pinned: Vec<u32>,
    pub scope_node: Option<usize>,
}

#[derive(Clone, PartialEq)]
struct ShownKey {
    idx_epoch: u64,
    status_epoch: u64,
    text: String,
    filter: usize,
    sort: u8,
    scope: Option<Uuid>,
}

#[derive(Default)]
pub struct NavState {
    /// Library revision: bumped by every `save_library`.
    pub rev: u64,
    /// The next `save_library` came from a view-only change (no rebuild needed).
    pub skip_rev: bool,
    pub cache: Option<Cache>,
    idx_epoch: u64,
    status_epoch: u64,
    pub exp_epoch: u64,
    /// Open/closed groups while a search is active (not stored).
    pub overrides: HashMap<Uuid, bool>,
    pub cursor: Option<Cursor>,
    pub scroll_to: Option<Cursor>,
    pub save_due: Option<Instant>,
    /// "Save this view" sheet: the name being typed.
    pub view_name: Option<String>,
    /// Badge editor of a group: (group, text).
    pub badge: Option<(Uuid, String)>,
    /// Put the keyboard focus into the search field next frame.
    pub focus_search: bool,
    /// The search text the overrides belong to.
    override_for: String,
    pub table: crate::ui::nav_table::TableCache,
    pub pal: PaletteCache,
    /// Which order was last handed to Shift+click selection (0 none, 1 list, 2 table).
    pub order_for: u8,
    /// Scroll offset and height of the list viewport (last frame).
    pub recent_hosts: Vec<Uuid>,
    pub scroll_y: f32,
    pub view_h: f32,
}

/// Memo of the palette's host search.
#[derive(Default)]
pub struct PaletteCache {
    key: Option<(String, u64, u64, Option<Uuid>)>,
    hits: Vec<usize>,
}

impl NavState {
    /// Marks the library as changed (called from `App::save_library`).
    pub fn bump(&mut self) {
        if std::mem::take(&mut self.skip_rev) {
            return;
        }
        self.rev += 1;
    }
}

fn is_open(nav: &NavState, lib: &crate::model::Library, auto: bool, id: Uuid, node: usize) -> bool {
    nav.overrides.get(&id).copied().unwrap_or(if auto {
        true
    } else {
        !lib.groups[node].collapsed
    })
}

/// Hosts that have a maintenance window right now (directly or through their group).
fn maintenance_hosts(lib: &crate::model::Library) -> HashSet<Uuid> {
    let now = chrono::Utc::now();
    let mut out = HashSet::new();
    for m in lib.maintenance.iter().filter(|m| m.is_active(now)) {
        if let Some(h) = m.host {
            out.insert(h);
        }
        if let Some(g) = m.group {
            let groups: HashSet<Uuid> = lib
                .groups
                .iter()
                .filter(|x| lib.is_descendant(x.id, g))
                .map(|x| x.id)
                .collect();
            out.extend(
                lib.hosts
                    .iter()
                    .filter(|h| h.group_id.is_some_and(|x| groups.contains(&x)))
                    .map(|h| h.id),
            );
        }
    }
    out
}

/// Reachability of a host as the list shows it (same rules as the row's LED).
fn make_status<'a>(
    app: &'a App,
    live: &'a HashSet<String>,
) -> impl FnMut(&Host) -> (Status, bool) + 'a {
    let reach: HashMap<Uuid, Reach> = if app.lib.settings.reachability {
        app.reach.all()
    } else {
        HashMap::new()
    };
    let maint = maintenance_hosts(&app.lib);
    move |h| {
        let is_live = !live.is_empty()
            && (live.contains(&h.id.to_string()) || live.contains(h.display_name()));
        let st = if maint.contains(&h.id) && !is_live {
            Status::Maint
        } else if app.failed.contains(&h.id) && !is_live {
            Status::Failed
        } else {
            match reach.get(&h.id) {
                Some(Reach::Up(_)) => Status::Up,
                Some(Reach::Down) => Status::Down,
                None => Status::Unknown,
            }
        };
        (st, is_live)
    }
}

/// Is this group open in the list right now?
pub fn is_open_node(app: &App, c: &Cache, node: usize) -> bool {
    is_open(&app.nav, &app.lib, c.auto_open, c.idx.nodes[node].id, node)
}

pub fn host_heights(compact: bool) -> Heights {
    Heights {
        group_root: 52.0,
        group: 44.0,
        host: if compact { ROW_H_COMPACT } else { ROW_H },
        loose: 44.0,
    }
}

/// Brings the cache up to date for this frame (cheap when nothing changed).
pub fn ensure(app: &mut App, ctx: &egui::Context, drag: bool) {
    let mut nav = std::mem::take(&mut app.nav);
    ensure_inner(&mut nav, app, ctx, drag);
    app.nav = nav;
}

fn ensure_inner(nav: &mut NavState, app: &App, ctx: &egui::Context, drag: bool) {
    let lib = &app.lib;
    let live: HashSet<String> = crate::ui::hosts::live_hosts(app).into_iter().collect();
    let idx_stamp = (nav.rev, lib.hosts.len(), lib.groups.len());
    let reach_on = lib.settings.reachability;
    let status_stamp = (
        if reach_on { app.reach.rev() } else { 0 },
        app.failed.len(),
        live.len(),
        reach_on,
    );
    let text = app.search.trim().to_string();
    let compact = lib.settings.compact;

    let stale = nav.cache.as_ref().is_none_or(|c| c.idx_stamp != idx_stamp);
    if stale {
        let idx = Index::build(lib, &mut make_status(app, &live));
        nav.idx_epoch += 1;
        let empty = Layout {
            rows: Vec::new(),
            y: vec![0.0],
        };
        nav.cache = Some(Cache {
            idx,
            idx_stamp,
            status_stamp,
            status_at: Instant::now(),
            epoch: 0,
            shown_key: ShownKey {
                idx_epoch: 0,
                status_epoch: u64::MAX,
                text: String::new(),
                filter: 0,
                sort: 0,
                scope: None,
            },
            shown: Shown {
                scope: None,
                hosts: Vec::new(),
                total: Vec::new(),
                roll: Vec::new(),
                loose: Vec::new(),
                order: Vec::new(),
            },
            query: Query::default(),
            flat: None,
            auto_open: false,
            filtering: false,
            layout: empty,
            layout_key: (u64::MAX, 0, false, false),
            host_ids: Vec::new(),
            recent: Vec::new(),
            pinned: Vec::new(),
            scope_node: None,
        });
        nav.table.invalidate();
        let mut last: Vec<(i64, Uuid)> = lib
            .hosts
            .iter()
            .filter_map(|h| h.last_connected.map(|t| (t.timestamp(), h.id)))
            .collect();
        last.sort_by_key(|x| std::cmp::Reverse(x.0));
        nav.recent_hosts = last.into_iter().take(5).map(|x| x.1).collect();
    } else if let Some(c) = nav.cache.as_mut()
        && c.status_stamp != status_stamp
    {
        let since = c.status_at.elapsed();
        // Big libraries re-read the states less often (20 000 hosts: every 5 s).
        let every = STATUS_EVERY + Duration::from_millis(lib.hosts.len() as u64 / 5);
        if since >= every {
            c.idx.refresh_status(lib, &mut make_status(app, &live));
            c.status_stamp = status_stamp;
            c.status_at = Instant::now();
            nav.status_epoch += 1;
        } else {
            ctx.request_repaint_after(every - since);
        }
    }
    let Some(c) = nav.cache.as_mut() else { return };

    // Overrides belong to one search text.
    if nav.override_for != text {
        nav.overrides.clear();
        nav.override_for = text.clone();
        nav.exp_epoch += 1;
    }

    let scope = lib.nav.scope.filter(|s| c.idx.by_id.contains_key(s));
    let key = ShownKey {
        idx_epoch: nav.idx_epoch,
        status_epoch: nav.status_epoch,
        text: text.clone(),
        filter: app.host_filter,
        sort: lib.settings.sort,
        scope,
    };
    if c.shown_key != key {
        let query = Query::parse(&text);
        let scope_node = scope.and_then(|s| c.idx.by_id.get(&s).copied());
        let filter = app.host_filter;
        let idx = &c.idx;
        let keep = |e: usize| {
            let en = &idx.entries[e];
            crate::ui::hosts::passes_filter(filter, &lib.hosts[en.host])
                && (query.conds.is_empty() || query.matches(&idx.facets(lib, e)))
        };
        let sort = Sort::from_u8(lib.settings.sort);
        c.shown = nav_tree::compute(idx, &keep, scope_node, sort);
        c.flat = (!query.words.is_empty()).then(|| nav_tree::rank(idx, &query, &keep, scope_node));
        c.filtering = !query.is_empty() || filter != 0;
        c.auto_open = c.filtering && c.shown.order.len() <= AUTO_OPEN_MAX;
        c.query = query;
        c.scope_node = scope_node;
        // Quick access: most recently used and pinned hosts of the (unfiltered) scope.
        c.recent = Vec::new();
        c.pinned = Vec::new();
        if !c.filtering {
            let mut recent: Vec<u32> = c
                .shown
                .order
                .iter()
                .copied()
                .filter(|e| idx.entries[*e as usize].last > 0)
                .collect();
            let k = 5.min(recent.len());
            if k > 0 {
                recent.select_nth_unstable_by_key(k - 1, |e| {
                    std::cmp::Reverse(idx.entries[*e as usize].last)
                });
                recent.truncate(k);
                recent.sort_by_key(|e| std::cmp::Reverse(idx.entries[*e as usize].last));
            }
            c.recent = recent;
            c.pinned = c
                .shown
                .order
                .iter()
                .copied()
                .filter(|e| lib.hosts[idx.entries[*e as usize].host].favorite)
                .take(12)
                .collect();
        }
        c.shown_key = key;
        c.epoch += 1;
    }

    let lkey = (c.epoch, nav.exp_epoch, drag && !c.filtering, compact);
    if c.layout_key != lkey {
        let heights = host_heights(compact);
        c.layout = match &c.flat {
            Some(flat) => nav_tree::flat_layout(flat, heights.host),
            None => {
                let ov = &nav.overrides;
                let auto = c.auto_open;
                let nodes = &c.idx.nodes;
                let open = |n: usize| {
                    ov.get(&nodes[n].id).copied().unwrap_or(if auto {
                        true
                    } else {
                        !lib.groups[n].collapsed
                    })
                };
                nav_tree::flatten(
                    &c.idx,
                    &c.shown,
                    &FlattenOpts {
                        filtering: c.filtering,
                        drag: lkey.2,
                        heights,
                        is_open: &open,
                    },
                )
            }
        };
        c.host_ids = c
            .layout
            .rows
            .iter()
            .filter_map(|r| match r.kind {
                nav_tree::RowKind::Host(e) => Some(lib.hosts[c.idx.entries[e as usize].host].id),
                _ => None,
            })
            .collect();
        c.layout_key = lkey;
        nav.order_for = 0;
    }
    if nav.order_for != 1 && nav.table.mode_is_list(lib) {
        let ids = c.host_ids.clone();
        ctx.data_mut(|d| d.insert_temp(crate::ui::hosts::order_id(), ids));
        nav.order_for = 1;
    }
}

/// Flip a group open/closed. Outside a search the choice is stored in the library
/// (written after a short pause); during a search it only lasts for that search.
pub fn toggle_group(app: &mut App, id: Uuid) {
    let searching = app.nav.cache.as_ref().map(|c| c.filtering).unwrap_or(false);
    let Some(i) = app.lib.groups.iter().position(|g| g.id == id) else {
        return;
    };
    if searching {
        let auto = app.nav.cache.as_ref().is_some_and(|c| c.auto_open);
        let now = is_open(&app.nav, &app.lib, auto, id, i);
        app.nav.overrides.insert(id, !now);
    } else {
        let g = &mut app.lib.groups[i];
        g.collapsed = !g.collapsed;
        app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
    }
    app.nav.exp_epoch += 1;
}

/// Opens (true) or closes (false) all groups below the scope.
fn set_all(app: &mut App, open: bool) {
    let searching = app.nav.cache.as_ref().map(|c| c.filtering).unwrap_or(false);
    let scope = app.lib.nav.scope;
    let ids: Vec<Uuid> = app
        .lib
        .groups
        .iter()
        .filter(|g| scope.is_none_or(|s| app.lib.is_descendant(g.id, s) && g.id != s))
        .map(|g| g.id)
        .collect();
    if searching {
        for id in ids {
            app.nav.overrides.insert(id, open);
        }
    } else {
        for g in app.lib.groups.iter_mut().filter(|g| ids.contains(&g.id)) {
            g.collapsed = !open;
        }
        app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
    }
    app.nav.exp_epoch += 1;
}

/// Opens the group and everything above it.
fn open_chain(app: &mut App, group: Uuid) {
    let mut cur = Some(group);
    let mut guard = 0;
    while let Some(id) = cur {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Some(g) = app.lib.groups.iter_mut().find(|g| g.id == id) else {
            break;
        };
        g.collapsed = false;
        app.nav.overrides.insert(id, true);
        cur = g.parent_id;
    }
    app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
    app.nav.exp_epoch += 1;
}

pub fn set_scope(app: &mut App, scope: Option<Uuid>) {
    app.lib.nav.scope = scope;
    if let Some(g) = scope {
        app.lib.nav.touch_group(g);
    }
    // The selection only holds hosts that are still in view.
    let lib = &app.lib;
    let keep: HashSet<Uuid> = app
        .selected_hosts
        .iter()
        .copied()
        .filter(|id| {
            lib.host(*id)
                .is_some_and(|h| in_scope(lib, h.group_id, scope))
        })
        .collect();
    app.selected_hosts = keep;
    app.nav.cursor = None;
    app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
}

/// Is a host of this group inside the scope? `None` scope = everything.
pub fn in_scope(lib: &crate::model::Library, group: Option<Uuid>, scope: Option<Uuid>) -> bool {
    match scope {
        None => true,
        Some(s) => group.is_some_and(|g| lib.is_descendant(g, s)),
    }
}

/// Name of the scope group for chips (None = not scoped or group gone).
pub fn scope_name(lib: &crate::model::Library) -> Option<&str> {
    lib.nav
        .scope
        .and_then(|s| lib.group(s))
        .map(|g| g.name.as_str())
}

/// Badge text of a group: the stored letters or the first letter of its name.
pub fn badge_text(lib: &crate::model::Library, g: &crate::model::Group) -> String {
    match lib
        .nav
        .meta(g.id)
        .map(|m| m.letter.trim())
        .filter(|l| !l.is_empty())
    {
        Some(l) => l.to_uppercase(),
        None => g
            .name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into()),
    }
}

/// Palette: hosts matching the query inside the scope, best first (memoised per
/// keystroke). With an empty query the most recently used hosts.
pub fn palette_hosts(app: &mut App, ctx: &egui::Context, query: &str, limit: usize) -> Vec<usize> {
    ensure(app, ctx, false);
    let key = (
        query.trim().to_string(),
        app.nav.idx_epoch,
        app.nav.status_epoch,
        app.lib.nav.scope,
    );
    if app.nav.pal.key.as_ref() == Some(&key) {
        return app.nav.pal.hits.clone();
    }
    let lib = &app.lib;
    let Some(c) = app.nav.cache.as_ref() else {
        return Vec::new();
    };
    let scope = lib.nav.scope.and_then(|s| c.idx.by_id.get(&s).copied());
    let q = Query::parse(&key.0);
    let mut hits: Vec<(i32, i64, u32)> = Vec::new();
    for &e in &c.idx.by_name {
        let en = &c.idx.entries[e as usize];
        if let Some(s) = scope
            && !en.node.is_some_and(|n| c.idx.under(n, s))
        {
            continue;
        }
        if !q.conds.is_empty() && !q.matches(&c.idx.facets(lib, e as usize)) {
            continue;
        }
        if let Some(score) = q.score(&en.hay) {
            hits.push((score, en.last, e));
        }
    }
    if q.words.is_empty() {
        hits.sort_by_key(|h| std::cmp::Reverse(h.1));
    } else {
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    }
    hits.truncate(limit);
    let ids: Vec<usize> = hits
        .into_iter()
        .map(|(_, _, e)| c.idx.entries[e as usize].host)
        .collect();
    app.nav.pal.key = Some(key);
    app.nav.pal.hits = ids.clone();
    ids
}

/// Writes view-only changes (expansion, scope, view mode) once the user paused.
pub fn flush(app: &mut App, ctx: &egui::Context) {
    match app.nav.save_due {
        Some(t) if t <= Instant::now() => {
            app.nav.save_due = None;
            app.nav.skip_rev = true;
            app.save_library();
        }
        Some(t) => ctx.request_repaint_after(t.saturating_duration_since(Instant::now())),
        None => {}
    }
}

fn current_view(app: &App, name: &str) -> SavedView {
    SavedView {
        id: Uuid::new_v4(),
        name: name.to_string(),
        query: app.search.trim().to_string(),
        filter: app.host_filter,
        sort: app.lib.settings.sort,
        scope: app.lib.nav.scope,
        table: app.lib.nav.mode(app.lib.nav.scope) == ViewMode::Table,
    }
}

pub fn apply(app: &mut App, action: NavAction, ctx: &egui::Context) {
    match action {
        NavAction::Scope(s) => {
            set_scope(app, s);
            app.view = View::Hosts;
        }
        NavAction::ExpandAll(open) => set_all(app, open),
        NavAction::Reveal(g) => {
            app.view = View::Hosts;
            app.active = None;
            // A search or filter would hide the group again.
            app.search.clear();
            app.host_filter = 0;
            // Outside the customer scope the target would stay invisible: leave the scope.
            if let Some(sc) = app.lib.nav.scope
                && !in_scope(&app.lib, Some(g), Some(sc))
            {
                set_scope(app, None);
            }
            open_chain(app, g);
            app.lib.nav.touch_group(g);
            app.nav.cursor = Some(Cursor::Group(g));
            app.nav.scroll_to = Some(Cursor::Group(g));
            app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
        }
        NavAction::RevealHost(h) => {
            app.view = View::Hosts;
            app.active = None;
            app.search.clear();
            app.host_filter = 0;
            let group = app.lib.host(h).and_then(|h| h.group_id);
            if let Some(sc) = app.lib.nav.scope
                && !in_scope(&app.lib, group, Some(sc))
            {
                set_scope(app, None);
            }
            if let Some(g) = group {
                open_chain(app, g);
            }
            app.nav.cursor = Some(Cursor::Host(h));
            app.nav.scroll_to = Some(Cursor::Host(h));
        }
        NavAction::SetMode(m) => {
            let scope = app.lib.nav.scope;
            app.lib.nav.set_mode(scope, m);
            app.nav.order_for = 0;
            app.nav.table.invalidate();
            app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
        }
        NavAction::PinGroup(g, on) => {
            app.lib.nav.set_favorite(g, on);
            app.save_library();
        }
        NavAction::SetBadge(g, text) => {
            app.lib.nav.set_letter(g, &text);
            app.save_library();
        }
        NavAction::SaveView(name) => {
            let view = current_view(app, &name);
            if nav_model::save_view(&mut app.lib.settings.saved_views, view) {
                app.save_library();
            } else {
                app.toast(
                    t!("Ansicht nicht gespeichert (Name fehlt oder Liste voll)"),
                    crate::theme::amber(),
                );
            }
        }
        NavAction::ApplyView(id) => {
            let Some(v) = app
                .lib
                .settings
                .saved_views
                .iter()
                .find(|v| v.id == id)
                .cloned()
            else {
                return;
            };
            // A scope whose group is gone falls back to everything.
            let scope = v.scope.filter(|s| app.lib.group(*s).is_some());
            app.search = v.query.clone();
            app.host_filter = v.filter.min(5);
            app.lib.settings.sort = v.sort.min(2);
            set_scope(app, scope);
            app.lib.nav.set_mode(
                scope,
                if v.table {
                    ViewMode::Table
                } else {
                    ViewMode::List
                },
            );
            app.nav.order_for = 0;
            app.view = View::Hosts;
            app.active = None;
        }
        NavAction::DeleteView(id) => {
            nav_model::delete_view(&mut app.lib.settings.saved_views, id);
            app.save_library();
        }
        NavAction::MoveView(id, d) => {
            nav_model::move_view(&mut app.lib.settings.saved_views, id, d);
            app.save_library();
        }
        NavAction::Table(op) => {
            use crate::ui::nav_table::TableOp;
            let t = &mut app.lib.nav.table;
            match op {
                TableOp::Sort(c) => t.click_sort(c),
                TableOp::Toggle(c) => t.toggle(c),
                TableOp::Width(c, w) => t.set_width(c, w),
            }
            app.nav.save_due = Some(Instant::now() + SAVE_DELAY);
        }
        NavAction::Tag(ids, tag, add) => {
            let tag = tag.trim().to_string();
            if tag.is_empty() || tag.chars().any(|c| c.is_control()) {
                return;
            }
            let set: HashSet<Uuid> = ids.into_iter().collect();
            let mut n = 0;
            for h in app.lib.hosts.iter_mut().filter(|h| set.contains(&h.id)) {
                let has = h.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag));
                if add && !has {
                    h.tags.push(tag.clone());
                    n += 1;
                } else if !add && has {
                    h.tags.retain(|t| !t.eq_ignore_ascii_case(&tag));
                    n += 1;
                }
            }
            app.save_library();
            app.toast(
                if add {
                    t!("Tag „{}“ bei {} Hosts hinzugefügt", tag, n)
                } else {
                    t!("Tag „{}“ bei {} Hosts entfernt", tag, n)
                },
                crate::theme::accent(),
            );
        }
    }
    let _ = ctx;
}

/// Builds the confirmation for an environment change on a whole group.
pub fn group_env_action(app: &App, group: Uuid, env: Env) -> Option<Action> {
    let ids: Vec<Uuid> = app
        .lib
        .hosts_in_group(group)
        .into_iter()
        .map(|h| h.id)
        .collect();
    if ids.is_empty() {
        return None;
    }
    let name = app.lib.group(group).map(|g| g.name.as_str()).unwrap_or("?");
    let label = if env == Env::None {
        t!("keine")
    } else {
        env.label()
    };
    Some(Action::Confirm(
        t!(
            "Umgebung „{}“ für {} Hosts in „{}“ setzen?",
            label,
            ids.len(),
            name
        ),
        Box::new(Action::SetEnvMany(ids, env)),
    ))
}
