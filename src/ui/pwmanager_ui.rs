//! Password-manager integration in the app: resolving credentials right before a
//! connection (worker thread, spinner, unlock dialog, error dialog with fallback to the
//! normal password prompt), the item picker of the host / identity editor and the
//! settings card.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{RichText, Vec2};
use uuid::Uuid;

use crate::app::{App, Purpose};
use crate::backend::pwmanager_cli::{self as cli, Ctx, Sessions, Status};
use crate::backend::pwmanager_types::{
    CredentialSource, PwmError, PwmItem, PwmProvider, PwmSettings, Resolved, Secret, SourceKind,
};
use crate::model::{Host, Identity};
use crate::theme;
use crate::ui::icons::Icon;

// ---------------------------------------------------------------- searcher

type SearchMsg = (u64, Result<Vec<PwmItem>, PwmError>);

/// Debounced background search in one provider (item pickers).
#[derive(Default)]
pub struct Searcher {
    pub results: Vec<PwmItem>,
    pub error: Option<PwmError>,
    pub busy: bool,
    rx: Option<Receiver<SearchMsg>>,
    seq: u64,
    shown: Option<(PwmProvider, String)>,
    pending: Option<((PwmProvider, String), Instant)>,
    epoch: u64,
}

impl Searcher {
    /// Forget what was shown, the next `update` searches again.
    pub fn invalidate(&mut self) {
        self.shown = None;
        self.pending = None;
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok((seq, r)) = rx.try_recv() {
            if seq != self.seq {
                continue;
            }
            self.busy = false;
            match r {
                Ok(v) => {
                    self.results = v;
                    self.error = None;
                }
                Err(e) => {
                    self.results.clear();
                    self.error = Some(e);
                }
            }
        }
    }

    pub fn update(
        &mut self,
        settings: &PwmSettings,
        sessions: &Arc<Sessions>,
        provider: PwmProvider,
        query: &str,
        epoch: u64,
        ctx: &egui::Context,
    ) {
        if epoch != self.epoch {
            self.epoch = epoch;
            self.invalidate();
        }
        self.poll();
        let want = (provider, query.to_string());
        if self.shown.as_ref() == Some(&want) {
            self.pending = None;
            return;
        }
        let immediate = self.shown.as_ref().is_none_or(|s| s.0 != provider);
        let now = Instant::now();
        if self.pending.as_ref().is_none_or(|(w, _)| *w != want) {
            self.pending = Some((want.clone(), now));
        }
        let since = self.pending.as_ref().map(|p| p.1).unwrap_or(now);
        if !immediate && now.duration_since(since) < Duration::from_millis(300) {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        self.seq += 1;
        let seq = self.seq;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let c = Ctx::new(settings.clone(), sessions.clone());
        let q = query.to_string();
        let egui_ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = cli::search(&c, provider, &q);
            let _ = tx.send((seq, r));
            egui_ctx.request_repaint();
        });
        self.shown = Some(want);
        self.pending = None;
        self.busy = true;
    }
}

// ---------------------------------------------------------------- state

struct Job {
    host: Host,
    purpose: Purpose,
    provider: PwmProvider,
    rx: Receiver<Result<Vec<(Uuid, Resolved)>, PwmError>>,
}

struct UnlockDlg {
    provider: PwmProvider,
    password: String,
    error: Option<String>,
    retry: Vec<(Host, Purpose)>,
    rx: Option<Receiver<Result<(), PwmError>>>,
}

