//! Terminal extras that live next to the tabs: the command history ("Befehls-Gedächtnis")
//! with its Ctrl+R popup, palette entries and snippet suggestions, the pending paste of
//! the secret guard, and the time travel buffers.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{CornerRadius, Margin, RichText, Stroke, Vec2};
use uuid::Uuid;

use crate::app::{Action, App, ConnState, ConnectSpec, Purpose, TabKind};
use crate::backend::TermCmd;
use crate::history::{self, History};
use crate::theme::{self, Btn};
use crate::ui::icons::Icon;
use crate::ui::paste_guard::PendingPaste;
use crate::ui::timetravel::{Rewind, RewindView};

/// Ctrl+R popup of one terminal.
#[derive(Default)]
pub struct HistPopup {
    pub query: String,
    pub selected: usize,
    /// The prompt line had text when the popup opened: clear it before inserting.
    pub clear_line: bool,
    focus: bool,
}

/// A snippet suggestion shown as a hint.
pub struct Suggestion {
    pub key: String,
    pub template: String,
    pub runs: u32,
    pub since: Instant,
}

#[derive(Default)]
pub struct TermExtras {
    pub history: History,
    loaded: bool,
    key_id: u64,
    dirty: bool,
    last_save: Option<Instant>,
    /// Tab id → open Ctrl+R popup.
    pub popups: HashMap<u64, HistPopup>,
    pub suggestion: Option<Suggestion>,
    /// Command to type (without Enter) into the next session of a host.
    pub pending_type: Option<(Uuid, String, Instant)>,
    pub paste: Option<PendingPaste>,
    /// Tab id → recent output.
    pub rewinds: HashMap<u64, Rewind>,
    /// Tab id → open rewind overlay.
    pub rewind_views: HashMap<u64, RewindView>,
}

impl TermExtras {
    /// Stores output of a terminal tab before it is fed to the terminal.
    pub fn record_output(
        &mut self,
        enabled: bool,
        tab: u64,
        term: &crate::terminal::Terminal,
        data: &[u8],
    ) {
        if enabled {
            self.rewinds
                .entry(tab)
                .or_insert_with(Rewind::live)
                .push(term, data);
        }
    }

    /// Opens or closes the rewind overlay of a tab. Replay tabs scrub the whole recording.
    pub fn toggle_rewind(&mut self, tab: u64, spec: &ConnectSpec) {
        if self.rewind_views.remove(&tab).is_some() {
            return;
        }
        let own = match spec {
            ConnectSpec::Replay(path) => crate::backend::record::read(path)
                .ok()
                .map(|(w, h, frames)| Rewind::from_cast(w, h, &frames)),
            _ => None,
        };
        let view = RewindView::new(self.rewinds.get(&tab), own);
        self.rewind_views.insert(tab, view);
    }

    /// Writes the history now (before locking and on exit) and forgets it when `forget`.
    pub fn flush(&mut self, vault: Option<&crate::store::Vault>, forget: bool) {
        if let Some(v) = vault
            && self.loaded
            && self.dirty
            && history::save(v, &self.history).is_ok()
        {
            self.dirty = false;
        }
        if forget {
            self.history = History::default();
            self.loaded = false;
            self.dirty = false;
            self.popups.clear();
            self.suggestion = None;
            self.pending_type = None;
            self.paste = None;
        }
    }
}

