use super::super::{
    CAPABILITY_REPORT_SCHEMA_VERSION, CapabilityEvaluationReport, capability_catalog,
};
use super::support::{all_supported_facts, baseline_facts};

#[test]
fn report_bytes_are_deterministic_for_the_same_catalog_and_facts() {
    let catalog = capability_catalog().unwrap();
    let facts = all_supported_facts(catalog);
    let first = serde_json::to_vec(&catalog.evaluate(&facts).unwrap().report()).unwrap();
    let second = serde_json::to_vec(&catalog.evaluate(&facts).unwrap().report()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn report_schema_is_exact_and_has_no_scalar_posture_grade() {
    let catalog = capability_catalog().unwrap();
    let report = catalog.evaluate(&baseline_facts()).unwrap().report();
    assert_eq!(report.schema_version, CAPABILITY_REPORT_SCHEMA_VERSION);
    let encoded = serde_json::to_string(&report).unwrap();
    assert!(!encoded.contains("\"tier\""));
    assert!(!encoded.contains("\"level\""));

    let mut value = serde_json::to_value(report).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_string(), serde_json::json!(true));
    assert!(serde_json::from_value::<CapabilityEvaluationReport>(value).is_err());
}

#[test]
fn report_reason_keys_are_stable_capability_identifiers() {
    let catalog = capability_catalog().unwrap();
    let report = catalog.evaluate(&[]).unwrap().report();
    assert!(
        report
            .reasons
            .keys()
            .all(|key| key.starts_with("protocol.") || key.starts_with("custody."))
    );
}
