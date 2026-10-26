//! The connections list for big libraries: a virtualised group tree (only the rows in the
//! viewport are drawn), breadcrumb, scope chip, quick access chips, keyboard navigation
//! and group headers with counts, status roll-up and badges. Data and caching live in
//! `nav_state`/`nav_tree`.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, parse_quick};
use crate::model::{Env, Host, Protocol};
use crate::nav_model::ViewMode;
use crate::nav_tree::{self, RowKind};
use crate::theme;
use crate::ui::hosts::{host_row, protocol_tag};
use crate::ui::icons::{self, Icon};
use crate::ui::maintenance::Target;
use crate::ui::nav_state::{self, Cache, Cursor, NavAction};

// ---------------------------------------------------------------- small widgets

/// Rounded badge with the customer's letter(s) in the group colour.
pub fn paint_badge(p: &egui::Painter, rect: Rect, text: &str, color: Color32) {
    p.rect_filled(rect, CornerRadius::same(5), theme::soft(color));
    p.rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(1.0, color.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let size = if text.chars().count() > 1 { 9.5 } else { 11.5 };
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        theme::bold(size),
        color,
    );
}

/// "Kunde A  ✕" in the header: the whole app shows only this group.
pub fn scope_chip(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    scope_chip_lib(&app.lib, ui, actions);
}

/// [`scope_chip`] for callers that only hold the library (disjoint borrows of `App`).
pub fn scope_chip_lib(lib: &crate::model::Library, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let Some(g) = lib.nav.scope.and_then(|s| lib.group(s)) else {
        return;
    };
    let color = theme::rgb(g.color);
    let font = theme::semibold(13.0);
    let name = ui
        .painter()
        .layout_no_wrap(g.name.clone(), font.clone(), theme::text());
    let w = (name.size().x.min(200.0)) + 26.0 + 30.0 + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(14), theme::accent_soft());
    p.rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(1.0, theme::accent()),
        egui::StrokeKind::Inside,
    );
    paint_badge(
        p,
        Rect::from_min_size(rect.left_top() + Vec2::new(5.0, 4.0), Vec2::splat(20.0)),
        &nav_state::badge_text(lib, g),
        color,
    );
    let t = theme::truncated(p, g.name.clone(), font, theme::text(), 200.0);
    p.galley(
        Pos2::new(rect.left() + 32.0, rect.center().y - t.size().y / 2.0),
        t,
        theme::text(),
    );
    let x = Rect::from_center_size(
        Pos2::new(rect.right() - 15.0, rect.center().y),
        Vec2::splat(18.0),
    );
    let xr = ui.interact(x, ui.id().with("scope-x"), Sense::click());
    icons::paint(
        ui.painter(),
        x.shrink(3.0),
        Icon::Close,
        if xr.hovered() {
            theme::text()
        } else {
            theme::text_dim()
        },
    );
    resp.on_hover_text(t!(
        "Nur „{}“ – alles andere ist ausgeblendet. ✕ zeigt wieder alles.",
        g.name
    ));
    if xr.on_hover_text(t!("Fokus beenden")).clicked() {
        actions.push(Action::Nav(NavAction::Scope(None)));
    }
}

/// Extra toolbar buttons: views menu, list/table switch, expand/collapse all.
/// Called inside the right-to-left toolbar group, so the order is reversed.
#[allow(clippy::too_many_arguments)]
pub fn toolbar_extras(
    ui: &mut egui::Ui,
    compact: bool,
    views: &[crate::nav_model::SavedView],
    mode: ViewMode,
    query: &str,
    scope_name: Option<&str>,
    nav: &mut nav_state::NavState,
    actions: &mut Vec<Action>,
) {
    let r = theme::btn(
        ui,
        theme::Btn::Small,
        Some(Icon::Star),
        if compact { "" } else { t!("Ansichten") },
    );
    let r = r.on_hover_text(t!("Gespeicherte Ansichten (Suche + Filter + Fokus)"));
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(240.0);
        if views.is_empty() {
            ui.label(
                RichText::new(t!("Noch keine Ansicht gespeichert"))
                    .color(theme::text_dim())
                    .size(12.5),
            );
        }
        for v in views {
            ui.horizontal(|ui| {
                if ui.button(&v.name).clicked() {
                    actions.push(Action::Nav(NavAction::ApplyView(v.id)));
                    ui.close();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text(t!("Ansicht löschen"))
                        .clicked()
                    {
                        actions.push(Action::Nav(NavAction::DeleteView(v.id)));
                        ui.close();
                    }
                });
            });
        }
        ui.separator();
        if ui.button(t!("Aktuelle Ansicht speichern…")).clicked() {
            nav.view_name = Some(crate::nav_model::suggest_name(query, scope_name));
            ui.close();
        }
    });
    let sel = usize::from(mode == ViewMode::Table);
    if let Some(i) = theme::segmented(ui, &[t!("Liste"), t!("Tabelle")], sel) {
        actions.push(Action::Nav(NavAction::SetMode(if i == 1 {
            ViewMode::Table
        } else {
            ViewMode::List
        })));
    }
    if theme::icon_btn(ui, Icon::Up, t!("Alle Gruppen zuklappen")).clicked() {
        actions.push(Action::Nav(NavAction::ExpandAll(false)));
    }
    if theme::icon_btn(ui, Icon::Expand, t!("Alle Gruppen aufklappen")).clicked() {
        actions.push(Action::Nav(NavAction::ExpandAll(true)));
    }
}

