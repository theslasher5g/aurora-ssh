//! Multi-select of guests (Ctrl/Shift+click, click on a tag), the selection bar with
//! batch actions (three at a time) and their result list, drag & drop of a guest row
//! onto a node heading (migration), and the extra entries of the row menus.

use std::collections::BTreeSet;

use super::*;

/// Batch jobs running at the same time.
const PARALLEL: usize = 3;

#[derive(Default)]
pub struct Selection {
    pub set: Vec<(Uuid, u32)>,
    anchor: Option<(Uuid, u32)>,
    /// Row order of the last frame (for Shift ranges), and of the frame being drawn.
    order: Vec<(Uuid, u32)>,
    drawing: Vec<(Uuid, u32)>,
}

/// What a guest row needs from the extras.
pub struct RowExt {
    /// CPU history of the last hour (percent).
    pub spark: Option<Vec<f32>>,
    pub selected: bool,
    /// Something is selected: a plain click toggles too.
    pub selecting: bool,
    /// Other online nodes (for "Migrieren nach ▸").
    pub nodes: Vec<String>,
}

/// A guest row being dragged.
struct DragGuest(GuestRef);

/// Per-row data for the guests of one node, in drawing order.
pub fn row_ext(pve: &mut PveState, server: Uuid, guests: &[&Guest]) -> Vec<RowExt> {
    let online: Vec<String> = pve
        .live
        .get(&server)
        .and_then(|l| l.snap.as_ref())
        .map(|s| {
            s.nodes
                .iter()
                .filter(|n| n.online)
                .map(|n| n.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let selecting = !pve.ext.sel.set.is_empty();
    guests
        .iter()
        .map(|g| {
            pve.ext.sel.drawing.push((server, g.vmid));
            RowExt {
                spark: pve.ext.sparks.visible(server, g),
                selected: pve.ext.sel.set.contains(&(server, g.vmid)),
                selecting,
                nodes: online.iter().filter(|n| **n != g.node).cloned().collect(),
            }
        })
        .collect()
}

/// Selected rows get a tint (painted under the row content).
pub fn row_background(p: &egui::Painter, rect: Rect, ext: &RowExt) {
    if ext.selected {
        p.rect_filled(rect, CornerRadius::ZERO, theme::accent_soft());
    }
}

fn ext_action(a: GuestAction) -> Action {
    Action::Pve(PveAction::Ext(a))
}

/// Clicks (selection), dragging (migration) and the tag pills of a row.
pub fn row_hook(
    ui: &mut egui::Ui,
    resp: &egui::Response,
    g: &Guest,
    r: &GuestRef,
    ext: &RowExt,
    tags: &[(Rect, String)],
    actions: &mut Vec<Action>,
) {
    for (rect, tag) in tags {
        let t = ui
            .interact(
                *rect,
                ui.id().with(("pve-tag", r.server, r.vmid, tag)),
                Sense::click(),
            )
            .on_hover_text(t!("Alle mit Tag „{}“ auswählen", tag))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if t.clicked() {
            actions.push(ext_action(GuestAction::SelectTag(r.server, tag.clone())));
        }
    }
    if resp.clicked() && !g.template {
        let m = ui.input(|i| i.modifiers);
        let mode = if m.shift {
            2
        } else if m.command {
            1
        } else {
            0
        };
        if mode != 0 || ext.selecting {
            actions.push(ext_action(GuestAction::Select(r.clone(), mode)));
        }
    }
    resp.dnd_set_drag_payload(DragGuest(r.clone()));
    if resp.dragged()
        && let Some(pos) = ui.ctx().pointer_latest_pos()
    {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("pve-guest-drag"),
        ));
        let text = t!("{} → Node wählen", r.display());
        let gl = painter.layout_no_wrap(text, theme::semibold(13.0), theme::text());
        let rr = Rect::from_min_size(
            pos + Vec2::new(14.0, 10.0),
            gl.size() + Vec2::new(22.0, 14.0),
        );
        painter.rect(
            rr,
            CornerRadius::same(10),
            theme::surface(),
            Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
        painter.galley(rr.min + Vec2::new(11.0, 7.0), gl, theme::text());
    }
}

/// Clone, hardware, statistics and migration entries (row context menu and the
/// "Mehr" button).
pub fn menu_entries(
    ui: &mut egui::Ui,
    g: &Guest,
    r: &GuestRef,
    ext: &RowExt,
    actions: &mut Vec<Action>,
) {
    if g.template && ui.button(t!("Neu aus Vorlage…")).clicked() {
        actions.push(ext_action(GuestAction::Clone(r.clone(), true)));
        ui.close();
    }
    if ui.button(t!("Klonen…")).clicked() {
        actions.push(ext_action(GuestAction::Clone(r.clone(), false)));
        ui.close();
    }
    if !g.template {
        if ui.button(t!("Hardware…")).clicked() {
            actions.push(ext_action(GuestAction::Hardware(r.clone())));
            ui.close();
        }
        if ui.button(t!("Statistik…")).clicked() {
            actions.push(Action::Pve(PveAction::OpenGuest(r.clone(), 2)));
            ui.close();
        }
    }
    if !ext.nodes.is_empty() {
        ui.menu_button(t!("Migrieren nach"), |ui| {
            for n in &ext.nodes {
                if ui.button(n).clicked() {
                    actions.push(ext_action(GuestAction::Migrate(r.clone(), n.clone())));
                    ui.close();
                }
            }
        });
    }
}

/// "Mehr" button of the hover actions.
pub fn more_button(
    ui: &mut egui::Ui,
    g: &Guest,
    r: &GuestRef,
    ext: &RowExt,
    actions: &mut Vec<Action>,
) {
    let resp = theme::icon_btn(
        ui,
        Icon::Chevron,
        t!("Klonen, Hardware, Statistik, Migration"),
    );
    egui::Popup::menu(&resp)
        .id(more_id(r))
        .show(|ui| menu_entries(ui, g, r, ext, actions));
}

fn more_id(r: &GuestRef) -> egui::Id {
    egui::Id::new(("pve-more", r.server, r.vmid))
}

/// The "Mehr" menu of the row is open (the row keeps its hover actions meanwhile).
pub fn more_open(ctx: &egui::Context, r: &GuestRef) -> bool {
    egui::Popup::is_id_open(ctx, more_id(r))
}

/// Hover actions of a template row.
pub fn template_actions(
    ui: &mut egui::Ui,
    rect: Rect,
    g: &Guest,
    r: &GuestRef,
    ext: &RowExt,
    actions: &mut Vec<Action>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(rect.right() - 400.0, rect.top()),
                rect.right_bottom() - Vec2::new(10.0, 0.0),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 6.0;
    if theme::btn(
        &mut child,
        theme::Btn::SmallPrimary,
        Some(Icon::Plus),
        t!("Neu aus Vorlage…"),
    )
    .clicked()
    {
        actions.push(ext_action(GuestAction::Clone(r.clone(), true)));
    }
    more_button(&mut child, g, r, ext, actions);
}

/// Node heading: click opens its graphs; a dragged guest row can be dropped on it.
pub fn node_heading_hook(
    ui: &egui::Ui,
    server: Uuid,
    node: &str,
    resp: &egui::Response,
    actions: &mut Vec<Action>,
) {
    let dragged = egui::DragAndDrop::payload::<DragGuest>(ui.ctx())
        .filter(|d| d.0.server == server && d.0.node != node);
    if let Some(d) = &dragged {
        let hot = resp.contains_pointer();
        let r = resp.rect.expand2(Vec2::new(0.0, 2.0));
        let p = ui.painter();
        p.rect(
            r,
            CornerRadius::same(8),
            if hot {
                theme::accent_soft()
            } else {
                Color32::TRANSPARENT
            },
            Stroke::new(
                if hot { 1.5 } else { 1.0 },
                if hot {
                    theme::accent()
                } else {
                    theme::fill_strong()
                },
            ),
            egui::StrokeKind::Inside,
        );
        if hot {
            p.text(
                Pos2::new(r.right() - 12.0, r.center().y),
                egui::Align2::RIGHT_CENTER,
                t!("„{}“ hierher migrieren", d.0.display()),
                theme::medium(12.5),
                theme::accent(),
            );
        }
    }
    if let Some(d) = resp.dnd_release_payload::<DragGuest>()
        && d.0.server == server
        && d.0.node != node
    {
        actions.push(ext_action(GuestAction::Migrate(
            d.0.clone(),
            node.to_string(),
        )));
    }
    if dragged.is_none() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        actions.push(ext_action(GuestAction::NodeStats(server, node.to_string())));
    }
}

// ------------------------------------------------------------------ selection

pub fn select(app: &mut App, g: &GuestRef, mode: u8) {
    if live_guest(app, g.server, g.vmid).is_none_or(|x| x.template) {
        return;
    }
    let key = (g.server, g.vmid);
    let sel = &mut app.pve.ext.sel;
    if mode == 2
        && let Some(a) = sel.anchor
        && let (Some(i), Some(j)) = (
            sel.order.iter().position(|k| *k == a),
            sel.order.iter().position(|k| *k == key),
        )
    {
        let (lo, hi) = (i.min(j), i.max(j));
        let range: Vec<(Uuid, u32)> = sel.order[lo..=hi].to_vec();
        for k in range {
            if !sel.set.contains(&k) {
                sel.set.push(k);
            }
        }
        // Templates may sit in the range.
        let pve = &app.pve;
        let keep: Vec<(Uuid, u32)> = pve
            .ext
            .sel
            .set
            .iter()
            .copied()
            .filter(|(s, v)| {
                pve.live
                    .get(s)
                    .and_then(|l| l.snap.as_ref())
                    .and_then(|sn| sn.guests.iter().find(|x| x.vmid == *v))
                    .is_some_and(|x| !x.template)
            })
            .collect();
        app.pve.ext.sel.set = keep;
        return;
    }
    match sel.set.iter().position(|k| *k == key) {
        Some(i) => {
            sel.set.remove(i);
        }
        None => sel.set.push(key),
    }
    sel.anchor = Some(key);
}

pub fn select_tag(app: &mut App, server: Uuid, tag: &str) {
    let keys: Vec<(Uuid, u32)> = app
        .pve
        .live
        .get(&server)
        .and_then(|l| l.snap.as_ref())
        .map(|s| {
            s.guests
                .iter()
                .filter(|g| !g.template && g.tags.iter().any(|t| t == tag))
                .map(|g| (server, g.vmid))
                .collect()
        })
        .unwrap_or_default();
    let n = keys.len();
    app.pve.ext.sel.set = keys;
    app.pve.ext.sel.anchor = None;
    app.toast(t!("Tag „{}“: {} ausgewählt", tag, n), theme::accent());
}

/// Selected guests that still exist (templates never count).
fn selected(app: &App) -> Vec<(GuestRef, Guest)> {
    app.pve
        .ext
        .sel
        .set
        .iter()
        .filter_map(|(s, v)| {
            let g = live_guest(app, *s, *v).filter(|g| !g.template)?;
            Some((GuestRef::of(*s, &g), g))
        })
        .collect()
}

fn modal_open(app: &App) -> bool {
    let e = &app.pve.ext;
    app.pve.panel.is_some()
        || app.pve.editor.is_some()
        || app.pve.ip_prompt.is_some()
        || app.confirm_delete.is_some()
        || e.node_stats.is_some()
        || e.clone.is_some()
        || e.hw.is_some()
        || e.migrate.is_some()
        || e.batch_form.is_some()
        || e.batch.as_ref().is_some_and(|b| b.visible)
}

/// Bar above the guest lists while guests are selected or a batch runs.
pub fn selection_bar(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    {
        let sel = &mut app.pve.ext.sel;
        sel.order = std::mem::take(&mut sel.drawing);
    }
    let live: Vec<(Uuid, u32)> = selected(app)
        .iter()
        .map(|(r, _)| (r.server, r.vmid))
        .collect();
    app.pve.ext.sel.set.retain(|k| live.contains(k));
    if !app.pve.ext.sel.set.is_empty()
        && !modal_open(app)
        && ui.input(|i| i.key_pressed(egui::Key::Escape))
    {
        app.pve.ext.sel.set.clear();
    }
    let n = app.pve.ext.sel.set.len();
    let progress = app.pve.ext.batch.as_ref().map(|b| {
        (
            b.items.iter().filter(|i| i.result.is_some()).count(),
            b.items.len(),
            b.label.clone(),
        )
    });
    if n == 0 && progress.is_none() {
        return;
    }
    let running = progress.as_ref().is_some_and(|(d, t, _)| d < t);
    egui::Frame::NONE
        .fill(theme::accent_soft())
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if n > 0 {
                    ui.label(
                        RichText::new(t!("{} ausgewählt", n))
                            .font(theme::semibold(14.0))
                            .color(theme::text()),
                    );
                    ui.add_space(8.0);
                    ui.add_enabled_ui(!running, |ui| {
                        if theme::btn(ui, theme::Btn::SmallPrimary, Some(Icon::Play), t!("Start"))
                            .clicked()
                        {
                            actions.push(ext_action(GuestAction::BatchPower(Power::Start)));
                        }
                        if theme::btn(ui, theme::Btn::Small, None, t!("Herunterfahren")).clicked() {
                            actions.push(Action::Confirm(
                                t!("{} ausgewählte Gäste sauber herunterfahren?", n),
                                Box::new(ext_action(GuestAction::BatchPower(Power::Shutdown))),
                            ));
                        }
                        if theme::btn(ui, theme::Btn::Small, None, t!("Neustart")).clicked() {
                            actions.push(Action::Confirm(
                                t!("{} ausgewählte Gäste neu starten?", n),
                                Box::new(ext_action(GuestAction::BatchPower(Power::Reboot))),
                            ));
                        }
                        if theme::btn(ui, theme::Btn::Small, None, t!("Snapshot…")).clicked() {
                            actions.push(ext_action(GuestAction::BatchForm(0)));
                        }
                        if theme::btn(
                            ui,
                            theme::Btn::Small,
                            Some(Icon::Download),
                            t!("Backup jetzt…"),
                        )
                        .clicked()
                        {
                            actions.push(ext_action(GuestAction::BatchForm(1)));
                        }
                    });
                    if theme::btn(ui, theme::Btn::Link, None, t!("Auswahl aufheben"))
                        .on_hover_text("Esc")
                        .clicked()
                    {
                        actions.push(ext_action(GuestAction::ClearSelection));
                    }
                }
                if let Some((done, total, label)) = &progress {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::btn(ui, theme::Btn::Link, None, t!("Ergebnis anzeigen")).clicked()
                            && let Some(b) = app.pve.ext.batch.as_mut()
                        {
                            b.visible = true;
                        }
                        ui.label(
                            RichText::new(t!("{}: {} von {}", label, done, total))
                                .size(13.0)
                                .color(theme::text_dim()),
                        );
                        if done < total {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                    });
                }
            });
        });
    ui.add_space(12.0);
}

