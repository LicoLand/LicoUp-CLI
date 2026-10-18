//! Discovery-descriptor knowledge client. No copied schema, no live calls.

use licoup_conversation::continuity::{
    ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage, ContinuitySourceOwnerKind,
    ContinuitySourceRef, ContinuitySourceValidity, ContinuityVisibilityScope,
    DiscoveredKnowledgePort,
};

use super::types::continuity_failure;

/// Advertised capability descriptor. Holds a digest of the remote input
/// contract, not a copy of that schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeDiscoveryDescriptor {
    pub capability: String,
    pub advertised_version: String,
    pub input_contract_digest: String,
    pub disclosure_class: String,
    pub available: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UnavailableKnowledgeService {
    descriptors: Vec<KnowledgeDiscoveryDescriptor>,
}

impl UnavailableKnowledgeService {
    pub fn from_descriptors(descriptors: Vec<KnowledgeDiscoveryDescriptor>) -> Self {
        Self { descriptors }
    }

    pub fn descriptors(&self) -> &[KnowledgeDiscoveryDescriptor] {
        &self.descriptors
    }
}

impl DiscoveredKnowledgePort for UnavailableKnowledgeService {
    fn lookup(&self, capability: &str) -> Result<ContinuitySourceRef, ContinuityFailure> {
        let Some(descriptor) = self
            .descriptors
            .iter()
            .find(|item| item.capability == capability)
        else {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        };
        if !descriptor.available {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        Ok(ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Knowledge,
            opaque_id: descriptor.capability.clone(),
            part_id: Some(descriptor.advertised_version.clone()),
            span: None,
            source_revision: 1,
            digest: descriptor.input_contract_digest.clone(),
            visibility_scope: ContinuityVisibilityScope::Conversation,
            validity: ContinuitySourceValidity::Current,
        })
    }
}