struct FailDlg {
    message: String,
    retry: Option<(Host, Purpose)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickTarget {
    Host,
    Identity,
}

struct Picker {
    target: PickTarget,
    kind: SourceKind,
    provider: PwmProvider,
    query: String,
    sel: usize,
    use_username: bool,
    search: Searcher,
}

#[derive(Default)]
pub struct PwmState {
    pub sessions: Arc<Sessions>,
    jobs: Vec<Job>,
    overrides: HashMap<Uuid, Resolved>,
    bypass: bool,
    unlock: Option<UnlockDlg>,
    fail: Option<FailDlg>,
    picker: Option<Picker>,
    statuses: HashMap<PwmProvider, Status>,
    status_rx: Option<Receiver<Vec<(PwmProvider, Status)>>>,
    status_started: bool,
    /// Bumped after every successful unlock; item lists search again.
    pub epoch: u64,
}

impl PwmState {
    /// Aurora locks: forget sessions, resolved secrets and every open dialog.
    pub fn lock(&mut self) {
        self.sessions.lock_all();
        self.overrides.clear();
        self.jobs.clear();
        self.unlock = None;
        self.fail = None;
        self.picker = None;
        self.statuses.clear();
        self.status_started = false;
    }

    /// Applies the secrets resolved for `host_id` to the identity (creating one) and
    /// returns the item's user name too.
    pub fn apply(
        &self,
        host_id: Uuid,
        ident: Option<Identity>,
    ) -> (Option<Identity>, Option<String>) {
        let Some(r) = self.overrides.get(&host_id) else {
            return (ident, None);
        };
        let mut i = ident.unwrap_or_default();
        if let Some(p) = &r.password {
            i.password = p.expose().to_string();
        }
        if let Some(k) = &r.private_key {
            i.private_key = k.expose().to_string();
        }
        if let Some(p) = &r.passphrase {
            i.passphrase = p.expose().to_string();
        }
        (Some(i), r.username.clone())
    }

    pub fn open_unlock(&mut self, provider: PwmProvider, retry: Option<(Host, Purpose)>) {
        match &mut self.unlock {
            Some(d) if d.provider == provider => d.retry.extend(retry),
            _ => {
                self.unlock = Some(UnlockDlg {
                    provider,
                    password: String::new(),
                    error: None,
                    retry: retry.into_iter().collect(),
                    rx: None,
                })
            }
        }
    }

    pub fn open_picker(
        &mut self,
        target: PickTarget,
        current: Option<&CredentialSource>,
        settings: &PwmSettings,
    ) {
        let enabled = settings.enabled_providers();
        let provider = current
            .map(|c| c.provider)
            .filter(|p| enabled.contains(p))
            .or_else(|| enabled.first().copied())
            .unwrap_or_default();
        self.picker = Some(Picker {
            target,
            kind: current.map(|c| c.kind).unwrap_or_default(),
            provider,
            query: String::new(),
            sel: 0,
            use_username: current.map(|c| c.use_username).unwrap_or(true),
            search: Searcher::default(),
        });
    }

    /// An unlock dialog is open (it owns the keyboard).
    pub fn unlock_open(&self) -> bool {
        self.unlock.is_some()
    }

