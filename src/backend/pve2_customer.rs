//! Customers of Proxmox servers. A customer is the top-level group a server is assigned
//! to (`PveServer::group` / `PbsServer::group`); servers without a group form the
//! "ohne Kunde" bucket. Pure logic over a flat group list.

use std::collections::HashMap;

use uuid::Uuid;

/// A group as far as customer logic cares.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupRow {
    pub id: Uuid,
    pub parent: Option<Uuid>,
    pub name: String,
}

/// Which customer the Proxmox pages show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sel {
    #[default]
    All,
    Customer(Uuid),
    /// Servers without a group.
    Unassigned,
}

/// One entry of the selector.
#[derive(Clone, Debug, PartialEq)]
pub struct Customer {
    /// `None` = servers without a group.
    pub id: Option<Uuid>,
    pub name: String,
    /// Servers (VE and PBS) of this customer.
    pub servers: usize,
}

fn by_id(groups: &[GroupRow]) -> HashMap<Uuid, &GroupRow> {
    groups.iter().map(|g| (g.id, g)).collect()
}

/// Top-level ancestor of `group` (the group itself if it has no parent); `None` for an
/// unknown id. Cycle-safe.
pub fn root_of(groups: &[GroupRow], group: Uuid) -> Option<Uuid> {
    let map = by_id(groups);
    let mut cur = *map.get(&group)?;
    for _ in 0..=groups.len() {
        match cur.parent.and_then(|p| map.get(&p)) {
            Some(p) => cur = p,
            None => return Some(cur.id),
        }
    }
    // A loop: pick something stable.
    Some(cur.id)
}

/// The customer of a server's group (`None` = no customer, also for deleted groups).
pub fn customer_of(groups: &[GroupRow], group: Option<Uuid>) -> Option<Uuid> {
    root_of(groups, group?)
}

/// Group names from the top-level group down to `group`.
pub fn path_of(groups: &[GroupRow], group: Uuid) -> Vec<String> {
    let map = by_id(groups);
    let mut out = Vec::new();
    let mut cur = map.get(&group).copied();
    for _ in 0..=groups.len() {
        let Some(g) = cur else { break };
        out.push(g.name.clone());
        cur = g.parent.and_then(|p| map.get(&p).copied());
    }
    out.reverse();
    out
}

/// Does a server with `group` belong to `sel`?
pub fn matches(groups: &[GroupRow], group: Option<Uuid>, sel: Sel) -> bool {
    match sel {
        Sel::All => true,
        Sel::Customer(c) => customer_of(groups, group) == Some(c),
        Sel::Unassigned => customer_of(groups, group).is_none(),
    }
}

/// Is `group` inside the global customer scope (`Library::nav.scope`)?
pub fn in_scope(groups: &[GroupRow], group: Option<Uuid>, scope: Option<Uuid>) -> bool {
    let Some(scope) = scope else { return true };
    let map = by_id(groups);
    let mut cur = group.and_then(|g| map.get(&g).copied());
    for _ in 0..=groups.len() {
        let Some(g) = cur else { return false };
        if g.id == scope {
            return true;
        }
        cur = g.parent.and_then(|p| map.get(&p).copied());
    }
    false
}

/// Selector entries for servers with the given groups: customers by name, then the
/// "ohne Kunde" bucket (only if some server has none).
pub fn customers(groups: &[GroupRow], server_groups: &[Option<Uuid>]) -> Vec<Customer> {
    let mut count: HashMap<Option<Uuid>, usize> = HashMap::new();
    for g in server_groups {
        *count.entry(customer_of(groups, *g)).or_default() += 1;
    }
    let names: HashMap<Uuid, &str> = groups.iter().map(|g| (g.id, g.name.as_str())).collect();
    let mut out: Vec<Customer> = count
        .into_iter()
        .map(|(id, servers)| Customer {
            id,
            name: id
                .and_then(|i| names.get(&i).map(|n| n.to_string()))
                .unwrap_or_default(),
            servers,
        })
        .collect();
    out.sort_by(|a, b| match (a.id, b.id) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        _ => crate::nav_tree::natural_cmp(&a.name, &b.name),
    });
    out
}

