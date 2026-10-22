//! Window chrome: the port rail on the left and the slim session header.
//!
//! Open sessions live in the rail as numbered "ports" (the number is the Ctrl+N shortcut),
//! each with a status LED, instead of a tab strip.

use egui::{Color32, CornerRadius, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::{Action, App, ConnState, TabKind, View};
use crate::theme;
use crate::ui::icons::{self, Icon};

const RAIL_WIDTH: f32 = 72.0;
const SIDEBAR_WIDTH: f32 = 232.0;
const TILE: f32 = 44.0;

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

/// Navigation entry: icon only (rail) or icon + label (sidebar).
fn nav_item(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    wide: bool,
    tint: Option<Color32>,
    dot: Option<Color32>,
) -> egui::Response {
    let size = if wide {
        Vec2::new(ui.available_width(), 34.0)
    } else {
        Vec2::splat(TILE)
    };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let radius = if wide { 8 } else { 12 };
    if selected {
        p.rect_filled(rect, CornerRadius::same(radius), theme::accent_soft());
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(radius), theme::hover());
    }
    let color = tint.unwrap_or(if selected {
        theme::accent()
    } else {
        theme::text_dim()
    });
    let icon_rect = if wide {
        Rect::from_center_size(
            Pos2::new(rect.left() + 19.0, rect.center().y),
            Vec2::splat(18.0),
        )
    } else {
        Rect::from_center_size(rect.center(), Vec2::splat(18.0))
    };
    icons::paint(p, icon_rect, icon, color);
    if wide {
        p.text(
            Pos2::new(rect.left() + 38.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            theme::medium(14.0),
            theme::text(),
        );
        if let Some(c) = dot {
            p.circle_filled(Pos2::new(rect.right() - 14.0, rect.center().y), 3.5, c);
        }
    } else if let Some(c) = dot {
        p.circle_filled(rect.right_top() + Vec2::new(-9.0, 9.0), 3.5, c);
    }
    let resp = if wide {
        resp
    } else {
        resp.on_hover_text(label)
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[allow(clippy::too_many_arguments)]
fn port_tile(
    ui: &mut egui::Ui,
    in_split: bool,
    index: usize,
    label: &str,
    led: Color32,
    live: bool,
    active: bool,
    attention: bool,
    dead: bool,
    env: crate::model::Env,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(TILE), Sense::click_and_drag());
    let p = ui.painter();
    let pulse = if attention {
        // Gentle blink until the session has been looked at.
        let t = ui.input(|i| i.time) as f32;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
        0.35 + 0.65 * (0.5 + 0.5 * (t * 4.0).sin())
    } else {
        0.0
    };
    let fill = if resp.hovered() {
        theme::fill_strong()
    } else {
        theme::fill()
    };
    p.rect_filled(rect, CornerRadius::same(12), fill);
    if attention {
        p.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(2.0, theme::accent().gamma_multiply(pulse)),
            egui::StrokeKind::Inside,
        );
    } else if active {
        p.rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(13),
            Stroke::new(2.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::bold(13.0),
        if dead {
            theme::text_dim().gamma_multiply(0.6)
        } else {
            theme::text()
        },
    );
    if matches!(env, crate::model::Env::Prod | crate::model::Env::Staging) {
        // Environment stripe along the top edge.
        p.rect_filled(
            Rect::from_min_size(
                rect.left_top() + Vec2::new(12.0, 2.0),
                Vec2::new(TILE - 24.0, 2.5),
            ),
            CornerRadius::same(2),
            theme::env_color(env),
        );
    }
    if dead {
        icons::paint(
            p,
            Rect::from_center_size(
                rect.right_bottom() + Vec2::new(-9.0, -9.0),
                Vec2::splat(10.0),
            ),
            Icon::Refresh,
            theme::amber(),
        );
    }
    if index <= 9 {
        p.text(
            rect.left_bottom() + Vec2::new(5.0, -3.0),
            egui::Align2::LEFT_BOTTOM,
            index.to_string(),
            theme::semibold(9.5),
            theme::text_dim(),
        );
    }
    let dot = if attention {
        theme::accent().gamma_multiply(pulse)
    } else {
        led
    };
    let c = rect.right_top() + Vec2::new(-9.0, 9.0);
    if live || attention {
        p.circle_filled(c, 6.0, theme::side());
        p.circle_filled(c, 4.0, dot);
    } else {
        p.circle_filled(c, 5.5, theme::side());
        p.circle_stroke(c, 3.5, Stroke::new(1.5, dot));
    }
    if in_split {
        p.rect_filled(
            Rect::from_min_size(
                rect.left_top() + Vec2::new(-10.0, 12.0),
                Vec2::new(3.0, TILE - 24.0),
            ),
            CornerRadius::same(2),
            theme::accent(),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Sidebar with labels while no session is open, slim rail with session tiles otherwise.
pub fn rail(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let wide = app.tabs.is_empty() && ui.ctx().content_rect().width() >= 900.0;
    let width = if wide { SIDEBAR_WIDTH } else { RAIL_WIDTH };
    egui::Panel::left("rail")
        .exact_size(width)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(theme::side()).inner_margin(if wide {
            Margin {
                left: 12,
                right: 12,
                top: 20,
                bottom: 16,
            }
        } else {
            Margin::symmetric(14, 16)
        }))
        .show(ui, |ui| {
            // Hairline on the edge towards the content.
            let edge = ui.max_rect().right() + if wide { 12.0 } else { 14.0 } - 0.5;
            let r = ui.clip_rect();
            ui.painter().line_segment(
                [
                    Pos2::new(edge, r.top() - 40.0),
                    Pos2::new(edge, r.bottom() + 40.0),
                ],
                Stroke::new(1.0, theme::line()),
            );
            let layout = if wide {
                egui::Layout::top_down(egui::Align::Min)
            } else {
                egui::Layout::top_down(egui::Align::Center)
            };
            ui.with_layout(layout, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                // Low windows: the upper part scrolls instead of running into the bottom items.
                let item_h = if wide { 38.0 } else { TILE + 4.0 };
                let n_bottom = 2
                    + usize::from(app.update.latest.is_some())
                    + if wide {
                        0
                    } else {
                        1 + usize::from(!app.tabs.is_empty())
                    };
                let top_h = (ui.available_height() - n_bottom as f32 * item_h - 8.0).max(item_h);
                egui::ScrollArea::vertical()
                    .id_salt("rail-top")
                    .max_height(top_h)
                    .auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL)
                    .show(ui, |ui| {
                        ui.with_layout(layout, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let low = ui.ctx().content_rect().height() < 560.0;
                            rail_top(app, ui, wide, low, actions);
                        });
                    });
                let home = app.active.is_none();

                ui.with_layout(
                    if wide {
                        egui::Layout::bottom_up(egui::Align::Min)
                    } else {
                        egui::Layout::bottom_up(egui::Align::Center)
                    },
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if nav_item(
                            ui,
                            Icon::Help,
                            t!("Hilfe"),
                            home && app.view == View::Help,
                            wide,
                            None,
                            None,
                        )
                        .on_hover_text(t!("Tastenkürzel (F1)"))
                        .clicked()
                        {
                            actions.push(Action::Navigate(View::Help));
                        }
                        if nav_item(
                            ui,
                            Icon::Sun,
                            t!("Einstellungen"),
                            home && app.view == View::Settings,
                            wide,
                            None,
                            None,
                        )
                        .clicked()
                        {
                            actions.push(Action::Navigate(View::Settings));
                        }
                        if let Some(rel) = &app.update.latest {
                            let tip = t!("Update auf {} verfügbar", rel.version_text());
                            if nav_item(
                                ui,
                                Icon::Download,
                                &tip,
                                false,
                                wide,
                                Some(theme::accent()),
                                None,
                            )
                            .clicked()
                            {
                                // Straight to "Data & updates", where the update is installed.
                                app.settings_tab = 4;
                                actions.push(Action::Navigate(View::Settings));
                            }
                        }
                        if !wide {
                            if nav_item(ui, Icon::Lock, t!("Sperren"), false, false, None, None)
                                .clicked()
                            {
                                actions.push(Action::Lock);
                            }
                            if !app.tabs.is_empty() {
                                let tip = if app.broadcast {
                                    t!("Broadcast an: Eingaben gehen an alle Terminals")
                                } else {
                                    t!("Broadcast: Eingaben an alle Terminals senden")
                                };
                                if nav_item(
                                    ui,
                                    Icon::Broadcast,
                                    tip,
                                    app.broadcast,
                                    false,
                                    app.broadcast.then(theme::pink),
                                    None,
                                )
                                .clicked()
                                {
                                    app.broadcast = !app.broadcast;
                                }
                            }
                        }
                    },
                );
            });
        });
}