fn chip(
    ui: &mut egui::Ui,
    badge: Option<(&str, Color32)>,
    text: &str,
    extra: &str,
) -> egui::Response {
    let font = theme::medium(12.5);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), theme::text());
    let ex = (!extra.is_empty()).then(|| {
        ui.painter()
            .layout_no_wrap(extra.to_string(), theme::medium(12.0), theme::text_dim())
    });
    let w = 10.0
        + if badge.is_some() { 24.0 } else { 0.0 }
        + g.size().x.min(150.0)
        + ex.as_ref().map(|e| e.size().x + 6.0).unwrap_or(0.0)
        + 10.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(13),
        if resp.hovered() {
            theme::fill_strong()
        } else {
            theme::fill()
        },
    );
    let mut x = rect.left() + 6.0;
    if let Some((b, c)) = badge {
        paint_badge(
            p,
            Rect::from_min_size(Pos2::new(x, rect.top() + 4.0), Vec2::splat(18.0)),
            b,
            c,
        );
        x += 24.0;
    } else {
        x += 4.0;
    }
    let t = theme::truncated(p, text, font, theme::text(), 150.0);
    let tw = t.size().x;
    p.galley(
        Pos2::new(x, rect.center().y - t.size().y / 2.0),
        t,
        theme::text(),
    );
    if let Some(e) = ex {
        p.galley(
            Pos2::new(x + tw + 6.0, rect.center().y - e.size().y / 2.0),
            e,
            theme::text_dim(),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pinned and recent groups and hosts as one row of chips above the list.
fn quick_strip(app: &App, c: &Cache, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let lib = &app.lib;
    let scope = c.scope_node;
    let mut groups: Vec<(usize, bool)> = Vec::new();
    for (i, n) in c.idx.nodes.iter().enumerate() {
        if n.favorite && scope.is_none_or(|s| s != i && c.idx.under(i, s)) {
            groups.push((i, true));
        }
    }
    for id in lib.nav.recent_groups.iter().take(4) {
        if let Some(&i) = c.idx.by_id.get(id)
            && !groups.iter().any(|g| g.0 == i)
            && scope.is_none_or(|s| s != i && c.idx.under(i, s))
        {
            groups.push((i, false));
        }
    }
    groups.truncate(8);
    // Small libraries need no host shortcuts (the sidebar has the recent ones).
    let hosts: Vec<u32> = c
        .pinned
        .iter()
        .chain(c.recent.iter())
        .copied()
        .fold(Vec::new(), |mut v, e| {
            if !v.contains(&e) {
                v.push(e);
            }
            v
        })
        .into_iter()
        .take(if lib.hosts.len() >= 30 { 8 } else { 0 })
        .collect();
    if groups.is_empty() && hosts.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for (i, pinned) in groups {
            let g = &lib.groups[i];
            let badge = nav_state::badge_text(lib, g);
            let n = c.shown.total[i];
            let r = chip(
                ui,
                Some((&badge, theme::rgb(g.color))),
                &g.name,
                &n.to_string(),
            );
            let r = r.on_hover_text(if pinned {
                t!("Fixierte Gruppe – Klick zeigt sie in der Liste")
            } else {
                t!("Zuletzt geöffnete Gruppe – Klick zeigt sie in der Liste")
            });
            if r.clicked() {
                actions.push(Action::Nav(NavAction::Reveal(g.id)));
            }
            r.context_menu(|ui| {
                if ui.button(t!("Nur diese Gruppe anzeigen")).clicked() {
                    actions.push(Action::Nav(NavAction::Scope(Some(g.id))));
                    ui.close();
                }
            });
        }
        for e in hosts {
            let h = &lib.hosts[c.idx.entries[e as usize].host];
            let r = chip(
                ui,
                None,
                h.display_name(),
                if h.favorite { "★" } else { "" },
            );
            let r = r.on_hover_text(t!("Verbinden mit {}", h.display_name()));
            if r.clicked() {
                actions.push(Action::Connect(h.id));
            }
            r.context_menu(|ui| {
                if ui.button(t!("In der Liste zeigen")).clicked() {
                    actions.push(Action::Nav(NavAction::RevealHost(h.id)));
                    ui.close();
                }
            });
        }
    });
    ui.add_space(8.0);
}

