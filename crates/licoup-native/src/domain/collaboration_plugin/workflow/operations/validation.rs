use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use uuid::Uuid;

use super::super::super::lifecycle::{InstalledWorkflowPlugin, epoch_seconds};
use super::super::super::registration::{AgentDestination, PlannedAgentRegistration};
use super::super::model::{
    PlannedPayloadFile, WORKFLOW_PLAN_SCHEMA, WORKFLOW_PLAN_TTL_SECONDS, WorkflowKind,
    WorkflowPlanRecord, validate_sha256,
};

pub(super) struct ApplyRequest {
    pub(super) plan_id: String,
    pub(super) expected_plan_digest_sha256: String,
    pub(super) expected_package_digest_sha256: String,
}

impl ApplyRequest {
    pub(super) fn from_params(params: &Value) -> Result<Self> {
        let plan_id =
            required_text(params, "planId", "collaboration_workflow_plan_id_required")?.to_owned();
        let parsed = Uuid::parse_str(&plan_id)
            .map_err(|_| anyhow!("collaboration_workflow_plan_id_invalid"))?;
        ensure!(
            parsed.to_string() == plan_id,
            "collaboration_workflow_plan_id_invalid"
        );
        let expected_plan_digest_sha256 = required_text(
            params,
            "expectedPlanDigestSha256",
            "collaboration_workflow_expected_plan_digest_required",
        )?
        .to_owned();
        let expected_package_digest_sha256 = required_text(
            params,
            "expectedPackageDigestSha256",
            "collaboration_workflow_expected_package_digest_required",
        )?
        .to_owned();
        validate_sha256(&expected_plan_digest_sha256)?;
        validate_sha256(&expected_package_digest_sha256)?;
        Ok(Self {
            plan_id,
            expected_plan_digest_sha256,
            expected_package_digest_sha256,
        })
    }
}

pub(super) fn validate_apply_binding(
    record: &WorkflowPlanRecord,
    expected_kind: WorkflowKind,
    request: &ApplyRequest,
) -> Result<()> {
    validate_expected_digests(record, request)?;
    ensure!(
        record.workflow_kind == expected_kind,
        "collaboration_workflow_kind_mismatch"
    );
    ensure!(
        record.expires_at_epoch_seconds > epoch_seconds(),
        "collaboration_workflow_plan_expired"
    );
    Ok(())
}

pub(super) fn validate_expected_digests(
    record: &WorkflowPlanRecord,
    request: &ApplyRequest,
) -> Result<()> {
    ensure!(
        record.plan_id == request.plan_id,
        "collaboration_workflow_plan_id_mismatch"
    );
    ensure!(
        record.plan_digest_sha256 == request.expected_plan_digest_sha256,
        "collaboration_workflow_plan_digest_mismatch"
    );
    ensure!(
        record.package_digest_sha256 == request.expected_package_digest_sha256,
        "collaboration_workflow_package_digest_mismatch"
    );
    Ok(())
}

pub(super) fn new_plan_record(
    workflow_kind: WorkflowKind,
    installed: &InstalledWorkflowPlugin,
    selected_ids: Vec<String>,
    local_destination: Option<String>,
    agent_destinations: Vec<AgentDestination>,
    payload_files: Vec<PlannedPayloadFile>,
    agent_registrations: Vec<PlannedAgentRegistration>,
) -> Result<WorkflowPlanRecord> {
    let created_at_epoch_seconds = epoch_seconds();
    let expires_at_epoch_seconds = created_at_epoch_seconds
        .checked_add(WORKFLOW_PLAN_TTL_SECONDS)
        .ok_or_else(|| anyhow!("collaboration_workflow_plan_expiry_invalid"))?;
    Ok(WorkflowPlanRecord {
        schema_version: WORKFLOW_PLAN_SCHEMA.to_owned(),
        plan_id: Uuid::new_v4().to_string(),
        plan_digest_sha256: String::new(),
        workflow_kind,
        plugin_id: installed.plugin_id.clone(),
        package_digest_sha256: installed.digest_sha256.clone(),
        selected_ids,
        local_destination,
        local_assembly: None,
        agent_destinations,
        payload_files,
        agent_registrations,
        created_at_epoch_seconds,
        expires_at_epoch_seconds,
    })
}

pub(super) fn selected_ids(params: &Value, key: &str) -> Result<Vec<String>> {
    let value = params
        .get(key)
        .ok_or_else(|| anyhow!("collaboration_workflow_selection_required"))?;
    let mut values = match value {
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("collaboration_workflow_selection_invalid"))
            })
            .collect::<Result<Vec<_>>>()?,
        Value::String(value) if value.trim_start().starts_with('[') => {
            serde_json::from_str::<Vec<String>>(value)
                .map_err(|_| anyhow!("collaboration_workflow_selection_invalid"))?
        }
        Value::String(value) => value.split(',').map(str::to_owned).collect(),
        _ => return Err(anyhow!("collaboration_workflow_selection_invalid")),
    };
    ensure!(
        !values.is_empty() && values.len() <= 256,
        "collaboration_workflow_selection_required"
    );
    ensure!(
        values.iter().all(|value| {
            value == value.trim()
                && !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        }),
        "collaboration_workflow_selection_invalid"
    );
    values.sort();
    let original_len = values.len();
    values.dedup();
    ensure!(
        values.len() == original_len,
        "collaboration_workflow_selection_duplicate"
    );
    Ok(values)
}

pub(super) fn require_direct_origin(params: &Value) -> Result<()> {
    ensure!(
        params.get("requestOrigin").and_then(Value::as_str) == Some("direct-user"),
        "collaboration_workflow_direct_user_origin_required"
    );
    for key in ["agentTriggered", "scheduled", "startupTriggered"] {
        ensure!(
            !bool_value(params.get(key)).unwrap_or(false),
            "collaboration_workflow_automatic_trigger_forbidden"
        );
    }
    Ok(())
}

pub(super) fn require_bool(params: &Value, key: &str, code: &'static str) -> Result<()> {
    ensure!(bool_value(params.get(key)) == Some(true), code);
    Ok(())
}

pub(super) fn bool_value(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::String(value) if value == "true" => Some(true),
        Value::String(value) if value == "false" => Some(false),
        _ => None,
    }
}

pub(super) fn required_text<'a>(
    params: &'a Value,
    key: &str,
    code: &'static str,
) -> Result<&'a str> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!(code))?;
    ensure!(value == value.trim() && !value.is_empty(), code);
    Ok(value)
}
