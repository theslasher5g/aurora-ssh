//! "Run on many hosts": setup sheet (hosts, command or snippet, placeholders, options),
//! the confirmation dialog with the PROD / maintenance / dangerous-command gates, and the
//! result table with details, filter, export and "run again for the failed ones".
//! Also the single entry point of the server features (`SrvAction`).

use std::collections::HashSet;
use std::time::Duration;

use chrono::Local;
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::App;
use crate::backend::Sink;
use crate::backend::multirun_core::{
    self as core, Event, Handle, HostResult, Job, Options, Status, Transport,
};
use crate::backend::multirun_report::{self as report, Meta, StatusFilter};
use crate::backend::multirun_safety::{self as safety, CONFIRM_WORD, Gate};
use crate::backend::runbook::{open_folder, operator, output_dir};
use crate::model::{Env, Host, Protocol, ShellKind};
use crate::nav_query::Query;
use crate::theme;
use crate::ui::hoststats_ui::StatsState;
use crate::ui::icons::Icon;
use crate::ui::winrm_ui::{self, WinrmState};

// ------------------------------------------------------------------ entry

#[derive(Default)]
pub struct ServerUi {
    pub multi: MultiState,
    pub stats: StatsState,
    pub winrm: WinrmState,
}

#[derive(Clone, Debug)]
pub enum SrvAction {
    /// Run on these hosts (the text says where the selection came from).
    Run(Vec<Uuid>, String),
    /// Run on every host of a group and its subgroups.
    RunGroup(Uuid),
    /// Run on the hosts a search/filter finds.
    RunFilter(String),
    /// Run this snippet (opens the host selection with the snippet picked).
    RunSnippet(Uuid, Vec<Uuid>),
    /// Reopen the last result table.
    ShowResults,
    /// Live statistics sheet of a host.
    Stats(Uuid),
    /// Collect the facts of Windows hosts through WinRM (empty = all).
    WinrmRefresh(Vec<Uuid>),
}

pub fn apply(app: &mut App, a: SrvAction, ctx: &egui::Context) {
    match a {
        SrvAction::Run(ids, title) => open_setup(app, ids, title, None),
        SrvAction::RunGroup(g) => {
            let ids = app.lib.hosts_in_group(g).iter().map(|h| h.id).collect();
            let name = app.lib.group(g).map(|g| g.name.clone()).unwrap_or_default();
            open_setup(app, ids, t!("Gruppe „{}“", name), None);
        }
        SrvAction::RunFilter(text) => {
            let ids = search_hosts(app, &text);
            let title = if text.trim().is_empty() {
                t!("Alle Hosts").to_string()
            } else {
                t!("Suche „{}“", text.trim())
            };
            open_setup(app, ids, title, None);
            if let Some(s) = app.srv.multi.setup.as_mut() {
                s.query = text;
            }
        }
        SrvAction::RunSnippet(id, ids) => open_setup(app, ids, t!("Snippet").to_string(), Some(id)),
        SrvAction::ShowResults => {
            if app.srv.multi.run.is_some() {
                app.srv.multi.run_open = true;
            } else {
                app.toast(t!("Noch kein Mehrfachlauf"), theme::amber());
            }
        }
        SrvAction::Stats(id) => crate::ui::hoststats_ui::open(app, id),
        SrvAction::WinrmRefresh(ids) => winrm_ui::start(app, ctx, &ids),
    }
}

/// Once per frame.
pub fn show(app: &mut App, ctx: &egui::Context) {
    crate::ui::hoststats_ui::show(app, ctx);
    winrm_ui::pump(app);
    pump(app);
    if app.srv.multi.confirm.is_some() {
        confirm_dialog(app, ctx);
    } else if app.srv.multi.setup.is_some() {
        setup_sheet(app, ctx);
    } else if app.srv.multi.run_open && app.srv.multi.run.is_some() {
        results_sheet(app, ctx);
    }
}

/// Hosts a search finds (protocols with a command channel only).
pub fn search_hosts(app: &App, text: &str) -> Vec<Uuid> {
    let q = Query::parse(text);
    let Some(c) = app.nav.cache.as_ref() else {
        return Vec::new();
    };
    let lib = &app.lib;
    (0..c.idx.entries.len())
        .filter(|&e| {
            let en = &c.idx.entries[e];
            (q.conds.is_empty() || q.matches(&c.idx.facets(lib, e)))
                && (q.words.is_empty() || q.score(&en.hay).is_some())
        })
        .map(|e| &lib.hosts[c.idx.entries[e].host])
        .filter(|h| matches!(h.protocol, Protocol::Ssh | Protocol::Rdp))
        .map(|h| h.id)
        .collect()
}

// ------------------------------------------------------------------ state

#[derive(Default)]
pub struct MultiState {
    setup: Option<Setup>,
    confirm: Option<Confirm>,
    run: Option<RunUi>,
    run_open: bool,
    io: Option<(Sink<Event>, UnboundedReceiver<Event>)>,
}