// ------------------------------------------------------------------ batches

pub struct BatchItem {
    pub g: GuestRef,
    op: Op,
    task: Option<u64>,
    /// `Some(Ok(text))` also for skipped guests (see `skipped`).
    result: Option<Result<String, String>>,
    skipped: bool,
}

pub struct Batch {
    label: String,
    pub items: Vec<BatchItem>,
    visible: bool,
}

fn batch_running(app: &App) -> bool {
    app.pve
        .ext
        .batch
        .as_ref()
        .is_some_and(|b| b.items.iter().any(|i| i.result.is_none()))
}

fn start_batch(app: &mut App, label: String, items: Vec<BatchItem>) {
    if batch_running(app) {
        app.toast(t!("Es läuft schon eine Sammelaktion"), theme::amber());
        return;
    }
    if items.is_empty() {
        return;
    }
    app.pve.ext.batch = Some(Batch {
        label,
        items,
        visible: true,
    });
    pump(app);
}

pub fn run_power(app: &mut App, p: Power) {
    let items = selected(app)
        .into_iter()
        .map(|(r, g)| {
            let fits = match p {
                Power::Start => !g.running(),
                Power::Shutdown | Power::Reboot | Power::Stop => g.running(),
                Power::Resume => matches!(g.status.as_str(), "paused" | "suspended"),
            };
            BatchItem {
                g: r,
                op: Op::Ext(Box::new(ExtOp::Power(p))),
                task: None,
                result: (!fits).then(|| {
                    Ok(if g.running() {
                        t!("übersprungen – läuft schon").to_string()
                    } else {
                        t!("übersprungen – läuft nicht").to_string()
                    })
                }),
                skipped: !fits,
            }
        })
        .collect();
    let label = match p {
        Power::Start => t!("Start"),
        Power::Shutdown => t!("Herunterfahren"),
        Power::Reboot => t!("Neustart"),
        Power::Stop => t!("Stopp"),
        Power::Resume => t!("Fortsetzen"),
    };
    start_batch(app, label.to_string(), items);
}