    pub fn is_busy(&self) -> bool {
        !self.jobs.is_empty()
    }
}

// ---------------------------------------------------------------- connecting

/// The sources that have to be resolved before connecting to `host` (the host itself and
/// the jump hosts on its way).
fn wants_for(app: &App, host: &Host) -> Vec<(Uuid, CredentialSource)> {
    let mut hosts = vec![host.clone()];
    for id in crate::backend::route::jump_chain_lossy(&app.lib, host) {
        if let Some(h) = app.lib.host(id) {
            hosts.push(h.clone());
        }
    }
    let mut out = Vec::new();
    for h in hosts {
        let no_user = h.username.trim().is_empty();
        if let Some(s) = &h.credential_source {
            let mut s = s.clone();
            s.use_username = s.use_username && no_user;
            out.push((h.id, s));
        }
        let ident = h
            .identity_id
            .or(app.lib.inherited_defaults(&h).1)
            .and_then(|i| app.identity(i));
        if let Some(i) = ident
            && let Some(s) = &i.secret_source
        {
            let mut s = s.clone();
            s.use_username = s.use_username && no_user && i.username.trim().is_empty();
            out.push((h.id, s));
        }
    }
    out
}

impl App {
    /// Called first thing in `connect_host`: if the host (or a jump host / identity) takes
    /// credentials from a password manager, start resolving them and return `true`; the
    /// connection continues when the worker is done.
    pub fn pwm_intercept(&mut self, host: &Host, purpose: Purpose, ctx: &egui::Context) -> bool {
        if self.pwm.bypass {
            return false;
        }
        let wants = wants_for(self, host);
        let Some(first) = wants.first() else {
            return false;
        };
        let provider = first.1.provider;
        let c = Ctx::new(self.lib.settings.pwm.clone(), self.pwm.sessions.clone());
        let (tx, rx) = channel();
        let egui_ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = cli::resolve_all(&c, &wants);
            let _ = tx.send(r);
            egui_ctx.request_repaint();
        });
        self.pwm.jobs.push(Job {
            host: host.clone(),
            purpose,
            provider,
            rx,
        });
        true
    }

    /// Per frame: finished lookups continue the connection; failures open a dialog.
    pub fn pwm_poll(&mut self, ctx: &egui::Context) {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.pwm.jobs.len() {
            match self.pwm.jobs[i].rx.try_recv() {
                Ok(r) => {
                    let j = self.pwm.jobs.remove(i);
                    done.push((j.host, j.purpose, r));
                }
                Err(TryRecvError::Disconnected) => {
                    let j = self.pwm.jobs.remove(i);
                    done.push((j.host, j.purpose, Err(PwmError::Failed(None))));
                }
                Err(TryRecvError::Empty) => i += 1,
            }
        }
        for (host, purpose, r) in done {
            match r {
                Ok(list) => {
                    for (key, v) in list {
                        match self.pwm.overrides.get_mut(&key) {
                            Some(e) => e.merge(v),
                            None => {
                                self.pwm.overrides.insert(key, v);
                            }
                        }
                    }
                    self.pwm.bypass = true;
                    self.connect_host(host, purpose, ctx);
                    self.pwm.bypass = false;
                    self.pwm.overrides.clear();
                }
                Err(PwmError::Locked(p)) if p.asks_password() => {
                    self.pwm.open_unlock(p, Some((host, purpose)));
                }
                Err(e) => {
                    if self.pwm.fail.is_none() {
                        self.pwm.fail = Some(FailDlg {
                            message: e.message(),
                            retry: Some((host, purpose)),
                        });
                    } else {
                        self.toast(e.message(), theme::red());
                    }
                }
            }
        }
        if self.pwm.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

// ---------------------------------------------------------------- dialogs

/// Spinner, unlock dialog, error dialog and item picker.
pub fn show(app: &mut App, ctx: &egui::Context) {
    app.pwm_poll(ctx);
    spinner(app, ctx);
    unlock_dialog(app, ctx);
    fail_dialog(app, ctx);
    picker(app, ctx);
}

fn spinner(app: &App, ctx: &egui::Context) {
    let Some(job) = app.pwm.jobs.first() else {
        return;
    };
    egui::Area::new(egui::Id::new("pwm-spinner"))
        .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -46.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::surface())
                .stroke(egui::Stroke::new(1.0, theme::line()))
                .corner_radius(egui::CornerRadius::same(14))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new(t!("Hole Zugangsdaten aus {}…", job.provider.label()))
                                .color(theme::text()),
                        );
                    });
                });
        });
}

