//! Proxmox extras for managed-service use: customers (groups) for servers, a guest
//! query language, multi-guest migration planning, cloud-init editing and the PBS
//! archive browser / prune / verify / garbage collection. Everything here is either
//! pure logic (unit tested) or a small blocking API job; path segments are validated
//! before they go into a URL and no secrets are logged.

#[path = "pve2_ci.rs"]
pub mod ci;
#[path = "pve2_customer.rs"]
pub mod customer;
#[path = "pve2_migrate.rs"]
pub mod migrate;
#[path = "pve2_pbs.rs"]
pub mod pbs;
#[path = "pve2_query.rs"]
pub mod query;
