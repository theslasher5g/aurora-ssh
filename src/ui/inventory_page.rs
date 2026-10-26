//! "Inventar" page: every saved host in a sortable, filterable table (or cards in narrow
//! windows) with warnings, background refresh of the host facts and export.

use std::collections::{HashSet, VecDeque};

use chrono::Utc;
use egui::{CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{Action, App};
use crate::backend::Sink;
use crate::backend::inventory_doc as doc;
use crate::backend::inventory_export::{self as export, Format};
use crate::backend::inventory_info::InfoSnippet;
use crate::backend::inventory_refresh::{self as refresh, Done};
use crate::backend::inventory_rules::{self as rules, Col, InventorySettings, Row};
use crate::backend::ssh::SshParams;
use crate::model::Protocol;
use crate::theme;
use crate::ui::containers_page::{Skip, background_params};
use crate::ui::icons::Icon;

/// Refresh run in progress or finished.
pub struct Run {
    queue: VecDeque<(Uuid, SshParams, Vec<InfoSnippet>)>,
    running: HashSet<Uuid>,
    total: usize,
    pub done: Vec<(Uuid, Result<(), String>)>,
    pub skipped: Vec<(Uuid, Skip)>,
    finished: bool,
}

impl Run {
    pub fn active(&self) -> bool {
        !self.finished
    }

    /// The host is being refreshed or waits for it.
    pub fn busy(&self, id: Uuid) -> bool {
        self.running.contains(&id) || self.queue.iter().any(|(i, _, _)| *i == id)
    }
}

#[derive(Default)]
pub struct InvState {
    io: Option<(Sink<Done>, UnboundedReceiver<Done>)>,
    pub text: String,
    pub only_problems: bool,
    pub only_prod: bool,
    pub group: Option<Uuid>,
    pub selected: HashSet<Uuid>,
    pub run: Option<Run>,
    /// The result panel was dismissed.
    pub hide_result: bool,
    /// Detail sheet of one host.
    pub detail: crate::ui::inventory_detail::DetailState,
    /// Export dialog is open.
    pub export_open: bool,
    /// Rows of the page, rebuilt only when something changed (`nav2_inv`).
    pub cache: crate::ui::nav2_inv::InvCache,
    /// Bumped whenever collected facts arrive (cache key).
    pub facts_epoch: u64,
}

impl InvState {
    fn sink(&mut self, ctx: &egui::Context) -> Sink<Done> {
        self.io
            .get_or_insert_with(|| Sink::new(ctx.clone()))
            .0
            .clone()
    }
}

#[derive(Clone, Debug)]
pub enum InvAction {
    /// Refresh the facts of these hosts (empty = all SSH hosts).
    Refresh(Vec<Uuid>),
    /// Export with the saved choices; `true` = to the clipboard.
    Export(bool),
    ExportDialog,
    /// Open the detail sheet of this host.
    Detail(Uuid),
    OpenFolder,
}

// ---------------------------------------------------------------------------
// Background refresh

pub fn pump(app: &mut App, ctx: &egui::Context) {
    let mut events = Vec::new();
    if let Some((_, rx)) = app.inv.io.as_mut() {
        while let Ok(d) = rx.try_recv() {
            events.push(d);
        }
    }
    if events.is_empty() {
        return;
    }
    for d in events {
        let result = match d.result {
            Ok(c) => {
                app.inv.facts_epoch += 1;
                app.lib.facts.insert(d.host, c.facts);
                app.lib.detail.insert(d.host, c.detail);
                Ok(())
            }
            Err(e) => Err(e),
        };
        if let Some(run) = app.inv.run.as_mut() {
            run.running.remove(&d.host);
            run.done.push((d.host, result));
        }
    }
    fill(app, ctx);
    let finished = app
        .inv
        .run
        .as_ref()
        .is_some_and(|r| r.queue.is_empty() && r.running.is_empty() && !r.finished);
    if finished {
        if let Some(r) = app.inv.run.as_mut() {
            r.finished = true;
        }
        app.save_library();
        let (ok, bad, skipped) = app.inv.run.as_ref().map_or((0, 0, 0), |r| {
            let ok = r.done.iter().filter(|(_, x)| x.is_ok()).count();
            (ok, r.done.len() - ok, r.skipped.len())
        });
        let text = t!(
            "Fakten aktualisiert: {} ok, {} fehlgeschlagen, {} übersprungen",
            ok,
            bad,
            skipped
        );
        app.toast(
            text,
            if bad > 0 {
                theme::amber()
            } else {
                theme::green()
            },
        );
    }
}

/// Starts queued hosts while fewer than [`refresh::PARALLEL`] run.
fn fill(app: &mut App, ctx: &egui::Context) {
    loop {
        let sink = app.inv.sink(ctx);
        let Some(run) = app.inv.run.as_mut() else {
            return;
        };
        if run.running.len() >= refresh::PARALLEL {
            return;
        }
        let Some((id, params, snippets)) = run.queue.pop_front() else {
            return;
        };
        run.running.insert(id);
        refresh::spawn(
            &app.rt,
            id,
            params,
            snippets,
            sink,
            app.prompts_sink.clone(),
        );
    }
}

fn start_refresh(app: &mut App, ctx: &egui::Context, ids: Vec<Uuid>) {
    if app.inv.run.as_ref().is_some_and(Run::active) {
        return;
    }
    let hosts: Vec<_> = if ids.is_empty() {
        app.lib
            .hosts
            .iter()
            .filter(|h| h.protocol == Protocol::Ssh)
            .cloned()
            .collect()
    } else {
        app.lib
            .hosts
            .iter()
            .filter(|h| ids.contains(&h.id) && h.protocol == Protocol::Ssh)
            .cloned()
            .collect()
    };
    let mut run = Run {
        queue: VecDeque::new(),
        running: HashSet::new(),
        total: hosts.len(),
        done: Vec::new(),
        skipped: Vec::new(),
        finished: false,
    };
    for h in hosts {
        match background_params(app, &h) {
            Ok(p) => {
                let chain = rules::group_path(&app.lib, &h).1;
                let snippets = app
                    .lib
                    .settings
                    .inventory
                    .info_snippets
                    .iter()
                    .filter(|s| s.applies(&h, &chain))
                    .cloned()
                    .collect();
                run.queue.push_back((h.id, p, snippets));
            }
            Err(s) => run.skipped.push((h.id, s)),
        }
    }
    let nothing = run.queue.is_empty();
    app.inv.hide_result = false;
    app.inv.run = Some(run);
    if nothing {
        if let Some(r) = app.inv.run.as_mut() {
            r.finished = true;
        }
    } else {
        fill(app, ctx);
    }
}

// ---------------------------------------------------------------------------
// Actions

pub fn apply(app: &mut App, a: InvAction, ctx: &egui::Context) {
    match a {
        InvAction::Refresh(ids) => start_refresh(app, ctx, ids),
        InvAction::ExportDialog => app.inv.export_open = true,
        InvAction::Detail(id) => app.inv.detail.open(id),
        InvAction::Export(to_clipboard) => {
            let now = Utc::now();
            let (rows, cols) = visible(app, now);
            let prefs = app.lib.settings.inventory.export.clone();
            let f = match prefs.format.as_str() {
                "csv" => Format::Csv,
                "html" => Format::Html,
                "json" => Format::Json,
                _ => Format::Markdown,
            };
            let text = if f == Format::Csv || (prefs.table_only && f != Format::Html) {
                export::render(f, &rows, &cols, now)
            } else {
                let hosts = doc::hosts_from(&app.lib, &rows);
                let meta = doc::DocMeta {
                    title: t!("Serverdokumentation").to_string(),
                    created: now,
                    scope: scope_text(app, rows.len()),
                };
                doc::render(f, &hosts, &(&prefs).into(), &meta)
            };
            if to_clipboard {
                ctx.copy_text(text);
                app.toast(t!("In die Zwischenablage kopiert"), theme::green());
            } else {
                match export::save(f, &text) {
                    Ok(p) => app.toast(t!("Exportiert nach {}", p.display()), theme::green()),
                    Err(e) => app.toast(format!("{e:#}"), theme::red()),
                }
            }
        }
        InvAction::OpenFolder => {
            let d = export::dir();
            let _ = std::fs::create_dir_all(&d);
            #[cfg(windows)]
            let _ = std::process::Command::new("explorer.exe").arg(&d).spawn();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&d).spawn();
            #[cfg(all(unix, not(target_os = "macos")))]
            let _ = std::process::Command::new("xdg-open").arg(&d).spawn();
        }
    }
}

