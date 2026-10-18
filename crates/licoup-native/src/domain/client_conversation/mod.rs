//! Native host composition for the independent Canonical Conversation crate.
//!
//! Durable records and SQLite state are owned by `licoup-conversation`; this
//! module retains only host-specific migration, snapshot authorities, runtime
//! closures, and the stable FFI-facing re-export.

mod migration;
pub mod peer_ingress;
mod profile_snapshot;
pub(crate) mod projection_delta;
pub mod selection_policy;
mod service;
#[allow(hidden_glob_reexports)]
mod store;

pub use licoup_conversation::*;
pub use migration::{MigrationReport, migrate_legacy_state};
pub use profile_snapshot::{
    CandidateFilters, PriceFacts, ProfileSnapshotAuthority, SharedSnapshotAuthority, TargetFacts,
    production_snapshot_authority, project_profile_snapshot, project_profile_snapshots,
    rank_candidates,
};
pub use selection_policy::{
    SELECTION_POLICY_SETTINGS_KEY, SelectionPolicyBinding, SelectionPolicyFailure,
    SelectionPolicyPreferences, SelectionPolicyRegister, SelectionPolicyRevision,
    UNADOPTED_REVISION, adopt_policy, current_binding, revoke_policy, supersede_policy,
};
#[cfg(test)]
pub(crate) use service::route_receipt_under;
pub use service::{ConversationService, PersistentRuntimePorts, dispatch_attachments_param};

/// Product-owned private dispatch guidance remains composed by the native host
/// and is never written into Conversation Event text.
pub(crate) const LICOUP_GUIDE_SKILL_SOURCE: &str =
    include_str!("../../../resources/licoup-guide/SKILL.md");
