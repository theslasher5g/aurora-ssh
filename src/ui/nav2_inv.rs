//! Cache of the inventory page for big libraries. `rules::rows` (every host with its
//! facts, warnings and group path) used to run twice per frame; now the rows are built
//! once and only rebuilt when the library, the collected facts, the scope or the clock
//! (every minute, for "facts are N days old") change. Searching, filtering, sorting and
//! grouping only redo the cheap second stage. The table draws only the rows in view
//! (see `inventory_table`), so a frame costs the same for 200 and for 20 000 hosts.

use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::app::App;
use crate::backend::inventory_rules::{self as rules, Col, Filter, Row, Summary};

/// Height of a host row and of a group header in the table (points).
pub const ROW_H: f32 = 48.0;
pub const HEAD_H: f32 = 32.0;
/// "Facts are N days old" and the relative times need a refresh now and then.
const MAX_AGE_SECS: u64 = 60;

/// One line of the table: a group header or a row (index into [`InvCache::all`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Head(u32),
    Row(u32),
}

/// What the table draws: lines, their tops (prefix sums) and the group headers.
#[derive(Default)]
pub struct Items {
    pub items: Vec<Item>,
    /// `y[i]` = top of line `i`, `y[len]` = total height.
    pub y: Vec<f32>,
    /// `(group name, rows)` for `Item::Head`.
    pub heads: Vec<(String, usize)>,
}

#[derive(Default, Clone, PartialEq)]
struct Stage1 {
    rev: u64,
    facts: u64,
    hosts: usize,
    scope: Option<Uuid>,
}

#[derive(Default, Clone, PartialEq)]
struct Stage2 {
    s1: Stage1,
    built: u64,
    text: String,
    only_problems: bool,
    only_prod: bool,
    group: Option<Uuid>,
    sort: String,
    desc: bool,
    by_group: bool,
}

#[derive(Default)]
pub struct InvCache {
    k1: Option<Stage1>,
    k2: Option<Stage2>,
    built: Option<Instant>,
    /// Bumped whenever the rows were rebuilt (stage 1).
    built_gen: u64,
    /// Rows of the scope in library order, with their lowercase search text.
    pub all: Vec<Row>,
    hay: Vec<String>,
    pub by_id: HashMap<Uuid, u32>,
    pub summary: Summary,
    /// Indices into `all`: filtered and sorted.
    pub order: Vec<u32>,
    pub items: Items,
    /// Milliseconds the last stage-1 / stage-2 builds took (for `AURORA_PERF`).
    pub ms: (f32, f32),
}

impl InvCache {
    pub fn row(&self, id: Uuid) -> Option<&Row> {
        self.by_id.get(&id).map(|i| &self.all[*i as usize])
    }

    /// Rows in display order.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.order.iter().map(|i| &self.all[*i as usize])
    }

    /// Does the cache describe exactly what `app` shows right now?
    fn fresh(&self, app: &App) -> bool {
        match (&self.k1, &self.k2) {
            (Some(a), Some(b)) => *a == stage1(app) && *b == stage2(app, a.clone(), self.built_gen),
            _ => false,
        }
    }
}

fn stage1(app: &App) -> Stage1 {
    Stage1 {
        rev: app.nav.rev,
        facts: app.inv.facts_epoch,
        hosts: app.lib.hosts.len(),
        scope: app.lib.nav.scope,
    }
}

fn stage2(app: &App, s1: Stage1, built: u64) -> Stage2 {
    let s = &app.lib.settings.inventory;
    Stage2 {
        s1,
        built,
        text: app.inv.text.clone(),
        only_problems: app.inv.only_problems,
        only_prod: app.inv.only_prod,
        group: app.inv.group,
        sort: s.sort_col.clone(),
        desc: s.sort_desc,
        by_group: s.group_by_group,
    }
}

