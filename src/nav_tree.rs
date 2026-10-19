//! The host list of a big library as pure data: an index of groups and hosts, what is
//! shown after search/scope, the flattened visible rows with their heights and the
//! culling math for virtual scrolling. No egui in here, so it is unit-testable.
//!
//! Cost model: [`Index::build`] and [`compute`] are O(hosts · group depth) and run only
//! when the library, the filter, the scope or the host states change; [`flatten`] is
//! O(visible rows); every frame only needs [`visible_range`] (two binary searches).

use std::cmp::{Ordering, Reverse};
use std::collections::HashMap;

use uuid::Uuid;

use crate::backend::inventory_rules::InventorySettings;
use crate::model::{Host, HostFacts, Library};
use crate::nav_query::{Facets, Status};

/// Host order inside a group (`Settings::sort`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Name,
    LastUsed,
    OnlineFirst,
}

impl Sort {
    pub fn from_u8(v: u8) -> Sort {
        match v {
            1 => Sort::LastUsed,
            2 => Sort::OnlineFirst,
            _ => Sort::Name,
        }
    }
}

/// Numbers inside names compare as numbers (`kunde 2` < `kunde 10`), case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.next_if(|c| c.is_ascii_digit()) {
                    na.push(c);
                }
                let mut nb = String::new();
                while let Some(c) = b.next_if(|c| c.is_ascii_digit()) {
                    nb.push(c);
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// 0 = fine, 1 = warning, 2 = critical, from the facts and the inventory thresholds
/// (disk usage, pending updates, security updates). Cheap on purpose: it runs for
/// every host.
pub fn facts_level(f: &HostFacts, s: &InventorySettings) -> u8 {
    let mut level = 0;
    if let Some(d) = f.disk_used_pct {
        if d >= s.disk_crit {
            level = 2;
        } else if d >= s.disk_warn {
            level = 1;
        }
    }
    if f.security_updates.is_some_and(|n| n > 0) {
        level = 2;
    }
    if level == 0
        && f.updates
            .is_some_and(|n| s.updates_warn > 0 && n >= s.updates_warn)
    {
        level = 1;
    }
    level
}

/// One host in the index.
pub struct Entry {
    /// Position in `Library::hosts`.
    pub host: usize,
    /// Group node of the host (`None` = loose).
    pub node: Option<usize>,
    pub status: Status,
    pub live: bool,
    /// See [`facts_level`].
    pub warn: u8,
    pub name_lc: String,
    /// Lowercase text the plain search words match against.
    pub hay: String,
    /// Unix seconds of the last connection (0 = never).
    pub last: i64,
}

pub struct Node {
    pub id: Uuid,
    pub parent: Option<usize>,
    pub depth: u8,
    pub children: Vec<usize>,
    pub name: String,
    /// Group names, outermost first, ending with this group's own name.
    pub path: Vec<String>,
    pub favorite: bool,
}

pub struct Index {
    pub entries: Vec<Entry>,
    pub nodes: Vec<Node>,
    /// Top-level groups, pinned first, then by name.
    pub roots: Vec<usize>,
    pub by_id: HashMap<Uuid, usize>,
    /// Entries by name (the base order of every list).
    pub by_name: Vec<u32>,
}

fn host_address(h: &Host) -> &str {
    match h.protocol {
        crate::model::Protocol::Serial => &h.serial.port_name,
        _ if h.protocol.is_container() => &h.container.name,
        _ => &h.hostname,
    }
}

impl Index {
    /// `status(host)` returns the reachability and whether a session is open.
    pub fn build(lib: &Library, status: &mut dyn FnMut(&Host) -> (Status, bool)) -> Index {
        // --- group nodes
        let mut nodes: Vec<Node> = lib
            .groups
            .iter()
            .map(|g| Node {
                id: g.id,
                parent: None,
                depth: 0,
                children: Vec::new(),
                name: g.name.clone(),
                path: Vec::new(),
                favorite: lib.nav.is_favorite(g.id),
            })
            .collect();
        let by_id: HashMap<Uuid, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        for (i, g) in lib.groups.iter().enumerate() {
            nodes[i].parent = g.parent_id.and_then(|p| by_id.get(&p).copied());
        }
        // Break parent cycles (a corrupt library must not hang the list).
        for i in 0..nodes.len() {
            let mut cur = nodes[i].parent;
            let mut steps = 0;
            while let Some(c) = cur {
                if c == i || steps > 64 {
                    nodes[i].parent = None;
                    break;
                }
                steps += 1;
                cur = nodes[c].parent;
            }
        }
        for i in 0..nodes.len() {
            let mut chain = vec![nodes[i].name.clone()];
            let mut cur = nodes[i].parent;
            while let Some(c) = cur {
                chain.push(nodes[c].name.clone());
                cur = nodes[c].parent;
            }
            chain.reverse();
            nodes[i].depth = (chain.len() - 1).min(255) as u8;
            nodes[i].path = chain;
        }
        let mut order: Vec<usize> = (0..nodes.len()).collect();
        order.sort_by(|&a, &b| {
            nodes[b]
                .favorite
                .cmp(&nodes[a].favorite)
                .then_with(|| natural_cmp(&nodes[a].name, &nodes[b].name))
        });
        let mut roots = Vec::new();
        for i in order {
            match nodes[i].parent {
                Some(p) => nodes[p].children.push(i),
                None => roots.push(i),
            }
        }

        // --- hosts
        let entries: Vec<Entry> = lib
            .hosts
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let node = h.group_id.and_then(|g| by_id.get(&g).copied());
                let (st, live) = status(h);
                let name = h.display_name();
                let mut hay = String::with_capacity(64);
                hay.push_str(&name.to_lowercase());
                hay.push(' ');
                hay.push_str(&host_address(h).to_lowercase());
                hay.push(' ');
                hay.push_str(&h.username.to_lowercase());
                for t in &h.tags {
                    hay.push(' ');
                    hay.push_str(&t.to_lowercase());
                }
                if let Some(n) = node {
                    hay.push(' ');
                    hay.push_str(&nodes[n].path.join(" ").to_lowercase());
                }
                Entry {
                    host: i,
                    node,
                    status: st,
                    live,
                    warn: lib
                        .facts
                        .get(&h.id)
                        .map(|f| facts_level(f, &lib.settings.inventory))
                        .unwrap_or(0),
                    name_lc: name.to_lowercase(),
                    hay,
                    last: h.last_connected.map(|t| t.timestamp()).unwrap_or(0),
                }
            })
            .collect();
        let mut by_name: Vec<u32> = (0..entries.len() as u32).collect();
        by_name.sort_by(|&a, &b| {
            natural_cmp(&entries[a as usize].name_lc, &entries[b as usize].name_lc)
        });
        Index {
            entries,
            nodes,
            roots,
            by_id,
            by_name,
        }
    }

    /// Updates reachability and warnings in place (no allocation); the structure must
    /// still match `lib` (same hosts in the same order).
    pub fn refresh_status(
        &mut self,
        lib: &Library,
        status: &mut dyn FnMut(&Host) -> (Status, bool),
    ) {
        for e in &mut self.entries {
            let Some(h) = lib.hosts.get(e.host) else {
                continue;
            };
            let (st, live) = status(h);
            e.status = st;
            e.live = live;
            e.warn = lib
                .facts
                .get(&h.id)
                .map(|f| facts_level(f, &lib.settings.inventory))
                .unwrap_or(0);
        }
    }

    /// Is `node` the `scope` node or below it?
    pub fn under(&self, node: usize, scope: usize) -> bool {
        let mut cur = Some(node);
        let mut steps = 0;
        while let Some(c) = cur {
            if c == scope {
                return true;
            }
            steps += 1;
            if steps > 64 {
                return false;
            }
            cur = self.nodes[c].parent;
        }
        false
    }

    /// `node` and its ancestors, outermost first.
    pub fn chain(&self, node: usize) -> Vec<usize> {
        let mut v = vec![node];
        let mut cur = self.nodes[node].parent;
        while let Some(c) = cur {
            if v.len() > 64 {
                break;
            }
            v.push(c);
            cur = self.nodes[c].parent;
        }
        v.reverse();
        v
    }

    pub fn facets<'a>(&'a self, lib: &'a Library, e: usize) -> Facets<'a> {
        let en = &self.entries[e];
        let h = &lib.hosts[en.host];
        const NO_PATH: &[String] = &[];
        Facets {
            name: h.display_name(),
            address: host_address(h),
            tags: &h.tags,
            protocol: h.protocol,
            env: lib.env_of(h),
            path: en
                .node
                .map(|n| self.nodes[n].path.as_slice())
                .unwrap_or(NO_PATH),
            status: en.status,
            live: en.live,
            favorite: h.favorite,
            warn: en.warn,
            os: lib
                .facts
                .get(&h.id)
                .map(|f| f.os_name.as_str())
                .unwrap_or(""),
        }
    }
}

