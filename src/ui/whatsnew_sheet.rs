//! The "Neu in Aurora X.Y.Z" sheet shown once after an update (and from Help / the palette).

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, RichText};

use crate::app::App;
use crate::theme;
use crate::update::{self, Version};
use crate::whatsnew_data::{self as data, Block, Release, Span, Startup};

#[derive(Default)]
pub struct State {
    /// Parsed changelog (filled on first use).
    releases: Option<Vec<Release>>,
    /// The start-up check ran for this session.
    checked: bool,
    /// Showing: the versions to list, and whether older ones exist.
    shown: Option<Shown>,
    /// Open the sheet by hand (Help page, palette) on the next frame.
    pub request: bool,
}

struct Shown {
    versions: Vec<Version>,
    more: bool,
}

impl State {
    fn releases(&mut self) -> &[Release] {
        self.releases
            .get_or_insert_with(|| data::parse(data::CHANGELOG))
    }
}

/// Called every frame while unlocked and not over the tour.
pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.lib.settings.onboarded {
        // The first-start tour has the stage; a fresh install records the version silently.
        if !app.whatsnew.checked {
            app.whatsnew.checked = true;
            record(app);
        }
        return;
    }
    let current = update::current();
    if !app.whatsnew.checked {
        app.whatsnew.checked = true;
        match data::startup(&app.lib.settings.last_seen_version, true, current) {
            Startup::Nothing => {}
            Startup::Record => record(app),
            Startup::Show => {
                let last = update::parse_version(&app.lib.settings.last_seen_version);
                let rels = app.whatsnew.releases().to_vec();
                let all = data::newer_than(&rels, last, current);
                if all.is_empty() {
                    record(app);
                } else {
                    app.whatsnew.shown = Some(Shown {
                        versions: all
                            .iter()
                            .take(data::MAX_VERSIONS)
                            .map(|r| r.version)
                            .collect(),
                        more: all.len() > data::MAX_VERSIONS,
                    });
                }
            }
        }
    }
    if std::mem::take(&mut app.whatsnew.request) {
        let rels = app.whatsnew.releases().to_vec();
        let all = data::latest(&rels, current);
        if !all.is_empty() {
            app.whatsnew.shown = Some(Shown {
                versions: all
                    .iter()
                    .take(data::MAX_VERSIONS)
                    .map(|r| r.version)
                    .collect(),
                more: all.len() > data::MAX_VERSIONS,
            });
        }
    }
    // Do not stack on top of a password or host key prompt.
    if app.password_prompt.is_some() || !app.hostkey_queue.is_empty() {
        return;
    }
    let Some(shown) = app.whatsnew.shown.as_ref() else {
        return;
    };
    let versions = shown.versions.clone();
    let more = shown.more;
    let rels: Vec<Release> = {
        let all = app.whatsnew.releases();
        versions
            .iter()
            .filter_map(|v| all.iter().find(|r| r.version == *v).cloned())
            .collect()
    };
    let Some(top) = rels.first().map(|r| r.version) else {
        app.whatsnew.shown = None;
        return;
    };
    let title = t!("Neu in Aurora {}", data::version_text(top));
    let (act, dismissed) = theme::sheet(
        ctx,
        egui::Id::new("whatsnew"),
        580.0,
        &title,
        |ui| body(ui, &rels),
        |ui, ()| {
            let mut act = None;
            if theme::primary_button(ui, t!("Verstanden")).clicked() {
                act = Some(Act::Done);
            }
            if theme::btn(ui, theme::Btn::Normal, None, t!("Auf GitHub ansehen")).clicked() {
                act = Some(Act::Release(top));
            }
            if more && theme::btn(ui, theme::Btn::Link, None, t!("Alle Änderungen")).clicked() {
                act = Some(Act::All);
            }
            act
        },
    );
    match act {
        Some(Act::Release(v)) => update::open_url(&data::release_url(v)),
        Some(Act::All) => update::open_url(&data::changelog_url()),
        Some(Act::Done) => close(app),
        None if dismissed => close(app),
        None => {}
    }
}

enum Act {
    Done,
    Release(Version),
    All,
}

fn close(app: &mut App) {
    app.whatsnew.shown = None;
    record(app);
}

/// Remembers that this version's news were seen.
fn record(app: &mut App) {
    let v = data::version_text(update::current());
    if app.lib.settings.last_seen_version != v {
        app.lib.settings.last_seen_version = v;
        app.save_library();
    }
}

fn body(ui: &mut egui::Ui, rels: &[Release]) {
    for (i, r) in rels.iter().enumerate() {
        if rels.len() > 1 {
            if i > 0 {
                ui.add_space(14.0);
            }
            ui.label(
                RichText::new(format!("Aurora {}", r.text()))
                    .font(theme::semibold(15.0))
                    .color(theme::accent()),
            );
        }
        for b in &r.blocks {
            match b {
                Block::Heading(h) => {
                    ui.add_space(6.0);
                    theme::section_title(ui, h);
                }
                Block::Bullet(t) => {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(RichText::new("•").color(theme::accent()));
                        ui.add(egui::Label::new(job(ui, t)).wrap());
                    });
                }
                Block::Para(t) => {
                    ui.add(egui::Label::new(job(ui, t)).wrap());
                }
            }
        }
    }
}

/// One wrapped paragraph with **bold** and `code`.
fn job(ui: &egui::Ui, text: &str) -> LayoutJob {
    let size = 13.5;
    let mut job = LayoutJob::default();
    let ink = theme::text();
    for s in data::spans(text) {
        let (txt, fmt) = match s {
            Span::Plain(t) => (
                t,
                TextFormat::simple(FontId::new(size, FontFamily::Proportional), ink),
            ),
            Span::Bold(t) => (t, TextFormat::simple(theme::semibold(size), ink)),
            Span::Italic(t) => (
                t,
                TextFormat {
                    italics: true,
                    ..TextFormat::simple(FontId::new(size, FontFamily::Proportional), ink)
                },
            ),
            Span::Code(t) => (
                t,
                TextFormat {
                    font_id: FontId::new(size - 1.0, FontFamily::Monospace),
                    color: theme::accent(),
                    background: theme::lerp(theme::surface(), Color32::WHITE, 0.07),
                    ..Default::default()
                },
            ),
        };
        job.append(&txt, 0.0, fmt);
    }
    let _ = ui;
    job
}

/// Palette / Help: open the sheet with the latest versions.
pub fn open(app: &mut App) {
    app.whatsnew.request = true;
}

pub fn open_flag(state: &mut State) {
    state.request = true;
}
