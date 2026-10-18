//! Runbooks page (next to Snippets), editor, Markdown import/export, the start sheet and
//! the run panel next to a terminal.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Local;
use egui::text::LayoutJob;
use egui::{Color32, CornerRadius, FontId, Pos2, RichText, Sense, Stroke, TextFormat, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, ConnState, TabKind, TermTab};
use crate::backend::TermCmd;
use crate::backend::runbook::{self, RunLog, Runbook, RunbookStep};
use crate::model::{Protocol, ShellKind};
use crate::theme;
use crate::ui::editors::{field_text, save_cancel};
use crate::ui::icons::Icon;
use crate::ui::ops::{OpsAction, OpsState};
use crate::ui::{empty_list, fuzzy, row_buttons};

/// A run attached to a terminal tab.
pub struct ActiveRun {
    pub log: RunLog,
    /// Output goes to this step (the one started last).
    pub capturing: Option<usize>,
    /// Step whose "Bestätigung nötig" question is shown.
    pub armed: Option<usize>,
    /// Panel collapsed (the run keeps capturing).
    pub hidden: bool,
    /// Last written report.
    pub report: Option<PathBuf>,
}

/// Hosts being connected for a run; the run attaches to their new tab.
struct Pending {
    host: Uuid,
    /// Only tabs with a higher id are new.
    after: u64,
    log: RunLog,
    since: Instant,
}

pub struct StartSheet {
    pub runbook: Runbook,
    /// 0 = open sessions, 1 = hosts (new sessions).
    pub mode: usize,
    pub tabs: HashSet<u64>,
    pub hosts: HashSet<Uuid>,
    pub filter: String,
    pub values: Vec<(String, String)>,
}

