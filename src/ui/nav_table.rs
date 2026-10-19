//! Dense table alternative to the host list: sortable, resizable and hideable columns,
//! multi-select (Ctrl/Shift+click), the same context menu as the list, virtualised rows.
//! Columns that do not fit are dropped by priority, so it works down to 720 px.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::model::Library;
use crate::nav_model::{Col, TableCfg, ViewMode};
use crate::nav_query::Status;
use crate::nav_tree::{self, natural_cmp};
use crate::theme;
use crate::ui::nav_state::{Cache, NavAction};
use crate::ui::relative_time;

const ROW_H: f32 = 30.0;
const HEAD_H: f32 = 32.0;
const PAD: f32 = 10.0;

#[derive(Debug, Clone)]
pub enum TableOp {
    Sort(Col),
    Toggle(Col),
    Width(Col, f32),
}

#[derive(Default)]
pub struct TableCache {
    /// Bumped to force a re-sort (library or mode changed).
    gen_: u64,
}

impl TableCache {
    pub fn invalidate(&mut self) {
        self.gen_ += 1;
    }

    pub fn gen_value(&self) -> u64 {
        self.gen_
    }

    pub fn mode_is_list(&self, lib: &Library) -> bool {
        lib.nav.mode(lib.nav.scope) == ViewMode::List
    }
}

struct SortedRows {
    key: (u64, u64, Col, bool),
    rows: Vec<u32>,
    ids: Vec<Uuid>,
}

/// Columns that fit into `avail` points, in display order. The name column is always
/// there; the others are dropped by priority (least important first).
pub fn visible_cols(cfg: &TableCfg, avail: f32) -> Vec<Col> {
    let mut want: Vec<Col> = Col::ALL
        .into_iter()
        .filter(|c| !cfg.is_hidden(*c))
        .collect();
    let total = |v: &[Col]| v.iter().map(|c| cfg.width(*c)).sum::<f32>() + PAD * 2.0;
    while total(&want) > avail && want.len() > 1 {
        // Remove the least important column that is not the name.
        let Some((i, _)) = want
            .iter()
            .enumerate()
            .filter(|(_, c)| **c != Col::Name)
            .max_by_key(|(_, c)| c.priority())
        else {
            break;
        };
        want.remove(i);
    }
    want
}

/// Sorts the shown entries by a column (stable, ties by name).
pub fn sort_rows(c: &Cache, lib: &Library, col: Col, desc: bool) -> Vec<u32> {
    let mut rows = c.shown.order.clone();
    let name = |e: u32| &c.idx.entries[e as usize].name_lc;
    rows.sort_by(|a, b| natural_cmp(name(*a), name(*b)));
    let host = |e: u32| &lib.hosts[c.idx.entries[e as usize].host];
    let ent = |e: u32| &c.idx.entries[e as usize];
    match col {
        Col::Name => {}
        Col::Address => rows.sort_by_cached_key(|e| host(*e).hostname.to_lowercase()),
        Col::Kind => rows.sort_by_key(|e| host(*e).protocol.label()),
        Col::Tags => rows.sort_by_cached_key(|e| host(*e).tags.join(",").to_lowercase()),
        Col::Status => rows.sort_by_key(|e| match ent(*e).status {
            _ if ent(*e).live => 0u8,
            Status::Up => 0,
            Status::Down => 1,
            Status::Failed => 2,
            Status::Maint => 3,
            Status::Unknown => 4,
        }),
        Col::Seen => rows.sort_by_key(|e| std::cmp::Reverse(ent(*e).last)),
        Col::Env => rows.sort_by_key(|e| lib.env_of(host(*e)).label()),
        Col::Group => rows.sort_by_cached_key(|e| {
            ent(*e)
                .node
                .map(|n| c.idx.nodes[n].path.join(" / ").to_lowercase())
                .unwrap_or_default()
        }),
    }
    if desc {
        rows.reverse();
    }
    rows
}

