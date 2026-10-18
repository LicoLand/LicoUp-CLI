//! Local release-governance receipts. One candidate binds one target.
//! Tools record verified / not_run / authorization_required and never sign,
//! publish, or mutate a remote branch.

mod receipt;

pub use receipt::{
    ClosureStatus, LocalReleaseReceipt, ReceiptBinding, ReceiptCheck, ReceiptError,
    ReleaseTrainEdge,
};