/// Once per frame while unlocked: load/save the history, type pending commands, drop the
/// state of closed tabs, and draw the snippet hint and the paste dialog.
pub fn frame(app: &mut App, ctx: &egui::Context) {
    let Some(vault) = app.vault.as_ref() else {
        return;
    };
    let x = &mut app.termx;
    if !x.loaded {
        x.loaded = true;
        x.key_id = vault.key_id();
        match history::load(vault) {
            Ok(h) => x.history = h,
            Err(e) => {
                log::warn!("history: {e:#}");
                x.history = History::default();
            }
        }
    }
    if x.key_id != vault.key_id() {
        // New master password: write the history with the new key.
        x.key_id = vault.key_id();
        x.dirty = true;
        x.last_save = None;
    }
    if x.dirty
        && x.last_save
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
    {
        if let Err(e) = history::save(vault, &x.history) {
            log::warn!("history: {e:#}");
        }
        x.dirty = false;
        x.last_save = Some(Instant::now());
    }
    if x.dirty {
        ctx.request_repaint_after(Duration::from_secs(3));
    }
    if !app.lib.settings.time_travel {
        x.rewinds.clear();
        x.rewind_views.clear();
    }
    let ids: Vec<u64> = app.tabs.iter().map(|t| t.id).collect();
    x.rewinds.retain(|id, _| ids.contains(id));
    x.rewind_views.retain(|id, _| ids.contains(id));
    x.popups.retain(|id, _| ids.contains(id));
    if x.paste.as_ref().is_some_and(|p| !ids.contains(&p.tab)) {
        x.paste = None;
    }
    type_pending(app);
    crate::ui::paste_guard::dialog(app, ctx);
    suggestion_hint(app, ctx);
}

/// Types the command picked in the palette once the new session shows a prompt.
fn type_pending(app: &mut App) {
    let Some((host, cmd, since)) = app.termx.pending_type.clone() else {
        return;
    };
    if since.elapsed() > Duration::from_secs(60) {
        app.termx.pending_type = None;
        return;
    }
    let ready = app.tabs.iter().rev().find_map(|t| match &t.kind {
        TabKind::Term(term)
            if t.host_id == Some(host)
                && matches!(term.state, ConnState::Connected)
                && term.term.prompt_command().is_some_and(|c| c.is_empty()) =>
        {
            Some(t.id)
        }
        _ => None,
    });
    if let Some(id) = ready {
        app.termx.pending_type = None;
        type_into(app, id, &cmd, false);
    }
}

/// Sends `cmd` to a terminal tab like typed input; with `run` also Enter.
fn type_into(app: &mut App, tab: u64, cmd: &str, run: bool) {
    let mut bytes = cmd.as_bytes().to_vec();
    if run {
        bytes.push(b'\r');
    }
    if let Some(TabKind::Term(t)) = app.tabs.iter().find(|t| t.id == tab).map(|t| &t.kind) {
        let _ = t.tx.send(TermCmd::Input(bytes));
    }
    app.active = Some(tab);
    app.focus_term = Some(tab);
}

/// Palette entry picked: type the command into a session of the host (opened if needed),
/// without Enter.
pub fn type_command(app: &mut App, host: Uuid, cmd: String, ctx: &egui::Context) {
    let live = |t: &crate::app::Tab| {
        t.host_id == Some(host)
            && matches!(&t.kind, TabKind::Term(term) if matches!(term.state, ConnState::Connected) && matches!(term.spec, ConnectSpec::Ssh(_)))
    };
    let target = app
        .active
        .filter(|a| app.tabs.iter().any(|t| t.id == *a && live(t)))
        .or_else(|| app.tabs.iter().find(|t| live(t)).map(|t| t.id));
    match target {
        Some(id) => type_into(app, id, &cmd, false),
        None => {
            if let Some(h) = app.lib.host(host).cloned() {
                app.termx.pending_type = Some((host, cmd, Instant::now()));
                app.connect_host(h, Purpose::Terminal, ctx);
            }
        }
    }
}

/// Enter was pressed on `cmd` in an SSH session of `host`.
pub fn on_command(app: &mut App, host: Uuid, cmd: &str, cwd: Option<String>) {
    let s = &app.lib.settings;
    if !s.command_history || !app.termx.loaded || !history::should_record(cmd) {
        return;
    }
    let cmd = cmd.trim_end();
    let hit = app.termx.history.record(host, cmd, cwd, chrono::Utc::now());
    app.termx.dirty = true;
    let Some(key) = hit else { return };
    if !s.snippet_hints
        || s.snippet_hints_dismissed
            .contains(&history::pattern_id(&key))
        || app.lib.snippets.iter().any(|sn| sn.script.trim() == cmd)
    {
        return;
    }
    let Some(p) = app.termx.history.patterns.get(&key) else {
        return;
    };
    let template = history::template(&p.samples);
    app.termx.suggestion = Some(Suggestion {
        key,
        template,
        runs: p.count,
        since: Instant::now(),
    });
}