/// Logo, page entries and session tiles (the scrollable upper part of the rail).
fn rail_top(app: &mut App, ui: &mut egui::Ui, wide: bool, low: bool, actions: &mut Vec<Action>) {
    if wide {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            theme::logo(ui, 34.0).on_hover_text("Aurora");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_btn(ui, Icon::Lock, t!("Sperren")).clicked() {
                    actions.push(Action::Lock);
                }
            });
        });
        ui.add_space(22.0);
    } else if !low {
        theme::logo(ui, 34.0).on_hover_text("Aurora");
        ui.add_space(14.0);
    }

    let home = app.active.is_none();
    for (view, icon, label) in [
        (View::Hosts, Icon::Hosts, "Verbindungen"),
        (View::Keychain, Icon::Key, "Schlüssel"),
        (View::Files, Icon::Folder, "Dateien"),
        (View::Snippets, Icon::Bolt, "Snippets"),
        (View::Proxmox, Icon::Server, "Proxmox"),
        (View::Containers, Icon::Cube, "Container"),
        (View::Inventory, Icon::Table, "Inventar"),
    ] {
        if nav_item(
            ui,
            icon,
            crate::i18n::tr(label),
            home && app.view == view,
            wide,
            None,
            None,
        )
        .clicked()
        {
            actions.push(Action::Navigate(view));
        }
    }
    tunnel_item(app, ui, wide, actions);
    xfer_item(app, ui, wide);
    if wide {
        crate::ui::nav_rail::sections(app, ui, actions);
    } else {
        crate::ui::nav_rail::scope_tile(app, ui, actions);
    }

    if !app.tabs.is_empty() {
        ui.add_space(6.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(28.0, 1.0), Sense::hover());
        ui.painter()
            .rect_filled(r, CornerRadius::ZERO, theme::line());
        ui.add_space(6.0);
    }

    ports(app, ui, actions);
}

/// "Übertragungen" entry with the number of active transfers; opens the queue panel.
fn xfer_item(app: &mut App, ui: &mut egui::Ui, wide: bool) {
    if app.xfer.q.batches.is_empty() {
        return;
    }
    let n = app.xfer.active();
    let dot = (n > 0).then(theme::accent);
    let resp = nav_item(
        ui,
        Icon::Transfer,
        t!("Übertragungen"),
        false,
        wide,
        None,
        None,
    );
    if n > 0 {
        let c = if wide {
            Pos2::new(resp.rect.right() - 16.0, resp.rect.center().y)
        } else {
            resp.rect.right_top() + Vec2::new(-8.0, 9.0)
        };
        ui.painter().circle_filled(c, 8.0, dot.unwrap_or_default());
        ui.painter().text(
            c,
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            theme::semibold(10.5),
            theme::on_accent(),
        );
    }
    crate::ui::xfer::toggle(&mut app.xfer, &resp);
}

/// "Tunnel" entry: a menu with every tunnel of every host, open ones first.
fn tunnel_item(app: &App, ui: &mut egui::Ui, wide: bool, actions: &mut Vec<Action>) {
    let up = app
        .tunnels
        .values()
        .filter(|r| r.state == crate::app::TunnelState::Up)
        .count();
    let dot = (!app.tunnels.is_empty()).then(|| {
        if up > 0 {
            theme::green()
        } else {
            theme::amber()
        }
    });
    let resp = nav_item(ui, Icon::Tunnel, t!("Tunnel"), false, wide, None, dot);
    let resp = if wide {
        resp
    } else {
        resp.on_hover_text(t!("{} Tunnel offen", up))
    };
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(300.0);
        let mut any = false;
        for h in &app.lib.hosts {
            for tnl in &h.tunnels {
                any = true;
                let run = app.tunnels.get(&tnl.id);
                let on = run.is_some_and(|r| r.state == crate::app::TunnelState::Up);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    theme::led(
                        ui.painter(),
                        r.center(),
                        if on {
                            theme::green()
                        } else if run.is_some() {
                            theme::amber()
                        } else {
                            theme::off_color()
                        },
                        on,
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(tnl.label()).font(theme::semibold(13.0)));
                        ui.label(
                            theme::mono(
                                format!("{} · localhost:{}", h.display_name(), tnl.local_port),
                                11.5,
                            )
                            .color(theme::text_dim()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if run.is_some() {
                            t!("Schließen")
                        } else {
                            t!("Öffnen")
                        };
                        if theme::btn(ui, theme::Btn::Small, None, label).clicked() {
                            actions.push(Action::ToggleTunnel(h.id, tnl.id));
                        }
                    });
                });
            }
        }
        if !any {
            ui.label(
                RichText::new(t!(
                    "Noch keine Tunnel. Lege sie beim Host an (Bearbeiten → Tunnel)."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
        }
    });
}

