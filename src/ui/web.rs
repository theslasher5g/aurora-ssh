//! Web interface entries: the form section, the session tab and the certificate dialog.
//!
//! The page itself runs in the `aurora-web` helper: embedded as a child window inside the
//! tab area (see `web_embed`) or, where that is not possible or wanted, in its own window.
//! [`WebTab::body`] is the single place that draws the area the page occupies.

use egui::{Color32, RichText};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{Action, App, ConnState, TabKind};
use crate::backend::Sink;
use crate::backend::web::{self, TlsMode, WebCmd, WebEvent, WebHandle, proto};
use crate::model::Host;
use crate::theme;
use crate::ui::editors::field_text;
use crate::ui::icons::{self, Icon};

// ------------------------------------------------------------------------------------
// Form
// ------------------------------------------------------------------------------------

/// "Vorlage" and "Adresse" rows of the host form (replaces host:port for Web entries).
pub fn address_rows(ui: &mut egui::Ui, f: &mut theme::Rows, h: &mut Host, tags: &mut String) {
    f.field(ui, t!("Vorlage"), |ui| {
        let current = web::template(&h.web.template)
            .map(|t| crate::i18n::tr(t.name).to_string())
            .unwrap_or_else(|| t!("— keine (eigene Adresse) —").to_string());
        let mut picked = None;
        egui::ComboBox::from_id_salt("web-template")
            .selected_text(current)
            .width(ui.available_width() - 14.0)
            .height(360.0)
            .show_ui(ui, |ui| {
                for t in web::TEMPLATES {
                    let label = crate::i18n::tr(t.name);
                    let on = h.web.template == t.id;
                    let r = ui.selectable_label(on, label);
                    if !t.hint.is_empty() {
                        r.clone().on_hover_text(crate::i18n::tr(t.hint));
                    }
                    if r.clicked() {
                        picked = Some(t);
                    }
                }
            });
        if let Some(t) = picked {
            web::forget_template_tag(tags, &h.web.template);
            web::apply_template(h, tags, t);
        }
    });
    f.field(ui, t!("Adresse"), |ui| {
        let bad = !h.web.url.trim().is_empty() && proto::parse_url(&h.web.url).is_err();
        ui.add(
            egui::TextEdit::singleline(&mut h.web.url)
                .hint_text(RichText::new("https://192.168.1.1:8443/").color(theme::text_dim()))
                .text_color_opt(bad.then(theme::red))
                .desired_width(f32::INFINITY),
        );
    });
    if let Err(e) = proto::parse_url(&h.web.url)
        && !h.web.url.trim().is_empty()
    {
        f.full(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::tr(e.message()))
                    .size(12.5)
                    .color(theme::red()),
            );
        });
    } else if let Some(t) = web::template(&h.web.template)
        && !t.hint.is_empty()
    {
        f.full(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(crate::i18n::tr(t.hint))
                        .size(12.5)
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
        });
    }
}

fn tls_labels() -> [&'static str; 3] {
    [
        t!("Streng"),
        t!("Selbstsigniert (anheften)"),
        t!("Alles ignorieren"),
    ]
}