/// The current filter in words, for the document header.
fn scope_text(app: &App, hosts: usize) -> String {
    let mut parts = Vec::new();
    if !app.inv.text.trim().is_empty() {
        parts.push(t!("Suche „{}“", app.inv.text.trim()));
    }
    if app.inv.only_problems {
        parts.push(t!("nur Hosts mit Problemen").to_string());
    }
    if app.inv.only_prod {
        parts.push(t!("nur PROD").to_string());
    }
    if let Some(g) = app.inv.group.and_then(|g| app.lib.group(g)) {
        parts.push(t!("Gruppe „{}“", g.name));
    }
    if parts.is_empty() {
        t!("Alle Hosts ({})", hosts)
    } else {
        t!("Gefiltert: {} ({} Hosts)", parts.join(", "), hosts)
    }
}

/// Filtered and sorted rows and the visible columns (what the table shows).
pub fn visible(app: &App, _now: chrono::DateTime<Utc>) -> (Vec<Row>, Vec<Col>) {
    (
        crate::ui::nav2_inv::export_rows(app),
        columns(&app.lib.settings.inventory),
    )
}

pub fn columns(s: &InventorySettings) -> Vec<Col> {
    Col::ALL
        .iter()
        .copied()
        .filter(|c| rules::col_visible(s, *c))
        .collect()
}