/// Open sessions as tiles; the number is the Ctrl+N shortcut.
fn ports(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let mut activate = None;
    let mut close = None;
    let mut reconnect = None;
    let mut revive = None;
    let dragging = dragged_tab(ui.ctx()).is_some();
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
                    t!("verbunden").into()
                } else {
                    t!("getrennt").into()
                },
            ),
            TabKind::Vnc(t) => (
                crate::app::state_color(&t.view.state),
                matches!(t.view.state, ConnState::Connected),
                state_text(&t.view.state),
            ),
            TabKind::Log(t) => (
                crate::app::state_color(&t.state),
                matches!(t.state, ConnState::Connected),
                state_text(&t.state),
            ),
            TabKind::Edit(t) => (
                crate::app::state_color(t.state()),
                matches!(t.state(), ConnState::Connected),
                state_text(t.state()),
            ),
            TabKind::Web(t) => (
                crate::app::state_color(&t.state),
                t.is_live(),
                state_text(&t.state),
            ),
        };
        let kind = match &tab.kind {
            TabKind::Term(_) => "Terminal",
            TabKind::Sftp(_) => t!("Dateien"),
            TabKind::Rdp(_) | TabKind::Vnc(_) => "Desktop",
            TabKind::Log(_) => "Log",
            TabKind::Edit(_) => t!("Editor"),
            TabKind::Web(_) => "Web",
        };
        let dead = match &tab.kind {
            TabKind::Term(t) => matches!(t.state, ConnState::Closed(_)),
            TabKind::Vnc(t) => matches!(t.view.state, ConnState::Closed(_)),
            TabKind::Log(t) => matches!(t.state, ConnState::Closed(_)),
            TabKind::Web(t) => t.is_closed(),
            _ => false,
        };
        let resp = port_tile(
            ui,
            app.split.len() >= 2 && app.split.contains(&tab.id),
            i + 1,
            &initials(&tab.title),
            state_color,
            live,
            app.active == Some(tab.id),
            tab.attention,
            dead,
            tab.env,
        );
        let tile = resp.rect;
        if matches!(&tab.kind, TabKind::Term(t) if t.auto.is_some()) {
            // Waiting for the automatic reconnect.
            ui.painter().text(
                tile.right_bottom() - Vec2::new(11.0, 11.0),
                egui::Align2::CENTER_CENTER,
                "⟳",
                egui::FontId::proportional(14.0),
                theme::amber(),
            );
        }
        // Low windows scroll the rail: keep the active session in view (unless the
        // pointer is on the rail, where the user may be scrolling).
        if app.active == Some(tab.id) {
            let seen_id = egui::Id::new("rail-active-seen");
            let clip = ui.clip_rect();
            let hidden = !clip.contains_rect(tile)
                && !ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|p| clip.contains(p));
            if hidden || ui.ctx().data(|d| d.get_temp::<u64>(seen_id)) != Some(tab.id) {
                ui.ctx().data_mut(|d| d.insert_temp(seen_id, tab.id));
                resp.scroll_to_me(None);
            }
        }
        if resp.drag_started() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(drag_id(), DragTab(tab.id)));
        }
        let resp = if dragging {
            resp
        } else {
            resp.on_hover_ui(|ui| {
                ui.label(theme::heading(&tab.title, 13.5));
                ui.label(theme::mono(&tab.subtitle, 12.0).color(theme::text_dim()));
                ui.label(
                    RichText::new(format!("{kind} · {state}"))
                        .size(12.0)
                        .color(theme::text_dim()),
                );
                if dead {
                    ui.label(
                        RichText::new(t!("Getrennt – Klick verbindet neu"))
                            .size(12.0)
                            .color(theme::amber()),
                    );
                }
                if tab.attention {
                    ui.label(
                        RichText::new(t!("Neue Ausgabe / Befehl fertig"))
                            .size(12.0)
                            .color(theme::accent()),
                    );
                }
                ui.label(
                    RichText::new(t!("Ziehen, um sie neben eine Session zu legen"))
                        .size(11.5)
                        .color(theme::text_dim()),
                );
            })
        };
        // Small × in the top-left corner while hovering: end the session.
        if !dragging && ui.rect_contains_pointer(tile) {
            let xr = Rect::from_min_size(tile.left_top() + Vec2::splat(-3.0), Vec2::splat(16.0));
            let x = ui.interact(xr, ui.id().with(("port-x", tab.id)), Sense::click());
            close_glyph(ui.painter(), xr.center(), x.hovered(), true);
            if x.on_hover_text(t!("Session beenden")).clicked() {
                close = Some(tab.id);
            }
        }
        if resp.clicked() {
            activate = Some(tab.id);
            if dead {
                revive = Some(tab.id);
            }
        }
        if resp.middle_clicked() {
            close = Some(tab.id);
        }
        resp.context_menu(|ui| {
            if let Some(a) = app.active.filter(|a| *a != tab.id)
                && !app.split.contains(&tab.id)
                && app.split.len() < MAX_SPLIT
                && ui.button(t!("Neben aktuelle Session legen")).clicked()
            {
                push_split_op(ui.ctx(), SplitOp::Add(a, tab.id));
                ui.close();
            }
            if app.split.len() >= 2 {
                ui.separator();
                layout_menu(ui, app.lib.settings.split_layout);
                ui.separator();
            }
            if ui.button(t!("Neu verbinden")).clicked() {
                reconnect = Some(tab.id);
                ui.close();
            }
            if ui.button(t!("Schließen")).clicked() {
                close = Some(tab.id);
                ui.close();
            }
        });
        ui.add_space(4.0);
    }
    if let Some(id) = activate {
        app.active = Some(id);
    }
    if let Some(id) = reconnect.or(revive) {
        actions.push(Action::Reconnect(id));
    }
    if let Some(id) = close {
        app.close_tab(id);
    }
}