/// The "Web-Oberfläche" section of the host form. `session_open`: the entry's window is
/// open right now (its saved data cannot be deleted then).
pub fn section(ui: &mut egui::Ui, h: &mut Host, session_open: bool) {
    theme::form_caption(ui, t!("Web-Oberfläche"));
    theme::form(ui, 120.0, |ui, f| {
        f.field(ui, t!("TLS-Zertifikat"), |ui| {
            let cur = match h.web.tls {
                TlsMode::Strict => 0,
                TlsMode::Pinned => 1,
                TlsMode::InsecureAll => 2,
            };
            let labels = tls_labels();
            if let Some(i) = theme::segmented(ui, &labels, cur) {
                h.web.tls = [TlsMode::Strict, TlsMode::Pinned, TlsMode::InsecureAll][i];
            }
        });
        f.full(ui, |ui| {
            let (text, color) = match h.web.tls {
                TlsMode::Strict => (
                    t!("Nur Zertifikate, denen das System vertraut. Selbstsignierte Geräte werden abgelehnt."),
                    theme::text_dim(),
                ),
                TlsMode::Pinned => (
                    t!("Beim ersten Öffnen zeigt Aurora den Fingerabdruck des Zertifikats und fragt. Danach wird genau dieses Zertifikat akzeptiert; ändert es sich, warnt Aurora laut."),
                    theme::text_dim(),
                ),
                TlsMode::InsecureAll => (
                    t!("ACHTUNG: Alle Zertifikatsfehler werden ignoriert – jeder im Netz könnte sich als dieses Gerät ausgeben und das Passwort mitlesen. Nur für Geräte, die du nicht anders erreichst."),
                    theme::red(),
                ),
            };
            ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).wrap());
        });
        if h.web.tls == TlsMode::Pinned && !h.web.pinned_fp.is_empty() {
            f.field(ui, t!("Angeheftet"), |ui| {
                if theme::btn(ui, theme::Btn::Small, None, t!("Entfernen")).clicked() {
                    h.web.pinned_fp.clear();
                    h.web.pinned_at = None;
                }
                ui.add(
                    egui::Label::new(theme::mono(&h.web.pinned_fp, 11.0).color(theme::green()))
                        .wrap(),
                );
            });
        }
        let http = proto::parse_url(&h.web.url).is_ok_and(|u| !u.https);
        f.switch(ui, t!("Automatisch anmelden"), &mut h.web.auto_login)
            .on_hover_text(t!(
                "Füllt das Anmeldeformular mit Benutzer und Passwort der Identität (oder des Passwort-Managers) oben. Das Passwort wird nur an das Fenster dieses Eintrags übergeben."
            ));
        if h.web.auto_login {
            f.switch(ui, t!("Anmelden-Taste drücken"), &mut h.web.submit);
            f.field(ui, t!("Benutzer-Feld"), |ui| {
                field_text(
                    ui,
                    &mut h.web.user_selector,
                    t!("CSS-Selektor, leer = automatisch"),
                );
            });
            f.field(ui, t!("Passwort-Feld"), |ui| {
                field_text(
                    ui,
                    &mut h.web.pass_selector,
                    t!("CSS-Selektor, leer = automatisch"),
                );
            });
            f.full(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(t!(
                            "Automatisch heißt: erstes sichtbares Passwortfeld und das Textfeld davor. Es wird höchstens einmal pro Öffnen abgeschickt, damit ein falsches Passwort das Konto nicht sperrt."
                        ))
                        .size(12.5)
                        .color(theme::text_dim()),
                    )
                    .wrap(),
                );
                if http {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(t!(
                                "Die Adresse ist unverschlüsselt (http): das Passwort ginge im Klartext durchs Netz."
                            ))
                            .size(12.5)
                            .color(theme::amber()),
                        )
                        .wrap(),
                    );
                }
            });
        }
        f.field(ui, t!("Weitere Hosts"), |ui| {
            field_text(
                ui,
                &mut h.web.extra_hosts,
                t!("z.B. sso.example.com (Anmelde-Weiterleitung)"),
            );
        });
        f.full(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let can_forget = !session_open && web::profile_dir(h.id).is_dir();
                ui.add_enabled_ui(can_forget, |ui| {
                    if theme::btn(ui, theme::Btn::Small, None, t!("Gespeicherte Anmeldungen löschen"))
                        .on_hover_text(t!(
                            "Löscht Cookies, Anmeldungen und Cache dieses Eintrags (nur möglich, solange das Fenster zu ist)"
                        ))
                        .clicked()
                    {
                        web::forget_profile(h.id);
                    }
                });
            });
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t!(
                        "Jeder Eintrag hat sein eigenes Profil: Cookies und Anmeldungen sind getrennt. Links zu anderen Adressen öffnen im Standard-Browser; Downloads landen in Downloads/Aurora."
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                )
                .wrap(),
            );
        });
    });
}

// ------------------------------------------------------------------------------------
// Tab
// ------------------------------------------------------------------------------------

/// A certificate the helper refused and Aurora has to decide about.
#[derive(Clone, Debug, PartialEq)]
pub struct CertPrompt {
    pub fingerprint: String,
    /// The pinned fingerprint that did not match (`None` = nothing pinned yet).
    pub expected: Option<String>,
}

