//! Wake-on-LAN in the UI: "Aufwecken" / "Aufwecken und verbinden" in the host list (with
//! a progress pill while Aurora waits for the port) and the MAC fields of the host editor.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Vec2};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::{Action, App};
use crate::backend::Sink;
use crate::backend::wol::{self, WakeEvent, WakeJob};
use crate::model::Host;
use crate::theme;
use crate::ui::icons::Icon;

pub struct Waker {
    pub host: Uuid,
    name: String,
    proto: &'static str,
    connect: bool,
    started: Instant,
    sent: bool,
    rx: UnboundedReceiver<WakeEvent>,
    cancel: Arc<AtomicBool>,
}

struct Lookup {
    host: Uuid,
    rx: UnboundedReceiver<Result<[u8; 6], String>>,
}

#[derive(Default)]
pub struct WolState {
    wakers: Vec<Waker>,
    lookup: Option<Lookup>,
}

impl WolState {
    pub fn waiting(&self, host: Uuid) -> Option<&Waker> {
        self.wakers.iter().find(|w| w.host == host)
    }

    /// "wartet auf SSH… 23 s" for the host list.
    pub fn pill_text(&self, host: Uuid) -> Option<String> {
        let w = self.waiting(host)?;
        Some(if !w.sent {
            t!("weckt…").to_string()
        } else {
            t!(
                "wartet auf {}… {} s",
                w.proto,
                w.started.elapsed().as_secs()
            )
        })
    }

    pub fn looking_up(&self, host: Uuid) -> bool {
        self.lookup.as_ref().is_some_and(|l| l.host == host)
    }
}

/// The host has a usable MAC address.
pub fn can_wake(h: &Host) -> bool {
    h.protocol.is_network() && wol::parse_mac(&h.wol.mac).is_some()
}

/// Sends the magic packet (from this PC or the host's jump host) and, with `connect`,
/// waits for the port and connects.
pub fn wake(app: &mut App, id: Uuid, connect: bool, ctx: &egui::Context) {
    let Some(h) = app.lib.host(id).cloned() else {
        return;
    };
    let Some(mac) = wol::parse_mac(&h.wol.mac) else {
        app.toast(
            t!("{}: keine gültige MAC-Adresse hinterlegt", h.display_name()),
            theme::red(),
        );
        return;
    };
    if app.net.wol.waiting(id).is_some() {
        return;
    }
    // Remote subnet: the jump host next to the target sends the packet.
    let via = match h.jump_host.and_then(|j| app.lib.host(j).cloned()) {
        Some(j) => match crate::ui::net::stored_params(app, &j) {
            Ok(p) => Some(p),
            Err(e) => {
                app.toast(format!("{e:#}"), theme::red());
                return;
            }
        },
        None => None,
    };
    let (broadcast, port) = wol::target_of(&h.wol.broadcast, h.wol.port);
    let job = WakeJob {
        mac,
        broadcast,
        port,
        via,
        wait_for: connect.then(|| (h.hostname.trim().to_string(), h.port)),
    };
    let (sink, rx) = Sink::new(ctx.clone());
    let cancel = wol::spawn_wake(&app.rt, job, sink, app.prompts_sink.clone());
    app.net.wol.wakers.push(Waker {
        host: id,
        name: h.display_name().to_string(),
        proto: h.protocol.label(),
        connect,
        started: Instant::now(),
        sent: false,
        rx,
        cancel,
    });
}

pub fn cancel(app: &mut App, id: Uuid) {
    app.net.wol.wakers.retain(|w| {
        if w.host == id {
            w.cancel.store(true, Ordering::Relaxed);
        }
        w.host != id
    });
}

/// MAC address from the neighbour table (of this PC, or of the jump host).
pub fn lookup_mac(app: &mut App, host: Host, ctx: &egui::Context) {
    let address = host.hostname.trim().to_string();
    if address.is_empty() {
        app.toast(t!("Erst eine Adresse eintragen"), theme::amber());
        return;
    }
    let via = match host.jump_host.and_then(|j| app.lib.host(j).cloned()) {
        Some(j) => match crate::ui::net::stored_params(app, &j) {
            Ok(p) => Some(p),
            Err(e) => {
                app.toast(format!("{e:#}"), theme::red());
                return;
            }
        },
        None => None,
    };
    let (sink, rx) = Sink::new(ctx.clone());
    let prompts = app.prompts_sink.clone();
    let port = host.port;
    app.rt.spawn(async move {
        let r = match via {
            Some(p) => wol::lookup_via(p, address, prompts).await,
            None => wol::lookup_local(&address, port).await,
        };
        sink.send(r.map_err(|e| format!("{e:#}")));
    });
    app.net.wol.lookup = Some(Lookup { host: host.id, rx });
}

