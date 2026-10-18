//! Host key dialog: a new or changed server key, with second opinions (SSHFP in DNS,
//! the jump host's view) and a helper to compare on the server console.

use egui::{Color32, RichText};

use crate::app::App;
use crate::backend::hostkey_check::{
    CheckState, Finding, Verdict, console_fingerprint_command, verdict,
};
use crate::theme;
use crate::ui::icons::{self, Icon};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.hostkey_queue.front() else {
        return;
    };
    let changed = p.previous.is_some();
    let checks: Option<CheckState> = p
        .checks
        .as_ref()
        .and_then(|c| c.lock().ok().map(|s| s.clone()));
    let hostname = p
        .host
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(&p.host)
        .to_string();
    let saved = app
        .lib
        .hosts
        .iter()
        .find(|h| format!("{}:{}", h.hostname.trim(), h.port) == p.host);
    let proxmox_guest = saved.is_some_and(|h| {
        h.notes.starts_with("Proxmox ")
            || h.group_id
                .and_then(|g| app.lib.group(g))
                .is_some_and(|g| g.name.starts_with("Proxmox "))
    });
    let previous_since = p.previous.as_ref().and_then(|prev| {
        app.lib
            .host_key_history
            .get(&p.host)?
            .iter()
            .rev()
            .find(|r| &r.fingerprint == prev)?
            .trusted
    });
    let verdict = checks.as_ref().map(verdict);
    let blocked = matches!(verdict, Some(Verdict::Contradicts(_)));
    let typed_id = egui::Id::new("hostkey-typed");
    let mut typed: String = ctx.data(|d| d.get_temp(typed_id)).unwrap_or_default();
    let unlocked = !blocked || typed.trim() == hostname;
    let ssh_key = !p.algorithm.starts_with("RDP");

    let mut answer: Option<(bool, bool)> = None; // (accept, remember)
    egui::Modal::new(egui::Id::new("hostkey"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(540.0);
            if changed {
                ui.label(theme::heading(t!("Server-Schlüssel geändert"), 20.0).color(theme::red()));
                ui.label(
                    RichText::new(t!("Das passiert nach einer Neuinstallation, kann aber auch ein Angriff sein. Verbinde nur, wenn du den neuen Fingerprint geprüft hast."))
                        .color(theme::text_dim()),
                );
            } else {
                ui.label(theme::heading(t!("Neuer Server"), 20.0));
                ui.label(
                    RichText::new(t!("Vergleiche den Fingerprint mit dem Server, bevor du ihm vertraust."))
                        .color(theme::text_dim()),
                );
            }
            ui.add_space(8.0);
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::mono(&p.host, 13.0).color(theme::text()));
                ui.label(RichText::new(&p.algorithm).size(12.5).color(theme::text_dim()));
                ui.add_space(4.0);
                ui.label(theme::mono(&p.fingerprint, 13.0).color(theme::accent()));
                if let Some(prev) = &p.previous {
                    let since = previous_since
                        .map(|d| {
                            format!(
                                " · {}",
                                t!(
                                    "vertraut seit {}",
                                    d.with_timezone(&chrono::Local).format("%d.%m.%Y")
                                )
                            )
                        })
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(format!("{}{since}", t!("vorher: {}", prev)))
                            .monospace()
                            .size(11.5)
                            .color(theme::red()),
                    );
                }
            });

            if let (Some(state), Some(v)) = (&checks, &verdict) {
                ui.add_space(10.0);
                verdict_banner(ui, v);
                ui.add_space(6.0);
                if let Some(f) = &state.dns {
                    source_row(ui, t!("DNS (SSHFP)"), f);
                }
                if let Some((name, f)) = &state.jump {
                    source_row(ui, &t!("Jump-Host {}", name), f);
                }
                if proxmox_guest {
                    source_row(
                        ui,
                        t!("Proxmox"),
                        &Finding::Nothing(
                            t!("Proxmox-Gast – keine zweite Quelle automatisch; vergleiche über die Konsole").to_string(),
                        ),
                    );
                }
            }

            if ssh_key {
                ui.add_space(10.0);
                let cmd = console_fingerprint_command(
                    p.algorithm.trim_end_matches(" (cert)"),
                );
                ui.label(RichText::new(t!("Fingerabdruck vom Server")).strong().color(theme::text()));
                ui.label(
                    RichText::new(t!(
                        "Auf der Server-Konsole ausführen (z.B. Proxmox noVNC, iLO/IPMI, Cloud-Konsole) und mit dem Fingerprint oben vergleichen:"
                    ))
                    .size(12.5)
                    .color(theme::text_dim()),
                );
                ui.horizontal(|ui| {
                    egui::Frame::NONE
                        .fill(theme::fill())
                        .corner_radius(egui::CornerRadius::same(7))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(theme::mono(&cmd, 12.5).color(theme::text())).selectable(true));
                        });
                    if theme::icon_btn(ui, Icon::Copy, t!("Kopieren")).clicked() {
                        ui.ctx().copy_text(cmd.clone());
                    }
                });
            }

            if blocked {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(t!(
                        "Eine zweite Quelle widerspricht. Wenn du trotzdem verbinden willst, tippe den Hostnamen ein: {}",
                        hostname
                    ))
                    .size(12.5)
                    .color(theme::red()),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .hint_text(hostname.as_str())
                        .font(egui::FontId::monospace(13.0))
                        .desired_width(f32::INFINITY),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let trust = ui.add_enabled_ui(unlocked, |ui| theme::primary_button(ui, t!("Vertrauen")));
                    if trust.inner.clicked() {
                        answer = Some((true, true));
                    }
                    let once = ui.add_enabled_ui(unlocked, |ui| ui.button(t!("Nur dieses Mal")));
                    if once.inner.clicked() {
                        answer = Some((true, false));
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        answer = Some((false, false));
                    }
                });
            });
        });
    if let Some((accept, remember)) = answer
        && let Some(p) = app.hostkey_queue.pop_front()
    {
        ctx.data_mut(|d| d.remove::<String>(typed_id));
        if accept && remember {
            let alg = p.algorithm.clone();
            app.lib.remember_host_key(&p.host, &p.fingerprint, &alg);
            app.save_library();
        }
        let _ = p.reply.send(accept);
    } else {
        ctx.data_mut(|d| d.insert_temp(typed_id, typed));
    }
}