// ---------------------------------------------------------------------------
// Page

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let now = Utc::now();
    crate::ui::nav2_inv::ensure(app);
    let summary = app.inv.cache.summary;
    let cols = columns(&app.lib.settings.inventory);
    let n_rows = app.inv.cache.order.len();
    let n_all = app.inv.cache.all.len();
    if app.inv.run.as_ref().is_some_and(Run::active) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }

    let mut st = std::mem::take(&mut app.inv);
    let mut settings = app.lib.settings.inventory.clone();
    let before = settings.clone();
    let running = st.run.as_ref().is_some_and(Run::active);
    let sel: Vec<Uuid> = st.selected.iter().copied().collect();
    let groups: Vec<(Uuid, String)> = app
        .lib
        .groups
        .iter()
        .map(|g| {
            let (path, _) = rules::group_path(
                &app.lib,
                &crate::model::Host {
                    group_id: Some(g.id),
                    ..Default::default()
                },
            );
            (g.id, path)
        })
        .collect();

    let mut text = std::mem::take(&mut st.text);
    let mut chip_actions = Vec::new();
    let app_ref = &*app;
    theme::page_header(
        ui,
        "inv",
        |ui| {
            theme::page_title(ui, t!("Inventar"), Some(n_rows));
            crate::ui::nav_list::scope_chip(app_ref, ui, &mut chip_actions);
        },
        |_| {},
        |ui, bar| {
            export_button(ui, bar.compact, actions);
            let label = if bar.compact {
                String::new()
            } else if sel.is_empty() {
                t!("Fakten aktualisieren").to_string()
            } else {
                t!("Fakten aktualisieren ({})", sel.len())
            };
            ui.add_enabled_ui(!running, |ui| {
                if theme::btn(ui, theme::Btn::Primary, Some(Icon::Refresh), &label)
                    .on_hover_text(t!(
                        "Betriebssystem, Updates, Festplatte … aller (markierten) SSH-Hosts im Hintergrund neu einlesen – nur mit gespeicherten Zugangsdaten"
                    ))
                    .clicked()
                {
                    actions.push(Action::Inv(InvAction::Refresh(sel.clone())));
                }
            });
            let width = bar.flex(if bar.compact { 120.0 } else { 160.0 }, 280.0);
            theme::search_field(
                ui,
                &mut text,
                t!("Name, OS, Tag, Gruppe"),
                width,
                egui::Id::new("inv-search"),
            );
        },
    );
    st.text = text;
    actions.append(&mut chip_actions);
    ui.add_space(14.0);

    tiles(ui, &summary, &mut st);
    ui.add_space(12.0);
    filter_bar(ui, &mut st, &mut settings, &groups);
    ui.add_space(10.0);
    run_panel(ui, app, &mut st);

    egui::ScrollArea::vertical()
        .id_salt("inv-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            if n_rows == 0 {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(if n_all == 0 {
                            t!("Noch keine Hosts gespeichert")
                        } else {
                            t!("Keine Hosts passen zum Filter")
                        })
                        .color(theme::text_dim()),
                    );
                });
            } else {
                let cache = std::mem::take(&mut st.cache);
                crate::ui::inventory_table::show(
                    ui,
                    &cache,
                    &cols,
                    &mut settings,
                    &mut st.selected,
                    now,
                    actions,
                );
                st.cache = cache;
            }
            ui.add_space(40.0);
        });

    app.inv = st;
    if settings != before {
        app.lib.settings.inventory = settings;
        app.save_library();
    }
    export_dialog(app, ui.ctx(), actions);
    crate::ui::inventory_detail::show(app, ui.ctx(), actions);
}

