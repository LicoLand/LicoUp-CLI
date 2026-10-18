use super::super::*;
use licoup_native::domain::client_conversation::ConversationService;

pub(super) fn requires_worker(params: &Value) -> bool {
    params.get("action").and_then(Value::as_str) == Some("conversation.dispatch.after-post")
}

pub(super) fn spawn_execute<W>(
    writer: Arc<StdioRpcWriter<W>>,
    request_id: String,
    workflow_id: String,
    params: Value,
    service: ConversationService,
    portable_data_dir: Option<PathBuf>,
) -> io::Result<std::thread::JoinHandle<()>>
where
    W: Write + Send + 'static,
{
    std::thread::Builder::new()
        .name("conversation-after-post".to_owned())
        .spawn(move || {
            let _ = execute(
                &writer,
                StdioRpcFrameLane::Stream,
                &request_id,
                &workflow_id,
                params,
                service,
                portable_data_dir,
            );
        })
}

pub(super) fn execute<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    params: Value,
    service: ConversationService,
    portable_data_dir: Option<PathBuf>,
) -> Result<()> {
    let subagent_host_operation = params
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(|action| action.starts_with("conversation.subagent."));
    let execution = catch_unwind(AssertUnwindSafe(|| {
        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
        service.execute(params)
    }));
    let target = writer.lane_target(lane);
    match execution {
        Ok(Ok(result)) => write_stdio_rpc_success_shared(
            target,
            request_id,
            workflow_id,
            json!({"ok": true, "result": result}),
        )?,
        Ok(Err(error)) if subagent_host_operation => write_stdio_rpc_success_shared(
            target,
            request_id,
            workflow_id,
            json!({
                "ok": false,
                "error": {"code": subagent_host_error_code(&error)},
            }),
        )?,
        Ok(Err(error)) => write_stdio_rpc_client_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            &stdio_rpc_command_error(&error),
        )?,
        Err(_) => write_stdio_rpc_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            "command_panicked",
        )?,
    }
    Ok(())
}

fn subagent_host_error_code(error: &anyhow::Error) -> &'static str {
    match error.to_string().split(':').next().unwrap_or("") {
        "conversation_not_found" => "conversation_not_found",
        "subagent_self_call_rejected" => "subagent_self_call_rejected",
        "subagent_caller_membership_inactive" => "subagent_caller_membership_inactive",
        "subagent_target_membership_inactive" => "subagent_target_membership_inactive",
        "subagent_target_invalid" => "subagent_target_invalid",
        "subagent_duplicate_active_edge" => "subagent_duplicate_active_edge",
        "subagent_parent_dispatch_unavailable" => "subagent_parent_dispatch_unavailable",
        "subagent_cross_conversation_rejected" => "subagent_cross_conversation_rejected",
        "subagent_lineage_caller_mismatch" => "subagent_lineage_caller_mismatch",
        "subagent_repeated_ancestor" | "subagent_lineage_cycle" => "subagent_lineage_cycle",
        "subagent_depth_exceeded" => "subagent_depth_exceeded",
        "subagent_dispatch_not_found" => "subagent_dispatch_not_found",
        "subagent_dispatch_transition_invalid" => "subagent_dispatch_transition_invalid",
        "invalid_request" => "invalid_request",
        _ => "conversation_state_unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_after_post_dispatch_requires_a_worker() {
        assert!(!requires_worker(&json!({
            "action": "conversation.message.post",
            "mentionedMembershipIds": ["membership:agent"]
        })));
        assert!(!requires_worker(&json!({
            "action": "conversation.message.post",
            "mentionedMembershipIds": []
        })));
        assert!(requires_worker(&json!({
            "action": "conversation.dispatch.after-post",
            "eventId": "event:1",
            "mentionedMembershipIds": ["membership:agent"]
        })));
        assert!(!requires_worker(&json!({
            "action": "conversation.list",
            "mentionedMembershipIds": ["membership:agent"]
        })));
    }

    #[test]
    fn subagent_host_failures_are_reduced_to_stable_safe_codes() {
        assert_eq!(
            subagent_host_error_code(&anyhow::anyhow!("subagent_duplicate_active_edge: private")),
            "subagent_duplicate_active_edge"
        );
        assert_eq!(
            subagent_host_error_code(&anyhow::anyhow!("database path unavailable")),
            "conversation_state_unavailable"
        );
    }
}
