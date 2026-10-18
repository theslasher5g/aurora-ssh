//! Maintenance windows ("Wartungsfenster") for a host or a group: while one is active the
//! host shows an amber banner, its alerts are muted (disconnect/bell/command toasts, the
//! red "connection failed" LED); the PROD guard for dangerous commands stays on. Five
//! minutes before the end a card offers to extend; the end itself is announced.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveTime, TimeZone, Utc};
use egui::{CornerRadius, Margin, RichText, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::model::Library;
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::ops::OpsAction;

/// How long before the end the "extend?" card appears.
pub const WARN_BEFORE: i64 = 5 * 60;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Maintenance {
    pub id: Uuid,
    /// The host in maintenance, or …
    pub host: Option<Uuid>,
    /// … every host of this group and its subgroups.
    pub group: Option<Uuid>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub reason: String,
}

impl Default for Maintenance {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            host: None,
            group: None,
            start: now,
            end: now + chrono::Duration::hours(1),
            reason: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Host(Uuid),
    Group(Uuid),
}

impl Maintenance {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.start <= now && now < self.end
    }

    /// Less than [`WARN_BEFORE`] seconds left.
    pub fn ending_soon(&self, now: DateTime<Utc>) -> bool {
        self.is_active(now) && (self.end - now).num_seconds() <= WARN_BEFORE
    }

    pub fn target(&self) -> Option<Target> {
        match (self.host, self.group) {
            (Some(h), _) => Some(Target::Host(h)),
            (None, Some(g)) => Some(Target::Group(g)),
            _ => None,
        }
    }

    /// Applies to this host (directly or through its group chain).
    pub fn covers(&self, lib: &Library, host: Uuid) -> bool {
        if self.host == Some(host) {
            return true;
        }
        match (self.group, lib.host(host).and_then(|h| h.group_id)) {
            (Some(g), Some(hg)) => lib.is_descendant(hg, g),
            _ => false,
        }
    }

    /// Moves the end by `minutes` (from now if it already passed).
    pub fn extend(&mut self, minutes: i64, now: DateTime<Utc>) {
        let base = self.end.max(now);
        self.end = base + chrono::Duration::minutes(minutes);
    }

    /// `bis 14:30` (or with the date when it ends on another day).
    pub fn until_text(&self) -> String {
        until_text(self.end, Local::now())
    }

    /// Host or group name.
    pub fn target_name(&self, lib: &Library) -> String {
        match self.target() {
            Some(Target::Host(h)) => lib
                .host(h)
                .map(|h| h.display_name().to_string())
                .unwrap_or_else(|| "?".into()),
            Some(Target::Group(g)) => lib
                .group(g)
                .map(|g| g.name.clone())
                .unwrap_or_else(|| "?".into()),
            None => "?".into(),
        }
    }

    /// Banner text: `Wartung bis 14:30 – Grund`.
    pub fn banner(&self) -> String {
        let mut s = t!("Wartung {}", self.until_text());
        if !self.reason.trim().is_empty() {
            s.push_str(" – ");
            s.push_str(self.reason.trim());
        }
        s
    }
}

pub fn until_text(end: DateTime<Utc>, now: DateTime<Local>) -> String {
    let end = end.with_timezone(&Local);
    if end.date_naive() == now.date_naive() {
        t!("bis {}", end.format("%H:%M"))
    } else {
        t!("bis {}", end.format("%d.%m. %H:%M"))
    }
}

/// The active window covering `host` that ends last.
pub fn active_for(lib: &Library, host: Uuid, now: DateTime<Utc>) -> Option<&Maintenance> {
    lib.maintenance
        .iter()
        .filter(|m| m.is_active(now) && m.covers(lib, host))
        .max_by_key(|m| m.end)
}

/// The active window of a group itself (not inherited).
pub fn active_for_group(lib: &Library, group: Uuid, now: DateTime<Utc>) -> Option<&Maintenance> {
    lib.maintenance
        .iter()
        .filter(|m| m.is_active(now) && m.group == Some(group))
        .max_by_key(|m| m.end)
}

