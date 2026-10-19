//! "Aus Keychain wählen": credential picker inside the password prompt – keychain
//! identities that hold a password plus items of the enabled password managers.
//! Nothing secret is ever listed, only names and user names.

use std::sync::Arc;

use egui::RichText;
use uuid::Uuid;

use crate::backend::pwmanager_cli::Sessions;
use crate::backend::pwmanager_types::{PwmItem, PwmProvider, PwmSettings};
use crate::model::Keychain;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::pwmanager_ui::{Searcher, item_row, item_sub, search_status};

/// Above this many identities the list gets a search box (it also gets one as soon as a
/// password manager is enabled).
pub const SEARCH_THRESHOLD: usize = 6;

/// What the user picked.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Identity(Uuid),
    Manager(PwmProvider, PwmItem),
}

/// A keychain identity as shown in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: Uuid,
    pub title: String,
    pub sub: String,
}

/// Identities that can supply a password: a stored password, or a password-manager source.
pub fn identity_entries(kc: &Keychain) -> Vec<Entry> {
    let mut v: Vec<Entry> = kc
        .identities
        .iter()
        .filter(|i| {
            !i.password.is_empty()
                || i.secret_source.as_ref().is_some_and(|s| {
                    s.kind == crate::backend::pwmanager_types::SourceKind::Password
                })
        })
        .map(|i| Entry {
            id: i.id,
            title: if i.label.trim().is_empty() {
                i.username.clone()
            } else {
                i.label.clone()
            },
            sub: i.username.clone(),
        })
        .collect();
    v.sort_by_key(|e| e.title.to_lowercase());
    v
}

/// Indices of the entries matching `query` (case-insensitive, name or user).
pub fn filter_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            q.is_empty() || e.title.to_lowercase().contains(&q) || e.sub.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Moves a selection by `delta` inside `len` rows, wrapping around.
pub fn step(sel: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (sel as i64 + delta as i64).rem_euclid(len as i64) as usize
}

/// Does the list need a search box?
pub fn needs_search(identities: usize, any_manager: bool) -> bool {
    identities > SEARCH_THRESHOLD || any_manager
}

/// Maps a flat selection index over `ids` identity rows followed by `managers` manager
/// rows to which of the two it is.
pub fn locate(sel: usize, ids: usize, managers: usize) -> Option<(bool, usize)> {
    if sel < ids {
        Some((false, sel))
    } else if sel - ids < managers {
        Some((true, sel - ids))
    } else {
        None
    }
}

#[derive(Default)]
pub struct PromptPick {
    pub open: bool,
    pub query: String,
    pub sel: usize,
    /// Store the choice on the host.
    pub remember: bool,
    pub provider: Option<PwmProvider>,
    pub search: Searcher,
    /// The highlighted row, so Enter / "Verbinden" in the password field can use it.
    pub current: Option<Choice>,
    focus: bool,
}

pub struct PickCtx<'a> {
    pub entries: &'a [Entry],
    pub providers: &'a [PwmProvider],
    pub settings: &'a PwmSettings,
    pub sessions: &'a Arc<Sessions>,
    pub epoch: u64,
    pub can_remember: bool,
}

#[derive(Default)]
pub struct PickOut {
    pub choice: Option<Choice>,
    pub unlock: Option<PwmProvider>,
}