/// A selection that no longer exists (customer lost its last server) falls back to
/// "all".
pub fn normalize(sel: Sel, list: &[Customer]) -> Sel {
    match sel {
        Sel::All => Sel::All,
        Sel::Customer(c) if list.iter().any(|x| x.id == Some(c)) => sel,
        Sel::Unassigned if list.iter().any(|x| x.id.is_none()) => sel,
        _ => Sel::All,
    }
}

/// Servers grouped customer → server for display: `(customer, indexes into the input)`
/// in selector order. Servers keep their input order inside a customer.
pub fn group_servers(
    groups: &[GroupRow],
    server_groups: &[Option<Uuid>],
) -> Vec<(Customer, Vec<usize>)> {
    customers(groups, server_groups)
        .into_iter()
        .map(|c| {
            let idx = server_groups
                .iter()
                .enumerate()
                .filter(|(_, g)| customer_of(groups, **g) == c.id)
                .map(|(i, _)| i)
                .collect();
            (c, idx)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(n: u128, parent: Option<u128>, name: &str) -> GroupRow {
        GroupRow {
            id: Uuid::from_u128(n),
            parent: parent.map(Uuid::from_u128),
            name: name.into(),
        }
    }

    fn id(n: u128) -> Option<Uuid> {
        Some(Uuid::from_u128(n))
    }

    #[test]
    fn roots_and_paths() {
        let groups = vec![
            g(1, None, "Kunde B"),
            g(2, None, "Kunde A"),
            g(3, Some(2), "Standort 1"),
            g(4, Some(3), "Rack 7"),
        ];
        assert_eq!(root_of(&groups, Uuid::from_u128(4)), id(2));
        assert_eq!(root_of(&groups, Uuid::from_u128(2)), id(2));
        assert_eq!(root_of(&groups, Uuid::from_u128(99)), None);
        assert_eq!(
            path_of(&groups, Uuid::from_u128(4)),
            vec!["Kunde A", "Standort 1", "Rack 7"]
        );
        assert!(in_scope(&groups, id(4), id(3)));
        assert!(!in_scope(&groups, id(1), id(3)));
        assert!(!in_scope(&groups, None, id(3)));
        assert!(in_scope(&groups, None, None));
    }

    #[test]
    fn cycles_do_not_hang() {
        let groups = vec![g(1, Some(2), "a"), g(2, Some(1), "b")];
        assert!(root_of(&groups, Uuid::from_u128(1)).is_some());
        assert!(!in_scope(&groups, id(1), id(9)));
        assert!(path_of(&groups, Uuid::from_u128(1)).len() <= 3);
    }

    #[test]
    fn customer_list_and_grouping() {
        let groups = vec![
            g(1, None, "Kunde B"),
            g(2, None, "kunde a"),
            g(3, Some(2), "Standort 1"),
        ];
        // Servers: in a subgroup of A, in B, none, in A directly, in a deleted group.
        let servers = vec![id(3), id(1), None, id(2), id(77)];
        let list = customers(&groups, &servers);
        let names: Vec<(&str, usize)> = list.iter().map(|c| (c.name.as_str(), c.servers)).collect();
        assert_eq!(names, vec![("kunde a", 2), ("Kunde B", 1), ("", 2)]);
        assert_eq!(list[2].id, None);
        let grouped = group_servers(&groups, &servers);
        assert_eq!(grouped[0].1, vec![0, 3]);
        assert_eq!(grouped[1].1, vec![1]);
        assert_eq!(grouped[2].1, vec![2, 4]);
        assert!(matches(&groups, id(3), Sel::Customer(Uuid::from_u128(2))));
        assert!(!matches(&groups, id(1), Sel::Customer(Uuid::from_u128(2))));
        assert!(matches(&groups, id(77), Sel::Unassigned));
        assert!(matches(&groups, id(1), Sel::All));
    }

    #[test]
    fn stale_selection_resets() {
        let groups = vec![g(1, None, "A")];
        let list = customers(&groups, &[id(1)]);
        assert_eq!(
            normalize(Sel::Customer(Uuid::from_u128(1)), &list),
            Sel::Customer(Uuid::from_u128(1))
        );
        assert_eq!(
            normalize(Sel::Customer(Uuid::from_u128(2)), &list),
            Sel::All
        );
        assert_eq!(normalize(Sel::Unassigned, &list), Sel::All);
    }
}
