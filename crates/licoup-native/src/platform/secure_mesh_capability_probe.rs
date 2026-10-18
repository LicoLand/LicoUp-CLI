use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvidenceKind, CapabilityFact, CapabilityFactState,
    SecurityCapability, capability_catalog, mandatory_protocol_facts,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CAPABILITY_PROBE_SCHEMA_VERSION: u32 = 1;

pub trait SecureMeshCapabilityProbe: Send + Sync {
    fn probe(&self) -> Result<CapabilityProbeSnapshot>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilityProbeSnapshot {
    pub schema_version: u32,
    pub facts: Vec<CapabilityFact>,
}

impl CapabilityProbeSnapshot {
    pub fn new(facts: Vec<CapabilityFact>) -> Result<Self> {
        let mut identifiers = BTreeSet::new();
        for fact in &facts {
            ensure!(
                identifiers.insert(fact.capability),
                "secure mesh capability probe returned a duplicate fact"
            );
        }
        Ok(Self {
            schema_version: CAPABILITY_PROBE_SCHEMA_VERSION,
            facts,
        })
    }

    pub fn evaluate_with_protocol_foundation(&self) -> Result<CapabilityEvaluation> {
        let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::SourceContract)?;
        let mut identifiers = facts
            .iter()
            .map(|fact| fact.capability)
            .collect::<BTreeSet<_>>();
        for fact in &self.facts {
            ensure!(
                identifiers.insert(fact.capability),
                "secure mesh capability probe conflicts with protocol foundation facts"
            );
            facts.push(fact.clone());
        }
        capability_catalog()?.evaluate(&facts)
    }
}

#[derive(Clone, Debug)]
pub struct StaticCapabilityProbe {
    snapshot: CapabilityProbeSnapshot,
}

impl StaticCapabilityProbe {
    pub fn new(facts: Vec<CapabilityFact>) -> Result<Self> {
        Ok(Self {
            snapshot: CapabilityProbeSnapshot::new(facts)?,
        })
    }
}

impl SecureMeshCapabilityProbe for StaticCapabilityProbe {
    fn probe(&self) -> Result<CapabilityProbeSnapshot> {
        Ok(self.snapshot.clone())
    }
}

pub fn supported_fact(
    capability: SecurityCapability,
    evidence_kind: CapabilityEvidenceKind,
) -> CapabilityFact {
    CapabilityFact::supported(capability, evidence_kind)
}

pub fn unsupported_fact(
    capability: SecurityCapability,
    evidence_kind: CapabilityEvidenceKind,
    reason_code: impl Into<String>,
) -> Result<CapabilityFact> {
    CapabilityFact::unavailable(
        capability,
        CapabilityFactState::Unsupported,
        evidence_kind,
        reason_code,
    )
}

pub fn temporarily_unavailable_fact(
    capability: SecurityCapability,
    evidence_kind: CapabilityEvidenceKind,
    reason_code: impl Into<String>,
) -> Result<CapabilityFact> {
    CapabilityFact::unavailable(
        capability,
        CapabilityFactState::TemporarilyUnavailable,
        evidence_kind,
        reason_code,
    )
}

pub fn unverified_fact(
    capability: SecurityCapability,
    evidence_kind: CapabilityEvidenceKind,
    reason_code: impl Into<String>,
) -> Result<CapabilityFact> {
    CapabilityFact::unavailable(
        capability,
        CapabilityFactState::Unverified,
        evidence_kind,
        reason_code,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secure_mesh_capability::{CustodyRestartSemantics, SecretCustodyStrategy};

    #[test]
    fn probe_keeps_all_states_independent_and_emits_no_readiness_authority() {
        let snapshot = CapabilityProbeSnapshot::new(vec![
            supported_fact(
                SecurityCapability::OsSecureStore,
                CapabilityEvidenceKind::RuntimeOperation,
            ),
            unsupported_fact(
                SecurityCapability::Tee,
                CapabilityEvidenceKind::GeneratedKeyInspection,
                "tee_not_supported",
            )
            .unwrap(),
            temporarily_unavailable_fact(
                SecurityCapability::OsUserPresence,
                CapabilityEvidenceKind::OsAuthorization,
                "system_credential_not_configured",
            )
            .unwrap(),
            unverified_fact(
                SecurityCapability::SecureEnclave,
                CapabilityEvidenceKind::NotMeasured,
                "host_measurement_pending",
            )
            .unwrap(),
        ])
        .unwrap();
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("productionReady"));
        assert!(!encoded.contains("releaseReady"));
        assert!(!encoded.contains("deviceId"));

        let evaluation = snapshot.evaluate_with_protocol_foundation().unwrap();
        assert!(evaluation.mandatory_foundation_complete());
        assert_eq!(
            evaluation.custody().map(|selection| selection.strategy),
            Some(SecretCustodyStrategy::OsSecureStore)
        );
        assert_eq!(
            evaluation
                .custody()
                .map(|selection| selection.restart_semantics),
            Some(CustodyRestartSemantics::PersistentStateAvailable)
        );
        assert!(!evaluation.enabled().contains(&SecurityCapability::Tee));
    }

    #[test]
    fn probe_rejects_duplicate_facts_and_unknown_schema_fields() {
        let fact = supported_fact(
            SecurityCapability::OsSecureStore,
            CapabilityEvidenceKind::TestFixture,
        );
        assert!(CapabilityProbeSnapshot::new(vec![fact.clone(), fact]).is_err());

        let invalid = serde_json::json!({
            "schemaVersion": CAPABILITY_PROBE_SCHEMA_VERSION,
            "facts": [],
            "ready": true
        });
        assert!(serde_json::from_value::<CapabilityProbeSnapshot>(invalid).is_err());
    }

    #[test]
    fn static_probe_returns_an_immutable_snapshot() {
        let probe = StaticCapabilityProbe::new(vec![supported_fact(
            SecurityCapability::SoftwareBacked,
            CapabilityEvidenceKind::TestFixture,
        )])
        .unwrap();
        assert_eq!(probe.probe().unwrap(), probe.probe().unwrap());
    }
}