struct Setup {
    source: String,
    pool: Vec<Uuid>,
    chosen: HashSet<Uuid>,
    query: String,
    snippet_mode: bool,
    powershell: bool,
    command: String,
    snippet: Option<Uuid>,
    values: Vec<(String, String)>,
    sudo: bool,
    concurrency: usize,
    timeout: u64,
    error: Option<String>,
}

#[derive(Clone)]
struct Row {
    id: Uuid,
    name: String,
    address: String,
    env: Env,
    maint: Option<String>,
    skip: Option<String>,
    via: &'static str,
}

struct Confirm {
    real: String,
    shown: String,
    powershell: bool,
    sudo: bool,
    concurrency: usize,
    timeout: u64,
    secrets: Vec<String>,
    rows: Vec<Row>,
    dangerous: Option<&'static str>,
    ack_prod: bool,
    ack_maint: bool,
    typed: String,
}

struct RunUi {
    rows: Vec<Row>,
    opts: Options,
    powershell: bool,
    shown: String,
    started: chrono::DateTime<Local>,
    results: Vec<HostResult>,
    handle: Option<Handle>,
    running: bool,
    ack_maint: bool,
    filter: StatusFilter,
    text: String,
    selected: Option<usize>,
}

fn open_setup(app: &mut App, ids: Vec<Uuid>, source: String, snippet: Option<Uuid>) {
    let mut pool: Vec<Uuid> = ids
        .into_iter()
        .filter(|id| {
            app.lib
                .host(*id)
                .is_some_and(|h| matches!(h.protocol, Protocol::Ssh | Protocol::Rdp))
        })
        .collect();
    pool.dedup();
    let chosen = pool.iter().copied().collect();
    let ps = snippet
        .and_then(|s| app.lib.snippet(s))
        .is_some_and(|s| s.shell == ShellKind::PowerShell);
    app.srv.multi.setup = Some(Setup {
        source,
        pool,
        chosen,
        query: String::new(),
        snippet_mode: snippet.is_some(),
        powershell: ps,
        command: String::new(),
        snippet,
        values: Vec::new(),
        sudo: false,
        concurrency: core::DEFAULT_CONCURRENCY,
        timeout: core::DEFAULT_TIMEOUT_SECS,
        error: None,
    });
    app.srv.multi.confirm = None;
}

// ------------------------------------------------------------------ planning

/// How a host would be reached for this command, or why not.
fn route(app: &App, h: &Host, powershell: bool) -> Result<(Transport, bool), String> {
    match h.protocol {
        Protocol::Ssh => {
            let windows = app
                .lib
                .facts
                .get(&h.id)
                .is_some_and(|f| f.os_id == "windows");
            if powershell && !windows {
                return Err(t!(
                    "PowerShell-Befehl, aber der Host ist nicht als Windows bekannt (Fakten aktualisieren)"
                )
                .to_string());
            }
            if !powershell && windows {
                return Err(t!("Windows-Host: den Befehl als PowerShell ausführen").to_string());
            }
            crate::ui::containers_page::background_params(app, h)
                .map(|p| (Transport::Ssh(Box::new(p)), windows))
                .map_err(|s| s.text().to_string())
        }
        Protocol::Rdp => {
            if !winrm_ui::usable(h) {
                return Err(t!("Kein Befehlskanal: SSH oder WinRM im Host einrichten").to_string());
            }
            if !powershell {
                return Err(
                    t!("WinRM führt PowerShell aus – die Shell auf PowerShell stellen").to_string(),
                );
            }
            winrm_ui::target_for(app, h, true).map(|t| (Transport::Winrm(Box::new(t)), false))
        }
        _ => Err(t!("Kein Befehlskanal (SSH oder WinRM)").to_string()),
    }
}

fn plan_rows(app: &App, ids: &[Uuid], powershell: bool) -> Vec<Row> {
    let now = chrono::Utc::now();
    ids.iter()
        .filter_map(|id| app.lib.host(*id))
        .map(|h| {
            let r = route(app, h, powershell);
            Row {
                id: h.id,
                name: h.display_name().to_string(),
                address: h.hostname.trim().to_string(),
                env: app.lib.env_of(h),
                maint: crate::ui::maintenance::active_for(&app.lib, h.id, now).map(|m| m.banner()),
                via: match &r {
                    Ok((Transport::Winrm(_), _)) => "WinRM",
                    Ok(_) => "SSH",
                    Err(_) => "",
                },
                skip: r.err(),
            }
        })
        .collect()
}

fn make_job(app: &App, row: &Row, powershell: bool, ack_maint: bool) -> Job {
    let mut job = Job {
        name: row.name.clone(),
        address: row.address.clone(),
        env: if row.env == Env::None {
            String::new()
        } else {
            row.env.label().to_string()
        },
        transport: Transport::None,
        skip: None,
        ssh_powershell: false,
    };
    if let Some(m) = row.maint.as_ref().filter(|_| !ack_maint) {
        job.skip = Some(t!("im Wartungsfenster ({}) – übersprungen", m));
        return job;
    }
    let Some(h) = app.lib.host(row.id) else {
        job.skip = Some(t!("Host nicht mehr vorhanden").to_string());
        return job;
    };
    match route(app, h, powershell) {
        Ok((tr, ssh_ps)) => {
            job.transport = tr;
            job.ssh_powershell = ssh_ps;
        }
        Err(e) => job.skip = Some(e),
    }
    job
}

