//! Connections overview: one search field (doubles as quick connect), a protocol filter and
//! the host table grouped by folder. Rows instead of cards keep many hosts scannable.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, ConnState, TabKind};
use crate::backend::reach::Reach;
use crate::model::{Host, Protocol};
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::maintenance::Target;
use crate::ui::ops::OpsAction;
use crate::ui::relative_time;

const ROW_H: f32 = crate::ui::nav_state::ROW_H;
const ROW_H_COMPACT: f32 = crate::ui::nav_state::ROW_H_COMPACT;

/// Visible host order of the last frame, for Shift+click ranges.
pub fn order_id() -> egui::Id {
    egui::Id::new("aurora-host-order")
}

pub fn protocol_color(p: Protocol) -> Color32 {
    match p {
        Protocol::Ssh => theme::accent(),
        Protocol::Rdp | Protocol::Vnc => theme::accent2(),
        Protocol::Serial | Protocol::Telnet => theme::green(),
        Protocol::Docker | Protocol::Kubernetes => theme::amber(),
        Protocol::Web => theme::accent2(),
    }
}

pub fn protocol_tag(p: Protocol) -> &'static str {
    match p {
        Protocol::Ssh => "SSH",
        Protocol::Rdp => "RDP",
        Protocol::Serial => "COM",
        Protocol::Telnet => "TELNET",
        Protocol::Vnc => "VNC",
        Protocol::Docker => "DOCKER",
        Protocol::Kubernetes => "K8S",
        Protocol::Web => "WEB",
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.lib.settings.reachability {
        app.reach.tick(&app.rt, &app.lib.hosts, ui.ctx());
    }
    // Header: title, search/quick connect, primary action.
    let mut enter = false;
    let mut pill_actions = Vec::new();
    let count = match (app.lib.nav.scope, app.nav.cache.as_ref()) {
        (Some(_), Some(c)) => c.shown.order.len(),
        _ => app.lib.hosts.len(),
    };
    let mut search_text = std::mem::take(&mut app.search);
    let app_ref = &*app;
    theme::page_header(
        ui,
        "hosts",
        |ui| {
            theme::page_title(ui, t!("Verbindungen"), Some(count));
            crate::ui::maintenance::header_pill(app_ref, ui, &mut pill_actions);
            crate::ui::nav_list::scope_chip(app_ref, ui, &mut pill_actions);
        },
        |_| {},
        |ui, bar| {
            let new = theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("Neu"));
            egui::Popup::menu(&new).show(|ui| {
                ui.set_min_width(210.0);
                for (label, proto) in [
                    ("SSH-Host", Protocol::Ssh),
                    ("Remote Desktop", Protocol::Rdp),
                    ("VNC", Protocol::Vnc),
                    ("Telnet", Protocol::Telnet),
                    ("Serielles Gerät", Protocol::Serial),
                    ("Docker-Container", Protocol::Docker),
                    ("Kubernetes-Pod", Protocol::Kubernetes),
                    ("Web-Oberfläche", Protocol::Web),
                ] {
                    if ui
                        .button(format!("{}  {}", proto.icon(), crate::i18n::tr(label)))
                        .clicked()
                    {
                        actions.push(Action::NewHostOf(proto, None));
                        ui.close();
                    }
                }
                ui.separator();
                if ui
                    .button(format!("{}  {}", ">_", t!("Lokales Terminal")))
                    .on_hover_text("Ctrl+Shift+T")
                    .clicked()
                {
                    actions.push(Action::LocalTerminal);
                    ui.close();
                }
                if ui.button(t!("Container durchsuchen…")).clicked() {
                    actions.push(Action::BrowseContainers(Protocol::Docker, None, false));
                    ui.close();
                }
                if ui.button(t!("Gruppe")).clicked() {
                    actions.push(Action::NewGroup(None));
                    ui.close();
                }
                if ui.button(t!("Netz durchsuchen…")).clicked() {
                    actions.push(Action::OpenScan);
                    ui.close();
                }
            });
            if crate::ui::certradar::header_button(app_ref, ui).clicked() {
                actions.push(Action::Net(crate::ui::net::NetAction::OpenRadar));
            }
            let scan_tip = t!("Geräte mit SSH/RDP im lokalen Netz finden und übernehmen");
            let scan = if bar.compact {
                theme::icon_btn(
                    ui,
                    Icon::Wifi,
                    &format!("{}\n{}", t!("Netz scannen"), scan_tip),
                )
            } else {
                theme::btn(ui, theme::Btn::Normal, Some(Icon::Wifi), t!("Netz scannen"))
                    .on_hover_text(scan_tip)
            };
            if scan.clicked() {
                actions.push(Action::OpenScan);
            }
            let width = bar.flex(if bar.compact { 150.0 } else { 190.0 }, 300.0);
            let search = theme::search_field(
                ui,
                &mut search_text,
                t!("Suchen oder user@host verbinden"),
                width,
                egui::Id::new("host-search"),
            );
            enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if app_ref.nav.focus_search {
                search.request_focus();
                let id = egui::Id::new("host-search");
                if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                    let n = search_text.chars().count();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(n),
                    )));
                    st.store(ui.ctx(), id);
                }
            }
        },
    );
    app.search = search_text;
    app.nav.focus_search = false;
    actions.append(&mut pill_actions);
    ui.add_space(18.0);

    if app.lib.hosts.is_empty() && app.search.trim().is_empty() {
        empty_state(ui, actions);
        return;
    }

    let mut filter = app.host_filter;
    let scope_label = crate::ui::nav_state::scope_name(&app.lib).map(str::to_string);
    let view_mode = app.lib.nav.mode(app.lib.nav.scope);
    let settings = &mut app.lib.settings;
    let nav_state = &mut app.nav;
    let search_now = app.search.trim().to_string();
    let mut changed = false;
    theme::toolbar(
        ui,
        "host-filter",
        |ui| {
            if let Some(i) = theme::segmented(
                ui,
                &[
                    "Alle",
                    "SSH",
                    "Desktop",
                    "Konsole",
                    "Container",
                    "Favoriten",
                    "Web",
                ]
                .map(crate::i18n::tr),
                filter,
            ) {
                filter = i;
            }
        },
        |ui, bar| {
            let compact = settings.compact;
            let kind = if compact {
                theme::Btn::SmallPrimary
            } else {
                theme::Btn::Small
            };
            if theme::btn(ui, kind, None, t!("Kompakt"))
                .on_hover_text(t!("Kleinere Zeilen für viele Hosts"))
                .clicked()
            {
                settings.compact = !compact;
                changed = true;
            }
            let labels = ["Name", "Zuletzt verwendet", "Online zuerst"];
            egui::ComboBox::from_id_salt("host-sort")
                .selected_text(
                    RichText::new(crate::i18n::tr(labels[settings.sort.min(2) as usize]))
                        .font(theme::medium(13.0)),
                )
                .width(if bar.compact { 120.0 } else { 150.0 })
                .show_ui(ui, |ui| {
                    for (i, l) in labels.iter().enumerate() {
                        changed |= ui
                            .selectable_value(&mut settings.sort, i as u8, crate::i18n::tr(l))
                            .changed();
                    }
                });
            if !bar.compact {
                ui.label(
                    RichText::new(t!("Sortierung"))
                        .size(13.0)
                        .color(theme::text_dim()),
                );
            }
            crate::ui::nav_list::toolbar_extras(
                ui,
                bar.compact,
                &settings.saved_views,
                view_mode,
                &search_now,
                scope_label.as_deref(),
                nav_state,
                actions,
            );
        },
    );
    app.host_filter = filter;
    if changed {
        app.save_library();
    }
    ui.add_space(14.0);
    selection_bar(app, ui, actions);
    let live = live_hosts(app);
    // Loose hosts that share a /24: offer a group (small sets only).
    if app
        .nav
        .cache
        .as_ref()
        .is_some_and(|c| !c.filtering && c.shown.loose.len() >= 3 && c.shown.loose.len() <= 300)
        && app.search.trim().is_empty()
    {
        let loose: Vec<Host> = app
            .nav
            .cache
            .as_ref()
            .map(|c| {
                c.shown
                    .loose
                    .iter()
                    .map(|e| app.lib.hosts[c.idx.entries[*e as usize].host].clone())
                    .collect()
            })
            .unwrap_or_default();
        subnet_hint(app, ui, &loose, actions);
    }
    crate::ui::nav_list::body(app, ui, actions, enter, &live);
}