/// Path of the current group as clickable segments: `Alle › Kunde A › Berlin`.
fn breadcrumb(
    app: &App,
    c: &Cache,
    ui: &mut egui::Ui,
    cursor: Option<Cursor>,
    actions: &mut Vec<Action>,
) {
    let scoped = c.scope_node;
    let node = scoped.or_else(|| match cursor {
        Some(Cursor::Group(id)) => c.idx.by_id.get(&id).copied(),
        Some(Cursor::Host(id)) => app
            .lib
            .host(id)
            .and_then(|h| h.group_id)
            .and_then(|g| c.idx.by_id.get(&g).copied()),
        None => None,
    });
    let Some(node) = node else { return };
    let chain = c.idx.chain(node);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let seg = |ui: &mut egui::Ui, text: &str, last: bool| {
            let r = ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .font(if last {
                            theme::semibold(13.0)
                        } else {
                            theme::medium(13.0)
                        })
                        .color(if last {
                            theme::text()
                        } else {
                            theme::text_dim()
                        }),
                )
                .sense(Sense::click()),
            );
            r.on_hover_cursor(egui::CursorIcon::PointingHand)
        };
        let sep = |ui: &mut egui::Ui| {
            ui.label(RichText::new("›").color(theme::text_dim()));
        };
        if seg(ui, t!("Alle"), false).clicked() {
            actions.push(Action::Nav(NavAction::Scope(None)));
        }
        for (i, n) in chain.iter().enumerate() {
            sep(ui);
            let id = c.idx.nodes[*n].id;
            if seg(ui, &c.idx.nodes[*n].name, i + 1 == chain.len()).clicked() {
                actions.push(Action::Nav(if scoped.is_some() {
                    NavAction::Scope(Some(id))
                } else {
                    NavAction::Reveal(id)
                }));
            }
        }
    });
    ui.add_space(8.0);
}

// ---------------------------------------------------------------- group header

/// Drag payload of a group header (reorder among siblings); hosts drag a plain `Uuid`.
#[derive(Clone, Copy)]
struct GroupDrag {
    id: Uuid,
    parent: Option<Uuid>,
}

