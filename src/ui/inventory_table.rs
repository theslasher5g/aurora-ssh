//! Table and card layout of the inventory page.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::Action;
use crate::backend::inventory_doc::no_facts_reason;
use crate::backend::inventory_rules::{Col, InventorySettings, Level, Row, cell};
use crate::model::{Env, Protocol};
use crate::theme;
use crate::ui::inventory_page::InvAction;

use crate::ui::nav2_inv::{HEAD_H as GROUP_H, InvCache, Item, ROW_H};

const HEAD_H: f32 = 34.0;
/// Below this width the page shows cards.
const MIN_TABLE: f32 = 620.0;
/// Cards have varying heights and are not virtualised: this many at a time.
const CARD_STEP: usize = 150;

pub fn show(
    ui: &mut egui::Ui,
    cache: &InvCache,
    cols: &[Col],
    s: &mut InventorySettings,
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    actions: &mut Vec<Action>,
) {
    if ui.available_width() < MIN_TABLE {
        cards(ui, cache, s.group_by_group, cols, selected, now, actions);
    } else {
        table(ui, cache, cols, s, selected, now, actions);
    }
}

fn level_color(l: Option<Level>) -> Color32 {
    match l {
        Some(Level::Crit) => theme::red(),
        Some(Level::Warn) => theme::amber(),
        None => theme::green(),
    }
}

