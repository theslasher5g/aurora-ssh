//! The customer focus ("only this customer") on pages other than the host list: which
//! Proxmox servers, key groups and hosts belong to it, and the hint shown when the focus
//! leaves a page empty. The host list, palette and inventory already follow the scope in
//! `nav_state`; this module is the shared rule for the rest.

use std::collections::{HashMap, HashSet};

use egui::RichText;
use uuid::Uuid;

use crate::app::{Action, App};
use crate::model::{Host, KeyGroup, Library, PveServer};
use crate::theme;
use crate::ui::nav_state::NavAction;

/// All group ids at or below `scope` (`None` = no focus, everything is visible).
pub fn group_set(lib: &Library, scope: Option<Uuid>) -> Option<HashSet<Uuid>> {
    let scope = scope?;
    let parent: HashMap<Uuid, Option<Uuid>> =
        lib.groups.iter().map(|g| (g.id, g.parent_id)).collect();
    let mut set = HashSet::new();
    for g in &lib.groups {
        let mut cur = Some(g.id);
        let mut steps = 0;
        while let Some(c) = cur {
            if c == scope {
                set.insert(g.id);
                break;
            }
            steps += 1;
            if steps > 64 {
                break;
            }
            cur = parent.get(&c).copied().flatten();
        }
    }
    Some(set)
}

/// The set for the app's current focus.
pub fn current(app: &App) -> Option<HashSet<Uuid>> {
    group_set(&app.lib, app.lib.nav.scope)
}

/// Is the host inside the focus? (hosts without a group never are)
pub fn host_ok(set: &Option<HashSet<Uuid>>, h: &Host) -> bool {
    match set {
        None => true,
        Some(s) => h.group_id.is_some_and(|g| s.contains(&g)),
    }
}

/// A Proxmox server belongs to a customer when the SSH host that reaches it lives in the
/// customer's groups, when a dynamic folder of the customer shows its guests, or when its
/// guests were saved into the customer's "Proxmox …" group.
pub fn pve_ok(lib: &Library, set: &Option<HashSet<Uuid>>, s: &PveServer) -> bool {
    let Some(set) = set else { return true };
    if s.ssh_host
        .and_then(|id| lib.host(id))
        .and_then(|h| h.group_id)
        .is_some_and(|g| set.contains(&g))
    {
        return true;
    }
    if lib.nav.dynamic.iter().any(|(g, f)| {
        set.contains(g)
            && matches!(&f.source, crate::nav2_dyn::DynSource::Proxmox { server } if *server == s.id)
    }) {
        return true;
    }
    let saved_group = format!("Proxmox {}", s.display_name());
    lib.groups
        .iter()
        .any(|g| g.name == saved_group && set.contains(&g.id))
}

/// Key groups of the focus: those assigned to a customer inside it; with
/// `with_unassigned` also the ones that belong to no customer.
pub fn key_group_ok(set: &Option<HashSet<Uuid>>, g: &KeyGroup, with_unassigned: bool) -> bool {
    match (set, g.customer) {
        (None, _) => true,
        (Some(s), Some(c)) => s.contains(&c),
        (Some(_), None) => with_unassigned,
    }
}

/// Picker for a customer (host group): none, or a group by its path.
pub fn customer_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<Uuid>, lib: &Library) {
    let path = |g: &crate::model::Group| {
        let (p, _) = crate::backend::inventory_rules::group_path(
            lib,
            &Host {
                group_id: Some(g.id),
                ..Default::default()
            },
        );
        p
    };
    let selected = value
        .and_then(|v| lib.group(v))
        .map(path)
        .unwrap_or_else(|| t!("— keiner —").to_string());
    let mut items: Vec<(Uuid, String)> = lib.groups.iter().map(|g| (g.id, path(g))).collect();
    items.sort_by_key(|a| a.1.to_lowercase());
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width((ui.available_width() - 14.0).max(160.0))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, t!("— keiner —"));
            for (gid, name) in items {
                ui.selectable_value(value, Some(gid), name);
            }
        });
}

/// "Nothing here in this focus" with a button that ends it.
pub fn empty_hint(ui: &mut egui::Ui, app: &App, what: &str, actions: &mut Vec<Action>) {
    let name = crate::ui::nav_state::scope_name(&app.lib).unwrap_or("?");
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(t!("Im Fokus „{}“ gibt es hier {}.", name, what))
                .color(theme::text_dim()),
        );
        ui.add_space(8.0);
        if theme::btn(ui, theme::Btn::Normal, None, t!("Fokus beenden")).clicked() {
            actions.push(Action::Nav(NavAction::Scope(None)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Group;

    fn lib() -> (Library, Uuid, Uuid, Uuid) {
        let mut lib = Library::default();
        let a = Group {
            name: "A".into(),
            ..Default::default()
        };
        let a1 = Group {
            name: "A1".into(),
            parent_id: Some(a.id),
            ..Default::default()
        };
        let b = Group {
            name: "B".into(),
            ..Default::default()
        };
        let ids = (a.id, a1.id, b.id);
        lib.groups = vec![a, a1, b];
        (lib, ids.0, ids.1, ids.2)
    }

    #[test]
    fn group_set_holds_the_subtree() {
        let (lib, a, a1, b) = lib();
        assert!(group_set(&lib, None).is_none());
        let s = group_set(&lib, Some(a)).unwrap();
        assert!(s.contains(&a) && s.contains(&a1) && !s.contains(&b));
    }

    #[test]
    fn servers_follow_their_ssh_host_dynamic_folder_or_saved_group() {
        let (mut lib, a, a1, b) = lib();
        let h_a = Host {
            group_id: Some(a1),
            ..Default::default()
        };
        let h_b = Host {
            group_id: Some(b),
            ..Default::default()
        };
        let s_a = PveServer {
            name: "pa".into(),
            ssh_host: Some(h_a.id),
            ..Default::default()
        };
        let s_b = PveServer {
            name: "pb".into(),
            ssh_host: Some(h_b.id),
            ..Default::default()
        };
        let s_dyn = PveServer {
            name: "pd".into(),
            ..Default::default()
        };
        let s_none = PveServer {
            name: "pn".into(),
            ..Default::default()
        };
        lib.hosts = vec![h_a, h_b];
        lib.nav.dynamic.insert(
            a,
            crate::nav2_dyn::DynFolder {
                source: crate::nav2_dyn::DynSource::Proxmox { server: s_dyn.id },
                ..Default::default()
            },
        );
        let set = group_set(&lib, Some(a));
        assert!(pve_ok(&lib, &set, &s_a));
        assert!(!pve_ok(&lib, &set, &s_b));
        assert!(pve_ok(&lib, &set, &s_dyn));
        assert!(!pve_ok(&lib, &set, &s_none));
        assert!(
            pve_ok(&lib, &None, &s_none),
            "without a focus everything shows"
        );
    }

    #[test]
    fn key_groups_follow_their_customer() {
        let (lib, a, a1, b) = lib();
        let set = group_set(&lib, Some(a));
        let kg = |c| KeyGroup {
            customer: c,
            ..Default::default()
        };
        assert!(key_group_ok(&set, &kg(Some(a1)), false));
        assert!(!key_group_ok(&set, &kg(Some(b)), true));
        assert!(!key_group_ok(&set, &kg(None), false));
        assert!(key_group_ok(&set, &kg(None), true));
        assert!(key_group_ok(&None, &kg(Some(b)), false));
    }
}