#[allow(clippy::too_many_arguments)]
fn group_header(
    app: &App,
    c: &Cache,
    ui: &mut egui::Ui,
    rect: Rect,
    node: usize,
    depth: u8,
    open: bool,
    at_cursor: bool,
    actions: &mut Vec<Action>,
) {
    let lib = &app.lib;
    let g = &lib.groups[node];
    // Header sits at the bottom of its row; the top part is the gap to the group above.
    let head = Rect::from_min_max(
        Pos2::new(rect.left() + depth as f32 * 20.0, rect.bottom() - 36.0),
        rect.right_bottom(),
    );
    let resp = ui.interact(head, ui.id().with(("grp", g.id)), Sense::click_and_drag());
    resp.dnd_set_drag_payload(GroupDrag {
        id: g.id,
        parent: g.parent_id,
    });
    let drop_hot =
        resp.dnd_hover_payload::<Uuid>().is_some() && crate::ui::nav2_dyn::accepts_hosts(lib, g.id);
    // Another group of the same parent is dragged over this header: insertion line.
    let mut gd_line: Option<(f32, bool)> = None;
    if let Some(gd) = resp.dnd_hover_payload::<GroupDrag>()
        && gd.id != g.id
        && gd.parent == g.parent_id
        && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
    {
        let after = pos.y > head.center().y;
        gd_line = Some((if after { head.bottom() } else { head.top() }, after));
    }
    if let Some(gd) = resp.dnd_release_payload::<GroupDrag>()
        && gd.id != g.id
        && gd.parent == g.parent_id
    {
        let after = ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| p.y > head.center().y);
        actions.push(Action::Nav(NavAction::DropGroup(gd.id, g.id, after)));
    }
    if let Some(id) = resp.dnd_release_payload::<Uuid>() {
        let ids = if app.selected_hosts.contains(&id) {
            app.selected_hosts.iter().copied().collect()
        } else {
            vec![*id]
        };
        actions.push(Action::MoveHosts(ids, Some(g.id)));
    }
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let cy = head.center().y;
    if let Some((y, _)) = gd_line {
        p.line_segment(
            [Pos2::new(head.left(), y), Pos2::new(head.right(), y)],
            Stroke::new(2.5, theme::accent()),
        );
    }
    if drop_hot {
        p.rect_filled(head, CornerRadius::same(8), theme::accent_soft());
    } else if at_cursor {
        p.rect_stroke(
            head,
            CornerRadius::same(8),
            Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        p.rect_filled(head, CornerRadius::same(8), theme::hover());
    }
    // Chevron.
    let chev = Rect::from_center_size(Pos2::new(head.left() + 10.0, cy), Vec2::splat(12.0));
    if open {
        icons::paint(&p, chev, Icon::Chevron, theme::text_dim());
    } else {
        let ctr = chev.center();
        let s = Stroke::new(2.0, theme::text_dim());
        p.line_segment([ctr + Vec2::new(-2.0, -4.0), ctr + Vec2::new(2.5, 0.0)], s);
        p.line_segment([ctr + Vec2::new(2.5, 0.0), ctr + Vec2::new(-2.0, 4.0)], s);
    }
    paint_badge(
        &p,
        Rect::from_center_size(Pos2::new(head.left() + 34.0, cy), Vec2::splat(22.0)),
        &nav_state::badge_text(lib, g),
        theme::rgb(g.color),
    );
    let right_limit = head.right() - if head.width() > 620.0 { 230.0 } else { 8.0 };
    let name = theme::truncated(
        &p,
        g.name.clone(),
        theme::semibold(14.5),
        theme::text(),
        (right_limit - head.left() - 60.0).clamp(40.0, 420.0),
    );
    let nw = name.size().x;
    p.galley(
        Pos2::new(head.left() + 52.0, cy - name.size().y / 2.0),
        name,
        theme::text(),
    );
    let mut tx = head.left() + 52.0 + nw + 10.0;
    if lib.nav.is_favorite(g.id) && tx + 14.0 < right_limit {
        icons::paint(
            &p,
            Rect::from_center_size(Pos2::new(tx + 6.0, cy), Vec2::splat(13.0)),
            Icon::Star,
            Color32::from_rgb(255, 214, 10),
        );
        tx += 20.0;
    }
    if let Some(f) = lib.nav.dynamic.get(&g.id) {
        tx = crate::ui::nav2_dyn::group_badge(&p, f, tx, cy, right_limit);
    }
    let total = c.shown.total[node];
    let roll = c.shown.roll[node];
    let count = if lib.settings.reachability && roll.probed() > 0 {
        t!("{} · {} online", total, roll.up)
    } else {
        total.to_string()
    };
    let cg = p.layout_no_wrap(count, FontId::proportional(13.0), theme::text_dim());
    let cw = cg.size().x;
    if tx + cw < right_limit {
        p.galley(Pos2::new(tx, cy - cg.size().y / 2.0), cg, theme::text_dim());
        tx += cw + 10.0;
    }
    // Status roll-up: red = unreachable or critical, amber = warnings.
    for (n, color, tip) in [
        (
            roll.red(),
            theme::red(),
            t!(
                "{} Hosts nicht erreichbar oder kritisch (voller Datenträger, Sicherheits-Updates)",
                roll.red()
            ),
        ),
        (
            roll.warn,
            theme::amber(),
            t!("{} Hosts mit Warnung (Datenträger, Updates)", roll.warn),
        ),
    ] {
        if n == 0 {
            continue;
        }
        let label = n.to_string();
        let tg = p.layout_no_wrap(label, theme::bold(11.5), theme::on_accent());
        let w = tg.size().x + 18.0;
        if tx + w > right_limit {
            break;
        }
        let r = Rect::from_min_size(Pos2::new(tx, cy - 9.0), Vec2::new(w, 18.0));
        p.rect_filled(r, CornerRadius::same(9), color);
        p.galley(r.center() - tg.size() / 2.0, tg, theme::on_accent());
        ui.interact(r, ui.id().with(("roll", g.id, n)), Sense::hover())
            .on_hover_text(tip);
        tx += w + 6.0;
    }
    if g.env != Env::None && tx + 70.0 < right_limit {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    Pos2::new(tx, cy - 9.0),
                    Vec2::new(90.0, 18.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        theme::pill(&mut child, g.env.label(), theme::env_color(g.env));
        tx += 76.0;
    }
    let maint = crate::ui::maintenance::active_for_group(lib, g.id, chrono::Utc::now()).cloned();
    if let Some(m) = &maint
        && tx + 120.0 < right_limit
    {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    Pos2::new(tx, cy - 11.0),
                    Vec2::new((right_limit - tx).min(360.0), 22.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        theme::tag(&mut child, &m.banner(), Some(theme::amber()));
    }
    // Quiet links on the right (wide rows only; the context menu has everything).
    let openable = (
        (total as usize).min(crate::ui::chrome::MAX_SPLIT),
        total as usize,
    );
    let mut clicked_link = false;
    if head.width() > 620.0 {
        let mut links = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(head.right() - 230.0, head.top()),
                    head.right_bottom() - Vec2::new(4.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        links.spacing_mut().item_spacing.x = 4.0;
        if !lib.nav.dynamic.contains_key(&g.id)
            && theme::btn(&mut links, theme::Btn::Link, None, "+ Host").clicked()
        {
            actions.push(Action::NewHost(Some(g.id)));
            clicked_link = true;
        }
        if total > 0 {
            let a = theme::btn(&mut links, theme::Btn::Link, None, t!("Alle öffnen"));
            let a = a.on_hover_text(t!(
                "Verbindet die ersten {} von {} Hosts der Gruppe im Splitscreen",
                openable.0,
                openable.1
            ));
            if a.clicked() {
                actions.push(Action::OpenGroup(g.id));
                clicked_link = true;
            }
        }
    }
    if resp.clicked() && !clicked_link {
        actions.push(Action::ToggleGroup(g.id));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .context_menu(|ui| group_menu(app, ui, node, g.id, total, maint.as_ref(), actions));
}

/// (hosts "Alle öffnen" would connect, hosts in the group below it).
fn app_open_count(app: &App, group: Uuid) -> (usize, usize) {
    let all = app.lib.hosts_in_group(group);
    let n = all
        .iter()
        .filter(|h| h.protocol != Protocol::Serial)
        .take(crate::ui::chrome::MAX_SPLIT)
        .count();
    (n, all.len())
}

/// The menu scrolls when the window is too small for all entries (720x480).
fn group_menu(
    app: &App,
    ui: &mut egui::Ui,
    node: usize,
    gid: Uuid,
    total: u32,
    maint: Option<&crate::ui::maintenance::Maintenance>,
    actions: &mut Vec<Action>,
) {
    let max_h = (ui.ctx().content_rect().height() - 24.0).max(160.0);
    egui::ScrollArea::vertical()
        .max_height(max_h)
        .auto_shrink([true, true])
        .show(ui, |ui| {
            group_menu_body(app, ui, node, gid, total, maint, actions)
        });
}

#[allow(clippy::too_many_arguments)]
fn group_menu_body(
    app: &App,
    ui: &mut egui::Ui,
    _node: usize,
    gid: Uuid,
    total: u32,
    maint: Option<&crate::ui::maintenance::Maintenance>,
    actions: &mut Vec<Action>,
) {
    let lib = &app.lib;
    let Some(g) = lib.group(gid) else { return };
    let openable = app_open_count(app, gid);
    let dynf = lib.nav.dynamic.get(&gid);
    let is_rule = dynf.is_some_and(|f| matches!(f.source, crate::nav2_dyn::DynSource::Rule { .. }));
    if !is_rule && ui.button(t!("Nur diese Gruppe anzeigen")).clicked() {
        actions.push(Action::Nav(NavAction::Scope(Some(gid))));
        ui.close();
    }
    if dynf.is_some() && ui.button(t!("Jetzt abgleichen")).clicked() {
        actions.push(Action::DynRefresh(gid));
        ui.close();
    }
    let (can_up, can_down) = (
        crate::ui::nav_state::can_move_group(lib, gid, -1),
        crate::ui::nav_state::can_move_group(lib, gid, 1),
    );
    if can_up || can_down {
        ui.menu_button(t!("Reihenfolge"), |ui| {
            if ui
                .add_enabled(can_up, egui::Button::new(t!("Nach oben verschieben")))
                .clicked()
            {
                actions.push(Action::Nav(NavAction::MoveGroup(gid, -1)));
                ui.close();
            }
            if ui
                .add_enabled(can_down, egui::Button::new(t!("Nach unten verschieben")))
                .clicked()
            {
                actions.push(Action::Nav(NavAction::MoveGroup(gid, 1)));
                ui.close();
            }
        });
    }
    if ui.button(t!("Alle Untergruppen aufklappen")).clicked() {
        actions.push(Action::Nav(NavAction::Reveal(gid)));
        ui.close();
    }
    let fav = lib.nav.is_favorite(gid);
    if ui
        .button(if fav {
            t!("Nicht mehr fixieren")
        } else {
            t!("Oben fixieren")
        })
        .clicked()
    {
        actions.push(Action::Nav(NavAction::PinGroup(gid, !fav)));
        ui.close();
    }
    if ui.button(t!("Abzeichen ändern…")).clicked() {
        let cur = lib
            .nav
            .meta(gid)
            .map(|m| m.letter.clone())
            .unwrap_or_default();
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("nav-badge-open"), (gid, cur)));
        ui.close();
    }
    ui.separator();
    if total > 0
        && ui
            .button(t!("Alle öffnen ({} von {})", openable.0, openable.1))
            .on_hover_text(t!(
                "Verbindet die ersten {} von {} Hosts der Gruppe im Splitscreen",
                openable.0,
                openable.1
            ))
            .clicked()
    {
        actions.push(Action::OpenGroup(gid));
        ui.close();
    }
    if openable.1 > 0
        && ui
            .button(t!("Befehl auf allen Hosts ausführen…"))
            .on_hover_text(t!("Befehl oder Snippet parallel auf der Gruppe ausführen"))
            .clicked()
    {
        actions.push(Action::Srv(crate::ui::multirun_ui::SrvAction::RunGroup(
            gid,
        )));
        ui.close();
    }
    if dynf.is_none() && ui.button(t!("Host hinzufügen")).clicked() {
        actions.push(Action::NewHost(Some(gid)));
        ui.close();
    }
    if ui.button(t!("Untergruppe")).clicked() {
        actions.push(Action::NewGroup(Some(gid)));
        ui.close();
    }
    if openable.1 > 0 {
        ui.menu_button(t!("Für alle {} Hosts", openable.1), |ui| {
            ui.menu_button(t!("Umgebung setzen"), |ui| {
                for e in Env::ALL {
                    let label = if e == Env::None {
                        t!("keine")
                    } else {
                        e.label()
                    };
                    if ui.button(label).clicked() {
                        if let Some(a) = nav_state::group_env_action(app, gid, e) {
                            actions.push(a);
                        }
                        ui.close();
                    }
                }
            });
            ui.menu_button(t!("Tag hinzufügen/entfernen"), |ui| {
                let id = egui::Id::new("nav-group-tag");
                let mut text: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
                ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text(t!("Tag"))
                        .desired_width(160.0),
                );
                ui.ctx().data_mut(|d| d.insert_temp(id, text.clone()));
                let ids: Vec<Uuid> = lib.hosts_in_group(gid).iter().map(|h| h.id).collect();
                let ok = !text.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(ok, egui::Button::new(t!("Hinzufügen")))
                        .clicked()
                    {
                        actions.push(Action::Confirm(
                            t!("Tag „{}“ bei {} Hosts hinzufügen?", text.trim(), ids.len()),
                            Box::new(Action::Nav(NavAction::Tag(ids.clone(), text.clone(), true))),
                        ));
                        ui.close();
                    }
                    if ui
                        .add_enabled(ok, egui::Button::new(t!("Entfernen")))
                        .clicked()
                    {
                        actions.push(Action::Confirm(
                            t!("Tag „{}“ bei {} Hosts entfernen?", text.trim(), ids.len()),
                            Box::new(Action::Nav(NavAction::Tag(
                                ids.clone(),
                                text.clone(),
                                false,
                            ))),
                        ));
                        ui.close();
                    }
                });
            });
        });
    }
    ui.separator();
    crate::ui::hosts::maintenance_menu(ui, maint, Target::Group(gid), actions);
    if ui.button(t!("Bearbeiten")).clicked() {
        actions.push(Action::EditGroup(gid));
        ui.close();
    }
    if ui
        .button(RichText::new(t!("Löschen")).color(theme::red()))
        .clicked()
    {
        actions.push(Action::Confirm(
            t!("Gruppe „{}“ löschen? Die Hosts bleiben erhalten.", g.name),
            Box::new(Action::DeleteGroup(gid)),
        ));
        ui.close();
    }
}