fn export_button(ui: &mut egui::Ui, compact: bool, actions: &mut Vec<Action>) {
    let label = if compact { "" } else { t!("Export") };
    if theme::btn(ui, theme::Btn::Normal, Some(Icon::Download), label)
        .on_hover_text(t!(
            "Inventar als CSV, Markdown, HTML oder JSON exportieren (ohne Zugangsdaten)"
        ))
        .clicked()
    {
        actions.push(Action::Inv(InvAction::ExportDialog));
    }
}

/// Export dialog: format and what goes into the file.
fn export_dialog(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    if !app.inv.export_open {
        return;
    }
    let mut prefs = app.lib.settings.inventory.export.clone();
    let before = prefs.clone();
    let hosts = app.inv.cache.order.len();
    let scope = scope_text(app, hosts);
    let formats = [
        ("csv", "CSV"),
        ("md", "Markdown"),
        ("html", "HTML"),
        ("json", "JSON"),
    ];
    let (act, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("inv-export"),
        540.0,
        t!("Inventar exportieren"),
        |ui| {
            let labels: Vec<&str> = formats.iter().map(|(_, n)| *n).collect();
            let cur = formats
                .iter()
                .position(|(k, _)| *k == prefs.format)
                .unwrap_or(1);
            if let Some(i) = theme::segmented(ui, &labels, cur) {
                prefs.format = formats[i].0.to_string();
            }
            let blurb = match prefs.format.as_str() {
                "csv" => t!("Tabelle mit den Spalten der Ansicht, für Excel und Co."),
                "html" => t!(
                    "Eine Datei zum Drucken oder als PDF speichern: Inhaltsverzeichnis, Kapitel je Host, helles Layout."
                ),
                "json" => t!("Alle gesammelten Daten maschinenlesbar."),
                _ => t!(
                    "Systemdokumentation: ein Kapitel je Host mit Hardware, Betriebssystem, Netzwerk, Festplatten, Diensten, Ports und Firewall."
                ),
            };
            ui.add(
                egui::Label::new(RichText::new(blurb).size(12.5).color(theme::text_dim())).wrap(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(t!("Umfang: {}", scope))
                        .size(12.5)
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
            if matches!(prefs.format.as_str(), "md" | "json") {
                ui.checkbox(
                    &mut prefs.table_only,
                    t!("Nur Tabelle (Spalten der Ansicht, ohne Details)"),
                );
            }
            let detailed = prefs.format != "csv"
                && !(prefs.table_only && matches!(prefs.format.as_str(), "md" | "json"));
            ui.add_enabled_ui(detailed, |ui| {
                ui.add_space(4.0);
                theme::section_title(ui, t!("Enthalten"));
                ui.checkbox(&mut prefs.services, t!("Dienste"));
                ui.checkbox(&mut prefs.ports, t!("Offene Ports"));
                ui.checkbox(&mut prefs.firewall, t!("Firewall-Regeln"));
                ui.checkbox(&mut prefs.info, t!("Info-Snippets (eigene Befehle)"));
                ui.checkbox(
                    &mut prefs.ips,
                    t!("IP-Adressen (Host, Schnittstellen, Ports, Regeln)"),
                )
                .on_hover_text(t!(
                    "Aus: Adressen fehlen oder werden durch ‹IP› ersetzt – gut zum Weitergeben"
                ));
                ui.checkbox(&mut prefs.notes, t!("Notizen der Hosts"))
                    .on_hover_text(t!(
                        "Notizen können sensible Hinweise enthalten – deshalb standardmäßig aus"
                    ));
            });
        },
        |ui, _| {
            let mut act = None;
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                act = Some(0);
            }
            if theme::btn(
                ui,
                theme::Btn::Primary,
                Some(Icon::Download),
                t!("Speichern"),
            )
            .clicked()
            {
                act = Some(1);
            }
            if theme::btn(ui, theme::Btn::Normal, None, t!("Kopieren")).clicked() {
                act = Some(2);
            }
            if theme::btn(ui, theme::Btn::Link, None, t!("Ordner öffnen")).clicked() {
                actions.push(Action::Inv(InvAction::OpenFolder));
            }
            act
        },
    );
    if prefs != before {
        app.lib.settings.inventory.export = prefs;
        app.save_library();
    }
    match act {
        Some(1) => actions.push(Action::Inv(InvAction::Export(false))),
        Some(2) => actions.push(Action::Inv(InvAction::Export(true))),
        _ => {}
    }
    if dismissed || act.is_some() {
        app.inv.export_open = false;
    }
}

/// One summary tile; returns the click.
fn tile(
    ui: &mut egui::Ui,
    w: f32,
    number: &str,
    caption: &str,
    color: egui::Color32,
    on: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 62.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(12),
        if on {
            theme::accent_soft()
        } else {
            theme::surface()
        },
    );
    p.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(
            1.0,
            if resp.hovered() {
                theme::line()
            } else {
                theme::card_edge()
            },
        ),
        egui::StrokeKind::Inside,
    );
    p.text(
        Pos2::new(rect.left() + 14.0, rect.top() + 24.0),
        egui::Align2::LEFT_CENTER,
        number,
        theme::bold(22.0),
        color,
    );
    let g = theme::truncated(p, caption, theme::medium(12.0), theme::text_dim(), w - 24.0);
    p.galley(
        Pos2::new(rect.left() + 14.0, rect.top() + 47.0 - g.size().y / 2.0),
        g,
        theme::text_dim(),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn tiles(ui: &mut egui::Ui, s: &rules::Summary, st: &mut InvState) {
    let gap = 10.0;
    let avail = ui.available_width();
    let n = if avail >= 4.0 * 150.0 + 3.0 * gap {
        4.0
    } else {
        2.0
    };
    let w = ((avail - (n - 1.0) * gap) / n).floor().min(260.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(gap);
        tile(
            ui,
            w,
            &s.hosts.to_string(),
            t!("Hosts"),
            theme::text(),
            false,
        );
        let warn = if s.critical > 0 {
            theme::red()
        } else if s.with_warnings > 0 {
            theme::amber()
        } else {
            theme::green()
        };
        if tile(
            ui,
            w,
            &s.with_warnings.to_string(),
            &if s.critical > 0 {
                t!("mit Warnungen ({} kritisch)", s.critical)
            } else {
                t!("mit Warnungen").to_string()
            },
            warn,
            st.only_problems,
        )
        .on_hover_text(t!("Klick: nur Hosts mit Warnungen"))
        .clicked()
        {
            st.only_problems = !st.only_problems;
        }
        tile(
            ui,
            w,
            &s.updates.to_string(),
            t!("Updates offen"),
            if s.updates > 0 {
                theme::amber()
            } else {
                theme::text()
            },
            false,
        );
        tile(
            ui,
            w,
            &s.security.to_string(),
            t!("Sicherheits-Updates"),
            if s.security > 0 {
                theme::red()
            } else {
                theme::text()
            },
            false,
        );
    });
    // The summary line from the brief, as text.
    ui.add_space(6.0);
    ui.label(
        RichText::new(t!(
            "{} Hosts · {} mit Warnungen · {} Updates offen",
            s.hosts,
            s.with_warnings,
            s.updates
        ))
        .size(12.5)
        .color(theme::text_dim()),
    );
}

