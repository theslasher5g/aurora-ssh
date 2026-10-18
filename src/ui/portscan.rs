//! "Ports scannen" dialog for one host: TCP scan from this PC or the listening ports as
//! the server sees them; open ports can be opened in the browser or reached through a
//! new SSH tunnel.

use egui::{RichText, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{Action, App, Purpose};
use crate::backend::portscan::{self, OpenPort, PortEvent};
use crate::backend::{Sink, ssh::SshParams};
use crate::model::Protocol;
use crate::theme;
use crate::ui::icons::Icon;

pub struct PortScan {
    pub host: Uuid,
    pub name: String,
    pub address: String,
    /// The host is an SSH host: "on the server" mode and tunnels are possible.
    pub ssh: bool,
    /// 0 = from this PC, 1 = listening on the server.
    pub mode: usize,
    /// 0 known ports, 1 = 1–1024, 2 = all, 3 = own list.
    pub preset: usize,
    pub custom: String,
    pub results: Vec<OpenPort>,
    pub progress: (usize, usize),
    pub running: bool,
    pub error: Option<String>,
    pub rx: Option<UnboundedReceiver<PortEvent>>,
}

impl PortScan {
    pub fn new(app: &App, host: Uuid) -> Option<Self> {
        let h = app.lib.host(host)?;
        Some(Self {
            host,
            name: h.display_name().to_string(),
            address: h.hostname.trim().to_string(),
            ssh: h.protocol == Protocol::Ssh,
            mode: 0,
            preset: 0,
            custom: "1-1024, 3000, 8000-8100".into(),
            results: Vec::new(),
            progress: (0, 0),
            running: false,
            error: None,
            rx: None,
        })
    }

    fn ports(&self) -> anyhow::Result<Vec<u16>> {
        match self.preset {
            0 => Ok(portscan::KNOWN.iter().map(|(p, _)| *p).collect()),
            1 => Ok((1..=1024).collect()),
            2 => Ok((1..=65535).collect()),
            _ => portscan::parse_ports(&self.custom),
        }
    }

    fn reset(&mut self, ctx: &egui::Context) -> Sink<PortEvent> {
        let (sink, rx) = Sink::new(ctx.clone());
        self.rx = Some(rx);
        self.results.clear();
        self.error = None;
        self.running = true;
        self.progress = (0, 0);
        sink
    }
}

/// Starts the server-side list once `connect_host` has the login (`Purpose::ListPorts`).
pub fn start_listening(app: &mut App, params: SshParams, ctx: &egui::Context) {
    let prompts = app.prompts_sink.clone();
    let Some(s) = app.port_scan.as_mut() else {
        return;
    };
    let sink = s.reset(ctx);
    portscan::spawn_listening(&app.rt, params, sink, prompts);
}

fn start(app: &mut App, ctx: &egui::Context) {
    let Some(s) = app.port_scan.as_mut() else {
        return;
    };
    if s.mode == 1 {
        let host = s.host;
        s.running = true;
        s.error = None;
        s.results.clear();
        if let Some(h) = app.lib.host(host).cloned() {
            app.connect_host(h, Purpose::ListPorts, ctx);
        }
        return;
    }
    match s.ports() {
        Ok(ports) => {
            let sink = s.reset(ctx);
            s.progress = (0, ports.len());
            portscan::spawn_tcp(&app.rt, s.address.clone(), ports, sink);
        }
        Err(e) => s.error = Some(e.to_string()),
    }
}

/// New tunnel `localhost:<free port>` → `localhost:<port>` on the host, switched on.
fn tunnel(app: &App, host: Uuid, port: u16) -> Option<(Uuid, crate::model::Tunnel)> {
    let h = app.lib.host(host)?;
    if let Some(t) = h.tunnels.iter().find(|t| {
        t.remote_port == port && matches!(t.remote_host.as_str(), "localhost" | "127.0.0.1")
    }) {
        return Some((h.id, t.clone()));
    }
    let used: Vec<u16> = app
        .lib
        .hosts
        .iter()
        .flat_map(|h| h.tunnels.iter().map(|t| t.local_port))
        .collect();
    let free = |p: u16| !used.contains(&p) && std::net::TcpListener::bind(("127.0.0.1", p)).is_ok();
    let local = [port, port.saturating_add(10000), port.saturating_add(20000)]
        .into_iter()
        .filter(|p| *p >= 1024)
        .find(|p| free(*p))
        .or_else(|| (20000..60000).find(|p| free(*p)))?;
    let name = match portscan::service(port) {
        "" => format!("Port {port}"),
        s => s.to_string(),
    };
    Some((
        h.id,
        crate::model::Tunnel {
            name,
            local_port: local,
            remote_host: "localhost".into(),
            remote_port: port,
            open_browser: portscan::web_scheme(port).is_some(),
            ..Default::default()
        },
    ))
}

pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some(s) = app.port_scan.as_mut() else {
        return;
    };
    // Collect results.
    let mut finished = false;
    while let Some(ev) = s.rx.as_mut().and_then(|rx| rx.try_recv().ok()) {
        match ev {
            PortEvent::Progress(d, t) => s.progress = (d, t),
            PortEvent::Open(p) => {
                if !s.results.iter().any(|r| r.port == p.port) {
                    s.results.push(p);
                    s.results.sort_by_key(|r| r.port);
                }
            }
            PortEvent::Error(e) => {
                s.error = Some(e);
                s.running = false;
            }
            PortEvent::Done => {
                s.running = false;
                finished = true;
            }
        }
    }
    // Remember the open ports (reachable from outside) for the certificate radar.
    if finished {
        let ports: Vec<u16> = s
            .results
            .iter()
            .filter(|p| !p.local_only())
            .map(|p| p.port)
            .collect();
        let host = s.host;
        app.lib.net.open_ports.insert(host, ports);
        app.save_library();
    }
    let Some(s) = app.port_scan.as_mut() else {
        return;
    };
    let title = t!("Ports · {}", s.name);
    let (running, count, scanned) = (s.running, s.results.len(), s.progress.1 > 0);
    let mut cancel = false;
    let mut run = false;
    let mut new_tunnel = None;
    let ((), dismissed) = theme::sheet(
        ctx,
        egui::Id::new("port-scan"),
        560.0,
        &title,
        |ui| {
            ui.label(theme::mono(&s.address, 13.0).color(theme::text_dim()));
            ui.add_space(6.0);
            if s.ssh {
                let modes = [t!("Von hier (TCP)"), t!("Auf dem Server lauschend")];
                if let Some(i) = theme::segmented(ui, &modes, s.mode) {
                    s.mode = i;
                }
                ui.label(
                    RichText::new(if s.mode == 0 {
                        t!("Verbindet sich von diesem PC zu jedem Port – zeigt, was von hier erreichbar ist.")
                    } else {
                        t!("Fragt den Server per SSH (ss -tln) – zeigt auch Dienste, die nur auf localhost lauschen und per Tunnel erreichbar sind.")
                    })
                    .size(12.0)
                    .color(theme::text_dim()),
                );
            }
            if s.mode == 0 {
                ui.add_space(4.0);
                let presets = [
                    t!("Bekannte Ports"),
                    "1–1024",
                    t!("Alle (1–65535)"),
                    t!("Eigene"),
                ];
                if let Some(i) = theme::segmented(ui, &presets, s.preset) {
                    s.preset = i;
                }
                if s.preset == 3 {
                    ui.add(
                        egui::TextEdit::singleline(&mut s.custom)
                            .hint_text("22, 80, 8000-8100")
                            .font(egui::FontId::monospace(13.0))
                            .desired_width(f32::INFINITY),
                    );
                }
            }
            ui.add_space(6.0);
            if s.running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let (d, t) = s.progress;
                    ui.label(if t > 0 {
                        t!("{} von {} Ports geprüft", d, t)
                    } else {
                        t!("Frage den Server…").to_string()
                    });
                });
                if s.progress.1 > 0 {
                    ui.add(
                        egui::ProgressBar::new(s.progress.0 as f32 / s.progress.1.max(1) as f32)
                            .desired_height(4.0),
                    );
                }
            }
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(theme::red()));
            }
            if !s.running && s.results.is_empty() && s.error.is_none() && s.progress.1 > 0 {
                ui.label(RichText::new(t!("Kein offener Port gefunden")).color(theme::text_dim()));
            }
            for p in &s.results {
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 40.0),
                    egui::Sense::hover(),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(Vec2::new(4.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let ui = &mut child;
                ui.label(theme::mono(format!("{:>5}", p.port), 14.0).color(theme::text()));
                ui.add_space(6.0);
                let svc = portscan::service(p.port);
                let what = p
                    .process
                    .clone()
                    .or_else(|| p.banner.clone())
                    .unwrap_or_else(|| svc.to_string());
                ui.add(
                    egui::Label::new(RichText::new(what).size(13.0).color(theme::text()))
                        .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let host_reachable = !p.local_only();
                    // Fixed slot for the browser button so the tunnel buttons line up.
                    match portscan::web_scheme(p.port).filter(|_| host_reachable) {
                        Some(scheme) => {
                            if theme::icon_btn(ui, Icon::External, t!("Im Browser öffnen")).clicked()
                            {
                                let host = if s.address.contains(':') {
                                    format!("[{}]", s.address)
                                } else {
                                    s.address.clone()
                                };
                                crate::update::open_url(&format!("{scheme}://{host}:{}", p.port));
                            }
                        }
                        None => ui.add_space(32.0),
                    }
                    if s.ssh
                        && theme::btn(ui, theme::Btn::Small, Some(Icon::Tunnel), t!("Tunnel"))
                            .on_hover_text(t!(
                                "Über SSH auf einen lokalen Port holen (auch wenn der Dienst nur auf localhost lauscht)"
                            ))
                            .clicked()
                    {
                        new_tunnel = Some(p.port);
                    }
                    if let Some(ms) = p.ms {
                        ui.label(theme::mono(format!("{ms} ms"), 11.5).color(theme::text_dim()));
                    }
                    if p.local_only() {
                        theme::tag(ui, t!("nur localhost"), Some(theme::amber()));
                    } else if let Some(b) = &p.bind
                        && s.mode == 1
                    {
                        ui.label(theme::mono(b, 11.5).color(theme::text_dim()));
                    }
                });
            }
        },
        |ui, ()| {
            let label = if running {
                t!("Läuft…")
            } else if count == 0 && !scanned {
                t!("Scannen")
            } else {
                t!("Erneut scannen")
            };
            if ui
                .add_enabled_ui(!running, |ui| {
                    theme::btn(ui, theme::Btn::Primary, Some(Icon::Search), label)
                })
                .inner
                .clicked()
            {
                run = true;
            }
            if running && theme::btn(ui, theme::Btn::Normal, None, t!("Abbrechen")).clicked() {
                cancel = true;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if count > 0 {
                    ui.label(
                        RichText::new(t!("{} offen", count))
                            .size(12.5)
                            .color(theme::text_dim()),
                    );
                }
            });
        },
    );
    if dismissed {
        app.port_scan = None;
        return;
    }
    if cancel && let Some(s) = app.port_scan.as_mut() {
        s.rx = None;
        s.running = false;
    }
    if run {
        start(app, ctx);
    }
    if let Some(port) = new_tunnel
        && let Some(host) = app.port_scan.as_ref().map(|s| s.host)
        && let Some((host_id, t)) = tunnel(app, host, port)
    {
        let tid = t.id;
        if let Some(h) = app.lib.hosts.iter_mut().find(|h| h.id == host_id)
            && !h.tunnels.iter().any(|x| x.id == tid)
        {
            h.tunnels.push(t.clone());
            app.save_library();
        }
        if !app.tunnels.contains_key(&tid) {
            actions.push(Action::ToggleTunnel(host_id, tid));
        }
        app.toast(
            t!("Tunnel: localhost:{} → {}", t.local_port, port),
            theme::green(),
        );
    }
}