pub fn state_text(s: &ConnState) -> String {
    match s {
        ConnState::Connecting(_) => t!("verbindet").into(),
        ConnState::Connected => t!("verbunden").into(),
        ConnState::Closed(Some(_)) => t!("Fehler").into(),
        ConnState::Closed(None) => t!("getrennt").into(),
    }
}

/// Status label and LED colour for a session header.
pub fn state_pair(s: &ConnState) -> (String, Color32) {
    match s {
        ConnState::Connecting(m) => (m.clone(), theme::amber()),
        ConnState::Connected => (t!("verbunden").into(), theme::green()),
        ConnState::Closed(Some(e)) => (e.clone(), theme::red()),
        ConnState::Closed(None) => (t!("getrennt").into(), theme::text_dim()),
    }
}

/// Which sessions exist and which are shown side by side; published by the app each
/// frame so every session header can offer the split menu.
#[derive(Clone, Default)]
pub struct SplitInfo {
    pub split: Vec<u64>,
    pub maximized: Option<u64>,
    pub split_broadcast: bool,
    /// Environment per tab, same order as `tabs`.
    pub envs: Vec<crate::model::Env>,
    pub tabs: Vec<(u64, String)>,
    /// Connected state per tab, same order as `tabs`.
    pub live: Vec<bool>,
    /// Full-screen mode is on.
    pub focus: bool,
    /// Sessions in their own window.
    pub popped: Vec<u64>,
    pub layout: SplitLayout,
}

#[derive(Clone, Copy, Debug)]
pub enum SplitOp {
    /// Show `other` next to `base`.
    Add(u64, u64),
    Remove(u64),
    Clear,
    /// End the session (header ×).
    Close(u64),
    /// Show this split pane alone / back in the split.
    Maximize(u64),
    /// Toggle typing into all split panes at once.
    Broadcast,
    /// Full screen with this session (`None` = leave full screen).
    Focus(Option<u64>),
    /// Show the session in its own window, or bring it back.
    PopOut(u64),
    /// Arrange the panes of the split.
    Layout(SplitLayout),
}

/// Session tile currently dragged out of the rail.
#[derive(Clone, Copy, Debug)]
pub struct DragTab(pub u64);

pub fn drag_id() -> egui::Id {
    egui::Id::new("aurora-drag-tab")
}

pub fn dragged_tab(ctx: &egui::Context) -> Option<u64> {
    ctx.data(|d| d.get_temp::<DragTab>(drag_id())).map(|d| d.0)
}

pub fn end_drag(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<DragTab>(drag_id()));
}

/// Pane order after dropping `dragged` at `pos` over `area` that currently shows `shown`.
/// Every free slot of the resulting layout is a drop zone, so the preview is exactly the
/// pane the session will get. When four are already shown, the pane under `pos` is replaced.
pub fn drop_order(
    area: Rect,
    shown: &[u64],
    dragged: u64,
    pos: Pos2,
    layout: SplitLayout,
    saved: &SplitRatios,
) -> Option<Vec<u64>> {
    if !area.contains(pos) {
        return None;
    }
    let base: Vec<u64> = shown.iter().copied().filter(|x| *x != dragged).collect();
    if base.is_empty() {
        return Some(vec![dragged]);
    }
    if base.len() >= MAX_SPLIT {
        let i = split_rects_for(area, base.len(), layout, saved)
            .iter()
            .position(|r| r.contains(pos))?;
        let mut order = base;
        order[i] = dragged;
        return Some(order);
    }
    (0..=base.len()).find_map(|k| {
        let mut order = base.clone();
        order.insert(k, dragged);
        split_rects_for(area, order.len(), layout, saved)[k]
            .contains(pos)
            .then_some(order)
    })
}

/// Painted ×, optionally on a small round plate (for the rail tiles).
fn close_glyph(p: &egui::Painter, c: Pos2, hot: bool, plate: bool) {
    let color = if hot { theme::red() } else { theme::text_dim() };
    if plate {
        p.circle(
            c,
            6.5,
            theme::bg_panel(),
            Stroke::new(1.0, color.linear_multiply(0.7)),
        );
    }
    let d = if plate { 2.5 } else { 3.5 };
    let s = Stroke::new(1.4, color);
    p.line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], s);
    p.line_segment([c + Vec2::new(-d, d), c + Vec2::new(d, -d)], s);
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

/// "Two panes" icon button.
fn split_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    theme::icon_toggle(ui, Icon::Split, t!("Ansicht teilen"), on)
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
    let resp = split_button(ui, in_split);
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
            if others.is_empty() && !in_split {
                ui.label(
                    RichText::new(t!(
                        "Öffne eine weitere Session, um sie hier daneben zu legen."
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                );
            }
            for (id, title) in others {
                if ui.button(t!("Daneben: {}", title)).clicked() {
                    push_split_op(ui.ctx(), SplitOp::Add(me, *id));
                    ui.close();
                }
            }
        } else {
            ui.label(
                RichText::new(t!("Maximal vier Sessions nebeneinander."))
                    .size(12.5)
                    .color(theme::text_dim()),
            );
        }
        if in_split {
            ui.separator();
            layout_menu(ui, info.layout);
            ui.label(
                RichText::new(t!(
                    "Tipp: Pane an der Kopfzeile ziehen, um sie zu verschieben."
                ))
                .size(12.0)
                .color(theme::text_dim()),
            );
            ui.separator();
            let mark = if info.split_broadcast { "✔ " } else { "" };
            if ui
                .button(t!("{}Eingaben an alle Panes senden", mark))
                .on_hover_text(t!(
                    "Was du in einem Terminal tippst, geht an alle Terminals im Split"
                ))
                .clicked()
            {
                push_split_op(ui.ctx(), SplitOp::Broadcast);
                ui.close();
            }
            let maxed = info.maximized == Some(me);
            if ui
                .button(if maxed {
                    t!("Zurück zum Split")
                } else {
                    t!("Maximieren")
                })
                .clicked()
            {
                push_split_op(ui.ctx(), SplitOp::Maximize(me));
                ui.close();
            }
            if ui.button(t!("Diese Session lösen")).clicked() {
                push_split_op(ui.ctx(), SplitOp::Remove(me));
                ui.close();
            }
            if ui.button(t!("Split beenden")).clicked() {
                push_split_op(ui.ctx(), SplitOp::Clear);
                ui.close();
            }
        }
    });
}

