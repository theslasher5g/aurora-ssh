//! "Verlauf und Kommentare zur Sitzung": markers with notes (live and in replays), the
//! full-text search over recordings, the session history per host and the export.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Margin, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, TabKind};
use crate::theme::{self, Btn};
use crate::ui::icons::Icon;
use crate::ui::sessionlog_export::{self, Kind};
use crate::ui::sessionlog_history::{self, Session};
use crate::ui::sessionlog_index::{self, Hit, Index};
use crate::ui::sessionlog_markers::{self, Marker, clean_label};

/// Marker of a running session or replay.
#[derive(Clone, Debug)]
pub struct LiveMarker {
    /// Seconds since the session started (replays: since the recording started).
    pub t: f64,
    /// Moment of a live marker (position on the rewind timeline).
    pub at: Option<Instant>,
    /// Position in the recording file, if the session is recorded.
    pub rec_t: Option<f64>,
    pub label: String,
}

/// Markers and history data of one terminal tab.
pub struct Sess {
    pub id: String,
    pub host: Option<Uuid>,
    pub started: Instant,
    /// A replay of this recording.
    pub replay: Option<PathBuf>,
    /// The recording of the live session.
    pub rec: Option<PathBuf>,
    pub markers: Vec<LiveMarker>,
    pub panel: bool,
    /// Marker whose note field takes the focus.
    pub focus: Option<usize>,
    pub ended: bool,
}

impl Sess {
    /// Markers that are in a recording, for the sidecar file.
    fn recorded(&self) -> Vec<Marker> {
        self.markers
            .iter()
            .filter_map(|m| {
                Some(Marker {
                    t: m.rec_t?,
                    label: m.label.clone(),
                })
            })
            .collect()
    }

    /// File that stores the notes of this session (replayed or recorded file).
    pub fn file(&self) -> Option<&PathBuf> {
        self.replay.as_ref().or(self.rec.as_ref())
    }
}

#[derive(Default)]
pub struct State {
    pub sess: HashMap<u64, Sess>,
    pub query: String,
    pub index: Option<Index>,
    rx: Option<mpsc::Receiver<sessionlog_index::Msg>>,
    progress: (usize, usize),
    last_scan: Option<Instant>,
    generation: u64,
    hits: Vec<Hit>,
    hits_key: (String, u64),
    /// Host filter of the history list.
    pub history_host: Option<Uuid>,
    scroll_to_history: bool,
    exports: Vec<mpsc::Receiver<Result<PathBuf, String>>>,
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

impl State {
    // ------------------------------------------------------------ sessions

    /// A live session started: remember it and add it to the history file.
    pub fn session_start(&mut self, tab: u64, host: Option<Uuid>, name: &str) {
        let id = Uuid::new_v4().to_string();
        if host.is_some() {
            sessionlog_history::append(&sessionlog_history::Line {
                ev: "start".into(),
                id: id.clone(),
                host: host.map(|h| h.to_string()),
                name: name.to_string(),
                t: now_unix(),
                ..Default::default()
            });
        }
        self.sess.insert(
            tab,
            Sess {
                id,
                host,
                started: Instant::now(),
                replay: None,
                rec: None,
                markers: Vec::new(),
                panel: false,
                focus: None,
                ended: false,
            },
        );
    }

    /// A replay tab: markers from the recording and its sidecar.
    pub fn replay_start(&mut self, tab: u64, path: &std::path::Path) {
        let in_cast = crate::backend::record::read_full(path)
            .map(|c| c.markers)
            .unwrap_or_default();
        let markers = sessionlog_markers::load(path, &in_cast)
            .into_iter()
            .map(|m| LiveMarker {
                t: m.t,
                at: None,
                rec_t: Some(m.t),
                label: m.label,
            })
            .collect();
        self.sess.insert(
            tab,
            Sess {
                id: String::new(),
                host: None,
                started: Instant::now(),
                replay: Some(path.to_path_buf()),
                rec: None,
                markers,
                panel: false,
                focus: None,
                ended: false,
            },
        );
    }