fn warning_tip(r: &Row) -> String {
    r.warnings
        .iter()
        .map(|w| format!("• {}", w.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Cell colour: the thresholds decide.
fn cell_color(r: &Row, c: Col, s: &InventorySettings, now: DateTime<Utc>) -> Color32 {
    use crate::backend::inventory_rules::WarnKind as K;
    let has = |k: K| r.warnings.iter().find(|w| w.kind == k).map(|w| w.level);
    match c {
        Col::Disk => match has(K::Disk) {
            Some(l) => level_color(Some(l)),
            None => theme::text(),
        },
        Col::Updates => has(K::Updates).map_or(theme::text(), |_| theme::amber()),
        Col::SecUpdates => has(K::SecurityUpdates).map_or(theme::text(), |_| theme::red()),
        Col::Uptime => has(K::Uptime).map_or(theme::text(), |_| theme::amber()),
        Col::FactsAge => has(K::FactsStale).map_or(theme::text_dim(), |_| theme::amber()),
        Col::Os => has(K::Eol).map_or(theme::text(), |_| theme::red()),
        Col::Failed => {
            if r.failed_services.unwrap_or(0) > 0 {
                theme::red()
            } else {
                theme::text_dim()
            }
        }
        Col::Name | Col::Env => theme::text(),
        _ => {
            let _ = (s, now);
            theme::text_dim()
        }
    }
}

/// Columns that only an SSH refresh can fill.
fn is_fact_col(c: Col) -> bool {
    matches!(
        c,
        Col::Os
            | Col::Kernel
            | Col::Uptime
            | Col::Disk
            | Col::Updates
            | Col::SecUpdates
            | Col::Cpu
            | Col::Ram
            | Col::Load
            | Col::FactsAge
            | Col::Failed
    )
}

/// Hosts that can never have facts (RDP, VNC …) say so once instead of showing dashes.
fn no_facts_row(r: &Row) -> bool {
    !r.has_facts && r.protocol != Protocol::Ssh
}

fn can_files(r: &Row) -> bool {
    matches!(r.protocol, Protocol::Ssh | Protocol::Rdp)
}

// --- table ---------------------------------------------------------------------

fn table(
    ui: &mut egui::Ui,
    cache: &InvCache,
    cols: &[Col],
    s: &mut InventorySettings,
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    actions: &mut Vec<Action>,
) {
    const CHECK_W: f32 = 34.0;
    let by_group = s.group_by_group;
    let avail = ui.available_width();
    // Columns that fit; the rest is dropped from the right.
    let mut shown: Vec<Col> = Vec::new();
    let mut used = CHECK_W + 12.0;
    for c in cols {
        if *c == Col::Name || used + c.width() <= avail {
            used += c.width();
            shown.push(*c);
        }
    }
    let hidden = cols.len() - shown.len();
    let extra = ((avail - used) / shown.len() as f32).clamp(0.0, 50.0);
    let widths: Vec<f32> = shown.iter().map(|c| c.width() + extra).collect();
    let sort = Col::from_key(&s.sort_col).unwrap_or(Col::Name);

    if hidden > 0 {
        ui.label(
            RichText::new(t!("+{} Spalten ausgeblendet (zu wenig Platz)", hidden))
                .size(11.5)
                .color(theme::text_dim()),
        );
    }
    // Header
    let (hrect, _) = ui.allocate_exact_size(Vec2::new(avail, HEAD_H), Sense::hover());
    let mut x = hrect.left() + CHECK_W;
    {
        // "All selected" only needs a full scan when as many rows are selected as shown.
        let n = cache.order.len();
        let mut all =
            n > 0 && selected.len() >= n && cache.rows().all(|r| selected.contains(&r.id));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    Pos2::new(hrect.left() + 10.0, hrect.top()),
                    Vec2::new(CHECK_W, HEAD_H),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if child
            .checkbox(&mut all, "")
            .on_hover_text(t!("Alle auswählen"))
            .changed()
        {
            if all {
                selected.extend(cache.rows().map(|r| r.id));
            } else {
                selected.clear();
            }
        }
    }
    for (c, w) in shown.iter().zip(&widths) {
        let r = Rect::from_min_size(Pos2::new(x, hrect.top()), Vec2::new(*w, HEAD_H));
        let resp = ui.interact(r, ui.id().with(("inv-head", c.key())), Sense::click());
        let arrow = if sort == *c {
            if s.sort_desc { " ▼" } else { " ▲" }
        } else {
            ""
        };
        let color = if sort == *c || resp.hovered() {
            theme::text()
        } else {
            theme::text_dim()
        };
        let g = theme::truncated(
            ui.painter(),
            format!("{}{arrow}", c.label()),
            theme::semibold(12.0),
            color,
            *w - 10.0,
        );
        let gx = if c.numeric() {
            r.right() - 10.0 - g.size().x
        } else {
            r.left() + 4.0
        };
        ui.painter()
            .galley(Pos2::new(gx, r.center().y - g.size().y / 2.0), g, color);
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            if sort == *c {
                s.sort_desc = !s.sort_desc;
            } else {
                s.sort_col = c.key().to_string();
                s.sort_desc = false;
            }
        }
        x += *w;
    }
    // Only the lines in view are drawn (virtual scrolling): the lines above and below
    // are stand-ins of the right height.
    theme::list_card().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let items = &cache.items;
        let n = items.items.len();
        let clip = ui.clip_rect();
        let top = ui.cursor().top();
        let range = crate::nav_tree::visible_range(
            &items.y,
            clip.top() - top - 2.0 * ROW_H,
            clip.bottom() - top + 2.0 * ROW_H,
        );
        let (first, last) = (range.start, range.end.min(n));
        ui.add_space(items.y.get(first).copied().unwrap_or(0.0));
        for i in first..last {
            match items.items[i] {
                Item::Head(h) => {
                    let (name, count) = &items.heads[h as usize];
                    let (r, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), GROUP_H),
                        Sense::hover(),
                    );
                    ui.painter()
                        .rect_filled(r, CornerRadius::ZERO, theme::fill());
                    ui.painter().text(
                        Pos2::new(r.left() + 14.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("{name}  ·  {count}"),
                        theme::semibold(12.5),
                        theme::text(),
                    );
                }
                Item::Row(ri) => {
                    let divider = i > 0;
                    table_row(
                        ui,
                        &cache.all[ri as usize],
                        &shown,
                        &widths,
                        s,
                        selected,
                        now,
                        divider || by_group,
                        actions,
                    );
                }
            }
        }
        let total = items.y.last().copied().unwrap_or(0.0);
        let below = total - items.y.get(last).copied().unwrap_or(total);
        ui.add_space(below.max(0.0));
    });
}