fn filter_bar(
    ui: &mut egui::Ui,
    st: &mut InvState,
    s: &mut InventorySettings,
    groups: &[(Uuid, String)],
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        let toggle = |ui: &mut egui::Ui, on: &mut bool, label: &str| {
            let kind = if *on {
                theme::Btn::SmallPrimary
            } else {
                theme::Btn::Small
            };
            if theme::btn(ui, kind, None, label).clicked() {
                *on = !*on;
            }
        };
        toggle(ui, &mut st.only_problems, t!("Nur Probleme"));
        toggle(ui, &mut st.only_prod, t!("Nur PROD"));
        let cur = st
            .group
            .and_then(|g| groups.iter().find(|(id, _)| *id == g))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("Alle Gruppen").to_string());
        egui::ComboBox::from_id_salt("inv-group")
            .selected_text(cur)
            .width(170.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut st.group, None, t!("Alle Gruppen"));
                for (id, n) in groups {
                    ui.selectable_value(&mut st.group, Some(*id), n);
                }
            });
        toggle(ui, &mut s.group_by_group, t!("Nach Gruppe"));
        // Sorting for the card layout (the table sorts by clicking the headers).
        let sort = Col::from_key(&s.sort_col).unwrap_or(Col::Name);
        egui::ComboBox::from_id_salt("inv-sort")
            .selected_text(format!("{}: {}", t!("Sortierung"), sort.label()))
            .width(190.0)
            .show_ui(ui, |ui| {
                for c in Col::ALL {
                    if ui.selectable_label(c == sort, c.label()).clicked() {
                        s.sort_col = c.key().to_string();
                    }
                }
            });
        if theme::icon_btn(
            ui,
            Icon::Down,
            if s.sort_desc {
                t!("Absteigend")
            } else {
                t!("Aufsteigend")
            },
        )
        .clicked()
        {
            s.sort_desc = !s.sort_desc;
        }
        let cols = theme::btn(ui, theme::Btn::Small, Some(Icon::Table), t!("Spalten"));
        egui::Popup::menu(&cols).show(|ui| {
            for c in Col::ALL {
                let mut on = rules::col_visible(s, c);
                if ui
                    .add_enabled(c != Col::Name, egui::Checkbox::new(&mut on, c.label()))
                    .changed()
                {
                    rules::set_col(s, c, on);
                }
            }
            ui.separator();
            if ui.button(t!("Standard")).clicked() {
                s.hidden_cols = InventorySettings::default().hidden_cols;
                s.extra_cols.clear();
            }
        });
    });
}