    pub fn set_rec(&mut self, tab: u64, path: Option<PathBuf>) {
        if let Some(s) = self.sess.get_mut(&tab) {
            s.rec = path;
        }
    }

    /// The connection ended: close the history entry (the markers stay visible).
    pub fn session_end(&mut self, tab: u64) {
        let Some(s) = self.sess.get_mut(&tab) else {
            return;
        };
        if s.ended || s.replay.is_some() {
            return;
        }
        s.ended = true;
        if s.host.is_some() {
            sessionlog_history::append(&sessionlog_history::Line {
                ev: "end".into(),
                id: s.id.clone(),
                t: now_unix(),
                rec: s
                    .rec
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned()),
                markers: s.markers.len(),
                ..Default::default()
            });
        }
    }

    pub fn tab_closed(&mut self, tab: u64) {
        self.session_end(tab);
        self.sess.remove(&tab);
    }

    /// Adds a marker now (live) or at `replay_t` (replay). Writes it into a running
    /// recording. Returns false when a replay has no position.
    pub fn add_marker(
        &mut self,
        tab: u64,
        recorder: Option<&mut crate::backend::record::Recorder>,
        replay_t: Option<f64>,
    ) -> bool {
        let Some(s) = self.sess.get_mut(&tab) else {
            return false;
        };
        let label = t!("Marker").to_string();
        let marker = if s.replay.is_some() {
            let Some(t) = replay_t else { return false };
            LiveMarker {
                t,
                at: None,
                rec_t: Some(t),
                label,
            }
        } else {
            let rec_t = recorder.map(|r| {
                r.marker(&label);
                r.elapsed()
            });
            LiveMarker {
                t: s.started.elapsed().as_secs_f64(),
                at: Some(Instant::now()),
                rec_t,
                label,
            }
        };
        let t_new = marker.t;
        s.markers.push(marker);
        s.markers.sort_by(|a, b| a.t.total_cmp(&b.t));
        s.focus = s.markers.iter().position(|m| m.t == t_new);
        s.panel = true;
        if s.replay.is_some() {
            persist(s);
        }
        true
    }

    /// Marker positions on the timeline of the rewind buffer.
    pub fn timeline_marks(
        &self,
        tab: u64,
        rewind: Option<&crate::ui::timetravel::Rewind>,
        replay: bool,
    ) -> Vec<(f64, String)> {
        let Some(s) = self.sess.get(&tab) else {
            return Vec::new();
        };
        s.markers
            .iter()
            .filter_map(|m| {
                let t = if replay { m.t } else { rewind?.time_at(m.at?) };
                Some((t, m.label.clone()))
            })
            .collect()
    }

    // ------------------------------------------------------------ index

    fn ensure_scan(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        if self
            .last_scan
            .is_some_and(|t| t.elapsed() < Duration::from_secs(20))
        {
            return;
        }
        self.last_scan = Some(Instant::now());
        let c = ctx.clone();
        self.rx = Some(sessionlog_index::spawn(
            crate::backend::record::list(),
            sessionlog_index::cache_path(),
            move || c.request_repaint(),
        ));
    }