fn verdict_banner(ui: &mut egui::Ui, v: &Verdict) {
    let (color, icon, text): (Color32, Icon, String) = match v {
        Verdict::Checking => (
            theme::text_dim(),
            Icon::Refresh,
            t!("Prüfe zweite Quellen…").to_string(),
        ),
        Verdict::Confirmed(by) => (theme::green(), Icon::Shield, t!("Bestätigt durch {}", by)),
        Verdict::Contradicts(by) => (theme::red(), Icon::Shield, t!("Widerspricht: {}", by)),
        Verdict::NoSource => (
            theme::amber(),
            Icon::Help,
            t!("Keine zweite Quelle").to_string(),
        ),
    };
    egui::Frame::NONE
        .fill(theme::soft(color))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if matches!(v, Verdict::Checking) {
                    ui.add(egui::Spinner::new().size(16.0).color(color));
                } else {
                    icons::show(ui, icon, 18.0, color);
                }
                ui.label(RichText::new(text).font(theme::semibold(14.0)).color(color));
            });
        });
}

fn source_row(ui: &mut egui::Ui, name: &str, f: &Finding) {
    let (color, text) = match f {
        Finding::Pending => (theme::text_dim(), t!("wird geprüft…").to_string()),
        Finding::Match(t) => (theme::green(), t.clone()),
        Finding::Mismatch(t) => (theme::red(), t.clone()),
        Finding::Nothing(t) => (theme::text_dim(), t.clone()),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        ui.label(RichText::new(name).size(12.5).strong().color(theme::text()));
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).wrap());
    });
}