pub struct WebTab {
    pub host_id: Uuid,
    pub url: String,
    pub tls: TlsMode,
    /// `host:port` of the entry (pin scope, shown in the dialog).
    pub address: String,
    pub state: ConnState,
    pub page_title: String,
    pub current_url: String,
    pub engine: String,
    pub cert: Option<CertPrompt>,
    /// Strict mode refused a certificate (fingerprint).
    pub refused: Option<String>,
    pub notes: Vec<(String, bool)>,
    /// The user said "no" to the pin dialog (until the next reload).
    pub declined: bool,
    last_error: Option<String>,
    handle: Option<WebHandle>,
    rx: UnboundedReceiver<WebEvent>,
    /// The page lives inside Aurora's window (confirmed by the helper).
    pub embedded: bool,
    /// Embedded tab shows the details card instead of the page.
    show_info: bool,
    /// Last rectangle / visibility sent to the helper (only changes are sent).
    sent: Option<(proto::PixelRect, bool)>,
    pending_toasts: Vec<(String, bool)>,
}

impl WebTab {
    /// Starts the helper for `host`; failures show up as a closed tab with the reason.
    pub fn open(host: &Host, user: &str, password: Option<&str>, ctx: &egui::Context) -> Self {
        let (sink, rx) = Sink::new(ctx.clone());
        let parsed = proto::parse_url(&host.web.url).ok();
        let mut tab = Self {
            host_id: host.id,
            url: host.web.url.clone(),
            tls: host.web.tls,
            address: parsed.as_ref().map(web::pin_scope).unwrap_or_default(),
            state: ConnState::Connecting(t!("Fenster wird geöffnet…").into()),
            page_title: String::new(),
            current_url: String::new(),
            engine: String::new(),
            cert: None,
            refused: None,
            notes: Vec::new(),
            declined: false,
            last_error: None,
            handle: None,
            rx,
            embedded: false,
            show_info: false,
            sent: None,
            pending_toasts: Vec::new(),
        };
        let started = web::start_for(host, user, password).and_then(|mut s| {
            s.embed = crate::ui::web_embed::parent_for_open();
            web::spawn(&s, sink).map_err(|e| e.to_string())
        });
        match started {
            Ok(h) => tab.handle = Some(h),
            Err(e) => tab.state = ConnState::Closed(Some(e)),
        }
        tab
    }

