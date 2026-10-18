//! Connections overview: one search field (doubles as quick connect), a protocol filter and
//! the host table grouped by folder. Rows instead of cards keep many hosts scannable.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, ConnState, TabKind, parse_quick};
use crate::backend::reach::Reach;
use crate::model::{Host, Protocol};
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::{fuzzy, relative_time};

const ROW_H: f32 = 66.0;
const ROW_H_COMPACT: f32 = 44.0;

/// Visible host order of the last frame, for Shift+click ranges.
pub fn order_id() -> egui::Id {
    egui::Id::new("aurora-host-order")
}

fn sort_hosts(app: &App, hosts: &mut [Host]) {
    match app.lib.settings.sort {
        1 => hosts.sort_by_key(|h| std::cmp::Reverse(h.last_connected)),
        2 => hosts.sort_by_key(|h| {
            (
                !matches!(app.reach.get(h.id), Some(Reach::Up(_))),
                h.display_name().to_lowercase(),
            )
        }),
        _ => hosts.sort_by_key(|h| h.display_name().to_lowercase()),
    }
}

pub fn protocol_color(p: Protocol) -> Color32 {
    match p {
        Protocol::Ssh => theme::accent(),
        Protocol::Rdp | Protocol::Vnc => theme::accent2(),
        Protocol::Serial | Protocol::Telnet => theme::green(),
        Protocol::Docker | Protocol::Kubernetes => theme::amber(),
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
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.lib.settings.reachability {
        app.reach.tick(&app.rt, &app.lib.hosts, ui.ctx());
    }
    // Header: title, search/quick connect, primary action.
    let mut enter = false;
    ui.horizontal(|ui| {
        theme::page_title(ui, t!("Verbindungen"), Some(app.lib.hosts.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
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
            if crate::ui::certradar::header_button(app, ui).clicked() {
                actions.push(Action::Net(crate::ui::net::NetAction::OpenRadar));
            }
            if theme::btn(ui, theme::Btn::Normal, Some(Icon::Wifi), t!("Netz scannen"))
                .on_hover_text(t!(
                    "Geräte mit SSH/RDP im lokalen Netz finden und übernehmen"
                ))
                .clicked()
            {
                actions.push(Action::OpenScan);
            }
            let width = (ui.available_width() - 24.0).clamp(180.0, 300.0);
            let search = theme::search_field(
                ui,
                &mut app.search,
                t!("Suchen oder user@host verbinden"),
                width,
                egui::Id::new("host-search"),
            );
            enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        });
    });
    ui.add_space(18.0);

    if app.lib.hosts.is_empty() && app.search.trim().is_empty() {
        empty_state(ui, actions);
        return;
    }

    ui.horizontal(|ui| {
        if let Some(i) = theme::segmented(
            ui,
            &[
                "Alle",
                "SSH",
                "Desktop",
                "Konsole",
                "Container",
                "Favoriten",
            ]
            .map(crate::i18n::tr),
            app.host_filter,
        ) {
            app.host_filter = i;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let mut changed = false;
            let compact = app.lib.settings.compact;
            let kind = if compact {
                theme::Btn::SmallPrimary
            } else {
                theme::Btn::Small
            };
            if theme::btn(ui, kind, None, t!("Kompakt"))
                .on_hover_text(t!("Kleinere Zeilen für viele Hosts"))
                .clicked()
            {
                app.lib.settings.compact = !compact;
                changed = true;
            }
            let labels = ["Name", "Zuletzt verwendet", "Online zuerst"];
            egui::ComboBox::from_id_salt("host-sort")
                .selected_text(
                    RichText::new(crate::i18n::tr(
                        labels[app.lib.settings.sort.min(2) as usize],
                    ))
                    .font(theme::medium(13.0)),
                )
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (i, l) in labels.iter().enumerate() {
                        changed |= ui
                            .selectable_value(
                                &mut app.lib.settings.sort,
                                i as u8,
                                crate::i18n::tr(l),
                            )
                            .changed();
                    }
                });
            ui.label(
                RichText::new(t!("Sortierung"))
                    .size(13.0)
                    .color(theme::text_dim()),
            );
            if changed {
                app.save_library();
            }
        });
    });
    ui.add_space(14.0);
    selection_bar(app, ui, actions);
    ui.ctx()
        .data_mut(|d| d.insert_temp(order_id(), Vec::<Uuid>::new()));

    let query = app.search.trim().to_string();
    let live = live_hosts(app);

    egui::ScrollArea::vertical()
        .id_salt("hosts-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the scroll bar next to the cards.
            ui.set_max_width(ui.available_width() - 14.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            if !query.is_empty() {
                let quick = parse_quick(&query).filter(|_| query.contains('@'));
                if let Some(h) = &quick {
                    quick_row(
                        ui,
                        &format!("{}@{}:{}", h.username, h.hostname, h.port),
                        actions,
                        &query,
                    );
                }
                let hosts = filter_hosts(app, &query);
                if enter {
                    match (&quick, hosts.first()) {
                        (Some(_), _) => actions.push(Action::QuickConnect(query.clone())),
                        (None, Some(h)) => actions.push(Action::Connect(h.id)),
                        _ => {}
                    }
                }
                if hosts.is_empty() && quick.is_none() {
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
                        if theme::primary_button(ui, &t!("„{}“ als Host anlegen", query)).clicked()
                        {
                            actions.push(Action::NewHostNamed(query.clone()));
                        }
                        if !query.contains(' ')
                            && theme::btn(ui, theme::Btn::Normal, None, t!("Direkt verbinden"))
                                .clicked()
                        {
                            actions.push(Action::QuickConnect(query.clone()));
                        }
                    });
                }
                if !hosts.is_empty() {
                    host_card(app, ui, &hosts, &live, actions);
                }
                return;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !app.selected_hosts.is_empty() {
                actions.push(Action::ClearSelection);
            }

            group_tree(app, ui, None, 0, &live, actions);
            let mut loose: Vec<Host> = app
                .lib
                .hosts
                .iter()
                .filter(|h| h.group_id.is_none_or(|g| app.lib.group(g).is_none()))
                .filter(|h| passes_filter(app.host_filter, h))
                .cloned()
                .collect();
            sort_hosts(app, &mut loose);
            subnet_hint(app, ui, &loose, actions);
            if !loose.is_empty() || (!app.lib.groups.is_empty() && dragging_host(ui)) {
                if !app.lib.groups.is_empty() {
                    ui.add_space(22.0);
                    // Drop zone: hosts dragged here leave their group.
                    let (r, resp) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
                    let hot = resp.dnd_hover_payload::<Uuid>().is_some();
                    if hot {
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
                        actions.push(Action::MoveHosts(drag_set(app, *id), None));
                    }
                    ui.add_space(6.0);
                } else {
                    ui.add_space(4.0);
                }
                if !loose.is_empty() {
                    host_card(app, ui, &loose, &live, actions);
                }
            }
            ui.add_space(40.0);
        });
}