fn unlock_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut dlg) = app.pwm.unlock.take() else {
        return;
    };
    // Result of a running unlock.
    let mut finished = None;
    if let Some(rx) = &dlg.rx {
        match rx.try_recv() {
            Ok(r) => finished = Some(r),
            Err(TryRecvError::Disconnected) => finished = Some(Err(PwmError::Failed(None))),
            Err(TryRecvError::Empty) => {}
        }
    }
    if let Some(r) = finished {
        dlg.rx = None;
        match r {
            Ok(()) => {
                app.pwm.epoch += 1;
                app.pwm.status_started = false;
                for (host, purpose) in std::mem::take(&mut dlg.retry) {
                    app.connect_host(host, purpose, ctx);
                }
                return;
            }
            Err(e) => dlg.error = Some(e.message()),
        }
    }
    let busy = dlg.rx.is_some();
    let (mut submit, mut cancel) = (false, false);
    let p = dlg.provider;
    egui::Modal::new(egui::Id::new("pwm-unlock"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(380.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.label(theme::heading(t!("{} entsperren", p.label()), 20.0));
            ui.label(
                RichText::new(match p {
                    PwmProvider::KeePassXc => t!("Passwort der KeePass-Datenbank"),
                    _ => t!("Master-Passwort des Tresors"),
                })
                .color(theme::text_dim())
                .size(13.0),
            );
            ui.add_space(8.0);
            let r = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut dlg.password)
                    .password(true)
                    .hint_text(t!("Passwort"))
                    .font(egui::FontId::monospace(14.0))
                    .margin(egui::Margin::symmetric(10, 8))
                    .desired_width(f32::INFINITY),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            } else if !busy {
                r.request_focus();
            }
            if let Some(e) = &dlg.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(theme::red()).size(12.5));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(t!("Entsperren")).fill(theme::accent()),
                        )
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        cancel = true;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    if cancel {
        return;
    }
    if submit && !busy && !dlg.password.is_empty() {
        let secret = Secret::new(std::mem::take(&mut dlg.password));
        dlg.error = None;
        let (tx, rx) = channel();
        dlg.rx = Some(rx);
        let c = Ctx::new(app.lib.settings.pwm.clone(), app.pwm.sessions.clone());
        let egui_ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = cli::unlock(&c, p, secret);
            let _ = tx.send(r);
            egui_ctx.request_repaint();
        });
    }
    if dlg.rx.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    app.pwm.unlock = Some(dlg);
}

fn fail_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(dlg) = app.pwm.fail.as_ref() else {
        return;
    };
    let message = dlg.message.clone();
    let has_retry = dlg.retry.is_some();
    let mut answer = None;
    egui::Modal::new(egui::Id::new("pwm-fail"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(400.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.label(theme::heading(t!("Zugangsdaten nicht verfügbar"), 20.0));
            ui.add_space(6.0);
            ui.label(RichText::new(&message).color(theme::text()));
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if has_retry && theme::primary_button(ui, t!("Passwort eingeben")).clicked() {
                    answer = Some(1);
                }
                if has_retry && ui.button(t!("Erneut versuchen")).clicked() {
                    answer = Some(2);
                }
                if ui.button(t!("Abbrechen")).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    answer = Some(0);
                }
            });
        });
    let Some(a) = answer else { return };
    let dlg = app.pwm.fail.take();
    let Some((host, purpose)) = dlg.and_then(|d| d.retry) else {
        return;
    };
    match a {
        1 => {
            // Normal password prompt (without the manager source).
            let mut h = host;
            h.credential_source = None;
            if app.password_prompt.is_some() {
                app.connect_queue.push_back((purpose, h));
            } else {
                app.password_prompt = Some(crate::app::PasswordPrompt {
                    host: h,
                    purpose,
                    password: String::new(),
                    save: false,
                    pick: Default::default(),
                });
            }
        }
        2 => app.connect_host(host, purpose, ctx),
        _ => {
            if let Purpose::Tunnel(tid) = purpose {
                app.tunnels.remove(&tid);
            }
        }
    }
}

// ---------------------------------------------------------------- list widgets

