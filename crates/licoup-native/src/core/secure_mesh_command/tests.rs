use super::runtime::successful_agent_dispatch;
use super::*;
use crate::core::secure_mesh_crypto::{ContentKey, SecureMeshContentContext};
use crate::core::secure_mesh_response::{open_command_result, seal_command_result};
use serde_json::json;

#[test]
fn secure_mesh_command_gate_accepts_allowlisted_bound_command() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(evaluation.accepted);
    assert!(evaluation.should_execute);
    assert!(!evaluation.replayed);
    assert_eq!(evaluation.code, "execute");
    assert_eq!(evaluation.to_json()["bodyRedacted"], true);
    assert!(evaluation.to_json().get("body").is_none());
}

#[test]
fn agent_sessions_list_params_forward_offset_and_limit() {
    let mut raw = command_fixture();
    raw["commandKind"] = json!("agent.sessions.list");
    raw["riskClass"] = json!("read_only");
    raw["targetBinding"]["targetAgentId"] = json!("codex");
    raw["body"] = json!({
        "limit": 20,
        "offset": 40,
    });
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let params = agent_sessions_list_params(&payload).unwrap();
    assert_eq!(params["agent"], "codex");
    assert_eq!(params["limit"], 20);
    assert_eq!(params["offset"], 40);
}

#[test]
fn agent_sessions_describe_params_require_session_identity() {
    let mut raw = command_fixture();
    raw["commandKind"] = json!("agent.sessions.describe");
    raw["riskClass"] = json!("read_only");
    raw["targetBinding"]["targetAgentId"] = json!("codex");
    raw["body"] = json!({
        "nativeSessionId": "codex-native-exact",
        "messageBefore": "message-50",
        "messageLimit": 100,
    });
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let params = agent_sessions_describe_params(&payload).unwrap();
    assert_eq!(params["agent"], "codex");
    assert_eq!(params["sessionId"], "codex-native-exact");
    assert_eq!(params["limit"], 1);
    assert_eq!(params["offset"], 0);
    assert_eq!(params["messageBefore"], "message-50");
    assert_eq!(params["messageLimit"], 100);

    raw["body"] = json!({});
    let missing = SecureCommandPayload::from_value(&raw).unwrap();
    assert!(agent_sessions_describe_params(&missing).is_err());
}

#[test]
fn secure_mesh_command_gate_executes_exactly_bound_read_only_sessions_describe() {
    let mut raw = command_fixture();
    raw["commandKind"] = json!("agent.sessions.describe");
    raw["riskClass"] = json!("read_only");
    raw["targetBinding"]["targetAgentId"] = json!("codex");
    raw["body"] = json!({
        "sessionId": "codex-native-exact",
    });
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let mut context = context_fixture();
    context["allowedAgentIds"] = json!(["codex"]);
    let context = SecureCommandEvaluationContext::from_value(&context).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(evaluation.accepted);
    assert!(evaluation.should_execute);
    assert_eq!(evaluation.risk_class, "read_only");
    assert_eq!(evaluation.code, "execute");
}

#[test]
fn secure_mesh_command_gate_rejects_web_limited_high_risk() {
    let mut raw = command_fixture();
    raw["riskClass"] = json!("high_risk");
    raw["senderIdentity"]["endpointKind"] = json!("web_limited");
    let mut context = context_fixture();
    context["senderEndpointKind"] = json!("web_limited");
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(!evaluation.accepted);
    assert!(!evaluation.should_execute);
    assert_eq!(evaluation.code, "high_risk_sender_rejected");
}

#[test]
fn secure_mesh_command_gate_rejects_understated_command_risk() {
    let mut raw = command_fixture();
    raw["riskClass"] = json!("read_only");
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();

    assert!(!evaluation.accepted);
    assert!(!evaluation.should_execute);
    assert_eq!(evaluation.code, "risk_class_understated");
}

#[test]
fn secure_mesh_command_gate_requires_user_confirmation_for_protected_operations() {
    let mut raw = command_fixture();
    raw["commandKind"] = json!("secure_mesh.device.verify");
    raw["riskClass"] = json!("local_effect");
    raw["targetBinding"]["targetAgentId"] = Value::Null;
    raw["body"] = json!({});
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let mut context = context_fixture();
    context["allowedAgentIds"] = json!([]);
    let context = SecureCommandEvaluationContext::from_value(&context).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();

    assert!(evaluation.accepted);
    assert!(!evaluation.should_execute);
    assert_eq!(evaluation.code, "user_confirmation_required");
}