/// Ids of hosts that currently have a connected session.
pub(crate) fn live_hosts(app: &App) -> Vec<String> {
    app.tabs
        .iter()
        .filter(|t| match &t.kind {
            TabKind::Term(t) => matches!(t.state, ConnState::Connected),
            TabKind::Sftp(t) => matches!(t.state(), ConnState::Connected),
            TabKind::Rdp(t) => t.is_live(),
            TabKind::Vnc(t) => matches!(t.view.state, ConnState::Connected),
            TabKind::Web(t) => t.is_live(),
            TabKind::Log(_) => false,
            TabKind::Edit(t) => matches!(t.state(), ConnState::Connected),
        })
        .flat_map(|t| {
            [
                t.title.clone(),
                t.host_id.map(|h| h.to_string()).unwrap_or_default(),
            ]
        })
        .collect()
}

pub(crate) fn passes_filter(filter: usize, h: &Host) -> bool {
    match filter {
        1 => h.protocol == Protocol::Ssh,
        2 => matches!(h.protocol, Protocol::Rdp | Protocol::Vnc),
        3 => matches!(h.protocol, Protocol::Serial | Protocol::Telnet),
        4 => h.protocol.is_container(),
        5 => h.favorite,
        6 => h.protocol == Protocol::Web,
        _ => true,
    }
}

fn empty_state(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.add_space(40.0);
    theme::card(ui, theme::bg_deep(), |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(28, 28))
            .show(ui, |ui| {
                ui.set_max_width(560.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                theme::logo(ui, 44.0);
                ui.add_space(10.0);
                ui.label(theme::heading(t!("Noch keine Verbindungen"), 22.0));
                ui.label(
                    RichText::new(t!("Lege deine erste Verbindung an. Für einen schnellen Test reicht user@host oben im Suchfeld."))
                        .color(theme::text_dim()),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("SSH-Host anlegen")).clicked() {
                        actions.push(Action::NewHostOf(Protocol::Ssh, None));
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Remote Desktop")).clicked() {
                        actions.push(Action::NewHostOf(Protocol::Rdp, None));
                    }
                    if theme::btn(ui, theme::Btn::Normal, None, t!("Serielles Gerät")).clicked() {
                        actions.push(Action::NewHostOf(Protocol::Serial, None));
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t!("Oder such dein Netz nach SSH-/RDP-Geräten ab:"))
                            .color(theme::text_dim()),
                    );
                    if theme::btn(ui, theme::Btn::Small, Some(Icon::Wifi), t!("Netz scannen")).clicked() {
                        actions.push(Action::OpenScan);
                    }
                });
                let found = crate::app::ssh_config_hosts().len();
                if found > 0 {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t!("In deiner ~/.ssh/config stehen {} Hosts.", found))
                                .color(theme::text_dim()),
                        );
                        if theme::btn(ui, theme::Btn::Small, None, t!("Importieren")).clicked() {
                            actions.push(Action::ImportSshConfig);
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t!("Du kommst von mRemoteNG, Royal TS oder RDM?"))
                            .color(theme::text_dim()),
                    );
                    if theme::btn(ui, theme::Btn::Small, None, t!("Importieren")).clicked() {
                        actions.push(Action::OpenImport);
                    }
                });
            });
    });
}