/// One rounded card with a row per host.
fn host_card(
    app: &App,
    ui: &mut egui::Ui,
    hosts: &[Host],
    live: &[String],
    actions: &mut Vec<Action>,
) {
    theme::card(ui, theme::bg_deep(), |ui| {
        for (i, h) in hosts.iter().enumerate() {
            host_row(app, ui, h, live, i > 0, actions);
        }
    });
}

/// Ids of hosts that currently have a connected session.
fn live_hosts(app: &App) -> Vec<String> {
    app.tabs
        .iter()
        .filter(|t| match &t.kind {
            TabKind::Term(t) => matches!(t.state, ConnState::Connected),
            TabKind::Sftp(t) => matches!(t.state(), ConnState::Connected),
            TabKind::Rdp(t) => t.is_live(),
            TabKind::Vnc(t) => matches!(t.view.state, ConnState::Connected),
            TabKind::Log(_) => false,
        })
        .flat_map(|t| {
            [
                t.title.clone(),
                t.host_id.map(|h| h.to_string()).unwrap_or_default(),
            ]
        })
        .collect()
}

fn passes_filter(filter: usize, h: &Host) -> bool {
    match filter {
        1 => h.protocol == Protocol::Ssh,
        2 => matches!(h.protocol, Protocol::Rdp | Protocol::Vnc),
        3 => matches!(h.protocol, Protocol::Serial | Protocol::Telnet),
        4 => h.protocol.is_container(),
        5 => h.favorite,
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
            });
    });
}