    pub fn is_live(&self) -> bool {
        matches!(self.state, ConnState::Connected)
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.state, ConnState::Closed(_))
    }

    pub fn send(&self, cmd: WebCmd) {
        if let Some(h) = &self.handle {
            h.send(&cmd);
        }
    }

    pub fn close(&mut self) {
        if let Some(mut h) = self.handle.take() {
            h.close();
        }
        if !self.is_closed() {
            self.state = ConnState::Closed(None);
        }
    }

    fn note(&mut self, text: String, ok: bool) {
        if self.embedded {
            // The details card is hidden behind the page: say it where it is seen.
            self.pending_toasts.push((text.clone(), ok));
        }
        self.notes.push((text, ok));
        if self.notes.len() > 12 {
            self.notes.remove(0);
        }
    }

    /// Applies the helper's events (also while the tab is in the background).
    pub fn pump(&mut self, title: &str, toasts: &mut Vec<(String, Color32)>) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                WebEvent::Ready { engine, embedded } => {
                    self.engine = engine.chars().take(40).collect();
                    self.embedded = embedded;
                    self.state = ConnState::Connected;
                }
                WebEvent::Title { title } => self.page_title = title,
                WebEvent::Loaded { url } => {
                    self.current_url = url;
                    self.state = ConnState::Connected;
                }
                WebEvent::Cert {
                    fingerprint,
                    expected,
                    strict,
                } => {
                    if strict || self.tls != TlsMode::Pinned {
                        self.refused = Some(fingerprint);
                    } else if !self.declined || expected.is_some() {
                        toasts.push((
                            t!("{}: Zertifikat wartet auf deine Bestätigung", title),
                            theme::amber(),
                        ));
                        self.cert = Some(CertPrompt {
                            fingerprint,
                            expected,
                        });
                    }
                }
                WebEvent::External { url } => {
                    if web::open_external(&url) {
                        let host = proto::parse_url(&url).map(|u| u.host).unwrap_or_default();
                        self.note(t!("Link im Browser geöffnet: {}", host), true);
                    } else {
                        self.note(t!("Link nicht geöffnet (ungültig)").to_string(), false);
                    }
                }
                WebEvent::Download { name, ok } => {
                    let name: String = name.chars().take(80).collect();
                    if ok {
                        self.note(t!("Download gespeichert: {}", name), true);
                    } else {
                        self.note(t!("Download fehlgeschlagen: {}", name), false);
                    }
                }
                WebEvent::Blocked { url } => {
                    let url: String = url.chars().take(80).collect();
                    self.note(t!("Navigation blockiert: {}", url), false);
                }
                WebEvent::Error { message } => {
                    let message: String = message.chars().take(200).collect();
                    toasts.push((format!("{title}: {message}"), theme::red()));
                    self.last_error = Some(message);
                }
                WebEvent::Closed => {
                    self.state = ConnState::Closed(self.last_error.take());
                    self.handle = None;
                    self.cert = None;
                    self.sent = None;
                }
            }
        }
        for (text, ok) in std::mem::take(&mut self.pending_toasts) {
            toasts.push((
                format!("{title}: {text}"),
                if ok { theme::accent() } else { theme::amber() },
            ));
        }
        if self.handle.as_mut().is_some_and(|h| !h.alive()) && !self.is_closed() {
            self.state = ConnState::Closed(Some(t!("Web-Komponente unerwartet beendet").into()));
            self.handle = None;
        }
    }

    /// Positions the embedded page (`view` = pixel rectangle in the main window, `None` =
    /// hidden because something else is shown or drawn over it). Only changes are sent.
    pub fn apply_view(&mut self, view: Option<proto::PixelRect>, active: bool, pressed: bool) {
        if self.handle.is_none() {
            return;
        }
        let was_visible = self.sent.is_some_and(|s| s.1);
        let (rect, visible) = match view {
            Some(r) => (r, true),
            None => (
                self.sent.map(|s| s.0).unwrap_or(proto::PixelRect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                }),
                false,
            ),
        };
        if self.sent != Some((rect, visible)) {
            self.send(WebCmd::Bounds {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
                visible,
            });
            self.sent = Some((rect, visible));
        }
        if visible && !was_visible && active {
            // Typing goes to the page that just appeared.
            self.send(WebCmd::Focus);
        } else if (!visible && was_visible) || (visible && pressed) {
            // A click anywhere in Aurora (the page itself never reaches Aurora) or the
            // page going away: the keyboard belongs to Aurora again.
            self.send(WebCmd::Blur);
        }
    }

    /// The area the page occupies: the page itself (embedded) or the status card.
    fn body(&self, ui: &mut egui::Ui, tab_id: u64, pinned: &str) {
        let in_main = ui.ctx().viewport_id() == egui::ViewportId::ROOT;
        if self.embedded && in_main && !self.show_info && !self.is_closed() {
            let rect = ui.available_rect_before_wrap();
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::ZERO, theme::bg_deep());
            ui.allocate_rect(rect, egui::Sense::hover());
            crate::ui::web_embed::register(ui, tab_id, rect);
        } else {
            status_card(ui, self, pinned);
        }
    }
}