/// Hosts whose notifications are muted right now.
pub fn muted_hosts(lib: &Library) -> HashSet<Uuid> {
    let now = Utc::now();
    if !lib.maintenance.iter().any(|m| m.is_active(now)) {
        return HashSet::new();
    }
    lib.hosts
        .iter()
        .filter(|h| active_for(lib, h.id, now).is_some())
        .map(|h| h.id)
        .collect()
}

/// `14:30`, `9.05`, `930` → that time today, or tomorrow if it already passed.
pub fn parse_end(input: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    let s = input.trim().replace('.', ":");
    let (h, m) = match s.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None if s.len() == 3 || s.len() == 4 => {
            let n: u32 = s.parse().ok()?;
            (n / 100, n % 100)
        }
        None => (s.parse::<u32>().ok()?, 0),
    };
    let t = NaiveTime::from_hms_opt(h, m, 0)?;
    let mut day = now.date_naive();
    if day.and_time(t) <= now.naive_local() {
        day = day.succ_opt()?;
    }
    Local
        .from_local_datetime(&day.and_time(t))
        .earliest()
        .map(|d| d.with_timezone(&Utc))
}

// ------------------------------------------------------------------ UI state

/// Durations offered in the start sheet (minutes); the last choice is "custom end".
const CHOICES: [(i64, &str); 4] = [(30, "30 min"), (60, "1 h"), (120, "2 h"), (240, "4 h")];

pub struct StartSheet {
    pub target: Target,
    pub choice: usize,
    pub custom: String,
    pub reason: String,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct MaintUi {
    pub sheet: Option<StartSheet>,
    /// "Ending soon" cards the user closed.
    pub dismissed: HashSet<Uuid>,
}

pub fn open_sheet(app: &mut App, target: Target) {
    app.ops.maint.sheet = Some(StartSheet {
        target,
        choice: 1,
        custom: (Local::now() + chrono::Duration::hours(1))
            .format("%H:%M")
            .to_string(),
        reason: String::new(),
        error: None,
    });
}

pub fn extend(app: &mut App, id: Uuid, minutes: i64) {
    let now = Utc::now();
    if let Some(m) = app.lib.maintenance.iter_mut().find(|m| m.id == id) {
        m.extend(minutes, now);
        let text = t!("Wartung verlängert {}", m.until_text());
        app.ops.maint.dismissed.remove(&id);
        app.save_library();
        app.toast(text, theme::amber());
    }
}

pub fn end(app: &mut App, id: Uuid) {
    let Some(pos) = app.lib.maintenance.iter().position(|m| m.id == id) else {
        return;
    };
    let m = app.lib.maintenance.remove(pos);
    app.ops.maint.dismissed.remove(&id);
    app.save_library();
    app.toast(
        t!("Wartung beendet: {}", m.target_name(&app.lib)),
        theme::green(),
    );
}

/// Timers (ending soon, ended), the start sheet and the "ending soon" cards.
pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let now = Utc::now();
    // Ended windows: announce and drop.
    let ended: Vec<Maintenance> = app
        .lib
        .maintenance
        .iter()
        .filter(|m| m.end <= now)
        .cloned()
        .collect();
    if !ended.is_empty() {
        app.lib.maintenance.retain(|m| m.end > now);
        app.save_library();
        for m in ended {
            app.ops.maint.dismissed.remove(&m.id);
            app.toast(
                t!("Wartungsfenster abgelaufen: {}", m.target_name(&app.lib)),
                theme::amber(),
            );
        }
    }
    // Wake up for the next deadline.
    let next = app
        .lib
        .maintenance
        .iter()
        .flat_map(|m| [m.end - chrono::Duration::seconds(WARN_BEFORE), m.end])
        .filter(|t| *t > now)
        .min();
    if let Some(t) = next {
        let secs = (t - now).num_milliseconds().clamp(200, 30_000) as u64;
        ctx.request_repaint_after(Duration::from_millis(secs));
    }
    soon_cards(app, ctx, actions, now);
    sheet(app, ctx);
}