/// One two-line row of an item list: title, dim subtitle, optional badge on the right.
pub fn item_row(ui: &mut egui::Ui, title: &str, sub: &str, selected: bool) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 40.0), egui::Sense::click());
    let bg = if selected {
        theme::accent_soft()
    } else if resp.hovered() {
        theme::hover()
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(8), bg);
    let text_w = (w - 20.0).max(40.0);
    let line = |text: &str, size: f32, color: egui::Color32| {
        let mut job = egui::text::LayoutJob::simple(
            text.to_string(),
            egui::FontId::proportional(size),
            color,
            text_w,
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        ui.painter().layout_job(job)
    };
    let t = line(title, 14.0, theme::text());
    let s = line(sub, 12.0, theme::text_dim());
    let total = t.size().y
        + if sub.is_empty() {
            0.0
        } else {
            s.size().y + 1.0
        };
    let top = rect.top() + ((40.0 - total) / 2.0).max(1.0);
    let th = t.size().y;
    ui.painter()
        .galley(egui::pos2(rect.left() + 10.0, top), t, theme::text());
    if !sub.is_empty() {
        ui.painter().galley(
            egui::pos2(rect.left() + 10.0, top + th + 1.0),
            s,
            theme::text_dim(),
        );
    }
    resp
}

/// Subtitle `user · folder` of a search hit.
pub fn item_sub(i: &PwmItem) -> String {
    match (i.username.is_empty(), i.folder.is_empty()) {
        (false, false) => format!("{} · {}", i.username, i.folder),
        (false, true) => i.username.clone(),
        (true, false) => i.folder.clone(),
        (true, true) => String::new(),
    }
}

/// Message and (for lockable providers) an unlock button under an item list.
/// Returns `Some(provider)` when the user wants to unlock.
pub fn search_status(
    ui: &mut egui::Ui,
    s: &Searcher,
    provider: PwmProvider,
) -> Option<PwmProvider> {
    let mut unlock = None;
    if s.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(t!("Suche…")).color(theme::text_dim()));
        });
    }
    if let Some(e) = &s.error {
        ui.label(RichText::new(e.message()).color(theme::amber()).size(12.5));
        if matches!(e, PwmError::Locked(_))
            && provider.asks_password()
            && theme::btn(ui, theme::Btn::Small, Some(Icon::Lock), t!("Entsperren")).clicked()
        {
            unlock = Some(provider);
        }
    }
    unlock
}

// ---------------------------------------------------------------- editor rows

/// Row of the host / identity editor: the chosen manager item with change / remove
/// buttons. Returns `true` when the picker should open.
pub fn source_row(ui: &mut egui::Ui, source: &mut Option<CredentialSource>) -> bool {
    let mut open = false;
    let mut remove = false;
    ui.horizontal(|ui| match source.as_ref() {
        Some(s) => {
            let name = if s.label.is_empty() {
                &s.item
            } else {
                &s.label
            };
            let kind = match s.kind {
                SourceKind::Password => "",
                SourceKind::PrivateKey => " · SSH-Key",
            };
            let text = format!("{} · {}{kind}", s.provider.label(), name);
            let w = (ui.available_width() - 150.0).max(60.0);
            ui.add_sized(
                [w, 22.0],
                egui::Label::new(RichText::new(text).color(theme::text()))
                    .truncate()
                    .selectable(false),
            );
            if theme::btn(ui, theme::Btn::Small, None, t!("Ändern")).clicked() {
                open = true;
            }
            if theme::icon_btn(ui, Icon::Close, t!("Entfernen")).clicked() {
                remove = true;
            }
        }
        None => {
            if theme::btn(
                ui,
                theme::Btn::Small,
                Some(Icon::Key),
                t!("Aus Passwort-Manager…"),
            )
            .clicked()
            {
                open = true;
            }
        }
    });
    if remove {
        *source = None;
    }
    open
}

// ---------------------------------------------------------------- picker (editors)