// ---------------------------------------------------------------- keyboard

pub(crate) fn modal_or_popup(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.top_modal_layer().is_some())
        || egui::Popup::is_any_open(ctx)
        || ctx.memory(|m| m.focused().is_some())
}

fn cursor_of(app: &App, c: &Cache, kind: RowKind) -> Option<Cursor> {
    match kind {
        RowKind::Group(n) => Some(Cursor::Group(c.idx.nodes[n as usize].id)),
        RowKind::Host(e) => Some(Cursor::Host(
            app.lib.hosts[c.idx.entries[e as usize].host].id,
        )),
        RowKind::Loose => None,
    }
}

fn row_of(app: &App, c: &Cache, cur: Cursor) -> Option<usize> {
    match cur {
        Cursor::Group(id) => c
            .idx
            .by_id
            .get(&id)
            .and_then(|n| c.layout.row_of_group(*n)),
        Cursor::Host(id) => c.layout.rows.iter().position(|r| {
            matches!(r.kind, RowKind::Host(e) if app.lib.hosts[c.idx.entries[e as usize].host].id == id)
        }),
    }
}

/// Arrow keys, Enter, Space, Home/End on the cursor. Returns the new cursor.
fn keys(
    app: &App,
    c: &Cache,
    ui: &egui::Ui,
    cursor: Option<Cursor>,
    actions: &mut Vec<Action>,
) -> (Option<Cursor>, bool) {
    use egui::{Key, Modifiers};
    let rows = &c.layout.rows;
    if rows.is_empty() || modal_or_popup(ui.ctx()) {
        return (cursor, false);
    }
    let cur_row = cursor.and_then(|cu| row_of(app, c, cu));
    let (up, down, pgup, pgdn, home, end, left, right, enter, space) = ui.input_mut(|i| {
        let m = Modifiers::NONE;
        (
            i.consume_key(m, Key::ArrowUp),
            i.consume_key(m, Key::ArrowDown),
            i.consume_key(m, Key::PageUp),
            i.consume_key(m, Key::PageDown),
            i.consume_key(m, Key::Home),
            i.consume_key(m, Key::End),
            i.consume_key(m, Key::ArrowLeft),
            i.consume_key(m, Key::ArrowRight),
            i.consume_key(m, Key::Enter),
            i.consume_key(m, Key::Space),
        )
    });
    let step = |from: Option<usize>, delta: i64| -> Option<usize> {
        let mut i =
            from.map(|f| f as i64)
                .unwrap_or(if delta > 0 { -1 } else { rows.len() as i64 });
        let dir = delta.signum();
        let mut left = delta.abs();
        while left > 0 {
            let mut j = i + dir;
            while j >= 0 && (j as usize) < rows.len() && rows[j as usize].kind == RowKind::Loose {
                j += dir;
            }
            if j < 0 || j as usize >= rows.len() {
                break;
            }
            i = j;
            left -= 1;
        }
        (i >= 0 && (i as usize) < rows.len()).then_some(i as usize)
    };
    let mut target: Option<usize> = None;
    if up {
        target = step(cur_row, -1);
    } else if down {
        target = step(cur_row, 1);
    } else if pgup {
        target = step(cur_row, -10);
    } else if pgdn {
        target = step(cur_row, 10);
    } else if home {
        target = step(None, 1);
    } else if end {
        target = step(None, -1);
    }
    if let Some(r) = cur_row {
        match rows[r].kind {
            RowKind::Group(n) => {
                let open = c.layout.rows.get(r + 1).is_some_and(|nx| {
                    nx.depth > rows[r].depth
                        || (matches!(nx.kind, RowKind::Host(_)) && nx.depth == rows[r].depth)
                }) && nav_open(app, c, n as usize);
                if right {
                    if !nav_open(app, c, n as usize) {
                        actions.push(Action::ToggleGroup(c.idx.nodes[n as usize].id));
                    } else {
                        target = step(Some(r), 1);
                    }
                } else if left {
                    if open || nav_open(app, c, n as usize) {
                        actions.push(Action::ToggleGroup(c.idx.nodes[n as usize].id));
                    } else if let Some(p) = c.idx.nodes[n as usize].parent {
                        target = c.layout.row_of_group(p);
                    }
                } else if enter || space {
                    actions.push(Action::ToggleGroup(c.idx.nodes[n as usize].id));
                }
            }
            RowKind::Host(e) => {
                let id = app.lib.hosts[c.idx.entries[e as usize].host].id;
                if enter {
                    actions.push(Action::Connect(id));
                } else if space {
                    actions.push(Action::SelectHost(id, 1));
                } else if left {
                    target = c.idx.entries[e as usize]
                        .node
                        .and_then(|n| c.layout.row_of_group(n));
                }
            }
            RowKind::Loose => {}
        }
    }
    match target {
        Some(t) => (cursor_of(app, c, rows[t].kind).or(cursor), true),
        None => (cursor, false),
    }
}

