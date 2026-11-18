//! The authority answer this process can actually make.
//!
//! A registration names an authority reference; admitting it is an authority
//! owner's answer. The owner that exists here is the verified actor claim: the
//! shared facade checks the claim before any family port runs, so by the time
//! this directory is asked, "the caller holds this membership" is a fact the
//! process established rather than a claim the request restated.
//!
//! The directory therefore admits exactly one reference — the caller's own —
//! and refuses every other one. A caller cannot register a project under an
//! authority it does not hold, and this owner never invents a grant to make a
//! registration succeed.

use licoup_application::ActorClaim;
use licoup_project::{AuthorityReference, ProjectAuthorityDirectory, ProjectFailure};

/// The membership the verified claim acts as, as an authority reference.
#[derive(Clone, Debug)]
pub struct VerifiedClaimAuthority {
    reference: AuthorityReference,
}

impl VerifiedClaimAuthority {
    /// Read the authority of one already-verified claim.
    ///
    /// A claim that names no membership cannot authorize a durable identity, so
    /// it is refused rather than admitted under a placeholder.
    pub fn of(claim: &ActorClaim) -> Result<Self, ProjectFailure> {
        let membership_id = claim
            .membership_id()
            .ok_or_else(|| ProjectFailure::registration("project_authority_reference_required"))?;
        Ok(Self {
            reference: AuthorityReference::membership(membership_id)?,
        })
    }

    /// The one reference this caller holds.
    pub fn reference(&self) -> &AuthorityReference {
        &self.reference
    }
}

impl ProjectAuthorityDirectory for VerifiedClaimAuthority {
    fn admits(&self, reference: &AuthorityReference) -> bool {
        reference == &self.reference
    }
}