/// "Anordnung" section of the split menus: one row per layout, ✔ on the current one.
fn layout_menu(ui: &mut egui::Ui, current: SplitLayout) {
    ui.label(
        RichText::new(t!("Anordnung"))
            .size(12.5)
            .color(theme::text_dim()),
    );
    for l in SplitLayout::ALL {
        let on = current == l;
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(220.0, 28.0),
            Sense::click(),
        );
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(6), theme::bg_card());
        }
        let col = if on { theme::accent() } else { theme::text() };
        if on {
            p.text(
                Pos2::new(rect.left() + 12.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "✔",
                egui::FontId::proportional(13.0),
                col,
            );
        }
        let icon = Rect::from_center_size(
            Pos2::new(rect.left() + 38.0, rect.center().y),
            Vec2::new(18.0, 14.0),
        );
        paint_layout_icon(p, icon, l, col);
        p.text(
            Pos2::new(rect.left() + 56.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            l.label(),
            egui::FontId::proportional(14.0),
            col,
        );
        if resp.clicked() {
            push_split_op(ui.ctx(), SplitOp::Layout(l));
            ui.close();
        }
    }
}

/// Tiny pictogram of a layout (four panes), drawn so it needs no special font glyphs.
fn paint_layout_icon(p: &egui::Painter, r: Rect, l: SplitLayout, col: Color32) {
    let stroke = Stroke::new(1.2, col);
    // Lay out on a large canvas (the pane gap is fixed in px), then scale down.
    let big = Rect::from_min_size(Pos2::ZERO, Vec2::new(90.0, 70.0));
    let n = if l == SplitLayout::Auto { 3 } else { 4 };
    let k = Vec2::new(r.width() / big.width(), r.height() / big.height());
    for q in split_rects_for(big, n, l) {
        let q = Rect::from_min_max(
            r.min + Vec2::new(q.min.x * k.x, q.min.y * k.y),
            r.min + Vec2::new(q.max.x * k.x, q.max.y * k.y),
        );
        p.rect_stroke(q, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
    }
}

/// How the panes of a split are arranged.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum SplitLayout {
    /// Side by side, then 1 + 2, then a 2×2 grid.
    #[default]
    Auto,
    /// All panes next to each other.
    Columns,
    /// All panes below each other.
    Rows,
    /// One big pane on the left, the others stacked on the right.
    MainLeft,
    /// One big pane on top, the others side by side below.
    MainTop,
    /// Two by two (a lone last pane spans the row).
    Grid,
}

impl SplitLayout {
    pub const ALL: [SplitLayout; 6] = [
        SplitLayout::Auto,
        SplitLayout::Columns,
        SplitLayout::Rows,
        SplitLayout::MainLeft,
        SplitLayout::MainTop,
        SplitLayout::Grid,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SplitLayout::Auto => t!("Automatisch"),
            SplitLayout::Columns => t!("Nebeneinander"),
            SplitLayout::Rows => t!("Untereinander"),
            SplitLayout::MainLeft => t!("Eine groß links"),
            SplitLayout::MainTop => t!("Eine groß oben"),
            SplitLayout::Grid => t!("Raster"),
        }
    }
}

/// Saved divider positions per layout and pane count (`"Rows-3"` → ratios of its dividers,
/// `0.0` = not moved yet).
pub type SplitRatios = std::collections::BTreeMap<String, Vec<f32>>;

pub fn ratio_key(layout: SplitLayout, n: usize) -> String {
    format!("{layout:?}-{n}")
}

/// Panes closer than this to a window edge or each other are not allowed (px).
const MIN_PANE: f32 = 120.0;

/// A draggable divider between panes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divider {
    /// Index into the layout's ratios.
    pub idx: usize,
    /// Mouse hit area (the gap and a little more).
    pub hit: Rect,
    /// Panes on both sides sit next to each other (the line is vertical).
    pub vertical: bool,
    /// The area that is divided – the pointer position inside it gives the ratio.
    pub region: Rect,
    /// Ratio of an equal division.
    pub default: f32,
}