pub fn pump(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let mut toasts: Vec<(String, Color32)> = Vec::new();
    let mut ready = Vec::new();
    app.net.wol.wakers.retain_mut(|w| {
        while let Ok(ev) = w.rx.try_recv() {
            match ev {
                WakeEvent::Sent(tool) => {
                    w.sent = true;
                    if !w.connect {
                        let how = if tool.is_empty() {
                            String::new()
                        } else {
                            format!(" ({tool})")
                        };
                        toasts.push((
                            t!("Magic Packet an {} gesendet{}", w.name, how),
                            theme::green(),
                        ));
                        return false;
                    }
                }
                WakeEvent::Waiting => {}
                WakeEvent::Ready => {
                    toasts.push((t!("{} ist wach – verbinde…", w.name), theme::green()));
                    ready.push(w.host);
                    return false;
                }
                WakeEvent::Failed(e) => {
                    toasts.push((format!("{}: {e}", w.name), theme::red()));
                    return false;
                }
            }
        }
        true
    });
    if !app.net.wol.wakers.is_empty() {
        // The pill counts seconds.
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    for id in ready {
        app.failed.remove(&id);
        actions.push(Action::Connect(id));
    }
    if let Some(l) = app.net.wol.lookup.as_mut()
        && let Ok(r) = l.rx.try_recv()
    {
        let host = l.host;
        app.net.wol.lookup = None;
        match r {
            Ok(mac) => {
                let text = wol::format_mac(mac);
                if let Some(ed) = app.host_editor.as_mut().filter(|e| e.host.id == host) {
                    ed.host.wol.mac = text.clone();
                }
                toasts.push((t!("MAC-Adresse gefunden: {}", text), theme::green()));
            }
            Err(e) => toasts.push((e, theme::red())),
        }
    }
    for (text, color) in toasts {
        app.toast(text, color);
    }
}

/// Paints the "wartet auf SSH… 23 s" pill right-aligned at `right_center`.
pub fn paint_pill(p: &egui::Painter, right_center: Pos2, text: &str) -> Rect {
    let fg = theme::accent();
    let g = p.layout_no_wrap(text.to_string(), theme::medium(12.0), fg);
    let w = g.size().x + 34.0;
    let r = Rect::from_min_size(
        Pos2::new(right_center.x - w, right_center.y - 11.0),
        Vec2::new(w, 22.0),
    );
    p.rect_filled(r, CornerRadius::same(11), theme::soft(fg));
    let icon = Rect::from_center_size(Pos2::new(r.left() + 14.0, r.center().y), Vec2::splat(13.0));
    crate::ui::icons::paint(p, icon, Icon::Power, fg);
    p.galley(
        Pos2::new(r.left() + 25.0, r.center().y - g.size().y / 2.0),
        g,
        fg,
    );
    r
}

/// Editor request after drawing the Wake-on-LAN rows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorRequest {
    LookupMac,
}

/// "Wake-on-LAN" group of the host editor.
pub fn editor_section(ui: &mut egui::Ui, h: &mut Host, busy: bool) -> Option<EditorRequest> {
    let mut req = None;
    theme::form_caption(ui, "Wake-on-LAN");
    theme::form(ui, 92.0, |ui, f| {
        f.field(ui, t!("MAC-Adresse"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut h.wol.mac)
                    .hint_text(RichText::new("aa:bb:cc:dd:ee:ff").color(theme::text_dim()))
                    .font(FontId::monospace(13.0))
                    .desired_width(ui.available_width() - 128.0),
            );
            if busy {
                ui.add(egui::Spinner::new().size(14.0));
            } else if theme::btn(ui, theme::Btn::Small, None, t!("MAC ermitteln"))
                .on_hover_text(t!(
                    "Liest die ARP-/Nachbartabelle dieses PCs – oder die des Jump-Hosts, wenn der Host über einen erreicht wird"
                ))
                .clicked()
            {
                req = Some(EditorRequest::LookupMac);
            }
        });
        f.field(ui, "Broadcast", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut h.wol.broadcast)
                    .hint_text(RichText::new(wol::DEFAULT_BROADCAST).color(theme::text_dim()))
                    .font(FontId::monospace(13.0))
                    .desired_width(ui.available_width() - 86.0),
            );
            ui.label(RichText::new(":").color(theme::text_dim()));
            let mut port = if h.wol.port == 0 {
                wol::DEFAULT_PORT
            } else {
                h.wol.port
            };
            if ui
                .add(egui::DragValue::new(&mut port).range(1..=65535))
                .changed()
            {
                h.wol.port = port;
            }
        });
        let mac = h.wol.mac.trim();
        if !mac.is_empty() && wol::parse_mac(mac).is_none() {
            f.full(ui, |ui| {
                ui.label(
                    RichText::new(t!("Keine gültige MAC-Adresse"))
                        .size(12.5)
                        .color(theme::red()),
                );
            });
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Mit MAC-Adresse zeigt die Hostliste „Aufwecken“, wenn der Host aus ist. Hinter einem Jump-Host sendet der Jump-Host das Paket (Broadcast seines Subnetzes)."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            )
            .wrap(),
        );
    });
    req
}
