//! "Container" page: Docker/Podman containers and Kubernetes pods of this PC and of every
//! SSH host that has them, with live CPU/RAM, row actions and compose groups.
//!
//! Polling runs only while the page is visible (one SSH exec per host and round). Names
//! are validated by `backend::containers_parse` before they reach a command line, and the
//! page never opens a connection that would need a password prompt or an unknown host key.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use egui::{CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::backend::Sink;
use crate::backend::container::Target;
use crate::backend::containers_parse::{self as cp, DockerContainer, Pod, Severity};
use crate::backend::containers_run::{
    self as run, CtrEvent, CtrRx, DockerRound, HostKey, KubeRound, KubeSpec, RoundSpec,
};
use crate::backend::ssh::SshParams;
use crate::model::{ContainerSettings, Env, Host, Protocol};
use crate::theme;
use crate::ui::icons::{self, Icon};

const ROW_H: f32 = 54.0;
const POLL: Duration = Duration::from_secs(10);
const PARALLEL_PROBES: usize = 4;

// ---------------------------------------------------------------------------
// Background access

/// Why a host is not contacted in the background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    NotSsh,
    /// A password or key would have to be asked for.
    NoCredentials,
    /// The server key was never accepted (a dialog would pop up).
    UnknownKey,
}

impl Skip {
    pub fn text(self) -> &'static str {
        match self {
            Skip::NotSsh => t!("Kein SSH-Host"),
            Skip::NoCredentials => {
                t!(
                    "Zugangsdaten nicht gespeichert – im Host eine Identität oder ein Passwort hinterlegen"
                )
            }
            Skip::UnknownKey => {
                t!("Serverschlüssel noch nicht bestätigt – einmal normal verbinden")
            }
        }
    }
}

/// Connection parameters for a background job, only from stored credentials and
/// already accepted server keys (also of the jump hosts).
pub fn background_params(app: &App, host: &Host) -> Result<SshParams, Skip> {
    if host.protocol != Protocol::Ssh {
        return Err(Skip::NotSsh);
    }
    let (user, ident) = app.credentials(host);
    let has_secret = ident
        .as_ref()
        .is_some_and(|i| !i.password.is_empty() || !i.private_key.is_empty())
        || crate::backend::sshca::login_for(host, &user, &app.keychain).is_some();
    if !has_secret {
        return Err(Skip::NoCredentials);
    }
    let password = ident
        .as_ref()
        .map(|i| i.password.clone())
        .filter(|p| !p.is_empty());
    let params = app.ssh_params(host, &user, &password, &ident);
    let mut cur = Some(&params);
    while let Some(p) = cur {
        if p.known_fingerprint.is_none() {
            return Err(Skip::UnknownKey);
        }
        cur = p.jump.as_deref();
    }
    Ok(params)
}