enum Node {
    Leaf(usize),
    /// `side_by_side`: `a` left of `b`, else `a` above `b`.
    Split {
        side_by_side: bool,
        idx: usize,
        default: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// `leaves` in a row (or column) with equal default sizes; dividers numbered from `idx0`.
fn chain(side_by_side: bool, leaves: &[usize], idx0: usize) -> Node {
    match leaves {
        [one] => Node::Leaf(*one),
        [first, rest @ ..] => Node::Split {
            side_by_side,
            idx: idx0,
            default: 1.0 / leaves.len() as f32,
            a: Box::new(Node::Leaf(*first)),
            b: Box::new(chain(side_by_side, rest, idx0 + 1)),
        },
        [] => Node::Leaf(0),
    }
}

fn split2(side_by_side: bool, idx: usize, a: Node, b: Node) -> Node {
    Node::Split {
        side_by_side,
        idx,
        default: 0.5,
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn grid4(n: usize) -> Node {
    // Both rows share one vertical divider (index 1).
    let top = split2(true, 1, Node::Leaf(0), Node::Leaf(1));
    match n {
        3 => split2(false, 0, top, Node::Leaf(2)),
        _ => split2(false, 0, top, split2(true, 1, Node::Leaf(2), Node::Leaf(3))),
    }
}

fn layout_tree(layout: SplitLayout, n: usize) -> Node {
    let all: Vec<usize> = (0..n).collect();
    match layout {
        SplitLayout::Columns => chain(true, &all, 0),
        SplitLayout::Rows => chain(false, &all, 0),
        SplitLayout::MainLeft => split2(true, 0, Node::Leaf(0), chain(false, &all[1..], 1)),
        SplitLayout::MainTop => split2(false, 0, Node::Leaf(0), chain(true, &all[1..], 1)),
        SplitLayout::Grid => match n {
            2 => chain(true, &all, 0),
            _ => grid4(n),
        },
        SplitLayout::Auto => match n {
            2 => chain(true, &all, 0),
            3 => split2(
                true,
                0,
                Node::Leaf(0),
                split2(false, 1, Node::Leaf(1), Node::Leaf(2)),
            ),
            _ => grid4(n),
        },
    }
}

fn place(
    node: &Node,
    r: Rect,
    ratios: &[f32],
    panes: &mut Vec<(usize, Rect)>,
    dividers: &mut Vec<Divider>,
) {
    // Half the gap between panes.
    let g = 5.0;
    match node {
        Node::Leaf(i) => panes.push((*i, r)),
        Node::Split {
            side_by_side,
            idx,
            default,
            a,
            b,
        } => {
            let len = if *side_by_side { r.width() } else { r.height() };
            let lo = (MIN_PANE / len).min(0.45);
            let ratio = ratios
                .get(*idx)
                .copied()
                .filter(|r| *r > 0.0)
                .unwrap_or(*default)
                .clamp(lo, 1.0 - lo);
            let (ra, rb, hit) = if *side_by_side {
                let mid = r.left() + r.width() * ratio;
                (
                    Rect::from_min_max(r.min, Pos2::new(mid - g, r.bottom())),
                    Rect::from_min_max(Pos2::new(mid + g, r.top()), r.max),
                    Rect::from_min_max(
                        Pos2::new(mid - g - 2.0, r.top()),
                        Pos2::new(mid + g + 2.0, r.bottom()),
                    ),
                )
            } else {
                let mid = r.top() + r.height() * ratio;
                (
                    Rect::from_min_max(r.min, Pos2::new(r.right(), mid - g)),
                    Rect::from_min_max(Pos2::new(r.left(), mid + g), r.max),
                    Rect::from_min_max(
                        Pos2::new(r.left(), mid - g - 2.0),
                        Pos2::new(r.right(), mid + g + 2.0),
                    ),
                )
            };
            dividers.push(Divider {
                idx: *idx,
                hit,
                vertical: *side_by_side,
                region: r,
                default: *default,
            });
            place(a, ra, ratios, panes, dividers);
            place(b, rb, ratios, panes, dividers);
        }
    }
}

/// Pane rectangles (in pane order) and the draggable dividers for `n` sessions.
pub fn split_geometry(
    r: Rect,
    n: usize,
    layout: SplitLayout,
    saved: &SplitRatios,
) -> (Vec<Rect>, Vec<Divider>) {
    if n <= 1 {
        return (vec![r], Vec::new());
    }
    let ratios = saved
        .get(&ratio_key(layout, n))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut panes = Vec::new();
    let mut dividers = Vec::new();
    place(
        &layout_tree(layout, n),
        r,
        ratios,
        &mut panes,
        &mut dividers,
    );
    panes.sort_by_key(|(i, _)| *i);
    (panes.into_iter().map(|(_, r)| r).collect(), dividers)
}

/// Pane rectangles for 1–4 sessions in `layout`.
pub fn split_rects_for(r: Rect, n: usize, layout: SplitLayout, saved: &SplitRatios) -> Vec<Rect> {
    split_geometry(r, n, layout, saved).0
}

/// Ratio for a divider when the pointer is at `pos`.
pub fn ratio_at(d: &Divider, pos: Pos2) -> f32 {
    let (p, from, len) = if d.vertical {
        (pos.x, d.region.left(), d.region.width())
    } else {
        (pos.y, d.region.top(), d.region.height())
    };
    ((p - from) / len.max(1.0)).clamp(0.05, 0.95)
}

/// Header above a session: close, port number, name and address, state dot; actions on
/// the right. In full-screen mode it becomes the top bar with the session switcher.
pub fn session_header(
    ui: &mut egui::Ui,
    id: egui::Id,
    index: usize,
    title: &str,
    subtitle: &str,
    state: (String, Color32),
    right: impl FnOnce(&mut egui::Ui),
) {
    let info: SplitInfo = ui
        .ctx()
        .data(|d| d.get_temp(split_info_id()))
        .unwrap_or_default();
    if info.focus {
        return focus_bar(ui, id, index, subtitle, &info, right);
    }
    egui::Panel::top(id)
        .exact_size(56.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::surface())
                .inner_margin(Margin {
                    left: 8,
                    right: 10,
                    top: 0,
                    bottom: 0,
                }),
        )
        .show(ui, |ui| {
            let full = ui.max_rect();
            ui.painter().line_segment(
                [
                    Pos2::new(full.left() - 8.0, full.bottom() - 0.5),
                    Pos2::new(full.right() + 10.0, full.bottom() - 0.5),
                ],
                Stroke::new(1.0, theme::line()),
            );
            // Right side first, so the title/address get whatever space is left and
            // truncate instead of running under the buttons in narrow split panes.
            let narrow = ui.available_width() < 560.0;
            let me = info.tabs.get(index.wrapping_sub(1)).map(|t| t.0);
            let in_split = me.is_some_and(|m| info.split.len() >= 2 && info.split.contains(&m));
            if let (Some(me), true) = (me, in_split) {
                let bg = ui.interact(
                    ui.max_rect(),
                    ui.id().with("header-bg"),
                    Sense::click_and_drag(),
                );
                if bg.double_clicked() {
                    push_split_op(ui.ctx(), SplitOp::Maximize(me));
                }
                // Drag the pane by its header: the drop preview shows where it lands.
                if bg.drag_started() {
                    ui.ctx().data_mut(|d| d.insert_temp(drag_id(), DragTab(me)));
                }
                if bg.hovered() || bg.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if let Some(me) = me {
                    let popped = info.popped.contains(&me);
                    let tip = if popped {
                        t!("Zurück ins Hauptfenster")
                    } else {
                        t!("In eigenem Fenster öffnen")
                    };
                    if theme::icon_toggle(ui, Icon::External, tip, popped).clicked() {
                        push_split_op(ui.ctx(), SplitOp::PopOut(me));
                    }
                    if !popped
                        && theme::icon_btn(ui, Icon::Maximize, t!("Vollbild (F11)")).clicked()
                    {
                        push_split_op(ui.ctx(), SplitOp::Focus(Some(me)));
                    }
                }
                split_menu(ui, index);
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 18.0), Sense::hover());
                ui.painter()
                    .rect_filled(r, CornerRadius::ZERO, theme::line());
                ui.add_space(6.0);
                // Narrow panes: the session tools move into a "…" menu (one row), so the
                // title keeps its room. The full width of the tools is remembered.
                let tools_id = id.with("tools-width");
                let tools_w: f32 = ui.ctx().data(|d| d.get_temp(tools_id)).unwrap_or(0.0);
                if ui.available_width() < tools_w + 200.0 {
                    let more = theme::icon_button(ui, "…", t!("Weitere Aktionen"));
                    egui::Popup::menu(&more).id(id.with("tools")).show(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            right(ui);
                        });
                    });
                } else {
                    let before = ui.min_rect().width();
                    right(ui);
                    let w = ui.min_rect().width() - before;
                    if (w - tools_w).abs() > 0.5 {
                        ui.ctx().data_mut(|d| d.insert_temp(tools_id, w));
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if let Some(me) = me {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                        if resp.hovered() {
                            ui.painter()
                                .rect_filled(r, CornerRadius::same(9), theme::fill());
                        }
                        icons::paint_weighted(
                            ui.painter(),
                            Rect::from_center_size(r.center(), Vec2::splat(12.0)),
                            Icon::Close,
                            if resp.hovered() {
                                theme::text()
                            } else {
                                theme::text_dim()
                            },
                            2.8,
                        );
                        if resp
                            .on_hover_text(t!("Session beenden"))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            push_split_op(ui.ctx(), SplitOp::Close(me));
                        }
                    }
                    ui.label(
                        RichText::new(format!("{index:02}"))
                            .font(theme::mono_medium(12.0))
                            .color(theme::accent()),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            // The title truncates; pill and status dot keep their room
                            // (the pill only while the title still has some).
                            let env = info
                                .envs
                                .get(index.wrapping_sub(1))
                                .copied()
                                .filter(|e| *e != crate::model::Env::None);
                            let pill_w = env.map_or(0.0, |e| {
                                ui.painter()
                                    .layout_no_wrap(
                                        e.label().to_uppercase(),
                                        theme::bold(10.5),
                                        theme::text(),
                                    )
                                    .size()
                                    .x
                                    + 14.0
                                    + 8.0
                            });
                            let room = ui.available_width() - 12.0 - 8.0;
                            let env = env.filter(|_| room - pill_w >= 60.0);
                            let title_w = if env.is_some() { room - pill_w } else { room };
                            ui.allocate_ui_with_layout(
                                Vec2::new(title_w.max(20.0), 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(title)
                                                .font(theme::semibold(14.5))
                                                .color(theme::text()),
                                        )
                                        .truncate(),
                                    );
                                },
                            );
                            if let Some(env) = env {
                                theme::pill(ui, env.label(), theme::env_color(env));
                            }
                            let (label, color) = &state;
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                            theme::led(
                                ui.painter(),
                                r.center(),
                                *color,
                                *color != theme::text_dim(),
                            );
                            resp.on_hover_text(label.as_str());
                            if narrow {
                                return;
                            }
                            if !matches!(color, c if *c == theme::green()) {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(label.as_str())
                                            .size(12.0)
                                            .color(theme::text_dim()),
                                    )
                                    .truncate(),
                                );
                            }
                        });
                        ui.add(
                            egui::Label::new(theme::mono(subtitle, 11.5).color(theme::text_dim()))
                                .truncate(),
                        );
                    });
                });
            });
        });
}