#[test]
fn secure_mesh_command_gate_rejects_target_mismatch() {
    let mut raw = command_fixture();
    raw["targetBinding"]["targetEndpointId"] = json!("pc-c");
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(!evaluation.accepted);
    assert_eq!(evaluation.code, "target_endpoint_mismatch");
}

#[test]
fn secure_mesh_command_idempotency_prevents_duplicate_execution() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let first = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(first.should_execute);

    let mut retry_raw = command_fixture();
    retry_raw["commandId"] = json!("cmd-b");
    let retry = SecureCommandPayload::from_value(&retry_raw).unwrap();
    let second = evaluate_secure_command(&retry, &context, &mut ledger).unwrap();
    assert!(second.accepted);
    assert!(!second.should_execute);
    assert!(second.replayed);
    assert_eq!(second.code, "idempotent_replay");
}

#[test]
fn secure_mesh_command_idempotency_conflict_rejected() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let first = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(first.should_execute);

    let mut conflicting_raw = command_fixture();
    conflicting_raw["commandId"] = json!("cmd-c");
    conflicting_raw["body"] = json!({"message": "changed"});
    let conflicting = SecureCommandPayload::from_value(&conflicting_raw).unwrap();
    let second = evaluate_secure_command(&conflicting, &context, &mut ledger).unwrap();
    assert!(!second.accepted);
    assert!(!second.should_execute);
    assert_eq!(second.code, "idempotency_conflict");
}

#[test]
fn secure_mesh_command_replay_command_id_does_not_execute() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let first = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(first.should_execute);

    let mut replay_raw = command_fixture();
    replay_raw["idempotencyKey"] = json!("idem-b");
    let replay = SecureCommandPayload::from_value(&replay_raw).unwrap();
    let second = evaluate_secure_command(&replay, &context, &mut ledger).unwrap();
    assert!(second.accepted);
    assert!(!second.should_execute);
    assert!(second.replayed);
    assert_eq!(second.code, "command_replay_rejected");
}

#[test]
fn secure_mesh_command_schema_rejects_extra_fields() {
    let mut raw = command_fixture();
    raw["plaintext"] = json!("not allowed");
    let error = SecureCommandPayload::from_value(&raw).unwrap_err();
    assert!(error.to_string().contains("unsupported field plaintext"));
}