fn target_for(app: &App, key: HostKey) -> Result<Target, String> {
    match key {
        None => Ok(Target::Local),
        Some(id) => {
            let host = app
                .lib
                .host(id)
                .ok_or_else(|| t!("Host nicht mehr vorhanden").to_string())?;
            background_params(app, host)
                .map(Target::Ssh)
                .map_err(|s| s.text().to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// State

#[derive(Default, Clone, PartialEq)]
enum Probe {
    #[default]
    Idle,
    Running,
    Tools(Vec<String>),
    Failed(String),
    Skipped(Skip),
}

#[derive(Default)]
pub struct HostLive {
    probe: Probe,
    loading: bool,
    last: Option<Instant>,
    docker: Vec<DockerRound>,
    kube: Option<KubeRound>,
    error: Option<String>,
    kube_ctx: String,
    kube_ns: String,
    prefs_loaded: bool,
    known_ns: BTreeSet<String>,
}

impl HostLive {
    fn tools(&self) -> &[String] {
        match &self.probe {
            Probe::Tools(t) => t,
            _ => &[],
        }
    }
}

#[derive(Default)]
pub struct CtrState {
    io: Option<(Sink<CtrEvent>, CtrRx)>,
    hosts: HashMap<HostKey, HostLive>,
    local_probed: bool,
    pub search: String,
    pub only_running: bool,
    collapsed: HashSet<String>,
    /// Containers/pods with an action in flight.
    busy: HashSet<(HostKey, String)>,
    probe_queue: VecDeque<Uuid>,
    probes_running: usize,
    pub sheet: Option<crate::ui::containers_sheets::Sheet>,
}

impl CtrState {
    fn sink(&mut self, ctx: &egui::Context) -> Sink<CtrEvent> {
        self.io
            .get_or_insert_with(|| Sink::new(ctx.clone()))
            .0
            .clone()
    }
}

#[derive(Clone, Debug)]
pub enum CtrAction {
    Ctl {
        key: HostKey,
        rt: String,
        op: cp::Ctl,
        name: String,
    },
    Compose {
        key: HostKey,
        rt: String,
        project: String,
        op: cp::Compose,
    },
    DeletePod {
        key: HostKey,
        context: String,
        ns: String,
        pod: String,
    },
    Rollout {
        key: HostKey,
        context: String,
        ns: String,
        target: String,
    },
    Inspect {
        key: HostKey,
        rt: String,
        name: String,
    },
    Describe {
        key: HostKey,
        context: String,
        ns: String,
        pod: String,
    },
    SaveHost(Box<Host>),
    /// Show / hide a host on the page.
    Show(Uuid, bool),
    Rescan,
    Refresh,
    SetContext(HostKey, String),
    SetNamespace(HostKey, String),
}

// ---------------------------------------------------------------------------
// Events

pub fn pump(app: &mut App, ctx: &egui::Context) {
    let mut events = Vec::new();
    if let Some((_, rx)) = app.ctr.io.as_mut() {
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
    }
    for ev in events {
        match ev {
            CtrEvent::Probe(key, res) => {
                if key.is_some() {
                    app.ctr.probes_running = app.ctr.probes_running.saturating_sub(1);
                }
                let live = app.ctr.hosts.entry(key).or_default();
                live.probe = match res {
                    Ok(t) => Probe::Tools(t),
                    Err(e) => Probe::Failed(e),
                };
            }
            CtrEvent::Round(key, res) => {
                let live = app.ctr.hosts.entry(key).or_default();
                live.loading = false;
                live.last = Some(Instant::now());
                match res {
                    Ok(d) => {
                        live.error = None;
                        live.docker = d.docker;
                        if let Some(k) = &d.kube {
                            if live.kube_ns.is_empty() {
                                live.known_ns = cp::namespaces(&k.pods).into_iter().collect();
                            } else if let Some(n) = k.pods.first() {
                                live.known_ns.insert(n.namespace.clone());
                            }
                        }
                        live.kube = d.kube;
                    }
                    Err(e) => live.error = Some(e),
                }
            }
            CtrEvent::Op(key, label, subject, res) => {
                app.ctr.busy.remove(&(key, subject));
                if let Some(l) = app.ctr.hosts.get_mut(&key) {
                    l.last = None; // refresh right away
                }
                match res {
                    Ok(_) => app.toast(label, theme::green()),
                    Err(e) => app.toast(format!("{label}: {e}"), theme::red()),
                }
            }
            CtrEvent::Text(inspect, title, res) => {
                use crate::ui::containers_sheets::Sheet;
                app.ctr.sheet = Some(match res {
                    Ok(text) if inspect => match cp::parse_inspect(&text) {
                        Some(info) => Sheet::Inspect {
                            title,
                            info: Box::new(info),
                            reveal: false,
                        },
                        None => Sheet::Text { title, text },
                    },
                    Ok(text) => Sheet::Text { title, text },
                    Err(msg) => Sheet::Error { title, msg },
                });
            }
        }
    }
    start_probes(app, ctx);
}

fn start_probes(app: &mut App, ctx: &egui::Context) {
    while app.ctr.probes_running < PARALLEL_PROBES {
        let Some(id) = app.ctr.probe_queue.pop_front() else {
            break;
        };
        let Some(host) = app.lib.host(id).cloned() else {
            continue;
        };
        match background_params(app, &host) {
            Ok(p) => {
                app.ctr.probes_running += 1;
                app.ctr.hosts.entry(Some(id)).or_default().probe = Probe::Running;
                let sink = app.ctr.sink(ctx);
                run::spawn_probe(
                    &app.rt,
                    Some(id),
                    Target::Ssh(p),
                    sink,
                    app.prompts_sink.clone(),
                );
            }
            Err(s) => app.ctr.hosts.entry(Some(id)).or_default().probe = Probe::Skipped(s),
        }
    }
}

// ---------------------------------------------------------------------------
// Actions

/// Stop, restart and remove on PROD hosts ask first; removing and deleting always asks.
fn guarded(prod: bool, ask: bool, text: String, a: CtrAction) -> Action {
    let a = Action::Ctr(a);
    if ask || prod {
        Action::Confirm(text, Box::new(a))
    } else {
        a
    }
}

pub fn apply(app: &mut App, a: CtrAction, ctx: &egui::Context) {
    match a {
        CtrAction::Ctl { key, rt, op, name } => {
            let Some(argv) = cp::ctl_argv(&rt, op, &name) else {
                return app.toast(t!("Ungültiger Name"), theme::red());
            };
            if op == cp::Ctl::Inspect {
                return;
            }
            let label = match op {
                cp::Ctl::Start => t!("„{}“ gestartet", name),
                cp::Ctl::Stop => t!("„{}“ gestoppt", name),
                cp::Ctl::Restart => t!("„{}“ neu gestartet", name),
                cp::Ctl::Pause => t!("„{}“ pausiert", name),
                cp::Ctl::Unpause => t!("„{}“ fortgesetzt", name),
                cp::Ctl::Remove => t!("„{}“ entfernt", name),
                cp::Ctl::Inspect => String::new(),
            };
            spawn_op(app, ctx, key, label, name, argv);
        }
        CtrAction::Compose {
            key,
            rt,
            project,
            op,
        } => {
            let Some(argv) = cp::compose_argv(&rt, &project, op) else {
                return app.toast(t!("Ungültiger Name"), theme::red());
            };
            let label = match op {
                cp::Compose::Up => t!("Projekt „{}“ gestartet", project),
                cp::Compose::Stop => t!("Projekt „{}“ gestoppt", project),
                cp::Compose::Restart => t!("Projekt „{}“ neu gestartet", project),
            };
            spawn_op(app, ctx, key, label, format!("compose:{project}"), argv);
        }
        CtrAction::DeletePod {
            key,
            context,
            ns,
            pod,
        } => {
            let Some(argv) = cp::pod_delete_argv(&context, &ns, &pod) else {
                return app.toast(t!("Ungültiger Name"), theme::red());
            };
            spawn_op(
                app,
                ctx,
                key,
                t!("Pod „{}“ gelöscht", pod),
                format!("{ns}/{pod}"),
                argv,
            );
        }
        CtrAction::Rollout {
            key,
            context,
            ns,
            target,
        } => {
            let Some(argv) = cp::rollout_argv(&context, &ns, &target) else {
                return app.toast(t!("Ungültiger Name"), theme::red());
            };
            spawn_op(
                app,
                ctx,
                key,
                t!("Rollout-Neustart von {} ausgelöst", target),
                format!("rollout:{ns}/{target}"),
                argv,
            );
        }
        CtrAction::Inspect { key, rt, name } => {
            let Some(argv) = cp::ctl_argv(&rt, cp::Ctl::Inspect, &name) else {
                return app.toast(t!("Ungültiger Name"), theme::red());
            };
            spawn_text(app, ctx, key, true, name, argv);
        }
        CtrAction::Describe {
            key,
            context,
            ns,
            pod,
        } => {
            let Some(argv) = cp::pod_describe_argv(&context, &ns, &pod) else {
                return app.toast(t!("Ungültiger Name"), theme::red());
            };
            spawn_text(app, ctx, key, false, format!("{ns}/{pod}"), argv);
        }
        CtrAction::SaveHost(h) => {
            let dup = app.lib.hosts.iter().any(|x| {
                x.protocol == h.protocol
                    && x.container.via == h.container.via
                    && x.container.name == h.container.name
                    && x.container.namespace == h.container.namespace
                    && x.container.context == h.container.context
            });
            if dup {
                app.toast(t!("Schon in der Hostliste"), theme::amber());
            } else {
                app.toast(t!("„{}“ gespeichert", h.display_name()), theme::green());
                app.lib.hosts.push(*h);
                app.save_library();
            }
        }
        CtrAction::Show(id, on) => {
            let p = &mut app.lib.ctr;
            p.marked.retain(|x| *x != id);
            p.hidden.retain(|x| *x != id);
            if on {
                p.marked.push(id);
                if let Some(l) = app.ctr.hosts.get_mut(&Some(id))
                    && matches!(l.probe, Probe::Skipped(_) | Probe::Failed(_))
                {
                    l.probe = Probe::Idle;
                }
            } else {
                p.hidden.push(id);
            }
            app.save_library();
        }
        CtrAction::Rescan => {
            for (k, l) in app.ctr.hosts.iter_mut() {
                if k.is_some() {
                    l.probe = Probe::Idle;
                }
            }
            app.ctr.probe_queue.clear();
        }
        CtrAction::Refresh => {
            for l in app.ctr.hosts.values_mut() {
                l.last = None;
            }
        }
        CtrAction::SetContext(key, c) => {
            app.lib.ctr.context.insert(cp::pref_key(key), c.clone());
            app.lib.ctr.namespace.remove(&cp::pref_key(key));
            let l = app.ctr.hosts.entry(key).or_default();
            l.kube_ctx = c;
            l.kube_ns.clear();
            l.known_ns.clear();
            l.kube = None;
            l.last = None;
            app.save_library();
        }
        CtrAction::SetNamespace(key, ns) => {
            app.lib.ctr.namespace.insert(cp::pref_key(key), ns.clone());
            let l = app.ctr.hosts.entry(key).or_default();
            l.kube_ns = ns;
            l.last = None;
            app.save_library();
        }
    }
}

fn spawn_op(
    app: &mut App,
    ctx: &egui::Context,
    key: HostKey,
    label: String,
    subject: String,
    argv: Vec<String>,
) {
    match target_for(app, key) {
        Ok(target) => {
            app.ctr.busy.insert((key, subject.clone()));
            let sink = app.ctr.sink(ctx);
            run::spawn_op(
                &app.rt,
                key,
                target,
                label,
                subject,
                argv,
                sink,
                app.prompts_sink.clone(),
            );
        }
        Err(e) => app.toast(e, theme::red()),
    }
}

fn spawn_text(
    app: &mut App,
    ctx: &egui::Context,
    key: HostKey,
    inspect: bool,
    title: String,
    argv: Vec<String>,
) {
    match target_for(app, key) {
        Ok(target) => {
            app.ctr.sheet = Some(crate::ui::containers_sheets::Sheet::Loading(title.clone()));
            let sink = app.ctr.sink(ctx);
            run::spawn_text(
                &app.rt,
                target,
                inspect,
                title,
                argv,
                sink,
                app.prompts_sink.clone(),
            );
        }
        Err(e) => app.toast(e, theme::red()),
    }
}

/// Sheets (inspect / describe).
pub fn sheets(app: &mut App, ctx: &egui::Context) {
    crate::ui::containers_sheets::show(app, ctx);
}

// ---------------------------------------------------------------------------
// Page

/// A host section on the page.
struct Entry {
    key: HostKey,
    name: String,
    env: Env,
}

fn entries(app: &App, st: &CtrState) -> Vec<Entry> {
    let mut out = Vec::new();
    let has = |key: HostKey| st.hosts.get(&key).is_some_and(|l| !l.tools().is_empty());
    // Customer focus: only that customer's hosts (this PC belongs to none).
    let scope = crate::ui::nav2_scope::current(app);
    if has(None) && scope.is_none() {
        out.push(Entry {
            key: None,
            name: t!("Dieser PC").to_string(),
            env: Env::None,
        });
    }
    for h in app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh && crate::ui::nav2_scope::host_ok(&scope, h))
    {
        let marked = app.lib.ctr.marked.contains(&h.id);
        if app.lib.ctr.hidden.contains(&h.id) {
            continue;
        }
        if marked || has(Some(h.id)) {
            out.push(Entry {
                key: Some(h.id),
                name: h.display_name().to_string(),
                env: app.lib.env_of(h),
            });
        }
    }
    out
}

pub fn show(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    // Local tools, once.
    if !app.ctr.local_probed {
        app.ctr.local_probed = true;
        let found: Vec<String> = ["docker", "podman", "kubectl"]
            .iter()
            .filter(|c| crate::backend::local::find_program(c).is_some())
            .map(|c| c.to_string())
            .collect();
        app.ctr.hosts.entry(None).or_default().probe = Probe::Tools(found);
    }
    // Look for container hosts among the saved SSH hosts (lazily, a few at a time).
    let ssh_ids: Vec<Uuid> = app
        .lib
        .hosts
        .iter()
        .filter(|h| h.protocol == Protocol::Ssh && !app.lib.ctr.hidden.contains(&h.id))
        .map(|h| h.id)
        .collect();
    for id in &ssh_ids {
        let idle = app
            .ctr
            .hosts
            .get(&Some(*id))
            .is_none_or(|l| l.probe == Probe::Idle);
        if idle && !app.ctr.probe_queue.contains(id) {
            app.ctr.hosts.entry(Some(*id)).or_default().probe = Probe::Running;
            app.ctr.probe_queue.push_back(*id);
        }
    }
    start_probes(app, &ctx);
    let probing = app.ctr.probes_running + app.ctr.probe_queue.len();

    // Polling.
    let list = entries(app, &app.ctr);
    for e in &list {
        poll_host(app, &ctx, e.key);
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));

    let total: usize = list
        .iter()
        .filter_map(|e| app.ctr.hosts.get(&e.key))
        .map(|l| {
            l.docker.iter().map(|d| d.containers.len()).sum::<usize>()
                + l.kube.as_ref().map_or(0, |k| k.pods.len())
        })
        .sum();

    let mut st = std::mem::take(&mut app.ctr);
    {
        let app_ref = &*app;
        let mut search = std::mem::take(&mut st.search);
        let mut chip_actions = Vec::new();
        theme::page_header(
            ui,
            "ctr",
            |ui| {
                theme::page_title(ui, t!("Container"), Some(total));
                crate::ui::nav_list::scope_chip(app_ref, ui, &mut chip_actions);
            },
            |_| {},
            |ui, bar| {
                if theme::icon_btn(ui, Icon::Refresh, t!("Jetzt aktualisieren")).clicked() {
                    actions.push(Action::Ctr(CtrAction::Refresh));
                }
                hosts_menu(app_ref, ui, bar.compact, actions);
                let width = bar.flex(if bar.compact { 130.0 } else { 170.0 }, 300.0);
                theme::search_field(
                    ui,
                    &mut search,
                    t!("Name, Image, Status, Port"),
                    width,
                    egui::Id::new("ctr-search"),
                );
            },
        );
        st.search = search;
        ui.add_space(14.0);
        let mut only = st.only_running;
        theme::toolbar(
            ui,
            "ctr-filter",
            |ui| {
                let kind = if only {
                    theme::Btn::SmallPrimary
                } else {
                    theme::Btn::Small
                };
                if theme::btn(ui, kind, None, t!("Nur laufende")).clicked() {
                    only = !only;
                }
                if probing > 0 {
                    ui.spinner();
                    ui.label(
                        RichText::new(t!("Suche Container-Hosts… ({})", probing))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                }
            },
            |_, _| {},
        );
        st.only_running = only;
        actions.append(&mut chip_actions);
        ui.add_space(12.0);

        if list.is_empty() && app_ref.lib.nav.scope.is_some() {
            crate::ui::nav2_scope::empty_hint(ui, app_ref, t!("keine Container-Hosts"), actions);
        } else if list.is_empty() {
            empty_state(ui, probing > 0);
        }
        let query = st.search.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("ctr-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 14.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, e) in list.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(26.0);
                    }
                    host_section(app_ref, &mut st, ui, e, &query, actions);
                }
                ui.add_space(40.0);
            });
    }
    app.ctr = st;
}