fn start_run(app: &mut App, ctx: &egui::Context, c: Confirm) {
    let opts = Options {
        command: c.real.clone(),
        sudo: c.sudo && !c.powershell,
        concurrency: c.concurrency,
        timeout: Duration::from_secs(c.timeout),
        secrets: c.secrets.clone(),
    };
    let jobs: Vec<(usize, Job)> = c
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i, make_job(app, r, c.powershell, c.ack_maint)))
        .collect();
    let results = jobs.iter().map(|(_, j)| HostResult::pending(j)).collect();
    let sink = app
        .srv
        .multi
        .io
        .get_or_insert_with(|| Sink::new(ctx.clone()))
        .0
        .clone();
    let handle = core::spawn(&app.rt, jobs, opts.clone(), sink, app.prompts_sink.clone());
    app.srv.multi.run = Some(RunUi {
        rows: c.rows,
        opts,
        powershell: c.powershell,
        shown: c.shown,
        started: Local::now(),
        results,
        handle: Some(handle),
        running: true,
        ack_maint: c.ack_maint,
        filter: StatusFilter::All,
        text: String::new(),
        selected: None,
    });
    app.srv.multi.run_open = true;
}

fn rerun_failed(app: &mut App, ctx: &egui::Context) {
    let Some(run) = app.srv.multi.run.as_ref() else {
        return;
    };
    let idx: Vec<usize> = run
        .results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status.retryable())
        .map(|(i, _)| i)
        .collect();
    if idx.is_empty() || run.running {
        return;
    }
    let jobs: Vec<(usize, Job)> = idx
        .iter()
        .map(|&i| {
            (
                i,
                make_job(app, &run.rows[i], run.powershell, run.ack_maint),
            )
        })
        .collect();
    let opts = run.opts.clone();
    let sink = app
        .srv
        .multi
        .io
        .get_or_insert_with(|| Sink::new(ctx.clone()))
        .0
        .clone();
    let pending: Vec<(usize, HostResult)> = jobs
        .iter()
        .map(|(i, j)| (*i, HostResult::pending(j)))
        .collect();
    let handle = core::spawn(&app.rt, jobs, opts, sink, app.prompts_sink.clone());
    if let Some(run) = app.srv.multi.run.as_mut() {
        for (i, r) in pending {
            run.results[i] = r;
        }
        run.handle = Some(handle);
        run.running = true;
        run.selected = None;
    }
}

