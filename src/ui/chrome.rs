//! Window chrome: the port rail on the left and the slim session header.
//!
//! Open sessions live in the rail as numbered "ports" (the number is the Ctrl+N shortcut),
//! each with a status LED, instead of a tab strip.

use egui::{Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::{Action, App, ConnState, TabKind, View};
use crate::theme;

const RAIL_WIDTH: f32 = 64.0;
const TILE: f32 = 40.0;

/// Two-letter label for a session tile.
pub fn initials(title: &str) -> String {
    let clean: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '.' || *c == '_')
        .collect();
    let words: Vec<&str> = clean
        .split(|c: char| c.is_whitespace() || c == '-' || c == '.' || c == '_')
        .filter(|w| !w.is_empty())
        .collect();
    let s: String = match words.as_slice() {
        [] => "··".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

fn rail_button(
    ui: &mut egui::Ui,
    glyph: &str,
    tooltip: &str,
    selected: bool,
    tint: Option<Color32>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(TILE), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(8), theme::bg_hover());
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), theme::bg_card());
    }
    let color = tint.unwrap_or(if selected {
        theme::accent()
    } else if resp.hovered() {
        theme::text()
    } else {
        theme::text_dim()
    });
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(17.0),
        color,
    );
    resp.on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[allow(clippy::too_many_arguments)]
fn port_tile(
    ui: &mut egui::Ui,
    in_split: bool,
    index: usize,
    label: &str,
    cable: Color32,
    led: Color32,
    live: bool,
    active: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(TILE), Sense::click());
    let p = ui.painter();
    let stroke = if active {
        Stroke::new(1.5, theme::accent())
    } else if resp.hovered() {
        Stroke::new(1.0, theme::text_dim())
    } else {
        Stroke::new(1.0, theme::border())
    };
    p.rect(
        rect,
        CornerRadius::same(8),
        if active {
            theme::bg_card()
        } else {
            Color32::TRANSPARENT
        },
        stroke,
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center() + Vec2::new(0.0, 1.0),
        egui::Align2::CENTER_CENTER,
        label,
        theme::mono_medium(13.0),
        if active {
            theme::text()
        } else {
            theme::text_dim()
        },
    );
    if index <= 9 {
        p.text(
            rect.left_bottom() + Vec2::new(5.0, -3.0),
            egui::Align2::LEFT_BOTTOM,
            index.to_string(),
            FontId::monospace(8.5),
            theme::text_dim().linear_multiply(0.8),
        );
    }
    // Cable colour = group colour of the host.
    p.line_segment(
        [
            rect.center_bottom() + Vec2::new(-6.0, -5.0),
            rect.center_bottom() + Vec2::new(6.0, -5.0),
        ],
        Stroke::new(2.0, cable.linear_multiply(if active { 1.0 } else { 0.7 })),
    );
    theme::led(p, rect.right_top() + Vec2::new(-7.0, 7.0), led, live);
    if in_split {
        p.rect_filled(
            Rect::from_min_size(
                rect.left_top() + Vec2::new(-8.0, 8.0),
                Vec2::new(2.0, TILE - 16.0),
            ),
            CornerRadius::same(1),
            theme::accent(),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Left rail: navigation, open sessions as ports, global toggles.
pub fn rail(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    egui::Panel::left("rail")
        .exact_size(RAIL_WIDTH)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(Margin::symmetric(12, 14)),
        )
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                theme::logo(ui, 30.0).on_hover_text("Aurora");
                ui.add_space(14.0);

                let home = app.active.is_none();
                for (view, glyph, tip) in [
                    (View::Hosts, "🖧", "Verbindungen"),
                    (View::Keychain, "🔑", "Schlüssel & Passwörter"),
                    (View::Files, "🗀", "Dateien (SFTP)"),
                    (View::Snippets, "⚡", "Snippets"),
                ] {
                    if rail_button(ui, glyph, tip, home && app.view == view, None).clicked() {
                        actions.push(Action::Navigate(view));
                    }
                }

                if !app.tabs.is_empty() {
                    ui.add_space(8.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::new(24.0, 1.0), Sense::hover());
                    ui.painter()
                        .rect_filled(r, CornerRadius::ZERO, theme::border());
                    ui.add_space(8.0);
                }

                let mut activate = None;
                let mut close = None;
                let mut reconnect = None;
                for (i, tab) in app.tabs.iter().enumerate() {
                    let (state_color, live, state) = match &tab.kind {
                        TabKind::Term(t) => (
                            crate::app::state_color(&t.state),
                            matches!(t.state, ConnState::Connected),
                            state_text(&t.state),
                        ),
                        TabKind::Sftp(t) => (
                            crate::app::state_color(t.state()),
                            matches!(t.state(), ConnState::Connected),
                            state_text(t.state()),
                        ),
                        TabKind::Rdp(t) => (
                            t.status_color(),
                            t.is_live(),
                            if t.is_live() {
                                "verbunden".into()
                            } else {
                                "getrennt".into()
                            },
                        ),
                    };
                    let kind = match &tab.kind {
                        TabKind::Term(_) => "Terminal",
                        TabKind::Sftp(_) => "Dateien",
                        TabKind::Rdp(_) => "Desktop",
                    };
                    let resp = port_tile(
                        ui,
                        app.split.len() >= 2 && app.split.contains(&tab.id),
                        i + 1,
                        &initials(&tab.title),
                        tab.color,
                        state_color,
                        live,
                        app.active == Some(tab.id),
                    );
                    let resp = resp.on_hover_ui(|ui| {
                        ui.label(theme::heading(&tab.title, 13.5));
                        ui.label(theme::mono(&tab.subtitle, 12.0).color(theme::text_dim()));
                        ui.label(
                            RichText::new(format!("{kind} · {state}"))
                                .size(12.0)
                                .color(theme::text_dim()),
                        );
                    });
                    if resp.clicked() {
                        activate = Some(tab.id);
                    }
                    if resp.middle_clicked() {
                        close = Some(tab.id);
                    }
                    resp.context_menu(|ui| {
                        if let Some(a) = app.active.filter(|a| *a != tab.id)
                            && !app.split.contains(&tab.id)
                            && app.split.len() < MAX_SPLIT
                            && ui.button("Neben aktuelle Session legen").clicked()
                        {
                            push_split_op(ui.ctx(), SplitOp::Add(a, tab.id));
                            ui.close();
                        }
                        if ui.button("Neu verbinden").clicked() {
                            reconnect = Some(tab.id);
                            ui.close();
                        }
                        if ui.button("Schließen").clicked() {
                            close = Some(tab.id);
                            ui.close();
                        }
                    });
                }
                if let Some(id) = activate {
                    app.active = Some(id);
                }
                if let Some(id) = reconnect {
                    actions.push(Action::Reconnect(id));
                }
                if let Some(id) = close {
                    app.close_tab(id);
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    if rail_button(ui, "🔒", "Sperren", false, None).clicked() {
                        actions.push(Action::Lock);
                    }
                    if let Some(rel) = &app.update.latest {
                        let tip = format!("Update auf {} verfügbar", rel.version_text());
                        if rail_button(ui, "⬆", &tip, false, Some(theme::accent())).clicked() {
                            actions.push(Action::Navigate(View::Settings));
                        }
                    }
                    if rail_button(
                        ui,
                        "⚙",
                        "Einstellungen",
                        home && app.view == View::Settings,
                        None,
                    )
                    .clicked()
                    {
                        actions.push(Action::Navigate(View::Settings));
                    }
                    let tip = if app.broadcast {
                        "Broadcast an: Eingaben gehen an alle Terminals"
                    } else {
                        "Broadcast: Eingaben an alle Terminals senden"
                    };
                    if rail_button(
                        ui,
                        "📡",
                        tip,
                        app.broadcast,
                        app.broadcast.then(theme::pink),
                    )
                    .clicked()
                    {
                        app.broadcast = !app.broadcast;
                    }
                });
            });
        });
}