/// Bulk actions for the multi-selection (Ctrl/Shift+click).
fn selection_bar(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let n = app.selected_hosts.len();
    if n == 0 {
        return;
    }
    let ids: Vec<Uuid> = app.selected_hosts.iter().copied().collect();
    egui::Frame::NONE
        .fill(theme::accent_soft())
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(
                    RichText::new(t!("{} ausgewählt", n))
                        .font(theme::semibold(14.0))
                        .color(theme::text()),
                );
                ui.add_space(8.0);
                if theme::btn(
                    ui,
                    theme::Btn::SmallPrimary,
                    None,
                    if n > 1 {
                        t!("Öffnen (nebeneinander)")
                    } else {
                        t!("Öffnen")
                    },
                )
                .clicked()
                {
                    actions.push(Action::ConnectMany(ids.clone()));
                }
                if theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(Icon::Terminal),
                    t!("Befehl ausführen…"),
                )
                .on_hover_text(t!(
                    "Einen Befehl oder ein Snippet auf allen gewählten Hosts ausführen"
                ))
                .clicked()
                {
                    actions.push(Action::Srv(crate::ui::multirun_ui::SrvAction::Run(
                        ids.clone(),
                        t!("Auswahl ({} Hosts)", n),
                    )));
                }
                let r = theme::btn(ui, theme::Btn::Small, None, t!("In Gruppe"));
                egui::Popup::menu(&r).show(|ui| {
                    if ui.button(t!("Ohne Gruppe")).clicked() {
                        actions.push(Action::MoveHosts(ids.clone(), None));
                        ui.close();
                    }
                    for g in &app.lib.groups {
                        if ui.button(&g.name).clicked() {
                            actions.push(Action::MoveHosts(ids.clone(), Some(g.id)));
                            ui.close();
                        }
                    }
                });
                let r = theme::btn(ui, theme::Btn::Small, None, t!("Umgebung"));
                egui::Popup::menu(&r).show(|ui| {
                    for e in crate::model::Env::ALL {
                        let label = if e == crate::model::Env::None {
                            t!("keine")
                        } else {
                            e.label()
                        };
                        if ui.button(label).clicked() {
                            actions.push(Action::SetEnvMany(ids.clone(), e));
                            ui.close();
                        }
                    }
                });
                if theme::btn(ui, theme::Btn::Small, None, t!("Löschen")).clicked() {
                    actions.push(Action::Confirm(
                        t!("{} Hosts löschen?", n),
                        Box::new(Action::DeleteHosts(ids.clone())),
                    ));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_btn(ui, Icon::Close, t!("Auswahl aufheben (Esc)")).clicked() {
                        actions.push(Action::ClearSelection);
                    }
                    ui.label(
                        RichText::new(t!("Ctrl/Shift+Klick wählt aus · ziehen in Gruppen"))
                            .size(12.0)
                            .color(theme::text_dim()),
                    );
                });
            });
        });
    ui.add_space(12.0);
}

fn subnet_hint(app: &App, ui: &mut egui::Ui, loose: &[Host], actions: &mut Vec<Action>) {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for h in loose {
        if let Some(net) = crate::model::subnet24(&h.hostname) {
            match counts.iter_mut().find(|(n, _)| *n == net) {
                Some((_, c)) => *c += 1,
                None => counts.push((net, 1)),
            }
        }
    }
    let Some((net, n)) = counts
        .into_iter()
        .filter(|(net, c)| *c >= 3 && !app.lib.settings.dismissed_subnets.contains(net))
        .max_by_key(|(_, c)| *c)
    else {
        return;
    };
    ui.add_space(22.0);
    egui::Frame::NONE
        .fill(theme::lerp(theme::bg_deep(), theme::accent(), 0.16))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                icons::show(ui, Icon::Sparkle, 18.0, theme::accent());
                ui.add_space(8.0);
                ui.label(
                    RichText::new(t!("{} Hosts liegen im Netz {}.x.", n, net))
                        .font(theme::semibold(14.0))
                        .color(theme::text()),
                );
                ui.label(
                    RichText::new(t!("Zusammen als Gruppe ablegen?"))
                        .size(14.0)
                        .color(theme::text_dim()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::btn(ui, theme::Btn::Normal, Some(Icon::Close), "")
                        .on_hover_text(t!("Nicht mehr vorschlagen"))
                        .clicked()
                    {
                        actions.push(Action::DismissSubnet(net.clone()));
                    }
                    if theme::btn(ui, theme::Btn::Primary, None, t!("Gruppe anlegen"))
                        .on_hover_text(t!("Gruppe „LAN {}“ anlegen", net))
                        .clicked()
                    {
                        actions.push(Action::GroupSubnet(net.clone()));
                    }
                });
            });
        });
}

pub(crate) fn quick_row(ui: &mut egui::Ui, target: &str, actions: &mut Vec<Action>, query: &str) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(14),
        if resp.hovered() {
            theme::soft(theme::accent()).gamma_multiply(1.4)
        } else {
            theme::accent_soft()
        },
    );
    icons::paint(
        p,
        Rect::from_center_size(rect.left_center() + Vec2::new(26.0, 0.0), Vec2::splat(18.0)),
        Icon::Terminal,
        theme::accent(),
    );
    p.text(
        rect.left_center() + Vec2::new(48.0, 0.0),
        egui::Align2::LEFT_CENTER,
        t!("Direkt verbinden"),
        theme::semibold(14.5),
        theme::text(),
    );
    p.text(
        rect.left_center() + Vec2::new(190.0, 0.0),
        egui::Align2::LEFT_CENTER,
        target,
        FontId::monospace(13.0),
        theme::accent(),
    );
    p.text(
        rect.right_center() - Vec2::new(18.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        "Enter ↵",
        theme::medium(12.0),
        theme::text_dim(),
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        actions.push(Action::QuickConnect(query.to_string()));
    }
    ui.add_space(12.0);
}

fn uptime_text(secs: u64) -> String {
    let d = secs / 86_400;
    match (d, secs / 3600) {
        (d, _) if d > 0 => format!("up {d} d"),
        (_, h) if h > 0 => format!("up {h} h"),
        _ => format!("up {} min", secs / 60),
    }
}

/// "…" menu of a narrow host row (keeps the row in its hover state while open).
fn more_menu_id(id: Uuid) -> egui::Id {
    egui::Id::new(("host-row-more", id))
}

pub fn rename_id() -> egui::Id {
    egui::Id::new("aurora-rename")
}