#[allow(clippy::too_many_arguments)]
fn table_row(
    ui: &mut egui::Ui,
    r: &Row,
    cols: &[Col],
    widths: &[f32],
    s: &InventorySettings,
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    divider: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let sel = selected.contains(&r.id);
    if sel {
        p.rect_filled(rect, CornerRadius::ZERO, theme::accent_soft());
    } else if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if divider {
        p.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, theme::line()),
        );
    }
    let mut x = rect.left() + 34.0;
    let merge = no_facts_row(r);
    let mut i = 0;
    while i < cols.len() {
        if merge && is_fact_col(cols[i]) {
            // One note over the whole run of fact columns.
            let mut j = i;
            let mut total = 0.0;
            while j < cols.len() && is_fact_col(cols[j]) {
                total += widths[j];
                j += 1;
            }
            let note_rect = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(total, ROW_H));
            let note = no_facts_reason(r.protocol);
            let g = theme::truncated(
                &p,
                note.clone(),
                theme::medium(12.0),
                theme::text_dim(),
                total - 10.0,
            );
            p.galley(
                Pos2::new(
                    note_rect.left() + 4.0,
                    note_rect.center().y - g.size().y / 2.0,
                ),
                g,
                theme::text_dim(),
            );
            ui.interact(
                note_rect,
                ui.id().with(("inv-nofacts", r.id)),
                Sense::hover(),
            )
            .on_hover_text(note);
            x += total;
            i = j;
            continue;
        }
        let cell_rect = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(widths[i], ROW_H));
        paint_cell(ui, &p, r, cols[i], cell_rect, s, now);
        x += widths[i];
        i += 1;
    }
    // Checkbox
    {
        let mut v = sel;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    Pos2::new(rect.left() + 10.0, rect.top()),
                    Vec2::new(26.0, ROW_H),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if child.checkbox(&mut v, "").changed() {
            if v {
                selected.insert(r.id);
            } else {
                selected.remove(&r.id);
            }
        }
    }
    if hovered {
        let strip_w = 210.0;
        theme::fade_cover(
            &p,
            Rect::from_min_max(
                Pos2::new(rect.right() - strip_w - 12.0, rect.top() + 1.0),
                rect.right_bottom(),
            ),
            16.0,
            &[
                theme::surface(),
                if sel {
                    theme::accent_soft()
                } else {
                    theme::hover()
                },
            ],
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - strip_w - 8.0, rect.top()),
                    rect.right_bottom() - Vec2::new(8.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 6.0;
        if theme::btn(&mut child, theme::Btn::Small, None, t!("Bearbeiten")).clicked() {
            actions.push(Action::EditHost(r.id));
        }
        if can_files(r) && theme::btn(&mut child, theme::Btn::Small, None, t!("Dateien")).clicked()
        {
            actions.push(Action::OpenSftp(r.id));
        }
        if theme::btn(&mut child, theme::Btn::SmallPrimary, None, t!("Verbinden")).clicked() {
            actions.push(Action::Connect(r.id));
        }
    } else if !r.warnings.is_empty() {
        // Hover over the row's left part explains the badge.
        ui.interact(
            Rect::from_min_size(
                Pos2::new(rect.left() + 34.0, rect.top()),
                Vec2::new(24.0, ROW_H),
            ),
            ui.id().with(("inv-badge", r.id)),
            Sense::hover(),
        )
        .on_hover_text(warning_tip(r));
    }
    if resp.clicked() {
        actions.push(Action::Inv(InvAction::Detail(r.id)));
    }
    resp.context_menu(|ui| {
        if ui.button(t!("Details")).clicked() {
            actions.push(Action::Inv(InvAction::Detail(r.id)));
            ui.close();
        }
        if ui.button(t!("Verbinden")).clicked() {
            actions.push(Action::Connect(r.id));
            ui.close();
        }
        if can_files(r) && ui.button(t!("Dateien (SFTP)")).clicked() {
            actions.push(Action::OpenSftp(r.id));
            ui.close();
        }
        if ui.button(t!("Bearbeiten")).clicked() {
            actions.push(Action::EditHost(r.id));
            ui.close();
        }
        if r.protocol == Protocol::Ssh && ui.button(t!("Fakten aktualisieren")).clicked() {
            actions.push(Action::Inv(crate::ui::inventory_page::InvAction::Refresh(
                vec![r.id],
            )));
            ui.close();
        }
    });
}

