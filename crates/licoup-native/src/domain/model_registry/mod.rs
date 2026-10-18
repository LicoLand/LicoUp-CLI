//! The native paths the canonical model identities' callers already use.
//!
//! The registry itself moved to `licoup-model-catalog`, which owns declared
//! canonical identity, the provider and historical aliases, the private catalog
//! cache and the refresh of the public source. Every former path stays
//! reachable through this re-export for the consumers that still live in
//! `licoup-native`: the FFI command layer, the usage projection, the workflow
//! runtime and the Agent inventory port this host composes.
//!
//! Identity is a declaration, not entitlement: this module supplies which model
//! a selector names. Observed availability is a separate fact, and the probe
//! that produces it arrives through the catalogue port the crate root composes.
//!
//! A native unit test states its expectations against a synthetic snapshot. The
//! seam stays here rather than in the catalogue because the two disagree about
//! what a test build is: `cfg(test)` is true for this crate's own test build and
//! false for a dependency, and it must stay false for an integration test,
//! which exercises the real file-backed cache on purpose.

#[cfg(not(test))]
pub use licoup_model_catalog::identity::{
    CanonicalModel, RegistrySnapshot, model_display_name, read, refresh, refresh_cached_snapshot,
    refresh_cached_snapshot_for_state_root, snapshot,
};

#[cfg(test)]
pub use licoup_model_catalog::identity::{CanonicalModel, RegistrySnapshot, model_display_name};

#[cfg(test)]
use licoup_model_catalog::identity::{SnapshotProvenance, snapshot_report};
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    static TEST_SNAPSHOT: std::cell::RefCell<Arc<RegistrySnapshot>> =
        std::cell::RefCell::new(Arc::new(RegistrySnapshot::empty()));
}

#[cfg(test)]
fn test_snapshot() -> Arc<RegistrySnapshot> {
    TEST_SNAPSHOT.with(|snapshot| Arc::clone(&snapshot.borrow()))
}

/// No file or network work. Capture one Arc at each report boundary.
#[cfg(test)]
pub fn snapshot() -> Arc<RegistrySnapshot> {
    test_snapshot()
}

/// The synthetic snapshot is the current one for the rest of the call scope.
#[cfg(test)]
pub fn refresh_cached_snapshot() -> Arc<RegistrySnapshot> {
    test_snapshot()
}

/// A synthetic test build reads no state root: the same snapshot answers every
/// request, which is what makes an isolated-store assertion meaningful.
#[cfg(test)]
pub fn refresh_cached_snapshot_for_state_root(_state_root: Option<&Path>) -> Arc<RegistrySnapshot> {
    test_snapshot()
}

/// The report a test build reads describes its own snapshot and states no
/// provenance, because no source was read.
#[cfg(test)]
pub fn read() -> serde_json::Value {
    snapshot_report(
        &test_snapshot(),
        true,
        "ready",
        &SnapshotProvenance::default(),
        None,
    )
}

/// An explicit refresh is a public-network operation; a test build reports the
/// unchanged synthetic snapshot instead of performing one.
#[cfg(test)]
pub fn refresh() -> serde_json::Value {
    read()
}

/// Run one closure against a synthetic snapshot, restoring the previous one
/// afterwards.
#[cfg(test)]
pub fn with_test_snapshot<T>(snapshot: RegistrySnapshot, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<RegistrySnapshot>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                TEST_SNAPSHOT.with(|snapshot| {
                    snapshot.replace(previous);
                });
            }
        }
    }
    let previous = TEST_SNAPSHOT.with(|current| current.replace(Arc::new(snapshot)));
    let _restore = Restore(Some(previous));
    action()
}