fn empty_state(ui: &mut egui::Ui, searching: bool) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(if searching {
                t!("Suche nach Docker, Podman und kubectl…")
            } else {
                t!("Weder auf diesem PC noch auf einem SSH-Host wurde Docker, Podman oder kubectl gefunden")
            })
            .color(theme::text_dim()),
        );
        if !searching {
            ui.add_space(6.0);
            ui.label(
                RichText::new(t!(
                    "Über „Hosts“ oben lassen sich SSH-Hosts von Hand als Container-Host einblenden."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
        }
    });
}

fn hosts_menu(app: &App, ui: &mut egui::Ui, compact: bool, actions: &mut Vec<Action>) {
    let label = if compact { "" } else { t!("Hosts") };
    let r = theme::btn(ui, theme::Btn::Normal, Some(Icon::Server), label)
        .on_hover_text(t!("Welche SSH-Hosts auf dieser Seite erscheinen"));
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(260.0);
        let scope = crate::ui::nav2_scope::current(app);
        let ssh: Vec<&Host> = app
            .lib
            .hosts
            .iter()
            .filter(|h| h.protocol == Protocol::Ssh && crate::ui::nav2_scope::host_ok(&scope, h))
            .collect();
        if ssh.is_empty() {
            ui.label(RichText::new(t!("Keine SSH-Hosts gespeichert")).color(theme::text_dim()));
        }
        for h in ssh {
            let live = app.ctr.hosts.get(&Some(h.id));
            let found = live.is_some_and(|l| !l.tools().is_empty());
            let on = !app.lib.ctr.hidden.contains(&h.id)
                && (app.lib.ctr.marked.contains(&h.id) || found);
            let mut v = on;
            let text = if found {
                format!(
                    "{}  ·  {}",
                    h.display_name(),
                    live.map_or(String::new(), |l| l.tools().join("/"))
                )
            } else {
                h.display_name().to_string()
            };
            if ui.checkbox(&mut v, text).changed() {
                actions.push(Action::Ctr(CtrAction::Show(h.id, v)));
            }
        }
        ui.separator();
        if ui.button(t!("Erneut suchen")).clicked() {
            actions.push(Action::Ctr(CtrAction::Rescan));
            ui.close();
        }
    });
}