fn pump(app: &mut App) {
    let mut events = Vec::new();
    if let Some((_, rx)) = app.srv.multi.io.as_mut() {
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    let mut finished = false;
    if let Some(run) = app.srv.multi.run.as_mut() {
        for e in events {
            match e {
                Event::Started(i) => {
                    if let Some(r) = run.results.get_mut(i) {
                        r.status = Status::Running;
                    }
                }
                Event::Finished(i, r) => {
                    if let Some(slot) = run.results.get_mut(i) {
                        *slot = *r;
                    }
                }
                Event::Done => {
                    run.running = false;
                    finished = true;
                }
            }
        }
    }
    if finished && let Some(run) = app.srv.multi.run.as_ref() {
        let c = report::counts(&run.results);
        let text = t!(
            "Mehrfachlauf fertig: {} ok, {} Probleme, {} übersprungen",
            c.ok,
            c.failed + c.timeout + c.unreachable,
            c.skipped
        );
        let bad = c.failed + c.timeout + c.unreachable > 0;
        let open = app.srv.multi.run_open;
        app.toast(text, if bad { theme::amber() } else { theme::green() });
        if !open {
            app.toast(
                t!("Ergebnis: Befehlspalette → „Letzten Mehrfachlauf anzeigen“"),
                theme::accent(),
            );
        }
    }
}

// ------------------------------------------------------------------ setup sheet

fn sync_values(values: &mut Vec<(String, String)>, vars: Vec<String>) {
    values.retain(|(k, _)| vars.contains(k));
    for v in vars {
        if !values.iter().any(|(k, _)| *k == v) {
            values.push((v, String::new()));
        }
    }
}

fn template(app: &App, s: &Setup) -> String {
    if s.snippet_mode {
        s.snippet
            .and_then(|id| app.lib.snippet(id))
            .map(|x| x.script.clone())
            .unwrap_or_default()
    } else {
        s.command.clone()
    }
}

fn env_tag(ui: &mut egui::Ui, env: Env) {
    if env != Env::None {
        theme::pill(ui, env.label(), theme::env_color(env));
    }
}

fn setup_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(mut s) = app.srv.multi.setup.take() else {
        return;
    };
    let tpl = template(app, &s);
    sync_values(&mut s.values, crate::backend::runbook::variables_in(&tpl));
    // Pool entries with names, for the list (cheap: only what is drawn).
    let mut new_pool: Option<Vec<Uuid>> = None;
    let mut next = false;
    let snippets: Vec<(Uuid, String, ShellKind)> = app
        .lib
        .snippets
        .iter()
        .map(|x| (x.id, x.name.clone(), x.shell))
        .collect();
    let now = chrono::Utc::now();
    let (_, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("multirun-setup"),
        720.0,
        t!("Auf mehreren Hosts ausführen"),
        |ui| {
            // --- hosts
            theme::form_caption(ui, &t!("Hosts · {}", s.source));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t!("{} von {} gewählt", s.chosen.len(), s.pool.len()))
                        .color(theme::text_dim()),
                );
                if theme::btn(ui, theme::Btn::Small, None, t!("Alle")).clicked() {
                    s.chosen = s.pool.iter().copied().collect();
                }
                if theme::btn(ui, theme::Btn::Small, None, t!("Keine")).clicked() {
                    s.chosen.clear();
                }
            });
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 170.0).max(120.0);
                theme::search_field(
                    ui,
                    &mut s.query,
                    t!("Hosts suchen: env:prod tag:web type:ssh …"),
                    w,
                    egui::Id::new("multirun-query"),
                );
                if theme::btn(ui, theme::Btn::Small, None, t!("Treffer übernehmen"))
                    .on_hover_text(t!(
                        "Ersetzt die Liste durch die Hosts, die die Suche findet"
                    ))
                    .clicked()
                {
                    new_pool = Some(search_hosts(app, &s.query));
                }
            });
            let row_h = 28.0;
            egui::ScrollArea::vertical()
                .id_salt("multirun-pool")
                .max_height(row_h * 6.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_h, s.pool.len(), |ui, range| {
                    for i in range {
                        let id = s.pool[i];
                        let Some(h) = app.lib.host(id) else { continue };
                        ui.horizontal(|ui| {
                            ui.set_height(row_h);
                            let mut on = s.chosen.contains(&id);
                            if ui.checkbox(&mut on, "").changed() {
                                if on {
                                    s.chosen.insert(id);
                                } else {
                                    s.chosen.remove(&id);
                                }
                            }
                            ui.add(
                                egui::Label::new(
                                    RichText::new(h.display_name()).font(theme::semibold(13.5)),
                                )
                                .truncate(),
                            );
                            env_tag(ui, app.lib.env_of(h));
                            if crate::ui::maintenance::active_for(&app.lib, id, now).is_some() {
                                theme::pill(ui, t!("Wartung"), theme::amber());
                            }
                            ui.label(
                                RichText::new(h.hostname.trim())
                                    .size(12.0)
                                    .color(theme::text_dim()),
                            );
                        });
                    }
                });
            if s.pool.is_empty() {
                ui.label(
                    RichText::new(t!("Keine Hosts – suche oben und übernimm die Treffer."))
                        .color(theme::text_dim()),
                );
            }
            // --- command
            theme::form_caption(ui, t!("Befehl"));
            ui.horizontal(|ui| {
                if let Some(i) = theme::segmented(
                    ui,
                    &[t!("Eigener Befehl"), t!("Snippet")],
                    usize::from(s.snippet_mode),
                ) {
                    s.snippet_mode = i == 1;
                }
            });
            if s.snippet_mode {
                let cur = s
                    .snippet
                    .and_then(|id| snippets.iter().find(|x| x.0 == id))
                    .map_or(t!("Snippet wählen…").to_string(), |x| x.1.clone());
                egui::ComboBox::from_id_salt("multirun-snippet")
                    .selected_text(cur)
                    .width(ui.available_width().min(420.0))
                    .show_ui(ui, |ui| {
                        for (id, name, shell) in &snippets {
                            if ui.selectable_label(s.snippet == Some(*id), name).clicked() {
                                s.snippet = Some(*id);
                                s.powershell = *shell == ShellKind::PowerShell;
                            }
                        }
                    });
                if let Some(x) = s.snippet.and_then(|id| app.lib.snippet(id)) {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&x.script)
                                .font(egui::FontId::monospace(12.5))
                                .color(theme::text_dim()),
                        )
                        .truncate(),
                    );
                }
            } else {
                ui.add(
                    egui::TextEdit::multiline(&mut s.command)
                        .font(egui::FontId::monospace(13.0))
                        .hint_text(t!("z. B. uptime  ·  systemctl status {{dienst}}"))
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t!("Shell:")).color(theme::text_dim()));
                    if let Some(i) =
                        theme::segmented(ui, &["POSIX sh", "PowerShell"], usize::from(s.powershell))
                    {
                        s.powershell = i == 1;
                    }
                });
            }
            if !s.values.is_empty() {
                theme::form_caption(ui, t!("Platzhalter"));
                theme::form(ui, 140.0, |ui, f| {
                    for (name, value) in s.values.iter_mut() {
                        let secret = safety::is_secret_name(name);
                        f.field(ui, name, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .password(secret)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    }
                });
            }
            theme::form_caption(ui, t!("Optionen"));
            theme::form(ui, 140.0, |ui, f| {
                if !s.powershell {
                    f.switch(ui, t!("Mit sudo (ohne Passwort)"), &mut s.sudo);
                }
                f.field(ui, t!("Parallel"), |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.concurrency).range(1..=core::MAX_CONCURRENCY),
                    );
                    ui.label(
                        RichText::new(t!("Hosts gleichzeitig"))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                });
                f.field(ui, t!("Zeitlimit"), |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.timeout)
                            .range(5..=3600)
                            .suffix(" s"),
                    );
                    ui.label(
                        RichText::new(t!("je Host"))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                });
            });
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(theme::red()));
            }
            !s.chosen.is_empty()
                && if s.snippet_mode {
                    s.snippet.is_some()
                } else {
                    !s.command.trim().is_empty()
                }
        },
        |ui, ok| {
            ui.add_enabled_ui(ok, |ui| {
                if theme::btn(ui, theme::Btn::Primary, Some(Icon::Play), t!("Weiter…")).clicked()
                {
                    next = true;
                }
            });
        },
    );
    if let Some(p) = new_pool {
        s.chosen = p.iter().copied().collect();
        s.pool = p;
    }
    if dismissed {
        return;
    }
    if next {
        let tpl = template(app, &s);
        match safety::render_command(&tpl, &s.values) {
            Err(e) => s.error = Some(format!("{e:#}")),
            Ok((real, shown)) => {
                if real.contains("{{") && real.contains("}}") {
                    s.error = Some(t!("Nicht alle Platzhalter sind ausgefüllt").to_string());
                } else if s.snippet_mode && s.snippet.is_none() {
                    s.error = Some(t!("Wähle ein Snippet").to_string());
                } else {
                    let ids: Vec<Uuid> = s
                        .pool
                        .iter()
                        .copied()
                        .filter(|i| s.chosen.contains(i))
                        .collect();
                    let rows = plan_rows(app, &ids, s.powershell);
                    let dangerous = real
                        .lines()
                        .find_map(|l| safety::dangerous(l, s.powershell))
                        .or_else(|| safety::dangerous(&real, s.powershell));
                    app.srv.multi.confirm = Some(Confirm {
                        secrets: safety::secret_values(&s.values),
                        real,
                        shown,
                        powershell: s.powershell,
                        sudo: s.sudo,
                        concurrency: s.concurrency,
                        timeout: s.timeout,
                        rows,
                        dangerous,
                        ack_prod: false,
                        ack_maint: false,
                        typed: String::new(),
                    });
                    s.error = None;
                }
            }
        }
    }
    app.srv.multi.setup = Some(s);
}

