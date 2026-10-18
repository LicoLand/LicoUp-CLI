use super::super::{
    CapabilityEvidenceKind, CapabilityFact, CapabilityFactState, CapabilityScope,
    SecurityCapability, capability_catalog, mandatory_protocol_facts,
};

#[test]
fn mandatory_fact_projection_contains_only_direct_mandatory_protocol_capabilities() {
    let catalog = capability_catalog().unwrap();
    let facts = mandatory_protocol_facts(CapabilityEvidenceKind::SourceContract).unwrap();
    assert!(!facts.is_empty());
    for fact in facts {
        let definition = catalog.definition(fact.capability).unwrap();
        assert!(definition.mandatory);
        assert!(!definition.derived);
        assert_eq!(definition.scope, CapabilityScope::ProtocolSession);
        assert_eq!(fact.state, CapabilityFactState::Supported);
        assert_eq!(fact.evidence_kind, CapabilityEvidenceKind::SourceContract);
    }
}

#[test]
fn unavailable_facts_reject_supported_state_and_unbounded_or_sensitive_reasons() {
    assert!(
        CapabilityFact::unavailable(
            SecurityCapability::Tee,
            CapabilityFactState::Supported,
            CapabilityEvidenceKind::TestFixture,
            "tee_supported",
        )
        .is_err()
    );
    for invalid_reason in [
        "contains forbidden whitespace".to_string(),
        "UPPERCASE".to_string(),
        "x".repeat(97),
    ] {
        assert!(
            CapabilityFact::unavailable(
                SecurityCapability::Tee,
                CapabilityFactState::Unsupported,
                CapabilityEvidenceKind::TestFixture,
                invalid_reason,
            )
            .is_err()
        );
    }
}

#[test]
fn fact_schema_rejects_unknown_fields() {
    let value = serde_json::json!({
        "capability": "custody.tee",
        "state": "unverified",
        "evidenceKind": "not_measured",
        "measuredAtUnixSeconds": null,
        "reasonCode": "measurement_pending",
        "localPath": "<user-home>/private.txt"
    });
    assert!(serde_json::from_value::<CapabilityFact>(value).is_err());
}
