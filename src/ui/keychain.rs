//! Keys & passwords: identities (user + password and/or private key), in groups with a
//! search.

use egui::{CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::keygen::KeyMeta;
use crate::model::{Identity, KeyGroup, Keychain};
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::{empty_list, row_buttons};

/// State of the keys page that outlives a frame.
#[derive(Default)]
pub struct KeysUi {
    /// Group being created (`true`) or edited.
    pub group_editor: Option<(KeyGroup, bool)>,
}

/// Changes to key groups, applied after the frame (`Action::KeyGroup`).
#[derive(Clone, Debug)]
pub enum KeyGroupOp {
    New,
    Edit(Uuid),
    /// Removes the group; its keys move to "Ohne Gruppe".
    Delete(Uuid),
    Toggle(Uuid),
    /// Key into group (`None` = no group).
    Move(Uuid, Option<Uuid>),
    /// Key editor for a new key in this group.
    NewKeyIn(Uuid),
}

/// Drag payload of a key row (distinct from host drags).
#[derive(Clone, Copy)]
struct KeyDrag(Uuid);

pub fn apply(app: &mut App, op: KeyGroupOp) {
    match op {
        KeyGroupOp::New => {
            let n = app.keychain.groups.len();
            let g = KeyGroup {
                color: theme::TAG_COLORS[n % theme::TAG_COLORS.len()],
                ..Default::default()
            };
            app.keys_ui.group_editor = Some((g, true));
        }
        KeyGroupOp::Edit(id) => {
            if let Some(g) = app.keychain.key_group(id) {
                app.keys_ui.group_editor = Some((g.clone(), false));
            }
        }
        KeyGroupOp::Delete(id) => {
            for i in &mut app.keychain.identities {
                if i.group_id == Some(id) {
                    i.group_id = None;
                }
            }
            app.keychain.groups.retain(|g| g.id != id);
            app.save_keychain();
        }
        KeyGroupOp::Toggle(id) => {
            if let Some(g) = app.keychain.groups.iter_mut().find(|g| g.id == id) {
                g.collapsed = !g.collapsed;
            }
            app.save_keychain();
        }
        KeyGroupOp::Move(key, group) => {
            if let Some(i) = app.keychain.identities.iter_mut().find(|i| i.id == key) {
                i.group_id = group;
            }
            // A collapsed target opens so the key stays visible.
            if let Some(g) = app.keychain.groups.iter_mut().find(|g| Some(g.id) == group) {
                g.collapsed = false;
            }
            app.save_keychain();
        }
        KeyGroupOp::NewKeyIn(group) => {
            let mut ident = Identity::new();
            ident.group_id = Some(group);
            app.identity_editor = Some((ident, true, String::new()));
        }
    }
}

/// Identities for a picker, in page order: (id, name, group name or "").
pub fn choices(kc: &Keychain, keep: impl Fn(&Identity) -> bool) -> Vec<(Uuid, String, String)> {
    kc.grouped()
        .into_iter()
        .flat_map(|(g, idents)| {
            let group = g.map(|g| g.name.clone()).unwrap_or_default();
            idents
                .into_iter()
                .filter(|i| keep(i))
                .map(|i| {
                    let name = if i.label.is_empty() {
                        i.username.clone()
                    } else {
                        i.label.clone()
                    };
                    (i.id, name, group.clone())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Type, comment and fingerprint of a key, cached per key text.
fn meta(ctx: &egui::Context, ident: &Identity) -> Option<KeyMeta> {
    if ident.private_key.trim().is_empty() {
        return None;
    }
    let id = egui::Id::new(("key-meta", &ident.private_key));
    ctx.data_mut(|d| d.get_temp::<Option<KeyMeta>>(id))
        .unwrap_or_else(|| {
            let m = crate::keygen::key_meta(&ident.private_key);
            ctx.data_mut(|d| d.insert_temp(id, m.clone()));
            m
        })
}

fn badge(ident: &Identity) -> &'static str {
    match (ident.private_key.is_empty(), ident.password.is_empty()) {
        (false, false) => "K+P",
        (false, true) => "KEY",
        (true, false) => "PW",
        (true, true) => "USR",
    }
}

/// Lower-case text the search looks at.
fn haystack(ident: &Identity, group: Option<&KeyGroup>, meta: Option<&KeyMeta>) -> String {
    let mut s = format!("{} {}", ident.label, ident.username);
    if !ident.private_key.is_empty() {
        s.push_str(" ssh-key key");
    }
    if !ident.password.is_empty() {
        s.push_str(" passwort password");
    }
    if let Some(m) = meta {
        s.push_str(&format!(" {} {} {}", m.kind, m.comment, m.fingerprint));
    }
    if let Some(g) = group {
        s.push(' ');
        s.push_str(&g.name);
    }
    s.to_lowercase()
}

/// Every word of the query appears somewhere in the text.
fn matches(query: &str, hay: &str) -> bool {
    query
        .split_whitespace()
        .all(|w| hay.contains(&w.to_lowercase()))
}

struct Row {
    ident: Identity,
    meta: Option<KeyMeta>,
    used_by: usize,
}

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let count = app.keychain.identities.len();
    ui.horizontal(|ui| {
        theme::page_title(ui, t!("Schlüssel"), Some(count));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("Neu")).clicked() {
                actions.push(Action::NewIdentity);
            }
            if theme::btn(
                ui,
                theme::Btn::Normal,
                Some(Icon::Folder),
                t!("Neue Gruppe"),
            )
            .clicked()
            {
                actions.push(Action::KeyGroup(KeyGroupOp::New));
            }
            let width = (ui.available_width() - 24.0).clamp(180.0, 300.0);
            let search = theme::search_field(
                ui,
                &mut app.keychain_search,
                t!("Suchen: Name, Typ, Gruppe …"),
                width,
                egui::Id::new("key-search"),
            );
            if (search.has_focus() || search.lost_focus())
                && ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                app.keychain_search.clear();
            }
        });
    });
    ui.add_space(18.0);
    group_editor(app, ui.ctx());

    if count == 0 && app.keychain.groups.is_empty() {
        if empty_list(
            ui,
            t!("Noch keine Schlüssel"),
            t!(
                "Speichere Passwörter und SSH-Keys einmal und verwende sie für beliebig viele Hosts."
            ),
            t!("Schlüssel anlegen"),
        ) {
            actions.push(Action::NewIdentity);
        }
        return;
    }

    // Sections in page order, filtered by the search.
    let query = app.keychain_search.trim().to_string();
    let searching = !query.is_empty();
    let ctx = ui.ctx().clone();
    let sections: Vec<(Option<KeyGroup>, usize, Vec<Row>)> = app
        .keychain
        .grouped()
        .into_iter()
        .map(|(g, idents)| {
            let total = idents.len();
            let rows = idents
                .into_iter()
                .filter_map(|i| {
                    let meta = meta(&ctx, i);
                    if !matches(&query, &haystack(i, g, meta.as_ref())) {
                        return None;
                    }
                    let used_by = app
                        .lib
                        .hosts
                        .iter()
                        .filter(|h| h.identity_id == Some(i.id))
                        .count()
                        + app
                            .lib
                            .groups
                            .iter()
                            .filter(|hg| hg.default_identity == Some(i.id))
                            .count();
                    Some(Row {
                        ident: i.clone(),
                        meta,
                        used_by,
                    })
                })
                .collect();
            (g.cloned(), total, rows)
        })
        .collect();
    let groups: Vec<(Uuid, String, [u8; 3])> = app
        .keychain
        .groups
        .iter()
        .map(|g| (g.id, g.name.clone(), g.color))
        .collect();
    let dragging = egui::DragAndDrop::has_payload_of_type::<KeyDrag>(ui.ctx());

    let mut toast = None;
    egui::ScrollArea::vertical()
        .id_salt("keychain-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the scroll bar next to the cards.
            ui.set_max_width(ui.available_width() - 14.0);
            if searching && sections.iter().all(|(_, _, rows)| rows.is_empty()) {
                theme::card(ui, theme::bg_deep(), |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(22, 20))
                        .show(ui, |ui| {
                            ui.label(theme::heading(t!("Keine Treffer"), 16.0));
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(t!(
                                    "Kein Schlüssel passt zu „{}“. Gesucht wird in Name, Benutzer, Typ, Kommentar, Fingerprint und Gruppe.",
                                    query
                                ))
                                .color(theme::text_dim()),
                            );
                        });
                });
                return;
            }
            let mut first = true;
            for (g, total, rows) in &sections {
                match g {
                    Some(g) => {
                        // While searching, only groups with hits are shown, opened.
                        if searching && rows.is_empty() {
                            continue;
                        }
                        ui.add_space(if first { 4.0 } else { 22.0 });
                        first = false;
                        group_header(ui, g, *total, rows.len(), searching, actions);
                        if g.collapsed && !searching {
                            continue;
                        }
                        ui.add_space(6.0);
                        if rows.is_empty() {
                            empty_group_hint(ui, g.id, actions);
                        } else {
                            key_card(ui, rows, &groups, &mut toast, actions);
                        }
                    }
                    None => {
                        if rows.is_empty() && !(dragging && !groups.is_empty()) {
                            continue;
                        }
                        if !groups.is_empty() {
                            ui.add_space(if first { 4.0 } else { 22.0 });
                            loose_header(ui, rows.len(), actions);
                            ui.add_space(6.0);
                        }
                        first = false;
                        if !rows.is_empty() {
                            key_card(ui, rows, &groups, &mut toast, actions);
                        }
                    }
                }
            }
            ui.add_space(40.0);
        });
    if let Some((text, color)) = toast {
        app.toast(text, color);
    }
}