/// Progress and result of the last "Fakten aktualisieren".
fn run_panel(ui: &mut egui::Ui, app: &App, st: &mut InvState) {
    let Some(run) = st.run.as_ref() else {
        return;
    };
    if st.hide_result && !run.active() {
        return;
    }
    let name = |id: Uuid| {
        app.lib
            .host(id)
            .map(|h| h.display_name().to_string())
            .unwrap_or_default()
    };
    let mut close = false;
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let done = run.done.len();
            let total = run.total - run.skipped.len();
            if run.active() {
                ui.spinner();
                ui.label(
                    RichText::new(t!("Fakten werden aktualisiert… {} von {}", done, total))
                        .font(theme::semibold(13.5)),
                );
            } else {
                let bad = run.done.iter().filter(|(_, r)| r.is_err()).count();
                ui.label(
                    RichText::new(t!(
                        "Fakten aktualisiert: {} ok, {} fehlgeschlagen, {} übersprungen",
                        done - bad,
                        bad,
                        run.skipped.len()
                    ))
                    .font(theme::semibold(13.5)),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !run.active() && theme::icon_btn(ui, Icon::Close, t!("Schließen")).clicked() {
                    close = true;
                }
            });
        });
        if run.active() && run.total > 0 {
            let frac = run.done.len() as f32 / (run.total - run.skipped.len()).max(1) as f32;
            let (bar, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 5.0), Sense::hover());
            ui.painter()
                .rect_filled(bar, CornerRadius::same(3), theme::fill_strong());
            ui.painter().rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(bar.width() * frac.clamp(0.0, 1.0), 5.0)),
                CornerRadius::same(3),
                theme::accent(),
            );
        }
        let problems: Vec<(String, String, bool)> = run
            .done
            .iter()
            .filter_map(|(id, r)| r.as_ref().err().map(|e| (name(*id), e.clone(), true)))
            .chain(
                run.skipped
                    .iter()
                    .map(|(id, s)| (name(*id), s.text().to_string(), false)),
            )
            .collect();
        if !problems.is_empty() {
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("inv-run-problems")
                .max_height(120.0)
                .show(ui, |ui| {
                    for (n, why, failed) in &problems {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} {n}: {why}",
                                    if *failed { "✕" } else { "–" }
                                ))
                                .size(12.5)
                                .color(if *failed {
                                    theme::red()
                                } else {
                                    theme::text_dim()
                                }),
                            )
                            .wrap(),
                        );
                    }
                });
        }
    });
    ui.add_space(10.0);
    if close {
        st.hide_result = true;
    }
}

