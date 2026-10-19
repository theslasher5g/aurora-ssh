//! Table and card layout of the inventory page.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::Action;
use crate::backend::inventory_rules::{Col, InventorySettings, Level, Row, cell};
use crate::model::{Env, Protocol};
use crate::theme;

const ROW_H: f32 = 48.0;
const HEAD_H: f32 = 34.0;
/// Below this width the page shows cards.
const MIN_TABLE: f32 = 620.0;

pub fn show(
    ui: &mut egui::Ui,
    rows: &[Row],
    cols: &[Col],
    s: &mut InventorySettings,
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    actions: &mut Vec<Action>,
) {
    let groups = grouped(rows, s.group_by_group);
    if ui.available_width() < MIN_TABLE {
        cards(ui, &groups, s.group_by_group, cols, selected, now, actions);
    } else {
        table(
            ui,
            &groups,
            s.group_by_group,
            cols,
            s,
            selected,
            now,
            actions,
        );
    }
}

/// `(group name, rows)`; one unnamed group when grouping is off.
fn grouped(rows: &[Row], by_group: bool) -> Vec<(String, Vec<&Row>)> {
    if !by_group {
        return vec![(String::new(), rows.iter().collect())];
    }
    let mut out: Vec<(String, Vec<&Row>)> = Vec::new();
    for r in rows {
        match out.iter_mut().find(|(g, _)| *g == r.group) {
            Some((_, v)) => v.push(r),
            None => out.push((r.group.clone(), vec![r])),
        }
    }
    out.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
    });
    for (g, _) in out.iter_mut() {
        if g.is_empty() {
            *g = t!("Ohne Gruppe").to_string();
        }
    }
    out
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
        Col::Name | Col::Env => theme::text(),
        _ => {
            let _ = (s, now);
            theme::text_dim()
        }
    }
}

fn can_files(r: &Row) -> bool {
    matches!(r.protocol, Protocol::Ssh | Protocol::Rdp)
}

// --- table ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn table(
    ui: &mut egui::Ui,
    groups: &[(String, Vec<&Row>)],
    by_group: bool,
    cols: &[Col],
    s: &mut InventorySettings,
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    actions: &mut Vec<Action>,
) {
    const CHECK_W: f32 = 34.0;
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
    let all_rows: Vec<&Row> = groups.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    {
        let mut all = !all_rows.is_empty() && all_rows.iter().all(|r| selected.contains(&r.id));
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
                selected.extend(all_rows.iter().map(|r| r.id));
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
    theme::list_card().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (gi, (name, items)) in groups.iter().enumerate() {
            if by_group {
                let (r, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
                ui.painter()
                    .rect_filled(r, CornerRadius::ZERO, theme::fill());
                ui.painter().text(
                    Pos2::new(r.left() + 14.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("{name}  ·  {}", items.len()),
                    theme::semibold(12.5),
                    theme::text(),
                );
            }
            for (i, r) in items.iter().enumerate() {
                table_row(
                    ui,
                    r,
                    &shown,
                    &widths,
                    s,
                    selected,
                    now,
                    i > 0 || gi > 0 || by_group,
                    actions,
                );
            }
        }
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
    for (c, w) in cols.iter().zip(widths) {
        let cell_rect = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(*w, ROW_H));
        paint_cell(ui, &p, r, *c, cell_rect, s, now);
        x += *w;
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
        actions.push(Action::EditHost(r.id));
    }
    resp.context_menu(|ui| {
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
    groups: &[(String, Vec<&Row>)],
    by_group: bool,
    cols: &[Col],
    selected: &mut HashSet<Uuid>,
    now: DateTime<Utc>,
    actions: &mut Vec<Action>,
) {
    for (name, items) in groups {
        if by_group {
            theme::section_title(ui, &format!("{name} · {}", items.len()));
            ui.add_space(6.0);
        }
        for r in items {
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
                let mut facts = Vec::new();
                for c in cols {
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
}