fn nav_open(app: &App, c: &Cache, n: usize) -> bool {
    nav_state::is_open_node(app, c, n)
}

// ---------------------------------------------------------------- the list

/// Everything below the toolbar: breadcrumb, quick chips, results or tree/table.
pub fn body(
    app: &mut App,
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    enter: bool,
    live: &[String],
) {
    let ctx = ui.ctx().clone();
    // Type-to-filter: printable keys with nothing focused go to the search field.
    if !modal_or_popup(&ctx) {
        let typed: String = ctx.input(|i| {
            if i.modifiers.command || i.modifiers.alt {
                return String::new();
            }
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) if !t.chars().all(char::is_whitespace) => Some(t.as_str()),
                    _ => None,
                })
                .collect()
        });
        if !typed.is_empty() {
            app.search.push_str(&typed);
            app.nav.focus_search = true;
        }
    }
    let drag = egui::DragAndDrop::has_payload_of_type::<Uuid>(&ctx);
    nav_state::ensure(app, &ctx, drag);
    nav_state::flush(app, &ctx);
    let Some(cache) = app.nav.cache.take() else {
        return;
    };
    let mut cursor = app.nav.cursor;
    let mut scroll_to = app.nav.scroll_to.take();
    let (mut scroll_y, mut view_h) = (app.nav.scroll_y, app.nav.view_h);
    let mode = app.lib.nav.mode(app.lib.nav.scope);

    {
        let app_ref = &*app;
        let query = app_ref.search.trim().to_string();
        breadcrumb(app_ref, &cache, ui, cursor, actions);
        if !cache.filtering {
            quick_strip(app_ref, &cache, ui, actions);
        }
        if !query.is_empty() {
            let quick = parse_quick(&query).filter(|_| query.contains('@'));
            if let Some(h) = &quick {
                crate::ui::hosts::quick_row(
                    ui,
                    &format!("{}@{}:{}", h.username, h.hostname, h.port),
                    actions,
                    &query,
                );
            }
            let first = match &cache.flat {
                Some(f) => f.first().copied(),
                None => cache.shown.order.first().copied(),
            };
            if enter {
                match (&quick, first) {
                    (Some(_), _) => actions.push(Action::QuickConnect(query.clone())),
                    (None, Some(e)) => actions.push(Action::Connect(
                        app_ref.lib.hosts[cache.idx.entries[e as usize].host].id,
                    )),
                    _ => {}
                }
            }
            if cache.shown.order.is_empty() && quick.is_none() {
                no_results(ui, &query, actions);
            }
        } else if ui.input(|i| i.key_pressed(egui::Key::Escape))
            && !app_ref.selected_hosts.is_empty()
        {
            actions.push(Action::ClearSelection);
        }
        let live_set = live;
        if mode == ViewMode::Table {
            crate::ui::nav_table::show(app_ref, &cache, ui, &mut cursor, &mut scroll_to, actions);
        } else {
            let (moved, scrolled) = keys(app_ref, &cache, ui, cursor, actions);
            cursor = moved;
            if scrolled {
                scroll_to = cursor;
            }
            let target_row = scroll_to.and_then(|cu| row_of(app_ref, &cache, cu));
            let offset = target_row
                .and_then(|r| nav_tree::scroll_to_row(&cache.layout.y, r, scroll_y, view_h));
            let mut area = egui::ScrollArea::vertical()
                .id_salt("nav-list")
                .auto_shrink([false, false]);
            if let Some(o) = offset {
                area = area.vertical_scroll_offset(o);
            }
            let out = area.show_viewport(ui, |ui, viewport| {
                draw_rows(app_ref, &cache, ui, viewport, cursor, live_set, actions);
            });
            scroll_y = out.state.offset.y;
            view_h = out.inner_rect.height();
        }
    }
    app.nav.cursor = cursor;
    app.nav.scroll_y = scroll_y;
    app.nav.view_h = view_h;
    app.nav.cache = Some(cache);
}