/// Reachability roll-up of a group (all hosts below it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Roll {
    pub up: u32,
    /// Unreachable or last connection failed.
    pub down: u32,
    pub unknown: u32,
    pub maint: u32,
    /// Hosts with a warning (disk, updates).
    pub warn: u32,
    /// Hosts with a critical warning (full disk, security updates).
    pub crit: u32,
}

impl Roll {
    fn add(&mut self, e: &Entry) {
        match e.status {
            _ if e.live => self.up += 1,
            Status::Up => self.up += 1,
            Status::Down | Status::Failed => self.down += 1,
            Status::Maint => self.maint += 1,
            Status::Unknown => self.unknown += 1,
        }
        match e.warn {
            2 => self.crit += 1,
            1 => self.warn += 1,
            _ => {}
        }
    }

    /// Red badge: unreachable hosts plus critical warnings.
    pub fn red(&self) -> u32 {
        self.down + self.crit
    }

    /// Hosts that were probed at all.
    pub fn probed(&self) -> u32 {
        self.up + self.down
    }
}

/// What is shown: the hosts that pass search, protocol filter and scope, sorted and
/// bucketed per group, with counts and roll-ups per group.
pub struct Shown {
    pub scope: Option<usize>,
    /// Per node: its own hosts (entry indices), sorted.
    pub hosts: Vec<Vec<u32>>,
    /// Per node: hosts below it (incl. subgroups).
    pub total: Vec<u32>,
    pub roll: Vec<Roll>,
    /// Hosts without a (known) group.
    pub loose: Vec<u32>,
    /// Every shown host in sort order.
    pub order: Vec<u32>,
}