/// "5 Hosts im Netz 192.168.0.x – als Gruppe anlegen?" for loose hosts sharing a /24.
/// Hosts that a drag of `id` carries: the whole selection if `id` is part of it.
fn drag_set(app: &App, id: Uuid) -> Vec<Uuid> {
    if app.selected_hosts.contains(&id) {
        app.selected_hosts.iter().copied().collect()
    } else {
        vec![id]
    }
}

fn dragging_host(ui: &egui::Ui) -> bool {
    egui::DragAndDrop::has_payload_of_type::<Uuid>(ui.ctx())
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

fn quick_row(ui: &mut egui::Ui, target: &str, actions: &mut Vec<Action>, query: &str) {
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

fn filter_hosts(app: &App, query: &str) -> Vec<Host> {
    if let Some(name) = query.strip_prefix("group:") {
        let name = name.trim().to_lowercase();
        let ids: Vec<Uuid> = app
            .lib
            .groups
            .iter()
            .filter(|g| g.name.to_lowercase() == name)
            .map(|g| g.id)
            .collect();
        return app
            .lib
            .hosts
            .iter()
            .filter(|h| {
                h.group_id
                    .is_some_and(|g| ids.iter().any(|id| app.lib.is_descendant(g, *id)))
            })
            .cloned()
            .collect();
    }
    let mut scored: Vec<(i32, Host)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| passes_filter(app.host_filter, h))
        .filter_map(|h| {
            let group = h
                .group_id
                .and_then(|g| app.lib.group(g))
                .map(|g| g.name.as_str())
                .unwrap_or("");
            let hay = format!(
                "{} {} {} {} {}",
                h.display_name(),
                h.hostname,
                h.username,
                h.tags.join(" "),
                group
            );
            fuzzy(query, &hay).map(|s| (s, h.clone()))
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    scored.into_iter().map(|(_, h)| h).collect()
}

fn group_tree(
    app: &mut App,
    ui: &mut egui::Ui,
    parent: Option<Uuid>,
    depth: usize,
    live: &[String],
    actions: &mut Vec<Action>,
) {
    let groups: Vec<_> = app
        .lib
        .groups
        .iter()
        .filter(|g| g.parent_id == parent)
        .cloned()
        .collect();
    for (gi, g) in groups.into_iter().enumerate() {
        let mut hosts: Vec<Host> = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id == Some(g.id) && passes_filter(app.host_filter, h))
            .cloned()
            .collect();
        sort_hosts(app, &mut hosts);
        let total = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.group_id.is_some_and(|hg| app.lib.is_descendant(hg, g.id)))
            .count();
        // With a protocol filter, groups without a matching host disappear.
        if app.host_filter != 0
            && !app.lib.hosts.iter().any(|h| {
                h.group_id.is_some_and(|hg| app.lib.is_descendant(hg, g.id))
                    && passes_filter(app.host_filter, h)
            })
        {
            continue;
        }
        ui.add_space(if depth == 0 && gi == 0 && parent.is_none() {
            4.0
        } else if depth == 0 {
            22.0
        } else {
            12.0
        });
        let indent = depth as f32 * 20.0;
        let (full, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
        // Hosts dropped on a group header move into that group.
        let drop_hot = resp.dnd_hover_payload::<Uuid>().is_some();
        if let Some(id) = resp.dnd_release_payload::<Uuid>() {
            actions.push(Action::MoveHosts(drag_set(app, *id), Some(g.id)));
        }
        let rect = Rect::from_min_max(full.min + Vec2::new(indent, 0.0), full.max);
        let p = ui.painter();
        let cy = rect.center().y;
        if drop_hot {
            p.rect_filled(rect, CornerRadius::same(8), theme::accent_soft());
        }
        // Chevron, colour square, name, online count.
        let chev = Rect::from_center_size(Pos2::new(rect.left() + 10.0, cy), Vec2::splat(12.0));
        if g.collapsed {
            // Pointing right.
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
        let galley = p.layout_no_wrap(g.name.clone(), theme::semibold(14.5), theme::text());
        let w = galley.size().x;
        p.galley(
            Pos2::new(rect.left() + 40.0, cy - galley.size().y / 2.0),
            galley,
            theme::text(),
        );
        let mut tx = rect.left() + 40.0 + w + 10.0;
        // "2 von 3 online" from the reachability probe.
        let members = app.lib.hosts_in_group(g.id);
        let up = members
            .iter()
            .filter(|h| matches!(app.reach.get(h.id), Some(Reach::Up(_))))
            .count();
        let count = if app.lib.settings.reachability && total > 0 {
            t!("{} von {} online", up, total)
        } else {
            total.to_string()
        };
        let cg = p.layout_no_wrap(count, FontId::proportional(13.0), theme::text_dim());
        let cw = cg.size().x;
        p.galley(Pos2::new(tx, cy - cg.size().y / 2.0), cg, theme::text_dim());
        tx += cw + 10.0;
        if g.env != crate::model::Env::None {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_size(
                        Pos2::new(tx, cy - 9.0),
                        Vec2::new(90.0, 18.0),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            theme::pill(&mut child, g.env.label(), theme::env_color(g.env));
        }
        // "Alle öffnen" and "+ Host" as quiet links on the right.
        let mut links = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - 260.0, rect.top()),
                    rect.right_bottom() - Vec2::new(4.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        links.spacing_mut().item_spacing.x = 4.0;
        let plus = theme::btn(&mut links, theme::Btn::Link, None, "+ Host");
        let all =
            (total > 0).then(|| theme::btn(&mut links, theme::Btn::Link, None, t!("Alle öffnen")));
        if plus.clicked() {
            actions.push(Action::NewHost(Some(g.id)));
        } else if all.as_ref().is_some_and(|a| a.clicked()) {
            actions.push(Action::OpenGroup(g.id));
        } else if resp.clicked() {
            actions.push(Action::ToggleGroup(g.id));
        }
        if let Some(a) = all {
            a.on_hover_text(t!(
                "Verbindet die ersten vier Hosts der Gruppe im Splitscreen"
            ));
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .context_menu(|ui| {
                if total > 0
                    && ui
                        .button(t!("Alle öffnen (nebeneinander)"))
                        .on_hover_text(t!(
                            "Verbindet die ersten vier Hosts der Gruppe im Splitscreen"
                        ))
                        .clicked()
                {
                    actions.push(Action::OpenGroup(g.id));
                    ui.close();
                }
                if ui.button(t!("Host hinzufügen")).clicked() {
                    actions.push(Action::NewHost(Some(g.id)));
                    ui.close();
                }
                if ui.button(t!("Untergruppe")).clicked() {
                    actions.push(Action::NewGroup(Some(g.id)));
                    ui.close();
                }
                if ui.button(t!("Bearbeiten")).clicked() {
                    actions.push(Action::EditGroup(g.id));
                    ui.close();
                }
                if ui
                    .button(RichText::new(t!("Löschen")).color(theme::red()))
                    .clicked()
                {
                    actions.push(Action::Confirm(
                        t!("Gruppe „{}“ löschen? Die Hosts bleiben erhalten.", g.name),
                        Box::new(Action::DeleteGroup(g.id)),
                    ));
                    ui.close();
                }
            });
        if !g.collapsed {
            if !hosts.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    ui.vertical(|ui| host_card(app, ui, &hosts, live, actions));
                });
            }
            group_tree(app, ui, Some(g.id), depth + 1, live, actions);
        }
    }
}

fn uptime_text(secs: u64) -> String {
    let d = secs / 86_400;
    match (d, secs / 3600) {
        (d, _) if d > 0 => format!("up {d} d"),
        (_, h) if h > 0 => format!("up {h} h"),
        _ => format!("up {} min", secs / 60),
    }
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

fn host_row(
    app: &App,
    ui: &mut egui::Ui,
    h: &Host,
    live: &[String],
    divider: bool,
    actions: &mut Vec<Action>,
) {
    let compact = app.lib.settings.compact;
    let row_h = if compact { ROW_H_COMPACT } else { ROW_H };
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), row_h),
        Sense::click_and_drag(),
    );
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Uuid>>(order_id())
            .push(h.id)
    });
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
    if app.failed.contains(&h.id) && !is_live {
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
        let tip = if app.failed.contains(&h.id) && !is_live {
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

    // --- name (or inline rename)
    let renaming: Option<(Uuid, String)> = ui.ctx().data(|d| d.get_temp(rename_id()));
    let mut nx;
    if let Some((_, mut text)) = renaming.filter(|(rid, _)| *rid == h.id) {
        let r = Rect::from_min_size(Pos2::new(col_name - 6.0, y1 - 14.0), Vec2::new(260.0, 28.0));
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
        let name_galley = p.layout_no_wrap(
            h.display_name().to_string(),
            theme::semibold(15.0),
            theme::text(),
        );
        let name_w = name_galley.size().x;
        p.galley(
            Pos2::new(col_name, y1 - name_galley.size().y / 2.0),
            name_galley,
            theme::text(),
        );
        nx = col_name + name_w + 8.0;
    }
    if h.favorite {
        icons::paint(
            &p,
            Rect::from_center_size(Pos2::new(nx + 6.0, y1), Vec2::splat(13.0)),
            Icon::Star,
            Color32::from_rgb(255, 214, 10),
        );
        nx += 20.0;
    }
    let env = app.lib.env_of(h);
    if env != crate::model::Env::None {
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
    if !h.notes.trim().is_empty() {
        let r = Rect::from_center_size(Pos2::new(nx + 7.0, y1), Vec2::splat(14.0));
        icons::paint(&p, r, Icon::File, theme::text_dim());
        ui.interact(r, ui.id().with(("notes", h.id)), Sense::hover())
            .on_hover_text(h.notes.trim());
        nx += 20.0;
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
        (nx.max(col_name + 180.0) + 8.0, y1)
    } else {
        (col_name, y2)
    };
    let mut x = ax;
    if !addr.is_empty() {
        let g = p.layout_no_wrap(addr, FontId::monospace(12.0), theme::text_dim());
        let gw = g.size().x;
        p.galley(Pos2::new(x, ay - g.size().y / 2.0), g, theme::text_dim());
        x += gw;
    }
    let info_start = x;
    for (i, (text, color)) in extra.iter().enumerate() {
        let sep = if i == 0 && x == ax { "" } else { "  ·  " };
        let g = p.layout_no_wrap(format!("{sep}{text}"), FontId::proportional(12.5), *color);
        let gw = g.size().x;
        p.galley(Pos2::new(x, ay - g.size().y / 2.0), g, *color);
        x += gw;
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

    let show_actions = hovered || resp.context_menu_opened();
    let slot = 150.0;
    if !show_actions {
        // Meta: latency and last use.
        let mut parts = Vec::new();
        if let Some(Reach::Up(ms)) = reach {
            parts.push(format!("{ms} ms"));
        }
        if let Some(t) = h.last_connected {
            parts.push(relative_time(t));
        }
        if let Some(text) = app.net.wol.pill_text(h.id) {
            crate::ui::wol::paint_pill(&p, Pos2::new(rect.right() - 16.0, cy), &text);
        } else {
            p.text(
                Pos2::new(rect.right() - 16.0, cy),
                egui::Align2::RIGHT_CENTER,
                parts.join(" · "),
                FontId::proportional(12.5),
                theme::text_dim(),
            );
        }
        let mut tx = rect.right() - 16.0 - slot - 12.0;
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
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - 400.0, rect.top()),
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
        if matches!(h.protocol, Protocol::Ssh | Protocol::Rdp)
            && theme::btn(&mut child, theme::Btn::Small, None, t!("Dateien"))
                .on_hover_text(t!("Dateien (SFTP) öffnen"))
                .clicked()
        {
            actions.push(Action::OpenSftp(h.id));
        }
        if theme::btn(&mut child, theme::Btn::Small, None, t!("Bearbeiten"))
            .on_hover_text(t!("Host bearbeiten"))
            .clicked()
        {
            actions.push(Action::EditHost(h.id));
        }
        let offline =
            !is_live && (app.failed.contains(&h.id) || matches!(reach, Some(Reach::Down)));
        if offline
            && crate::ui::wol::can_wake(h)
            && app.net.wol.waiting(h.id).is_none()
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
    resp.context_menu(|ui| {
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
    });
}
