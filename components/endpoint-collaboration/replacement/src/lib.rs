//! Source retirement for `org.licoland.feature.collaboration`.
//!
//! This is the consumer side of a transfer. It answers exactly one question —
//! may the source device's own data be retired or erased? — and it answers it
//! only from [`VerifiedTargetEvidence`], never from a status flag, a progress
//! percentage or an elapsed time.
//!
//! Two rules the owner keeps:
//!
//! * **A failed or abandoned transfer preserves the source.** The ordinary
//!   outcome of an interrupted replacement is [`RetirementDecision::Refused`], and
//!   a refusal is what keeps the source available for a later attempt.
//! * **Lost activation is independent.** When the source device is gone, a new
//!   device is activated on accepted fresh authority. It needs no history, no
//!   archive and no acknowledgement from the old device, and unsynchronized
//!   content is reported as unavailable rather than reconstructed.
//!
//! Nothing in this crate erases anything by itself. Admitting retirement is a
//! decision a separately authorized cleanup flow acts on; this crate never
//! performs a wipe, never deletes a credential and never touches a device
//! identity.

use licoup_endpoint_collaboration_transfer::{
    LostActivationPath, RetirementEligibility, RetirementRefusal, TargetBinding,
    VerifiedTargetEvidence,
};

/// What a source-retirement flow is allowed to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetirementDecision {
    /// A staged target verified every required owner, for this exact binding.
    Admitted { evidence: VerifiedTargetEvidence },
    /// The source stays exactly where it is.
    Refused(RetirementRefusal),
}

impl RetirementDecision {
    /// Decide whether the source device of `subject` may be retired.
    ///
    /// The caller names the source it is about to retire; evidence that another
    /// subject or another source device verified is refused, so a verified target
    /// cannot be spent on a source it does not belong to.
    #[must_use]
    pub fn decide_for(
        evidence: Option<VerifiedTargetEvidence>,
        subject: &str,
        source_device: &str,
    ) -> Self {
        match RetirementEligibility::decide_for(evidence, subject, source_device) {
            RetirementEligibility::Eligible(evidence) => Self::Admitted { evidence },
            RetirementEligibility::Refused(refusal) => Self::Refused(refusal),
        }
    }

    #[must_use]
    pub const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted { .. })
    }

    /// The fixed, non-secret explanation of this decision.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Admitted { .. } => {
                "every required owner verified on the target, so the source may be retired by its own authorized flow"
            }
            Self::Refused(refusal) => refusal.reason(),
        }
    }

    /// The binding the admitted retirement belongs to.
    #[must_use]
    pub fn binding(&self) -> Option<&TargetBinding> {
        match self {
            Self::Admitted { evidence } => Some(evidence.binding()),
            Self::Refused(_) => None,
        }
    }
}

/// A lost-device activation, kept separate from any transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LostDeviceActivation {
    path: LostActivationPath,
}

impl LostDeviceActivation {
    /// Activate a lost or offline device on accepted authority.
    ///
    /// It takes no evidence, no archive and no provider acknowledgement, because
    /// none of them may be required: the old device is gone. Whatever it could not
    /// hand over is reported as unavailable.
    #[must_use]
    pub fn from_accepted_authority(path: LostActivationPath) -> Self {
        Self { path }
    }

    /// The fresh authority this activation needs.
    #[must_use]
    pub fn requirement(&self) -> &licoup_endpoint_collaboration_transfer::IdentityActivationRequirement {
        &self.path.requirement
    }

    /// Content that no surviving source holds. It stays unavailable; it is
    /// reported, never fabricated.
    #[must_use]
    pub fn unavailable_content(&self) -> &[String] {
        &self.path.unavailable_content
    }

    /// Whether this activation waits for a transfer. It never does.
    #[must_use]
    pub const fn waits_for_transfer(&self) -> bool {
        self.path.depends_on_transfer()
    }
}
