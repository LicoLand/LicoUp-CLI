use super::super::{
    CapabilityEvidenceKind, CapabilityFact, CapabilityFactState, SecurityCapability,
    capability_catalog,
};
use super::support::{all_supported_facts, baseline_facts};

#[test]
fn canonical_evaluation_is_complete_and_linear_in_nodes_and_edges() {
    let catalog = capability_catalog().unwrap();
    let evaluation = catalog.evaluate(&all_supported_facts(catalog)).unwrap();
    assert_eq!(evaluation.enabled().len(), SecurityCapability::COUNT);
    assert_eq!(
        evaluation.evaluation_work(),
        (SecurityCapability::COUNT, catalog.edge_count())
    );
    assert!(evaluation.mandatory_foundation_complete());
}

#[test]
fn supported_fact_additions_are_monotonic() {
    let catalog = capability_catalog().unwrap();
    let base = catalog.evaluate(&baseline_facts()).unwrap();
    let mut expanded_facts = baseline_facts();
    expanded_facts.extend([
        CapabilityFact::supported(
            SecurityCapability::OsSecureStore,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(
            SecurityCapability::SoftwareBacked,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(
            SecurityCapability::LinuxSecretService,
            CapabilityEvidenceKind::TestFixture,
        ),
    ]);
    let expanded = catalog.evaluate(&expanded_facts).unwrap();
    assert!(base.enabled().is_subset(expanded.enabled()));
    assert!(
        expanded
            .enabled()
            .contains(&SecurityCapability::LinuxSecretService)
    );
}

#[test]
fn strongbox_and_tee_remain_independent_environment_facts() {
    let catalog = capability_catalog().unwrap();
    let mut facts = baseline_facts();
    for capability in [
        SecurityCapability::OsSecureStore,
        SecurityCapability::NonExportable,
        SecurityCapability::DeviceBound,
        SecurityCapability::HardwareBacked,
        SecurityCapability::AndroidKeystore,
        SecurityCapability::Strongbox,
    ] {
        facts.push(CapabilityFact::supported(
            capability,
            CapabilityEvidenceKind::TestFixture,
        ));
    }
    let evaluation = catalog.evaluate(&facts).unwrap();
    assert!(
        evaluation
            .enabled()
            .contains(&SecurityCapability::Strongbox)
    );
    assert!(!evaluation.enabled().contains(&SecurityCapability::Tee));
}

#[test]
fn missing_dependency_disables_the_node_and_dependents_with_a_stable_reason() {
    let catalog = capability_catalog().unwrap();
    let mut facts = baseline_facts();
    facts.extend([
        CapabilityFact::supported(
            SecurityCapability::HardwareBacked,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(SecurityCapability::Tee, CapabilityEvidenceKind::TestFixture),
    ]);
    let evaluation = catalog.evaluate(&facts).unwrap();
    assert!(evaluation.mandatory_foundation_complete());
    assert!(
        evaluation
            .available()
            .contains(&SecurityCapability::HardwareBacked)
    );
    assert!(
        !evaluation
            .enabled()
            .contains(&SecurityCapability::HardwareBacked)
    );
    assert!(!evaluation.enabled().contains(&SecurityCapability::Tee));
    assert_eq!(
        evaluation
            .reasons()
            .get(&SecurityCapability::HardwareBacked)
            .map(String::as_str),
        Some("capability_dependency_unmet")
    );
}

#[test]
fn every_missing_mandatory_node_is_rejected_without_downgrade() {
    let catalog = capability_catalog().unwrap();
    let baseline = baseline_facts();
    for omitted in baseline.iter().map(|fact| fact.capability) {
        let facts = baseline
            .iter()
            .filter(|fact| fact.capability != omitted)
            .cloned()
            .collect::<Vec<_>>();
        let evaluation = catalog.evaluate(&facts).unwrap();
        assert!(!evaluation.mandatory_foundation_complete());
        assert!(evaluation.require_mandatory_foundation().is_err());
        assert!(evaluation.missing_mandatory().contains(&omitted));
    }
}

#[test]
fn unavailable_and_unverified_states_remain_disjoint() {
    let catalog = capability_catalog().unwrap();
    let mut facts = baseline_facts();
    facts.extend([
        CapabilityFact::unavailable(
            SecurityCapability::Strongbox,
            CapabilityFactState::Unsupported,
            CapabilityEvidenceKind::GeneratedKeyInspection,
            "strongbox_not_supported",
        )
        .unwrap(),
        CapabilityFact::unavailable(
            SecurityCapability::OsUserPresence,
            CapabilityFactState::TemporarilyUnavailable,
            CapabilityEvidenceKind::OsAuthorization,
            "system_credential_not_configured",
        )
        .unwrap(),
        CapabilityFact::unavailable(
            SecurityCapability::SecureEnclave,
            CapabilityFactState::Unverified,
            CapabilityEvidenceKind::NotMeasured,
            "host_measurement_pending",
        )
        .unwrap(),
    ]);
    let evaluation = catalog.evaluate(&facts).unwrap();
    assert!(
        evaluation
            .unavailable()
            .contains(&SecurityCapability::Strongbox)
    );
    assert!(
        evaluation
            .unavailable()
            .contains(&SecurityCapability::OsUserPresence)
    );
    assert!(
        evaluation
            .unverified()
            .contains(&SecurityCapability::SecureEnclave)
    );
}

#[test]
fn evaluation_rejects_duplicate_derived_and_oversized_fact_sets() {
    let catalog = capability_catalog().unwrap();
    let fact = CapabilityFact::supported(
        SecurityCapability::AuthenticatedEncryption,
        CapabilityEvidenceKind::TestFixture,
    );
    assert!(catalog.evaluate(&[fact.clone(), fact]).is_err());

    let derived = CapabilityFact::supported(
        SecurityCapability::SecureSessionFoundation,
        CapabilityEvidenceKind::TestFixture,
    );
    assert!(catalog.evaluate(&[derived]).is_err());

    let oversized = vec![
        CapabilityFact::supported(
            SecurityCapability::AuthenticatedEncryption,
            CapabilityEvidenceKind::TestFixture,
        );
        SecurityCapability::COUNT + 1
    ];
    assert!(catalog.evaluate(&oversized).is_err());
}
