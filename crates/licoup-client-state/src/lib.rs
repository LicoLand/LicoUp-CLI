//! Bounded client persistence types and the collection store that writes them.
//!
//! `ClientStateStore` owns the privately written collection documents under the
//! portable state root, including the target-discovery cache and its typed
//! route records; `paths`, `policy` and `serialization` are its location, bound
//! and write rules. `ClientResourcePolicy` is the single configuration surface
//! for history pages, parser buffers, search results, archive workers, spools,
//! log segments, state quotas, and maintenance batches.
//!
//! The journal owners above the store (`ActivityLog`, `SnapshotStore`) still
//! live in `licoup-native` and read the store's rules from here through their
//! former paths. Nothing here reaches upward: the Agent inventory depends on
//! this crate rather than on the host that serves requests.

mod accessors;
pub mod activity;
pub mod collections;
pub mod migration;
pub mod paths;
pub mod policy;
pub mod redaction;
mod resource_policy;
pub mod serialization;
pub mod snapshots;

pub use activity::ActivityLog;
pub use collections::{
    ClientStateStore, TARGET_DISCOVERY_CACHE_COLLECTION, TARGET_DISCOVERY_CACHE_SCHEMA,
    TargetRouteRecord,
};
pub use migration::{migrate_collections, probe_collections};
pub use resource_policy::{ClientResourceBounds, ClientResourcePolicy};
pub use snapshots::{SnapshotRecord, SnapshotStore};

#[cfg(test)]
mod tests;