// ------------------------------------------------------------------ confirmation

fn confirm_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut c) = app.srv.multi.confirm.take() else {
        return;
    };
    let runnable = |r: &Row, ack: bool| r.skip.is_none() && (r.maint.is_none() || ack);
    let n_run = c.rows.iter().filter(|r| runnable(r, c.ack_maint)).count();
    let prod: Vec<&Row> = c
        .rows
        .iter()
        .filter(|r| r.env == Env::Prod && runnable(r, c.ack_maint))
        .collect();
    let maint: Vec<&Row> = c
        .rows
        .iter()
        .filter(|r| r.maint.is_some() && r.skip.is_none())
        .collect();
    let skipped: Vec<&Row> = c.rows.iter().filter(|r| r.skip.is_some()).collect();
    let gate = safety::gate(prod.len(), c.dangerous);
    let unlocked = n_run > 0
        && match gate {
            Gate::Plain => true,
            Gate::Prod => c.ack_prod,
            Gate::TypeToConfirm => c.typed.trim() == CONFIRM_WORD,
        };
    let prod_n = prod.len();
    let (mut go, mut back) = (false, false);
    egui::Modal::new(egui::Id::new("multirun-confirm"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(560.0f32.min(ctx.content_rect().width() - 80.0));
            ui.label(theme::heading(
                t!("Befehl auf {} Hosts ausführen?", n_run),
                20.0,
            ));
            ui.add_space(6.0);
            egui::Frame::NONE
                .fill(theme::fill())
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&c.shown)
                                        .font(egui::FontId::monospace(13.0))
                                        .color(theme::text()),
                                )
                                .wrap(),
                            );
                        });
                });
            let (n_ssh, n_winrm) = (
                c.rows
                    .iter()
                    .filter(|r| runnable(r, c.ack_maint) && r.via == "SSH")
                    .count(),
                c.rows
                    .iter()
                    .filter(|r| runnable(r, c.ack_maint) && r.via == "WinRM")
                    .count(),
            );
            ui.label(
                RichText::new(format!(
                    "SSH {n_ssh} · WinRM {n_winrm} · {} · {}{}",
                    if c.powershell {
                        "PowerShell"
                    } else {
                        "POSIX sh"
                    },
                    t!("{} parallel, {} s Zeitlimit", c.concurrency, c.timeout),
                    if c.sudo && !c.powershell {
                        " · sudo -n"
                    } else {
                        ""
                    }
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            ui.add_space(6.0);
            if !skipped.is_empty() {
                egui::CollapsingHeader::new(
                    RichText::new(t!("{} Hosts werden übersprungen", skipped.len()))
                        .color(theme::amber()),
                )
                .id_salt("mr-skipped")
                .show(ui, |ui| {
                    for r in skipped.iter().take(40) {
                        ui.label(
                            RichText::new(format!(
                                "{} – {}",
                                r.name,
                                r.skip.clone().unwrap_or_default()
                            ))
                            .size(12.0)
                            .color(theme::text_dim()),
                        );
                    }
                    if skipped.len() > 40 {
                        ui.label("…");
                    }
                });
            }
            if !maint.is_empty() {
                ui.add_space(4.0);
                ui.checkbox(
                    &mut c.ack_maint,
                    RichText::new(t!(
                        "Auch {} Hosts im Wartungsfenster einbeziehen (sonst übersprungen)",
                        maint.len()
                    ))
                    .color(theme::amber()),
                );
            }
            if let Some(why) = c.dangerous {
                ui.add_space(6.0);
                banner(ui, theme::red(), &t!("Gefährlicher Befehl: {}", why));
            }
            if prod_n > 0 {
                ui.add_space(6.0);
                let names: Vec<&str> = prod.iter().take(6).map(|r| r.name.as_str()).collect();
                banner(
                    ui,
                    theme::env_color(Env::Prod),
                    &t!(
                        "{} PROD-Hosts dabei: {}{}",
                        prod_n,
                        names.join(", "),
                        if prod_n > 6 { ", …" } else { "" }
                    ),
                );
                match gate {
                    Gate::TypeToConfirm => {
                        ui.label(
                            RichText::new(t!(
                                "Gefährlicher Befehl auf PROD: tippe {} zur Bestätigung.",
                                CONFIRM_WORD
                            ))
                            .color(theme::red()),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut c.typed)
                                .hint_text(CONFIRM_WORD)
                                .desired_width(160.0),
                        );
                    }
                    _ => {
                        ui.checkbox(&mut c.ack_prod, t!("Ja, auch auf den PROD-Hosts ausführen"));
                    }
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(unlocked, |ui| {
                    if theme::btn(
                        ui,
                        theme::Btn::Primary,
                        Some(Icon::Play),
                        &t!("Auf {} Hosts ausführen", n_run),
                    )
                    .clicked()
                    {
                        go = true;
                    }
                });
                if theme::btn(ui, theme::Btn::Normal, None, t!("Zurück")).clicked() {
                    back = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        back = true;
    }
    if go {
        app.srv.multi.setup = None;
        start_run(app, ctx, c);
    } else if back {
        // The setup is still there.
    } else {
        app.srv.multi.confirm = Some(c);
    }
}

fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::NONE
        .fill(theme::soft(color))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(RichText::new(text).font(theme::semibold(13.0)).color(color))
                    .wrap(),
            );
        });
}

