use super::super::{
    CapabilityEvidenceKind, CapabilityFact, CapabilityFactState, SecurityCapability,
};

#[test]
fn canonical_catalog_declares_only_custody_capabilities() {
    use super::super::capability_catalog;
    let catalog = capability_catalog().unwrap();
    for definition in catalog.definitions() {
        assert!(definition.capability.id().starts_with("custody."));
        assert!(
            !definition.derived || definition.capability == SecurityCapability::MemoryOnlyEphemeral
        );
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