fn no_results(ui: &mut egui::Ui, query: &str, actions: &mut Vec<Action>) {
    ui.add_space(20.0);
    ui.label(theme::heading(t!("Kein Host passt zu „{}“", query), 17.0));
    ui.add_space(4.0);
    ui.label(
        RichText::new(t!(
            "Leg ihn an – oder tippe user@host, um direkt zu verbinden."
        ))
        .color(theme::text_dim()),
    );
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        if theme::primary_button(ui, &t!("„{}“ als Host anlegen", query)).clicked() {
            actions.push(Action::NewHostNamed(query.to_string()));
        }
        if !query.contains(' ')
            && theme::btn(ui, theme::Btn::Normal, None, t!("Direkt verbinden")).clicked()
        {
            actions.push(Action::QuickConnect(query.to_string()));
        }
    });
    ui.add_space(8.0);
    ui.label(
        RichText::new(t!(
            "Tipp: type:switch  is:down  env:prod  #tag  10.1.  kunde/standort/name"
        ))
        .size(12.5)
        .color(theme::text_dim()),
    );
}

fn draw_rows(
    app: &App,
    c: &Cache,
    ui: &mut egui::Ui,
    viewport: Rect,
    cursor: Option<Cursor>,
    live: &[String],
    actions: &mut Vec<Action>,
) {
    let width = (ui.available_width() - 14.0).max(100.0);
    let total = c.layout.total() + 40.0;
    let (full, _) = ui.allocate_exact_size(Vec2::new(width, total), Sense::hover());
    let range = nav_tree::visible_range(&c.layout.y, viewport.top(), viewport.bottom());
    let lib = &app.lib;
    for i in range {
        let row = c.layout.rows[i];
        let top = full.top() + c.layout.y[i];
        let indent = row.depth as f32 * 20.0;
        match row.kind {
            RowKind::Group(n) => {
                let rect =
                    Rect::from_min_size(Pos2::new(full.left(), top), Vec2::new(width, row.h));
                let n = n as usize;
                let open = nav_state::is_open_node(app, c, n);
                let at = cursor == Some(Cursor::Group(c.idx.nodes[n].id));
                group_header(app, c, ui, rect, n, row.depth, open, at, actions);
            }
            RowKind::Loose => {
                let rect =
                    Rect::from_min_size(Pos2::new(full.left(), top), Vec2::new(width, row.h));
                let (r, resp) = (
                    rect.shrink2(Vec2::new(0.0, 8.0)),
                    ui.interact(rect, ui.id().with("loose"), Sense::hover()),
                );
                if resp.dnd_hover_payload::<Uuid>().is_some() {
                    ui.painter().rect_stroke(
                        r,
                        CornerRadius::same(8),
                        Stroke::new(1.5, theme::accent()),
                        egui::StrokeKind::Inside,
                    );
                }
                ui.painter().text(
                    r.left_center() + Vec2::new(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    t!("OHNE GRUPPE"),
                    theme::semibold(12.0),
                    theme::text_dim(),
                );
                if let Some(id) = resp.dnd_release_payload::<Uuid>() {
                    let ids = if app.selected_hosts.contains(&id) {
                        app.selected_hosts.iter().copied().collect()
                    } else {
                        vec![*id]
                    };
                    actions.push(Action::MoveHosts(ids, None));
                }
            }
            RowKind::Host(e) => {
                let en = &c.idx.entries[e as usize];
                let h: &Host = &lib.hosts[en.host];
                let rect = Rect::from_min_size(
                    Pos2::new(full.left() + indent, top),
                    Vec2::new((width - indent).max(60.0), row.h),
                );
                let r = 14;
                let radius = CornerRadius {
                    nw: if row.first { r } else { 0 },
                    ne: if row.first { r } else { 0 },
                    sw: if row.last { r } else { 0 },
                    se: if row.last { r } else { 0 },
                };
                let p = ui.painter();
                p.rect_filled(rect, radius, theme::surface());
                let edge = Stroke::new(1.0, theme::card_edge());
                p.line_segment([rect.left_top(), rect.left_bottom()], edge);
                p.line_segment([rect.right_top(), rect.right_bottom()], edge);
                if row.first {
                    p.line_segment(
                        [
                            rect.left_top() + Vec2::new(r as f32, 0.0),
                            rect.right_top() - Vec2::new(r as f32, 0.0),
                        ],
                        edge,
                    );
                }
                if row.last {
                    p.line_segment(
                        [
                            rect.left_bottom() + Vec2::new(r as f32, 0.0),
                            rect.right_bottom() - Vec2::new(r as f32, 0.0),
                        ],
                        edge,
                    );
                }
                let path = (c.flat.is_some())
                    .then(|| en.node.map(|n| c.idx.nodes[n].path.join(" / ")))
                    .flatten();
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .id_salt(("nav-row", i))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.spacing_mut().item_spacing.y = 0.0;
                host_row(
                    app,
                    &mut child,
                    h,
                    live,
                    !row.first,
                    path.as_deref(),
                    actions,
                );
                if cursor == Some(Cursor::Host(h.id)) {
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        radius,
                        Stroke::new(1.5, theme::accent()),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
    let _ = protocol_tag;
}