fn poll_host(app: &mut App, ctx: &egui::Context, key: HostKey) {
    let (prefs_ctx, prefs_ns) = (
        app.lib.ctr.context.get(&cp::pref_key(key)).cloned(),
        app.lib.ctr.namespace.get(&cp::pref_key(key)).cloned(),
    );
    let live = app.ctr.hosts.entry(key).or_default();
    if !live.prefs_loaded {
        live.prefs_loaded = true;
        live.kube_ctx = prefs_ctx.unwrap_or_default();
        live.kube_ns = prefs_ns.unwrap_or_default();
    }
    let tools: Vec<String> = live.tools().to_vec();
    if tools.is_empty() || live.loading || live.last.is_some_and(|t| t.elapsed() < POLL) {
        return;
    }
    let spec = RoundSpec {
        runtimes: tools
            .iter()
            .filter(|t| cp::valid_runtime(t))
            .cloned()
            .collect(),
        kube: tools.iter().any(|t| t == "kubectl").then(|| KubeSpec {
            context: live.kube_ctx.clone(),
            namespace: live.kube_ns.clone(),
        }),
    };
    match target_for(app, key) {
        Ok(target) => {
            let live = app.ctr.hosts.entry(key).or_default();
            live.loading = true;
            let sink = app.ctr.sink(ctx);
            run::spawn_round(&app.rt, key, target, spec, sink, app.prompts_sink.clone());
        }
        Err(e) => {
            let live = app.ctr.hosts.entry(key).or_default();
            live.error = Some(e);
            live.last = Some(Instant::now());
        }
    }
}

// ---------------------------------------------------------------------------
// Sections

fn host_section(
    app: &App,
    st: &mut CtrState,
    ui: &mut egui::Ui,
    e: &Entry,
    query: &str,
    actions: &mut Vec<Action>,
) {
    let live = st.hosts.get(&e.key);
    let loading = live.is_some_and(|l| l.loading);
    let section_id = format!("host:{:?}", e.key);
    let collapsed = st.collapsed.contains(&section_id);
    let words: Vec<&str> = query.split_whitespace().collect();

    // Summary over everything this host runs.
    let mut summary = cp::Summary::default();
    let mut pods = 0usize;
    let mut pod_problems = 0usize;
    if let Some(l) = live {
        for d in &l.docker {
            let s = cp::summarize(&d.containers);
            summary.running += s.running;
            summary.stopped += s.stopped;
            summary.paused += s.paused;
            summary.unhealthy += s.unhealthy;
        }
        if let Some(k) = &l.kube {
            pods = k.pods.len();
            pod_problems = k
                .pods
                .iter()
                .filter(|p| cp::pod_severity(p) != Severity::Ok)
                .count();
        }
    }
    let has_docker = live.is_some_and(|l| !l.docker.is_empty());

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (dot, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
        let color = match live {
            Some(l) if l.error.is_some() => theme::red(),
            Some(l) if matches!(l.probe, Probe::Failed(_) | Probe::Skipped(_)) => theme::amber(),
            Some(l) if !l.docker.is_empty() || l.kube.is_some() => theme::green(),
            _ => theme::off_color(),
        };
        theme::led(
            ui.painter(),
            dot.center(),
            color,
            color != theme::off_color(),
        );
        let (cr, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
        paint_chevron(ui.painter(), cr.center(), !collapsed, theme::text_dim());
        let r = ui.add(
            egui::Label::new(
                RichText::new(e.name.clone())
                    .font(theme::semibold(16.0))
                    .color(theme::text()),
            )
            .sense(Sense::click()),
        );
        if r.clicked() {
            if collapsed {
                st.collapsed.remove(&section_id);
            } else {
                st.collapsed.insert(section_id.clone());
            }
        }
        if e.env != Env::None {
            theme::pill(ui, e.env.label(), theme::env_color(e.env));
        }
        let mut parts = Vec::new();
        if has_docker {
            parts.push(summary.text());
        }
        if pods > 0 {
            parts.push(if pod_problems > 0 {
                t!("{} Pods · {} mit Problemen", pods, pod_problems)
            } else {
                t!("{} Pods", pods)
            });
        }
        if !parts.is_empty() {
            ui.label(RichText::new(parts.join("  ·  ")).size(12.5).color(
                if summary.unhealthy > 0 || pod_problems > 0 {
                    theme::amber()
                } else {
                    theme::text_dim()
                },
            ));
        }
        if loading && live.is_some_and(|l| l.last.is_none()) {
            ui.spinner();
        }
    });
    ui.add_space(8.0);
    if collapsed {
        return;
    }

    // Problems of this host.
    if let Some(l) = live {
        match &l.probe {
            Probe::Failed(m) => note(ui, &explain_failure(m), theme::red()),
            Probe::Skipped(s) => note(ui, s.text(), theme::amber()),
            _ => {}
        }
        if let Some(m) = &l.error {
            note(ui, &explain_failure(m), theme::red());
        }
        for d in &l.docker {
            if let Some(m) = &d.error {
                note(
                    ui,
                    &format!("{}: {}", d.runtime, explain_failure(m)),
                    theme::red(),
                );
            }
        }
    }
    let Some(l) = live else {
        return;
    };

    let group = e
        .key
        .and_then(|id| app.lib.host(id))
        .and_then(|h| h.group_id);
    let prod = e.env == Env::Prod;
    let docker: Vec<DockerRound> = l.docker.clone();
    let kube = l.kube.clone();
    let (ctx_sel, ns_sel) = (l.kube_ctx.clone(), l.kube_ns.clone());
    let known: Vec<String> = l.known_ns.iter().cloned().collect();
    for d in &docker {
        if d.error.is_some() {
            continue;
        }
        let rc = RowCtx {
            key: e.key,
            rt: d.runtime.clone(),
            prod,
            group,
        };
        docker_list(app, st, ui, &rc, &d.containers, &words, actions);
    }
    if let Some(k) = kube {
        kube_list(
            app, st, ui, e, &k, &ctx_sel, &ns_sel, &known, &words, group, actions,
        );
    }
}

/// Small triangle: pointing down when open, right when closed.
fn paint_chevron(p: &egui::Painter, c: Pos2, open: bool, color: egui::Color32) {
    let pts = if open {
        vec![
            c + Vec2::new(-4.5, -2.5),
            c + Vec2::new(4.5, -2.5),
            c + Vec2::new(0.0, 3.0),
        ]
    } else {
        vec![
            c + Vec2::new(-2.5, -4.5),
            c + Vec2::new(-2.5, 4.5),
            c + Vec2::new(3.0, 0.0),
        ]
    };
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

fn note(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).wrap());
    ui.add_space(6.0);
}