fn soon_cards(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>, now: DateTime<Utc>) {
    let soon: Vec<Maintenance> = app
        .lib
        .maintenance
        .iter()
        .filter(|m| m.ending_soon(now) && !app.ops.maint.dismissed.contains(&m.id))
        .cloned()
        .collect();
    if soon.is_empty() {
        return;
    }
    let lift = 40.0 + 56.0 * app.toasts.len() as f32;
    let mut dismiss = Vec::new();
    egui::Area::new(egui::Id::new("maint-soon"))
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -lift))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            for m in &soon {
                let left = (m.end - now).num_seconds().max(0);
                egui::Frame::NONE
                    .fill(theme::lerp(theme::surface(), theme::amber(), 0.10))
                    .stroke(Stroke::new(1.0, theme::amber()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(16, 12))
                    .shadow(egui::Shadow {
                        offset: [0, 10],
                        blur: 30,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(400.0);
                        ui.horizontal(|ui| {
                            icons::show(ui, Icon::Wrench, 16.0, theme::amber());
                            ui.label(
                                RichText::new(t!(
                                    "Wartung endet in {} Min – verlängern?",
                                    (left + 59) / 60
                                ))
                                .font(theme::semibold(13.5))
                                .color(theme::text()),
                            );
                        });
                        ui.label(
                            RichText::new(m.target_name(&app.lib))
                                .size(12.5)
                                .color(theme::text_dim()),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if theme::btn(ui, theme::Btn::SmallPrimary, None, "+30 min").clicked() {
                                actions.push(Action::Ops(OpsAction::MaintExtend(m.id, 30)));
                            }
                            if theme::btn(ui, theme::Btn::Small, None, t!("Beenden")).clicked() {
                                actions.push(Action::Ops(OpsAction::MaintEnd(m.id)));
                            }
                            if theme::btn(ui, theme::Btn::Link, None, t!("Ausblenden")).clicked() {
                                dismiss.push(m.id);
                            }
                        });
                    });
                ui.add_space(6.0);
            }
        });
    app.ops.maint.dismissed.extend(dismiss);
}

fn sheet(app: &mut App, ctx: &egui::Context) {
    let Some(s) = app.ops.maint.sheet.as_mut() else {
        return;
    };
    let name = match s.target {
        Target::Host(h) => app
            .lib
            .host(h)
            .map(|h| h.display_name().to_string())
            .unwrap_or_default(),
        Target::Group(g) => app
            .lib
            .group(g)
            .map(|g| t!("Gruppe {}", g.name))
            .unwrap_or_default(),
    };
    let title = t!("Wartung starten · {}", name);
    let now = Local::now();
    let ((start, cancel), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("maint-sheet"),
        520.0,
        &title,
        |ui| {
            ui.label(
                RichText::new(t!(
                    "Während der Wartung sind Warnungen und Benachrichtigungen für diese Hosts stumm. Die Rückfrage bei gefährlichen Befehlen auf PROD bleibt aktiv."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            );
            ui.add_space(4.0);
            let mut labels: Vec<&str> = CHOICES.iter().map(|(_, l)| *l).collect();
            labels.push(t!("Bis …"));
            if let Some(i) = theme::segmented(ui, &labels, s.choice) {
                s.choice = i;
            }
            theme::form(ui, 110.0, |ui, f| {
                if s.choice >= CHOICES.len() {
                    f.field(ui, t!("Ende (HH:MM)"), |ui| {
                        crate::ui::editors::field_text(ui, &mut s.custom, "14:30");
                    });
                }
                f.field(ui, t!("Grund"), |ui| {
                    crate::ui::editors::field_text(ui, &mut s.reason, t!("z. B. Kernel-Update"));
                });
            });
            let end = end_of(s, now);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let text = match end {
                    Some(e) => t!("Wartung {}", until_text(e, now)),
                    None => t!("Uhrzeit nicht erkannt").to_string(),
                };
                ui.label(RichText::new(text).size(12.5).color(theme::text_dim()));
            });
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(theme::red()));
            }
            end
        },
        |ui, end| {
            let mut out = (None, false);
            ui.add_enabled_ui(end.is_some(), |ui| {
                if theme::btn(
                    ui,
                    theme::Btn::Primary,
                    Some(Icon::Wrench),
                    t!("Wartung starten"),
                )
                .clicked()
                {
                    out.0 = end;
                }
            });
            if theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                out.1 = true;
            }
            out
        },
    );
    if cancel || dismissed {
        app.ops.maint.sheet = None;
        return;
    }
    let Some(end) = start else {
        return;
    };
    let Some(s) = app.ops.maint.sheet.take() else {
        return;
    };
    let m = Maintenance {
        host: match s.target {
            Target::Host(h) => Some(h),
            _ => None,
        },
        group: match s.target {
            Target::Group(g) => Some(g),
            _ => None,
        },
        start: Utc::now(),
        end,
        reason: s.reason.trim().to_string(),
        ..Default::default()
    };
    // One window per target: a new one replaces the old.
    app.lib
        .maintenance
        .retain(|x| !(x.host == m.host && x.group == m.group));
    let text = t!("Wartung gestartet: {} {}", name, m.until_text());
    app.lib.maintenance.push(m);
    app.save_library();
    app.toast(text, theme::amber());
}