/// Brings the cache of the inventory page up to date (cheap when nothing changed).
pub fn ensure(app: &mut App) {
    let mut c = std::mem::take(&mut app.inv.cache);
    let s1 = stage1(app);
    let old = c
        .built
        .is_none_or(|t| t.elapsed().as_secs() >= MAX_AGE_SECS);
    if c.k1.as_ref() != Some(&s1) || old {
        let t0 = Instant::now();
        let now = Utc::now();
        let scope = app.lib.nav.scope;
        let mut all = rules::rows(&app.lib, now);
        if let Some(s) = scope {
            all.retain(|r| r.group_chain.contains(&s));
        }
        c.hay = all.iter().map(rules::hay).collect();
        c.by_id = all
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id, i as u32))
            .collect();
        c.summary = rules::summarize(&all);
        c.all = all;
        c.k1 = Some(s1.clone());
        c.built = Some(Instant::now());
        c.built_gen += 1;
        c.k2 = None;
        c.ms.0 = t0.elapsed().as_secs_f32() * 1000.0;
        if crate::nav_perf::enabled() {
            eprintln!(
                "perf: inventory rows rebuilt: {} rows in {:.1}ms",
                c.all.len(),
                c.ms.0
            );
        }
        // The selection only holds hosts that still exist.
        let by_id = &c.by_id;
        app.inv.selected.retain(|i| by_id.contains_key(i));
    }
    let k2 = stage2(app, s1, c.built_gen);
    if c.k2.as_ref() != Some(&k2) {
        let t0 = Instant::now();
        let f = Filter {
            text: app.inv.text.clone(),
            only_problems: app.inv.only_problems,
            only_prod: app.inv.only_prod,
            group: app.inv.group,
        };
        let text_on = !f.text.trim().is_empty();
        let mut order: Vec<u32> = (0..c.all.len() as u32)
            .filter(|&i| {
                rules::matches_flags(&c.all[i as usize], &f)
                    && (!text_on || rules::matches_hay(&c.hay[i as usize], &f))
            })
            .collect();
        let col = Col::from_key(&k2.sort).unwrap_or(Col::Name);
        sort_indices(&c.all, &mut order, col, k2.desc, Utc::now());
        c.items = build_items(&c.all, &order, k2.by_group);
        c.order = order;
        c.k2 = Some(k2);
        c.ms.1 = t0.elapsed().as_secs_f32() * 1000.0;
        if crate::nav_perf::enabled() {
            eprintln!(
                "perf: inventory filter+sort: {} of {} rows in {:.1}ms",
                c.order.len(),
                c.all.len(),
                c.ms.1
            );
        }
    }
    app.inv.cache = c;
}

/// Sorts indices into `all` like `rules::sort_rows` sorts rows.
fn sort_indices(all: &[Row], order: &mut Vec<u32>, col: Col, desc: bool, now: DateTime<Utc>) {
    // `sort_rows` works on a slice of rows: sort lightweight copies of the sort keys by
    // sorting the rows themselves once and mapping back through the ids.
    let mut rows: Vec<Row> = order.iter().map(|i| all[*i as usize].clone()).collect();
    rules::sort_rows(&mut rows, col, desc, now);
    let pos: HashMap<Uuid, u32> = order.iter().map(|i| (all[*i as usize].id, *i)).collect();
    *order = rows.iter().map(|r| pos[&r.id]).collect();
}

/// Lines of the table: one group header per group (empty group last) or just rows.
pub fn build_items(all: &[Row], order: &[u32], by_group: bool) -> Items {
    let mut out = Items::default();
    if !by_group {
        out.items = order.iter().map(|i| Item::Row(*i)).collect();
    } else {
        let mut idx: HashMap<&str, usize> = HashMap::new();
        let mut buckets: Vec<(&str, Vec<u32>)> = Vec::new();
        for &i in order {
            let g = all[i as usize].group.as_str();
            let b = *idx.entry(g).or_insert_with(|| {
                buckets.push((g, Vec::new()));
                buckets.len() - 1
            });
            buckets[b].1.push(i);
        }
        buckets.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
        });
        for (name, rows) in buckets {
            let name = if name.is_empty() {
                t!("Ohne Gruppe").to_string()
            } else {
                name.to_string()
            };
            out.heads.push((name, rows.len()));
            out.items.push(Item::Head(out.heads.len() as u32 - 1));
            out.items.extend(rows.into_iter().map(Item::Row));
        }
    }
    let mut y = Vec::with_capacity(out.items.len() + 1);
    let mut acc = 0.0;
    for it in &out.items {
        y.push(acc);
        acc += match it {
            Item::Head(_) => HEAD_H,
            Item::Row(_) => ROW_H,
        };
    }
    y.push(acc);
    out.y = y;
    out
}