#[derive(Default)]
pub struct ImportSheet {
    pub path: String,
    pub text: String,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct RunbookUi {
    pub search: String,
    pub editor: Option<(Runbook, bool)>,
    pub import: Option<ImportSheet>,
    pub start: Option<StartSheet>,
    pub runs: HashMap<u64, ActiveRun>,
    pending: Vec<Pending>,
    /// Messages from places without access to the app (run panel).
    toasts: Vec<(String, Color32)>,
}

/// Terminal output of a tab while a run captures it.
pub fn capture(ops: &mut OpsState, tab_id: u64, bytes: &[u8]) {
    if let Some(run) = ops.runbooks.runs.get_mut(&tab_id)
        && let Some(i) = run.capturing
        && let Some(step) = run.log.steps.get_mut(i)
    {
        let room = runbook::MAX_OUTPUT.saturating_sub(step.output.len());
        step.output
            .extend_from_slice(&bytes[..bytes.len().min(room)]);
    }
}

pub fn apply(app: &mut App, a: OpsAction, _ctx: &egui::Context) {
    let ui = &mut app.ops.runbooks;
    match a {
        OpsAction::NewRunbook => {
            ui.editor = Some((
                Runbook {
                    steps: vec![RunbookStep::default()],
                    ..Default::default()
                },
                true,
            ))
        }
        OpsAction::EditRunbook(id) => {
            if let Some(rb) = app.lib.runbooks.iter().find(|r| r.id == id) {
                ui.editor = Some((rb.clone(), false));
            }
        }
        OpsAction::DuplicateRunbook(id) => {
            if let Some(rb) = app.lib.runbooks.iter().find(|r| r.id == id) {
                ui.editor = Some((
                    Runbook {
                        id: Uuid::new_v4(),
                        title: t!("{} (Kopie)", rb.title),
                        ..rb.clone()
                    },
                    true,
                ));
            }
        }
        OpsAction::DeleteRunbook(id) => {
            app.lib.runbooks.retain(|r| r.id != id);
            app.save_library();
        }
        OpsAction::ImportRunbook => ui.import = Some(ImportSheet::default()),
        OpsAction::ExportRunbook(id) => export(app, id),
        OpsAction::RunRunbook(id, tab) => open_start(app, id, tab),
        _ => {}
    }
}

fn export(app: &mut App, id: Uuid) {
    let Some(rb) = app.lib.runbooks.iter().find(|r| r.id == id) else {
        return;
    };
    let md = runbook::to_markdown(rb);
    let path =
        runbook::output_dir("Runbooks").join(format!("{}.md", runbook::safe_name(&rb.title)));
    match std::fs::write(&path, &md) {
        Ok(()) => {
            app.toast(
                t!(
                    "Exportiert: {} (auch in der Zwischenablage)",
                    path.display()
                ),
                theme::green(),
            );
        }
        Err(e) => app.toast(t!("Export fehlgeschlagen: {}", e), theme::red()),
    }
    CLIPBOARD.with(|c| *c.borrow_mut() = Some(md));
}

thread_local! {
    /// Text to put on the clipboard on the next frame (actions run without a `Ui`).
    static CLIPBOARD: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn open_start(app: &mut App, id: Uuid, tab: Option<u64>) {
    let Some(rb) = app.lib.runbooks.iter().find(|r| r.id == id).cloned() else {
        return;
    };
    let tabs: HashSet<u64> = tab
        .or(app.active)
        .filter(|t| term_tabs(app).iter().any(|(id, _, _)| id == t))
        .into_iter()
        .collect();
    let values = rb
        .variables()
        .into_iter()
        .map(|v| (v, String::new()))
        .collect();
    app.ops.runbooks.start = Some(StartSheet {
        mode: if tabs.is_empty() && term_tabs(app).is_empty() {
            1
        } else {
            0
        },
        runbook: rb,
        tabs,
        hosts: HashSet::new(),
        filter: String::new(),
        values,
    });
}

/// Connected terminal tabs without a run: (id, title, subtitle).
fn term_tabs(app: &App) -> Vec<(u64, String, String)> {
    app.tabs
        .iter()
        .filter(|t| {
            matches!(&t.kind, TabKind::Term(term) if matches!(term.state, ConnState::Connected))
                && !app.ops.runbooks.runs.contains_key(&t.id)
        })
        .map(|t| (t.id, t.title.clone(), t.subtitle.clone()))
        .collect()
}

// ------------------------------------------------------------------ page

pub fn page(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let count = app.lib.runbooks.len();
    let mut new = false;
    let mut import = false;
    theme::page_header(
        ui,
        "runbooks",
        |ui| theme::page_title(ui, "Runbooks", Some(count)),
        |_| {},
        |ui, bar| {
            new = theme::btn(ui, theme::Btn::Primary, Some(Icon::Plus), t!("Neu")).clicked();
            let tip = t!("Runbook aus einer Markdown-Datei übernehmen");
            import = if bar.compact {
                theme::icon_btn(
                    ui,
                    Icon::Download,
                    &format!("{}\n{}", t!("Importieren"), tip),
                )
            } else {
                theme::btn(
                    ui,
                    theme::Btn::Normal,
                    Some(Icon::Download),
                    t!("Importieren"),
                )
                .on_hover_text(tip)
            }
            .clicked();
            let width = bar.flex(160.0, 260.0);
            theme::search_field(
                ui,
                &mut app.ops.runbooks.search,
                t!("Suchen"),
                width,
                egui::Id::new("runbook-search"),
            );
        },
    );
    ui.add_space(22.0);
    if new {
        actions.push(Action::Ops(OpsAction::NewRunbook));
    }
    if import {
        actions.push(Action::Ops(OpsAction::ImportRunbook));
    }
    switch(app, ui);

    let q = app.ops.runbooks.search.clone();
    let mut list: Vec<(i32, Runbook)> = app
        .lib
        .runbooks
        .iter()
        .filter_map(|r| {
            let hay = format!(
                "{} {} {}",
                r.title,
                r.description,
                r.steps
                    .iter()
                    .map(|s| format!("{} {}", s.text, s.command))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            fuzzy(&q, &hay).map(|sc| (sc, r.clone()))
        })
        .collect();
    list.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.cmp(&b.1.title)));

    egui::ScrollArea::vertical()
        .id_salt("runbooks-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            if list.is_empty() {
                let (title, body) = if count == 0 {
                    (
                        t!("Noch keine Runbooks"),
                        t!("Ein Runbook führt Schritt für Schritt durch eine Aufgabe – mit Befehlen, Notizen und einem Protokoll am Ende."),
                    )
                } else {
                    (t!("Kein Runbook gefunden"), "")
                };
                if empty_list(ui, title, body, t!("Runbook anlegen")) {
                    actions.push(Action::Ops(OpsAction::NewRunbook));
                }
                return;
            }
            theme::card(ui, theme::bg_deep(), |ui| {
                for (i, (_, rb)) in list.into_iter().enumerate() {
                    row(ui, i, &rb, actions);
                }
            });
        });
}

/// "Snippets | Runbooks" switch shown on both pages.
pub fn switch(app: &mut App, ui: &mut egui::Ui) {
    if let Some(i) = theme::segmented(ui, &["Snippets", "Runbooks"], app.ops.page) {
        app.ops.page = i;
    }
    ui.add_space(14.0);
}