pub fn state_text(s: &ConnState) -> String {
    match s {
        ConnState::Connecting(_) => "verbindet".into(),
        ConnState::Connected => "verbunden".into(),
        ConnState::Closed(Some(_)) => "Fehler".into(),
        ConnState::Closed(None) => "getrennt".into(),
    }
}

/// Status label and LED colour for a session header.
pub fn state_pair(s: &ConnState) -> (String, Color32) {
    match s {
        ConnState::Connecting(m) => (m.clone(), theme::amber()),
        ConnState::Connected => ("verbunden".into(), theme::green()),
        ConnState::Closed(Some(e)) => (e.clone(), theme::red()),
        ConnState::Closed(None) => ("getrennt".into(), theme::text_dim()),
    }
}

/// Which sessions exist and which are shown side by side; published by the app each
/// frame so every session header can offer the split menu.
#[derive(Clone, Default)]
pub struct SplitInfo {
    pub split: Vec<u64>,
    pub tabs: Vec<(u64, String)>,
}

#[derive(Clone, Copy, Debug)]
pub enum SplitOp {
    /// Show `other` next to `base`.
    Add(u64, u64),
    Remove(u64),
    Clear,
}

pub const MAX_SPLIT: usize = 4;

pub fn split_info_id() -> egui::Id {
    egui::Id::new("aurora-split-info")
}

pub fn split_op_id() -> egui::Id {
    egui::Id::new("aurora-split-op")
}

fn push_split_op(ctx: &egui::Context, op: SplitOp) {
    ctx.data_mut(|d| d.insert_temp(split_op_id(), op));
}