/// Status card of the tab.
fn status_card(ui: &mut egui::Ui, t: &WebTab, pinned: &str) {
    let (color, state) = match &t.state {
        ConnState::Connecting(m) => (theme::amber(), m.clone()),
        ConnState::Connected if t.embedded => {
            (theme::green(), t!("Seite im Tab geöffnet").to_string())
        }
        ConnState::Connected => (theme::green(), t!("Fenster geöffnet").to_string()),
        ConnState::Closed(Some(e)) => (theme::red(), e.clone()),
        ConnState::Closed(None) => (theme::text_dim(), t!("Fenster geschlossen").to_string()),
    };
    theme::card(ui, theme::bg_deep(), |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(24, 22))
            .show(ui, |ui| {
                ui.set_max_width(640.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    icons::show(ui, Icon::Globe, 26.0, theme::accent());
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        let title = if t.page_title.is_empty() {
                            t.url.clone()
                        } else {
                            t.page_title.clone()
                        };
                        ui.add(egui::Label::new(theme::heading(title, 19.0)).truncate());
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(12.0, 14.0), egui::Sense::hover());
                            theme::led(ui.painter(), r.center(), color, t.is_live());
                            ui.add(
                                egui::Label::new(RichText::new(state).color(color)).wrap(),
                            );
                        });
                    });
                });
                ui.add_space(6.0);
                info_row(ui, t!("Adresse"), &t.url, theme::text());
                if !t.current_url.is_empty() && t.current_url.trim_end_matches('/') != t.url.trim_end_matches('/') {
                    info_row(ui, t!("Aktuelle Seite"), &t.current_url, theme::text_dim());
                }
                match t.tls {
                    TlsMode::Strict => {
                        info_row(ui, t!("Zertifikat"), t!("streng geprüft"), theme::green())
                    }
                    TlsMode::Pinned => {
                        if pinned.is_empty() {
                            info_row(
                                ui,
                                t!("Zertifikat"),
                                t!("selbstsigniert erlaubt – noch nichts angeheftet"),
                                theme::amber(),
                            );
                        } else {
                            info_row(
                                ui,
                                t!("Angeheftet"),
                                &format!("SHA-256 {pinned}"),
                                theme::green(),
                            );
                        }
                    }
                    TlsMode::InsecureAll => info_row(
                        ui,
                        t!("Zertifikat"),
                        t!("Fehler werden ignoriert (unsicher)"),
                        theme::red(),
                    ),
                }
                if let Some(fp) = &t.refused {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(t!(
                                "Das Zertifikat wurde abgelehnt (SHA-256 {}). Stelle den Eintrag auf „Selbstsigniert (anheften)“, wenn du diesem Gerät vertraust.",
                                fp
                            ))
                            .size(12.5)
                            .color(theme::red()),
                        )
                        .wrap(),
                    );
                }
                if !t.engine.is_empty() {
                    info_row(ui, t!("Browser-Engine"), &t.engine, theme::text_dim());
                }
                if !t.notes.is_empty() {
                    ui.add_space(8.0);
                    for (text, ok) in &t.notes {
                        ui.add(
                            egui::Label::new(
                                RichText::new(text)
                                    .size(12.5)
                                    .color(if *ok { theme::text_dim() } else { theme::amber() }),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(if t.embedded {
                            t!("Die Weboberfläche läuft eingebettet in diesem Tab mit eigenem Profil. Schließt du den Tab, endet auch die Seite.")
                        } else {
                            t!("Die Weboberfläche läuft in einem eigenen Fenster mit eigenem Profil. Schließt du den Tab, schließt auch das Fenster – und umgekehrt.")
                        })
                        .size(12.5)
                        .color(theme::text_dim()),
                    )
                    .wrap(),
                );
            });
    });
}

fn info_row(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add_sized(
            [110.0, 18.0],
            egui::Label::new(RichText::new(label).size(12.5).color(theme::text_dim())),
        );
        ui.add(egui::Label::new(theme::mono(value, 12.5).color(color)).wrap());
    });
}