pub fn show(ui: &mut egui::Ui, st: &mut PromptPick, cx: &PickCtx) -> PickOut {
    let mut out = PickOut::default();
    let has_any = !cx.entries.is_empty() || !cx.providers.is_empty();
    if !has_any {
        st.current = None;
        return out;
    }
    if !st.open {
        st.current = None;
        let down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
        ui.horizontal(|ui| {
            if theme::btn(
                ui,
                theme::Btn::Small,
                Some(Icon::Key),
                t!("Aus Keychain wählen"),
            )
            .clicked()
                || down
            {
                st.open = true;
                st.focus = true;
            }
        });
        return out;
    }
    // Provider for the manager part.
    if st.provider.is_none_or(|p| !cx.providers.contains(&p)) {
        st.provider = cx.providers.first().copied();
    }
    let query = st.query.trim().to_string();
    let ids = filter_entries(cx.entries, &query);
    let managers_on = st.provider.filter(|_| !query.is_empty());
    if let Some(p) = managers_on {
        st.search
            .update(cx.settings, cx.sessions, p, &query, cx.epoch, ui.ctx());
    }
    let mgr: &[PwmItem] = if managers_on.is_some() {
        &st.search.results
    } else {
        &[]
    };
    let total = ids.len() + mgr.len();
    st.sel = st.sel.min(total.saturating_sub(1));
    if total > 0 {
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                st.sel = step(st.sel, total, 1);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                st.sel = step(st.sel, total, -1);
            }
        });
    }
    let moved =
        ui.input(|i| i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::ArrowUp));

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(t!("Aus Keychain wählen"))
                .color(theme::text_dim())
                .size(12.5),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_btn(ui, Icon::Close, t!("Schließen")).clicked() {
                st.open = false;
            }
        });
    });
    let show_search = needs_search(cx.entries.len(), !cx.providers.is_empty());
    let mut enter = false;
    if show_search {
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.query)
                .hint_text(if cx.providers.is_empty() {
                    t!("Suchen…")
                } else {
                    t!("Suchen (auch im Passwort-Manager)…")
                })
                .margin(egui::Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        );
        if st.focus {
            r.request_focus();
            st.focus = false;
        }
        if r.changed() {
            st.sel = 0;
        }
        enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    }
    if cx.providers.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for p in cx.providers {
                let on = st.provider == Some(*p);
                if theme::tag(ui, p.label(), on.then(theme::accent)).clicked() {
                    st.provider = Some(*p);
                    st.sel = 0;
                }
            }
        });
    }
    let max_h = (ui.ctx().content_rect().height() - 360.0).clamp(70.0, 190.0);
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("pwm-prompt-list")
        .max_height(max_h)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (row, &i) in ids.iter().enumerate() {
                let e = &cx.entries[i];
                let r = item_row(ui, &e.title, &e.sub, st.sel == row);
                if moved && st.sel == row {
                    r.scroll_to_me(None);
                }
                if r.clicked() {
                    st.sel = row;
                }
                if r.double_clicked() {
                    picked = Some(row);
                }
            }
            for (k, it) in mgr.iter().enumerate() {
                let row = ids.len() + k;
                let r = item_row(ui, &it.name, &item_sub(it), st.sel == row);
                if moved && st.sel == row {
                    r.scroll_to_me(None);
                }
                if r.clicked() {
                    st.sel = row;
                }
                if r.double_clicked() {
                    picked = Some(row);
                }
            }
            if total == 0 && !st.search.busy && st.search.error.is_none() {
                ui.label(
                    RichText::new(t!("Keine Treffer"))
                        .color(theme::text_dim())
                        .size(12.5),
                );
            }
        });
    if let (Some(p), true) = (managers_on, true) {
        out.unlock = search_status(ui, &st.search, p);
    }
    if cx.can_remember {
        ui.checkbox(&mut st.remember, t!("Für diesen Host merken"));
    }
    // The highlighted row as a choice.
    let mgr_items: Vec<PwmItem> = mgr.to_vec();
    st.current = match locate(st.sel, ids.len(), mgr_items.len()) {
        Some((false, k)) => Some(Choice::Identity(cx.entries[ids[k]].id)),
        Some((true, k)) => st
            .provider
            .map(|p| Choice::Manager(p, mgr_items[k].clone())),
        None => None,
    };
    if (enter || picked.is_some()) && total > 0 {
        if let Some(row) = picked {
            st.sel = row;
            st.current = match locate(row, ids.len(), mgr_items.len()) {
                Some((false, k)) => Some(Choice::Identity(cx.entries[ids[k]].id)),
                Some((true, k)) => st
                    .provider
                    .map(|p| Choice::Manager(p, mgr_items[k].clone())),
                None => None,
            };
        }
        out.choice = st.current.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Identity;

    fn entry(t: &str, u: &str) -> Entry {
        Entry {
            id: Uuid::new_v4(),
            title: t.into(),
            sub: u.into(),
        }
    }

    #[test]
    fn filters_by_name_and_user() {
        let e = vec![
            entry("Deploy", "root"),
            entry("Web", "www-data"),
            entry("DB", "postgres"),
        ];
        assert_eq!(filter_entries(&e, ""), vec![0, 1, 2]);
        assert_eq!(filter_entries(&e, "  DEP "), vec![0]);
        assert_eq!(filter_entries(&e, "POST"), vec![2]);
        assert!(filter_entries(&e, "zzz").is_empty());
    }

    #[test]
    fn selection_wraps_and_handles_empty() {
        assert_eq!(step(0, 3, 1), 1);
        assert_eq!(step(2, 3, 1), 0);
        assert_eq!(step(0, 3, -1), 2);
        assert_eq!(step(0, 0, 1), 0);
        assert_eq!(step(5, 1, 1), 0);
    }

    #[test]
    fn search_box_rule_and_locate() {
        assert!(!needs_search(6, false));
        assert!(needs_search(7, false));
        assert!(needs_search(0, true));
        assert_eq!(locate(0, 2, 3), Some((false, 0)));
        assert_eq!(locate(2, 2, 3), Some((true, 0)));
        assert_eq!(locate(4, 2, 3), Some((true, 2)));
        assert_eq!(locate(5, 2, 3), None);
        assert_eq!(locate(0, 0, 0), None);
    }

    #[test]
    fn only_identities_with_a_password_are_listed() {
        let mut kc = Keychain::default();
        let mut a = Identity::new();
        a.label = "zeta".into();
        a.username = "root".into();
        a.password = "secret-pw".into();
        let mut b = Identity::new();
        b.label = "key-only".into();
        b.private_key = "KEY".into();
        let mut c = Identity::new();
        c.label = "alpha".into();
        c.secret_source = Some(Default::default());
        kc.identities = vec![a, b, c];
        let e = identity_entries(&kc);
        assert_eq!(
            e.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        // The listing carries no secret.
        assert!(!format!("{e:?}").contains("secret-pw"));
    }
}