// ------------------------------------------------------------------ results

fn status_color(s: Status) -> Color32 {
    match s {
        Status::Ok => theme::green(),
        Status::Failed => theme::red(),
        Status::Timeout | Status::Unreachable => theme::amber(),
        Status::Running => theme::accent(),
        Status::Pending | Status::Skipped => theme::text_dim(),
    }
}

/// Rounded label painted at `left_center`; returns its width.
fn paint_pill(p: &egui::Painter, left_center: Pos2, text: &str, color: Color32) -> f32 {
    let g = p.layout_no_wrap(text.to_string(), theme::medium(12.0), color);
    let w = g.size().x + 16.0;
    let r = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - 10.0),
        Vec2::new(w, 20.0),
    );
    p.rect_filled(r, CornerRadius::same(10), theme::soft(color));
    p.galley(
        Pos2::new(r.left() + 8.0, left_center.y - g.size().y / 2.0),
        g,
        color,
    );
    w
}

fn meta_of(run: &RunUi) -> Meta {
    Meta {
        command: run.shown.clone(),
        started: run.started.format("%Y-%m-%d %H:%M").to_string(),
        operator: operator(),
        sudo: run.opts.sudo,
        powershell: run.powershell,
    }
}

fn export(run: &RunUi, kind: &str) -> Result<std::path::PathBuf, String> {
    let meta = meta_of(run);
    let (ext, text) = match kind {
        "md" => ("md", report::markdown(&meta, &run.results)),
        "csv" => ("csv", report::csv(&run.results)),
        "json" => ("json", report::json(&meta, &run.results)),
        _ => ("html", report::html(&meta, &run.results)),
    };
    let dir = output_dir("Mehrfachlauf");
    let path = dir.join(format!(
        "mehrfachlauf_{}.{ext}",
        run.started.format("%Y-%m-%d_%H%M%S")
    ));
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

fn results_sheet(app: &mut App, ctx: &egui::Context) {
    let Some(mut run) = app.srv.multi.run.take() else {
        return;
    };
    let counts = report::counts(&run.results);
    let running = run.running;
    let shown = report::filter(&run.results, &run.filter, &run.text);
    let (mut close, mut cancel, mut again, mut exp, mut copy_md, mut folder) =
        (false, false, false, None::<&'static str>, false, false);
    let narrow = ctx.content_rect().width() < 860.0;
    let (_, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("multirun-results"),
        1000.0,
        t!("Mehrfachlauf"),
        |ui| {
            // Summary and progress.
            let done = counts.total - counts.pending;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let chips: [(StatusFilter, String, Color32); 5] = [
                    (
                        StatusFilter::All,
                        t!("Alle {}", counts.total),
                        theme::text_dim(),
                    ),
                    (
                        StatusFilter::Only(Status::Ok),
                        t!("ok {}", counts.ok),
                        theme::green(),
                    ),
                    (
                        StatusFilter::Problems,
                        t!(
                            "Probleme {}",
                            counts.failed + counts.timeout + counts.unreachable
                        ),
                        theme::red(),
                    ),
                    (
                        StatusFilter::Only(Status::Skipped),
                        t!("übersprungen {}", counts.skipped),
                        theme::text_dim(),
                    ),
                    (
                        StatusFilter::Only(Status::Running),
                        t!("läuft {}", counts.pending),
                        theme::accent(),
                    ),
                ];
                for (f, label, color) in chips {
                    let on = run.filter == f;
                    let text = RichText::new(label)
                        .font(theme::semibold(12.5))
                        .color(if on { theme::on_accent() } else { color });
                    let b = egui::Button::new(text)
                        .fill(if on { color } else { theme::soft(color) })
                        .corner_radius(CornerRadius::same(10));
                    if ui.add(b).clicked() {
                        run.filter = f;
                    }
                }
            });
            if run.running {
                ui.add(
                    egui::ProgressBar::new(done as f32 / counts.total.max(1) as f32)
                        .text(t!("{} von {}", done, counts.total))
                        .desired_height(14.0),
                );
            }
            theme::search_field(
                ui,
                &mut run.text,
                t!("Ergebnisse durchsuchen (Host, Status, Ausgabe)"),
                ui.available_width().min(460.0),
                egui::Id::new("multirun-search"),
            );
            ui.add_space(4.0);
            // Table.
            let row_h = 34.0;
            let w = ui.available_width();
            let (hdr, _) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::hover());
            let cols = columns(w, narrow);
            for (x, label) in [
                (cols.host, t!("Host")),
                (cols.status, t!("Status")),
                (cols.exit, "Exit"),
                (cols.time, t!("Dauer")),
                (cols.out, t!("Ausgabe")),
            ] {
                if x >= 0.0 {
                    ui.painter().text(
                        Pos2::new(hdr.left() + x, hdr.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        theme::semibold(12.0),
                        theme::text_dim(),
                    );
                }
            }
            egui::ScrollArea::vertical()
                .id_salt("multirun-table")
                .max_height(row_h * 8.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_h, shown.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for k in range {
                        let i = shown[k];
                        let r = &run.results[i];
                        let (rect, resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), row_h),
                            Sense::click(),
                        );
                        let p = ui.painter_at(rect);
                        if run.selected == Some(i) {
                            p.rect_filled(rect, CornerRadius::same(8), theme::accent_soft());
                        } else if resp.hovered() {
                            p.rect_filled(rect, CornerRadius::same(8), theme::hover());
                        }
                        let cy = rect.center().y;
                        let host_w = if cols.status > 0.0 {
                            cols.status - cols.host - 8.0
                        } else {
                            120.0
                        };
                        let g = theme::truncated(
                            &p,
                            &r.name,
                            theme::semibold(13.5),
                            theme::text(),
                            host_w.max(60.0),
                        );
                        p.galley(
                            Pos2::new(rect.left() + cols.host, cy - g.size().y / 2.0),
                            g,
                            theme::text(),
                        );
                        let mut sx = rect.left() + cols.status;
                        sx += paint_pill(
                            &p,
                            Pos2::new(sx, cy),
                            r.status.label(),
                            status_color(r.status),
                        );
                        if r.env == "PROD" {
                            paint_pill(
                                &p,
                                Pos2::new(sx + 4.0, cy),
                                "PROD",
                                theme::env_color(Env::Prod),
                            );
                        }
                        p.text(
                            Pos2::new(rect.left() + cols.exit, cy),
                            egui::Align2::LEFT_CENTER,
                            r.exit.map_or(String::new(), |e| e.to_string()),
                            theme::medium(13.0),
                            theme::text(),
                        );
                        if r.status.finished() && !r.took.is_zero() {
                            p.text(
                                Pos2::new(rect.left() + cols.time, cy),
                                egui::Align2::LEFT_CENTER,
                                format!("{:.1} s", r.took.as_secs_f32()),
                                theme::medium(13.0),
                                theme::text_dim(),
                            );
                        }
                        if cols.out >= 0.0 {
                            let max_w = (rect.width() - cols.out - 8.0).max(40.0);
                            let g = theme::truncated(
                                &p,
                                report::preview(r, 2, 200),
                                egui::FontId::monospace(12.0),
                                theme::text_dim(),
                                max_w,
                            );
                            p.galley(
                                Pos2::new(rect.left() + cols.out, cy - g.size().y / 2.0),
                                g,
                                theme::text_dim(),
                            );
                        }
                        if resp.clicked() {
                            run.selected = if run.selected == Some(i) {
                                None
                            } else {
                                Some(i)
                            };
                        }
                    }
                });
            if shown.is_empty() {
                ui.label(
                    RichText::new(t!("Keine Ergebnisse für diesen Filter"))
                        .color(theme::text_dim()),
                );
            }
            // Details of the selected row.
            if let Some(r) = run.selected.and_then(|i| run.results.get(i)) {
                ui.add_space(8.0);
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} ({})", r.name, r.address))
                                .font(theme::semibold(14.0)),
                        );
                        ui.label(
                            RichText::new(r.status.label())
                                .color(status_color(r.status))
                                .font(theme::semibold(13.0)),
                        );
                        if let Some(e) = r.exit {
                            ui.label(RichText::new(format!("exit {e}")).color(theme::text_dim()));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_btn(ui, Icon::Copy, t!("Ausgabe kopieren")).clicked() {
                                ui.ctx().copy_text(r.output.clone());
                            }
                        });
                    });
                    if !r.error.is_empty() {
                        ui.label(RichText::new(&r.error).color(theme::amber()).size(12.5));
                    }
                    let mut text: &str = &r.output;
                    egui::ScrollArea::vertical()
                        .id_salt("multirun-detail")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .font(egui::FontId::monospace(12.5))
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(4),
                            );
                        });
                    if r.truncated {
                        ui.label(
                            RichText::new(t!("Ausgabe gekürzt"))
                                .size(12.0)
                                .color(theme::text_dim()),
                        );
                    }
                });
            }
        },
        |ui, ()| {
            if theme::btn(ui, theme::Btn::Normal, None, t!("Schließen")).clicked() {
                close = true;
            }
            let ex = theme::btn(
                ui,
                theme::Btn::Normal,
                Some(Icon::Download),
                t!("Exportieren"),
            );
            egui::Popup::menu(&ex).show(|ui| {
                for (k, label) in [
                    ("md", "Markdown (.md)"),
                    ("csv", "CSV (.csv)"),
                    ("json", "JSON (.json)"),
                    ("html", t!("HTML zum Drucken (.html)")),
                ] {
                    if ui.button(label).clicked() {
                        exp = Some(k);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(t!("Als Markdown kopieren")).clicked() {
                    copy_md = true;
                    ui.close();
                }
                if ui.button(t!("Ordner öffnen")).clicked() {
                    folder = true;
                    ui.close();
                }
            });
            let retry = counts.failed + counts.timeout + counts.unreachable;
            ui.add_enabled_ui(!running && retry > 0, |ui| {
                if theme::btn(
                    ui,
                    theme::Btn::Normal,
                    Some(Icon::Refresh),
                    &t!("Fehlgeschlagene wiederholen ({})", retry),
                )
                .clicked()
                {
                    again = true;
                }
            });
            if running
                && theme::btn(ui, theme::Btn::Normal, Some(Icon::Stop), t!("Abbrechen")).clicked()
            {
                cancel = true;
            }
        },
    );
    if cancel && let Some(h) = &run.handle {
        h.cancel();
    }
    let mut toast: Option<(String, Color32)> = None;
    if let Some(kind) = exp {
        match export(&run, kind) {
            Ok(p) => toast = Some((t!("Exportiert nach {}", p.display()), theme::green())),
            Err(e) => toast = Some((t!("Export fehlgeschlagen: {}", e), theme::red())),
        }
    }
    if copy_md {
        ctx.copy_text(report::markdown(&meta_of(&run), &run.results));
        toast = Some((
            t!("In die Zwischenablage kopiert").to_string(),
            theme::green(),
        ));
    }
    if folder {
        let _ = open_folder(&output_dir("Mehrfachlauf"));
    }
    app.srv.multi.run = Some(run);
    if let Some((t, c)) = toast {
        app.toast(t, c);
    }
    if close || dismissed {
        app.srv.multi.run_open = false;
    }
    if again {
        rerun_failed(app, ctx);
    }
}

/// Column x offsets (relative to the table's left edge); `-1` = column hidden.
struct Cols {
    host: f32,
    status: f32,
    exit: f32,
    time: f32,
    out: f32,
}

fn columns(width: f32, narrow: bool) -> Cols {
    let host = 10.0;
    if narrow || width < 700.0 {
        Cols {
            host,
            status: (width * 0.42).max(150.0),
            exit: (width * 0.42).max(150.0) + 150.0,
            time: width - 70.0,
            out: -1.0,
        }
    } else {
        Cols {
            host,
            status: 230.0,
            exit: 400.0,
            time: 450.0,
            out: 525.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_the_template() {
        let mut v = vec![("a".to_string(), "1".to_string())];
        sync_values(&mut v, vec!["a".into(), "b".into()]);
        assert_eq!(
            v,
            vec![("a".into(), "1".into()), ("b".into(), String::new())]
        );
        sync_values(&mut v, vec!["b".into()]);
        assert_eq!(v, vec![("b".into(), String::new())]);
    }

    #[test]
    fn columns_hide_the_output_when_narrow() {
        assert!(columns(900.0, false).out > 0.0);
        assert!(columns(900.0, true).out < 0.0);
        assert!(columns(500.0, false).out < 0.0);
    }
}