fn row(ui: &mut egui::Ui, i: usize, rb: &Runbook, actions: &mut Vec<Action>) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 68.0), Sense::click());
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if i > 0 {
        p.line_segment(
            [Pos2::new(rect.left() + 18.0, rect.top()), rect.right_top()],
            Stroke::new(1.0, theme::line()),
        );
    }
    let left = rect.left() + 18.0;
    crate::ui::icons::paint(
        &p,
        egui::Rect::from_center_size(Pos2::new(left + 9.0, rect.center().y), Vec2::splat(18.0)),
        Icon::Checklist,
        theme::accent(),
    );
    let x = left + 32.0;
    let g = p.layout_no_wrap(rb.title.clone(), theme::semibold(15.0), theme::text());
    p.galley(
        Pos2::new(x, rect.top() + 23.0 - g.size().y / 2.0),
        g,
        theme::text(),
    );
    let n = rb.steps.len();
    let mut sub = if n == 1 {
        t!("1 Schritt").to_string()
    } else {
        t!("{} Schritte", n)
    };
    if !rb.description.trim().is_empty() {
        sub = format!(
            "{sub} · {}",
            rb.description
                .lines()
                .next()
                .unwrap_or_default()
                .replace("**", "")
                .replace('`', "")
        );
    }
    p.text(
        Pos2::new(x, rect.top() + 46.0),
        egui::Align2::LEFT_CENTER,
        sub,
        FontId::proportional(13.0),
        theme::text_dim(),
    );
    if hovered {
        let labels = [
            t!("Ausführen"),
            t!("Bearbeiten"),
            t!("Exportieren"),
            t!("Löschen"),
        ];
        match row_buttons(ui, rect, rb.id, &labels) {
            Some(0) => actions.push(Action::Ops(OpsAction::RunRunbook(rb.id, None))),
            Some(1) => actions.push(Action::Ops(OpsAction::EditRunbook(rb.id))),
            Some(2) => actions.push(Action::Ops(OpsAction::ExportRunbook(rb.id))),
            Some(3) => actions.push(Action::Confirm(
                t!("Runbook „{}“ löschen?", rb.title),
                Box::new(Action::Ops(OpsAction::DeleteRunbook(rb.id))),
            )),
            _ => {}
        }
    } else {
        let mut cx = rect.right() - 18.0;
        for v in rb.variables().into_iter().rev() {
            let g = p.layout_no_wrap(
                format!("{{{{{v}}}}}"),
                FontId::monospace(12.0),
                theme::accent(),
            );
            let r = egui::Rect::from_min_size(
                Pos2::new(cx - g.size().x - 18.0, rect.center().y - 11.0),
                Vec2::new(g.size().x + 18.0, 22.0),
            );
            p.rect_filled(r, CornerRadius::same(7), theme::accent_soft());
            p.galley(r.center() - g.size() / 2.0, g, theme::accent());
            cx = r.left() - 6.0;
        }
    }
    if resp.double_clicked() {
        actions.push(Action::Ops(OpsAction::RunRunbook(rb.id, None)));
    }
    resp.context_menu(|ui| {
        if ui.button(t!("Ausführen…")).clicked() {
            actions.push(Action::Ops(OpsAction::RunRunbook(rb.id, None)));
            ui.close();
        }
        if ui.button(t!("Bearbeiten")).clicked() {
            actions.push(Action::Ops(OpsAction::EditRunbook(rb.id)));
            ui.close();
        }
        if ui.button(t!("Duplizieren")).clicked() {
            actions.push(Action::Ops(OpsAction::DuplicateRunbook(rb.id)));
            ui.close();
        }
        if ui.button(t!("Als Markdown exportieren")).clicked() {
            actions.push(Action::Ops(OpsAction::ExportRunbook(rb.id)));
            ui.close();
        }
    });
}

// ------------------------------------------------------------------ sheets & side tasks

pub fn windows(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    if let Some(text) = CLIPBOARD.with(|c| c.borrow_mut().take()) {
        ctx.copy_text(text);
    }
    for (text, color) in std::mem::take(&mut app.ops.runbooks.toasts) {
        app.toast(text, color);
    }
    attach_pending(app);
    editor(app, ctx);
    import_sheet(app, ctx);
    start_sheet(app, ctx);
    // Runs of closed tabs are gone.
    let ids: HashSet<u64> = app.tabs.iter().map(|t| t.id).collect();
    app.ops.runbooks.runs.retain(|id, _| ids.contains(id));
    let _ = actions;
}

fn attach_pending(app: &mut App) {
    if app.ops.runbooks.pending.is_empty() {
        return;
    }
    let mut pending = std::mem::take(&mut app.ops.runbooks.pending);
    pending.retain(|p| p.since.elapsed() < Duration::from_secs(300));
    let mut keep = Vec::new();
    for mut p in pending {
        let tab = app.tabs.iter().find(|t| {
            t.id > p.after
                && t.host_id == Some(p.host)
                && matches!(t.kind, TabKind::Term(_))
                && !app.ops.runbooks.runs.contains_key(&t.id)
        });
        match tab {
            Some(t) => {
                p.log.host = t.title.clone();
                p.log.target = t.subtitle.clone();
                let id = t.id;
                app.ops.runbooks.runs.insert(id, new_run(p.log));
            }
            None => keep.push(p),
        }
    }
    app.ops.runbooks.pending = keep;
}

