//! "Weg finden": asks saved SSH hosts whether they reach a host that this PC can't reach,
//! shows the best route and connects through it or saves it as the host's jump chain.
//! Also the jump-chain row of the host editor.

use egui::{RichText, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{Action, App, Purpose};
use crate::backend::Sink;
use crate::backend::reach::Reach;
use crate::backend::route::{self, Candidate, Probe, RouteEvent};
use crate::model::{Host, Library, Protocol};
use crate::theme;
use crate::ui::icons::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Opened by the user: sheet right away.
    Sheet,
    /// After a failed connection: the sheet only appears once a route works.
    Auto,
    /// From the host editor: results inline in the editor.
    Editor,
}

pub struct RouteFinder {
    pub target: Host,
    pub mode: Mode,
    /// Candidates in probe order: (id, "hop → hop → candidate").
    pub names: Vec<(Uuid, String)>,
    pub probes: Vec<Probe>,
    pub running: bool,
    /// Hosts without stored credentials (not asked).
    pub skipped: Vec<String>,
    pub error: Option<String>,
    pub selected: Option<Uuid>,
    pub visible: bool,
    rx: Option<UnboundedReceiver<RouteEvent>>,
}

impl RouteFinder {
    pub fn best(&self) -> Option<&Probe> {
        self.probes.iter().find(|p| p.ok)
    }

    fn name(&self, id: Uuid) -> &str {
        self.names
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
            .unwrap_or("?")
    }

    /// "bastion-01 → db-03 (42 ms)"
    pub fn route_text(&self, p: &Probe) -> String {
        let mut s = format!("{} → {}", self.name(p.id), self.target.display_name());
        if let Some(ms) = p.ms {
            s.push_str(&format!(" ({ms} ms)"));
        }
        s
    }

    fn chosen(&self) -> Option<&Probe> {
        self.selected
            .and_then(|id| self.probes.iter().find(|p| p.id == id && p.ok))
            .or_else(|| self.best())
    }
}

/// Route label for a candidate: its own jump chain, then itself.
fn hop_label(lib: &Library, c: &Host) -> String {
    let mut chain = route::jump_chain_lossy(lib, c);
    chain.push(c.id);
    route::chain_names(lib, &chain)
}

/// Starts the finder for `target` (saved or still in the editor).
pub fn start(app: &mut App, target: Host, mode: Mode, ctx: &egui::Context) {
    let reach_known = app.lib.settings.reachability;
    let reachable = |id: Uuid| -> Option<bool> {
        if !reach_known {
            return None;
        }
        match app.reach.get(id)? {
            Reach::Up(_) => Some(true),
            Reach::Down => Some(false),
        }
    };
    let ids = route::candidates_for(&app.lib, &target, reachable);
    let mut names = Vec::new();
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let Some(h) = app.lib.host(id).cloned() else {
            continue;
        };
        match crate::ui::net::stored_params(app, &h) {
            Ok(params) => {
                names.push((id, hop_label(&app.lib, &h)));
                candidates.push(Candidate { id, params });
            }
            Err(_) => skipped.push(h.display_name().to_string()),
        }
    }
    let address = target.hostname.trim().to_string();
    let mut finder = RouteFinder {
        mode,
        names,
        probes: Vec::new(),
        running: false,
        skipped,
        error: None,
        selected: None,
        visible: mode == Mode::Sheet,
        rx: None,
        target,
    };
    if address.is_empty() {
        finder.error = Some(t!("Erst eine Adresse eintragen").into());
    } else if !route::safe_host(&address) {
        finder.error = Some(t!("Ungültiger Hostname").into());
    } else if candidates.is_empty() {
        if mode == Mode::Auto {
            return;
        }
        finder.error = Some(
            t!("Kein anderer SSH-Host mit gespeicherten Zugangsdaten, über den Aurora es versuchen könnte")
                .into(),
        );
    } else {
        if mode == Mode::Auto {
            app.toast(
                t!(
                    "{} ist direkt nicht erreichbar – Aurora sucht einen Weg über deine anderen Hosts…",
                    finder.target.display_name()
                ),
                theme::accent(),
            );
        }
        let (sink, rx) = Sink::new(ctx.clone());
        finder.rx = Some(rx);
        finder.running = true;
        route::spawn_finder(
            &app.rt,
            address,
            finder.target.port,
            candidates,
            sink,
            app.prompts_sink.clone(),
        );
    }
    app.net.route = Some(finder);
}

