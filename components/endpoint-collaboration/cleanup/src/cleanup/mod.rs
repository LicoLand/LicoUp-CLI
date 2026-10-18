//! The cleanup slice.
//!
//! One approved cleanup moves through ordered stages, each owned by the module
//! that performs its effect: the frozen target vocabulary ([`target`]), the
//! writers' quiescence and the file removals ([`file_owner`]), durable progress
//! ([`journal`]), the file stage itself ([`stage`]), and the restricted report
//! to the replacement endpoint ([`receipt`]).
//!
//! Credential deletion and terminal settlement are the platform custody
//! owner's own work — `licoup-native`'s mobile-relay secret custody owns the
//! authenticated replacement-endpoint authority, the bounded custody inventory
//! and the erase loop that walks it. This slice never deletes a credential and
//! never reads one; it reports those stages as outstanding until that owner
//! settles them.

mod file_owner;
mod journal;
mod receipt;
mod stage;
mod target;

#[cfg(test)]
mod tests;

pub use file_owner::{
    CleanupFileOwner, CleanupMaterialSettlement, FileStageOutcome, PrivateDataRootFileOwner,
    WriterQuiescence,
};
pub use journal::{
    CLEANUP_JOURNAL_FILE, CLEANUP_JOURNAL_SCHEMA, CLEANUP_STATE_DIRECTORY, CleanupJournal,
    CleanupJournalStore, CleanupStage, EntryOutcome, MAX_CLEANUP_JOURNAL_BYTES,
    MAX_CLEANUP_REASON_BYTES, RecordedEntry, StageEntryClass,
};
pub use receipt::{
    CleanupReceiptKind, CleanupReceiptPath, FileStageReceipt, MAX_CLEANUP_RECEIPT_BYTES,
    ReceiptDelivery, RestrictedReceiptEnvelope,
};
pub use stage::{FileStage, FileStageProgress, FileStageReport, PendingEntry};
pub use target::{
    CLEANUP_INVENTORY_LAYOUT, CleanupInventory, CleanupInventoryEntry, CleanupInventoryKind,
    CleanupSubject, CleanupTarget, DeviceId, MAX_CLEANUP_IDENTIFIER_BYTES,
    MAX_CLEANUP_INVENTORY_ENTRIES, OperationId, canonical_relative_posix,
};