fn picker(app: &mut App, ctx: &egui::Context) {
    let Some(mut pk) = app.pwm.picker.take() else {
        return;
    };
    let settings = app.lib.settings.pwm.clone();
    let providers = settings.enabled_providers();
    if !providers.contains(&pk.provider)
        && let Some(p) = providers.first()
    {
        pk.provider = *p;
    }
    if !providers.is_empty() {
        pk.search.update(
            &settings,
            &app.pwm.sessions,
            pk.provider,
            &pk.query,
            app.pwm.epoch,
            ctx,
        );
    }
    let n = pk.search.results.len();
    // The unlock dialog on top owns the keyboard.
    let covered = app.pwm.unlock.is_some();
    pk.sel = pk.sel.min(n.saturating_sub(1));
    let (mut cancel, mut choose, mut unlock) = (false, None::<usize>, None);
    egui::Modal::new(egui::Id::new("pwm-picker"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.label(theme::heading(t!("Passwort aus Passwort-Manager"), 20.0));
            ui.add_space(8.0);
            if providers.is_empty() {
                ui.label(
                    RichText::new(t!(
                        "Kein Passwort-Manager aktiviert. Unter Einstellungen → Sicherheit → Passwort-Manager einschalten."
                    ))
                    .color(theme::amber()),
                );
                ui.add_space(10.0);
                if ui.button(t!("Schließen")).clicked() {
                    cancel = true;
                }
                return;
            }
            if providers.len() > 1 {
                let labels: Vec<&str> = providers.iter().map(|p| p.label()).collect();
                let cur = providers.iter().position(|p| *p == pk.provider).unwrap_or(0);
                if let Some(i) = theme::segmented(ui, &labels, cur) {
                    pk.provider = providers[i];
                    pk.sel = 0;
                }
                ui.add_space(8.0);
            }
            if n > 0 && !covered {
                ui.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                        pk.sel = crate::ui::pwmanager_pick::step(pk.sel, n, 1);
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                        pk.sel = crate::ui::pwmanager_pick::step(pk.sel, n, -1);
                    }
                });
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut pk.query)
                    .hint_text(t!("Eintrag suchen…"))
                    .margin(egui::Margin::symmetric(10, 8))
                    .desired_width(f32::INFINITY),
            );
            if r.changed() {
                pk.sel = 0;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if n > 0 {
                    choose = Some(pk.sel);
                }
            } else if !covered {
                r.request_focus();
            }
            ui.add_space(6.0);
            let max_h = (ctx.content_rect().height() - 380.0).clamp(80.0, 260.0);
            egui::ScrollArea::vertical()
                .max_height(max_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, it) in pk.search.results.iter().enumerate() {
                        let resp = item_row(ui, &it.name, &item_sub(it), i == pk.sel);
                        if i == pk.sel && ui.input(|i| i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::ArrowUp)) {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() {
                            pk.sel = i;
                        }
                        if resp.double_clicked() {
                            choose = Some(i);
                        }
                    }
                    if n == 0 && !pk.search.busy && pk.search.error.is_none() {
                        ui.label(RichText::new(t!("Keine Treffer")).color(theme::text_dim()));
                    }
                });
            unlock = search_status(ui, &pk.search, pk.provider);
            ui.add_space(6.0);
            if pk.target == PickTarget::Identity {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t!("Verwenden als")).color(theme::text_dim()));
                    let cur = usize::from(pk.kind == SourceKind::PrivateKey);
                    if let Some(i) = theme::segmented(ui, &[t!("Passwort"), t!("SSH-Key (Notiz/Feld)")], cur) {
                        pk.kind = if i == 1 { SourceKind::PrivateKey } else { SourceKind::Password };
                    }
                });
            } else {
                ui.checkbox(
                    &mut pk.use_username,
                    t!("Benutzernamen des Eintrags verwenden, wenn der Host keinen hat"),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(n > 0, egui::Button::new(t!("Übernehmen")).fill(theme::accent()))
                        .clicked()
                    {
                        choose = Some(pk.sel);
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        cancel = true;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    if let Some(p) = unlock {
        app.pwm.open_unlock(p, None);
    }
    if cancel {
        return;
    }
    if let Some(i) = choose
        && let Some(item) = pk.search.results.get(i).cloned()
    {
        let src = CredentialSource {
            provider: pk.provider,
            item: item.id,
            label: item.name,
            kind: pk.kind,
            use_username: pk.use_username,
        };
        match pk.target {
            PickTarget::Host => {
                if let Some(ed) = app.host_editor.as_mut() {
                    ed.host.credential_source = Some(src);
                }
            }
            PickTarget::Identity => {
                if let Some((ident, _, _)) = app.identity_editor.as_mut() {
                    ident.secret_source = Some(src);
                }
            }
        }
        return;
    }
    app.pwm.picker = Some(pk);
}

// ---------------------------------------------------------------- settings card

fn start_checks(app: &mut App, ctx: &egui::Context) {
    app.pwm.status_started = true;
    let settings = app.lib.settings.pwm.clone();
    let sessions = app.pwm.sessions.clone();
    let (tx, rx) = channel();
    app.pwm.status_rx = Some(rx);
    let egui_ctx = ctx.clone();
    std::thread::spawn(move || {
        let c = Ctx::new(settings, sessions);
        let v: Vec<_> = c
            .settings
            .enabled_providers()
            .into_iter()
            .map(|p| (p, cli::status(&c, p)))
            .collect();
        let _ = tx.send(v);
        egui_ctx.request_repaint();
    });
}

fn status_color(s: &Status) -> egui::Color32 {
    match s {
        Status::Ready => theme::green(),
        Status::Locked | Status::LoggedOut => theme::amber(),
        Status::NotInstalled | Status::Error(_) => theme::red(),
        Status::Disabled => theme::text_dim(),
    }
}

fn path_row(ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(120.0, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add(
                    egui::Label::new(RichText::new(label).color(theme::text_dim()).size(13.0))
                        .truncate(),
                );
            },
        );
        ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(RichText::new(hint).color(theme::text_dim()))
                .desired_width(f32::INFINITY),
        )
    })
    .inner
}

