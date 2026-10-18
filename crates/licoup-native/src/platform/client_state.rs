// The generic local persistence owner now lives in `licoup-client-state`. It is
// re-exported here at its former paths and former visibility, so the journal
// callers in this crate and the Agent inventory's discovery cache name one
// implementation. What stays is the command surface: `operations` is the only
// module in this directory that depends on the generated wire contract.
mod operations;

pub use licoup_client_state::{ActivityLog, ClientStateStore, SnapshotRecord, SnapshotStore};
pub use licoup_client_state::{migrate_collections, probe_collections};
pub use operations::{activity_list, snapshots_list, snapshots_restore, state_get, state_set};

// The store's own rules stay reachable at the module paths this crate used
// before the move.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use licoup_client_state::TARGET_DISCOVERY_CACHE_COLLECTION;
#[allow(unused_imports)]
pub(crate) use licoup_client_state::{TARGET_DISCOVERY_CACHE_SCHEMA, TargetRouteRecord};
pub(crate) use licoup_client_state::{collections, paths};
pub(crate) use paths::state_root_for_data_home;

#[cfg(test)]
mod tests;