/// Chevron, colour square, name and count; drop target for key rows.
fn group_header(
    ui: &mut egui::Ui,
    g: &KeyGroup,
    total: usize,
    shown: usize,
    searching: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let drop_hot = resp.dnd_hover_payload::<KeyDrag>().is_some();
    if let Some(d) = resp.dnd_release_payload::<KeyDrag>() {
        actions.push(Action::KeyGroup(KeyGroupOp::Move(d.0, Some(g.id))));
    }
    let p = ui.painter();
    let cy = rect.center().y;
    if drop_hot {
        p.rect_filled(rect, CornerRadius::same(8), theme::accent_soft());
    }
    let chev = Rect::from_center_size(Pos2::new(rect.left() + 10.0, cy), Vec2::splat(12.0));
    if g.collapsed && !searching {
        let c = chev.center();
        let s = Stroke::new(2.0, theme::text_dim());
        p.line_segment([c + Vec2::new(-2.0, -4.0), c + Vec2::new(2.5, 0.0)], s);
        p.line_segment([c + Vec2::new(2.5, 0.0), c + Vec2::new(-2.0, 4.0)], s);
    } else {
        icons::paint(p, chev, Icon::Chevron, theme::text_dim());
    }
    p.rect_filled(
        Rect::from_center_size(Pos2::new(rect.left() + 28.0, cy), Vec2::splat(9.0)),
        CornerRadius::same(2),
        theme::rgb(g.color),
    );
    let name = if g.name.is_empty() {
        t!("(ohne Namen)").to_string()
    } else {
        g.name.clone()
    };
    let galley = p.layout_no_wrap(name, theme::semibold(14.5), theme::text());
    let w = galley.size().x;
    p.galley(
        Pos2::new(rect.left() + 40.0, cy - galley.size().y / 2.0),
        galley,
        theme::text(),
    );
    let count = if searching && shown != total {
        t!("{} von {}", shown, total)
    } else {
        total.to_string()
    };
    p.text(
        Pos2::new(rect.left() + 40.0 + w + 10.0, cy),
        egui::Align2::LEFT_CENTER,
        count,
        FontId::proportional(13.0),
        theme::text_dim(),
    );
    let mut links = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(rect.right() - 220.0, rect.top()),
                rect.right_bottom() - Vec2::new(4.0, 0.0),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    links.spacing_mut().item_spacing.x = 4.0;
    let plus = theme::btn(&mut links, theme::Btn::Link, None, t!("+ Schlüssel"));
    if plus.clicked() {
        actions.push(Action::KeyGroup(KeyGroupOp::NewKeyIn(g.id)));
    } else if resp.clicked() && !searching {
        actions.push(Action::KeyGroup(KeyGroupOp::Toggle(g.id)));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .context_menu(|ui| {
            if ui.button(t!("Schlüssel hinzufügen")).clicked() {
                actions.push(Action::KeyGroup(KeyGroupOp::NewKeyIn(g.id)));
                ui.close();
            }
            if ui.button(t!("Umbenennen…")).clicked() {
                actions.push(Action::KeyGroup(KeyGroupOp::Edit(g.id)));
                ui.close();
            }
            if ui
                .button(RichText::new(t!("Löschen")).color(theme::red()))
                .clicked()
            {
                actions.push(delete_group(g, total));
                ui.close();
            }
        });
}

/// Deleting asks first; the keys stay and move to "Ohne Gruppe".
fn delete_group(g: &KeyGroup, keys: usize) -> Action {
    let text = if keys == 0 {
        t!("Gruppe „{}“ löschen?", g.name)
    } else if keys == 1 {
        t!(
            "Gruppe „{}“ löschen? Der Schlüssel landet in „Ohne Gruppe“.",
            g.name
        )
    } else {
        t!(
            "Gruppe „{}“ löschen? Ihre {} Schlüssel landen in „Ohne Gruppe“.",
            g.name,
            keys
        )
    };
    Action::Confirm(text, Box::new(Action::KeyGroup(KeyGroupOp::Delete(g.id))))
}

/// "OHNE GRUPPE" above the ungrouped keys; dropping a key here takes it out of its group.
fn loose_header(ui: &mut egui::Ui, n: usize, actions: &mut Vec<Action>) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
    if resp.dnd_hover_payload::<KeyDrag>().is_some() {
        ui.painter().rect_stroke(
            r,
            CornerRadius::same(8),
            Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    let g = ui.painter().layout_no_wrap(
        t!("OHNE GRUPPE").to_string(),
        theme::semibold(12.0),
        theme::text_dim(),
    );
    let w = g.size().x;
    ui.painter().galley(
        Pos2::new(r.left() + 8.0, r.center().y - g.size().y / 2.0),
        g,
        theme::text_dim(),
    );
    ui.painter().text(
        Pos2::new(r.left() + 8.0 + w + 8.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        n.to_string(),
        FontId::proportional(12.5),
        theme::text_dim(),
    );
    if let Some(d) = resp.dnd_release_payload::<KeyDrag>() {
        actions.push(Action::KeyGroup(KeyGroupOp::Move(d.0, None)));
    }
}

/// Outlined placeholder below a group without keys.
fn empty_group_hint(ui: &mut egui::Ui, group: Uuid, actions: &mut Vec<Action>) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
    let hot = resp.dnd_hover_payload::<KeyDrag>().is_some();
    if let Some(d) = resp.dnd_release_payload::<KeyDrag>() {
        actions.push(Action::KeyGroup(KeyGroupOp::Move(d.0, Some(group))));
    }
    let p = ui.painter();
    let color = if hot { theme::accent() } else { theme::line() };
    if hot {
        p.rect_filled(rect, CornerRadius::same(14), theme::accent_soft());
    }
    p.rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.left_center() + Vec2::new(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        t!("Noch leer – Schlüssel hierher ziehen oder über „In Gruppe verschieben“ zuordnen"),
        FontId::proportional(13.0),
        theme::text_dim(),
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(t!("Neuen Schlüssel in dieser Gruppe anlegen"))
        .clicked()
    {
        actions.push(Action::KeyGroup(KeyGroupOp::NewKeyIn(group)));
    }
}

/// One rounded card with a row per key.
fn key_card(
    ui: &mut egui::Ui,
    rows: &[Row],
    groups: &[(Uuid, String, [u8; 3])],
    toast: &mut Option<(String, egui::Color32)>,
    actions: &mut Vec<Action>,
) {
    theme::card(ui, theme::bg_deep(), |ui| {
        for (i, row) in rows.iter().enumerate() {
            key_row(ui, row, i > 0, groups, toast, actions);
        }
    });
}

fn key_row(
    ui: &mut egui::Ui,
    row: &Row,
    divider: bool,
    groups: &[(Uuid, String, [u8; 3])],
    toast: &mut Option<(String, egui::Color32)>,
    actions: &mut Vec<Action>,
) {
    let ident = &row.ident;
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 68.0),
        Sense::click_and_drag(),
    );
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if hovered || resp.context_menu_opened() {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if divider {
        p.line_segment(
            [Pos2::new(rect.left() + 70.0, rect.top()), rect.right_top()],
            Stroke::new(1.0, theme::line()),
        );
    }
    let title = if ident.label.is_empty() {
        t!("(ohne Namen)")
    } else {
        &ident.label
    };
    // Drag onto a group heading.
    resp.dnd_set_drag_payload(KeyDrag(ident.id));
    if resp.dragged()
        && let Some(pos) = ui.ctx().pointer_latest_pos()
    {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("key-drag"),
        ));
        let g = painter.layout_no_wrap(title.to_string(), theme::semibold(13.0), theme::text());
        let r = Rect::from_min_size(
            pos + Vec2::new(14.0, 10.0),
            g.size() + Vec2::new(22.0, 14.0),
        );
        painter.rect(
            r,
            CornerRadius::same(10),
            theme::surface(),
            Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
        painter.galley(r.min + Vec2::new(11.0, 7.0), g, theme::text());
    }
    let cy = rect.center().y;
    let badge_rect = Rect::from_center_size(Pos2::new(rect.left() + 37.0, cy), Vec2::splat(38.0));
    p.rect_filled(badge_rect, CornerRadius::same(10), theme::accent_soft());
    p.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        badge(ident),
        theme::bold(12.0),
        theme::accent(),
    );
    let tg = p.layout_no_wrap(title.to_string(), theme::semibold(15.0), theme::text());
    let tw = tg.size().x;
    p.galley(
        Pos2::new(rect.left() + 72.0, cy - 9.0 - tg.size().y / 2.0),
        tg,
        theme::text(),
    );
    // Key type as a small tag next to the name.
    if let Some(m) = &row.meta {
        let kg = p.layout_no_wrap(m.kind.clone(), theme::medium(11.0), theme::text_dim());
        let kr = Rect::from_min_size(
            Pos2::new(rect.left() + 72.0 + tw + 10.0, cy - 9.0 - 9.0),
            kg.size() + Vec2::new(14.0, 18.0 - kg.size().y),
        );
        p.rect_filled(kr, CornerRadius::same(9), theme::fill());
        p.galley(kr.center() - kg.size() / 2.0, kg, theme::text_dim());
    }
    let mut sub = if ident.username.is_empty() {
        "—".to_string()
    } else {
        ident.username.clone()
    };
    if let Some(m) = &row.meta {
        let fp = m
            .fingerprint
            .strip_prefix("SHA256:")
            .unwrap_or(&m.fingerprint);
        let short: String = fp.chars().take(16).collect();
        sub.push_str(&format!("  ·  SHA256:{short}…"));
        if !m.comment.is_empty() && m.comment != ident.label {
            sub.push_str(&format!("  ·  {}", m.comment));
        }
    }
    p.text(
        Pos2::new(rect.left() + 72.0, cy + 11.0),
        egui::Align2::LEFT_CENTER,
        sub,
        FontId::monospace(12.5),
        theme::text_dim(),
    );

    let ctx = ui.ctx().clone();
    let copy_public = |toast: &mut Option<(String, egui::Color32)>| match crate::keygen::public_info(
        &ident.private_key,
        &ident.passphrase,
    ) {
        Ok((pubkey, _)) => {
            ctx.copy_text(pubkey);
            *toast = Some((t!("Public Key kopiert").to_string(), theme::accent()));
        }
        Err(e) => *toast = Some((t!("Key ungültig: {}", e), theme::red())),
    };
    let delete = || {
        Action::Confirm(
            t!("Schlüssel „{}“ löschen?", ident.label),
            Box::new(Action::DeleteIdentity(ident.id)),
        )
    };
    // Matched by the German key, displayed translated.
    let mut keys = vec!["Bearbeiten", "Löschen"];
    if !ident.private_key.is_empty() {
        keys.push("Public Key kopieren");
    }
    let labels: Vec<&str> = keys.iter().map(|k| crate::i18n::tr(k)).collect();
    if hovered && !resp.dragged() {
        match row_buttons(ui, rect, ident.id, &labels).map(|i| keys[i]) {
            Some("Public Key kopieren") => copy_public(toast),
            Some("Bearbeiten") => actions.push(Action::EditIdentity(ident.id)),
            Some("Löschen") => actions.push(delete()),
            _ => {}
        }
    } else {
        let usage = match row.used_by {
            0 => t!("nicht verwendet").to_string(),
            1 => t!("1 Host").to_string(),
            n => t!("{} Hosts", n),
        };
        p.text(
            rect.right_center() - Vec2::new(18.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            usage,
            FontId::proportional(13.0),
            theme::text_dim(),
        );
    }
    if resp.double_clicked() {
        actions.push(Action::EditIdentity(ident.id));
    }
    resp.context_menu(|ui| {
        if ui.button(t!("Bearbeiten")).clicked() {
            actions.push(Action::EditIdentity(ident.id));
            ui.close();
        }
        if !ident.private_key.is_empty() && ui.button(t!("Public Key kopieren")).clicked() {
            copy_public(toast);
            ui.close();
        }
        ui.menu_button(t!("In Gruppe verschieben"), |ui| {
            if ui
                .selectable_label(ident.group_id.is_none(), t!("Ohne Gruppe"))
                .clicked()
            {
                actions.push(Action::KeyGroup(KeyGroupOp::Move(ident.id, None)));
                ui.close();
            }
            if !groups.is_empty() {
                ui.separator();
            }
            for (id, name, color) in groups {
                let label = RichText::new(format!("     {name}"));
                let r = ui.selectable_label(ident.group_id == Some(*id), label);
                // Tint the square in the group colour.
                let sq = Rect::from_min_size(
                    r.rect.left_center() + Vec2::new(6.0, -4.5),
                    Vec2::splat(9.0),
                );
                ui.painter()
                    .rect_filled(sq, CornerRadius::same(2), theme::rgb(*color));
                if r.clicked() {
                    actions.push(Action::KeyGroup(KeyGroupOp::Move(ident.id, Some(*id))));
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui
            .button(RichText::new(t!("Löschen")).color(theme::red()))
            .clicked()
        {
            actions.push(delete());
            ui.close();
        }
    });
}

/// Sheet for a new or renamed key group: name and colour.
fn group_editor(app: &mut App, ctx: &egui::Context) {
    let Some((g, is_new)) = app.keys_ui.group_editor.as_mut() else {
        return;
    };
    let title = if *is_new {
        t!("Neue Gruppe")
    } else {
        t!("Gruppe bearbeiten")
    };
    let mut enter = false;
    let ((save, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("key-group-editor"),
        460.0,
        title,
        |ui| {
            theme::form(ui, 90.0, |ui, f| {
                f.field(ui, t!("Name"), |ui| {
                    let r = crate::ui::editors::field_text(ui, &mut g.name, t!("z.B. Produktion"));
                    if *is_new && !r.has_focus() && g.name.is_empty() {
                        r.request_focus();
                    }
                    enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
                f.field(ui, t!("Farbe"), |ui| {
                    let mut c = Some(g.color);
                    crate::ui::editors::color_row(ui, &mut c, false);
                    if let Some(c) = c {
                        g.color = c;
                    }
                });
            });
            !g.name.trim().is_empty()
        },
        crate::ui::editors::save_cancel,
    );
    let valid = !g.name.trim().is_empty();
    if cancel || dismissed {
        app.keys_ui.group_editor = None;
    } else if (save || (enter && valid))
        && let Some((mut g, _)) = app.keys_ui.group_editor.take()
    {
        g.name = g.name.trim().to_string();
        match app.keychain.groups.iter_mut().find(|x| x.id == g.id) {
            Some(x) => *x = g,
            None => app.keychain.groups.push(g),
        }
        app.save_keychain();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_every_field() {
        let mut ident = Identity::new();
        ident.label = "Deploy".into();
        ident.username = "ci".into();
        ident.private_key = "x".into();
        let g = KeyGroup {
            name: "Produktion".into(),
            ..Default::default()
        };
        let m = KeyMeta {
            kind: "Ed25519".into(),
            comment: "ci@build".into(),
            fingerprint: "SHA256:AbCd".into(),
        };
        let hay = haystack(&ident, Some(&g), Some(&m));
        for q in [
            "deploy",
            "CI",
            "ed25519",
            "build",
            "abcd",
            "produktion",
            "deploy prod",
            "key",
        ] {
            assert!(matches(q, &hay), "{q}");
        }
        assert!(!matches("rsa", &hay));
        assert!(!matches("deploy staging", &hay));
        assert!(matches("", &hay));
    }

    #[test]
    fn choices_follow_group_order() {
        let mut kc = Keychain::default();
        let g = KeyGroup {
            name: "Prod".into(),
            ..Default::default()
        };
        let mut a = Identity::new();
        a.label = "zeta".into();
        let mut b = Identity::new();
        b.username = "root".into();
        b.group_id = Some(g.id);
        kc.groups.push(g);
        kc.identities = vec![a.clone(), b.clone()];
        let c = choices(&kc, |_| true);
        assert_eq!(
            c,
            vec![
                (b.id, "root".to_string(), "Prod".to_string()),
                (a.id, "zeta".to_string(), String::new()),
            ]
        );
    }
}