fn dismiss_pattern(app: &mut App, key: &str) {
    let id = history::pattern_id(key);
    if !app.lib.settings.snippet_hints_dismissed.contains(&id) {
        app.lib.settings.snippet_hints_dismissed.push(id);
        app.save_library();
    }
}

/// "Als Snippet speichern?" in the bottom right corner, above the toasts.
fn suggestion_hint(app: &mut App, ctx: &egui::Context) {
    let Some(s) = app.termx.suggestion.as_ref() else {
        return;
    };
    if s.since.elapsed() > Duration::from_secs(40) {
        app.termx.suggestion = None;
        return;
    }
    ctx.request_repaint_after(Duration::from_secs(1));
    let toasts = app.toasts.len() as f32;
    // 0 close, 1 save, 2 never
    let mut answer = None;
    egui::Area::new(egui::Id::new("snippet-hint"))
        .anchor(
            egui::Align2::RIGHT_BOTTOM,
            Vec2::new(-18.0, -40.0 - toasts * 52.0),
        )
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::surface())
                .stroke(Stroke::new(1.0, theme::line()))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::symmetric(16, 12))
                .shadow(egui::Shadow {
                    offset: [0, 10],
                    blur: 30,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(90),
                })
                .show(ui, |ui| {
                    ui.set_width(380.0);
                    ui.horizontal(|ui| {
                        crate::ui::icons::show(ui, Icon::Bolt, 18.0, theme::accent());
                        ui.label(
                            RichText::new(t!("Als Snippet speichern?"))
                                .font(theme::semibold(14.5))
                                .color(theme::text()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_btn(ui, Icon::Close, t!("Später")).clicked() {
                                answer = Some(0);
                            }
                        });
                    });
                    ui.label(
                        RichText::new(t!("Diesen Befehl hast du schon {}× ausgeführt.", s.runs))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                    ui.add_space(4.0);
                    theme::card_frame().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(theme::mono(&s.template, 12.0).color(theme::text()))
                                .wrap(),
                        );
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if theme::btn(ui, Btn::SmallPrimary, Some(Icon::Bolt), t!("Speichern…"))
                            .clicked()
                        {
                            answer = Some(1);
                        }
                        if theme::btn(ui, Btn::Link, None, t!("Nicht mehr vorschlagen")).clicked() {
                            answer = Some(2);
                        }
                    });
                });
        });
    let Some(answer) = answer else { return };
    let Some(s) = app.termx.suggestion.take() else {
        return;
    };
    match answer {
        1 => {
            dismiss_pattern(app, &s.key);
            app.snippet_editor = Some((
                crate::model::Snippet {
                    name: history::snippet_name(&s.template),
                    script: s.template,
                    ..Default::default()
                },
                true,
                String::new(),
            ));
        }
        2 => dismiss_pattern(app, &s.key),
        _ => {}
    }
}

/// Palette rows for history commands matching `q`: (title, subtitle, action, score).
pub fn palette_items(app: &App, q: &str) -> Vec<(String, String, Action, i32)> {
    if q.trim().len() < 2 || !app.lib.settings.command_history {
        return Vec::new();
    }
    let mut out: Vec<(String, String, Action, i32)> = Vec::new();
    for (host_id, entries) in &app.termx.history.hosts {
        let Some(host) = app.lib.host(*host_id) else {
            continue;
        };
        for e in entries {
            if let Some(score) = crate::ui::fuzzy(q, &e.cmd) {
                out.push((
                    format!("› {}", e.cmd),
                    format!(
                        "{} · {}",
                        host.display_name(),
                        crate::ui::relative_time(e.at)
                    ),
                    Action::TypeCommand(*host_id, e.cmd.clone()),
                    score,
                ));
            }
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.3));
    out.truncate(5);
    // Below hosts and snippets with the same match quality.
    for i in &mut out {
        i.3 -= 15;
    }
    out
}