    fn poll(&mut self) -> Vec<(String, Color32)> {
        let mut toasts = Vec::new();
        if let Some(rx) = &self.rx {
            let mut done = false;
            while let Ok(m) = rx.try_recv() {
                match m {
                    sessionlog_index::Msg::Progress(d, n) => self.progress = (d, n),
                    sessionlog_index::Msg::Done(i) => {
                        self.index = Some(*i);
                        self.generation += 1;
                        done = true;
                    }
                }
            }
            if done || matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                self.rx = None;
            }
        }
        self.exports.retain(|rx| match rx.try_recv() {
            Ok(Ok(p)) => {
                toasts.push((t!("Exportiert: {}", p.display()), theme::green()));
                false
            }
            Ok(Err(e)) => {
                toasts.push((t!("Export fehlgeschlagen: {}", e), theme::red()));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        toasts
    }

    fn hits(&mut self) -> &[Hit] {
        let key = (self.query.trim().to_string(), self.generation);
        if self.hits_key != key {
            self.hits = match &self.index {
                Some(i) => sessionlog_index::search(i, &key.0, 60),
                None => Vec::new(),
            };
            self.hits_key = key;
        }
        &self.hits
    }
}

/// Stores the notes of a recorded or replayed session in the sidecar file.
fn persist(s: &Sess) {
    if let Some(path) = s.file() {
        let _ = sessionlog_markers::save_all(path, &s.recorded());
    }
}

// ------------------------------------------------------------------ actions

#[derive(Clone, Debug)]
pub enum Act {
    /// Play a recording and show it at this moment.
    OpenAt(PathBuf, f64),
    Export(PathBuf, Kind),
    /// Show the session history of a host.
    History(Uuid),
    /// Remove sessions from the history, and their recordings.
    Delete(Vec<String>, Vec<PathBuf>),
}

pub fn handle(app: &mut App, act: Act, ctx: &egui::Context) {
    match act {
        Act::OpenAt(path, t) => {
            app.play_recording(path, ctx);
            if let Some(id) = app.active
                && let Some(tab) = app.tabs.iter().find(|t| t.id == id)
                && let TabKind::Term(term) = &tab.kind
            {
                let spec = term.spec.clone();
                app.termx.toggle_rewind(id, &spec);
                if let Some(v) = app.termx.rewind_views.get_mut(&id) {
                    v.t = t;
                    v.playing = false;
                }
            }
        }
        Act::Export(path, kind) => {
            let (tx, rx) = mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let r = sessionlog_export::export(&path, kind).map_err(|e| format!("{e:#}"));
                let _ = tx.send(r);
                c.request_repaint();
            });
            app.termx.slog.exports.push(rx);
            app.toast(t!("Export läuft…"), theme::accent());
        }
        Act::Delete(ids, files) => {
            sessionlog_history::remove(&ids);
            for f in files {
                let _ = std::fs::remove_file(f);
            }
            app.toast(t!("Gelöscht"), theme::accent());
        }
        Act::History(h) => {
            app.termx.slog.history_host = Some(h);
            app.termx.slog.scroll_to_history = true;
            app.settings_tab = 1;
            app.view = crate::app::View::Settings;
        }
    }
}

/// Once per frame: collect the results of the workers.
pub fn frame(app: &mut App) {
    for (text, color) in app.termx.slog.poll() {
        app.toast(text, color);
    }
}

// ------------------------------------------------------------------ marker panel

/// What the marker panel asks the tab to do.
#[derive(Default)]
pub struct PanelOut {
    pub close: bool,
    /// Add a marker now.
    pub add: bool,
    /// Jump to this time on the timeline.
    pub seek: Option<(f64, Option<Instant>)>,
    pub export: Option<Kind>,
}

