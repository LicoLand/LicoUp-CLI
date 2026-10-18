mod apply_local;
mod apply_mcp;
mod cancel;
mod composition;
mod destination_policy;
mod package_revalidation;
mod plan_local;
mod plan_mcp;
mod projection;
mod staging;
mod validation;

use super::super::model::{WORKFLOW_PLAN_SCHEMA, WorkflowKind, WorkflowPlanRecord};

fn sample_record(workflow_kind: WorkflowKind) -> WorkflowPlanRecord {
    WorkflowPlanRecord {
        schema_version: WORKFLOW_PLAN_SCHEMA.to_owned(),
        plan_id: uuid::Uuid::nil().to_string(),
        plan_digest_sha256: "b".repeat(64),
        workflow_kind,
        plugin_id: "licomesh-collaboration".to_owned(),
        package_digest_sha256: "a".repeat(64),
        selected_ids: vec!["server-core".to_owned()],
        local_destination: None,
        local_assembly: None,
        agent_destinations: Vec::new(),
        payload_files: Vec::new(),
        agent_registrations: Vec::new(),
        created_at_epoch_seconds: 1,
        expires_at_epoch_seconds: u64::MAX,
    }
}
