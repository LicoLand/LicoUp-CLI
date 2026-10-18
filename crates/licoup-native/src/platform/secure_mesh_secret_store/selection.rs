use std::sync::Arc;

use anyhow::Result;

use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvidenceKind, CapabilityFact, CapabilityFactState,
    CustodyRestartSemantics, SecretCustodyStrategy, SecurityCapability, capability_catalog,
    mandatory_protocol_facts,
};
use crate::core::secure_mesh_secret_store::SecureMeshSecretStore;

use super::ephemeral::EphemeralSecretStore;

pub struct SecureMeshSecretStoreSelection {
    store: Arc<dyn SecureMeshSecretStore>,
    capability_evaluation: CapabilityEvaluation,
}

impl SecureMeshSecretStoreSelection {
    pub fn select(os_store: Option<Arc<dyn SecureMeshSecretStore>>) -> Result<Self> {
        let mut unavailable_platform_facts = Vec::new();
        if let Some(store) = os_store {
            let platform_facts = store.capability_facts()?;
            let runtime_store_measured = platform_facts.iter().any(|fact| {
                fact.capability == SecurityCapability::OsSecureStore
                    && fact.state == CapabilityFactState::Supported
                    && fact.evidence_kind == CapabilityEvidenceKind::RuntimeOperation
            });
            if store.supported() && runtime_store_measured {
                let capability_evaluation = evaluate_platform_facts(&platform_facts)?;
                if capability_evaluation
                    .custody()
                    .map(|selection| selection.strategy)
                    == Some(SecretCustodyStrategy::OsSecureStore)
                {
                    return Ok(Self {
                        store,
                        capability_evaluation,
                    });
                }
            }
            unavailable_platform_facts = conservative_unavailable_facts(platform_facts)?;
        }
        let store: Arc<dyn SecureMeshSecretStore> = Arc::new(
            EphemeralSecretStore::with_unavailable_platform_facts(unavailable_platform_facts)?,
        );
        let capability_evaluation = store.capability_evaluation()?;
        Ok(Self {
            store,
            capability_evaluation,
        })
    }

    pub fn store(&self) -> Arc<dyn SecureMeshSecretStore> {
        Arc::clone(&self.store)
    }

    pub fn capability_evaluation(&self) -> &CapabilityEvaluation {
        &self.capability_evaluation
    }

    pub fn strategy(&self) -> SecretCustodyStrategy {
        self.capability_evaluation
            .custody()
            .map(|selection| selection.strategy)
            .unwrap_or(SecretCustodyStrategy::MemoryOnlyEphemeral)
    }

    pub fn restart_semantics(&self) -> CustodyRestartSemantics {
        self.capability_evaluation
            .custody()
            .map(|selection| selection.restart_semantics)
            .unwrap_or(CustodyRestartSemantics::RePairRekeyAfterRestart)
    }
}

fn evaluate_platform_facts(platform_facts: &[CapabilityFact]) -> Result<CapabilityEvaluation> {
    let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::SourceContract)?;
    facts.extend_from_slice(platform_facts);
    capability_catalog()?.evaluate(&facts)
}

fn conservative_unavailable_facts(
    platform_facts: Vec<CapabilityFact>,
) -> Result<Vec<CapabilityFact>> {
    let mut unavailable = platform_facts
        .into_iter()
        .filter(|fact| fact.state != CapabilityFactState::Supported)
        .collect::<Vec<_>>();
    if !unavailable
        .iter()
        .any(|fact| fact.capability == SecurityCapability::OsSecureStore)
    {
        unavailable.push(CapabilityFact::unavailable(
            SecurityCapability::OsSecureStore,
            CapabilityFactState::Unverified,
            CapabilityEvidenceKind::NotMeasured,
            "platform_secret_store_runtime_operation_unverified",
        )?);
    }
    Ok(unavailable)
}
