//! Device-transfer ownership inventory for `org.licoland.feature.collaboration`.
//!
//! A device transfer starts from one real full-data-root archive. This component
//! says what that archive actually carries for a transfer: which areas are
//! managed payloads restored by their owners, which are external references that
//! stay with their own owner, and which are nonportable credentials the person
//! must re-establish on the new device. It reports limitations without values
//! and cannot activate an identity — see [`activation`].
//!
//! It composes the existing owners instead of copying them:
//! `licoup_foundation::core::full_data_root_archive` owns the data-root walk,
//! the archive layout, the portable-path grammar and the credential-custody
//! limitation this component classifies; `licoup_client_state` owns the
//! client-state location names the declared table uses.

pub mod activation;
pub mod inventory;
pub mod ownership;
pub mod verified_target;

pub use activation::{ActivationReason, IdentityActivationRequirement};
pub use inventory::{
    CredentialCustody, CredentialRequirement, ExternalOwner, ExternalReference,
    InventoryCompleteness, LimitationKind, ManagedDomain, ManagedPayload, TransferInventory,
    TransferLimitation, UnattributedPath, UnexplainedLimitation, classify_entries,
};
pub use ownership::TransferOwnership;
pub use verified_target::{
    LostActivationPath, RequiredOwner, RetirementEligibility, RetirementRefusal, TargetBinding,
    VerifiedTargetEvidence,
};