#[test]
fn secure_mesh_command_sqlite_ledger_survives_reopen_and_bounds_entries() {
    let path = std::env::temp_dir().join(format!(
        "lico-secure-mesh-command-ledger-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();

    {
        let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
        let mut ledger = SecureCommandSqliteReplayLedger::open_with_max_entries(&path, 2).unwrap();
        let first = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
        assert!(first.should_execute);
        assert_eq!(ledger.entry_count().unwrap(), 1);
    }

    {
        let mut ledger = SecureCommandSqliteReplayLedger::open_with_max_entries(&path, 2).unwrap();
        let retry = SecureCommandPayload::from_value(&command_fixture_with(
            "cmd-b",
            "idem-a",
            json!({"message": "hello"}),
        ))
        .unwrap();
        let replay = evaluate_secure_command(&retry, &context, &mut ledger).unwrap();
        assert!(replay.accepted);
        assert!(!replay.should_execute);
        assert!(replay.replayed);
        assert_eq!(replay.code, "idempotent_replay");

        for index in 0..3 {
            let payload = SecureCommandPayload::from_value(&command_fixture_with(
                &format!("cmd-extra-{index}"),
                &format!("idem-extra-{index}"),
                json!({"message": format!("extra-{index}")}),
            ))
            .unwrap();
            let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
            assert!(evaluation.should_execute);
        }
        assert!(ledger.entry_count().unwrap() <= 2);
    }

    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_command_completed_outcome_is_reused_without_duplicate_execution() {
    let mut ledger = SecureCommandReplayLedger::default();
    let mut executor = FixtureExecutor::default();
    let first = execute_secure_command_json(
        &command_fixture(),
        &context_fixture(),
        &mut ledger,
        &mut executor,
        "2026-01-01T00:02:00Z",
    )
    .unwrap();
    let second = execute_secure_command_json(
        &command_fixture(),
        &context_fixture(),
        &mut ledger,
        &mut executor,
        "2026-01-01T00:03:00Z",
    )
    .unwrap();

    assert_eq!(executor.calls, 1);
    assert_eq!(second, first);
    assert_eq!(second["execution"]["outcome"], "result");
}

#[test]
fn secure_mesh_command_sqlite_completed_outcome_survives_reopen() {
    let path = std::env::temp_dir().join(format!(
        "lico-secure-mesh-command-completed-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let first = {
        let mut ledger = SecureCommandSqliteReplayLedger::open(&path).unwrap();
        let mut executor = FixtureExecutor::default();
        let result = execute_secure_command_json(
            &command_fixture(),
            &context_fixture(),
            &mut ledger,
            &mut executor,
            "2026-01-01T00:02:00Z",
        )
        .unwrap();
        assert_eq!(executor.calls, 1);
        result
    };
    let mut reopened = SecureCommandSqliteReplayLedger::open(&path).unwrap();
    let mut executor = FixtureExecutor::default();
    let recovered = execute_secure_command_json(
        &command_fixture(),
        &context_fixture(),
        &mut reopened,
        &mut executor,
        "2026-01-01T00:03:00Z",
    )
    .unwrap();

    assert_eq!(executor.calls, 0);
    assert_eq!(recovered, first);
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_command_reserved_execution_fails_closed_after_reopen() {
    let path = std::env::temp_dir().join(format!(
        "lico-secure-mesh-command-reserved-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    {
        let mut ledger = SecureCommandSqliteReplayLedger::open(&path).unwrap();
        assert!(matches!(
            ledger.record_execution(&payload, context.now).unwrap(),
            SecureCommandReplayRecordStatus::Fresh
        ));
    }
    let mut reopened = SecureCommandSqliteReplayLedger::open(&path).unwrap();
    let mut executor = FixtureExecutor::default();
    let recovered = execute_secure_command_json(
        &command_fixture(),
        &context_fixture(),
        &mut reopened,
        &mut executor,
        "2026-01-01T00:03:00Z",
    )
    .unwrap();

    assert_eq!(executor.calls, 0);
    assert_eq!(recovered["evaluation"]["code"], "execution_outcome_unknown");
    assert_eq!(
        recovered["execution"]["errorCode"],
        "execution_outcome_unknown"
    );
    assert_eq!(recovered["execution"]["retryable"], false);
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_command_execution_wraps_result_payload_after_gate() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(evaluation.should_execute);

    let mut executor = FixtureExecutor::default();
    let outcome = execute_evaluated_secure_command(
        &payload,
        &evaluation,
        &mut executor,
        "2026-01-01T00:02:00Z",
    )
    .unwrap();
    assert_eq!(executor.calls, 1);
    let result = outcome.result().unwrap();
    assert_eq!(result.command_id, "cmd-a");
    assert_eq!(result.idempotency_key, "idem-a");
    assert!(!String::from_utf8_lossy(&result.output).contains("requiresUserConfirmation"));

    let key = ContentKey::from_bytes([31; 32]);
    let encrypted = seal_command_result(&key, &response_context_fixture(), &result).unwrap();
    let opened = open_command_result(&key, &response_context_fixture(), &encrypted).unwrap();
    assert_eq!(opened, result);
}

#[test]
fn secure_mesh_command_execution_does_not_call_executor_for_rejected_gate() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let mut context = context_fixture();
    context["senderRosterActive"] = json!(false);
    let context = SecureCommandEvaluationContext::from_value(&context).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(!evaluation.accepted);
    assert!(!evaluation.should_execute);

    let mut executor = FixtureExecutor::default();
    let outcome = execute_evaluated_secure_command(
        &payload,
        &evaluation,
        &mut executor,
        "2026-01-01T00:02:00Z",
    )
    .unwrap();
    assert_eq!(executor.calls, 0);
    let error = outcome.error().unwrap();
    assert_eq!(error.error_code, "roster_inactive");
    assert!(!error.retryable);
}

#[test]
fn secure_mesh_command_execution_redacts_executor_error_detail() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    let mut executor = FixtureExecutor {
        fail: true,
        failure_detail: "fixture execution failed at <user-home>/private with token=local-secret-canary",
        ..FixtureExecutor::default()
    };
    let outcome = execute_evaluated_secure_command(
        &payload,
        &evaluation,
        &mut executor,
        "2026-01-01T00:02:00Z",
    )
    .unwrap();
    assert_eq!(executor.calls, 1);
    let error = outcome.error().unwrap();
    assert_eq!(error.error_code, "local_execution_failed");
    assert!(error.retryable);
    assert_eq!(error.error_detail, LOCAL_EXECUTION_FAILED_REMOTE_DETAIL);
    assert!(!error.error_detail.contains("<user-home>/private"));
    assert!(!error.error_detail.contains("local-secret-canary"));
}

#[test]
fn secure_mesh_command_denies_unscoped_execution_fields_before_dispatch() {
    let payload = SecureCommandPayload::from_value(&command_fixture_with(
        "cmd-denied-unscoped-execution-fields",
        "idem-denied-unscoped-execution-fields",
        json!({
            "text": "hello",
            "command": "open",
            "args": ["test-data/not-allowed"],
            "cwd": "test-data/not-allowed",
            "env": {"LOCAL_SECRET_CANARY": "must-not-leak"},
            "shell": true
        }),
    ))
    .unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();
    assert!(evaluation.should_execute);

    let mut executor = FilteringExecutor::default();
    let outcome = execute_evaluated_secure_command(
        &payload,
        &evaluation,
        &mut executor,
        "2026-01-01T00:02:00Z",
    )
    .unwrap();
    assert_eq!(executor.runtime_calls, 0);
    let error = outcome.error().unwrap();
    assert_eq!(error.error_code, "local_execution_failed");
    assert_eq!(error.error_detail, LOCAL_EXECUTION_FAILED_REMOTE_DETAIL);
    let serialized = format!("{error:?}");
    assert!(!serialized.contains("test-data/not-allowed"));
    assert!(!serialized.contains("LOCAL_SECRET_CANARY"));
}

#[test]
fn empty_agent_allowlist_never_authorizes_a_bound_agent() {
    let payload = SecureCommandPayload::from_value(&command_fixture()).unwrap();
    let mut context = context_fixture();
    context["allowedAgentIds"] = json!([]);
    let context = SecureCommandEvaluationContext::from_value(&context).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();

    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();

    assert!(!evaluation.accepted);
    assert!(!evaluation.should_execute);
    assert_eq!(evaluation.code, "agent_binding_rejected");
}

#[test]
fn body_agent_selector_cannot_replace_the_authorized_target_binding() {
    let mut raw = command_fixture();
    raw["body"] = json!({
        "agentId": "agent-b",
        "message": "must not dispatch"
    });
    let payload = SecureCommandPayload::from_value(&raw).unwrap();
    let context = SecureCommandEvaluationContext::from_value(&context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();

    let evaluation = evaluate_secure_command(&payload, &context, &mut ledger).unwrap();

    assert!(!evaluation.accepted);
    assert!(!evaluation.should_execute);
    assert_eq!(evaluation.code, "agent_selector_must_use_target_binding");
}

#[test]
fn ready_agent_dispatch_uses_the_shared_conversation_lane() {
    let params = json!({
        "agent": "fixture-ready-agent",
        "text": "fixture message",
        "binaryPath": "/fixture/agent"
    });
    let mut observed_operation = String::new();
    let mut observed_agent = String::new();

    let result = dispatch_ready_agent_message(&params, |operation, dispatched| {
        observed_operation = operation.to_string();
        observed_agent = dispatched["agent"].as_str().unwrap_or_default().to_string();
        Ok(json!({"ok": true, "status": "fixture"}))
    })
    .unwrap();

    assert_eq!(observed_operation, "send");
    assert_eq!(observed_agent, "fixture-ready-agent");
    assert_eq!(result["ok"], true);
}

#[test]
fn nested_runtime_failure_is_not_wrapped_as_secure_mesh_success() {
    let error = successful_agent_dispatch(Ok(json!({
        "ok": false,
        "error": {"code": "fixture_failure"}
    })))
    .unwrap_err();

    assert_eq!(error.to_string(), "native_agent_dispatch_failed");
}

#[test]
fn secure_agent_deadline_is_receiver_owned_and_bounded() {
    let payload = SecureCommandPayload::from_value(&command_fixture_with(
        "cmd-receiver-deadline",
        "idem-receiver-deadline",
        json!({
            "text": "fixture message",
            "model": "model-canary",
            "reasoningEffort": "high"
        }),
    ))
    .unwrap();

    let params = agent_message_send_params(&payload).unwrap();

    assert_eq!(params["timeoutMs"], SECURE_AGENT_MESSAGE_TIMEOUT_MS);
    assert_eq!(params["idempotencyKey"], "idem-receiver-deadline");
    assert_eq!(params["model"], "model-canary");
    assert_eq!(params["reasoningEffort"], "high");
}

#[test]
fn adapter_failure_categories_survive_secure_mesh_redaction() {
    for (adapter_code, expected, retryable) in [
        (
            "acp_session_rejected",
            "native_agent_session_rejected",
            false,
        ),
        ("acp_protocol_timeout", "native_agent_timeout", true),
        (
            "acp_user_interaction_required",
            "native_agent_user_interaction_required",
            true,
        ),
    ] {
        let error = successful_agent_dispatch(Ok(json!({
            "ok": false,
            "error": {"code": adapter_code}
        })))
        .unwrap_err();
        let failure = error.downcast_ref::<SecureAgentDispatchFailure>().unwrap();
        assert_eq!(failure.code, expected);
        assert_eq!(failure.retryable, retryable);
    }
}

#[derive(Default)]
struct FixtureExecutor {
    calls: usize,
    fail: bool,
    failure_detail: &'static str,
}

#[derive(Default)]
struct FilteringExecutor {
    runtime_calls: usize,
}

impl SecureCommandLocalExecutor for FixtureExecutor {
    fn execute_secure_command(&mut self, payload: &SecureCommandPayload) -> Result<Value> {
        self.calls += 1;
        if self.fail {
            return Err(anyhow!(self.failure_detail));
        }
        assert_eq!(payload.command_kind, "agent.message.send");
        Ok(json!({
            "accepted": true,
            "message": payload.body().get("message").and_then(Value::as_str).unwrap_or_default(),
        }))
    }
}

impl SecureCommandLocalExecutor for FilteringExecutor {
    fn execute_secure_command(&mut self, payload: &SecureCommandPayload) -> Result<Value> {
        let _params = agent_message_send_params(payload)?;
        self.runtime_calls += 1;
        Ok(json!({"ok": true}))
    }
}

fn command_fixture_with(command_id: &str, idempotency_key: &str, body: Value) -> Value {
    let mut raw = command_fixture();
    raw["commandId"] = json!(command_id);
    raw["idempotencyKey"] = json!(idempotency_key);
    raw["body"] = body;
    raw
}

fn command_fixture() -> Value {
    json!({
        "schema": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": "cmd-a",
        "commandKind": "agent.message.send",
        "senderIdentity": {
            "endpointId": "pc-a",
            "identityFingerprint": "fingerprint-a",
            "trustState": "verified",
            "endpointKind": "desktop_sidecar"
        },
        "targetBinding": {
            "targetEndpointId": "pc-b",
            "targetAgentId": "agent-a",
            "workspaceId": "workspace-a"
        },
        "riskClass": "safe_write",
        "requiresUserConfirmation": false,
        "idempotencyKey": "idem-a",
        "createdAt": "2026-01-01T00:00:00Z",
        "expiresAt": "2026-01-01T00:10:00Z",
        "body": {"message": "hello"}
    })
}

fn context_fixture() -> Value {
    json!({
        "localEndpointId": "pc-b",
        "senderEndpointId": "pc-a",
        "senderIdentityFingerprint": "fingerprint-a",
        "senderTrustState": "verified",
        "senderEndpointKind": "desktop_sidecar",
        "senderRosterActive": true,
        "targetRosterActive": true,
        "sessionOrEpochValid": true,
        "userConfirmed": false,
        "allowedWorkspaceIds": ["workspace-a"],
        "allowedAgentIds": ["agent-a"],
        "now": "2026-01-01T00:01:00Z"
    })
}

fn response_context_fixture() -> SecureMeshContentContext {
    SecureMeshContentContext::new(
        "env_result",
        "msg_result",
        "mailbox_command",
        "pc-b",
        "pc-a",
        "command_session_test",
        "2026-01-01T00:02:00Z",
        "2026-01-01T00:10:00Z",
    )
}