/// Top bar of the full-screen mode: leave, switch session, address, actions.
fn focus_bar(
    ui: &mut egui::Ui,
    id: egui::Id,
    index: usize,
    subtitle: &str,
    info: &SplitInfo,
    right: impl FnOnce(&mut egui::Ui),
) {
    egui::Panel::top(id)
        .exact_size(56.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::side())
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let full = ui.max_rect();
            ui.painter().line_segment(
                [
                    Pos2::new(full.left() - 16.0, full.bottom() - 0.5),
                    Pos2::new(full.right() + 16.0, full.bottom() - 0.5),
                ],
                Stroke::new(1.0, theme::line()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                right(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    if theme::btn(ui, theme::Btn::Normal, Some(Icon::Exit), t!("Verlassen"))
                        .on_hover_text(t!("Vollbild verlassen (F11)"))
                        .clicked()
                    {
                        push_split_op(ui.ctx(), SplitOp::Focus(None));
                    }
                    // Session switcher: a segmented control with a status dot per session.
                    let (track, _) = ui.allocate_exact_size(
                        Vec2::new(
                            info.tabs
                                .iter()
                                .map(|(_, t)| switcher_width(ui, t))
                                .sum::<f32>()
                                + 2.0 * info.tabs.len() as f32
                                + 2.0,
                            34.0,
                        ),
                        Sense::hover(),
                    );
                    ui.painter()
                        .rect_filled(track, CornerRadius::same(10), theme::fill());
                    let mut x = track.left() + 2.0;
                    for (i, (tid, title)) in info.tabs.iter().enumerate() {
                        let w = switcher_width(ui, title);
                        let r = Rect::from_min_size(
                            Pos2::new(x, track.top() + 2.0),
                            Vec2::new(w, 30.0),
                        );
                        let resp = ui.interact(r, ui.id().with(("focus-tab", tid)), Sense::click());
                        let on = i + 1 == index;
                        if on {
                            ui.painter()
                                .rect_filled(r, CornerRadius::same(8), theme::seg_on());
                        } else if resp.hovered() {
                            ui.painter()
                                .rect_filled(r, CornerRadius::same(8), theme::hover());
                        }
                        let dot = info.live.get(i).copied().unwrap_or(false);
                        theme::led(
                            ui.painter(),
                            Pos2::new(r.left() + 17.0, r.center().y),
                            if dot {
                                theme::green()
                            } else {
                                theme::off_color()
                            },
                            false,
                        );
                        if dot {
                            ui.painter().circle_filled(
                                Pos2::new(r.left() + 17.0, r.center().y),
                                3.5,
                                theme::green(),
                            );
                        }
                        ui.painter().text(
                            Pos2::new(r.left() + 29.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            title,
                            theme::medium(13.0),
                            if on { theme::text() } else { theme::text_dim() },
                        );
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            push_split_op(ui.ctx(), SplitOp::Focus(Some(*tid)));
                        }
                        x += w + 2.0;
                    }
                    ui.add(
                        egui::Label::new(theme::mono(subtitle, 12.0).color(theme::text_dim()))
                            .truncate(),
                    );
                });
            });
        });
}