/// Painted "two panes" icon button.
fn split_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), theme::bg_hover());
    }
    let color = if on {
        theme::accent()
    } else if resp.hovered() {
        theme::text()
    } else {
        theme::text_dim()
    };
    let r = Rect::from_center_size(rect.center(), Vec2::new(14.0, 11.0));
    let p = ui.painter();
    p.rect_stroke(
        r,
        CornerRadius::same(2),
        Stroke::new(1.3, color),
        egui::StrokeKind::Inside,
    );
    p.line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.3, color));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Header button: put another session next to this one (up to four).
fn split_menu(ui: &mut egui::Ui, index: usize) {
    let info: SplitInfo = ui
        .ctx()
        .data(|d| d.get_temp(split_info_id()))
        .unwrap_or_default();
    let Some(&(me, _)) = info.tabs.get(index.wrapping_sub(1)) else {
        return;
    };
    let in_split = info.split.len() >= 2 && info.split.contains(&me);
    let resp = split_button(ui, in_split).on_hover_text("Nebeneinander anzeigen");
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(220.0);
        let shown: Vec<u64> = if in_split {
            info.split.clone()
        } else {
            vec![me]
        };
        let others: Vec<&(u64, String)> = info
            .tabs
            .iter()
            .filter(|(id, _)| !shown.contains(id))
            .collect();
        if shown.len() < MAX_SPLIT {
            if others.is_empty() {
                ui.label(
                    RichText::new("Öffne eine weitere Session, um sie hier daneben zu legen.")
                        .size(12.5)
                        .color(theme::text_dim()),
                );
            }
            for (id, title) in others {
                if ui.button(format!("Daneben: {title}")).clicked() {
                    push_split_op(ui.ctx(), SplitOp::Add(me, *id));
                    ui.close();
                }
            }
        } else {
            ui.label(
                RichText::new("Maximal vier Sessions nebeneinander.")
                    .size(12.5)
                    .color(theme::text_dim()),
            );
        }
        if in_split {
            ui.separator();
            if ui.button("Diese Session lösen").clicked() {
                push_split_op(ui.ctx(), SplitOp::Remove(me));
                ui.close();
            }
            if ui.button("Split beenden").clicked() {
                push_split_op(ui.ctx(), SplitOp::Clear);
                ui.close();
            }
        }
    });
}

/// Pane rectangles for 1–4 sessions: side by side, 1+2 or a 2×2 grid.
pub fn split_rects(r: Rect, n: usize) -> Vec<Rect> {
    let g = 1.0;
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let mx = r.center().x;
    let my = r.center().y;
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    };
    match n {
        0 | 1 => vec![r],
        2 => vec![rect(l, t, mx - g, b), rect(mx + g, t, rr, b)],
        3 => vec![
            rect(l, t, mx - g, b),
            rect(mx + g, t, rr, my - g),
            rect(mx + g, my + g, rr, b),
        ],
        _ => vec![
            rect(l, t, mx - g, my - g),
            rect(mx + g, t, rr, my - g),
            rect(l, my + g, mx - g, b),
            rect(mx + g, my + g, rr, b),
        ],
    }
}

/// Slim header above a session: port number, name, address, state and actions.
pub fn session_header(
    ui: &mut egui::Ui,
    id: egui::Id,
    index: usize,
    title: &str,
    subtitle: &str,
    state: (String, Color32),
    right: impl FnOnce(&mut egui::Ui),
) {
    egui::Panel::top(id)
        .exact_size(46.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_deep())
                .inner_margin(Margin::symmetric(18, 0)),
        )
        .show(ui, |ui| {
            // Right side first, so the title/address get whatever space is left and
            // truncate instead of running under the buttons in narrow split panes.
            let narrow = ui.available_width() < 640.0;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                split_menu(ui, index);
                right(ui);
                ui.add_space(10.0);
                let (label, color) = state;
                if !narrow {
                    ui.add(
                        egui::Label::new(RichText::new(&label).size(12.5).color(theme::text_dim()))
                            .truncate(),
                    );
                }
                let (r, resp) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), Sense::hover());
                theme::led(ui.painter(), r.center(), color, true);
                resp.on_hover_text(label);
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(
                        RichText::new(format!("{index:02}"))
                            .font(theme::mono_medium(12.5))
                            .color(theme::accent()),
                    );
                    ui.add(egui::Label::new(theme::heading(title, 15.0)).truncate());
                    if !narrow {
                        ui.add(
                            egui::Label::new(theme::mono(subtitle, 12.5).color(theme::text_dim()))
                                .truncate(),
                        );
                    }
                });
            });
        });
}

/// Paints a horizontal accent band (e.g. broadcast mode) at the top of `rect`.
pub fn band(painter: &egui::Painter, rect: Rect, color: Color32) {
    painter.rect_filled(
        Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + 2.0)),
        CornerRadius::ZERO,
        color,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn initials_are_short() {
        assert_eq!(super::initials("web-01"), "W0");
        assert_eq!(super::initials("Raspberry Pi"), "RP");
        assert_eq!(super::initials("db"), "DB");
        assert_eq!(super::initials("🗁 Local Test"), "LT");
        assert_eq!(super::initials(""), "··");
    }
}
