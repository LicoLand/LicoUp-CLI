use anyhow::Result;

/// Persistence port for a short-lived, one-shot MCP transfer preview.
///
/// Implementations retain only the opaque plan identifier, approval digest,
/// and expiry. The message, destination, purpose, and session identifier must
/// never be persisted by this port.
pub trait McpApprovalPlanStore {
    fn stage(&self, approval_digest: &str) -> Result<String>;
    fn claim(&self, plan_id: &str) -> Result<String>;
}