/// Draws the tab (session header + status card).
pub fn show(app: &mut App, ui: &mut egui::Ui, tab_id: u64, actions: &mut Vec<Action>) {
    let index = app.tabs.iter().position(|t| t.id == tab_id).unwrap_or(0) + 1;
    let Some(pos) = app.tabs.iter().position(|t| t.id == tab_id) else {
        return;
    };
    let (title, subtitle, host_id) = {
        let tab = &app.tabs[pos];
        (tab.title.clone(), tab.subtitle.clone(), tab.host_id)
    };
    let pinned = host_id
        .and_then(|h| app.lib.host(h))
        .map(|h| h.web.pinned_fp.clone())
        .unwrap_or_default();
    let TabKind::Web(t) = &mut app.tabs[pos].kind else {
        return;
    };
    let state = crate::ui::chrome::state_pair(&t.state);
    let closed = t.is_closed();
    let (mut front, mut reload, mut close, mut reopen, mut edit) =
        (false, false, false, false, false);
    let mut switch: Option<bool> = None;
    let mut toggle_info = false;
    let in_main = ui.ctx().viewport_id() == egui::ViewportId::ROOT;
    let embedded = t.embedded;
    let show_info = t.show_info;
    crate::ui::chrome::session_header(
        ui,
        egui::Id::new(("web-bar", tab_id)),
        index,
        &title,
        &subtitle,
        state,
        |ui| {
            if closed {
                if theme::btn(ui, theme::Btn::Small, Some(Icon::Refresh), t!("Neu öffnen"))
                    .clicked()
                {
                    reopen = true;
                }
            } else {
                if theme::btn(ui, theme::Btn::Small, Some(Icon::Close), t!("Schließen")).clicked()
                {
                    close = true;
                }
                if theme::btn(ui, theme::Btn::Small, Some(Icon::Refresh), t!("Neu laden")).clicked()
                {
                    reload = true;
                }
                if embedded {
                    if theme::btn(
                        ui,
                        theme::Btn::Small,
                        Some(Icon::Shield),
                        if show_info {
                            t!("Seite anzeigen")
                        } else {
                            t!("Details")
                        },
                    )
                    .clicked()
                    {
                        toggle_info = true;
                    }
                    if theme::btn(
                        ui,
                        theme::Btn::Small,
                        Some(Icon::External),
                        t!("In eigenem Fenster öffnen"),
                    )
                    .on_hover_text(t!("Öffnet die Seite neu in einem eigenen Fenster"))
                    .clicked()
                    {
                        switch = Some(false);
                    }
                } else {
                    if theme::btn(
                        ui,
                        theme::Btn::Small,
                        Some(Icon::External),
                        t!("Fenster nach vorn"),
                    )
                    .clicked()
                    {
                        front = true;
                    }
                    if crate::ui::web_embed::can_embed()
                        && theme::btn(
                            ui,
                            theme::Btn::Small,
                            Some(Icon::Expand),
                            t!("Im Tab einbetten"),
                        )
                        .on_hover_text(t!("Öffnet die Seite neu eingebettet in diesem Tab"))
                        .clicked()
                    {
                        switch = Some(true);
                    }
                }
            }
            if t.refused.is_some()
                && theme::btn(
                    ui,
                    theme::Btn::Small,
                    Some(Icon::Edit),
                    t!("Eintrag bearbeiten"),
                )
                .clicked()
            {
                edit = true;
            }
        },
    );
    if t.embedded && !t.show_info && !closed && in_main {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::bg_deep()))
            .show(ui, |ui| t.body(ui, tab_id, &pinned));
    } else {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::bg_deep())
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if t.embedded && !in_main {
                        ui.add(
                            egui::Label::new(
                                RichText::new(t!(
                                    "Die Seite ist in das Hauptfenster eingebettet und hier nicht sichtbar. Öffne sie in einem eigenen Fenster oder hole den Tab zurück."
                                ))
                                .color(theme::amber()),
                            )
                            .wrap(),
                        );
                        ui.add_space(10.0);
                    }
                    t.body(ui, tab_id, &pinned);
                });
            });
    }
    if toggle_info {
        t.show_info = !t.show_info;
    }
    if front {
        t.send(WebCmd::Focus);
    }
    if let Some(embed) = switch
        && let Some(h) = host_id.and_then(|h| app.lib.host(h)).cloned()
    {
        // The mode is chosen when the helper starts: reopen the entry the other way.
        app.close_tab(tab_id);
        crate::ui::web_embed::force_next_open(embed);
        // After the frame: the new tab becomes active then (the pane click of this very
        // frame must not win).
        actions.push(Action::Connect(h.id));
        return;
    }
    if reload {
        t.declined = false;
        t.refused = None;
        t.send(WebCmd::Reload);
    }
    if close {
        t.close();
    }
    if edit && let Some(h) = host_id {
        actions.push(Action::EditHost(h));
    }
    if reopen {
        let ctx = ui.ctx().clone();
        if let Some(h) = host_id.and_then(|h| app.lib.host(h)).cloned() {
            app.close_tab(tab_id);
            app.connect_host(h, crate::app::Purpose::Terminal, &ctx);
        }
    }
}

// ------------------------------------------------------------------------------------
// Certificate dialog
// ------------------------------------------------------------------------------------

