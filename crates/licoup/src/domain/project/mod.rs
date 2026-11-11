//! Authorized project and plan identity: the native composition.
//!
//! `licoup-project` owns the identity model and the durable record. This module
//! owns the two facts the crate cannot know by itself: where the portable data
//! root is, and which authority the caller of a request actually holds. Both
//! answers come from owners that already exist — the foundation's data-root
//! locator and the verified actor claim the shared facade established before the
//! project port ran.
//!
//! Nothing here reads a filesystem to name a project. The store's location is
//! the only path this module opens, and the authorized root each registration
//! declares is stored as the declaration it is.

mod authority;
mod port;
mod schedule;
#[cfg(test)]
mod tests;

pub use authority::VerifiedClaimAuthority;
pub use port::NativeProjectApplication;
pub use schedule::{NativeProjectWorkDirectory, ProjectWorkDirectory};

use licoup_foundation::platform::paths::portable_data_dir;
use licoup_project::{ProjectFailure, ProjectIdentityStore};
use std::path::Path;

/// Open the project identity store at one portable data root.
pub fn store_at(portable_root: &Path) -> Result<ProjectIdentityStore, ProjectFailure> {
    ProjectIdentityStore::open(portable_root)
}

/// Open the project identity store this process composes: the selected portable
/// data root, through the locator every other owner already uses.
pub fn portable_store() -> Result<ProjectIdentityStore, ProjectFailure> {
    let root = portable_data_dir().map_err(|error| {
        ProjectFailure::store("project_identity_store_unavailable").with_detail(error.to_string())
    })?;
    store_at(&root)
}