fn addr(h: &crate::model::Host) -> &str {
    match h.protocol {
        crate::model::Protocol::Serial => &h.serial.port_name,
        _ if h.protocol.is_container() => &h.container.name,
        _ => &h.hostname,
    }
}

fn status_text(app: &App, st: Status, live: bool, ms: Option<u32>) -> (String, Color32) {
    match (live, st) {
        (true, _) => (t!("Session offen").to_string(), theme::green()),
        (_, Status::Up) => (
            ms.map(|m| format!("{} · {m} ms", t!("online")))
                .unwrap_or_else(|| t!("online").to_string()),
            theme::green(),
        ),
        (_, Status::Down) => (t!("keine Antwort").to_string(), theme::text_dim()),
        (_, Status::Failed) => (t!("fehlgeschlagen").to_string(), theme::red()),
        (_, Status::Maint) => (t!("Wartung").to_string(), theme::amber()),
        (_, Status::Unknown) => (
            if app.lib.settings.reachability {
                "—".to_string()
            } else {
                String::new()
            },
            theme::text_dim(),
        ),
    }
}

pub fn show(app: &App, c: &Cache, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let lib = &app.lib;
    let cfg = &lib.nav.table;
    let (col, desc) = (cfg.sort_col(), cfg.desc);
    // Sorted rows are cached in the egui memory (keyed by what they depend on).
    let id = egui::Id::new("nav-table-rows");
    let key = (c.epoch, app.nav.table.gen_value(), col, desc);
    let sorted: Arc<SortedRows> = match ctx.data(|d| d.get_temp::<Arc<SortedRows>>(id)) {
        Some(s) if s.key == key => s,
        _ => {
            let rows = sort_rows(c, lib, col, desc);
            let ids = rows
                .iter()
                .map(|e| lib.hosts[c.idx.entries[*e as usize].host].id)
                .collect();
            let s = Arc::new(SortedRows { key, rows, ids });
            ctx.data_mut(|d| d.insert_temp(id, s.clone()));
            ctx.data_mut(|d| d.insert_temp(crate::ui::hosts::order_id(), s.ids.clone()));
            s
        }
    };
    let avail = ui.available_width() - 14.0;
    let cols = visible_cols(cfg, avail);
    // Name takes the room the others leave.
    let others: f32 = cols
        .iter()
        .filter(|c| **c != Col::Name)
        .map(|c| cfg.width(*c))
        .sum();
    let name_w = (avail - PAD * 2.0 - others)
        .max(cfg.width(Col::Name).min(avail * 0.5))
        .max(Col::Name.min_width());
    let width_of = |c: Col| if c == Col::Name { name_w } else { cfg.width(c) };

    // --- header
    let (head, _) = ui.allocate_exact_size(Vec2::new(avail, HEAD_H), Sense::hover());
    ui.painter()
        .rect_filled(head, CornerRadius::same(8), theme::fill());
    let mut x = head.left() + PAD;
    for (i, cc) in cols.iter().enumerate() {
        let w = width_of(*cc);
        let cell = Rect::from_min_size(Pos2::new(x, head.top()), Vec2::new(w, HEAD_H));
        let resp = ui.interact(cell, ui.id().with(("th", cc.key())), Sense::click());
        let sorted_here = *cc == col;
        let label = format!(
            "{}{}",
            crate::i18n::tr(cc.label()),
            if sorted_here {
                if desc { " ▼" } else { " ▲" }
            } else {
                ""
            }
        );
        let g = theme::truncated(
            ui.painter(),
            label,
            theme::semibold(12.5),
            if sorted_here {
                theme::text()
            } else {
                theme::text_dim()
            },
            w - 14.0,
        );
        ui.painter().galley(
            Pos2::new(cell.left(), head.center().y - g.size().y / 2.0),
            g,
            theme::text(),
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            actions.push(Action::Nav(NavAction::Table(TableOp::Sort(*cc))));
        }
        ui.interact(cell, ui.id().with(("th", cc.key())), Sense::click())
            .context_menu(|ui| column_menu(cfg, ui, actions));
        // Drag handle between this and the next column.
        if i + 1 < cols.len() || *cc != Col::Name {
            let h = Rect::from_min_size(
                Pos2::new(cell.right() - 4.0, head.top() + 4.0),
                Vec2::new(8.0, HEAD_H - 8.0),
            );
            let hr = ui.interact(h, ui.id().with(("thr", cc.key())), Sense::drag());
            if hr.hovered() || hr.dragged() {
                ui.painter().line_segment(
                    [
                        Pos2::new(cell.right(), h.top()),
                        Pos2::new(cell.right(), h.bottom()),
                    ],
                    Stroke::new(2.0, theme::accent()),
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            }
            if hr.dragged() && *cc != Col::Name {
                actions.push(Action::Nav(NavAction::Table(TableOp::Width(
                    *cc,
                    w + hr.drag_delta().x,
                ))));
            } else if hr.dragged() {
                // Dragging the name edge resizes the next column the other way.
                if let Some(next) = cols.get(i + 1) {
                    actions.push(Action::Nav(NavAction::Table(TableOp::Width(
                        *next,
                        cfg.width(*next) - hr.drag_delta().x,
                    ))));
                }
            }
        }
        x += w;
    }
    ui.add_space(4.0);

    // --- body
    let n = sorted.rows.len();
    egui::ScrollArea::vertical()
        .id_salt("nav-table")
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let (full, _) =
                ui.allocate_exact_size(Vec2::new(avail, n as f32 * ROW_H + 30.0), Sense::hover());
            let first = (viewport.top() / ROW_H).floor().max(0.0) as usize;
            let last = ((viewport.bottom() / ROW_H).ceil() as usize + 1).min(n);
            let reach = lib.settings.reachability;
            let _ = reach;
            for i in first..last {
                let e = sorted.rows[i];
                let en = &c.idx.entries[e as usize];
                let h = &lib.hosts[en.host];
                let rect = Rect::from_min_size(
                    Pos2::new(full.left(), full.top() + i as f32 * ROW_H),
                    Vec2::new(avail, ROW_H),
                );
                let resp = ui.interact(rect, ui.id().with(("tr", i)), Sense::click_and_drag());
                let selected = app.selected_hosts.contains(&h.id);
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, CornerRadius::same(6), theme::accent_soft());
                } else if resp.hovered() {
                    p.rect_filled(rect, CornerRadius::same(6), theme::hover());
                } else if i % 2 == 1 {
                    p.rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha(3));
                }
                resp.dnd_set_drag_payload(h.id);
                let mut cx = rect.left() + PAD;
                for cc in &cols {
                    let w = width_of(*cc);
                    let max = (w - 10.0).max(10.0);
                    let cy = rect.center().y;
                    let (text, color, font): (String, Color32, FontId) = match cc {
                        Col::Name => {
                            let dot = match (en.live, en.status) {
                                (true, _) | (_, Status::Up) => theme::green(),
                                (_, Status::Failed) => theme::red(),
                                (_, Status::Maint) => theme::amber(),
                                _ => theme::off_color(),
                            };
                            theme::led(p, Pos2::new(cx + 5.0, cy), dot, dot != theme::off_color());
                            let g = theme::truncated(
                                p,
                                h.display_name(),
                                theme::semibold(13.5),
                                theme::text(),
                                max - 18.0,
                            );
                            p.galley(
                                Pos2::new(cx + 18.0, cy - g.size().y / 2.0),
                                g,
                                theme::text(),
                            );
                            cx += w;
                            continue;
                        }
                        Col::Address => {
                            let port = if h.port == h.protocol.default_port()
                                || !h.protocol.is_network()
                            {
                                String::new()
                            } else {
                                format!(":{}", h.port)
                            };
                            (
                                format!("{}{}", addr(h), port),
                                theme::text_dim(),
                                FontId::monospace(12.0),
                            )
                        }
                        Col::Kind => (
                            crate::ui::hosts::protocol_tag(h.protocol).to_string(),
                            crate::ui::hosts::protocol_color(h.protocol),
                            theme::medium(12.0),
                        ),
                        Col::Tags => (h.tags.join(", "), theme::text_dim(), theme::medium(12.0)),
                        Col::Status => {
                            let ms = if reach {
                                match app.reach.get(h.id) {
                                    Some(crate::backend::reach::Reach::Up(ms)) => Some(ms),
                                    _ => None,
                                }
                            } else {
                                None
                            };
                            let (t, col) = status_text(app, en.status, en.live, ms);
                            (t, col, theme::medium(12.5))
                        }
                        Col::Seen => (
                            match (en.status, h.last_connected) {
                                (Status::Up, _) => t!("jetzt").to_string(),
                                (_, Some(t)) => relative_time(t),
                                _ => "—".to_string(),
                            },
                            theme::text_dim(),
                            theme::medium(12.5),
                        ),
                        Col::Env => {
                            let e = lib.env_of(h);
                            if e == crate::model::Env::None {
                                ("—".to_string(), theme::text_dim(), theme::medium(12.0))
                            } else {
                                (
                                    e.label().to_string(),
                                    theme::env_color(e),
                                    theme::bold(11.5),
                                )
                            }
                        }
                        Col::Group => (
                            en.node
                                .map(|nd| c.idx.nodes[nd].path.join(" / "))
                                .unwrap_or_default(),
                            theme::text_dim(),
                            theme::medium(12.5),
                        ),
                    };
                    if !text.is_empty() {
                        let g = theme::truncated(p, text, font, color, max);
                        p.galley(Pos2::new(cx, cy - g.size().y / 2.0), g, color);
                    }
                    cx += w;
                }
                if resp.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    let mode = if m.shift {
                        2
                    } else if m.command {
                        1
                    } else {
                        0
                    };
                    if mode != 0 || !app.selected_hosts.is_empty() {
                        actions.push(Action::SelectHost(h.id, mode));
                    }
                }
                if resp.double_clicked() {
                    actions.push(Action::Connect(h.id));
                }
                resp.context_menu(|ui| {
                    let maint = crate::ui::maintenance::active_for(lib, h.id, chrono::Utc::now());
                    crate::ui::hosts::host_menu(ui, app, h, maint, actions);
                });
            }
        });
    let _ = (RichText::new(""), nav_tree::visible_range);
}

