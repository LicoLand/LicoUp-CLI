use super::{AdmittedCommand, CliExecution};
use crate::domain::mcp_adapter::McpHttpTransportResponse;
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub(super) fn handle_preview(command: AdmittedCommand) -> Result<CliExecution> {
    let result = (|| -> Result<_> {
        let params = command
            .option_json("stdin-json")
            .cloned()
            .ok_or_else(|| anyhow!("mcp_transfer_private_input_invalid"))?;
        let plans =
            crate::platform::mcp_approval_plan_store::PrivateMcpApprovalPlanStore::open_default()?;
        crate::domain::mcp_adapter::preview_http_transfer(&params, &plans)
    })()
    .map_err(|_| anyhow!("mcp_transfer_preview_failed"))?;
    Ok(CliExecution::Json(result))
}

pub(super) fn handle_execute(command: AdmittedCommand) -> Result<CliExecution> {
    let result = (|| -> Result<_> {
        let params = command
            .option_json("stdin-json")
            .cloned()
            .ok_or_else(|| anyhow!("mcp_transfer_private_input_invalid"))?;
        let _presence = authorize_exact_transfer(&params)?;
        let plans =
            crate::platform::mcp_approval_plan_store::PrivateMcpApprovalPlanStore::open_default()?;
        crate::domain::mcp_adapter::execute_http_transfer(&params, &plans, |packet, session_id| {
            let response = crate::platform::mcp_streamable_http::exchange(packet, session_id)?;
            Ok(McpHttpTransportResponse {
                status: response.status,
                content_type: response.content_type,
                session_id: response.session_id,
                body: response.body,
            })
        })
    })()
    .map_err(|_| anyhow!("mcp_transfer_execute_failed"))?;
    Ok(CliExecution::Json(result))
}

fn authorize_exact_transfer(
    params: &Value,
) -> Result<crate::platform::user_presence::UserPresenceSession> {
    let scope = exact_approval_scope(params)?;
    ensure!(
        crate::platform::user_presence::available(),
        "mcp_transfer_user_presence_unavailable"
    );
    crate::platform::user_presence::authorize("Approve this exact MCP transfer in LicoUp", scope)
}

fn exact_approval_scope(params: &Value) -> Result<&str> {
    let digest = params
        .get("approvalDigest")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("mcp_transfer_approval_scope_invalid"))?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
        "mcp_transfer_approval_scope_invalid"
    );
    Ok(digest)
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_help_exposes_preview_and_direct_execution_only() {
        let help = super::super::build_command_table().help_text().join("\n");
        assert!(help.contains("mcp http preview"));
        assert!(help.contains("mcp http execute"));
        assert!(!help.contains("mcp http authorize"));
        assert!(!help.contains("mcp http automatic"));
    }

    #[test]
    fn exact_user_presence_scope_rejects_missing_or_noncanonical_digests() {
        assert!(super::exact_approval_scope(&serde_json::json!({})).is_err());
        assert!(
            super::exact_approval_scope(&serde_json::json!({
                "approvalDigest": "A".repeat(64)
            }))
            .is_err()
        );
        assert_eq!(
            super::exact_approval_scope(&serde_json::json!({
                "approvalDigest": "a".repeat(64)
            }))
            .unwrap(),
            "a".repeat(64)
        );
    }
}