fn end_of(s: &StartSheet, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    match CHOICES.get(s.choice) {
        Some((min, _)) => Some(now.with_timezone(&Utc) + chrono::Duration::minutes(*min)),
        None => parse_end(&s.custom, now),
    }
}

// ------------------------------------------------------------------ widgets

/// Amber pill "1 Wartung aktiv" for the connections page header, with a list menu.
pub fn header_pill(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let now = Utc::now();
    let active: Vec<&Maintenance> = app
        .lib
        .maintenance
        .iter()
        .filter(|m| m.is_active(now))
        .collect();
    if active.is_empty() {
        return;
    }
    let text = if active.len() == 1 {
        t!("1 Wartung aktiv").to_string()
    } else {
        t!("{} Wartungen aktiv", active.len())
    };
    let resp = theme::tag(ui, &text, Some(theme::amber()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(t!("Aktive Wartungsfenster anzeigen"));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(340.0);
        list(app, ui, &active, actions);
    });
}

/// Rows with the target, end, reason and the +30 min / end buttons.
fn list(app: &App, ui: &mut egui::Ui, active: &[&Maintenance], actions: &mut Vec<Action>) {
    for m in active {
        ui.horizontal(|ui| {
            icons::show(ui, Icon::Wrench, 15.0, theme::amber());
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(m.target_name(&app.lib)).font(theme::semibold(13.0)));
                let mut sub = m.until_text();
                if !m.reason.trim().is_empty() {
                    sub = format!("{sub} · {}", m.reason.trim());
                }
                ui.label(RichText::new(sub).size(12.0).color(theme::text_dim()));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::btn(ui, theme::Btn::Small, None, t!("Beenden")).clicked() {
                    actions.push(Action::Ops(OpsAction::MaintEnd(m.id)));
                    ui.close();
                }
                if theme::btn(ui, theme::Btn::Small, None, "+30 min").clicked() {
                    actions.push(Action::Ops(OpsAction::MaintExtend(m.id, 30)));
                    ui.close();
                }
            });
        });
    }
}