/// Asks whether to pin the certificate a Web entry presented (or, for a changed one,
/// warns loudly). Shown for the first tab that waits for an answer.
pub fn cert_dialog(app: &mut App, ctx: &egui::Context) {
    let Some((tab_id, prompt, address)) = app.tabs.iter().find_map(|tab| match &tab.kind {
        TabKind::Web(w) => w
            .cert
            .clone()
            .map(|c| (tab.id, c, format!("{} ({})", tab.title, w.address))),
        _ => None,
    }) else {
        return;
    };
    let changed = prompt.expected.is_some();
    let host_name = address
        .rsplit_once('(')
        .map(|(_, a)| a.trim_end_matches(')').to_string())
        .unwrap_or_default();
    let host_only = host_name
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| host_name.clone());
    // The page window may cover Aurora: bring Aurora forward once per prompt.
    let seen_id = egui::Id::new(("web-cert-seen", &prompt.fingerprint));
    if !ctx.data(|d| d.get_temp::<bool>(seen_id)).unwrap_or(false) {
        ctx.data_mut(|d| d.insert_temp(seen_id, true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    let typed_id = egui::Id::new("web-cert-typed");
    let mut typed: String = ctx.data(|d| d.get_temp(typed_id)).unwrap_or_default();
    let unlocked = !changed || typed.trim().eq_ignore_ascii_case(&host_only);

    let mut answer: Option<bool> = None;
    egui::Modal::new(egui::Id::new("web-cert"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(540.0);
            if changed {
                ui.label(
                    theme::heading(t!("Zertifikat hat sich geändert"), 20.0).color(theme::red()),
                );
                ui.label(
                    RichText::new(t!("Das passiert nach einer Neuinstallation oder einem Zertifikatswechsel, kann aber auch ein Angriff sein (jemand gibt sich als dieses Gerät aus). Vertraue nur, wenn du den neuen Fingerabdruck am Gerät geprüft hast."))
                        .color(theme::text_dim()),
                );
            } else {
                ui.label(theme::heading(t!("Selbstsigniertes Zertifikat anheften?"), 20.0));
                ui.label(
                    RichText::new(t!("Das Gerät weist sich mit einem Zertifikat aus, dem das System nicht vertraut. Vergleiche den Fingerabdruck mit der Anzeige am Gerät (oder vertraue deinem eigenen Netz). Aurora akzeptiert danach nur genau dieses Zertifikat."))
                        .color(theme::text_dim()),
                );
            }
            ui.add_space(8.0);
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::mono(&address, 13.0).color(theme::text()));
                ui.add_space(4.0);
                ui.label(RichText::new("SHA-256").size(12.0).color(theme::text_dim()));
                ui.add(
                    egui::Label::new(theme::mono(&prompt.fingerprint, 13.0).color(theme::accent()))
                        .wrap(),
                );
                if let Some(prev) = &prompt.expected {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(t!("bisher angeheftet"))
                            .size(12.0)
                            .color(theme::text_dim()),
                    );
                    ui.add(
                        egui::Label::new(theme::mono(prev, 12.5).color(theme::red())).wrap(),
                    );
                }
            });
            if changed {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(t!(
                        "Wenn du trotzdem vertrauen willst, tippe den Host ein: {}",
                        host_only
                    ))
                    .size(12.5)
                    .color(theme::red()),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .hint_text(host_only.as_str())
                        .font(egui::FontId::monospace(13.0))
                        .desired_width(f32::INFINITY),
                );
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if changed {
                    t!("Neu anheften und öffnen")
                } else {
                    t!("Anheften und öffnen")
                };
                let trust = ui.add_enabled_ui(unlocked, |ui| theme::primary_button(ui, label));
                if trust.inner.clicked() {
                    answer = Some(true);
                }
                if ui.button(t!("Abbrechen")).clicked() {
                    answer = Some(false);
                }
            });
        });
    match answer {
        Some(accept) => {
            ctx.data_mut(|d| d.remove::<String>(typed_id));
            let mut pin: Option<(Uuid, String)> = None;
            if let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab_id)
                && let TabKind::Web(w) = &mut tab.kind
            {
                w.cert = None;
                if accept {
                    w.refused = None;
                    w.declined = false;
                    w.send(WebCmd::TrustCert {
                        fingerprint: prompt.fingerprint.clone(),
                    });
                    pin = Some((w.host_id, w.address.clone()));
                } else {
                    w.declined = true;
                }
            }
            if let Some((host, scope)) = pin
                && let Some(h) = app.lib.hosts.iter_mut().find(|h| h.id == host)
            {
                h.web.pinned_fp = prompt.fingerprint.clone();
                h.web.pinned_for = scope;
                h.web.pinned_at = Some(chrono::Utc::now());
                app.save_library();
            }
        }
        None => {
            ctx.data_mut(|d| d.insert_temp(typed_id, typed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_labels_cover_modes() {
        assert_eq!(tls_labels().len(), 3);
    }
}
