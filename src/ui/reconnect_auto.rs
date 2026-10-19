//! Automatic reconnect of dropped SSH terminals: bookkeeping per tab, the tick that starts
//! the next attempt, the banner above the terminal and the settings widgets.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Margin, RichText};

use crate::app::{App, ConnState, ConnectSpec, TabKind, TermTab};
use crate::backend::reconnect_policy::{self as policy, Ended, Poll, Proto, Retry, Verdict};
use crate::model::ReconnectMode;
use crate::theme;

/// Seconds on the monotonic clock since the first call.
pub fn mono() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn wall() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Is the server reachable at all? Probed with plain TCP connects while we wait.
struct Probe {
    /// 0 unknown, 1 down, 2 up
    state: Arc<AtomicU8>,
    running: Arc<AtomicBool>,
    last: f64,
    /// A probe failed since the drop: a later success means the network is back.
    saw_down: bool,
}

const PROBE_EVERY: f64 = 3.0;

impl Probe {
    fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            last: f64::MIN,
            saw_down: false,
        }
    }

    /// Starts a probe when due; true once the target answers after having been down.
    fn step(
        &mut self,
        rt: &tokio::runtime::Runtime,
        ctx: &egui::Context,
        now: f64,
        target: Option<(String, u16)>,
    ) -> bool {
        let Some((host, port)) = target else {
            return false;
        };
        match self.state.load(Ordering::Relaxed) {
            1 => self.saw_down = true,
            2 if self.saw_down => {
                self.saw_down = false;
                self.state.store(0, Ordering::Relaxed);
                return true;
            }
            _ => {}
        }
        if now - self.last >= PROBE_EVERY && !self.running.swap(true, Ordering::Relaxed) {
            self.last = now;
            let (state, running, ctx) = (self.state.clone(), self.running.clone(), ctx.clone());
            rt.spawn(async move {
                let up = tokio::time::timeout(
                    Duration::from_secs(2),
                    tokio::net::TcpStream::connect((host.as_str(), port)),
                )
                .await
                .is_ok_and(|r| r.is_ok());
                state.store(if up { 2 } else { 1 }, Ordering::Relaxed);
                running.store(false, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
        false
    }
}

/// Reconnect state of one terminal tab (present from the drop until it is back or given up).
pub struct Auto {
    retry: Retry,
    last_error: Option<String>,
    probe: Probe,
    /// Banner buttons.
    want_now: bool,
    want_cancel: bool,
}

impl Auto {
    fn new(now: f64, minutes: u32, err: Option<String>) -> Self {
        Self {
            retry: Retry::new(now, minutes),
            last_error: err,
            probe: Probe::new(),
            want_now: false,
            want_cancel: false,
        }
    }
}

fn proto_of(spec: &ConnectSpec) -> Proto {
    match spec {
        ConnectSpec::Ssh(_) => Proto::Ssh,
        ConnectSpec::Telnet(_) => Proto::Telnet,
        _ => Proto::Other,
    }
}

/// Host and port the first hop of this session talks to.
fn target(spec: &ConnectSpec) -> Option<(String, u16)> {
    match spec {
        ConnectSpec::Ssh(p) => {
            let mut p = p;
            while let Some(j) = p.jump.as_deref() {
                p = j;
            }
            Some((p.host.clone(), p.port))
        }
        ConnectSpec::Telnet(p) => Some((p.host.clone(), p.port)),
        _ => None,
    }
}

fn feed_line(t: &mut TermTab, color: &str, text: &str) {
    t.term
        .feed(format!("\r\n\x1b[{color}m{text}\x1b[0m\r\n").as_bytes());
}

/// Called when a session ended. Returns true if automatic reconnecting took over (the caller
/// then must not print the usual "closed" message or mark the tab closed).
pub fn handle_closed(t: &mut TermTab, err: &Option<String>, on: bool, minutes: u32) -> bool {
    let ended = Ended {
        proto: proto_of(&t.spec),
        was_connected: matches!(t.state, ConnState::Connected),
        retrying: t.auto.is_some(),
        dropped: std::mem::take(&mut t.dropped),
        had_error: err.is_some(),
    };
    let verdict = policy::decide(on, ended);
    let now = mono();
    match verdict {
        Verdict::Retry => {
            t.cmd_started = None;
            t.upload = None;
            match t.auto.as_mut() {
                Some(a) => {
                    a.retry.attempt_failed(now);
                    a.last_error = err.clone();
                }
                None => {
                    t.auto = Some(Box::new(Auto::new(now, minutes, err.clone())));
                    t.term.reset_modes();
                    let at = chrono::Local::now().format("%H:%M:%S");
                    feed_line(t, "33", &t!("─── Verbindung verloren ({}) ───", at));
                }
            }
            t.state = ConnState::Connecting(t!("Warte auf neuen Versuch").into());
            true
        }
        Verdict::Fatal => {
            t.auto = None;
            feed_line(
                t,
                "2",
                t!(
                    "Automatisches Wiederverbinden beendet – der Fehler lässt sich nicht durch Warten beheben."
                ),
            );
            false
        }
        _ => {
            t.auto = None;
            false
        }
    }
}

/// Puts the host keys accepted so far into the connection parameters (and their jump hosts).
pub fn refresh_known(
    p: &mut crate::backend::ssh::SshParams,
    known: &std::collections::HashMap<String, String>,
) {
    if let Some(fp) = known.get(&p.host_id()) {
        p.known_fingerprint = Some(fp.clone());
    }
    if let Some(j) = p.jump.as_deref_mut() {
        refresh_known(j, known);
    }
}

/// The session is up again.
pub fn on_connected(t: &mut TermTab) {
    if let Some(a) = t.auto.take() {
        let secs = a.retry.down_for(mono()).round() as u64;
        feed_line(t, "32", &t!("✔ wieder verbunden nach {} s", secs));
    }
}

impl App {
    /// Once per frame: wake-up detection, probes, the next attempt, give-up and cancel.
    pub fn tick_auto_reconnect(&mut self, ctx: &egui::Context) {
        let (m, w) = (mono(), wall());
        let woke = ctx.data_mut(|d| {
            d.get_temp_mut_or_insert_with(egui::Id::new("sleep-detector"), || {
                policy::SleepDetector::new(m, w)
            })
            .tick(m, w)
        });
        let mut watching = false;
        let mut due = Vec::new();
        let mut gave_up = Vec::new();
        let mut cancelled = Vec::new();
        for tab in &mut self.tabs {
            let TabKind::Term(t) = &mut tab.kind else {
                continue;
            };
            if matches!(t.state, ConnState::Connected) && proto_of(&t.spec) == Proto::Ssh {
                watching = true;
            }
            let tg = target(&t.spec);
            let Some(a) = t.auto.as_mut() else {
                continue;
            };
            watching = true;
            if std::mem::take(&mut a.want_cancel) {
                cancelled.push(tab.id);
                continue;
            }
            if woke {
                a.retry.wake(m);
            }
            if std::mem::take(&mut a.want_now) || a.probe.step(&self.rt, ctx, m, tg) {
                a.retry.retry_now(m);
            }
            match a.retry.poll(m) {
                Poll::Attempt => due.push(tab.id),
                Poll::GiveUp => gave_up.push(tab.id),
                Poll::Wait(r) => {
                    ctx.request_repaint_after(Duration::from_secs_f64(r.clamp(0.05, 0.5)))
                }
                Poll::Busy => ctx.request_repaint_after(Duration::from_millis(500)),
            }
        }
        if watching {
            // Cheap heartbeat so a sleeping laptop is noticed within seconds of waking.
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        for id in cancelled.into_iter().chain(gave_up.iter().copied()) {
            let was_cancel = !gave_up.contains(&id);
            let mut toast = None;
            if let Some(tab) = self.tabs.iter_mut().find(|x| x.id == id)
                && let TabKind::Term(t) = &mut tab.kind
            {
                let err = t.auto.take().and_then(|a| a.last_error);
                let msg = if was_cancel {
                    t!("Automatisches Wiederverbinden abgebrochen – Enter verbindet neu.")
                        .to_string()
                } else {
                    t!(
                        "Aufgegeben nach {} Min. – Enter verbindet neu.",
                        self.lib.settings.auto_reconnect_minutes.max(1)
                    )
                };
                feed_line(t, if was_cancel { "2" } else { "31" }, &msg);
                t.state = ConnState::Closed(if was_cancel {
                    None
                } else {
                    Some(err.unwrap_or_else(|| msg.to_string()))
                });
                if !was_cancel {
                    toast = Some(format!("{}: {msg}", tab.title));
                }
            }
            if let Some(m) = toast {
                self.toast(m, theme::red());
            }
        }
        for id in due {
            self.reconnect_inner(id, ctx, true);
        }
    }
}

/// Banner above the terminal while waiting for the next attempt.
pub fn banner(ui: &mut egui::Ui, tab_id: u64, a: &mut Auto) {
    let now = mono();
    let text = policy::banner_text(&a.retry, a.retry.remaining(now));
    egui::Panel::top(egui::Id::new(("term-reconnect", tab_id)))
        .frame(
            egui::Frame::NONE
                .fill(theme::lerp(theme::surface(), theme::amber(), 0.16))
                .inner_margin(Margin::symmetric(14, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                ui.label(RichText::new("⟳").size(15.0).color(theme::amber()));
                ui.label(RichText::new(text).size(13.0).color(theme::text()));
                if let Some(e) = &a.last_error {
                    let short: String = e.chars().take(80).collect();
                    ui.label(RichText::new(short).size(12.0).color(theme::text_dim()))
                        .on_hover_text(e);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::btn(ui, theme::Btn::Small, None, t!("Abbrechen")).clicked() {
                        a.want_cancel = true;
                    }
                    if a.retry.is_waiting()
                        && theme::btn(ui, theme::Btn::SmallPrimary, None, t!("Jetzt verbinden"))
                            .clicked()
                    {
                        a.want_now = true;
                    }
                });
            });
        });
    if a.retry.is_waiting() {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
}

/// Per-host override in the host editor (inside a `theme::form`).
pub fn host_row(ui: &mut egui::Ui, f: &mut theme::Rows, mode: &mut ReconnectMode) {
    f.field(ui, t!("Auto-Reconnect"), |ui| {
        let labels = [t!("Standard"), t!("An"), t!("Aus")];
        let cur = match mode {
            ReconnectMode::Default => 0,
            ReconnectMode::On => 1,
            ReconnectMode::Off => 2,
        };
        if let Some(i) = theme::segmented(ui, &labels, cur) {
            *mode = [
                ReconnectMode::Default,
                ReconnectMode::On,
                ReconnectMode::Off,
            ][i];
        }
    });
}

/// Settings card rows: switch and maximum waiting time.
pub fn settings_rows(app: &mut App, ui: &mut egui::Ui, rows: &mut theme::Rows, changed: &mut bool) {
    rows.setting(
        ui,
        t!("Automatisch wiederverbinden"),
        Some(t!(
            "SSH-Sessions, deren Verbindung abbricht (Netz weg, Standby), verbinden sich selbst wieder – mit Scrollback, tmux/screen und ohne neue Passwortabfrage"
        )),
        |ui| {
            *changed |= theme::toggle(ui, &mut app.lib.settings.auto_reconnect).changed();
        },
    );
    rows.setting(
        ui,
        t!("Höchstens so lange versuchen"),
        Some(t!("Danach gibt Aurora auf; Enter verbindet dann von Hand")),
        |ui| {
            let mut m = app.lib.settings.auto_reconnect_minutes;
            let r = ui.add(
                egui::DragValue::new(&mut m)
                    .range(1..=240)
                    .suffix(format!(" {}", t!("Min."))),
            );
            if r.changed() {
                app.lib.settings.auto_reconnect_minutes = m;
                *changed = true;
            }
        },
    );
}
