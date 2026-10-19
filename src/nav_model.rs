//! Data the "large inventory" navigation remembers: group badges and pins, the scope
//! ("only this customer"), list or table view per scope, table columns and saved views.
//! Everything has serde defaults, so an old `library.json` loads unchanged.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Most saved views (keeps the sidebar short).
pub const MAX_VIEWS: usize = 30;
/// Most recently opened groups that are remembered.
pub const MAX_RECENT_GROUPS: usize = 8;

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(default)]
pub struct GroupMeta {
    /// Badge text (1–2 characters, e.g. the customer's initials); empty = first letter of
    /// the name.
    pub letter: String,
    /// Pinned to the top of the list and the sidebar.
    pub favorite: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    List,
    Table,
}

/// Columns of the host table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Col {
    Name,
    Address,
    Kind,
    Tags,
    Status,
    Seen,
    Env,
    Group,
}

impl Col {
    pub const ALL: [Col; 8] = [
        Col::Name,
        Col::Address,
        Col::Kind,
        Col::Tags,
        Col::Status,
        Col::Seen,
        Col::Env,
        Col::Group,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Address => "address",
            Col::Kind => "type",
            Col::Tags => "tags",
            Col::Status => "status",
            Col::Seen => "seen",
            Col::Env => "env",
            Col::Group => "group",
        }
    }

    pub fn from_key(k: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.key() == k)
    }

    pub fn label(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Address => "Adresse",
            Col::Kind => "Typ",
            Col::Tags => "Tags",
            Col::Status => "Status",
            Col::Seen => "Zuletzt gesehen",
            Col::Env => "Umgebung",
            Col::Group => "Gruppe",
        }
    }

    /// Default width in points.
    pub fn default_width(self) -> f32 {
        match self {
            Col::Name => 220.0,
            Col::Address => 170.0,
            Col::Kind => 80.0,
            Col::Tags => 160.0,
            Col::Status => 110.0,
            Col::Seen => 130.0,
            Col::Env => 90.0,
            Col::Group => 200.0,
        }
    }

    /// Smallest width a user may drag the column to.
    pub fn min_width(self) -> f32 {
        match self {
            Col::Name => 90.0,
            Col::Status | Col::Env | Col::Kind => 56.0,
            _ => 70.0,
        }
    }

    /// Lower number = hidden last when the window gets narrow.
    pub fn priority(self) -> u8 {
        match self {
            Col::Name => 0,
            Col::Status => 1,
            Col::Address => 2,
            Col::Env => 3,
            Col::Seen => 4,
            Col::Kind => 5,
            Col::Tags => 6,
            Col::Group => 7,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct TableCfg {
    /// Column keys the user hid.
    pub hidden: Vec<String>,
    /// Column key → width the user dragged it to.
    pub widths: HashMap<String, f32>,
    /// Column key of the sort order.
    pub sort: String,
    pub desc: bool,
}

impl Default for TableCfg {
    fn default() -> Self {
        Self {
            hidden: Vec::new(),
            widths: HashMap::new(),
            sort: "name".into(),
            desc: false,
        }
    }
}

impl TableCfg {
    pub fn is_hidden(&self, c: Col) -> bool {
        c != Col::Name && self.hidden.iter().any(|k| k == c.key())
    }

    pub fn toggle(&mut self, c: Col) {
        if c == Col::Name {
            return;
        }
        if let Some(i) = self.hidden.iter().position(|k| k == c.key()) {
            self.hidden.remove(i);
        } else {
            self.hidden.push(c.key().to_string());
        }
    }

    pub fn width(&self, c: Col) -> f32 {
        self.widths
            .get(c.key())
            .copied()
            .unwrap_or(c.default_width())
            .clamp(c.min_width(), 600.0)
    }

    pub fn set_width(&mut self, c: Col, w: f32) {
        self.widths
            .insert(c.key().to_string(), w.clamp(c.min_width(), 600.0));
    }

    pub fn sort_col(&self) -> Col {
        Col::from_key(&self.sort).unwrap_or(Col::Name)
    }

    /// Click on a header: same column flips the direction, another one starts ascending.
    pub fn click_sort(&mut self, c: Col) {
        if self.sort_col() == c {
            self.desc = !self.desc;
        } else {
            self.sort = c.key().to_string();
            self.desc = false;
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct NavData {
    pub groups: HashMap<Uuid, GroupMeta>,
    /// Groups opened or focused most recently, newest first.
    pub recent_groups: Vec<Uuid>,
    /// "Only this customer": the whole app shows only this group and its subgroups.
    pub scope: Option<Uuid>,
    /// List or table, per scope (`scope_key`).
    pub modes: HashMap<String, ViewMode>,
    pub table: TableCfg,
}

/// Key of a scope in `NavData::modes` (empty = everything).
pub fn scope_key(scope: Option<Uuid>) -> String {
    scope.map(|s| s.to_string()).unwrap_or_default()
}

impl NavData {
    pub fn meta(&self, g: Uuid) -> Option<&GroupMeta> {
        self.groups.get(&g)
    }

    pub fn is_favorite(&self, g: Uuid) -> bool {
        self.groups.get(&g).is_some_and(|m| m.favorite)
    }

    pub fn set_favorite(&mut self, g: Uuid, on: bool) {
        let m = self.groups.entry(g).or_default();
        m.favorite = on;
        if *m == GroupMeta::default() {
            self.groups.remove(&g);
        }
    }

    pub fn set_letter(&mut self, g: Uuid, letter: &str) {
        let letter: String = letter.trim().chars().take(2).collect();
        let m = self.groups.entry(g).or_default();
        m.letter = letter;
        if *m == GroupMeta::default() {
            self.groups.remove(&g);
        }
    }

    pub fn mode(&self, scope: Option<Uuid>) -> ViewMode {
        self.modes
            .get(&scope_key(scope))
            .copied()
            .unwrap_or_default()
    }

    pub fn set_mode(&mut self, scope: Option<Uuid>, mode: ViewMode) {
        if mode == ViewMode::List {
            self.modes.remove(&scope_key(scope));
        } else {
            self.modes.insert(scope_key(scope), mode);
        }
    }

    /// Remembers `g` as the most recently used group.
    pub fn touch_group(&mut self, g: Uuid) {
        self.recent_groups.retain(|x| *x != g);
        self.recent_groups.insert(0, g);
        self.recent_groups.truncate(MAX_RECENT_GROUPS);
    }

    /// Forgets everything about groups that no longer exist.
    pub fn prune(&mut self, exists: impl Fn(Uuid) -> bool) {
        self.groups.retain(|g, _| exists(*g));
        self.recent_groups.retain(|g| exists(*g));
        self.modes
            .retain(|k, _| k.is_empty() || Uuid::parse_str(k).is_ok_and(&exists));
        if self.scope.is_some_and(|s| !exists(s)) {
            self.scope = None;
        }
    }
}

/// A saved combination of search, protocol filter, sort and scope, shown in the sidebar.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct SavedView {
    pub id: Uuid,
    pub name: String,
    /// Search text including `type:`, `is:`, `env:` … conditions.
    pub query: String,
    /// Protocol filter of the list (0 = all).
    pub filter: usize,
    /// Sort order (see `Settings::sort`).
    pub sort: u8,
    pub scope: Option<Uuid>,
    pub table: bool,
}

impl Default for SavedView {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            query: String::new(),
            filter: 0,
            sort: 0,
            scope: None,
            table: false,
        }
    }
}

/// Adds or replaces (same name, case-insensitive) a saved view; returns false if the
/// name is empty or the list is full.
pub fn save_view(views: &mut Vec<SavedView>, mut view: SavedView) -> bool {
    view.name = view.name.trim().to_string();
    if view.name.is_empty() {
        return false;
    }
    if let Some(old) = views
        .iter_mut()
        .find(|v| v.name.to_lowercase() == view.name.to_lowercase())
    {
        view.id = old.id;
        *old = view;
        return true;
    }
    if views.len() >= MAX_VIEWS {
        return false;
    }
    views.push(view);
    true
}

pub fn delete_view(views: &mut Vec<SavedView>, id: Uuid) {
    views.retain(|v| v.id != id);
}

/// Moves a view one place up (`-1`) or down (`1`).
pub fn move_view(views: &mut [SavedView], id: Uuid, delta: i32) {
    let Some(i) = views.iter().position(|v| v.id == id) else {
        return;
    };
    let j = i as i32 + delta;
    if j >= 0 && (j as usize) < views.len() {
        views.swap(i, j as usize);
    }
}

/// A default name for a view built from the current search and scope.
pub fn suggest_name(query: &str, scope_name: Option<&str>) -> String {
    let q = query.trim();
    match (scope_name, q.is_empty()) {
        (Some(s), false) => format!("{q} · {s}"),
        (Some(s), true) => s.to_string(),
        (None, false) => q.to_string(),
        (None, true) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(name: &str, query: &str) -> SavedView {
        SavedView {
            name: name.into(),
            query: query.into(),
            ..Default::default()
        }
    }

    #[test]
    fn saved_views_replace_by_name() {
        let mut v = Vec::new();
        assert!(save_view(&mut v, view("Switches", "type:switch")));
        let id = v[0].id;
        assert!(save_view(&mut v, view(" switches ", "type:sw is:down")));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, id, "same name keeps the id");
        assert_eq!(v[0].query, "type:sw is:down");
        assert!(!save_view(&mut v, view("  ", "x")));
    }

    #[test]
    fn saved_views_are_capped_and_movable() {
        let mut v = Vec::new();
        for i in 0..MAX_VIEWS {
            assert!(save_view(&mut v, view(&format!("v{i}"), "")));
        }
        assert!(!save_view(&mut v, view("one too many", "")));
        let (a, b) = (v[0].id, v[1].id);
        move_view(&mut v, a, 1);
        assert_eq!(v[1].id, a);
        assert_eq!(v[0].id, b);
        move_view(&mut v, a, 100);
        assert_eq!(v[1].id, a);
        delete_view(&mut v, a);
        assert_eq!(v.len(), MAX_VIEWS - 1);
    }

    #[test]
    fn group_meta_is_dropped_when_default() {
        let mut n = NavData::default();
        let g = Uuid::new_v4();
        n.set_favorite(g, true);
        n.set_letter(g, " ACME ");
        assert_eq!(n.meta(g).unwrap().letter, "AC");
        n.set_favorite(g, false);
        assert!(n.groups.contains_key(&g), "letter is still set");
        n.set_letter(g, "");
        assert!(n.groups.is_empty());
    }

    #[test]
    fn view_mode_per_scope() {
        let mut n = NavData::default();
        let g = Uuid::new_v4();
        assert_eq!(n.mode(None), ViewMode::List);
        n.set_mode(Some(g), ViewMode::Table);
        assert_eq!(n.mode(Some(g)), ViewMode::Table);
        assert_eq!(n.mode(None), ViewMode::List);
        n.set_mode(Some(g), ViewMode::List);
        assert!(n.modes.is_empty());
    }

    #[test]
    fn recent_groups_move_to_front_and_cap() {
        let mut n = NavData::default();
        let ids: Vec<Uuid> = (0..12).map(|_| Uuid::new_v4()).collect();
        for g in &ids {
            n.touch_group(*g);
        }
        assert_eq!(n.recent_groups.len(), MAX_RECENT_GROUPS);
        n.touch_group(ids[8]);
        assert_eq!(n.recent_groups[0], ids[8]);
        assert_eq!(n.recent_groups.iter().filter(|g| **g == ids[8]).count(), 1);
    }

    #[test]
    fn prune_forgets_deleted_groups() {
        let mut n = NavData::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        n.set_favorite(a, true);
        n.set_favorite(b, true);
        n.touch_group(b);
        n.scope = Some(b);
        n.set_mode(Some(b), ViewMode::Table);
        n.prune(|g| g == a);
        assert!(n.groups.contains_key(&a) && !n.groups.contains_key(&b));
        assert!(n.recent_groups.is_empty() && n.modes.is_empty());
        assert_eq!(n.scope, None);
    }

    #[test]
    fn table_columns() {
        let mut t = TableCfg::default();
        t.toggle(Col::Name);
        assert!(!t.is_hidden(Col::Name), "the name column stays");
        t.toggle(Col::Tags);
        assert!(t.is_hidden(Col::Tags));
        t.toggle(Col::Tags);
        assert!(!t.is_hidden(Col::Tags));
        t.set_width(Col::Address, 5.0);
        assert_eq!(t.width(Col::Address), Col::Address.min_width());
        t.click_sort(Col::Seen);
        assert!(t.sort_col() == Col::Seen && !t.desc);
        t.click_sort(Col::Seen);
        assert!(t.desc);
        t.click_sort(Col::Env);
        assert!(!t.desc);
        for c in Col::ALL {
            assert_eq!(Col::from_key(c.key()), Some(c));
        }
    }

    #[test]
    fn old_data_loads_with_defaults() {
        let n: NavData = serde_json::from_str("{}").unwrap();
        assert!(n.scope.is_none() && n.groups.is_empty());
        let v: SavedView = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(v.name, "x");
        let back: NavData = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(back.table, n.table);
    }

    #[test]
    fn suggested_names() {
        assert_eq!(
            suggest_name("type:switch", Some("Acme")),
            "type:switch · Acme"
        );
        assert_eq!(suggest_name("", Some("Acme")), "Acme");
        assert_eq!(suggest_name(" is:down ", None), "is:down");
        assert_eq!(suggest_name("", None), "");
    }
}
