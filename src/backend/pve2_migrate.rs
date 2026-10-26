//! Planning of a migration of many guests at once: which guest goes to which node,
//! which ones are skipped (and why) and the API spec of each one that runs. Pure logic;
//! the batch runner starts the specs as ordinary tasks.

use std::collections::HashMap;

use uuid::Uuid;

use super::super::{Kind, MigrateSpec, valid_node};

/// Most migrations running at the same time.
pub const MAX_PARALLEL: usize = 4;

/// A selected guest as far as planning cares.
#[derive(Clone, Debug, PartialEq)]
pub struct MigGuest {
    pub server: Uuid,
    pub vmid: u32,
    pub name: String,
    pub node: String,
    pub kind: Kind,
    pub running: bool,
    pub template: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Running VMs migrate live, running containers restart on the target.
    #[default]
    Live,
    /// Only guests that are off; running guests are skipped (never stopped for you).
    Offline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub mode: Mode,
    /// Take local disks along (VMs; containers move their volumes anyway).
    pub local_disks: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    NoTarget,
    TargetOffline,
    AlreadyThere,
    RunningOffline,
    Template,
}

impl Skip {
    pub fn text(self) -> &'static str {
        match self {
            Skip::NoTarget => t!("kein Ziel-Node gewählt"),
            Skip::TargetOffline => t!("Ziel-Node nicht online"),
            Skip::AlreadyThere => t!("liegt schon auf dem Ziel-Node"),
            Skip::RunningOffline => t!("läuft – im Offline-Modus übersprungen"),
            Skip::Template => t!("Vorlage"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Run(MigrateSpec),
    Skip(Skip),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub items: Vec<(MigGuest, Outcome)>,
}

impl Plan {
    pub fn runs(&self) -> usize {
        self.items
            .iter()
            .filter(|(_, o)| matches!(o, Outcome::Run(_)))
            .count()
    }

    pub fn skips(&self) -> usize {
        self.items.len() - self.runs()
    }

    /// Guests that run and are running now (live / restart migrations).
    pub fn live(&self) -> usize {
        self.items
            .iter()
            .filter(|(g, o)| g.running && matches!(o, Outcome::Run(_)))
            .count()
    }

    /// Containers that will be restarted.
    pub fn restarts(&self) -> usize {
        self.items
            .iter()
            .filter(|(g, o)| g.running && g.kind == Kind::Lxc && matches!(o, Outcome::Run(_)))
            .count()
    }
}

pub fn clamp_parallel(n: usize) -> usize {
    n.clamp(1, MAX_PARALLEL)
}

/// `targets`: server → target node; `online`: server → online nodes.
pub fn plan(
    guests: &[MigGuest],
    targets: &HashMap<Uuid, String>,
    online: &HashMap<Uuid, Vec<String>>,
    opts: Options,
) -> Plan {
    let items = guests
        .iter()
        .map(|g| {
            let outcome = (|| {
                if g.template {
                    return Outcome::Skip(Skip::Template);
                }
                let Some(target) = targets.get(&g.server).filter(|t| valid_node(t)) else {
                    return Outcome::Skip(Skip::NoTarget);
                };
                if *target == g.node {
                    return Outcome::Skip(Skip::AlreadyThere);
                }
                if !online
                    .get(&g.server)
                    .is_some_and(|n| n.iter().any(|x| x == target))
                {
                    return Outcome::Skip(Skip::TargetOffline);
                }
                if opts.mode == Mode::Offline && g.running {
                    return Outcome::Skip(Skip::RunningOffline);
                }
                Outcome::Run(MigrateSpec {
                    target: target.clone(),
                    running: g.running,
                    storage: None,
                    local_disks: opts.local_disks && g.kind == Kind::Qemu,
                })
            })();
            (g.clone(), outcome)
        })
        .collect();
    Plan { items }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest(server: u128, vmid: u32, node: &str, kind: Kind, running: bool) -> MigGuest {
        MigGuest {
            server: Uuid::from_u128(server),
            vmid,
            name: format!("g{vmid}"),
            node: node.into(),
            kind,
            running,
            template: false,
        }
    }

    #[test]
    fn plans_per_server() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let guests = vec![
            guest(1, 100, "pve1", Kind::Qemu, true),
            guest(1, 101, "pve1", Kind::Lxc, true),
            guest(1, 102, "pve2", Kind::Qemu, false),
            guest(2, 200, "n1", Kind::Qemu, false),
            MigGuest {
                template: true,
                ..guest(1, 103, "pve1", Kind::Qemu, false)
            },
        ];
        let targets = HashMap::from([(a, "pve2".to_string())]);
        let online = HashMap::from([
            (a, vec!["pve1".to_string(), "pve2".to_string()]),
            (b, vec!["n1".to_string()]),
        ]);
        let p = plan(&guests, &targets, &online, Options::default());
        assert_eq!(p.runs(), 2);
        assert_eq!(p.skips(), 3);
        assert_eq!(p.live(), 2);
        assert_eq!(p.restarts(), 1);
        assert_eq!(p.items[2].1, Outcome::Skip(Skip::AlreadyThere));
        assert_eq!(p.items[3].1, Outcome::Skip(Skip::NoTarget));
        assert_eq!(p.items[4].1, Outcome::Skip(Skip::Template));
        match &p.items[0].1 {
            Outcome::Run(s) => assert!(s.running && s.target == "pve2" && !s.local_disks),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn offline_and_local_disks() {
        let a = Uuid::from_u128(1);
        let guests = vec![
            guest(1, 100, "pve1", Kind::Qemu, true),
            guest(1, 101, "pve1", Kind::Qemu, false),
            guest(1, 102, "pve1", Kind::Lxc, false),
        ];
        let targets = HashMap::from([(a, "pve2".to_string())]);
        let online = HashMap::from([(a, vec!["pve2".to_string()])]);
        let p = plan(
            &guests,
            &targets,
            &online,
            Options {
                mode: Mode::Offline,
                local_disks: true,
            },
        );
        assert_eq!(p.items[0].1, Outcome::Skip(Skip::RunningOffline));
        match (&p.items[1].1, &p.items[2].1) {
            (Outcome::Run(vm), Outcome::Run(ct)) => {
                assert!(vm.local_disks && !vm.running);
                assert!(!ct.local_disks);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn bad_or_offline_targets() {
        let a = Uuid::from_u128(1);
        let guests = vec![guest(1, 100, "pve1", Kind::Qemu, false)];
        let online = HashMap::from([(a, vec!["pve2".to_string()])]);
        for bad in ["pve3", "pve2;reboot", ""] {
            let p = plan(
                &guests,
                &HashMap::from([(a, bad.to_string())]),
                &online,
                Options::default(),
            );
            assert_eq!(p.runs(), 0, "{bad}");
        }
        assert_eq!(clamp_parallel(0), 1);
        assert_eq!(clamp_parallel(99), MAX_PARALLEL);
    }
}
