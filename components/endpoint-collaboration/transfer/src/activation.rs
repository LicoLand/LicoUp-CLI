//! Why a transfer inventory can never activate an identity.
//!
//! Possession is not authority. An archive, an imported recovery package, a
//! provider backup and a saved recovery secret are all things a person *has*;
//! none of them is a signed transition that says this new endpoint may become
//! the subject's endpoint. This module is the transfer owner's half of that
//! rule: the only identity-facing value it can produce is a *requirement*, and
//! the producer of an authorized activation is deliberately somewhere else
//! (`REPLACEMENT-AUTHORITY-PORTS` owns the verified authority adapter).
//!
//! The guarantee is structural, not a policy check that could be forgotten:
//! there is no function in this crate that returns a prepared identity, an
//! endpoint key, or an activation, and [`IdentityActivationRequirement`] has no
//! variant that reports authority as satisfied.

use crate::inventory::CredentialCustody;

/// One reason the new device still needs its own authority.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ActivationReason {
    /// Holding the backup, the archive or the recovery secret proves nothing
    /// about the right to activate an endpoint identity.
    BackupPossessionIsNotAuthority,
    /// A provider key must be re-entered by the person.
    ProviderKeyReentry,
    /// A provider account must be signed in to again.
    TokenSignIn,
    /// Non-exportable material must be re-authorized for this endpoint.
    NonExportableReauthorization,
}

impl ActivationReason {
    /// The fixed, non-secret explanation reported to a person.
    #[must_use]
    pub const fn requirement(self) -> &'static str {
        match self {
            Self::BackupPossessionIsNotAuthority => {
                "having this backup does not authorize an identity: a verified authority transition is required"
            }
            Self::ProviderKeyReentry => CredentialCustody::ProviderKeyReentry.requirement(),
            Self::TokenSignIn => CredentialCustody::TokenSignIn.requirement(),
            Self::NonExportableReauthorization => {
                CredentialCustody::NonExportableReauthorization.requirement()
            }
        }
    }
}

/// What an inventory can say about identity: that fresh authority is required.
///
/// It is one statement with a non-empty reason set, not an outcome that could be
/// equal to "authorized". `BackupPossessionIsNotAuthority` is always present,
/// including for an inventory whose classification is complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityActivationRequirement {
    reasons: Vec<ActivationReason>,
}

impl IdentityActivationRequirement {
    /// The requirement for an inventory that needs no particular credential.
    #[must_use]
    pub fn for_inventory(custody: impl IntoIterator<Item = CredentialCustody>) -> Self {
        let mut reasons = vec![ActivationReason::BackupPossessionIsNotAuthority];
        for custody in custody {
            reasons.push(match custody {
                CredentialCustody::ProviderKeyReentry => ActivationReason::ProviderKeyReentry,
                CredentialCustody::TokenSignIn => ActivationReason::TokenSignIn,
                CredentialCustody::NonExportableReauthorization => {
                    ActivationReason::NonExportableReauthorization
                }
            });
        }
        reasons.sort();
        reasons.dedup();
        Self { reasons }
    }

    /// The reasons the new device still needs its own authority.
    #[must_use]
    pub fn reasons(&self) -> &[ActivationReason] {
        &self.reasons
    }

    /// Whether an inventory alone may activate an identity. Always `false`; the
    /// method exists so a caller reads the rule instead of assuming it, and so a
    /// regression that changed it would fail a test rather than a review.
    #[must_use]
    pub const fn permits_activation(&self) -> bool {
        false
    }
}