/// Starts queued jobs while fewer than [`PARALLEL`] run.
pub fn pump(app: &mut App) {
    let Some(b) = app.pve.ext.batch.as_mut() else {
        return;
    };
    // Tasks dropped meanwhile (server edited, vault locked) never report back.
    let known: HashSet<u64> = app.pve.tasks.iter().map(|t| t.id).collect();
    for i in b.items.iter_mut() {
        if let (Some(id), None) = (i.task, &i.result)
            && !known.contains(&id)
        {
            i.result = Some(Err(t!("abgebrochen").to_string()));
        }
    }
    let running = b
        .items
        .iter()
        .filter(|i| i.task.is_some() && i.result.is_none())
        .count();
    let next: Vec<usize> = b
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.task.is_none() && i.result.is_none())
        .map(|(i, _)| i)
        .take(PARALLEL.saturating_sub(running))
        .collect();
    for i in next {
        let Some((g, op)) = app
            .pve
            .ext
            .batch
            .as_ref()
            .map(|b| (b.items[i].g.clone(), b.items[i].op.clone()))
        else {
            return;
        };
        let id = start_task_with(app, &g, op, true);
        if let Some(it) = app.pve.ext.batch.as_mut().map(|b| &mut b.items[i]) {
            if id == 0 {
                it.result = Some(Err(t!("Server nicht mehr vorhanden").to_string()));
            } else {
                it.task = Some(id);
            }
        }
    }
}