/// What the Ctrl+R popup decided this frame.
pub enum PopupOut {
    Keep,
    Close,
    /// Command, and whether to run it right away.
    Insert(String, bool),
}

/// Ctrl+R popup over the bottom of the terminal `rect`.
pub fn popup(
    ctx: &egui::Context,
    tab: u64,
    rect: egui::Rect,
    st: &mut HistPopup,
    entries: &[history::Entry],
) -> PopupOut {
    let hits = history::search(entries, &st.query, 8);
    if st.selected >= hits.len() {
        st.selected = hits.len().saturating_sub(1);
    }
    let (run, insert, esc, up, down) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::R),
        )
    });
    if esc {
        return PopupOut::Close;
    }
    // The list shows the newest at the bottom, next to the prompt: up = older.
    if up && st.selected + 1 < hits.len() {
        st.selected += 1;
    }
    if down {
        st.selected = st.selected.saturating_sub(1);
    }
    if (run || insert)
        && let Some(e) = hits.get(st.selected)
    {
        return PopupOut::Insert(e.cmd.clone(), run);
    }
    let mut out = PopupOut::Keep;
    let w = (rect.width() - 24.0).clamp(240.0, 640.0);
    egui::Area::new(egui::Id::new(("hist-popup", tab)))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(rect.left_bottom() + Vec2::new(12.0, -12.0))
        .show(ctx, |ui| {
            theme::dialog_frame()
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(w);
                    ui.horizontal(|ui| {
                        crate::ui::icons::show(ui, Icon::Clock, 16.0, theme::accent());
                        ui.label(
                            RichText::new(t!("Befehls-Gedächtnis"))
                                .font(theme::semibold(13.0))
                                .color(theme::text()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(
                                RichText::new(t!("einfügen + ausführen"))
                                    .size(11.0)
                                    .color(theme::text_dim()),
                            );
                            theme::kbd(ui, "Shift+Enter");
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(t!("einfügen"))
                                    .size(11.0)
                                    .color(theme::text_dim()),
                            );
                            theme::kbd(ui, "Enter");
                        });
                    });
                    ui.add_space(4.0);
                    if hits.is_empty() {
                        ui.label(
                            RichText::new(if entries.is_empty() {
                                t!("Für diesen Host ist noch nichts gespeichert.")
                            } else {
                                t!("Keine Treffer")
                            })
                            .color(theme::text_dim()),
                        );
                    }
                    for (i, e) in hits.iter().enumerate().rev() {
                        let sel = i == st.selected;
                        let (r, resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 26.0),
                            egui::Sense::click(),
                        );
                        if sel || resp.hovered() {
                            ui.painter().rect_filled(
                                r,
                                CornerRadius::same(6),
                                if sel {
                                    theme::accent_soft()
                                } else {
                                    theme::hover()
                                },
                            );
                        }
                        let meta = match &e.cwd {
                            Some(d) => format!("{d} · {}", crate::ui::relative_time(e.at)),
                            None => crate::ui::relative_time(e.at),
                        };
                        let p = ui.painter();
                        let g = p.layout_no_wrap(
                            meta,
                            egui::FontId::proportional(11.0),
                            theme::text_dim(),
                        );
                        let meta_w = g.size().x;
                        p.galley(
                            egui::pos2(r.right() - 8.0 - meta_w, r.center().y - g.size().y / 2.0),
                            g,
                            theme::text_dim(),
                        );
                        p.with_clip_rect(r.with_max_x(r.right() - meta_w - 16.0))
                            .text(
                                r.left_center() + Vec2::new(8.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                &e.cmd,
                                egui::FontId::monospace(12.5),
                                if sel {
                                    theme::text()
                                } else {
                                    theme::text_dim()
                                },
                            );
                        if resp.clicked() {
                            out = PopupOut::Insert(e.cmd.clone(), false);
                        }
                    }
                    ui.add_space(4.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut st.query)
                            .hint_text(t!("Verlauf durchsuchen"))
                            .font(egui::FontId::monospace(13.0))
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut st.focus) || !edit.has_focus() {
                        edit.request_focus();
                    }
                    if edit.changed() {
                        st.selected = 0;
                    }
                });
        });
    out
}

