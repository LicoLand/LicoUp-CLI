//! Provider-neutral retained-history backup and recovery.
//!
//! Provider authentication, vendor APIs, credentials, and UI integration are
//! deliberately supplied outside this module. The store port is usable only
//! after its caller has obtained provider authorization.

#[path = "../conversation/history_backup_catalog.rs"]
pub mod catalog;
mod policy;
mod recovery;
mod recovery_material;
mod store;

pub use policy::{
    HistoryKeySource, ObjectOpenError, StorageMode, StoredHistoryObject, StoredObjectHeader,
    open_history_object, seal_history_object,
};
pub use recovery::{
    FreshSessionRequirement, RecoveredHistoryObject, RecoveryError, RecoveryOutcome, RecoveryOwner,
    RecoveryProgress, RetainedContentDecoder, StagedRecoveryTarget, recover_replacement_endpoint,
    visit_authorized_history,
};
pub use recovery_material::{
    RecoveryMaterial, RecoveryMaterialError, RecoveryPackage, RecoverySecret,
};
pub use store::{
    AuthorizedHistoryStore, ContentKind, DeviceId, HistoryStoreError, InventoryPage,
    InventoryVersion, ManifestEntry, ManifestSegment, ObjectId, SegmentId, append_backup_segment,
};

#[cfg(test)]
#[path = "tests.rs"]
mod trusted_history;