/// A task finished: record it if it belongs to the batch.
pub fn task_done(app: &mut App, task: u64, result: &Result<String, String>) {
    let Some(b) = app.pve.ext.batch.as_mut() else {
        return;
    };
    let Some(it) = b.items.iter_mut().find(|i| i.task == Some(task)) else {
        return;
    };
    it.result = Some(result.clone());
    if b.items.iter().all(|i| i.result.is_some()) {
        let ok = b
            .items
            .iter()
            .filter(|i| !i.skipped && matches!(i.result, Some(Ok(_))))
            .count();
        let failed = b
            .items
            .iter()
            .filter(|i| matches!(i.result, Some(Err(_))))
            .count();
        let label = b.label.clone();
        app.toast(
            t!("{}: {} erfolgreich, {} fehlgeschlagen", label, ok, failed),
            if failed > 0 {
                theme::amber()
            } else {
                theme::green()
            },
        );
    }
}

/// Tasks that open sheets or the batch still show.
pub fn held_tasks(app: &App) -> Vec<u64> {
    let e = &app.pve.ext;
    let mut out: Vec<u64> = e
        .batch
        .iter()
        .flat_map(|b| b.items.iter().filter_map(|i| i.task))
        .collect();
    out.extend(e.clone.as_ref().and_then(|c| c.task));
    out.extend(e.hw.as_ref().and_then(|h| h.task));
    out
}