/// A label on the right side of a host row.
struct RowTag {
    text: String,
    color: Option<Color32>,
    /// Status dot in front (tunnels): Some(lit).
    dot: Option<bool>,
    tunnel: Option<Uuid>,
    tip: String,
}

/// "Wartung starten…" or, while one runs, extend/end it.
pub(crate) fn maintenance_menu(
    ui: &mut egui::Ui,
    active: Option<&crate::ui::maintenance::Maintenance>,
    target: Target,
    actions: &mut Vec<Action>,
) {
    match active {
        Some(m) => {
            ui.menu_button(t!("Wartung"), |ui| {
                ui.label(RichText::new(m.banner()).size(12.5).color(theme::amber()));
                if ui.button(t!("Um 30 Minuten verlängern")).clicked() {
                    actions.push(Action::Ops(OpsAction::MaintExtend(m.id, 30)));
                    ui.close();
                }
                if ui.button(t!("Wartung beenden")).clicked() {
                    actions.push(Action::Ops(OpsAction::MaintEnd(m.id)));
                    ui.close();
                }
            });
        }
        None => {
            if ui
                .button(t!("Wartung starten…"))
                .on_hover_text(t!(
                    "Wartungsfenster: Warnungen für eine Zeit stummschalten, mit Banner"
                ))
                .clicked()
            {
                actions.push(Action::Ops(OpsAction::MaintStart(target)));
                ui.close();
            }
        }
    }
}

