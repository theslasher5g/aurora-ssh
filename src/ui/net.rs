//! Network helpers around the host list: route finder (`ui::route`), certificate radar
//! (`ui::certradar`) and Wake-on-LAN (`ui::wol`). One state field and one action type in
//! `App`; everything else lives here.

use anyhow::{Result, anyhow};
use uuid::Uuid;

use crate::app::{Action, App};
use crate::backend::ssh::SshParams;
use crate::model::Host;

#[derive(Default)]
pub struct NetState {
    pub route: Option<crate::ui::route::RouteFinder>,
    pub radar: crate::ui::certradar::Radar,
    pub wol: crate::ui::wol::WolState,
}

#[derive(Clone, Debug)]
pub enum NetAction {
    /// Look for a jump host that reaches this host (sheet).
    FindRoute(Uuid),
    /// Send the magic packet; `true` = then wait for the port and connect.
    Wake(Uuid, bool),
    CancelWake(Uuid),
    OpenRadar,
}

pub fn apply(app: &mut App, a: NetAction, ctx: &egui::Context) {
    match a {
        NetAction::FindRoute(id) => {
            if let Some(h) = app.lib.host(id).cloned() {
                crate::ui::route::start(app, h, crate::ui::route::Mode::Sheet, ctx);
            }
        }
        NetAction::Wake(id, connect) => crate::ui::wol::wake(app, id, connect, ctx),
        NetAction::CancelWake(id) => crate::ui::wol::cancel(app, id),
        NetAction::OpenRadar => crate::ui::certradar::open(app, ctx),
    }
}

/// Dialogs and background work; called every frame while unlocked.
pub fn show(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    crate::ui::route::show(app, ctx, actions);
    crate::ui::certradar::show(app, ctx);
    crate::ui::wol::pump(app, ctx, actions);
}

/// A terminal connection to `host` failed: if the host looks unreachable from here and
/// has no jump host, look for a route in the background.
pub fn on_connect_failed(app: &mut App, host: Uuid, ctx: &egui::Context) {
    use crate::app::{ConnState, TabKind};
    let Some(h) = app.lib.host(host).cloned() else {
        return;
    };
    if h.protocol != crate::model::Protocol::Ssh || h.jump_host.is_some() || app.net.route.is_some()
    {
        return;
    }
    // A wake-up is in progress: the host is just not up yet.
    if app.net.wol.waiting(host).is_some() {
        return;
    }
    let unreachable = app.tabs.iter().any(|t| {
        t.host_id == Some(host)
            && matches!(&t.kind, TabKind::Term(term)
                if matches!(&term.state, ConnState::Closed(Some(e)) if crate::backend::route::looks_unreachable(e)))
    });
    if unreachable {
        crate::ui::route::start(app, h, crate::ui::route::Mode::Auto, ctx);
    }
}

/// Connection parameters of a saved SSH host with its stored credentials (and its own
/// jump chain); an error when nothing is stored (no password prompt in the background).
pub fn stored_params(app: &App, host: &Host) -> Result<SshParams> {
    let (user, ident) = app.credentials(host);
    let has_secret = ident
        .as_ref()
        .is_some_and(|i| !i.password.is_empty() || !i.private_key.is_empty());
    if !has_secret {
        return Err(anyhow!(
            "{}",
            t!(
                "{} hat keine gespeicherten Zugangsdaten (Identität mit Passwort oder Key)",
                host.display_name()
            )
        ));
    }
    let password = ident
        .as_ref()
        .map(|i| i.password.clone())
        .filter(|p| !p.is_empty());
    Ok(app.ssh_params(host, &user, &password, &ident))
}