fn switcher_width(ui: &egui::Ui, title: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(title.to_string(), theme::medium(13.0), theme::text())
        .size()
        .x
        + 42.0
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
    fn dragged_dividers_resize_panes_and_clamp() {
        use super::{SplitLayout, SplitRatios, ratio_at, ratio_key, split_geometry};
        use egui::{Pos2, Rect, pos2};
        let area = Rect::from_min_max(Pos2::ZERO, pos2(1000.0, 600.0));
        let (r, d) = split_geometry(area, 2, SplitLayout::Columns, &SplitRatios::new());
        assert_eq!(d.len(), 1);
        assert!((r[0].width() - r[1].width()).abs() < 1.0);
        // Drag the divider to 30 % of the width.
        let ratio = ratio_at(&d[0], pos2(300.0, 100.0));
        let mut saved = SplitRatios::new();
        saved.insert(ratio_key(SplitLayout::Columns, 2), vec![ratio]);
        let (r2, _) = split_geometry(area, 2, SplitLayout::Columns, &saved);
        assert!((r2[0].width() - 295.0).abs() < 2.0, "{}", r2[0].width());
        // Never smaller than the minimum pane.
        saved.insert(ratio_key(SplitLayout::Columns, 2), vec![0.01]);
        let (r3, _) = split_geometry(area, 2, SplitLayout::Columns, &saved);
        assert!(r3[0].width() >= 110.0);
        // 0.0 means "not moved": equal division.
        saved.insert(ratio_key(SplitLayout::Columns, 2), vec![0.0]);
        let (r4, _) = split_geometry(area, 2, SplitLayout::Columns, &saved);
        assert!((r4[0].width() - r4[1].width()).abs() < 1.0);
        // Stacked panes have horizontal dividers.
        let (_, d) = split_geometry(area, 3, SplitLayout::Rows, &SplitRatios::new());
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| !x.vertical));
        // The grid has three dividers (one shared vertical position for both rows).
        let (_, g) = split_geometry(area, 4, SplitLayout::Grid, &SplitRatios::new());
        assert_eq!(g.len(), 3);
    }

    #[test]
    fn initials_are_short() {
        assert_eq!(super::initials("web-01"), "W0");
        assert_eq!(super::initials("Raspberry Pi"), "RP");
        assert_eq!(super::initials("db"), "DB");
        assert_eq!(super::initials("🗁 Local Test"), "LT");
        assert_eq!(super::initials(""), "··");
    }

    #[test]
    fn layouts_cover_the_area_without_overlap() {
        use super::{SplitLayout, SplitRatios, split_rects_for};
        use egui::{Pos2, Rect, pos2};
        let area = Rect::from_min_max(Pos2::ZERO, pos2(1000.0, 600.0));
        for layout in SplitLayout::ALL {
            for n in 1..=4usize {
                let r = split_rects_for(area, n, layout, &SplitRatios::new());
                assert_eq!(r.len(), n, "{layout:?} {n}");
                for (i, a) in r.iter().enumerate() {
                    assert!(area.contains_rect(*a), "{layout:?} {n} pane {i} outside");
                    assert!(a.width() > 50.0 && a.height() > 50.0);
                    for b in &r[i + 1..] {
                        assert!(!a.intersects(*b), "{layout:?} {n} overlap");
                    }
                }
            }
        }
        // Two panes stacked: same width, one above the other.
        let rows = split_rects_for(area, 2, SplitLayout::Rows, &SplitRatios::new());
        assert_eq!((rows[0].width(), rows[1].width()), (1000.0, 1000.0));
        assert!(rows[0].bottom() < rows[1].top());
        // Side by side.
        let cols = split_rects_for(area, 3, SplitLayout::Columns, &SplitRatios::new());
        assert!(cols[0].right() < cols[1].left() && cols[1].right() < cols[2].left());
    }

    #[test]
    fn drop_in_stacked_layout_uses_the_stacked_slots() {
        use super::{SplitLayout, SplitRatios, drop_order};
        use egui::{Pos2, Rect, pos2};
        let area = Rect::from_min_max(Pos2::ZERO, pos2(100.0, 100.0));
        // One pane shown, stacked: the upper half is slot 0, the lower half slot 1.
        assert_eq!(
            drop_order(
                area,
                &[1],
                7,
                pos2(50.0, 20.0),
                SplitLayout::Rows,
                &SplitRatios::new()
            ),
            Some(vec![7, 1])
        );
        assert_eq!(
            drop_order(
                area,
                &[1],
                7,
                pos2(50.0, 80.0),
                SplitLayout::Rows,
                &SplitRatios::new()
            ),
            Some(vec![1, 7])
        );
        // Moving a pane within the stack.
        assert_eq!(
            drop_order(
                area,
                &[1, 2, 3],
                3,
                pos2(50.0, 10.0),
                SplitLayout::Rows,
                &SplitRatios::new()
            ),
            Some(vec![3, 1, 2])
        );
    }

    #[test]
    fn drop_positions() {
        use super::{SplitLayout, SplitRatios, drop_order};
        use egui::{Pos2, Rect, pos2};
        let area = Rect::from_min_max(Pos2::ZERO, pos2(100.0, 100.0));
        let at = |x, y| pos2(x, y);
        assert_eq!(
            drop_order(
                area,
                &[],
                7,
                at(50.0, 50.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![7])
        );
        assert_eq!(
            drop_order(
                area,
                &[1],
                7,
                at(20.0, 50.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![7, 1])
        );
        assert_eq!(
            drop_order(
                area,
                &[1],
                7,
                at(80.0, 50.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![1, 7])
        );
        // two shown -> left column, top right or bottom right
        assert_eq!(
            drop_order(
                area,
                &[1, 2],
                7,
                at(80.0, 20.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![1, 7, 2])
        );
        assert_eq!(
            drop_order(
                area,
                &[1, 2],
                7,
                at(80.0, 80.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![1, 2, 7])
        );
        // already shown: moves
        assert_eq!(
            drop_order(
                area,
                &[1, 2, 3],
                3,
                at(20.0, 50.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![3, 1, 2])
        );
        // full: replaces the pane under the pointer
        assert_eq!(
            drop_order(
                area,
                &[1, 2, 3, 4],
                7,
                at(20.0, 80.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            Some(vec![1, 2, 7, 4])
        );
        assert_eq!(
            drop_order(
                area,
                &[1],
                7,
                at(150.0, 50.0),
                SplitLayout::Auto,
                &SplitRatios::new()
            ),
            None
        );
    }
}