pub(crate) fn host_row(
    app: &App,
    ui: &mut egui::Ui,
    h: &Host,
    live: &[String],
    divider: bool,
    path: Option<&str>,
    actions: &mut Vec<Action>,
) {
    let compact = app.lib.settings.compact;
    let row_h = if compact { ROW_H_COMPACT } else { ROW_H };
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), row_h),
        Sense::click_and_drag(),
    );
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let selected = app.selected_hosts.contains(&h.id);
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if selected {
        p.rect_filled(rect, CornerRadius::ZERO, theme::accent_soft());
    } else if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if divider {
        p.line_segment(
            [
                Pos2::new(rect.left() + 44.0, rect.top()),
                Pos2::new(rect.right(), rect.top()),
            ],
            Stroke::new(1.0, theme::line()),
        );
    }
    // Drag the row (or the whole selection) onto a group header.
    resp.dnd_set_drag_payload(h.id);
    if resp.dragged() {
        let n = if selected {
            app.selected_hosts.len()
        } else {
            1
        };
        if let Some(pos) = ui.ctx().pointer_latest_pos() {
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("host-drag"),
            ));
            let text = if n > 1 {
                t!("{} Hosts", n)
            } else {
                h.display_name().to_string()
            };
            let g = painter.layout_no_wrap(text, theme::semibold(13.0), theme::text());
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

    let cy = rect.center().y;
    let (y1, y2) = if compact {
        (cy, cy)
    } else {
        (rect.top() + 24.0, rect.top() + 44.0)
    };
    // Dot: green = open session or reachable, ring = no answer / not checked,
    // red = the last connection attempt failed.
    let id = h.id.to_string();
    let is_live = live.iter().any(|t| *t == id || t == h.display_name());
    let reach = app
        .lib
        .settings
        .reachability
        .then(|| app.reach.get(h.id))
        .flatten();
    let led_pos = Pos2::new(rect.left() + 22.0, cy);
    let maint = crate::ui::maintenance::active_for(&app.lib, h.id, chrono::Utc::now());
    if maint.is_some() && !is_live {
        // Down or failed is expected during maintenance.
        theme::led(&p, led_pos, theme::amber(), true);
    } else if app.failed.contains(&h.id) && !is_live {
        theme::led(&p, led_pos, theme::red(), true);
    } else {
        match (is_live, reach) {
            (true, _) | (false, Some(Reach::Up(_))) => {
                theme::led(&p, led_pos, theme::green(), true)
            }
            _ => theme::led(&p, led_pos, theme::off_color(), false),
        }
    }
    {
        let tip_rect = Rect::from_center_size(led_pos, Vec2::splat(18.0));
        let tip = if let Some(m) = maint.filter(|_| !is_live) {
            m.banner()
        } else if app.failed.contains(&h.id) && !is_live {
            t!("letzte Verbindung fehlgeschlagen").to_string()
        } else if is_live {
            t!("Session offen").to_string()
        } else {
            match reach {
                Some(Reach::Up(ms)) => t!("erreichbar · {} ms", ms),
                Some(Reach::Down) => t!("keine Antwort – vermutlich aus").to_string(),
                None => t!("noch nicht geprüft").to_string(),
            }
        };
        ui.interact(tip_rect, ui.id().with(("reach", h.id)), Sense::hover())
            .on_hover_text(tip);
    }

    let col_name = rect.left() + 42.0;

    // --- the right side decides first where the text has to stop.
    let show_actions = hovered
        || resp.context_menu_opened()
        || egui::Popup::is_id_open(ui.ctx(), more_menu_id(h.id));
    let offline = !is_live && (app.failed.contains(&h.id) || matches!(reach, Some(Reach::Down)));
    let can_files = matches!(h.protocol, Protocol::Ssh | Protocol::Rdp);
    let can_wake = offline && crate::ui::wol::can_wake(h) && app.net.wol.waiting(h.id).is_none();
    let connect_w = theme::btn_width(ui, theme::Btn::SmallPrimary, None, t!("Verbinden"));
    let mut full_w =
        connect_w + 6.0 + theme::btn_width(ui, theme::Btn::Small, None, t!("Bearbeiten"));
    if can_files {
        full_w += 6.0 + theme::btn_width(ui, theme::Btn::Small, None, t!("Dateien"));
    }
    if can_wake {
        full_w += 6.0 + theme::btn_width(ui, theme::Btn::Small, Some(Icon::Power), t!("Aufwecken"));
    }
    // Narrow rows keep the main button and move the rest into a "…" menu.
    let collapsed = rect.width() - 42.0 - 150.0 - 8.0 < full_w;
    let strip_w = if collapsed {
        connect_w + 6.0 + theme::btn_width(ui, theme::Btn::Small, None, "…")
    } else {
        full_w
    };
    // Meta (latency, last use): a fixed column on wide rows, hidden on very narrow ones.
    let mut meta_parts = Vec::new();
    if let Some(Reach::Up(ms)) = reach {
        meta_parts.push(format!("{ms} ms"));
    }
    if let Some(t) = h.last_connected {
        meta_parts.push(relative_time(t));
    }
    let meta = meta_parts.join(" · ");
    let wol_pill = app.net.wol.pill_text(h.id);
    let meta_w = if wol_pill.is_some() {
        150.0
    } else {
        p.layout_no_wrap(meta.clone(), FontId::proportional(12.5), theme::text_dim())
            .size()
            .x
    };
    let slot = if meta_w <= 0.5 || rect.width() < 470.0 {
        0.0
    } else if rect.width() >= 640.0 {
        meta_w.max(150.0)
    } else {
        meta_w
    };
    let strip_left = rect.right() - 8.0 - strip_w;
    let text_right = if show_actions {
        strip_left - 14.0
    } else if slot > 0.0 {
        rect.right() - 16.0 - slot - 12.0
    } else {
        rect.right() - 16.0
    };
    // Tiny CPU/RAM graphs (setting) take room left of the meta column.
    let spark_w = if show_actions {
        0.0
    } else {
        crate::ui::hoststats_ui::row_spark_width(app, h, rect.width())
    };
    let spark_right = text_right;
    let text_right = text_right - spark_w;
    let name_max = if compact {
        ((text_right - col_name) * 0.5).max(60.0)
    } else {
        (text_right - col_name).max(40.0)
    };

    // --- name (or inline rename)
    let renaming: Option<(Uuid, String)> = ui.ctx().data(|d| d.get_temp(rename_id()));
    let mut nx;
    if let Some((_, mut text)) = renaming.filter(|(rid, _)| *rid == h.id) {
        let r = Rect::from_min_size(
            Pos2::new(col_name - 6.0, y1 - 14.0),
            Vec2::new((text_right - col_name + 6.0).clamp(120.0, 260.0), 28.0),
        );
        let edit = ui.put(
            r,
            egui::TextEdit::singleline(&mut text)
                .font(theme::semibold(14.5))
                .hint_text(h.hostname.as_str()),
        );
        edit.request_focus();
        let (enter, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if enter || (edit.lost_focus() && !esc) {
            actions.push(Action::RenameHost(h.id, text));
            ui.ctx()
                .data_mut(|d| d.remove::<(Uuid, String)>(rename_id()));
        } else if esc {
            ui.ctx()
                .data_mut(|d| d.remove::<(Uuid, String)>(rename_id()));
        } else {
            ui.ctx()
                .data_mut(|d| d.insert_temp(rename_id(), (h.id, text)));
        }
        nx = r.right() + 8.0;
    } else {
        let gone = h.dynamic.as_ref().is_some_and(|d| d.gone);
        let name_galley = theme::truncated(
            &p,
            h.display_name(),
            theme::semibold(15.0),
            if gone {
                theme::text_dim()
            } else {
                theme::text()
            },
            name_max,
        );
        let name_w = name_galley.size().x;
        p.galley(
            Pos2::new(col_name, y1 - name_galley.size().y / 2.0),
            name_galley,
            theme::text(),
        );
        nx = col_name + name_w + 8.0;
    }
    if h.favorite && nx + 14.0 <= text_right {
        icons::paint(
            &p,
            Rect::from_center_size(Pos2::new(nx + 6.0, y1), Vec2::splat(13.0)),
            Icon::Star,
            Color32::from_rgb(255, 214, 10),
        );
        nx += 20.0;
    }
    let env = app.lib.env_of(h);
    let pill_w = p
        .layout_no_wrap(env.label().to_uppercase(), theme::bold(10.5), theme::text())
        .size()
        .x
        + 14.0;
    if env != crate::model::Env::None && nx + pill_w <= text_right {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    Pos2::new(nx, y1 - 9.0),
                    Vec2::new(90.0, 18.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let r = theme::pill(&mut child, env.label(), theme::env_color(env));
        nx = r.rect.right() + 8.0;
    }
    if !h.notes.trim().is_empty() && nx + 14.0 <= text_right {
        let r = Rect::from_center_size(Pos2::new(nx + 7.0, y1), Vec2::splat(14.0));
        icons::paint(&p, r, Icon::File, theme::text_dim());
        ui.interact(r, ui.id().with(("notes", h.id)), Sense::hover())
            .on_hover_text(h.notes.trim());
        nx += 20.0;
    }
    // Entries a dynamic folder generated carry a small badge.
    nx = crate::ui::nav2_dyn::host_badge(&p, ui, &app.lib, h, nx, y1, text_right);

    // Search results: where the host lives (customer / site).
    if let Some(path) = path
        && nx + 60.0 <= text_right
    {
        let g = theme::truncated(
            &p,
            path,
            FontId::proportional(12.0),
            theme::text_dim(),
            text_right - nx,
        );
        let gw = g.size().x;
        p.galley(Pos2::new(nx, y1 - g.size().y / 2.0), g, theme::text_dim());
        nx += gw + 8.0;
    }

    // --- address: only what the name doesn't already say; default ports are hidden.
    let named = !h.label.trim().is_empty();
    let addr = match h.protocol {
        Protocol::Serial => format!("{} · {} Bd", h.serial.port_name, h.serial.baud_rate),
        Protocol::Docker | Protocol::Kubernetes => format!(
            "{}{}@ {}",
            if named { h.subtitle() } else { String::new() },
            if named { " " } else { "" },
            match h.container.via.and_then(|v| app.lib.host(v)) {
                Some(v) => v.display_name().to_string(),
                None => t!("dieser PC").to_string(),
            }
        ),
        _ => {
            let port = if h.port == h.protocol.default_port() {
                String::new()
            } else {
                format!(":{}", h.port)
            };
            match (h.username.is_empty(), named) {
                (false, true) => format!("{}@{}{}", h.username, h.hostname, port),
                (false, false) => format!("{}{}", h.username, port),
                (true, true) => format!("{}{}", h.hostname, port),
                (true, false) => port.trim_start_matches(':').to_string(),
            }
        }
    };
    // Facts from the server and the jump host follow the address.
    let mut extra: Vec<(String, Color32)> = Vec::new();
    let chain = crate::backend::route::jump_chain_lossy(&app.lib, h);
    if !chain.is_empty() {
        extra.push((
            t!(
                "via {}",
                crate::backend::route::chain_names(&app.lib, &chain)
            ),
            theme::text_dim(),
        ));
    }
    let facts = app.lib.facts.get(&h.id);
    if let Some(f) = facts {
        let os = f.short_os();
        if !os.is_empty() {
            extra.push((os, theme::text_dim()));
        }
        if f.uptime_secs > 0 {
            extra.push((uptime_text(f.uptime_secs), theme::text_dim()));
        }
        if let Some(d) = f.disk_used_pct {
            let c = if d >= 90 {
                theme::red()
            } else if d >= 80 {
                theme::amber()
            } else {
                theme::text_dim()
            };
            extra.push((format!("{d} %"), c));
        }
    }
    let (ax, ay) = if compact {
        (nx.max(col_name + 180.0_f32.min(name_max)) + 8.0, y1)
    } else {
        (col_name, y2)
    };
    // Address and facts as one line, cut with "…" where the right side begins.
    let mut job = egui::text::LayoutJob::default();
    let format = |font: FontId, color: Color32| egui::TextFormat {
        font_id: font,
        color,
        valign: egui::Align::Center,
        ..Default::default()
    };
    let addr_empty = addr.is_empty();
    if !addr_empty {
        job.append(
            &addr,
            0.0,
            format(FontId::monospace(12.0), theme::text_dim()),
        );
    }
    let addr_w = if addr_empty {
        0.0
    } else {
        p.layout_no_wrap(addr, FontId::monospace(12.0), theme::text_dim())
            .size()
            .x
    };
    for (i, (text, color)) in extra.iter().enumerate() {
        let sep = if i == 0 && addr_empty { "" } else { "  ·  " };
        job.append(
            &format!("{sep}{text}"),
            0.0,
            format(FontId::proportional(12.5), *color),
        );
    }
    let mut x = ax;
    let info_start;
    if text_right - ax >= 30.0 && !job.sections.is_empty() {
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_right - ax);
        let g = p.layout_job(job);
        let gw = g.size().x;
        p.galley(Pos2::new(ax, ay - g.size().y / 2.0), g, theme::text_dim());
        info_start = (ax + addr_w).min(ax + gw);
        x += gw;
    } else {
        info_start = x;
    }
    if let Some(f) = facts {
        let info_rect = Rect::from_min_max(
            Pos2::new(info_start, ay - 9.0),
            Pos2::new(x.max(info_start + 1.0), ay + 9.0),
        );
        let mut tip = format!("{}\nHostname: {}", f.os_name, f.hostname);
        if let Some(t) = f.collected {
            tip.push_str(&t!("\nStand: {}", relative_time(t)));
        }
        if f.updates.is_some_and(|n| n > 0) {
            tip.push_str(t!("\nRechtsklick → Updates installieren"));
        }
        ui.interact(info_rect, ui.id().with(("facts", h.id)), Sense::hover())
            .on_hover_text(tip);
    }
    if hovered && ui.input(|i| i.key_pressed(egui::Key::F2)) {
        ui.ctx()
            .data_mut(|d| d.insert_temp(rename_id(), (h.id, h.label.clone())));
    }

    // --- tags on the right: updates, protocol, tags, tunnels.
    let mut tags: Vec<RowTag> = Vec::new();
    if let Some(m) = maint {
        tags.push(RowTag {
            text: t!("Wartung {}", m.until_text()),
            color: Some(theme::amber()),
            dot: None,
            tunnel: None,
            tip: m.banner(),
        });
    }
    if let Some(n) = facts.and_then(|f| f.updates).filter(|n| *n > 0) {
        tags.push(RowTag {
            text: if n == 1 {
                t!("{} Update", n)
            } else {
                t!("{} Updates", n)
            },
            color: Some(theme::amber()),
            dot: None,
            tunnel: None,
            tip: t!("Rechtsklick → Updates installieren").into(),
        });
    }
    if h.protocol != Protocol::Ssh {
        tags.push(RowTag {
            text: protocol_tag(h.protocol).to_string(),
            color: Some(protocol_color(h.protocol)),
            dot: None,
            tunnel: None,
            tip: String::new(),
        });
    }
    for t in h.tags.iter().take(2) {
        tags.push(RowTag {
            text: t.clone(),
            color: None,
            dot: None,
            tunnel: None,
            tip: String::new(),
        });
    }
    for t in &h.tunnels {
        use crate::app::TunnelState;
        let run = app.tunnels.get(&t.id);
        let (color, tip) = match run.map(|r| &r.state) {
            Some(TunnelState::Up) => (
                Some(theme::green()),
                t!(
                    "offen: localhost:{} → {}:{} · {} Verbindung(en)\nKlick schließt den Tunnel",
                    t.local_port,
                    t.remote_host,
                    t.remote_port,
                    run.map(|r| r.conns).unwrap_or(0)
                ),
            ),
            Some(TunnelState::Connecting) => (Some(theme::amber()), t!("verbindet…").to_string()),
            Some(TunnelState::Failed(e)) => (Some(theme::red()), t!("{}\nKlick startet neu", e)),
            None => (
                None,
                t!(
                    "localhost:{} → {}:{}\nKlick öffnet den Tunnel",
                    t.local_port,
                    t.remote_host,
                    t.remote_port
                ),
            ),
        };
        tags.push(RowTag {
            text: if t.name.trim().is_empty() {
                format!(":{}", t.local_port)
            } else {
                t.name.clone()
            },
            color,
            dot: Some(run.is_some()),
            tunnel: Some(t.id),
            tip,
        });
    }

    if !show_actions {
        if spark_w > 0.0 {
            crate::ui::hoststats_ui::row_spark(
                app,
                &p,
                h,
                Rect::from_min_max(
                    Pos2::new(text_right + 8.0, cy - 10.0),
                    Pos2::new(spark_right - 2.0, cy + 10.0),
                ),
            );
        }
        // Meta: latency and last use.
        if slot > 0.0 {
            if let Some(text) = &wol_pill {
                crate::ui::wol::paint_pill(&p, Pos2::new(rect.right() - 16.0, cy), text);
            } else {
                p.text(
                    Pos2::new(rect.right() - 16.0, cy),
                    egui::Align2::RIGHT_CENTER,
                    meta,
                    FontId::proportional(12.5),
                    theme::text_dim(),
                );
            }
        }
        let mut tx = text_right;
        let left_limit = x.max(nx) + 16.0;
        for tag in tags.iter().rev() {
            let fg = tag.color.unwrap_or_else(theme::text_dim);
            let bg = tag.color.map(theme::soft).unwrap_or_else(theme::fill);
            let g = p.layout_no_wrap(tag.text.clone(), theme::medium(12.0), fg);
            let dot_w = if tag.dot.is_some() { 12.0 } else { 0.0 };
            let w = g.size().x + 18.0 + dot_w;
            let r = Rect::from_min_size(Pos2::new(tx - w, cy - 11.0), Vec2::new(w, 22.0));
            if r.left() < left_limit {
                break;
            }
            let hot = tag.tunnel.is_some() && ui.rect_contains_pointer(r);
            p.rect_filled(
                r,
                CornerRadius::same(11),
                if hot { theme::fill_strong() } else { bg },
            );
            if let Some(lit) = tag.dot {
                let c = Pos2::new(r.left() + 13.0, cy);
                if lit {
                    p.circle_filled(c, 3.0, fg);
                } else {
                    p.circle_stroke(c, 2.8, Stroke::new(1.3, fg));
                }
            }
            p.galley(
                Pos2::new(r.left() + 9.0 + dot_w, cy - g.size().y / 2.0),
                g,
                fg,
            );
            if let Some(tid) = tag.tunnel {
                let tr = ui.interact(r, ui.id().with(("tunnel", tid)), Sense::click());
                if tr
                    .on_hover_text(tag.tip.as_str())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    actions.push(Action::ToggleTunnel(h.id, tid));
                }
            } else if !tag.tip.is_empty() {
                ui.interact(
                    r,
                    ui.id().with(("tag", h.id, tag.text.as_str())),
                    Sense::hover(),
                )
                .on_hover_text(tag.tip.as_str());
            }
            tx = r.left() - 6.0;
        }
    } else {
        // Opaque strip (row colour, short fade) so nothing shows through the buttons.
        let layers = [
            theme::surface(),
            if selected {
                theme::accent_soft()
            } else {
                theme::hover()
            },
        ];
        theme::fade_cover(
            &p,
            Rect::from_min_max(
                Pos2::new(
                    strip_left - 6.0,
                    rect.top() + if divider { 1.0 } else { 0.0 },
                ),
                rect.right_bottom(),
            ),
            16.0,
            &layers,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(strip_left - 4.0, rect.top()),
                    rect.right_bottom() - Vec2::new(8.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 6.0;
        if theme::btn(&mut child, theme::Btn::SmallPrimary, None, t!("Verbinden"))
            .on_hover_text(t!("Verbinden (Doppelklick)"))
            .clicked()
        {
            actions.push(Action::Connect(h.id));
        }
        if collapsed {
            let more = theme::btn(&mut child, theme::Btn::Small, None, "…")
                .on_hover_text(t!("Weitere Aktionen"));
            egui::Popup::menu(&more).id(more_menu_id(h.id)).show(|ui| {
                if can_files && ui.button(t!("Dateien (SFTP)")).clicked() {
                    actions.push(Action::OpenSftp(h.id));
                    ui.close();
                }
                if ui.button(t!("Bearbeiten")).clicked() {
                    actions.push(Action::EditHost(h.id));
                    ui.close();
                }
                if can_wake && ui.button(t!("Aufwecken und verbinden")).clicked() {
                    actions.push(Action::Net(crate::ui::net::NetAction::Wake(h.id, true)));
                    ui.close();
                }
            });
        } else if can_files
            && theme::btn(&mut child, theme::Btn::Small, None, t!("Dateien"))
                .on_hover_text(t!("Dateien (SFTP) öffnen"))
                .clicked()
        {
            actions.push(Action::OpenSftp(h.id));
        }
        if !collapsed
            && theme::btn(&mut child, theme::Btn::Small, None, t!("Bearbeiten"))
                .on_hover_text(t!("Host bearbeiten"))
                .clicked()
        {
            actions.push(Action::EditHost(h.id));
        }
        if !collapsed
            && can_wake
            && theme::btn(
                &mut child,
                theme::Btn::Small,
                Some(Icon::Power),
                t!("Aufwecken"),
            )
            .on_hover_text(t!(
                "Wake-on-LAN: Magic Packet senden, auf den Host warten und verbinden"
            ))
            .clicked()
        {
            actions.push(Action::Net(crate::ui::net::NetAction::Wake(h.id, true)));
        }
    }

    if resp.double_clicked() {
        actions.push(Action::Connect(h.id));
    }
    resp.context_menu(|ui| host_menu(ui, app, h, maint, actions));
}

/// Right-click menu of a host (list row and table row).
pub(crate) fn host_menu(
    ui: &mut egui::Ui,
    app: &App,
    h: &Host,
    maint: Option<&crate::ui::maintenance::Maintenance>,
    actions: &mut Vec<Action>,
) {
    if ui.button(t!("Verbinden")).clicked() {
        actions.push(Action::Connect(h.id));
        ui.close();
    }
    if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp)
        && ui.button(t!("Dateien (SFTP)")).clicked()
    {
        actions.push(Action::OpenSftp(h.id));
        ui.close();
    }
    if (h.protocol == Protocol::Ssh || h.protocol.is_container())
        && ui
            .button(t!("Log ansehen…"))
            .on_hover_text(t!("Logdatei oder journalctl live verfolgen, mit Filter"))
            .clicked()
    {
        actions.push(Action::PickLog(Some(h.id)));
        ui.close();
    }
    if h.protocol == Protocol::Ssh && ui.button(t!("Verlauf…")).clicked() {
        actions.push(Action::SessionLog(crate::ui::sessionlog_ui::Act::History(
            h.id,
        )));
        ui.close();
    }
    if h.protocol.is_network()
        && ui
            .button(t!("Ports scannen…"))
            .on_hover_text(t!("Offene Ports des Hosts finden, mit Tunnel per Klick"))
            .clicked()
    {
        actions.push(Action::PortScan(h.id));
        ui.close();
    }
    if crate::ui::wol::can_wake(h) {
        if app.net.wol.waiting(h.id).is_some() {
            if ui.button(t!("Aufwecken abbrechen")).clicked() {
                actions.push(Action::Net(crate::ui::net::NetAction::CancelWake(h.id)));
                ui.close();
            }
        } else {
            if ui
                .button(t!("Aufwecken"))
                .on_hover_text(t!("Wake-on-LAN-Paket senden"))
                .clicked()
            {
                actions.push(Action::Net(crate::ui::net::NetAction::Wake(h.id, false)));
                ui.close();
            }
            if ui.button(t!("Aufwecken und verbinden")).clicked() {
                actions.push(Action::Net(crate::ui::net::NetAction::Wake(h.id, true)));
                ui.close();
            }
        }
    }
    if h.protocol == Protocol::Ssh
            && ui
                .button(t!("Weg finden…"))
                .on_hover_text(t!(
                    "Fragt deine anderen SSH-Hosts, ob sie diesen Host erreichen – für Hosts hinter einem Bastion/Jump-Host"
                ))
                .clicked()
        {
            actions.push(Action::Net(crate::ui::net::NetAction::FindRoute(h.id)));
            ui.close();
        }
    if h.protocol == Protocol::Ssh
        && ui
            .button(t!("Diagnose-Paket erstellen…"))
            .on_hover_text(t!(
                "System-, Log- und Dienstinformationen sammeln und als Archiv speichern"
            ))
            .clicked()
    {
        actions.push(Action::Ops(OpsAction::Diag(h.id)));
        ui.close();
    }
    if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp)
        && ui
            .button(t!("Befehl ausführen…"))
            .on_hover_text(t!(
                "Befehl oder Snippet auf diesem Host (und weiteren) ausführen"
            ))
            .clicked()
    {
        use crate::ui::multirun_ui::SrvAction;
        let ids = if app.selected_hosts.contains(&h.id) && app.selected_hosts.len() > 1 {
            app.selected_hosts.iter().copied().collect()
        } else {
            vec![h.id]
        };
        let title = t!("Auswahl ({} Hosts)", ids.len());
        actions.push(Action::Srv(SrvAction::Run(ids, title)));
        ui.close();
    }
    if h.protocol == Protocol::Ssh
        && ui
            .button(t!("Live-Statistik…"))
            .on_hover_text(t!("CPU, RAM, Last, Netzwerk und Datenträger live"))
            .clicked()
    {
        actions.push(Action::Srv(crate::ui::multirun_ui::SrvAction::Stats(h.id)));
        ui.close();
    }
    if crate::ui::winrm_ui::usable(h) && ui.button(t!("Fakten per WinRM aktualisieren")).clicked() {
        actions.push(Action::Srv(
            crate::ui::multirun_ui::SrvAction::WinrmRefresh(vec![h.id]),
        ));
        ui.close();
    }
    maintenance_menu(ui, maint, Target::Host(h.id), actions);
    if h.protocol == Protocol::Ssh {
        ui.menu_button(t!("Container auf diesem Host"), |ui| {
            for p in [Protocol::Docker, Protocol::Kubernetes] {
                if ui.button(format!("{} {}", p.icon(), p.label())).clicked() {
                    actions.push(Action::BrowseContainers(p, Some(h.id), false));
                    ui.close();
                }
            }
        });
    }
    ui.separator();
    if ui
        .button(if h.favorite {
            t!("Aus Favoriten entfernen")
        } else {
            t!("Zu Favoriten")
        })
        .clicked()
    {
        actions.push(Action::ToggleFavorite(h.id));
        ui.close();
    }
    if ui.button(t!("Bearbeiten")).clicked() {
        actions.push(Action::EditHost(h.id));
        ui.close();
    }
    if ui.button(t!("Umbenennen  (F2)")).clicked() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(rename_id(), (h.id, h.label.clone())));
        ui.close();
    }
    if h.protocol == Protocol::Ssh {
        if ui
            .button(t!("SSH-Key einrichten…"))
            .on_hover_text(t!(
                "Schlüssel auf dem Server hinterlegen – danach ohne Passwort anmelden"
            ))
            .clicked()
        {
            actions.push(Action::SetupKey(h.id));
            ui.close();
        }
        if let Some(n) = app
            .lib
            .facts
            .get(&h.id)
            .and_then(|f| f.updates)
            .filter(|n| *n > 0)
            && ui.button(t!("{} Updates installieren", n)).clicked()
        {
            actions.push(Action::RunUpdates(h.id));
            ui.close();
        }
    }
    if ui.button(t!("Duplizieren")).clicked() {
        actions.push(Action::DuplicateHost(h.id));
        ui.close();
    }
    ui.menu_button(t!("Verschieben nach"), |ui| {
        if ui.button(t!("Ohne Gruppe")).clicked() {
            actions.push(Action::MoveHost(h.id, None));
            ui.close();
        }
        for g in &app.lib.groups {
            if ui.button(&g.name).clicked() {
                actions.push(Action::MoveHost(h.id, Some(g.id)));
                ui.close();
            }
        }
    });
    ui.separator();
    if ui
        .button(RichText::new(t!("Löschen")).color(theme::red()))
        .clicked()
    {
        actions.push(Action::Confirm(
            t!("Host „{}“ löschen?", h.display_name()),
            Box::new(Action::DeleteHost(h.id)),
        ));
        ui.close();
    }
}