pub fn compute(
    idx: &Index,
    keep: &dyn Fn(usize) -> bool,
    scope: Option<usize>,
    sort: Sort,
) -> Shown {
    let n = idx.nodes.len();
    let mut order: Vec<u32> = idx
        .by_name
        .iter()
        .copied()
        .filter(|&e| {
            let en = &idx.entries[e as usize];
            let in_scope = match scope {
                None => true,
                Some(s) => en.node.is_some_and(|nd| idx.under(nd, s)),
            };
            in_scope && keep(e as usize)
        })
        .collect();
    match sort {
        Sort::Name => {}
        Sort::LastUsed => order.sort_by_key(|&e| Reverse(idx.entries[e as usize].last)),
        Sort::OnlineFirst => order.sort_by_key(|&e| {
            let en = &idx.entries[e as usize];
            !(en.live || en.status == Status::Up)
        }),
    }
    let mut s = Shown {
        scope,
        hosts: vec![Vec::new(); n],
        total: vec![0; n],
        roll: vec![Roll::default(); n],
        loose: Vec::new(),
        order,
    };
    for &e in &s.order {
        let en = &idx.entries[e as usize];
        match en.node {
            Some(nd) => {
                s.hosts[nd].push(e);
                let mut cur = Some(nd);
                let mut steps = 0;
                while let Some(c) = cur {
                    s.total[c] += 1;
                    s.roll[c].add(en);
                    steps += 1;
                    if steps > 64 {
                        break;
                    }
                    cur = idx.nodes[c].parent;
                }
            }
            None => s.loose.push(e),
        }
    }
    s
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Group(u32),
    Host(u32),
    /// "OHNE GRUPPE" header (also the drop zone to take hosts out of a group).
    Loose,
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub kind: RowKind,
    pub depth: u8,
    /// First / last host row of a block (rounded corners).
    pub first: bool,
    pub last: bool,
    pub h: f32,
}