fn pump(app: &mut App) {
    let Some(f) = app.net.route.as_mut() else {
        return;
    };
    let mut done = false;
    while let Some(ev) = f.rx.as_mut().and_then(|rx| rx.try_recv().ok()) {
        match ev {
            RouteEvent::Probe(p) => {
                f.probes.push(p);
                route::rank(&mut f.probes);
            }
            RouteEvent::Done => done = true,
        }
    }
    if f.mode == Mode::Auto && f.best().is_some() {
        f.visible = true;
    }
    if done {
        f.running = false;
        f.rx = None;
        if f.mode == Mode::Auto && f.best().is_none() {
            let name = f.target.display_name().to_string();
            app.net.route = None;
            app.toast(t!("Kein anderer Weg zu {} gefunden", name), theme::amber());
        }
    }
}

/// Session subtitle with the jump chain: "root@db:22 · über bastion-01".
pub fn with_route(lib: &Library, host: &Host, subtitle: String) -> String {
    if host.protocol != Protocol::Ssh {
        return subtitle;
    }
    let chain = route::jump_chain_lossy(lib, host);
    if chain.is_empty() {
        return subtitle;
    }
    format!(
        "{subtitle} · {}",
        t!("über {}", route::chain_names(lib, &chain))
    )
}

fn probe_row(ui: &mut egui::Ui, f: &RouteFinder, p: Option<&Probe>, id: Uuid) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), egui::Sense::click());
    let ok = p.is_some_and(|p| p.ok);
    let chosen = ok && f.chosen().is_some_and(|c| c.id == id);
    if chosen {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), theme::accent_soft());
    } else if ok && resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), theme::hover());
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(10.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    match p {
        None => {
            ui.add(egui::Spinner::new().size(12.0));
        }
        Some(p) => {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
            theme::led(
                ui.painter(),
                r.center(),
                if p.ok { theme::green() } else { theme::red() },
                true,
            );
        }
    }
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(f.name(id)).size(13.5).color(theme::text())).truncate());
    ui.with_layout(
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| match p {
            Some(Probe { ok: true, ms, .. }) => {
                if let Some(ms) = ms {
                    ui.label(theme::mono(format!("{ms} ms"), 12.0).color(theme::text_dim()));
                }
            }
            Some(p) => {
                ui.add(
                    egui::Label::new(
                        RichText::new(p.note.as_str())
                            .size(12.0)
                            .color(theme::text_dim()),
                    )
                    .truncate(),
                );
            }
            None => {
                ui.label(
                    RichText::new(t!("prüft…"))
                        .size(12.0)
                        .color(theme::text_dim()),
                );
            }
        },
    );
    ok && resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

