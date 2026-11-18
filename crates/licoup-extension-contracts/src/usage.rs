//! C11: usage observations and specialist metrics.
//!
//! The observation vocabulary, its exact-decimal arithmetic and the C11
//! refusal rules are owned by the published `token-usage-detector` crate. This
//! module keeps the contract path extensions already import
//! (`licoup_extension_contracts::usage`) as one re-export, so the host and the
//! detector read the same types rather than a second copy.
//!
//! The rules that shape the vocabulary — a source never names its own binding,
//! a correction is a new revision rather than an offset, and an unknown count
//! is absent rather than zero — are documented and enforced in the owner.
//!
//! A refusal is the owner's [`token_usage_detector::UsageFailure`], not a second
//! failure shape. A consumer that reports through the application vocabulary
//! projects it at its own boundary, keeping the stable code, stage and field;
//! the optional analytics package is that projection.

/// The refusal type every C11 validator in the owner returns.
pub use token_usage_detector::UsageFailure;
pub use token_usage_detector::usage::*;

#[cfg(test)]
mod tests {
    /// The contract's published identifier and the owner's identifier are one
    /// value: a payload accepted at the crate path is the payload the owner's
    /// validator accepts.
    #[test]
    fn the_reexport_and_the_owner_publish_one_wire_schema() {
        assert_eq!(crate::wire::USAGE, token_usage_detector::usage::wire::USAGE);
    }
}