/// Adds the usual cause to common failures.
fn explain_failure(m: &str) -> String {
    let low = m.to_lowercase();
    if low.contains("permission denied")
        && !low.contains("docker.sock")
        && !low.contains("publickey")
    {
        format!(
            "{m}\n{}",
            t!(
                "Hinweis: Benutzer in der Gruppe docker? (sudo usermod -aG docker $USER, danach neu anmelden)"
            )
        )
    } else {
        m.to_string()
    }
}

struct RowCtx {
    key: HostKey,
    rt: String,
    prod: bool,
    group: Option<Uuid>,
}

fn matches_words(words: &[&str], hay: &str) -> bool {
    let h = hay.to_lowercase();
    words.iter().all(|w| h.contains(w))
}

// --- Docker -----------------------------------------------------------------

fn docker_list(
    app: &App,
    st: &mut CtrState,
    ui: &mut egui::Ui,
    rc: &RowCtx,
    items: &[DockerContainer],
    words: &[&str],
    actions: &mut Vec<Action>,
) {
    let visible: Vec<usize> = (0..items.len())
        .filter(|i| {
            let c = &items[*i];
            (!st.only_running || c.running())
                && (words.is_empty()
                    || matches_words(
                        words,
                        &format!(
                            "{} {} {} {} {} {}",
                            c.name,
                            c.image,
                            c.status,
                            c.state,
                            c.ports,
                            c.project.as_deref().unwrap_or("")
                        ),
                    ))
        })
        .collect();
    let caption = if rc.rt == "podman" {
        "Podman"
    } else {
        "Docker"
    };
    ui.horizontal(|ui| {
        theme::section_title(ui, &format!("{caption} · {}", items.len()));
    });
    ui.add_space(6.0);
    if visible.is_empty() {
        ui.label(
            RichText::new(if items.is_empty() {
                t!("Keine Container")
            } else {
                t!("Keine Treffer")
            })
            .size(12.5)
            .color(theme::text_dim()),
        );
        ui.add_space(8.0);
        return;
    }
    let shown: Vec<DockerContainer> = visible.iter().map(|i| items[*i].clone()).collect();
    let groups = cp::group_by_project(&shown);
    let grouped = groups.iter().any(|g| g.project.is_some());
    theme::list_card().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut first = true;
        for g in &groups {
            let gid = format!("proj:{:?}:{}:{:?}", rc.key, rc.rt, g.project);
            let collapsed = st.collapsed.contains(&gid);
            if grouped {
                let label = g
                    .project
                    .clone()
                    .unwrap_or_else(|| t!("Einzelne Container").to_string());
                let all: Vec<&DockerContainer> = g.items.iter().map(|i| &shown[*i]).collect();
                if group_header(
                    ui,
                    rc,
                    &label,
                    g.project.as_deref(),
                    &all,
                    collapsed,
                    !first,
                    st.busy.contains(&(rc.key, format!("compose:{label}"))),
                    actions,
                ) {
                    if collapsed {
                        st.collapsed.remove(&gid);
                    } else {
                        st.collapsed.insert(gid);
                    }
                }
            }
            first = false;
            if collapsed {
                continue;
            }
            for (n, i) in g.items.iter().enumerate() {
                let c = &shown[*i];
                let busy = st.busy.contains(&(rc.key, c.name.clone()));
                container_row(app, ui, rc, c, busy, n > 0 || grouped, grouped, actions);
            }
        }
    });
    ui.add_space(14.0);
}

fn paint_pill(p: &egui::Painter, x: f32, cy: f32, text: &str, color: egui::Color32) -> f32 {
    let g = p.layout_no_wrap(text.to_uppercase(), theme::bold(10.0), color);
    let w = g.size().x + 12.0;
    let r = Rect::from_min_size(Pos2::new(x, cy - 8.0), Vec2::new(w, 16.0));
    p.rect_filled(r, CornerRadius::same(5), theme::soft(color));
    p.galley(r.center() - g.size() / 2.0, g, color);
    w
}

#[allow(clippy::too_many_arguments)]
fn group_header(
    ui: &mut egui::Ui,
    rc: &RowCtx,
    label: &str,
    project: Option<&str>,
    items: &[&DockerContainer],
    collapsed: bool,
    divider: bool,
    busy: bool,
    actions: &mut Vec<Action>,
) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect_filled(rect, CornerRadius::ZERO, theme::fill());
    if divider {
        p.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, theme::line()),
        );
    }
    let running = items.iter().filter(|c| c.running()).count();
    paint_chevron(
        &p,
        Pos2::new(rect.left() + 20.0, rect.center().y),
        !collapsed,
        theme::text_dim(),
    );
    let text = format!(
        "{}  ·  {}",
        label,
        t!("{} von {} laufen", running, items.len())
    );
    p.text(
        Pos2::new(rect.left() + 34.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        theme::semibold(13.0),
        if project.is_some() {
            theme::text()
        } else {
            theme::text_dim()
        },
    );
    if busy {
        p.circle_filled(
            Pos2::new(rect.right() - 14.0, rect.center().y),
            4.0,
            theme::amber(),
        );
    }
    let mut toggled = resp.clicked();
    if let Some(proj) = project
        && hovered
        && !busy
    {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - 140.0, rect.top()),
                    rect.right_bottom() - Vec2::new(6.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 2.0;
        let mk = |op: cp::Compose| CtrAction::Compose {
            key: rc.key,
            rt: rc.rt.clone(),
            project: proj.to_string(),
            op,
        };
        if theme::icon_btn(&mut child, Icon::Refresh, t!("Projekt neu starten")).clicked() {
            actions.push(guarded(
                rc.prod,
                false,
                t!("Alle Container des Projekts „{}“ neu starten?", proj),
                mk(cp::Compose::Restart),
            ));
            toggled = false;
        }
        if theme::icon_btn(&mut child, Icon::Stop, t!("Projekt stoppen")).clicked() {
            actions.push(guarded(
                rc.prod,
                false,
                t!("Alle Container des Projekts „{}“ stoppen?", proj),
                mk(cp::Compose::Stop),
            ));
            toggled = false;
        }
        if theme::icon_btn(&mut child, Icon::Play, t!("Projekt starten (up -d)")).clicked() {
            actions.push(guarded(false, false, String::new(), mk(cp::Compose::Up)));
            toggled = false;
        }
    }
    toggled
}

fn led_color(c: &DockerContainer, busy: bool) -> (egui::Color32, bool) {
    if busy {
        return (theme::amber(), true);
    }
    match (c.state.as_str(), c.health) {
        (_, cp::Health::Unhealthy) => (theme::red(), true),
        ("running", cp::Health::Starting) => (theme::amber(), true),
        ("running", _) => (theme::green(), true),
        ("paused" | "restarting", _) => (theme::amber(), true),
        ("dead", _) => (theme::red(), false),
        _ => (theme::off_color(), false),
    }
}