fn column_menu(cfg: &TableCfg, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.label(
        RichText::new(t!("Spalten"))
            .font(theme::semibold(12.5))
            .color(theme::text_dim()),
    );
    for c in Col::ALL {
        let mut on = !cfg.is_hidden(c);
        if ui
            .add_enabled(
                c != Col::Name,
                egui::Checkbox::new(&mut on, crate::i18n::tr(c.label())),
            )
            .clicked()
        {
            actions.push(Action::Nav(NavAction::Table(TableOp::Toggle(c))));
        }
    }
    ui.label(
        RichText::new(t!("Schmale Fenster blenden Spalten automatisch aus"))
            .size(11.5)
            .color(theme::text_dim()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_by_priority_when_narrow() {
        let cfg = TableCfg::default();
        let all = visible_cols(&cfg, 2000.0);
        assert_eq!(all.len(), Col::ALL.len());
        let narrow = visible_cols(&cfg, 560.0);
        assert!(narrow.contains(&Col::Name));
        assert!(!narrow.contains(&Col::Group), "least important goes first");
        assert!(narrow.contains(&Col::Status));
        let tiny = visible_cols(&cfg, 100.0);
        assert_eq!(tiny, vec![Col::Name], "the name always stays");
        // Display order stays canonical.
        let pos = |v: &[Col], c: Col| v.iter().position(|x| *x == c);
        assert!(pos(&narrow, Col::Name) < pos(&narrow, Col::Status));
    }

    #[test]
    fn hidden_columns_stay_hidden() {
        let mut cfg = TableCfg::default();
        cfg.toggle(Col::Tags);
        assert!(!visible_cols(&cfg, 2000.0).contains(&Col::Tags));
    }
}