fn new_run(log: RunLog) -> ActiveRun {
    ActiveRun {
        log,
        capturing: None,
        armed: None,
        hidden: false,
        report: None,
    }
}

fn editor(app: &mut App, ctx: &egui::Context) {
    let Some((rb, is_new)) = app.ops.runbooks.editor.as_mut() else {
        return;
    };
    let title = if *is_new {
        t!("Neues Runbook")
    } else {
        t!("Runbook bearbeiten")
    };
    let ((save, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("runbook-editor"),
        640.0,
        title,
        |ui| {
            theme::form(ui, 110.0, |ui, f| {
                f.field(ui, t!("Titel"), |ui| {
                    field_text(ui, &mut rb.title, t!("Webserver neu starten"));
                });
                f.full(ui, |ui| {
                    theme::frameless(ui);
                    ui.add(
                        egui::TextEdit::multiline(&mut rb.description)
                            .hint_text(t!("Beschreibung (optional)"))
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
            let n = rb.steps.len();
            let mut op: Option<(usize, i32)> = None;
            for (i, s) in rb.steps.iter_mut().enumerate() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    theme::section_title(ui, &t!("Schritt {}", i + 1));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if theme::icon_btn(ui, Icon::Trash, t!("Schritt entfernen")).clicked() {
                            op = Some((i, 0));
                        }
                        ui.add_enabled_ui(i + 1 < n, |ui| {
                            if theme::icon_btn(ui, Icon::Down, t!("Nach unten")).clicked() {
                                op = Some((i, 1));
                            }
                        });
                        ui.add_enabled_ui(i > 0, |ui| {
                            if theme::icon_btn(ui, Icon::Up, t!("Nach oben")).clicked() {
                                op = Some((i, -1));
                            }
                        });
                    });
                });
                theme::form(ui, 110.0, |ui, f| {
                    f.full(ui, |ui| {
                        theme::frameless(ui);
                        ui.add(
                            egui::TextEdit::multiline(&mut s.text)
                                .hint_text(t!(
                                    "Was ist zu tun? **fett** und `Code` werden hervorgehoben"
                                ))
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    f.full(ui, |ui| {
                        theme::frameless(ui);
                        ui.add(
                            egui::TextEdit::multiline(&mut s.command)
                                .font(egui::TextStyle::Monospace)
                                .hint_text(t!(
                                    "Befehl (optional), z. B. systemctl restart {{dienst}}"
                                ))
                                .desired_rows(1)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    f.field(ui, t!("Shell"), |ui| {
                        let cur = usize::from(s.shell == ShellKind::PowerShell);
                        if let Some(k) = theme::segmented(ui, &["Shell", "PowerShell"], cur) {
                            s.shell = [ShellKind::Shell, ShellKind::PowerShell][k];
                        }
                    });
                    f.switch(ui, t!("Bestätigung nötig"), &mut s.confirm);
                });
            }
            if let Some((i, d)) = op {
                match d {
                    0 => {
                        rb.steps.remove(i);
                    }
                    -1 => rb.steps.swap(i, i - 1),
                    _ => rb.steps.swap(i, i + 1),
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(Icon::Plus),
                    t!("Schritt hinzufügen"),
                )
                .clicked()
                {
                    rb.steps.push(RunbookStep::default());
                }
                let vars = rb.variables();
                if !vars.is_empty() {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(t!("Variablen:"))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                    for v in vars {
                        theme::chip(ui, &format!("{{{{{v}}}}}"), theme::accent());
                    }
                }
            });
            !rb.title.trim().is_empty() && !rb.steps.is_empty()
        },
        save_cancel,
    );
    if cancel || dismissed {
        app.ops.runbooks.editor = None;
    } else if save && let Some((mut rb, _)) = app.ops.runbooks.editor.take() {
        rb.title = rb.title.trim().to_string();
        match app.lib.runbooks.iter_mut().find(|x| x.id == rb.id) {
            Some(x) => *x = rb,
            None => app.lib.runbooks.push(rb),
        }
        app.save_library();
    }
}

fn import_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(s) = app.ops.runbooks.import.as_mut() else {
        return;
    };
    let ((go, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("runbook-import"),
        600.0,
        t!("Runbook importieren"),
        |ui| {
            ui.label(
                RichText::new(t!(
                    "Markdown-Datei: # Titel, Beschreibung, dann je Schritt eine ## Überschrift mit optionalem Codeblock als Befehl."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            theme::form(ui, 110.0, |ui, f| {
                f.field(ui, t!("Datei"), |ui| {
                    field_text(ui, &mut s.path, "~/Documents/Aurora/Runbooks/backup.md");
                });
            });
            theme::form_caption(ui, t!("Oder Markdown einfügen"));
            theme::form(ui, 110.0, |ui, f| {
                f.full(ui, |ui| {
                    theme::frameless(ui);
                    ui.add(
                        egui::TextEdit::multiline(&mut s.text)
                            .font(egui::TextStyle::Monospace)
                            .hint_text(
                                "# Backup\n\n## 1. Dump\n```bash\npg_dump app > app.sql\n```",
                            )
                            .desired_rows(8)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(theme::red()));
            }
            !s.path.trim().is_empty() || !s.text.trim().is_empty()
        },
        |ui, ok| {
            let mut out = (false, false);
            ui.add_enabled_ui(ok, |ui| {
                if theme::btn(ui, theme::Btn::Primary, None, t!("Importieren")).clicked() {
                    out.0 = true;
                }
            });
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                out.1 = true;
            }
            out
        },
    );
    if cancel || dismissed {
        app.ops.runbooks.import = None;
        return;
    }
    if !go {
        return;
    }
    let text = if s.text.trim().is_empty() {
        let p = expand_home(s.path.trim());
        match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) => {
                s.error = Some(t!("{}: {}", p.display(), e));
                return;
            }
        }
    } else {
        s.text.clone()
    };
    match runbook::from_markdown(&text) {
        Ok(rb) => {
            app.ops.runbooks.import = None;
            let title = rb.title.clone();
            app.lib.runbooks.push(rb);
            app.save_library();
            app.toast(t!("Runbook „{}“ importiert", title), theme::green());
        }
        Err(e) => s.error = Some(e.to_string()),
    }
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => directories::UserDirs::new()
            .map(|u| u.home_dir().join(rest))
            .unwrap_or_else(|| PathBuf::from(p)),
        None => PathBuf::from(p),
    }
}

fn start_sheet(app: &mut App, ctx: &egui::Context) {
    if app.ops.runbooks.start.is_none() {
        return;
    }
    let tabs = term_tabs(app);
    let hosts: Vec<(Uuid, String, String)> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol.is_terminal() && h.protocol != Protocol::Serial)
        .map(|h| (h.id, h.display_name().to_string(), h.subtitle()))
        .collect();
    let Some(s) = app.ops.runbooks.start.as_mut() else {
        return;
    };
    let title = t!("Ausführen · {}", s.runbook.title);
    let ((go, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("runbook-start"),
        560.0,
        &title,
        |ui| {
            let modes = [t!("Offene Sessions"), t!("Hosts verbinden")];
            if let Some(i) = theme::segmented(ui, &modes, s.mode) {
                s.mode = i;
            }
            ui.add_space(4.0);
            theme::form(ui, 110.0, |ui, f| {
                if s.mode == 0 {
                    if tabs.is_empty() {
                        f.full(ui, |ui| {
                            ui.label(
                                RichText::new(t!(
                                    "Keine verbundene Terminal-Session ohne laufendes Runbook."
                                ))
                                .color(theme::text_dim()),
                            );
                        });
                    }
                    for (id, name, sub) in &tabs {
                        f.full(ui, |ui| {
                            let mut on = s.tabs.contains(id);
                            if ui
                                .checkbox(&mut on, RichText::new(name).font(theme::semibold(13.5)))
                                .changed()
                            {
                                if on {
                                    s.tabs.insert(*id);
                                } else {
                                    s.tabs.remove(id);
                                }
                            }
                            ui.label(theme::mono(sub, 11.5).color(theme::text_dim()));
                        });
                    }
                } else {
                    f.field(ui, t!("Filter"), |ui| {
                        field_text(ui, &mut s.filter, t!("Name oder Adresse"));
                    });
                    let q = s.filter.to_lowercase();
                    for (id, name, sub) in hosts.iter().filter(|(_, n, a)| {
                        q.is_empty()
                            || n.to_lowercase().contains(&q)
                            || a.to_lowercase().contains(&q)
                    }) {
                        f.full(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut on = s.hosts.contains(id);
                                if ui
                                    .checkbox(
                                        &mut on,
                                        RichText::new(name).font(theme::semibold(13.5)),
                                    )
                                    .changed()
                                {
                                    if on {
                                        s.hosts.insert(*id);
                                    } else {
                                        s.hosts.remove(id);
                                    }
                                }
                                ui.label(theme::mono(sub, 11.5).color(theme::text_dim()));
                            });
                        });
                    }
                }
            });
            if !s.values.is_empty() {
                theme::form_caption(ui, t!("Variablen"));
                theme::form(ui, 130.0, |ui, f| {
                    for (name, value) in s.values.iter_mut() {
                        f.field(ui, name, |ui| {
                            field_text(ui, value, "");
                        });
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(t!(
                            "Werden einmal abgefragt und in alle Befehle eingesetzt."
                        ))
                        .size(12.0)
                        .color(theme::text_dim()),
                    );
                });
            }
            if s.mode == 0 {
                !s.tabs.is_empty()
            } else {
                !s.hosts.is_empty()
            }
        },
        |ui, ok| {
            let mut out = (false, false);
            ui.add_enabled_ui(ok, |ui| {
                if theme::btn(ui, theme::Btn::Primary, Some(Icon::Play), t!("Starten")).clicked() {
                    out.0 = true;
                }
            });
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                out.1 = true;
            }
            out
        },
    );
    if cancel || dismissed {
        app.ops.runbooks.start = None;
        return;
    }
    if !go {
        return;
    }
    let Some(s) = app.ops.runbooks.start.take() else {
        return;
    };
    let make = |host: String, target: String| {
        RunLog::new(s.runbook.clone(), s.values.clone(), host, target)
    };
    if s.mode == 0 {
        for (id, name, sub) in tabs.iter().filter(|(id, _, _)| s.tabs.contains(id)) {
            app.ops
                .runbooks
                .runs
                .insert(*id, new_run(make(name.clone(), sub.clone())));
        }
        if let Some(first) = s.tabs.iter().min() {
            app.active = Some(*first);
        }
    } else {
        let after = app.next_id;
        let ids: Vec<Uuid> = hosts
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| s.hosts.contains(id))
            .collect();
        for h in &ids {
            app.ops.runbooks.pending.push(Pending {
                host: *h,
                after,
                log: make(String::new(), String::new()),
                since: Instant::now(),
            });
        }
        // Several hosts end up side by side.
        if ids.len() == 1 {
            if let Some(h) = app.lib.host(ids[0]).cloned() {
                app.connect_host(h, crate::app::Purpose::Terminal, ctx);
            }
        } else {
            app.apply(Action::ConnectMany(ids), ctx);
        }
    }
}

// ------------------------------------------------------------------ session header & panel

/// Runbook button in a terminal header: picks a runbook to start, or shows/hides the
/// running one.
pub fn header_button(
    ui: &mut egui::Ui,
    ops: &mut OpsState,
    runbooks: &[(Uuid, String)],
    tab_id: u64,
    actions: &mut Vec<Action>,
) {
    if let Some(run) = ops.runbooks.runs.get_mut(&tab_id) {
        let r = theme::icon_toggle(
            ui,
            Icon::Checklist,
            t!("Runbook-Leiste ein-/ausblenden"),
            !run.hidden,
        );
        if r.clicked() {
            run.hidden = !run.hidden;
        }
        return;
    }
    if runbooks.is_empty() {
        return;
    }
    let r = theme::icon_btn(
        ui,
        Icon::Checklist,
        t!("Runbook in dieser Session ausführen"),
    );
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(220.0);
        for (id, title) in runbooks {
            if ui.button(title).clicked() {
                actions.push(Action::Ops(OpsAction::RunRunbook(*id, Some(tab_id))));
                ui.close();
            }
        }
    });
}

/// Steps of the run beside the terminal. Returns a dangerous command waiting for the PROD
/// confirmation (typed, Enter not yet sent).
pub fn terminal_panel(
    ops: &mut OpsState,
    ui: &mut egui::Ui,
    tab_id: u64,
    t: &mut TermTab,
    guarded: bool,
) -> Option<(String, &'static str)> {
    let toasts = &mut ops.runbooks.toasts;
    let run = ops.runbooks.runs.get_mut(&tab_id)?;
    if run.hidden {
        return None;
    }
    let live = matches!(t.state, ConnState::Connected);
    let mut exec: Option<usize> = None;
    let mut report = false;
    let mut close = false;
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    egui::Panel::right(egui::Id::new(("term-runbook", tab_id)))
        .resizable(false)
        .exact_size(360.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::bg_panel())
                .inner_margin(egui::Margin::symmetric(14, 12)),
        )
        .show(ui, |ui| {
            let now = Local::now();
            ui.horizontal(|ui| {
                crate::ui::icons::show(ui, Icon::Checklist, 16.0, theme::accent());
                ui.add(
                    egui::Label::new(
                        RichText::new(&run.log.runbook.title)
                            .font(theme::semibold(15.0))
                            .color(theme::text()),
                    )
                    .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_btn(ui, Icon::Close, t!("Runbook beenden")).clicked() {
                        close = true;
                    }
                });
            });
            let done = run.log.steps.iter().filter(|s| s.done).count();
            let total = run.log.steps.len();
            let took = (now - run.log.started).to_std().unwrap_or_default();
            ui.label(
                RichText::new(t!(
                    "{} von {} erledigt · {}",
                    done,
                    total,
                    crate::ui::short_duration(took)
                ))
                .size(12.0)
                .color(theme::text_dim()),
            );
            let w = ui.available_width();
            let (bar, _) = ui.allocate_exact_size(Vec2::new(w, 4.0), Sense::hover());
            ui.painter()
                .rect_filled(bar, CornerRadius::same(2), theme::fill());
            if total > 0 {
                let mut fillr = bar;
                fillr.set_width(w * done as f32 / total as f32);
                ui.painter()
                    .rect_filled(fillr, CornerRadius::same(2), theme::green());
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt(("runbook-steps", tab_id))
                .auto_shrink([false, false])
                .max_height(ui.available_height() - 44.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let values = run.log.values_map();
                    for i in 0..total {
                        let mut step = run.log.runbook.steps[i].clone();
                        step.text = runbook::render(&step.text, &values);
                        let current = run.capturing == Some(i);
                        let bg = if current {
                            theme::accent_soft()
                        } else {
                            theme::surface()
                        };
                        egui::Frame::NONE
                            .fill(bg)
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let st = &mut run.log.steps[i];
                                    let mut d = st.done;
                                    if ui
                                        .checkbox(&mut d, "")
                                        .on_hover_text(t!("Erledigt"))
                                        .changed()
                                    {
                                        st.done = d;
                                        if d {
                                            st.finished = Some(Local::now());
                                            st.started.get_or_insert(Local::now());
                                        } else {
                                            st.finished = None;
                                        }
                                    }
                                    ui.label(
                                        RichText::new(format!("{}.", i + 1))
                                            .font(theme::mono_medium(12.5))
                                            .color(theme::accent()),
                                    );
                                    let first = step.text.lines().next().unwrap_or_default();
                                    ui.add(
                                        egui::Label::new(rich(first, 13.5, true, st.done)).wrap(),
                                    );
                                });
                                let rest: Vec<&str> = step.text.lines().skip(1).collect();
                                if !rest.is_empty() {
                                    ui.add(
                                        egui::Label::new(rich(
                                            &rest.join("\n"),
                                            12.5,
                                            false,
                                            false,
                                        ))
                                        .wrap(),
                                    );
                                }
                                let cmd = runbook::render(&step.command, &values);
                                if !cmd.trim().is_empty() {
                                    egui::Frame::NONE
                                        .fill(theme::bg_deep())
                                        .corner_radius(CornerRadius::same(6))
                                        .inner_margin(egui::Margin::symmetric(8, 6))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.add(
                                                egui::Label::new(
                                                    theme::mono(cmd.trim_end(), 12.0)
                                                        .color(theme::text()),
                                                )
                                                .wrap(),
                                            );
                                        });
                                }
                                ui.horizontal(|ui| {
                                    if !cmd.trim().is_empty() {
                                        if run.armed == Some(i) {
                                            ui.label(
                                                RichText::new(t!("Wirklich ausführen?"))
                                                    .size(12.5)
                                                    .color(theme::amber()),
                                            );
                                            if theme::btn(
                                                ui,
                                                theme::Btn::SmallPrimary,
                                                None,
                                                t!("Ja"),
                                            )
                                            .clicked()
                                            {
                                                exec = Some(i);
                                            }
                                            if theme::btn(ui, theme::Btn::Small, None, t!("Nein"))
                                                .clicked()
                                            {
                                                run.armed = None;
                                            }
                                        } else {
                                            let label = if step.confirm {
                                                t!("Ausführen…")
                                            } else {
                                                t!("Ausführen")
                                            };
                                            let r = ui.add_enabled_ui(live, |ui| {
                                                theme::btn(
                                                    ui,
                                                    theme::Btn::SmallPrimary,
                                                    Some(Icon::Play),
                                                    label,
                                                )
                                            });
                                            if r.inner.clicked() {
                                                if step.confirm {
                                                    run.armed = Some(i);
                                                } else {
                                                    exec = Some(i);
                                                }
                                            }
                                            if step.confirm {
                                                theme::tag(
                                                    ui,
                                                    t!("Bestätigung"),
                                                    Some(theme::amber()),
                                                );
                                            }
                                        }
                                    }
                                    let st = &run.log.steps[i];
                                    if let Some(start) = st.started {
                                        let end = st.finished.unwrap_or(now);
                                        let d = (end - start).to_std().unwrap_or_default();
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}",
                                                        start.format("%H:%M:%S"),
                                                        crate::ui::short_duration(d)
                                                    ))
                                                    .size(11.5)
                                                    .color(theme::text_dim()),
                                                );
                                            },
                                        );
                                    }
                                });
                                let st = &mut run.log.steps[i];
                                ui.add(
                                    egui::TextEdit::singleline(&mut st.note)
                                        .hint_text(t!("Notiz"))
                                        .desired_width(f32::INFINITY),
                                );
                            });
                        ui.add_space(6.0);
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::btn(ui, theme::Btn::Primary, Some(Icon::File), t!("Protokoll"))
                    .on_hover_text(t!(
                        "Markdown-Bericht mit Zeiten, Notizen und Ausgaben speichern und kopieren"
                    ))
                    .clicked()
                {
                    report = true;
                }
                if let Some(p) = &run.report
                    && theme::btn(
                        ui,
                        theme::Btn::Link,
                        Some(Icon::Folder),
                        t!("Ordner öffnen"),
                    )
                    .on_hover_text(p.display().to_string())
                    .clicked()
                    && let Some(dir) = p.parent()
                    && let Err(e) = runbook::open_folder(dir)
                {
                    toasts.push((e.to_string(), theme::red()));
                }
            });
            if report {
                let md = runbook::report_markdown(&run.log, Local::now());
                ui.ctx().copy_text(md.clone());
                let name = format!(
                    "{}_{}_{}.md",
                    run.log.started.format("%Y-%m-%d_%H%M"),
                    runbook::safe_name(&run.log.runbook.title),
                    runbook::safe_name(&run.log.host)
                );
                let path = runbook::output_dir("Runbooks").join(name);
                match std::fs::write(&path, md) {
                    Ok(()) => {
                        toasts.push((
                            t!(
                                "Protokoll gespeichert: {} (in der Zwischenablage)",
                                path.display()
                            ),
                            theme::green(),
                        ));
                        run.report = Some(path);
                    }
                    Err(e) => toasts.push((
                        t!("Protokoll konnte nicht gespeichert werden: {}", e),
                        theme::red(),
                    )),
                }
            }
        });
    let mut danger = None;
    if let Some(i) = exec {
        run.armed = None;
        let cmd = run.log.command(i);
        let st = &mut run.log.steps[i];
        st.started.get_or_insert(Local::now());
        st.sent = Some(cmd.clone());
        run.capturing = Some(i);
        // Paste like a user would (bracketed paste aware), Enter separately.
        let mut bytes = t.term.paste_bytes(cmd.trim_end());
        let why = if guarded {
            cmd.lines().find_map(crate::model::dangerous_command)
        } else {
            None
        };
        match why {
            Some(why) => danger = Some((cmd.trim().to_string(), why)),
            None => bytes.push(b'\r'),
        }
        let _ = t.tx.send(TermCmd::Input(bytes));
        if danger.is_none() {
            t.cmd_started = Some(Instant::now());
        }
    }
    if close {
        ops.runbooks.runs.remove(&tab_id);
    }
    danger
}