fn paint_cell(
    ui: &egui::Ui,
    p: &egui::Painter,
    r: &Row,
    c: Col,
    rect: Rect,
    s: &InventorySettings,
    now: DateTime<Utc>,
) {
    let inner_w = rect.width() - 10.0;
    if c == Col::Name {
        let mut x = rect.left() + 4.0;
        if !r.warnings.is_empty() {
            let col = level_color(r.level());
            let center = Pos2::new(x + 8.0, rect.top() + 17.0);
            p.circle_filled(center, 8.0, theme::soft(col));
            p.text(
                center,
                egui::Align2::CENTER_CENTER,
                r.warnings.len().to_string(),
                theme::bold(10.5),
                col,
            );
            x += 22.0;
        }
        let g = theme::truncated(
            p,
            r.name.clone(),
            theme::semibold(14.0),
            theme::text(),
            rect.right() - x - 8.0,
        );
        let y1 = if r.warnings.is_empty() {
            rect.center().y
        } else {
            rect.top() + 17.0
        };
        p.galley(Pos2::new(x, y1 - g.size().y / 2.0), g, theme::text());
        if let Some(w) = r.warnings.first() {
            let more = if r.warnings.len() > 1 {
                format!("  (+{})", r.warnings.len() - 1)
            } else {
                String::new()
            };
            let col = level_color(Some(w.level));
            let g = theme::truncated(
                p,
                format!("{}{more}", w.text),
                FontId::proportional(11.5),
                col,
                rect.right() - rect.left() - 12.0 + 18.0,
            );
            p.galley(
                Pos2::new(rect.left() + 4.0, rect.top() + 35.0 - g.size().y / 2.0),
                g,
                col,
            );
        }
        let _ = ui;
        return;
    }
    if c == Col::Env {
        if r.env != Env::None {
            let col = theme::env_color(r.env);
            let g = p.layout_no_wrap(r.env.label().to_uppercase(), theme::bold(10.0), col);
            let w = g.size().x + 12.0;
            let pr = Rect::from_min_size(
                Pos2::new(rect.left() + 4.0, rect.center().y - 8.0),
                Vec2::new(w.min(inner_w), 16.0),
            );
            p.rect_filled(pr, CornerRadius::same(5), theme::soft(col));
            p.galley(pr.center() - g.size() / 2.0, g, col);
        }
        return;
    }
    let text = cell(r, c, now);
    if text.is_empty() {
        let dash = if r.has_facts || !matches!(c, Col::Os | Col::Disk | Col::Updates | Col::Uptime)
        {
            "–"
        } else {
            ""
        };
        if !dash.is_empty() {
            let x = if c.numeric() {
                rect.right() - 10.0
            } else {
                rect.left() + 4.0
            };
            p.text(
                Pos2::new(x, rect.center().y),
                if c.numeric() {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::LEFT_CENTER
                },
                dash,
                FontId::proportional(12.5),
                theme::text_dim().gamma_multiply(0.5),
            );
        }
        return;
    }
    let color = cell_color(r, c, s, now);
    let font = if matches!(c, Col::Address | Col::Kernel) {
        FontId::monospace(12.0)
    } else {
        FontId::proportional(13.0)
    };
    let g = theme::truncated(p, text, font, color, inner_w);
    let x = if c.numeric() {
        rect.right() - 10.0 - g.size().x
    } else {
        rect.left() + 4.0
    };
    p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, color);
}

