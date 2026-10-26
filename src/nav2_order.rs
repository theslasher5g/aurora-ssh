//! Manual order of groups among their siblings. The tree sorts pinned groups first, then
//! groups with a manual position (by that position), then everything else by name
//! (numbers sort naturally). Pure functions, no egui.

use std::cmp::Ordering;

use uuid::Uuid;

use crate::nav_tree::natural_cmp;

/// What the order of one group depends on.
#[derive(Clone, Debug)]
pub struct Sib<'a> {
    pub id: Uuid,
    pub pinned: bool,
    /// Manual position (`None` = never moved by hand).
    pub rank: Option<u32>,
    pub name: &'a str,
}

/// The comparison the tree uses for siblings: pinned first, then manual position (placed
/// groups before unplaced ones), then name.
pub fn cmp(a: &Sib, b: &Sib) -> Ordering {
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| match (a.rank, b.rank) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| natural_cmp(a.name, b.name))
}

/// Sibling ids in display order.
pub fn sorted(sibs: &[Sib]) -> Vec<Uuid> {
    let mut v: Vec<&Sib> = sibs.iter().collect();
    v.sort_by(|a, b| cmp(a, b));
    v.into_iter().map(|s| s.id).collect()
}

/// New order after moving `id` by `delta` places (`-1` up, `1` down). Pinned groups stay
/// above the others, so a group only moves among groups of its own kind; `None` if it
/// cannot move (already at the end, or unknown).
pub fn moved(sibs: &[Sib], id: Uuid, delta: i32) -> Option<Vec<Uuid>> {
    let order = sorted(sibs);
    let pos = order.iter().position(|x| *x == id)?;
    let pinned = |i: Uuid| sibs.iter().find(|s| s.id == i).is_some_and(|s| s.pinned);
    let kind = pinned(id);
    let mut target = pos as i64;
    let mut left = delta.unsigned_abs();
    let dir = delta.signum() as i64;
    while left > 0 {
        let next = target + dir;
        if next < 0 || next as usize >= order.len() || pinned(order[next as usize]) != kind {
            break;
        }
        target = next;
        left -= 1;
    }
    if target == pos as i64 {
        return None;
    }
    let mut out = order;
    let item = out.remove(pos);
    out.insert(target as usize, item);
    Some(out)
}

/// New order after dropping `id` before (`after == false`) or after the sibling `onto`.
/// Dropping a pinned group among unpinned ones (or the reverse) changes nothing.
pub fn dropped(sibs: &[Sib], id: Uuid, onto: Uuid, after: bool) -> Option<Vec<Uuid>> {
    if id == onto {
        return None;
    }
    let order = sorted(sibs);
    let pinned = |i: Uuid| sibs.iter().find(|s| s.id == i).is_some_and(|s| s.pinned);
    if pinned(id) != pinned(onto) {
        return None;
    }
    let from = order.iter().position(|x| *x == id)?;
    let mut out = order;
    out.remove(from);
    let at = out.iter().position(|x| *x == onto)?;
    out.insert(if after { at + 1 } else { at }, id);
    Some(out)
}

/// Ranks to store for a new order: every sibling gets its position.
pub fn ranks(order: &[Uuid]) -> Vec<(Uuid, u32)> {
    order
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sib(name: &str, pinned: bool, rank: Option<u32>) -> (Uuid, String, bool, Option<u32>) {
        (Uuid::new_v4(), name.to_string(), pinned, rank)
    }

    fn view(v: &[(Uuid, String, bool, Option<u32>)]) -> Vec<Sib<'_>> {
        v.iter()
            .map(|(id, n, p, r)| Sib {
                id: *id,
                pinned: *p,
                rank: *r,
                name: n,
            })
            .collect()
    }

    fn names(v: &[(Uuid, String, bool, Option<u32>)], order: &[Uuid]) -> Vec<String> {
        order
            .iter()
            .map(|i| v.iter().find(|x| x.0 == *i).unwrap().1.clone())
            .collect()
    }

    #[test]
    fn pinned_first_then_manual_then_name() {
        let v = vec![
            sib("kunde 10", false, None),
            sib("kunde 2", false, None),
            sib("zeta", false, Some(0)),
            sib("alpha", false, Some(1)),
            sib("pin", true, None),
        ];
        let o = sorted(&view(&v));
        assert_eq!(
            names(&v, &o),
            vec!["pin", "zeta", "alpha", "kunde 2", "kunde 10"]
        );
    }

    #[test]
    fn move_up_and_down() {
        let v = vec![
            sib("a", false, None),
            sib("b", false, None),
            sib("c", false, None),
        ];
        let s = view(&v);
        let c = v[2].0;
        let up = moved(&s, c, -1).unwrap();
        assert_eq!(names(&v, &up), vec!["a", "c", "b"]);
        let top = moved(&s, c, -5).unwrap();
        assert_eq!(names(&v, &top), vec!["c", "a", "b"]);
        assert!(moved(&s, v[0].0, -1).is_none(), "already first");
        assert!(moved(&s, c, 1).is_none(), "already last");
        assert!(moved(&s, Uuid::new_v4(), 1).is_none());
    }

    #[test]
    fn pinned_groups_do_not_cross_the_pin_line() {
        let v = vec![
            sib("p1", true, None),
            sib("p2", true, None),
            sib("a", false, None),
        ];
        let s = view(&v);
        assert!(
            moved(&s, v[2].0, -1).is_none(),
            "cannot move above the pins"
        );
        assert!(moved(&s, v[1].0, 1).is_none(), "cannot move below the pins");
        let swap = moved(&s, v[1].0, -1).unwrap();
        assert_eq!(names(&v, &swap), vec!["p2", "p1", "a"]);
    }

    #[test]
    fn drop_before_and_after() {
        let v = vec![
            sib("a", false, None),
            sib("b", false, None),
            sib("c", false, None),
        ];
        let s = view(&v);
        let o = dropped(&s, v[0].0, v[2].0, true).unwrap();
        assert_eq!(names(&v, &o), vec!["b", "c", "a"]);
        let o = dropped(&s, v[2].0, v[0].0, false).unwrap();
        assert_eq!(names(&v, &o), vec!["c", "a", "b"]);
        assert!(dropped(&s, v[0].0, v[0].0, true).is_none());
    }

    #[test]
    fn stored_ranks_reproduce_the_order() {
        let v = vec![
            sib("a", false, None),
            sib("b", false, None),
            sib("c", false, None),
        ];
        let o = moved(&view(&v), v[2].0, -2).unwrap();
        let r: std::collections::HashMap<Uuid, u32> = ranks(&o).into_iter().collect();
        let v2: Vec<_> = v
            .iter()
            .map(|(id, n, p, _)| (*id, n.clone(), *p, r.get(id).copied()))
            .collect();
        assert_eq!(sorted(&view(&v2)), o);
        // A group created later (no rank) goes after the placed ones.
        let mut v3 = v2.clone();
        v3.push(sib("0 new", false, None));
        let o3 = sorted(&view(&v3));
        assert_eq!(names(&v3, &o3).last().unwrap(), "0 new");
    }
}