fn container_host(rc: &RowCtx, c: &DockerContainer) -> Host {
    Host {
        protocol: Protocol::Docker,
        group_id: rc.group,
        port: 0,
        container: ContainerSettings {
            via: rc.key,
            podman: rc.rt == "podman",
            name: c.name.clone(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn container_row(
    app: &App,
    ui: &mut egui::Ui,
    rc: &RowCtx,
    c: &DockerContainer,
    busy: bool,
    divider: bool,
    indent: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let more_id = egui::Id::new(("ctr-more", rc.key, &rc.rt, &c.name));
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let active = hovered || egui::Popup::is_id_open(ui.ctx(), more_id);
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if divider {
        p.line_segment(
            [
                Pos2::new(rect.left() + 40.0, rect.top()),
                Pos2::new(rect.right(), rect.top()),
            ],
            Stroke::new(1.0, theme::line()),
        );
    }
    let (y1, y2) = (rect.top() + 19.0, rect.top() + 37.0);
    let x0 = rect.left() + if indent { 30.0 } else { 20.0 };
    let (color, lit) = led_color(c, busy);
    theme::led(&p, Pos2::new(x0, rect.center().y), color, lit);
    let col = x0 + 20.0;

    let narrow = rect.width() < 520.0;
    let stats_w = if rect.width() >= 600.0 { 120.0 } else { 0.0 };
    let ports_w = if rect.width() >= 780.0 { 170.0 } else { 0.0 };
    let strip_w = if narrow { 120.0 } else { 232.0 };
    let text_right = if active {
        rect.right() - 10.0 - strip_w - 14.0
    } else {
        rect.right() - 16.0 - stats_w - ports_w - 8.0
    };

    // line 1: name + health
    let pill_w = if c.health == cp::Health::Unhealthy {
        70.0
    } else {
        0.0
    };
    let ng = theme::truncated(
        &p,
        c.name.clone(),
        theme::semibold(14.5),
        theme::text(),
        (text_right - col - pill_w).max(40.0),
    );
    let nw = ng.size().x;
    p.galley(Pos2::new(col, y1 - ng.size().y / 2.0), ng, theme::text());
    if pill_w > 0.0 && col + nw + 8.0 + pill_w <= text_right + 10.0 {
        paint_pill(&p, col + nw + 8.0, y1, "unhealthy", theme::red());
    }
    // line 2: image · status · created
    let mut line = vec![c.image.clone(), c.status.clone()];
    if !c.created.is_empty() && !narrow {
        line.push(t!("erstellt {}", c.created));
    }
    line.retain(|s| !s.is_empty());
    let g = theme::truncated(
        &p,
        line.join("  ·  "),
        FontId::proportional(12.5),
        theme::text_dim(),
        (text_right - col).max(30.0),
    );
    p.galley(Pos2::new(col, y2 - g.size().y / 2.0), g, theme::text_dim());

    if !active {
        let right = rect.right() - 16.0;
        if stats_w > 0.0 && c.running() {
            let cpu = c.cpu.map(|v| format!("{v:.1} % CPU")).unwrap_or_default();
            let ram = if c.mem_usage.is_empty() {
                String::new()
            } else {
                match c.mem_pct {
                    Some(p) => format!("{} · {p:.0} %", c.mem_usage),
                    None => c.mem_usage.clone(),
                }
            };
            p.text(
                Pos2::new(right, y1),
                egui::Align2::RIGHT_CENTER,
                cpu,
                FontId::proportional(12.5),
                theme::text(),
            );
            p.text(
                Pos2::new(right, y2),
                egui::Align2::RIGHT_CENTER,
                ram,
                FontId::proportional(12.0),
                theme::text_dim(),
            );
        }
        if ports_w > 0.0 && !c.ports.is_empty() {
            let g = theme::truncated(
                &p,
                c.ports.clone(),
                FontId::monospace(11.5),
                theme::text_dim(),
                ports_w - 6.0,
            );
            let w = g.size().x;
            p.galley(
                Pos2::new(right - stats_w - 8.0 - w, y1 - g.size().y / 2.0),
                g,
                theme::text_dim(),
            );
        }
        ui.interact(
            Rect::from_min_max(
                Pos2::new(x0 - 9.0, rect.center().y - 9.0),
                Pos2::new(x0 + 9.0, rect.center().y + 9.0),
            ),
            ui.id().with(("ctr-led", rc.key, &rc.rt, &c.name)),
            Sense::hover(),
        )
        .on_hover_text(format!("{}\n{}", c.state, c.status));
    } else if !busy {
        let layers = [theme::surface(), theme::hover()];
        theme::fade_cover(
            &p,
            Rect::from_min_max(
                Pos2::new(rect.right() - strip_w - 16.0, rect.top() + 1.0),
                rect.right_bottom(),
            ),
            16.0,
            &layers,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - strip_w - 12.0, rect.top()),
                    rect.right_bottom() - Vec2::new(8.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 4.0;
        let more = theme::icon_button(&mut child, "…", t!("Weitere Aktionen"));
        egui::Popup::menu(&more)
            .id(more_id)
            .show(|ui| container_menu(ui, rc, c, actions));
        if !narrow && theme::icon_btn(&mut child, Icon::Logs, t!("Logs ansehen")).clicked() {
            actions.push(open_action(rc, c, true));
        }
        if c.running() {
            if theme::btn(&mut child, theme::Btn::SmallPrimary, None, t!("Shell")).clicked() {
                actions.push(open_action(rc, c, false));
            }
            if theme::icon_btn(&mut child, Icon::Stop, t!("Stoppen")).clicked() {
                actions.push(ctl_action(rc, c, cp::Ctl::Stop));
            }
            if !narrow && theme::icon_btn(&mut child, Icon::Refresh, t!("Neu starten")).clicked() {
                actions.push(ctl_action(rc, c, cp::Ctl::Restart));
            }
        } else if theme::btn(
            &mut child,
            theme::Btn::SmallPrimary,
            Some(Icon::Play),
            t!("Start"),
        )
        .clicked()
        {
            actions.push(ctl_action(rc, c, cp::Ctl::Start));
        }
    }
    if resp.double_clicked() && c.running() {
        actions.push(open_action(rc, c, false));
    }
    resp.context_menu(|ui| container_menu(ui, rc, c, actions));
    let _ = app;
}

fn open_action(rc: &RowCtx, c: &DockerContainer, logs: bool) -> Action {
    Action::OpenContainer(Box::new(container_host(rc, c)), logs)
}

fn ctl_action(rc: &RowCtx, c: &DockerContainer, op: cp::Ctl) -> Action {
    let a = CtrAction::Ctl {
        key: rc.key,
        rt: rc.rt.clone(),
        op,
        name: c.name.clone(),
    };
    match op {
        cp::Ctl::Remove => guarded(
            rc.prod,
            true,
            t!(
                "Container „{}“ entfernen? Nicht in Volumes gespeicherte Daten gehen verloren.",
                c.name
            ),
            a,
        ),
        cp::Ctl::Stop => guarded(rc.prod, false, t!("„{}“ stoppen?", c.name), a),
        cp::Ctl::Restart => guarded(rc.prod, false, t!("„{}“ neu starten?", c.name), a),
        _ => Action::Ctr(a),
    }
}

fn container_menu(ui: &mut egui::Ui, rc: &RowCtx, c: &DockerContainer, actions: &mut Vec<Action>) {
    let mut item = |ui: &mut egui::Ui, label: &str, a: Action| {
        if ui.button(label).clicked() {
            actions.push(a);
            ui.close();
        }
    };
    if c.running() {
        item(ui, t!("Shell öffnen"), open_action(rc, c, false));
    }
    item(ui, t!("Logs ansehen"), open_action(rc, c, true));
    item(
        ui,
        t!("Inspect…"),
        Action::Ctr(CtrAction::Inspect {
            key: rc.key,
            rt: rc.rt.clone(),
            name: c.name.clone(),
        }),
    );
    ui.separator();
    if c.running() {
        item(ui, t!("Neu starten"), ctl_action(rc, c, cp::Ctl::Restart));
        if c.paused() {
            item(ui, t!("Fortsetzen"), ctl_action(rc, c, cp::Ctl::Unpause));
        } else {
            item(ui, t!("Pausieren"), ctl_action(rc, c, cp::Ctl::Pause));
        }
        item(ui, t!("Stoppen"), ctl_action(rc, c, cp::Ctl::Stop));
    } else {
        item(ui, t!("Start"), ctl_action(rc, c, cp::Ctl::Start));
    }
    ui.separator();
    if ui.button(t!("Name kopieren")).clicked() {
        ui.ctx().copy_text(c.name.clone());
        ui.close();
    }
    item(
        ui,
        t!("Als Host speichern"),
        Action::Ctr(CtrAction::SaveHost(Box::new(container_host(rc, c)))),
    );
    ui.separator();
    if ui
        .button(RichText::new(t!("Entfernen…")).color(theme::red()))
        .clicked()
    {
        actions.push(ctl_action(rc, c, cp::Ctl::Remove));
        ui.close();
    }
}

// --- Kubernetes ---------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn kube_list(
    app: &App,
    st: &mut CtrState,
    ui: &mut egui::Ui,
    e: &Entry,
    k: &KubeRound,
    ctx_sel: &str,
    ns_sel: &str,
    known_ns: &[String],
    words: &[&str],
    group: Option<Uuid>,
    actions: &mut Vec<Action>,
) {
    let current = if ctx_sel.is_empty() {
        k.current.clone()
    } else {
        ctx_sel.to_string()
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        theme::section_title(ui, &format!("Kubernetes · {}", k.pods.len()));
        ui.label(
            RichText::new(t!("Kontext"))
                .size(12.5)
                .color(theme::text_dim()),
        );
        egui::ComboBox::from_id_salt(("ctr-ctx", e.key))
            .selected_text(if current.is_empty() {
                t!("aktueller").to_string()
            } else {
                current.clone()
            })
            .width(170.0)
            .show_ui(ui, |ui| {
                for c in &k.contexts {
                    if ui.selectable_label(*c == current, c).clicked() && *c != current {
                        actions.push(Action::Ctr(CtrAction::SetContext(e.key, c.clone())));
                    }
                }
            });
        ui.label(
            RichText::new(t!("Namespace"))
                .size(12.5)
                .color(theme::text_dim()),
        );
        egui::ComboBox::from_id_salt(("ctr-ns", e.key))
            .selected_text(if ns_sel.is_empty() {
                t!("Alle").to_string()
            } else {
                ns_sel.to_string()
            })
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(ns_sel.is_empty(), t!("Alle")).clicked() {
                    actions.push(Action::Ctr(CtrAction::SetNamespace(e.key, String::new())));
                }
                for n in known_ns {
                    if ui.selectable_label(n == ns_sel, n).clicked() && n != ns_sel {
                        actions.push(Action::Ctr(CtrAction::SetNamespace(e.key, n.clone())));
                    }
                }
            });
    });
    ui.add_space(6.0);
    if let Some(m) = &k.error {
        note(ui, &explain_failure(m), theme::red());
        return;
    }
    let mut pods: Vec<&Pod> = k
        .pods
        .iter()
        .filter(|p| {
            (!st.only_running || p.status == "Running")
                && (words.is_empty()
                    || matches_words(
                        words,
                        &format!("{}/{} {} {}", p.namespace, p.name, p.status, p.node),
                    ))
        })
        .collect();
    pods.sort_by(|a, b| cp::pod_order(a, b));
    if pods.is_empty() {
        ui.label(
            RichText::new(if k.pods.is_empty() {
                t!("Keine Pods gefunden")
            } else {
                t!("Keine Treffer")
            })
            .size(12.5)
            .color(theme::text_dim()),
        );
        ui.add_space(8.0);
        return;
    }
    let prod = e.env == Env::Prod;
    let now = chrono::Utc::now();
    theme::list_card().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, p) in pods.iter().enumerate() {
            let busy = st
                .busy
                .contains(&(e.key, format!("{}/{}", p.namespace, p.name)));
            pod_row(
                app,
                ui,
                e.key,
                &current,
                group,
                prod,
                p,
                busy,
                i > 0,
                now,
                actions,
            );
        }
    });
    ui.add_space(14.0);
}