pub fn result_sheet(app: &mut App, ctx: &egui::Context) {
    let tasks: HashMap<u64, String> = app
        .pve
        .tasks
        .iter()
        .map(|t| (t.id, t.progress.clone()))
        .collect();
    let Some(b) = app.pve.ext.batch.as_mut().filter(|b| b.visible) else {
        return;
    };
    let done = b.items.iter().filter(|i| i.result.is_some()).count();
    let total = b.items.len();
    let title = t!("Sammelaktion: {}", b.label);
    let (close, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-batch-result"),
        620.0,
        &title,
        |ui| {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if done < total {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                ui.label(
                    RichText::new(t!("{} von {} fertig", done, total)).color(theme::text_dim()),
                );
            });
            group(ui, |ui| {
                for (n, it) in b.items.iter().enumerate() {
                    if n > 0 {
                        let r = ui.max_rect();
                        let y = ui.cursor().top();
                        ui.painter().line_segment(
                            [Pos2::new(r.left() + 14.0, y), Pos2::new(r.right(), y)],
                            Stroke::new(1.0, theme::line()),
                        );
                    }
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                match &it.result {
                                    None if it.task.is_some() => {
                                        ui.add(egui::Spinner::new().size(14.0));
                                    }
                                    None => {
                                        ui.label(RichText::new("…").color(theme::text_dim()));
                                    }
                                    Some(_) if it.skipped => {
                                        ui.label(RichText::new("–").color(theme::text_dim()));
                                    }
                                    Some(Ok(_)) => {
                                        ui.label(RichText::new("✔").color(theme::green()));
                                    }
                                    Some(Err(_)) => {
                                        ui.label(RichText::new("✖").color(theme::red()));
                                    }
                                }
                                ui.label(RichText::new(it.g.label()).font(theme::medium(13.0)));
                                ui.label(
                                    RichText::new(&it.g.node)
                                        .size(12.0)
                                        .color(theme::text_dim()),
                                );
                                let (text, color) = match &it.result {
                                    None if it.task.is_some() => (
                                        it.task
                                            .and_then(|id| tasks.get(&id).cloned())
                                            .unwrap_or_default(),
                                        theme::text_dim(),
                                    ),
                                    None => (t!("wartet").to_string(), theme::text_dim()),
                                    Some(Ok(s)) => (s.clone(), theme::text_dim()),
                                    Some(Err(e)) => (e.clone(), theme::red()),
                                };
                                ui.add(
                                    egui::Label::new(RichText::new(text).size(12.5).color(color))
                                        .truncate(),
                                );
                            });
                        });
                }
            });
        },
        |ui, _| theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked(),
    );
    if close || dismissed {
        if done == total {
            app.pve.ext.batch = None;
        } else {
            b.visible = false;
        }
    }
    if done < total {
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

// ------------------------------------------------------------------ batch form

pub struct BatchForm {
    /// 0 snapshot, 1 backup.
    kind: u8,
    pub guests: Vec<GuestRef>,
    name: String,
    description: String,
    storage: String,
    mode: usize,
    /// Backup storages present on every involved node.
    stores: Option<Result<Vec<String>, String>>,
}

pub fn open_form(app: &mut App, kind: u8) {
    let guests: Vec<GuestRef> = selected(app).into_iter().map(|(r, _)| r).collect();
    if guests.is_empty() {
        return;
    }
    let server = guests[0].server;
    let nodes: Vec<String> = guests
        .iter()
        .map(|g| g.node.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let multi_server = guests.iter().any(|g| g.server != server);
    app.pve.ext.batch_form = Some(BatchForm {
        kind,
        guests,
        name: format!("aurora_{}", chrono::Local::now().format("%Y%m%d_%H%M")),
        description: String::new(),
        storage: String::new(),
        mode: 0,
        stores: (kind == 1 && multi_server).then(|| {
            Err(t!("Backups gehen nur für Gäste eines Servers gleichzeitig.").to_string())
        }),
    });
    if kind == 1
        && !multi_server
        && let Some(s) = server_of(app, server)
    {
        let c = conn(app, &s);
        pve::spawn_stores(
            &app.rt,
            s.id,
            app.pve.epoch(s.id),
            c,
            0,
            nodes,
            "backup",
            app.pve.sink.clone(),
        );
    }
}

/// Storage ids present in every node's list.
fn common_stores(lists: &[(String, Vec<pve::StoreInfo>)]) -> Vec<String> {
    let mut iter = lists.iter();
    let Some((_, first)) = iter.next() else {
        return Vec::new();
    };
    let mut ids: Vec<String> = first.iter().map(|s| s.id.clone()).collect();
    for (_, l) in iter {
        ids.retain(|id| l.iter().any(|s| s.id == *id));
    }
    ids
}

pub fn on_stores(app: &mut App, result: Result<Vec<(String, Vec<pve::StoreInfo>)>, String>) {
    if let Some(f) = app.pve.ext.batch_form.as_mut().filter(|f| f.kind == 1) {
        let r = result.map(|l| common_stores(&l));
        if let Ok(ids) = &r
            && !ids.contains(&f.storage)
        {
            f.storage = ids.first().cloned().unwrap_or_default();
        }
        f.stores = Some(r);
    }
}

pub fn form_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(f) = app.pve.ext.batch_form.as_mut() else {
        return;
    };
    let n = f.guests.len();
    let title = if f.kind == 0 {
        t!("Snapshot für {} Gäste", n)
    } else {
        t!("Backup für {} Gäste", n)
    };
    let ok = if f.kind == 0 {
        pve::valid_snapname(f.name.trim())
    } else {
        matches!(&f.stores, Some(Ok(ids)) if ids.contains(&f.storage))
    };
    let ((go, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("pve-batch-form"),
        560.0,
        &title,
        |ui| {
            let names: Vec<String> = f.guests.iter().map(|g| g.display()).collect();
            ui.add(
                egui::Label::new(
                    RichText::new(names.join(", "))
                        .size(12.5)
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
            if f.kind == 0 {
                theme::form(ui, 110.0, |ui, rows| {
                    rows.field(ui, t!("Name"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.name)
                                .char_limit(40)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    rows.field(ui, t!("Beschreibung"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.description)
                                .hint_text(RichText::new(t!("optional")).color(theme::text_dim()))
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
                if !ok && !f.name.is_empty() {
                    hint(
                        ui,
                        t!(
                            "Name: mit einem Buchstaben beginnen, dann nur A–Z, a–z, 0–9, _ und - (2–40 Zeichen)"
                        ),
                    );
                }
            } else {
                theme::form(ui, 110.0, |ui, rows| {
                    rows.field(ui, t!("Speicher"), |ui| match &f.stores {
                        None => {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                        Some(Err(_)) => {
                            ui.label(RichText::new("–").color(theme::text_dim()));
                        }
                        Some(Ok(ids)) => {
                            let sel = if f.storage.is_empty() {
                                t!("— wählen —").to_string()
                            } else {
                                f.storage.clone()
                            };
                            egui::ComboBox::from_id_salt("pve-batch-store")
                                .selected_text(sel)
                                .width(ui.available_width() - 14.0)
                                .show_ui(ui, |ui| {
                                    for id in ids {
                                        ui.selectable_value(&mut f.storage, id.clone(), id);
                                    }
                                });
                        }
                    });
                    rows.field(ui, t!("Modus"), |ui| {
                        if let Some(i) = theme::segmented(
                            ui,
                            &[t!("Snapshot"), t!("Anhalten"), t!("Stoppen")],
                            f.mode,
                        ) {
                            f.mode = i;
                        }
                    });
                });
                match &f.stores {
                    Some(Err(e)) => hint(ui, e),
                    Some(Ok(ids)) if ids.is_empty() => hint(
                        ui,
                        t!("Kein Backup-Speicher ist auf allen beteiligten Nodes verfügbar."),
                    ),
                    _ => hint(
                        ui,
                        t!("Je Gast ein eigener Backup-Job (zstd), höchstens drei gleichzeitig."),
                    ),
                }
            }
        },
        |ui, _| {
            let go = ui
                .add_enabled_ui(ok, |ui| {
                    theme::btn(ui, theme::Btn::Primary, None, t!("Für alle starten"))
                })
                .inner
                .clicked();
            let cancel = theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked();
            (go, cancel)
        },
    );
    if go {
        let f = app.pve.ext.batch_form.take().expect("batch form");
        let op = if f.kind == 0 {
            Op::Snapshot {
                name: f.name.trim().to_string(),
                description: f.description.clone(),
                vmstate: false,
            }
        } else {
            Op::Backup {
                storage: f.storage.clone(),
                mode: pve::BACKUP_MODES[f.mode.min(2)].to_string(),
            }
        };
        let label = if f.kind == 0 {
            t!("Snapshot „{}“", f.name.trim())
        } else {
            t!("Backup nach {}", f.storage)
        };
        let items = f
            .guests
            .into_iter()
            .map(|g| BatchItem {
                g,
                op: op.clone(),
                task: None,
                result: None,
                skipped: false,
            })
            .collect();
        start_batch(app, label, items);
    } else if cancel || dismissed {
        app.pve.ext.batch_form = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_backup_stores() {
        let s = |id: &str| pve::StoreInfo {
            id: id.into(),
            kind: "dir".into(),
            avail: 0,
            shared: false,
        };
        let lists = vec![
            ("pve1".to_string(), vec![s("local"), s("pbs"), s("nas")]),
            ("pve2".to_string(), vec![s("pbs"), s("local")]),
        ];
        assert_eq!(common_stores(&lists), vec!["local", "pbs"]);
        assert!(common_stores(&[]).is_empty());
    }
}
