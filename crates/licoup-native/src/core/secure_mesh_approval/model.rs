use anyhow::{Result, bail};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ApprovalDecision {
    Allow,
    Deny,
}

impl ApprovalDecision {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "allow" | "approve" | "approved" => Ok(Self::Allow),
            "deny" | "denied" | "reject" | "rejected" => Ok(Self::Deny),
            _ => bail!("secure mesh approval decision is unsupported"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PendingApproval {
    pub(super) pending_operation_id: String,
    pub(super) requester_agent_id: String,
    pub(super) target_client_id: String,
    pub(super) origin_endpoint_id: String,
    pub(super) risk_level: String,
    pub(super) display_summary: String,
    pub(super) policy_reason: String,
    pub(super) adapter_callback_token_ref: String,
    pub(super) adapter_style: String,
    pub(super) expires_at: String,
    pub(super) response_nonce: String,
    pub(super) requested_tools: Vec<String>,
    pub(super) trusted_endpoint_ids: Vec<String>,
    pub(super) created_at: String,
    pub(super) resolved: Option<ResolvedApproval>,
}

#[derive(Clone, Debug)]
pub(super) struct ResolvedApproval {
    pub(super) decision: ApprovalDecision,
    pub(super) responding_endpoint_id: String,
    pub(super) resolved_at: String,
    #[allow(dead_code)]
    pub(super) response_nonce: String,
}

#[derive(Default)]
pub(super) struct ApprovalLedger {
    pub(super) pending: HashMap<String, PendingApproval>,
}