/// Row heights (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heights {
    pub group_root: f32,
    pub group: f32,
    pub host: f32,
    pub loose: f32,
}

pub struct Layout {
    pub rows: Vec<Row>,
    /// `y[i]` = top of row `i`; `y[rows.len()]` = total height.
    pub y: Vec<f32>,
}

impl Layout {
    pub fn total(&self) -> f32 {
        self.y.last().copied().unwrap_or(0.0)
    }

    pub fn row_of_group(&self, node: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.kind == RowKind::Group(node as u32))
    }
}

pub struct FlattenOpts<'a> {
    /// A search or filter is active: groups without hits disappear.
    pub filtering: bool,
    /// A host is being dragged: show the "no group" drop zone even if it is empty.
    pub drag: bool,
    pub heights: Heights,
    pub is_open: &'a dyn Fn(usize) -> bool,
}

pub fn flatten(idx: &Index, shown: &Shown, o: &FlattenOpts) -> Layout {
    let mut rows: Vec<Row> = Vec::new();
    let hs = o.heights;

    fn host_block(rows: &mut Vec<Row>, hosts: &[u32], depth: u8, h: f32) {
        for (i, e) in hosts.iter().enumerate() {
            rows.push(Row {
                kind: RowKind::Host(*e),
                depth,
                first: i == 0,
                last: i + 1 == hosts.len(),
                h,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        idx: &Index,
        shown: &Shown,
        o: &FlattenOpts,
        rows: &mut Vec<Row>,
        n: usize,
        depth: u8,
        guard: usize,
    ) {
        if guard > 64 || (o.filtering && shown.total[n] == 0) {
            return;
        }
        rows.push(Row {
            kind: RowKind::Group(n as u32),
            depth,
            first: false,
            last: false,
            h: if depth == 0 {
                o.heights.group_root
            } else {
                o.heights.group
            },
        });
        if !(o.is_open)(n) {
            return;
        }
        host_block(rows, &shown.hosts[n], depth, o.heights.host);
        for &c in &idx.nodes[n].children {
            node(idx, shown, o, rows, c, depth + 1, guard + 1);
        }
    }

    match shown.scope {
        Some(s) => {
            host_block(&mut rows, &shown.hosts[s], 0, hs.host);
            for &c in &idx.nodes[s].children {
                node(idx, shown, o, &mut rows, c, 0, 0);
            }
        }
        None => {
            for &r in &idx.roots {
                node(idx, shown, o, &mut rows, r, 0, 0);
            }
            let has_groups = !idx.nodes.is_empty();
            if has_groups && (!shown.loose.is_empty() || (o.drag && !o.filtering)) {
                rows.push(Row {
                    kind: RowKind::Loose,
                    depth: 0,
                    first: false,
                    last: false,
                    h: hs.loose,
                });
            }
            host_block(&mut rows, &shown.loose, 0, hs.host);
        }
    }
    // The very first group header needs no gap above it.
    if let Some(first) = rows.first_mut()
        && matches!(first.kind, RowKind::Group(_))
    {
        first.h = hs.group;
    }
    let mut y = Vec::with_capacity(rows.len() + 1);
    let mut acc = 0.0;
    for r in &rows {
        y.push(acc);
        acc += r.h;
    }
    y.push(acc);
    Layout { rows, y }
}

/// Hosts that pass `keep` and match the plain words of `q`, best match first (ties by
/// name). `keep` is consulted first, so it should hold the cheap checks.
pub fn rank(
    idx: &Index,
    q: &crate::nav_query::Query,
    keep: &dyn Fn(usize) -> bool,
    scope: Option<usize>,
) -> Vec<u32> {
    let mut hits: Vec<(i32, u32)> = Vec::new();
    for &e in &idx.by_name {
        let en = &idx.entries[e as usize];
        if let Some(s) = scope
            && !en.node.is_some_and(|nd| idx.under(nd, s))
        {
            continue;
        }
        if !keep(e as usize) {
            continue;
        }
        if let Some(score) = q.score(&en.hay) {
            hits.push((score, e));
        }
    }
    // `by_name` is the tie order; the sort is stable.
    hits.sort_by_key(|(s, _)| Reverse(*s));
    hits.into_iter().map(|(_, e)| e).collect()
}

/// A plain list of host rows (search results, no groups).
pub fn flat_layout(hosts: &[u32], row_h: f32) -> Layout {
    let rows: Vec<Row> = hosts
        .iter()
        .enumerate()
        .map(|(i, e)| Row {
            kind: RowKind::Host(*e),
            depth: 0,
            first: i == 0,
            last: i + 1 == hosts.len(),
            h: row_h,
        })
        .collect();
    let y = (0..=rows.len()).map(|i| i as f32 * row_h).collect();
    Layout { rows, y }
}

/// Rows overlapping the viewport `top..bottom` (scroll offsets), as `start..end`.
/// `y` is the prefix-sum array of [`Layout`].
pub fn visible_range(y: &[f32], top: f32, bottom: f32) -> std::ops::Range<usize> {
    let n = y.len().saturating_sub(1);
    if n == 0 {
        return 0..0;
    }
    let first = y.partition_point(|&v| v <= top).saturating_sub(1).min(n);
    let end = y.partition_point(|&v| v < bottom).min(n);
    first..end.max(first)
}

/// Scroll offset that brings row `i` into the viewport of height `view_h` with minimal
/// movement; `None` if it is already fully visible.
pub fn scroll_to_row(y: &[f32], i: usize, top: f32, view_h: f32) -> Option<f32> {
    let (a, b) = (*y.get(i)?, *y.get(i + 1)?);
    if a < top {
        Some(a)
    } else if b > top + view_h {
        Some((b - view_h).max(0.0))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Group;

    fn group(lib: &mut Library, name: &str, parent: Option<Uuid>) -> Uuid {
        let g = Group {
            name: name.into(),
            parent_id: parent,
            ..Default::default()
        };
        let id = g.id;
        lib.groups.push(g);
        id
    }

    fn host(lib: &mut Library, name: &str, group: Option<Uuid>) -> Uuid {
        let h = Host {
            label: name.into(),
            hostname: format!("{name}.example"),
            group_id: group,
            ..Default::default()
        };
        let id = h.id;
        lib.hosts.push(h);
        id
    }

    fn st(map: HashMap<Uuid, Status>) -> impl FnMut(&Host) -> (Status, bool) {
        move |h| (map.get(&h.id).copied().unwrap_or_default(), false)
    }

    /// Kunde B{Berlin{sw-1, sw-2}, Wien{fw-1}}, Kunde A{sw-9}, loose host.
    fn sample() -> (Library, Vec<Uuid>) {
        let mut lib = Library::default();
        let b = group(&mut lib, "Kunde B", None);
        let a = group(&mut lib, "Kunde A", None);
        let berlin = group(&mut lib, "Berlin", Some(b));
        let wien = group(&mut lib, "Wien", Some(b));
        let ids = vec![
            host(&mut lib, "sw-2", Some(berlin)),
            host(&mut lib, "sw-1", Some(berlin)),
            host(&mut lib, "fw-1", Some(wien)),
            host(&mut lib, "sw-9", Some(a)),
            host(&mut lib, "loose", None),
        ];
        (lib, ids)
    }

    const HS: Heights = Heights {
        group_root: 50.0,
        group: 40.0,
        host: 10.0,
        loose: 30.0,
    };

    fn open_all(_: usize) -> bool {
        true
    }

    fn opts<'a>(is_open: &'a dyn Fn(usize) -> bool, filtering: bool) -> FlattenOpts<'a> {
        FlattenOpts {
            filtering,
            drag: false,
            heights: HS,
            is_open,
        }
    }

    fn names(lib: &Library, idx: &Index, l: &Layout) -> Vec<String> {
        l.rows
            .iter()
            .map(|r| match r.kind {
                RowKind::Group(n) => format!("[{}]", idx.nodes[n as usize].name),
                RowKind::Host(e) => lib.hosts[idx.entries[e as usize].host].label.clone(),
                RowKind::Loose => "(loose)".into(),
            })
            .collect()
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("kunde 2", "kunde 10"), Ordering::Less);
        assert_eq!(natural_cmp("Kunde A", "kunde b"), Ordering::Less);
        assert_eq!(natural_cmp("sw-007", "sw-7"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
    }

    #[test]
    fn builds_a_three_level_tree_sorted_by_name() {
        let (lib, _) = sample();
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let shown = compute(&idx, &|_| true, None, Sort::Name);
        let l = flatten(&idx, &shown, &opts(&open_all, false));
        assert_eq!(
            names(&lib, &idx, &l),
            [
                "[Kunde A]",
                "sw-9",
                "[Kunde B]",
                "[Berlin]",
                "sw-1",
                "sw-2",
                "[Wien]",
                "fw-1",
                "(loose)",
                "loose"
            ]
        );
        // Counts per node include subgroups.
        let kb = idx.by_id[&lib.groups[0].id];
        assert_eq!(shown.total[kb], 3);
        assert_eq!(shown.hosts[kb].len(), 0);
        assert_eq!(
            idx.nodes[idx.by_id[&lib.groups[2].id]].path,
            ["Kunde B", "Berlin"]
        );
    }

    #[test]
    fn collapsed_groups_hide_their_rows() {
        let (lib, _) = sample();
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let shown = compute(&idx, &|_| true, None, Sort::Name);
        let kb = idx.by_id[&lib.groups[0].id];
        let open = move |n: usize| n != kb;
        let l = flatten(&idx, &shown, &opts(&open, false));
        assert_eq!(
            names(&lib, &idx, &l),
            ["[Kunde A]", "sw-9", "[Kunde B]", "(loose)", "loose"]
        );
        // Heights: first header without gap, later root header with it.
        assert_eq!(l.rows[0].h, HS.group);
        assert_eq!(l.rows[2].h, HS.group_root);
        assert_eq!(l.total(), 40.0 + 10.0 + 50.0 + 30.0 + 10.0);
        assert!(l.rows[1].first && l.rows[1].last);
    }

    #[test]
    fn filtering_drops_empty_groups_and_counts_matches() {
        let (lib, ids) = sample();
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let want = ids[2]; // fw-1 in Kunde B / Wien
        let keep = |e: usize| lib.hosts[idx.entries[e].host].id == want;
        let shown = compute(&idx, &keep, None, Sort::Name);
        let l = flatten(&idx, &shown, &opts(&open_all, true));
        assert_eq!(names(&lib, &idx, &l), ["[Kunde B]", "[Wien]", "fw-1"]);
        let kb = idx.by_id[&lib.groups[0].id];
        assert_eq!(shown.total[kb], 1);
        assert!(shown.loose.is_empty());
    }

    #[test]
    fn scope_shows_only_that_subtree() {
        let (lib, _) = sample();
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let kb = idx.by_id[&lib.groups[0].id];
        let shown = compute(&idx, &|_| true, Some(kb), Sort::Name);
        assert_eq!(shown.order.len(), 3, "loose host and Kunde A are out");
        let l = flatten(&idx, &shown, &opts(&open_all, false));
        assert_eq!(
            names(&lib, &idx, &l),
            ["[Berlin]", "sw-1", "sw-2", "[Wien]", "fw-1"]
        );
        assert_eq!(l.rows[0].h, HS.group, "no gap above the first header");
    }

    #[test]
    fn scope_group_with_direct_hosts() {
        let mut lib = Library::default();
        let g = group(&mut lib, "Acme", None);
        let sub = group(&mut lib, "Site", Some(g));
        host(&mut lib, "direct", Some(g));
        host(&mut lib, "deep", Some(sub));
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let shown = compute(&idx, &|_| true, Some(0), Sort::Name);
        let l = flatten(&idx, &shown, &opts(&open_all, false));
        assert_eq!(names(&lib, &idx, &l), ["direct", "[Site]", "deep"]);
    }

    #[test]
    fn rollup_counts_states_up_the_tree() {
        let (mut lib, ids) = sample();
        // sw-1 disk 95 % (critical), sw-2 down, fw-1 up, sw-9 failed.
        lib.facts.insert(
            ids[1],
            HostFacts {
                disk_used_pct: Some(95),
                ..Default::default()
            },
        );
        let map = HashMap::from([
            (ids[0], Status::Down),
            (ids[2], Status::Up),
            (ids[3], Status::Failed),
        ]);
        let idx = Index::build(&lib, &mut st(map));
        let shown = compute(&idx, &|_| true, None, Sort::Name);
        let kb = idx.by_id[&lib.groups[0].id];
        let ka = idx.by_id[&lib.groups[1].id];
        let berlin = idx.by_id[&lib.groups[2].id];
        let r = shown.roll[kb];
        assert_eq!((r.up, r.down, r.unknown, r.crit), (1, 1, 1, 1));
        assert_eq!(r.red(), 2);
        assert_eq!(shown.roll[berlin].red(), 2);
        assert_eq!(shown.roll[ka].down, 1);
        assert_eq!(shown.roll[ka].probed(), 1);
    }

    #[test]
    fn refresh_status_updates_in_place() {
        let (lib, ids) = sample();
        let mut idx = Index::build(&lib, &mut st(HashMap::new()));
        assert_eq!(idx.entries[0].status, Status::Unknown);
        idx.refresh_status(&lib, &mut st(HashMap::from([(ids[0], Status::Down)])));
        assert_eq!(idx.entries[0].status, Status::Down);
        let shown = compute(&idx, &|_| true, None, Sort::Name);
        assert_eq!(shown.roll[idx.by_id[&lib.groups[2].id]].down, 1);
    }

    #[test]
    fn favorites_sort_first_and_sorts_apply() {
        let (mut lib, _) = sample();
        let kb = lib.groups[0].id;
        lib.nav.set_favorite(kb, true);
        lib.hosts[0].last_connected = Some(chrono::Utc::now());
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        assert_eq!(
            idx.nodes[idx.roots[0]].name, "Kunde B",
            "pinned group first"
        );
        let shown = compute(&idx, &|_| true, None, Sort::LastUsed);
        assert_eq!(shown.order[0], 0, "most recently used host first");
        let berlin = idx.by_id[&lib.groups[2].id];
        assert_eq!(shown.hosts[berlin][0], 0);
        let map = HashMap::from([(lib.hosts[2].id, Status::Up)]);
        let idx = Index::build(&lib, &mut st(map));
        let shown = compute(&idx, &|_| true, None, Sort::OnlineFirst);
        assert_eq!(shown.order[0], 2, "online host first");
    }

    #[test]
    fn broken_libraries_do_not_hang() {
        let mut lib = Library::default();
        let a = group(&mut lib, "A", None);
        let b = group(&mut lib, "B", Some(a));
        lib.groups[0].parent_id = Some(b); // cycle A <-> B
        let c = group(&mut lib, "C", Some(Uuid::new_v4())); // parent does not exist
        host(&mut lib, "h", Some(c));
        host(&mut lib, "orphan", Some(Uuid::new_v4())); // group does not exist
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let shown = compute(&idx, &|_| true, None, Sort::Name);
        let l = flatten(&idx, &shown, &opts(&open_all, false));
        assert!(l.rows.len() >= 3);
        assert_eq!(shown.loose.len(), 1, "host of a missing group is loose");
        assert!(idx.roots.contains(&idx.by_id[&c]));
    }

    #[test]
    fn drag_shows_the_drop_zone_for_loose_hosts() {
        let mut lib = Library::default();
        group(&mut lib, "A", None);
        host(&mut lib, "h", None);
        host(&mut lib, "h2", None);
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let shown = compute(&idx, &|_| false, None, Sort::Name);
        let calm = flatten(&idx, &shown, &opts(&open_all, false));
        assert_eq!(calm.rows.len(), 1, "only the group, nothing loose");
        let mut o = opts(&open_all, false);
        o.drag = true;
        let drag = flatten(&idx, &shown, &o);
        assert_eq!(drag.rows.len(), 2);
        assert_eq!(drag.rows[1].kind, RowKind::Loose);
    }

    #[test]
    fn culling_math() {
        // Rows of height 10: y = 0, 10, …, 100.
        let y: Vec<f32> = (0..=10).map(|i| i as f32 * 10.0).collect();
        assert_eq!(visible_range(&y, 0.0, 25.0), 0..3);
        assert_eq!(
            visible_range(&y, 10.0, 20.0),
            1..2,
            "exact row, no neighbours"
        );
        assert_eq!(visible_range(&y, 15.0, 55.0), 1..6);
        assert_eq!(visible_range(&y, 95.0, 400.0), 9..10);
        assert!(
            visible_range(&y, 500.0, 600.0).is_empty(),
            "scrolled past the end"
        );
        assert_eq!(visible_range(&[0.0], 0.0, 100.0), 0..0, "no rows");
        assert_eq!(visible_range(&[], 0.0, 100.0), 0..0);
        // Variable heights.
        let y = [0.0, 50.0, 60.0, 70.0, 120.0];
        assert_eq!(visible_range(&y, 55.0, 65.0), 1..3);
    }

    #[test]
    fn scroll_to_row_moves_minimally() {
        let y: Vec<f32> = (0..=100).map(|i| i as f32 * 10.0).collect();
        assert_eq!(scroll_to_row(&y, 5, 0.0, 100.0), None);
        assert_eq!(scroll_to_row(&y, 20, 0.0, 100.0), Some(110.0));
        assert_eq!(scroll_to_row(&y, 2, 100.0, 100.0), Some(20.0));
        assert_eq!(scroll_to_row(&y, 500, 0.0, 100.0), None);
    }

    #[test]
    fn facts_levels() {
        let s = InventorySettings::default();
        let mut f = HostFacts::default();
        assert_eq!(facts_level(&f, &s), 0);
        f.disk_used_pct = Some(100);
        assert_eq!(facts_level(&f, &s), 2);
        f.disk_used_pct = Some(0);
        f.security_updates = Some(1);
        assert_eq!(facts_level(&f, &s), 2);
        f.security_updates = None;
        f.updates = Some(10_000);
        assert_eq!(facts_level(&f, &s), 1);
    }

    #[test]
    fn ranking_and_flat_layout() {
        let (lib, _) = sample();
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let q = crate::nav_query::Query::parse("sw");
        let hits = rank(&idx, &q, &|_| true, None);
        let labels: Vec<&str> = hits
            .iter()
            .map(|e| lib.hosts[idx.entries[*e as usize].host].label.as_str())
            .collect();
        let mut sorted = labels.clone();
        sorted.sort();
        assert_eq!(sorted, ["sw-1", "sw-2", "sw-9"]);
        let kb = idx.by_id[&lib.groups[0].id];
        assert_eq!(
            rank(&idx, &q, &|_| true, Some(kb)).len(),
            2,
            "scope applies"
        );
        assert!(rank(&idx, &q, &|_| false, None).is_empty());
        let l = flat_layout(&hits, 20.0);
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.total(), 60.0);
        assert!(l.rows[0].first && l.rows[2].last && !l.rows[1].first);
        assert_eq!(flat_layout(&[], 20.0).total(), 0.0);
    }

    #[test]
    fn query_runs_on_the_index() {
        let (lib, _) = sample();
        let idx = Index::build(&lib, &mut st(HashMap::new()));
        let q = crate::nav_query::Query::parse("kunde b/ wien/fw");
        // "kunde b" has a space: only the quoted form is a path, so this is words.
        assert!(!q.words.is_empty());
        let q = crate::nav_query::Query::parse("\"kunde b\"/wien/fw");
        let keep = |e: usize| q.matches(&idx.facets(&lib, e));
        let shown = compute(&idx, &keep, None, Sort::Name);
        assert_eq!(shown.order.len(), 1);
        let q = crate::nav_query::Query::parse("type:berlin");
        let keep = |e: usize| q.matches(&idx.facets(&lib, e));
        assert_eq!(compute(&idx, &keep, None, Sort::Name).order.len(), 2);
    }
}