fn sev_color(s: Severity) -> egui::Color32 {
    match s {
        Severity::Ok => theme::green(),
        Severity::Warn => theme::amber(),
        Severity::Bad => theme::red(),
    }
}

#[allow(clippy::too_many_arguments)]
fn pod_row(
    _app: &App,
    ui: &mut egui::Ui,
    key: HostKey,
    context: &str,
    group: Option<Uuid>,
    prod: bool,
    pod: &Pod,
    busy: bool,
    divider: bool,
    now: chrono::DateTime<chrono::Utc>,
    actions: &mut Vec<Action>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let more_id = egui::Id::new(("pod-more", key, &pod.namespace, &pod.name));
    let hovered = ui.rect_contains_pointer(rect.shrink2(Vec2::new(0.0, 0.5)));
    let active = hovered || egui::Popup::is_id_open(ui.ctx(), more_id);
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    if divider {
        p.line_segment(
            [
                Pos2::new(rect.left() + 40.0, rect.top()),
                Pos2::new(rect.right(), rect.top()),
            ],
            Stroke::new(1.0, theme::line()),
        );
    }
    let sev = cp::pod_severity(pod);
    let (y1, y2) = (rect.top() + 19.0, rect.top() + 37.0);
    let x0 = rect.left() + 20.0;
    let color = if busy { theme::amber() } else { sev_color(sev) };
    theme::led(
        &p,
        Pos2::new(x0, rect.center().y),
        color,
        sev != Severity::Ok || busy || pod.status == "Running",
    );
    let col = x0 + 20.0;
    let narrow = rect.width() < 520.0;
    let wide = rect.width() >= 780.0;
    let strip_w = if narrow { 120.0 } else { 232.0 };
    let cols_w = if wide { 300.0 } else { 0.0 };
    let text_right = if active {
        rect.right() - 10.0 - strip_w - 14.0
    } else {
        rect.right() - 16.0 - cols_w - 8.0
    };
    let g = theme::truncated(
        &p,
        format!("{}/{}", pod.namespace, pod.name),
        theme::semibold(14.5),
        theme::text(),
        (text_right - col).max(40.0),
    );
    p.galley(Pos2::new(col, y1 - g.size().y / 2.0), g, theme::text());

    let age = pod
        .created
        .map(|t| cp::age_short((now - t).num_seconds()))
        .unwrap_or_default();
    let hint = cp::pod_hint(pod);
    let (line2, line2_color) = match hint {
        Some(h) if sev != Severity::Ok => (h.to_string(), sev_color(sev)),
        _ => {
            let mut parts = vec![format!("{} {}/{}", t!("bereit"), pod.ready, pod.total)];
            if pod.restarts > 0 {
                parts.push(t!("{} Neustarts", pod.restarts));
            }
            if !age.is_empty() {
                parts.push(age.clone());
            }
            if !pod.node.is_empty() {
                parts.push(pod.node.clone());
            }
            (parts.join("  ·  "), theme::text_dim())
        }
    };
    let g = theme::truncated(
        &p,
        line2,
        FontId::proportional(12.5),
        line2_color,
        (text_right - col).max(30.0),
    );
    p.galley(Pos2::new(col, y2 - g.size().y / 2.0), g, line2_color);

    if !active {
        if wide {
            let right = rect.right() - 16.0;
            // status | ready | restarts | age
            let cols = [
                (right - 290.0, pod.status.clone(), color),
                (
                    right - 170.0,
                    format!("{}/{}", pod.ready, pod.total),
                    theme::text_dim(),
                ),
                (
                    right - 90.0,
                    pod.restarts.to_string(),
                    if pod.restarts > 5 {
                        theme::amber()
                    } else {
                        theme::text_dim()
                    },
                ),
                (right, age.clone(), theme::text_dim()),
            ];
            for (i, (x, text, c)) in cols.into_iter().enumerate() {
                let g = theme::truncated(
                    &p,
                    text,
                    FontId::proportional(12.5),
                    c,
                    if i == 0 { 96.0 } else { 44.0 },
                );
                let w = g.size().x;
                let x = if i == 0 { x } else { x - w };
                p.galley(Pos2::new(x, y1 - g.size().y / 2.0), g, c);
            }
            let hdr = [
                (right - 290.0, t!("Status"), false),
                (right - 170.0, t!("Bereit"), true),
                (right - 90.0, t!("Neustarts"), true),
                (right, t!("Alter"), true),
            ];
            for (x, text, align_right) in hdr {
                p.text(
                    Pos2::new(x, y2),
                    if align_right {
                        egui::Align2::RIGHT_CENTER
                    } else {
                        egui::Align2::LEFT_CENTER
                    },
                    text,
                    FontId::proportional(10.5),
                    theme::text_dim().gamma_multiply(0.7),
                );
            }
        } else {
            let g = p.layout_no_wrap(pod.status.clone(), theme::medium(12.0), color);
            let w = g.size().x;
            if text_right - col > w + 90.0 {
                p.galley(
                    Pos2::new(rect.right() - 16.0 - w, y1 - g.size().y / 2.0),
                    g,
                    color,
                );
            }
        }
        if let Some(h) = hint {
            ui.interact(
                Rect::from_center_size(Pos2::new(x0, rect.center().y), Vec2::splat(18.0)),
                ui.id().with(("pod-led", key, &pod.namespace, &pod.name)),
                Sense::hover(),
            )
            .on_hover_text(h);
        }
    } else if !busy {
        let layers = [theme::surface(), theme::hover()];
        theme::fade_cover(
            &p,
            Rect::from_min_max(
                Pos2::new(rect.right() - strip_w - 16.0, rect.top() + 1.0),
                rect.right_bottom(),
            ),
            16.0,
            &layers,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(rect.right() - strip_w - 12.0, rect.top()),
                    rect.right_bottom() - Vec2::new(8.0, 0.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 4.0;
        let more = theme::icon_button(&mut child, "…", t!("Weitere Aktionen"));
        egui::Popup::menu(&more)
            .id(more_id)
            .show(|ui| pod_menu(ui, key, context, group, prod, pod, actions));
        if !narrow
            && theme::icon_btn(&mut child, Icon::Search, t!("Beschreiben (describe)")).clicked()
        {
            actions.push(describe_action(key, context, pod));
        }
        if theme::icon_btn(&mut child, Icon::Logs, t!("Logs folgen")).clicked() {
            actions.push(pod_open(key, context, group, pod, true));
        }
        if pod.status == "Running"
            && theme::btn(&mut child, theme::Btn::SmallPrimary, None, t!("Shell")).clicked()
        {
            actions.push(pod_open(key, context, group, pod, false));
        }
    }
    if resp.double_clicked() && pod.status == "Running" {
        actions.push(pod_open(key, context, group, pod, false));
    }
    resp.context_menu(|ui| pod_menu(ui, key, context, group, prod, pod, actions));
}

fn describe_action(key: HostKey, context: &str, pod: &Pod) -> Action {
    Action::Ctr(CtrAction::Describe {
        key,
        context: context.to_string(),
        ns: pod.namespace.clone(),
        pod: pod.name.clone(),
    })
}

fn pod_open(key: HostKey, context: &str, group: Option<Uuid>, pod: &Pod, logs: bool) -> Action {
    Action::OpenContainer(Box::new(pod_host(key, context, group, pod)), logs)
}

fn pod_host(key: HostKey, context: &str, group: Option<Uuid>, pod: &Pod) -> Host {
    Host {
        protocol: Protocol::Kubernetes,
        group_id: group,
        port: 0,
        container: ContainerSettings {
            via: key,
            name: pod.name.clone(),
            namespace: pod.namespace.clone(),
            context: context.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn pod_menu(
    ui: &mut egui::Ui,
    key: HostKey,
    context: &str,
    group: Option<Uuid>,
    prod: bool,
    pod: &Pod,
    actions: &mut Vec<Action>,
) {
    let mut item = |ui: &mut egui::Ui, label: &str, a: Action| {
        if ui.button(label).clicked() {
            actions.push(a);
            ui.close();
        }
    };
    if pod.status == "Running" {
        item(
            ui,
            t!("Shell öffnen"),
            pod_open(key, context, group, pod, false),
        );
    }
    item(
        ui,
        t!("Logs folgen"),
        pod_open(key, context, group, pod, true),
    );
    item(
        ui,
        t!("Beschreiben (describe)"),
        describe_action(key, context, pod),
    );
    if let Some(owner) = &pod.owner {
        ui.separator();
        item(
            ui,
            &t!("Rollout-Neustart ({})…", owner),
            guarded(
                prod,
                true,
                t!(
                    "{} neu ausrollen? Alle Pods werden nacheinander ersetzt.",
                    owner
                ),
                CtrAction::Rollout {
                    key,
                    context: context.to_string(),
                    ns: pod.namespace.clone(),
                    target: owner.clone(),
                },
            ),
        );
    }
    ui.separator();
    if ui.button(t!("Name kopieren")).clicked() {
        ui.ctx().copy_text(pod.name.clone());
        ui.close();
    }
    item(
        ui,
        t!("Als Host speichern"),
        Action::Ctr(CtrAction::SaveHost(Box::new(pod_host(
            key, context, group, pod,
        )))),
    );
    ui.separator();
    if ui
        .button(RichText::new(t!("Pod löschen…")).color(theme::red()))
        .clicked()
    {
        actions.push(guarded(
            prod,
            true,
            t!(
                "Pod „{}“ löschen? Ein Controller startet ihn ggf. neu.",
                pod.name
            ),
            CtrAction::DeletePod {
                key,
                context: context.to_string(),
                ns: pod.namespace.clone(),
                pod: pod.name.clone(),
            },
        ));
        ui.close();
    }
}

#[allow(dead_code)]
fn unused(_: &icons::Icon) {}