/// The "Passwort-Manager" card in Settings → Sicherheit.
pub fn settings_card(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    let ctx = ui.ctx().clone();
    if let Some(rx) = &app.pwm.status_rx
        && let Ok(v) = rx.try_recv()
    {
        app.pwm.statuses = v.into_iter().collect();
        app.pwm.status_rx = None;
    }
    let any = !app.lib.settings.pwm.enabled_providers().is_empty();
    if any && !app.pwm.status_started {
        start_checks(app, &ctx);
    }
    let checking = app.pwm.status_rx.is_some();
    ui.label(
        RichText::new(t!("Passwort-Manager").to_uppercase())
            .size(11.5)
            .color(theme::text_dim()),
    );
    ui.add_space(6.0);
    let mut recheck = false;
    let mut lock_all = false;
    theme::settings_card(ui, |ui, rows| {
        for p in PwmProvider::ALL {
            let status = app.pwm.statuses.get(&p).cloned();
            let enabled = app.lib.settings.pwm.enabled(p);
            let help = match p {
                PwmProvider::Bitwarden => {
                    t!("Befehlszeile „bw“ – Tresor mit Master-Passwort entsperren")
                }
                PwmProvider::OnePassword => t!("Befehlszeile „op“ – App-Anbindung / Biometrie"),
                PwmProvider::KeePassXc => t!("Befehlszeile „keepassxc-cli“ – Datenbank-Passwort"),
                PwmProvider::ProtonPass => t!("Befehlszeile „pass-cli“ – vorher „pass-cli login“"),
            };
            rows.setting(ui, p.label(), Some(help), |ui| {
                let mut on = enabled;
                if theme::toggle(ui, &mut on).changed() {
                    match p {
                        PwmProvider::Bitwarden => app.lib.settings.pwm.bw_enabled = on,
                        PwmProvider::OnePassword => app.lib.settings.pwm.op_enabled = on,
                        PwmProvider::KeePassXc => app.lib.settings.pwm.kp_enabled = on,
                        PwmProvider::ProtonPass => app.lib.settings.pwm.proton_enabled = on,
                    }
                    *changed = true;
                    recheck = true;
                }
                if enabled {
                    let (text, color) = match (&status, checking) {
                        (_, true) => (t!("prüfe…").to_string(), theme::text_dim()),
                        (Some(s), _) => (s.label(), status_color(s)),
                        (None, _) => ("…".to_string(), theme::text_dim()),
                    };
                    let r = theme::pill(ui, &text, color);
                    if let Some(Status::Error(e)) = &status {
                        r.on_hover_text(e.message());
                    }
                }
            });
            if enabled {
                rows.full(ui, |ui| {
                    let s = &mut app.lib.settings.pwm;
                    let (path, hint) = match p {
                        PwmProvider::Bitwarden => (&mut s.bw_path, "bw"),
                        PwmProvider::OnePassword => (&mut s.op_path, "op"),
                        PwmProvider::KeePassXc => (&mut s.kp_path, "keepassxc-cli"),
                        PwmProvider::ProtonPass => (&mut s.proton_path, "pass-cli"),
                    };
                    let r = path_row(ui, t!("Programm"), path, &t!("leer = aus PATH ({})", hint));
                    if r.changed() {
                        *changed = true;
                    }
                    if r.lost_focus() {
                        recheck = true;
                    }
                    if p == PwmProvider::OnePassword {
                        let r = path_row(
                            ui,
                            t!("Konto"),
                            &mut s.op_account,
                            t!("leer = Standardkonto"),
                        );
                        *changed |= r.changed();
                        if r.lost_focus() {
                            recheck = true;
                        }
                    }
                    if p == PwmProvider::KeePassXc {
                        let r = path_row(ui, t!("Datenbank"), &mut s.kp_db, "~/Passwoerter.kdbx");
                        *changed |= r.changed();
                        if r.lost_focus() {
                            recheck = true;
                        }
                        let r =
                            path_row(ui, t!("Schlüsseldatei"), &mut s.kp_keyfile, t!("optional"));
                        *changed |= r.changed();
                        if r.lost_focus() {
                            recheck = true;
                        }
                    }
                });
                // Unlock right from the card.
                if p.asks_password() && matches!(status, Some(Status::Locked)) {
                    rows.full(ui, |ui| {
                        if theme::btn(ui, theme::Btn::Small, Some(Icon::Lock), t!("Entsperren"))
                            .clicked()
                        {
                            app.pwm.open_unlock(p, None);
                        }
                    });
                }
            }
        }
        rows.setting(
            ui,
            t!("Sitzung vergessen"),
            Some(t!(
                "Entsperrte Sitzungen nur im Speicher; nach Leerlauf verworfen"
            )),
            |ui| {
                let m = app.lib.settings.pwm.timeout_minutes;
                let text = |m: u32| {
                    if m == 0 {
                        t!("beim Sperren von Aurora").to_string()
                    } else {
                        t!("nach {} min", m)
                    }
                };
                egui::ComboBox::from_id_salt("pwm-timeout")
                    .selected_text(text(m))
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for v in [5, 15, 30, 60, 0] {
                            *changed |= ui
                                .selectable_value(
                                    &mut app.lib.settings.pwm.timeout_minutes,
                                    v,
                                    text(v),
                                )
                                .changed();
                        }
                    });
            },
        );
        rows.full(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(Icon::Refresh),
                    t!("Verbindung testen"),
                )
                .clicked()
                {
                    recheck = true;
                }
                if theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(Icon::Lock),
                    t!("Sitzungen sperren"),
                )
                .clicked()
                {
                    lock_all = true;
                }
            });
        });
    });
    if lock_all {
        app.pwm.sessions.lock_all();
        recheck = true;
    }
    if recheck {
        app.pwm.status_started = false;
        app.pwm.statuses.clear();
        if app.lib.settings.pwm.enabled_providers().is_empty() {
            app.pwm.status_started = true;
        }
    }
}