pub fn show(app: &mut App, ctx: &egui::Context, _actions: &mut Vec<Action>) {
    pump(app);
    if app
        .net
        .route
        .as_ref()
        .is_some_and(|f| f.mode == Mode::Editor)
        && app.host_editor.is_none()
    {
        app.net.route = None;
    }
    let Some(f) = app.net.route.as_mut() else {
        return;
    };
    if f.mode == Mode::Editor || !f.visible {
        return;
    }
    let title = t!("Weg finden · {}", f.target.display_name());
    let mut pick = None;
    let ((connect, save), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("route-finder"),
        520.0,
        &title,
        |ui| {
            ui.label(
                theme::mono(
                    format!("{}:{}", f.target.hostname.trim(), f.target.port),
                    13.0,
                )
                .color(theme::text_dim()),
            );
            if f.mode == Mode::Auto {
                ui.label(
                    RichText::new(t!(
                        "Von diesem PC aus nicht erreichbar – aber über einen deiner Hosts:"
                    ))
                    .size(13.0)
                    .color(theme::text_dim()),
                );
            }
            ui.add_space(4.0);
            if let Some(e) = &f.error {
                ui.label(RichText::new(e).color(theme::red()));
            }
            if let Some(best) = f.chosen() {
                egui::Frame::NONE
                    .fill(theme::soft(theme::green()))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                            crate::ui::icons::paint(ui.painter(), r, Icon::Route, theme::green());
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(t!("Erreichbar über:"))
                                        .size(12.5)
                                        .color(theme::text_dim()),
                                );
                                ui.label(
                                    RichText::new(f.route_text(best))
                                        .font(theme::semibold(15.0))
                                        .color(theme::text()),
                                );
                            });
                        });
                    });
            } else if !f.running && f.error.is_none() {
                ui.label(
                    RichText::new(t!("Keiner der gefragten Hosts erreicht das Ziel."))
                        .color(theme::amber()),
                );
            }
            if !f.names.is_empty() {
                theme::form_caption(ui, t!("Gefragte Hosts"));
                theme::form(ui, 92.0, |ui, rows| {
                    rows.full(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        // Results in rank order, still running ones after them.
                        let mut ids: Vec<Uuid> = f.probes.iter().map(|p| p.id).collect();
                        ids.extend(
                            f.names
                                .iter()
                                .map(|(id, _)| *id)
                                .filter(|id| !f.probes.iter().any(|p| p.id == *id)),
                        );
                        for id in ids {
                            let p = f.probes.iter().find(|p| p.id == id);
                            if probe_row(ui, f, p, id) {
                                pick = Some(id);
                            }
                        }
                    });
                });
            }
            if !f.skipped.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(t!(
                        "Ohne gespeicherte Zugangsdaten übersprungen: {}",
                        f.skipped.join(", ")
                    ))
                    .size(12.0)
                    .color(theme::text_dim()),
                );
            }
        },
        |ui, ()| {
            let can = f.chosen().is_some();
            let connect = ui
                .add_enabled_ui(can, |ui| {
                    theme::btn(ui, theme::Btn::Primary, None, t!("Verbinden"))
                })
                .inner
                .clicked();
            let save = ui
                .add_enabled_ui(can, |ui| {
                    theme::btn(ui, theme::Btn::Normal, None, t!("Als Weg speichern"))
                })
                .inner
                .on_hover_text(t!(
                    "Trägt den Host als „Verbinden über“ ein – künftig geht jede Verbindung diesen Weg"
                ))
                .clicked();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if f.running {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(
                        RichText::new(t!("{} von {} geprüft", f.probes.len(), f.names.len()))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                }
            });
            (connect, save)
        },
    );
    if let Some(id) = pick {
        f.selected = Some(id);
    }
    let chosen = f.chosen().map(|p| p.id);
    let target = f.target.clone();
    if dismissed {
        app.net.route = None;
        return;
    }
    let Some(via) = chosen else {
        return;
    };
    if save {
        if let Some(h) = app.lib.hosts.iter_mut().find(|h| h.id == target.id) {
            h.jump_host = Some(via);
        }
        app.save_library();
        let name = app
            .lib
            .host(via)
            .map(|h| h.display_name().to_string())
            .unwrap_or_default();
        app.toast(
            t!("{} geht jetzt über {}", target.display_name(), name),
            theme::green(),
        );
        app.net.route = None;
    }
    if connect {
        app.net.route = None;
        let host = Host {
            jump_host: Some(via),
            ..app.lib.host(target.id).cloned().unwrap_or(target)
        };
        app.failed.remove(&host.id);
        app.connect_host(host, Purpose::Terminal, ctx);
    }
}

