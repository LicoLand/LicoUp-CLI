use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::assembly::PlannedLocalAssembly;
use super::super::registration::{AgentDestination, PlannedAgentRegistration};

pub(super) const WORKFLOW_PLAN_SCHEMA: &str = "licoup.optional-collaboration-workflow-plan.v3";
pub(super) const WORKFLOW_PLAN_TTL_SECONDS: u64 = 30 * 60;
pub(super) const MAX_WORKFLOW_PLAN_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_ACTIVE_WORKFLOW_PLANS: usize = 8;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum WorkflowKind {
    LocalDeployment,
    McpInstall,
}

impl WorkflowKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::LocalDeployment => "local-deployment",
            Self::McpInstall => "mcp-install",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct PlannedPayloadFile {
    pub(super) selection_id: String,
    pub(super) source_relative_path: String,
    pub(super) destination_relative_path: String,
    pub(super) digest_sha256: String,
    pub(super) bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct WorkflowPlanRecord {
    pub(super) schema_version: String,
    pub(super) plan_id: String,
    pub(super) plan_digest_sha256: String,
    pub(super) workflow_kind: WorkflowKind,
    pub(super) plugin_id: String,
    pub(super) package_digest_sha256: String,
    pub(super) selected_ids: Vec<String>,
    pub(super) local_destination: Option<String>,
    pub(super) local_assembly: Option<PlannedLocalAssembly>,
    pub(super) agent_destinations: Vec<AgentDestination>,
    pub(super) payload_files: Vec<PlannedPayloadFile>,
    pub(super) agent_registrations: Vec<PlannedAgentRegistration>,
    pub(super) created_at_epoch_seconds: u64,
    pub(super) expires_at_epoch_seconds: u64,
}

impl WorkflowPlanRecord {
    pub(super) fn seal(&mut self) -> Result<()> {
        self.plan_digest_sha256.clear();
        self.plan_digest_sha256 = digest_record(self)?;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == WORKFLOW_PLAN_SCHEMA,
            "collaboration_workflow_plan_schema_invalid"
        );
        ensure!(
            is_sha256(&self.package_digest_sha256) && is_sha256(&self.plan_digest_sha256),
            "collaboration_workflow_plan_digest_invalid"
        );
        let mut unsigned = self.clone();
        unsigned.plan_digest_sha256.clear();
        ensure!(
            digest_record(&unsigned)? == self.plan_digest_sha256,
            "collaboration_workflow_plan_binding_invalid"
        );
        match self.workflow_kind {
            WorkflowKind::LocalDeployment => {
                let assembly = self
                    .local_assembly
                    .as_ref()
                    .ok_or_else(|| anyhow!("collaboration_local_server_build_plan_missing"))?;
                assembly.validate()?;
                ensure!(
                    self.local_destination.is_some()
                        && self.agent_destinations.is_empty()
                        && self.agent_registrations.is_empty(),
                    "collaboration_local_server_build_plan_invalid"
                );
            }
            WorkflowKind::McpInstall => ensure!(
                self.local_assembly.is_none() && self.local_destination.is_none(),
                "collaboration_workflow_mcp_plan_shape_invalid"
            ),
        }
        Ok(())
    }
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn validate_sha256(value: &str) -> Result<()> {
    ensure!(is_sha256(value), "collaboration_workflow_digest_invalid");
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn digest_record(record: &WorkflowPlanRecord) -> Result<String> {
    let bytes = serde_json::to_vec(record)
        .map_err(|_| anyhow!("collaboration_workflow_plan_serialization_failed"))?;
    Ok(sha256_hex(&bytes))
}
