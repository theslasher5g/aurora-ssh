//! WinRM in the app: the host form section, refreshing the inventory facts of Windows
//! hosts through WinRM and building connection targets for "Run on many hosts".

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use egui::RichText;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::app::App;
use crate::backend::Sink;
use crate::backend::inventory_collect::Collected;
use crate::backend::inventory_rules as rules;
use crate::backend::winrm_client::open_trusted;
use crate::backend::winrm_collect;
use crate::backend::winrm_http::Target;
use crate::backend::winrm_types::{WinrmAuth, pin_key};
use crate::model::{Host, Protocol};
use crate::theme;

pub struct Done {
    host: Uuid,
    result: Result<Collected, String>,
}

#[derive(Default)]
pub struct WinrmState {
    io: Option<(Sink<Done>, UnboundedReceiver<Done>)>,
    busy: HashSet<Uuid>,
}

/// Whether the host can be reached through WinRM.
pub fn usable(h: &Host) -> bool {
    h.protocol == Protocol::Rdp && h.winrm.enabled
}

/// Connection target from the host entry and its keychain identity. With
/// `require_pin`, an HTTPS certificate that was never confirmed is an error (batch runs
/// never open a dialog).
pub fn target_for(app: &App, h: &Host, require_pin: bool) -> Result<Target, String> {
    if !usable(h) {
        return Err(t!("WinRM ist für diesen Host nicht eingerichtet").to_string());
    }
    let (user, ident) = app.credentials(h);
    let password = ident
        .as_ref()
        .map(|i| i.password.clone())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            t!("Kein Passwort gespeichert – im Host eine Identität mit Passwort hinterlegen")
                .to_string()
        })?;
    if user.is_empty() {
        return Err(t!("Kein Benutzername für diesen Host").to_string());
    }
    let host = h.hostname.trim().to_string();
    let port = h.winrm.effective_port();
    let pin = h
        .winrm
        .https
        .then(|| app.lib.known_hosts.get(&pin_key(&host, port)).cloned())
        .flatten();
    if require_pin && h.winrm.https && pin.is_none() {
        return Err(t!(
            "Zertifikat noch nicht bestätigt – einmal „Fakten per WinRM aktualisieren“ ausführen"
        )
        .to_string());
    }
    Ok(Target {
        host,
        port,
        https: h.winrm.https,
        auth: h.winrm.auth,
        user,
        domain: h.rdp.domain.trim().to_string(),
        password,
        pin,
    })
}

/// Refresh of the inventory facts for these hosts (empty = all WinRM hosts).
pub fn start(app: &mut App, ctx: &egui::Context, ids: &[Uuid]) {
    let hosts: Vec<Host> = app
        .lib
        .hosts
        .iter()
        .filter(|h| usable(h) && (ids.is_empty() || ids.contains(&h.id)))
        .cloned()
        .collect();
    if hosts.is_empty() {
        return;
    }
    let sink = app
        .srv
        .winrm
        .io
        .get_or_insert_with(|| Sink::new(ctx.clone()))
        .0
        .clone();
    for h in hosts {
        if app.srv.winrm.busy.contains(&h.id) {
            continue;
        }
        let target = match target_for(app, &h, false) {
            Ok(t) => t,
            Err(e) => {
                sink.send(Done {
                    host: h.id,
                    result: Err(e),
                });
                continue;
            }
        };
        let chain = rules::group_path(&app.lib, &h).1;
        let snippets: Vec<_> = app
            .lib
            .settings
            .inventory
            .info_snippets
            .iter()
            .filter(|s| s.applies(&h, &chain))
            .cloned()
            .collect();
        app.srv.winrm.busy.insert(h.id);
        let prompts = app.prompts_sink.clone();
        let (sink, id) = (sink.clone(), h.id);
        app.rt.spawn(async move {
            let cancel = Arc::new(AtomicBool::new(false));
            let work = tokio::task::spawn_blocking(move || {
                let (mut s, _) =
                    open_trusted(&target, Some(&prompts)).map_err(|e| format!("{e:#}"))?;
                let r = winrm_collect::collect(&mut s, &snippets, true, &cancel);
                s.close();
                r.map_err(|e| e.to_string())
            });
            // The certificate dialog may be open for a while; the collection itself is bounded.
            let result = match tokio::time::timeout(std::time::Duration::from_secs(900), work).await
            {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err(t!("Zeitüberschreitung").to_string()),
            };
            sink.send(Done { host: id, result });
        });
    }
}

/// Takes finished refreshes into the library; called every frame.
pub fn pump(app: &mut App) {
    let mut events = Vec::new();
    if let Some((_, rx)) = app.srv.winrm.io.as_mut() {
        while let Ok(d) = rx.try_recv() {
            events.push(d);
        }
    }
    for d in events {
        app.srv.winrm.busy.remove(&d.host);
        let name = app
            .lib
            .host(d.host)
            .map(|h| h.display_name().to_string())
            .unwrap_or_default();
        match d.result {
            Ok(c) => {
                app.lib.facts.insert(d.host, c.facts);
                app.lib.detail.insert(d.host, c.detail);
                app.save_library();
                app.toast(
                    t!("Fakten von {} per WinRM aktualisiert", name),
                    theme::green(),
                );
            }
            Err(e) => app.toast(t!("WinRM {}: {}", name, e), theme::red()),
        }
    }
}

/// "WinRM" section of the host form (Windows hosts).
pub fn form(ui: &mut egui::Ui, h: &mut Host) {
    theme::form_caption(ui, "WinRM");
    theme::form(ui, 120.0, |ui, f| {
        f.switch(ui, t!("Fakten und Befehle per WinRM"), &mut h.winrm.enabled);
        if !h.winrm.enabled {
            return;
        }
        f.switch(ui, "HTTPS", &mut h.winrm.https);
        f.field(ui, "Port", |ui| {
            ui.add(egui::DragValue::new(&mut h.winrm.port).range(0..=65535));
            ui.label(
                RichText::new(if h.winrm.https {
                    t!("0 = Standard (5986)")
                } else {
                    t!("0 = Standard (5985)")
                })
                .size(12.5)
                .color(theme::text_dim()),
            );
        });
        f.field(ui, t!("Anmeldung"), |ui| {
            if !h.winrm.https {
                h.winrm.auth = WinrmAuth::Ntlm;
            }
            if let Some(i) = theme::segmented(
                ui,
                &["NTLM", "Basic"],
                usize::from(h.winrm.auth == WinrmAuth::Basic),
            ) && h.winrm.https
            {
                h.winrm.auth = if i == 1 {
                    WinrmAuth::Basic
                } else {
                    WinrmAuth::Ntlm
                };
            }
        });
        f.field(ui, "", |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(if h.winrm.https {
                        t!("Benutzer und Passwort kommen aus der Identität des Hosts; das Zertifikat wird beim ersten Mal bestätigt und festgehalten.")
                    } else {
                        t!("Ohne HTTPS ist der Verkehr unverschlüsselt; Windows verlangt dafür „AllowUnencrypted“. HTTPS (5986) wird empfohlen.")
                    })
                    .size(12.0)
                    .color(theme::text_dim()),
                )
                .wrap(),
            );
        });
    });
}