/// The row of one host for the detail sheet: from the cache, or (host outside the scope
/// or cache not built yet) computed.
pub fn detail_row(app: &App, id: Uuid, now: DateTime<Utc>) -> Option<Row> {
    if let Some(r) = app.inv.cache.row(id) {
        return Some(r.clone());
    }
    rules::rows(&app.lib, now).into_iter().find(|r| r.id == id)
}

/// Filtered and sorted rows as owned values (export). Uses the cache if it is current.
pub fn export_rows(app: &App) -> Vec<Row> {
    let c = &app.inv.cache;
    if c.fresh(app) {
        return c.rows().cloned().collect();
    }
    // Cache not current (e.g. a filter changed in this very frame): compute directly.
    let now = Utc::now();
    let s = &app.lib.settings.inventory;
    let filter = Filter {
        text: app.inv.text.clone(),
        only_problems: app.inv.only_problems,
        only_prod: app.inv.only_prod,
        group: app.inv.group,
    };
    let scope = app.lib.nav.scope;
    let mut rows: Vec<Row> = rules::rows(&app.lib, now)
        .into_iter()
        .filter(|r| scope.is_none_or(|s| r.group_chain.contains(&s)))
        .filter(|r| rules::matches(r, &filter))
        .collect();
    let sort = Col::from_key(&s.sort_col).unwrap_or(Col::Name);
    rules::sort_rows(&mut rows, sort, s.sort_desc, now);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Env;

    fn row(name: &str, group: &str) -> Row {
        Row {
            id: Uuid::new_v4(),
            name: name.into(),
            group: group.into(),
            group_chain: Vec::new(),
            address: String::new(),
            protocol: crate::model::Protocol::Ssh,
            os: String::new(),
            kernel: String::new(),
            uptime_secs: None,
            disk: None,
            updates: None,
            sec_updates: None,
            cpus: None,
            mem_mb: None,
            mem_pct: None,
            load1: None,
            last_connected: None,
            facts_at: None,
            env: Env::None,
            tags: Vec::new(),
            warnings: Vec::new(),
            has_facts: false,
            failed_services: None,
        }
    }

    #[test]
    fn flat_items_have_no_headers() {
        let all = vec![row("a", "G"), row("b", "G")];
        let it = build_items(&all, &[1, 0], false);
        assert_eq!(it.items, vec![Item::Row(1), Item::Row(0)]);
        assert_eq!(it.y, vec![0.0, ROW_H, 2.0 * ROW_H]);
        assert!(it.heads.is_empty());
    }

    #[test]
    fn grouped_items_sort_groups_and_put_loose_last() {
        let all = vec![
            row("a", "Zeta"),
            row("b", ""),
            row("c", "alpha"),
            row("d", "Zeta"),
        ];
        let it = build_items(&all, &[0, 1, 2, 3], true);
        let names: Vec<&str> = it.heads.iter().map(|h| h.0.as_str()).collect();
        assert_eq!(names[0], "alpha");
        assert_eq!(names[1], "Zeta");
        assert_eq!(it.heads[1].1, 2);
        assert_eq!(it.heads.len(), 3, "the group-less rows form the last block");
        assert_eq!(it.items.len(), 4 + 3);
        assert!(matches!(it.items[0], Item::Head(_)));
        let total = it.y[it.items.len()];
        assert_eq!(total, 3.0 * HEAD_H + 4.0 * ROW_H);
    }

    #[test]
    fn sorting_indices_matches_sorting_rows() {
        let all = vec![row("kunde 10", ""), row("Kunde 2", ""), row("kunde 1", "")];
        let mut order = vec![0u32, 1, 2];
        sort_indices(&all, &mut order, Col::Name, false, Utc::now());
        assert_eq!(order, vec![2, 1, 0]);
        sort_indices(&all, &mut order, Col::Name, true, Utc::now());
        assert_eq!(order, vec![0, 1, 2]);
    }
}