/// What the editor's jump row wants after drawing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorRequest {
    Find,
}

/// "Verbinden über" row of the host editor: jump host, "Route" button, the resulting
/// chain (or what's wrong with it) and the finder's result.
pub fn editor_rows(
    ui: &mut egui::Ui,
    rows: &mut theme::Rows,
    h: &mut Host,
    lib: &Library,
    finder: Option<&RouteFinder>,
    ssh_hosts: &[(Uuid, String)],
) -> Option<EditorRequest> {
    let mut req = None;
    rows.field(ui, t!("Verbinden über"), |ui| {
        let me = h.id;
        let sel = h
            .jump_host
            .and_then(|id| ssh_hosts.iter().find(|(i, _)| *i == id))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("— direkt —").to_string());
        egui::ComboBox::from_id_salt("jump")
            .selected_text(sel)
            .width(ui.available_width() - 88.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut h.jump_host, None, t!("— direkt —"));
                for (id, n) in ssh_hosts
                    .iter()
                    .filter(|(i, _)| *i != me && !route::would_loop(lib, me, *i))
                {
                    ui.selectable_value(&mut h.jump_host, Some(*id), n);
                }
            })
            .response
            .on_hover_text(t!(
                "Jump-Host: Aurora verbindet sich zuerst mit diesem Server und von dort weiter. \
                 Der Jump-Host braucht gespeicherte Zugangsdaten (Identität)."
            ));
        let busy = finder.is_some_and(|f| f.mode == Mode::Editor && f.running);
        if ui
            .add_enabled_ui(!busy, |ui| {
                theme::btn(ui, theme::Btn::Small, Some(Icon::Route), t!("Route"))
            })
            .inner
            .on_hover_text(t!(
                "Weg finden: fragt deine anderen SSH-Hosts, ob sie diesen Host erreichen"
            ))
            .clicked()
        {
            req = Some(EditorRequest::Find);
        }
    });
    // The whole chain when it has more than one hop, or why it's broken.
    let chain_info = match route::jump_chain(lib, h) {
        Ok(chain) if chain.len() > 1 => Some((
            t!("Weg: {} → dieser Host", route::chain_names(lib, &chain)),
            theme::text_dim(),
        )),
        Ok(_) => None,
        Err(e) => Some((e.text().to_string(), theme::red())),
    };
    let finder = finder.filter(|f| f.mode == Mode::Editor && f.target.id == h.id);
    if chain_info.is_none() && finder.is_none() {
        return req;
    }
    rows.full(ui, |ui| {
        if let Some((text, color)) = chain_info {
            ui.label(RichText::new(text).size(12.5).color(color));
        }
        let Some(f) = finder else {
            return;
        };
        if let Some(e) = &f.error {
            ui.label(RichText::new(e).size(12.5).color(theme::red()));
            return;
        }
        ui.horizontal(|ui| match f.best() {
            Some(best) => {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                crate::ui::icons::paint(ui.painter(), r, Icon::Route, theme::green());
                ui.label(
                    RichText::new(t!("Erreichbar über: {}", f.route_text(best)))
                        .size(13.0)
                        .color(theme::text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if h.jump_host != Some(best.id)
                        && theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Übernehmen"))
                            .clicked()
                    {
                        h.jump_host = Some(best.id);
                    }
                });
            }
            None if f.running => {
                ui.add(egui::Spinner::new().size(13.0));
                ui.label(
                    RichText::new(t!(
                        "Frage {} mögliche Jump-Hosts… ({} fertig)",
                        f.names.len(),
                        f.probes.len()
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                );
            }
            None => {
                ui.label(
                    RichText::new(t!("Keiner der gefragten Hosts erreicht das Ziel."))
                        .size(12.5)
                        .color(theme::amber()),
                );
            }
        });
    });
    req
}