// ---------------------------------------------------------------------------
// Settings

/// Thresholds card in Settings → Allgemein.
pub fn settings_card(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, t!("Inventar"));
    });
    ui.add_space(2.0);
    let s = &mut app.lib.settings.inventory;
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Festplatte"),
            Some(t!("Warnung und kritisch ab … % belegt")),
            |ui| {
                ui.horizontal(|ui| {
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut s.disk_warn)
                                .range(1..=99)
                                .suffix(" %"),
                        )
                        .changed();
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut s.disk_crit)
                                .range(1..=100)
                                .suffix(" %"),
                        )
                        .changed();
                });
                if s.disk_crit < s.disk_warn {
                    s.disk_crit = s.disk_warn;
                }
            },
        );
        rows.setting(
            ui,
            t!("Ausstehende Updates"),
            Some(t!("Warnung ab … Paketen (0 = aus)")),
            |ui| {
                *changed |= ui
                    .add(egui::DragValue::new(&mut s.updates_warn).range(0..=999))
                    .changed();
            },
        );
        rows.setting(
            ui,
            t!("Laufzeit ohne Neustart"),
            Some(t!("Warnung ab … Tagen (0 = aus)")),
            |ui| {
                *changed |= ui
                    .add(egui::DragValue::new(&mut s.uptime_days).range(0..=3650))
                    .changed();
            },
        );
        rows.setting(
            ui,
            t!("Alter der Fakten"),
            Some(t!("Warnung, wenn älter als … Tage (0 = aus)")),
            |ui| {
                *changed |= ui
                    .add(egui::DragValue::new(&mut s.facts_stale_days).range(0..=3650))
                    .changed();
            },
        );
        rows.setting(
            ui,
            t!("Veraltete Distributionen"),
            Some(&t!(
                "Warnt vor Ubuntu 20.04 und älter, Debian 11 und älter, CentOS 8 und älter … (kleine eingebaute Tabelle, Stand {})",
                rules::EOL_TABLE_AS_OF
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut s.eol_check).changed();
            },
        );
    });
    ui.add_space(22.0);
    snippets_card(app, ui, changed);
}

