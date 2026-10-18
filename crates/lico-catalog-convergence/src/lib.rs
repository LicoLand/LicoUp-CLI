//! Portable catalog convergence engine shared by LicoUp desktop and mobile targets.
//!
//! Target adapters must depend on this crate for revision, cache, cohort, and receipt
//! policy. Platform bridges must not reimplement those rules.

mod dispatch;
mod engine;
mod model;
mod receipt;
mod store;

pub use dispatch::{dispatch, dispatch_with_engine};
pub use engine::CatalogConvergenceEngine;
pub use model::{
    ALLOWED_CLIENT_TARGETS, CATALOG_CONVERGENCE_SCHEMA, CatalogFetchedSnapshot, CatalogPullContext,
    CatalogSnapshot, CatalogToolEntry, ClientTarget, CohortEntry, CohortOutcome, DiscoveryResult,
    InvalidationNotification, InvalidationResult, OFFICIAL_CLIENT_RECEIPT_SCHEMA,
    OfficialClientReceipt, OutcomeRecord, PendingInvalidation, RefreshOutcomeKind, RefreshResult,
    digest_catalog_snapshot, is_hex_digest, is_opaque_partition_key, sha256_hex,
};
pub use receipt::{
    ReceiptContext, build_official_client_receipt, build_receipt_digest, build_summary_digest,
    scan_privacy_text, scan_privacy_value,
};
pub use store::CatalogCacheStore;

#[cfg(test)]
mod tests;