// --- cards ---------------------------------------------------------------------

fn cards(
    ui: &mut egui::Ui,
    cache: &InvCache,
    by_group: bool,
    cols: &[Col],
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    actions: &mut Vec<Action>,
) {
    let limit_id = ui.id().with("inv-card-limit");
    let limit: usize = ui
        .ctx()
        .data(|d| d.get_temp(limit_id))
        .unwrap_or(CARD_STEP)
        .max(CARD_STEP);
    let mut shown_n = 0usize;
    for it in &cache.items.items {
        if shown_n >= limit {
            break;
        }
        let r: &Row = match *it {
            Item::Head(h) => {
                let (name, count) = &cache.items.heads[h as usize];
                if by_group {
                    theme::section_title(ui, &format!("{name} · {count}"));
                    ui.add_space(6.0);
                }
                continue;
            }
            Item::Row(i) => &cache.all[i as usize],
        };
        shown_n += 1;
        {
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal_wrapped(|ui| {
                    let mut v = selected.contains(&r.id);
                    if ui.checkbox(&mut v, "").changed() {
                        if v {
                            selected.insert(r.id);
                        } else {
                            selected.remove(&r.id);
                        }
                    }
                    ui.label(RichText::new(&r.name).font(theme::semibold(15.0)));
                    if r.env != Env::None {
                        theme::pill(ui, r.env.label(), theme::env_color(r.env));
                    }
                    if !r.warnings.is_empty() {
                        let col = level_color(r.level());
                        ui.label(
                            RichText::new(format!("● {}", r.warnings.len()))
                                .size(12.5)
                                .color(col),
                        )
                        .on_hover_text(warning_tip(r));
                    }
                });
                if no_facts_row(r) {
                    ui.label(
                        RichText::new(no_facts_reason(r.protocol))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                }
                let mut facts = Vec::new();
                for c in cols {
                    if no_facts_row(r) && is_fact_col(*c) {
                        continue;
                    }
                    if *c == Col::Name || *c == Col::Env {
                        continue;
                    }
                    let t = cell(r, *c, now);
                    if !t.is_empty() {
                        facts.push(format!("{}: {t}", c.label()));
                    }
                }
                if !facts.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(facts.join("  ·  "))
                                .size(12.5)
                                .color(theme::text_dim()),
                        )
                        .wrap(),
                    );
                }
                for w in &r.warnings {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&w.text)
                                .size(12.0)
                                .color(level_color(Some(w.level))),
                        )
                        .wrap(),
                    );
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Verbinden")).clicked() {
                        actions.push(Action::Connect(r.id));
                    }
                    if theme::btn(ui, theme::Btn::Small, None, t!("Details")).clicked() {
                        actions.push(Action::Inv(InvAction::Detail(r.id)));
                    }
                    if can_files(r)
                        && theme::btn(ui, theme::Btn::Small, None, t!("Dateien")).clicked()
                    {
                        actions.push(Action::OpenSftp(r.id));
                    }
                    if theme::btn(ui, theme::Btn::Small, None, t!("Bearbeiten")).clicked() {
                        actions.push(Action::EditHost(r.id));
                    }
                });
            });
            ui.add_space(8.0);
        }
    }
    let total = cache.order.len();
    if shown_n < total {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(t!("{} von {} Hosts angezeigt", shown_n, total))
                    .size(12.5)
                    .color(theme::text_dim()),
            );
            if theme::btn(ui, theme::Btn::Normal, None, t!("Weitere anzeigen")).clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(limit_id, limit + CARD_STEP));
            }
        });
    }
}
