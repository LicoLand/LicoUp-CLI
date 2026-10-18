//! Verified-target evidence, and the retirement gate it opens.
//!
//! A replacement commits in two separate steps. First the destination is staged
//! and every owner verifies; that produces [`VerifiedTargetEvidence`], a typed
//! record of *which* owners verified for *which* subject, source device and new
//! identity. Only then may a separately authorized flow retire or erase the
//! source.
//!
//! The evidence is deliberately not a boolean a UI can set. It carries the
//! settled per-owner verification of one staged recovery, it has no public
//! constructor, it cannot be deserialized from a payload, and it is refused when
//! any required owner is missing. A caller that has no evidence gets
//! [`RetirementEligibility::Refused`] — there is no path from "the transfer looked
//! finished" to a source erase. The question is always asked about one named
//! subject and source device, so evidence for another source is refused too.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::activation::IdentityActivationRequirement;

/// The owners a replacement target had to verify, in the order they verify.
///
/// This mirrors the native recovery owner order; the transfer component states it
/// again so a consumer of the evidence does not have to depend on the native
/// crate's internals to read a verified-target record.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequiredOwner {
    /// The extracted payload staged on the destination filesystem.
    FilesystemPayload,
    /// The database stores opened and read back from the staged payload.
    DatabaseStores,
    /// The credential material this device's own custody holds.
    PlatformCredentials,
}

impl RequiredOwner {
    pub const ALL: [Self; 3] = [
        Self::FilesystemPayload,
        Self::DatabaseStores,
        Self::PlatformCredentials,
    ];
}

/// Which subject, source device and new identity one verified target belongs to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetBinding {
    /// The subject the replacement belongs to, as its authority identifies it.
    pub subject: String,
    /// The source device the payload came from.
    pub source_device: String,
    /// The new endpoint identity this device activated. It is never derived from
    /// the source identity.
    pub target_identity: String,
}

/// Settled evidence that one staged target verified every required owner.
///
/// There is no public constructor: the only way to obtain one is
/// [`VerifiedTargetEvidence::from_settled_owners`], which refuses when any
/// required owner is missing. A UI flag, a progress percentage or a successful
/// copy therefore cannot stand in for it.
///
/// It is deliberately **not** deserializable. A target report crosses the client
/// bridge as [`Serialize`] output — a person may be shown which owners verified —
/// but a payload can never be turned back into evidence, because rehydrating the
/// capability from data the client controls would make the retirement gate no
/// stronger than a UI boolean. Evidence is produced in the process that staged the
/// target, and only there.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedTargetEvidence {
    binding: TargetBinding,
    verified: BTreeSet<RequiredOwner>,
}

impl VerifiedTargetEvidence {
    /// Build evidence from the owners a staged recovery actually settled.
    ///
    /// Returns `None` when any required owner is absent: an incomplete target
    /// produces no evidence at all rather than partial evidence a caller might
    /// treat as good enough.
    pub fn from_settled_owners(
        binding: TargetBinding,
        settled: impl IntoIterator<Item = RequiredOwner>,
    ) -> Option<Self> {
        let verified = settled.into_iter().collect::<BTreeSet<_>>();
        if RequiredOwner::ALL
            .into_iter()
            .any(|owner| !verified.contains(&owner))
        {
            return None;
        }
        Some(Self { binding, verified })
    }

    #[must_use]
    pub fn binding(&self) -> &TargetBinding {
        &self.binding
    }

    /// The owners this evidence covers, which is always every required owner.
    #[must_use]
    pub fn verified_owners(&self) -> impl Iterator<Item = RequiredOwner> + '_ {
        self.verified.iter().copied()
    }

    /// Whether this evidence authorizes retiring the source device.
    ///
    /// It does: the evidence exists only for a fully verified target. The
    /// *retirement action* is still separately authorized — see
    /// `licoup_endpoint_collaboration_replacement`.
    #[must_use]
    pub const fn admits_source_retirement(&self) -> bool {
        true
    }
}

/// Why the source may not be retired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetirementRefusal {
    /// No staged target verified, so there is nothing to retire the source for.
    NoVerifiedTarget,
    /// A target verified, but not for the subject and source device whose
    /// retirement was asked about. Retiring the wrong source is not admitted.
    BindingMismatch,
}

impl RetirementRefusal {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoVerifiedTarget => {
                "the source is preserved: no target has verified every required owner"
            }
            Self::BindingMismatch => {
                "the source is preserved: the verified target belongs to another subject or source device"
            }
        }
    }
}

/// The decision a source-retirement or erase flow is allowed to act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetirementEligibility {
    Eligible(VerifiedTargetEvidence),
    Refused(RetirementRefusal),
}

impl RetirementEligibility {
    /// Decide whether *this* subject's source device may be retired.
    ///
    /// The caller states which source it is about to retire, so evidence that a
    /// different subject or a different source device verified is a refusal
    /// rather than an admission. `None` is the ordinary case after a failed,
    /// interrupted or abandoned transfer, and it refuses too. There is no third
    /// outcome and no way to decide without naming the source.
    #[must_use]
    pub fn decide_for(
        evidence: Option<VerifiedTargetEvidence>,
        subject: &str,
        source_device: &str,
    ) -> Self {
        match evidence {
            Some(evidence)
                if evidence.binding().subject == subject
                    && evidence.binding().source_device == source_device =>
            {
                Self::Eligible(evidence)
            }
            Some(_) => Self::Refused(RetirementRefusal::BindingMismatch),
            None => Self::Refused(RetirementRefusal::NoVerifiedTarget),
        }
    }

    #[must_use]
    pub const fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible(_))
    }

    /// The evidence, when the source may be retired.
    #[must_use]
    pub fn evidence(&self) -> Option<&VerifiedTargetEvidence> {
        match self {
            Self::Eligible(evidence) => Some(evidence),
            Self::Refused(_) => None,
        }
    }
}

/// What independently authorized lost activation needs.
///
/// It is a statement of the fresh authority a *new* device needs when the old one
/// is gone. It deliberately carries no transfer, no history and no archive: a
/// lost-device activation is possible without them, and unsynchronized content is
/// reported as unavailable rather than reconstructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LostActivationPath {
    pub requirement: IdentityActivationRequirement,
    /// Content the old device had that no surviving copy holds. It stays
    /// unavailable; nothing here recreates it.
    pub unavailable_content: Vec<String>,
}

impl LostActivationPath {
    #[must_use]
    pub fn new(requirement: IdentityActivationRequirement) -> Self {
        Self {
            requirement,
            unavailable_content: Vec::new(),
        }
    }

    /// Records content the old device had and no available source holds.
    pub fn report_unavailable(&mut self, domain: &str) {
        if !self.unavailable_content.iter().any(|seen| seen == domain) {
            self.unavailable_content.push(domain.to_string());
            self.unavailable_content.sort();
        }
    }

    /// Whether this path depends on a transfer, an archive or retained history.
    /// It never does.
    #[must_use]
    pub const fn depends_on_transfer(&self) -> bool {
        false
    }
}
