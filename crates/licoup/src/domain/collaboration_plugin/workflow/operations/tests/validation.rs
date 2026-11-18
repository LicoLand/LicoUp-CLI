use serde_json::json;

use super::super::super::model::WorkflowKind;
use super::super::validation::{ApplyRequest, selected_ids, validate_apply_binding};
use super::sample_record;

#[test]
fn selection_is_canonical_bounded_and_duplicate_free() {
    assert_eq!(
        selected_ids(
            &json!({"selected": ["server-tools", "server-core"]}),
            "selected"
        )
        .unwrap(),
        vec!["server-core", "server-tools"]
    );
    assert_eq!(
        selected_ids(
            &json!({"selected": ["server-core", "server-core"]}),
            "selected"
        )
        .unwrap_err()
        .to_string(),
        "collaboration_workflow_selection_duplicate"
    );
    assert_eq!(
        selected_ids(&json!({"selected": ["Server Core"]}), "selected")
            .unwrap_err()
            .to_string(),
        "collaboration_workflow_selection_invalid"
    );
}

#[test]
fn apply_binding_fails_closed_on_plan_or_digest_mismatch() {
    let record = sample_record(WorkflowKind::LocalDeployment);
    let request = ApplyRequest {
        plan_id: record.plan_id.clone(),
        expected_plan_digest_sha256: record.plan_digest_sha256.clone(),
        expected_package_digest_sha256: record.package_digest_sha256.clone(),
    };
    validate_apply_binding(&record, WorkflowKind::LocalDeployment, &request).unwrap();

    let wrong = ApplyRequest {
        expected_plan_digest_sha256: "c".repeat(64),
        ..request
    };
    assert_eq!(
        validate_apply_binding(&record, WorkflowKind::LocalDeployment, &wrong)
            .unwrap_err()
            .to_string(),
        "collaboration_workflow_plan_digest_mismatch"
    );
}

#[test]
fn apply_request_accepts_only_canonical_uuid_and_sha256_fields() {
    let request = ApplyRequest::from_params(&json!({
        "planId": uuid::Uuid::nil().to_string(),
        "expectedPlanDigestSha256": "a".repeat(64),
        "expectedPackageDigestSha256": "b".repeat(64)
    }))
    .unwrap();
    assert_eq!(request.plan_id, uuid::Uuid::nil().to_string());

    assert_eq!(
        ApplyRequest::from_params(&json!({
            "planId": "not-a-uuid",
            "expectedPlanDigestSha256": "a".repeat(64),
            "expectedPackageDigestSha256": "b".repeat(64)
        }))
        .err()
        .unwrap()
        .to_string(),
        "collaboration_workflow_plan_id_invalid"
    );
}