/// Opens the Ctrl+R popup for a tab; `typed` is what already stands on the prompt.
pub fn open_popup(app_x: &mut TermExtras, tab: u64, typed: &str) {
    app_x.popups.insert(
        tab,
        HistPopup {
            query: typed.trim().to_string(),
            selected: 0,
            clear_line: !typed.is_empty(),
            focus: true,
        },
    );
}

/// Settings → Terminal: history, snippet hints, secret guard, time travel.
pub fn settings_card(app: &mut App, ui: &mut egui::Ui, changed: &mut bool) {
    let (hosts, total) = app.termx.history.totals();
    let mut with_history: Vec<(Uuid, String, usize)> = app
        .termx
        .history
        .hosts
        .iter()
        .filter(|(_, l)| !l.is_empty())
        .map(|(id, l)| {
            let name = app
                .lib
                .host(*id)
                .map(|h| h.display_name().to_string())
                .unwrap_or_else(|| t!("Gelöschter Host").to_string());
            (*id, name, l.len())
        })
        .collect();
    with_history.sort_by_key(|a| a.1.to_lowercase());
    let mut clear: Option<Option<Uuid>> = None;
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Befehls-Gedächtnis"),
            Some(t!(
                "Merkt sich pro Host, was du in SSH-Sessions ausführst – verschlüsselt im Tresor. Suchen: Ctrl+K oder Ctrl+R im Terminal. Befehle mit Leerzeichen am Anfang oder mit Passwörtern/Tokens werden übersprungen."
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.command_history).changed();
            },
        );
        rows.setting(
            ui,
            t!("Snippet-Vorschläge"),
            Some(t!(
                "Bietet lange Befehle, die du oft wiederholst, als Snippet mit Variablen an"
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.snippet_hints).changed();
            },
        );
        rows.setting(
            ui,
            t!("Verlauf löschen"),
            Some(&t!("{} Befehle von {} Hosts gespeichert", total, hosts)),
            |ui| {
                ui.add_enabled_ui(total > 0, |ui| {
                    if theme::btn(ui, Btn::Small, Some(Icon::Trash), t!("Alle")).clicked() {
                        clear = Some(None);
                    }
                    egui::ComboBox::from_id_salt("history-host")
                        .selected_text(t!("Einzelner Host…"))
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for (id, name, n) in &with_history {
                                if ui.button(format!("{name} ({n})")).clicked() {
                                    clear = Some(Some(*id));
                                }
                            }
                        });
                });
            },
        );
        rows.setting(
            ui,
            t!("Geheimnis-Wächter"),
            Some(t!(
                "Warnt vor dem Einfügen von Schlüsseln, Tokens und Passwörtern und vor mehrzeiligem Einfügen auf PROD"
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.paste_guard).changed();
            },
        );
        rows.setting(
            ui,
            t!("Zeitreise"),
            Some(t!(
                "Hält die letzten 30 Minuten Ausgabe jeder Session im Speicher (höchstens 16 MB), zum Zurückspulen über die Uhr im Terminal-Kopf"
            )),
            |ui| {
                *changed |= theme::toggle(ui, &mut app.lib.settings.time_travel).changed();
            },
        );
    });
    if let Some(which) = clear {
        match which {
            Some(h) => app.termx.history.clear_host(h),
            None => app.termx.history.clear(),
        }
        app.termx.dirty = true;
        app.termx.last_save = None;
        app.toast(t!("Verlauf gelöscht"), theme::green());
    }
}