/// Info snippets: own commands that run on the hosts when the facts are refreshed.
fn snippets_card(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, t!("Info-Snippets"));
    });
    ui.add_space(2.0);
    let groups: Vec<(Uuid, String)> = app
        .lib
        .groups
        .iter()
        .map(|g| {
            let h = crate::model::Host {
                group_id: Some(g.id),
                ..Default::default()
            };
            (g.id, rules::group_path(&app.lib, &h).0)
        })
        .collect();
    let hosts: Vec<(Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh)
        .map(|h| (h.id, h.display_name().to_string()))
        .collect();
    let list = &mut app.lib.settings.inventory.info_snippets;
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Eigene Befehle, die bei „Fakten aktualisieren“ per SSH auf den passenden Hosts laufen. Die Textausgabe (höchstens 64 KB, 30 s) wird beim Host gespeichert, im Detail-Tab „Info“ gezeigt und in Markdown/HTML/JSON exportiert – für alles, was die eingebauten Fakten nicht abdecken (auch Windows mit OpenSSH: eigene PowerShell-Befehle). Keine Passwörter in Befehle schreiben: sie stehen im Klartext in der Bibliothek."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            )
            .wrap(),
        );
        let mut remove = None;
        for (i, s) in list.iter_mut().enumerate() {
            ui.add_space(8.0);
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    *changed |= ui.checkbox(&mut s.enabled, "").changed();
                    *changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut s.name)
                                .hint_text(t!("Name"))
                                .desired_width((ui.available_width() - 44.0).clamp(80.0, 280.0)),
                        )
                        .changed();
                    if theme::icon_btn(ui, Icon::Trash, t!("Löschen")).clicked() {
                        remove = Some(i);
                    }
                });
                *changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut s.command)
                            .hint_text(t!("Befehl, z. B. docker ps -a"))
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(t!("Nur auf:"))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                    let cur = s
                        .group
                        .and_then(|g| groups.iter().find(|(id, _)| *id == g))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| t!("Alle Gruppen").to_string());
                    egui::ComboBox::from_id_salt(("inv-info-group", i))
                        .selected_text(cur)
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            *changed |= ui.selectable_value(&mut s.group, None, t!("Alle Gruppen")).changed();
                            for (id, n) in &groups {
                                *changed |= ui.selectable_value(&mut s.group, Some(*id), n).changed();
                            }
                        });
                    let cur = s
                        .host
                        .and_then(|g| hosts.iter().find(|(id, _)| *id == g))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| t!("Alle Hosts").to_string());
                    egui::ComboBox::from_id_salt(("inv-info-host", i))
                        .selected_text(cur)
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            *changed |= ui.selectable_value(&mut s.host, None, t!("Alle Hosts")).changed();
                            for (id, n) in &hosts {
                                *changed |= ui.selectable_value(&mut s.host, Some(*id), n).changed();
                            }
                        });
                    *changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut s.tag)
                                .hint_text(t!("Tag"))
                                .desired_width(90.0),
                        )
                        .changed();
                });
            });
        }
        if let Some(i) = remove {
            list.remove(i);
            *changed = true;
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if theme::btn(ui, theme::Btn::Small, Some(Icon::Plus), t!("Info-Snippet")).clicked()
                && list.len() < crate::backend::inventory_info::MAX_SNIPPETS
            {
                list.push(InfoSnippet::default());
                *changed = true;
            }
            let r = theme::btn(ui, theme::Btn::Small, None, t!("Vorlage hinzufügen"));
            egui::Popup::menu(&r).show(|ui| {
                for (name, cmd) in crate::backend::inventory_info::presets() {
                    if ui.button(name).clicked() {
                        list.push(InfoSnippet {
                            name: name.to_string(),
                            command: cmd.to_string(),
                            ..Default::default()
                        });
                        *changed = true;
                        ui.close();
                    }
                }
            });
        });
    });
    ui.add_space(22.0);
}