/// Side list of the markers of a session: time, editable note, delete.
pub fn panel(ui: &mut egui::Ui, tab: u64, s: &mut Sess, can_add: bool) -> PanelOut {
    let mut out = PanelOut::default();
    let mut changed = false;
    let mut delete = None;
    ui.horizontal(|ui| {
        theme::section_title(ui, t!("Marker"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_btn(ui, Icon::Close, t!("Ausblenden")).clicked() {
                out.close = true;
            }
        });
    });
    ui.add_space(4.0);
    ui.add_enabled_ui(can_add, |ui| {
        if theme::btn(
            ui,
            Btn::SmallPrimary,
            Some(Icon::Flag),
            t!("Marker setzen  (Ctrl+Shift+M)"),
        )
        .on_disabled_hover_text(t!(
            "In der Aufnahme: erst die Zeitreise öffnen und eine Stelle wählen"
        ))
        .clicked()
        {
            out.add = true;
        }
    });
    ui.add_space(6.0);
    if s.markers.is_empty() {
        ui.label(
            RichText::new(t!(
                "Noch keine Marker. Mit einem Marker merkst du dir Stellen der Sitzung samt Notiz."
            ))
            .size(12.5)
            .color(theme::text_dim()),
        );
    }
    let focus = s.focus.take();
    egui::ScrollArea::vertical()
        .id_salt(("marker-list", tab))
        .max_height((ui.available_height() - 60.0).max(80.0))
        .show(ui, |ui| {
            for (i, m) in s.markers.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let when = sessionlog_export::clock(m.t);
                    if ui
                        .add(
                            egui::Label::new(
                                RichText::new(when)
                                    .font(egui::FontId::monospace(12.0))
                                    .color(theme::accent()),
                            )
                            .sense(Sense::click()),
                        )
                        .on_hover_text(t!("Zu dieser Stelle springen"))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        out.seek = Some((m.t, m.at));
                    }
                    let w = (ui.available_width() - 30.0).max(60.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut m.label)
                            .desired_width(w)
                            .char_limit(sessionlog_markers::MAX_LABEL),
                    );
                    if focus == Some(i) {
                        r.request_focus();
                        // Select the default text: typing replaces it.
                        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(m.label.chars().count()),
                            )));
                            st.store(ui.ctx(), r.id);
                        }
                    }
                    if r.lost_focus() {
                        m.label = clean_label(&m.label);
                        changed = true;
                    }
                    if theme::icon_btn(ui, Icon::Trash, t!("Marker löschen")).clicked() {
                        delete = Some(i);
                    }
                });
            }
        });
    if let Some(i) = delete {
        s.markers.remove(i);
        changed = true;
    }
    if changed {
        persist(s);
    }
    if s.file().is_some() {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if theme::btn(ui, Btn::Small, None, t!("Als Markdown exportieren")).clicked() {
                out.export = Some(Kind::Markdown);
            }
            if theme::btn(ui, Btn::Small, None, t!("Als Text exportieren")).clicked() {
                out.export = Some(Kind::Text);
            }
        });
    }
    out
}

// ------------------------------------------------------------------ settings cards

/// One line of the session history.
#[derive(Clone, Debug, PartialEq)]
pub struct HistRow {
    /// Session id in the history file (empty for recordings the file does not know).
    pub id: String,
    pub start: i64,
    pub duration: Option<f64>,
    pub name: String,
    /// File name of the recording.
    pub rec: Option<String>,
    pub markers: usize,
}

/// Past sessions of a host (or of all hosts): the history file joined with the recordings.
pub fn build_history(
    sessions: &[Session],
    index: Option<&Index>,
    host: Option<(Uuid, &str)>,
) -> Vec<HistRow> {
    let mut rows: Vec<HistRow> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for s in sessions {
        if let Some((id, _)) = host
            && s.host.as_deref() != Some(&id.to_string())
        {
            continue;
        }
        let entry = s.rec.as_ref().and_then(|r| index?.files.get(r));
        if let Some(r) = &s.rec {
            seen.push(r.clone());
        }
        rows.push(HistRow {
            id: s.id.clone(),
            start: s.start,
            duration: s.duration().map(|d| d as f64).or(entry.map(|e| e.duration)),
            name: s.name.clone(),
            rec: s.rec.clone(),
            markers: entry.map_or(s.markers, |e| e.notes.len()),
        });
    }
    // Recordings the history file does not know (older ones, or from other tools).
    if let Some(index) = index {
        for (file, e) in &index.files {
            if seen.contains(file) {
                continue;
            }
            let ours = match host {
                None => true,
                Some((id, name)) => match &e.host {
                    Some(h) => *h == id.to_string(),
                    None => e.title.split(" · ").next() == Some(name),
                },
            };
            if ours {
                rows.push(HistRow {
                    id: String::new(),
                    start: e.start,
                    duration: Some(e.duration),
                    name: e.title.clone(),
                    rec: Some(file.clone()),
                    markers: e.notes.len(),
                });
            }
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.start));
    rows
}