/// Markdown-ish text: `**bold**` and `` `code` ``.
fn rich(text: &str, size: f32, strong: bool, done: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = if done {
        theme::text_dim()
    } else if strong {
        theme::text()
    } else {
        theme::text_dim()
    };
    let font = |bold: bool| {
        if bold || strong {
            theme::semibold(size)
        } else {
            FontId::proportional(size)
        }
    };
    let mut bold = false;
    let mut rest = text;
    while !rest.is_empty() {
        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        match (next_bold, next_code) {
            (Some(b), c) if c.is_none_or(|c| b < c) => {
                push(&mut job, &rest[..b], font(bold), base, done);
                bold = !bold;
                rest = &rest[b + 2..];
            }
            (_, Some(c)) => {
                push(&mut job, &rest[..c], font(bold), base, done);
                let after = &rest[c + 1..];
                let end = after.find('`').unwrap_or(after.len());
                let mut f = TextFormat::simple(FontId::monospace(size - 1.0), theme::accent());
                f.background = theme::fill();
                if done {
                    f.strikethrough = Stroke::new(1.0, theme::text_dim());
                }
                job.append(&after[..end], 0.0, f);
                rest = after.get(end + 1..).unwrap_or("");
            }
            _ => {
                push(&mut job, rest, font(bold), base, done);
                rest = "";
            }
        }
    }
    job
}

fn push(job: &mut LayoutJob, s: &str, font: FontId, color: Color32, done: bool) {
    if s.is_empty() {
        return;
    }
    let mut f = TextFormat::simple(font, color);
    if done {
        f.strikethrough = Stroke::new(1.0, theme::text_dim());
    }
    job.append(s, 0.0, f);
}
