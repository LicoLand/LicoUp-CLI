//! Bounded, recoverable app-data removal for one revoked LicoUp endpoint.
//!
//! This package is the cleanup slice of the endpoint collaboration package
//! (`org.licoland.feature.collaboration`). It owns the *file half* of a staged
//! erase — close admission, settle the frozen file inventory entry by entry
//! with durable progress, and report a partial result — and it composes the
//! owners that perform each effect instead of reimplementing them:
//!
//! * [`licoup_foundation`] owns the data home, the bounded private-file
//!   primitives, the data-root admission lock and the data-root inventory
//!   grammar.
//! * `licoup-native` owns platform credential custody, the authenticated
//!   replacement-endpoint authority, its presence authorization and its
//!   capability facts. Credential deletion and terminal settlement stay there;
//!   this package reports those stages as outstanding.
//! * `licoup-endpoint-core` owns the closed endpoint session and operation
//!   policy enums and the transport port.
//!
//! Nothing here authenticates a caller with a cleanup-local scheme, reads a
//! protected key for any purpose, or promises forensic or operating-system-
//! backup erasure.

pub mod cleanup;

pub use cleanup::{
    CLEANUP_INVENTORY_LAYOUT, CLEANUP_JOURNAL_FILE, CLEANUP_JOURNAL_SCHEMA,
    CLEANUP_STATE_DIRECTORY, CleanupFileOwner, CleanupInventory, CleanupInventoryEntry,
    CleanupInventoryKind, CleanupJournal, CleanupJournalStore, CleanupMaterialSettlement,
    CleanupReceiptKind, CleanupReceiptPath, CleanupStage, CleanupSubject, CleanupTarget, DeviceId,
    EntryOutcome, FileStage, FileStageOutcome, FileStageProgress, FileStageReceipt,
    FileStageReport, OperationId, PendingEntry, PrivateDataRootFileOwner, ReceiptDelivery,
    RestrictedReceiptEnvelope, StageEntryClass, WriterQuiescence, canonical_relative_posix,
};