fn when(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d.%m.%Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn highlighted(ui: &egui::Ui, h: &Hit) -> egui::text::LayoutJob {
    let font = egui::FontId::monospace(12.0);
    let chars: Vec<char> = h.snippet.chars().collect();
    let (a, b) = (h.range.0.min(chars.len()), h.range.1.min(chars.len()));
    let part = |r: std::ops::Range<usize>| chars[r].iter().collect::<String>();
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat::simple(font.clone(), theme::text_dim());
    let mut hot = egui::TextFormat::simple(font, theme::text());
    hot.background = theme::accent().gamma_multiply(0.35);
    job.append(&part(0..a), 0.0, plain.clone());
    job.append(&part(a..b), 0.0, hot);
    job.append(&part(b..chars.len()), 0.0, plain);
    let _ = ui;
    job
}

/// "Aufnahmen durchsuchen" and "Verlauf der Sitzungen" below the recordings list.
pub fn settings_cards(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    app.termx.slog.ensure_scan(&ctx);
    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, t!("Aufnahmen durchsuchen"));
    });
    ui.add_space(2.0);
    let dir = crate::backend::record::dir();
    let slog = &mut app.termx.slog;
    theme::settings_card(ui, |ui, rows| {
        rows.full(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut slog.query)
                    .hint_text(t!("Text in allen Aufnahmen und Markern suchen"))
                    .desired_width(f32::INFINITY),
            );
            let status = match (&slog.index, slog.rx.is_some()) {
                (_, true) => t!(
                    "Index wird aktualisiert… {} von {}",
                    slog.progress.0,
                    slog.progress.1
                ),
                (Some(i), false) => t!(
                    "{} Aufnahmen, {} Zeilen im Index",
                    i.files.len(),
                    i.line_count()
                ),
                (None, false) => String::new(),
            };
            ui.label(RichText::new(status).size(12.0).color(theme::text_dim()));
        });
        let searching = !slog.query.trim().is_empty();
        let hits: Vec<Hit> = if searching {
            slog.hits().to_vec()
        } else {
            Vec::new()
        };
        if searching && hits.is_empty() && slog.rx.is_none() {
            rows.full(ui, |ui| {
                ui.label(RichText::new(t!("Keine Treffer")).color(theme::text_dim()));
            });
        }
        for (i, h) in hits.iter().enumerate() {
            rows.full(ui, |ui| {
                let job = highlighted(ui, h);
                let head = format!(
                    "{} · {} · {}{}",
                    h.title,
                    when(h.start),
                    sessionlog_export::clock(h.t),
                    if h.is_note {
                        format!(" · {}", t!("Marker"))
                    } else {
                        String::new()
                    }
                );
                let r = ui
                    .scope(|ui| {
                        ui.label(RichText::new(head).size(12.5).color(theme::text()));
                        ui.add(egui::Label::new(job).truncate());
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(t!("Aufnahme an dieser Stelle öffnen"));
                if r.clicked() {
                    actions.push(Action::SessionLog(Act::OpenAt(dir.join(&h.file), h.t)));
                }
                let _ = i;
            });
        }
    });

    // History per host.
    ui.add_space(22.0);
    let hist_top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, t!("Verlauf der Sitzungen"));
    });
    ui.add_space(2.0);
    if std::mem::take(&mut app.termx.slog.scroll_to_history) {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(
                egui::pos2(ui.min_rect().left(), hist_top),
                Vec2::new(10.0, 200.0),
            ),
            Some(egui::Align::TOP),
        );
    }
    let hosts: Vec<(Uuid, String)> = app
        .lib
        .hosts
        .iter()
        .map(|h| (h.id, h.display_name().to_string()))
        .collect();
    let sessions = sessionlog_history::load();
    let slog = &mut app.termx.slog;
    let filter = slog.history_host.and_then(|id| {
        hosts
            .iter()
            .find(|(h, _)| *h == id)
            .map(|(h, n)| (*h, n.as_str()))
    });
    let list = build_history(&sessions, slog.index.as_ref(), filter);
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Host"),
            Some(t!("Sitzungen mit Dauer, Aufnahme und Markern")),
            |ui| {
                let label =
                    filter.map_or_else(|| t!("Alle Hosts").to_string(), |f| f.1.to_string());
                egui::ComboBox::from_id_salt("history-filter")
                    .selected_text(label)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(slog.history_host.is_none(), t!("Alle Hosts"))
                            .clicked()
                        {
                            slog.history_host = None;
                        }
                        for (id, name) in &hosts {
                            if ui
                                .selectable_label(slog.history_host == Some(*id), name)
                                .clicked()
                            {
                                slog.history_host = Some(*id);
                            }
                        }
                    });
            },
        );
        if list.is_empty() {
            rows.full(ui, |ui| {
                ui.label(RichText::new(t!("Noch keine Sitzungen")).color(theme::text_dim()));
            });
        }
        let more_id = egui::Id::new("history-more");
        let all = ui.data(|d| d.get_temp::<bool>(more_id)).unwrap_or(false);
        let stale: Vec<&HistRow> = list.iter().filter(|r| r.rec.is_none()).collect();
        if !stale.is_empty() {
            rows.full(ui, |ui| {
                if theme::btn(
                    ui,
                    Btn::Small,
                    Some(Icon::Trash),
                    &t!("Nicht aufgenommene löschen ({})", stale.len()),
                )
                .clicked()
                {
                    actions.push(Action::SessionLog(Act::Delete(
                        stale.iter().map(|r| r.id.clone()).collect(),
                        Vec::new(),
                    )));
                }
            });
        }
        for r in list.iter().take(if all { 200 } else { 12 }) {
            let dur = r
                .duration
                .map(|d| crate::ui::short_duration(Duration::from_secs_f64(d)))
                .unwrap_or_else(|| "—".into());
            let help = format!(
                "{} · {} · {}",
                when(r.start),
                dur,
                if r.rec.is_some() {
                    t!("aufgenommen").to_string()
                } else {
                    t!("nicht aufgenommen").to_string()
                }
            );
            let help = if r.markers > 0 {
                format!("{help} · {}", t!("{} Marker", r.markers))
            } else {
                help
            };
            rows.setting(ui, &r.name, Some(&help), |ui| {
                if theme::btn(ui, Btn::Small, Some(Icon::Trash), "").clicked() {
                    actions.push(Action::SessionLog(Act::Delete(
                        vec![r.id.clone()],
                        r.rec.iter().map(|f| dir.join(f)).collect(),
                    )));
                }
                if let Some(file) = &r.rec {
                    let path = dir.join(file);
                    if !path.exists() {
                        return;
                    }
                    ui.menu_button(t!("Export"), |ui| {
                        if ui.button("Markdown").clicked() {
                            actions.push(Action::SessionLog(Act::Export(
                                path.clone(),
                                Kind::Markdown,
                            )));
                            ui.close();
                        }
                        if ui.button(t!("Text")).clicked() {
                            actions.push(Action::SessionLog(Act::Export(path.clone(), Kind::Text)));
                            ui.close();
                        }
                    });
                    if theme::btn(ui, Btn::Small, Some(Icon::Play), t!("Abspielen")).clicked() {
                        actions.push(Action::PlayRecording(path));
                    }
                }
            });
        }
        if list.len() > 12 {
            rows.full(ui, |ui| {
                let label = if all {
                    t!("Weniger anzeigen").to_string()
                } else {
                    t!("Alle {} anzeigen", list.len())
                };
                if theme::btn(ui, Btn::Link, None, &label).clicked() {
                    ui.data_mut(|d| d.insert_temp(more_id, !all));
                }
            });
        }
    });
    let _ = (Margin::ZERO, CornerRadius::ZERO);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sessionlog_index::Entry;

    #[test]
    fn history_joins_file_and_recordings() {
        let host = Uuid::new_v4();
        let other = Uuid::new_v4();
        let s = |id: &str, h: Uuid, start: i64, end: Option<i64>, rec: Option<&str>| Session {
            id: id.into(),
            host: Some(h.to_string()),
            name: "web-01".into(),
            start,
            end,
            rec: rec.map(str::to_string),
            markers: 1,
        };
        let sessions = vec![
            s("a", host, 100, Some(160), Some("a.cast")),
            s("b", host, 300, None, None),
            s("c", other, 200, Some(210), None),
        ];
        let mut index = Index::default();
        let entry = |start, host: Option<String>, title: &str| Entry {
            start,
            duration: 30.0,
            host,
            title: title.into(),
            notes: vec![(1.0, "n".into()), (2.0, "m".into())],
            ..Entry::default()
        };
        index
            .files
            .insert("a.cast".into(), entry(100, None, "web-01 · root"));
        // Recording without a history line, known by the id in its header …
        index
            .files
            .insert("old1.cast".into(), entry(50, Some(host.to_string()), "x"));
        // … or only by the host name in the title.
        index
            .files
            .insert("old2.cast".into(), entry(40, None, "web-01 · root"));
        index
            .files
            .insert("other.cast".into(), entry(30, None, "db-01 · root"));
        let rows = build_history(&sessions, Some(&index), Some((host, "web-01")));
        let files: Vec<_> = rows.iter().map(|r| r.rec.as_deref()).collect();
        assert_eq!(
            files,
            [None, Some("a.cast"), Some("old1.cast"), Some("old2.cast")]
        );
        // Session "b" never ended: no duration. "a": 60 s, marker count from the recording.
        assert_eq!(rows[0].duration, None);
        assert_eq!(rows[1].duration, Some(60.0));
        assert_eq!(rows[1].markers, 2);
        // All hosts: also the other session and recording.
        assert_eq!(build_history(&sessions, Some(&index), None).len(), 6);
    }

    #[test]
    fn markers_in_live_sessions_and_replays() {
        let dir = std::env::temp_dir().join(format!("aurora-slog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cast = dir.join("r.cast");
        std::fs::write(
            &cast,
            "{\"version\":2,\"width\":20,\"height\":3}\n[1.0,\"o\",\"hi\"]\n[2.0,\"m\",\"in file\"]\n",
        )
        .unwrap();
        let mut st = State::default();
        st.replay_start(1, &cast);
        assert_eq!(st.sess[&1].markers.len(), 1);
        // A replay needs a position; adding writes the sidecar, not the cast.
        assert!(!st.add_marker(1, None, None));
        assert!(st.add_marker(1, None, Some(1.5)));
        assert_eq!(st.sess[&1].markers.len(), 2);
        let notes = sessionlog_markers::load_notes(&cast);
        assert!(notes.complete && notes.markers.len() == 2);
        assert!(std::fs::read_to_string(&cast).unwrap().contains("in file"));
        assert_eq!(std::fs::read_to_string(&cast).unwrap().lines().count(), 3);
        // Live: markers are kept in memory with their time.
        st.sess.insert(
            2,
            Sess {
                id: "x".into(),
                host: None,
                started: Instant::now(),
                replay: None,
                rec: None,
                markers: Vec::new(),
                panel: false,
                focus: None,
                ended: false,
            },
        );
        assert!(st.add_marker(2, None, None));
        assert_eq!(st.sess[&2].markers[0].label, t!("Marker"));
        assert!(st.sess[&2].markers[0].rec_t.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