/// Amber strip under a session header while its host is in maintenance.
pub fn session_banner(ui: &mut egui::Ui, id: egui::Id, m: &Maintenance) -> Option<OpsAction> {
    let mut out = None;
    egui::Panel::top(id)
        .frame(
            egui::Frame::NONE
                .fill(theme::lerp(theme::surface(), theme::amber(), 0.16))
                .inner_margin(Margin::symmetric(14, 4)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::btn(ui, theme::Btn::Link, None, t!("Beenden")).clicked() {
                        out = Some(OpsAction::MaintEnd(m.id));
                    }
                    if theme::btn(ui, theme::Btn::Link, None, "+30 min").clicked() {
                        out = Some(OpsAction::MaintExtend(m.id, 30));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        icons::show(ui, Icon::Wrench, 15.0, theme::amber());
                        ui.add(
                            egui::Label::new(
                                RichText::new(m.banner())
                                    .font(theme::semibold(13.0))
                                    .color(theme::amber()),
                            )
                            .truncate(),
                        )
                        .on_hover_text(t!(
                            "Benachrichtigungen für diesen Host sind stumm; die PROD-Rückfrage bleibt aktiv"
                        ));
                    });
                });
            });
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Group, Host};

    #[test]
    fn windows_cover_hosts_and_groups() {
        let mut lib = Library::default();
        let parent = Group::default();
        let child = Group {
            parent_id: Some(parent.id),
            ..Default::default()
        };
        let a = Host {
            group_id: Some(child.id),
            ..Default::default()
        };
        let b = Host::default();
        lib.groups = vec![parent.clone(), child];
        lib.hosts = vec![a.clone(), b.clone()];
        let now = Utc::now();
        lib.maintenance.push(Maintenance {
            group: Some(parent.id),
            start: now - chrono::Duration::minutes(1),
            end: now + chrono::Duration::minutes(30),
            ..Default::default()
        });
        assert!(active_for(&lib, a.id, now).is_some());
        assert!(active_for(&lib, b.id, now).is_none());
        assert!(active_for_group(&lib, parent.id, now).is_some());
        let muted = muted_hosts(&lib);
        assert!(muted.contains(&a.id) && !muted.contains(&b.id));
        // Over: nothing active any more.
        assert!(active_for(&lib, a.id, now + chrono::Duration::hours(1)).is_none());
    }

    #[test]
    fn time_logic() {
        let now = Utc::now();
        let mut m = Maintenance {
            start: now,
            end: now + chrono::Duration::minutes(10),
            ..Default::default()
        };
        assert!(m.is_active(now) && !m.ending_soon(now));
        assert!(m.ending_soon(now + chrono::Duration::minutes(6)));
        assert!(!m.is_active(now + chrono::Duration::minutes(10)));
        m.extend(30, now);
        assert_eq!(m.end, now + chrono::Duration::minutes(40));
        // Extending a window that already ran out counts from now.
        let later = now + chrono::Duration::hours(2);
        m.extend(30, later);
        assert_eq!(m.end, later + chrono::Duration::minutes(30));
    }

    #[test]
    fn custom_end_times() {
        let now = Local
            .from_local_datetime(
                &chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
                    .unwrap()
                    .and_hms_opt(13, 0, 0)
                    .unwrap(),
            )
            .unwrap();
        let at = |s: &str| parse_end(s, now).map(|d| d.with_timezone(&Local).naive_local());
        let day = now.date_naive();
        assert_eq!(at("14:30"), Some(day.and_hms_opt(14, 30, 0).unwrap()));
        assert_eq!(at("14.30"), Some(day.and_hms_opt(14, 30, 0).unwrap()));
        assert_eq!(at("1430"), Some(day.and_hms_opt(14, 30, 0).unwrap()));
        assert_eq!(at("16"), Some(day.and_hms_opt(16, 0, 0).unwrap()));
        // Earlier than now: tomorrow.
        assert_eq!(
            at("9:15"),
            Some(day.succ_opt().unwrap().and_hms_opt(9, 15, 0).unwrap())
        );
        assert_eq!(at("25:00"), None);
        assert_eq!(at("abc"), None);
        let end = Local
            .from_local_datetime(&day.and_hms_opt(14, 30, 0).unwrap())
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(until_text(end, now), "bis 14:30");
    }

    #[test]
    fn old_libraries_load() {
        let lib: Library = serde_json::from_str(r#"{"hosts":[]}"#).unwrap();
        assert!(lib.maintenance.is_empty() && lib.runbooks.is_empty());
    }
}
