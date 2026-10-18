use super::model::{NormalizedEffectiveSettings, NormalizedExecution, NormalizedFailure};
use super::params::timestamp;
use super::port::{AcpCapabilityFacts, DrivenRun, DriverFailureFacts};
use super::{RUNTIME_SCHEMA_VERSION, RuntimeAdapter, root_cause};
use licoup_agent_adapter_sdk::port::{ExecutionFailure, ExecutionOutcome, ParserRegistration};
use serde_json::{Value, json};

/// One Agent driver failure, in the shape this host's response mapping reads.
///
/// The failure already crossed that Agent's own parser, so this is a field copy
/// and never a second reduction.
fn normalized_failure(failure: &DriverFailureFacts) -> NormalizedFailure {
    NormalizedFailure {
        code: failure.code.clone(),
        message: failure.message.clone(),
        stage: failure.stage.clone(),
        component: failure.component.clone(),
        retryable: failure.retryable,
        recovery: failure.recovery.clone(),
        user_interaction_required: failure.user_interaction_required,
        request_method: failure.request_method.clone(),
        session_id: failure.session_id.clone(),
        thread_id: failure.thread_id.clone(),
        turn_id: failure.turn_id.clone(),
        turn_status: failure.turn_status.clone(),
    }
}

/// One Agent driver failure whose reported thread identity is its session
/// identity.
///
/// Four of the thirteen Agents report the native session in both positions;
/// this keeps that projection explicit rather than folding it into the
/// ordinary one.
fn normalized_failure_session_as_thread(failure: &DriverFailureFacts) -> NormalizedFailure {
    NormalizedFailure {
        thread_id: failure.session_id.clone(),
        ..normalized_failure(failure)
    }
}

/// One Agent driver failure reduced to the shared execution-failure vocabulary,
/// for the Agents whose transitions their own parser derives from the outcome.
fn execution_failure(failure: &DriverFailureFacts) -> ExecutionFailure<'_> {
    ExecutionFailure {
        code: &failure.code,
        stage: &failure.stage,
        message: &failure.message,
    }
}

pub fn execution_response(adapter: RuntimeAdapter, execution: NormalizedExecution) -> Value {
    debug_assert_eq!(execution.driver_id, adapter.driver_id());
    execution_response_named(
        adapter.id(),
        adapter.label(),
        adapter.driver_id(),
        execution,
    )
}

pub fn execution_response_named(
    adapter_id: &str,
    adapter_label: &str,
    driver_id: &str,
    execution: NormalizedExecution,
) -> Value {
    // Vendor frames have already crossed their isolated parser. Downstream
    // receives only the parser-produced closed transition vocabulary.
    let transitions = execution
        .transitions
        .iter()
        .map(licoup_agent_adapter_sdk::Transition::to_json)
        .collect::<Vec<_>>();
    let lifecycle_prefix = transitions
        .iter()
        .filter_map(|transition| {
            (transition.get("kind").and_then(Value::as_str) == Some("lifecycle"))
                .then(|| {
                    transition
                        .get("stage")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    let terminal_transition = transitions
        .iter()
        .rev()
        .find(|transition| {
            matches!(
                transition.get("kind").and_then(Value::as_str),
                Some("failed")
            ) || transition.get("stage").and_then(Value::as_str) == Some("completed")
        })
        .cloned();
    debug_assert_eq!(execution.driver_id, driver_id);
    let verified_native_session_id = if adapter_id == "codex" {
        execution.thread_id.clone()
    } else {
        execution.session_id.clone()
    };
    let native_session_id = execution
        .ok
        .then_some(verified_native_session_id)
        .filter(|value| !value.trim().is_empty());
    let status_code = execution.status_code;
    let error = execution.error.as_ref().map(|failure| {
        let mut error = json!({
            "code": failure.code,
            "message": failure.message,
            "stage": failure.stage,
            "userInteractionRequired": failure.user_interaction_required,
            "requestMethod": failure.request_method,
            "sessionId": failure.session_id,
            "threadId": failure.thread_id,
            "turnId": failure.turn_id,
            "turnStatus": failure.turn_status
        });
        if let Some(component) = failure.component.as_deref() {
            error["component"] = json!(component);
        }
        if let Some(retryable) = failure.retryable {
            error["retryable"] = json!(retryable);
        }
        if let Some(recovery) = failure.recovery.as_deref() {
            error["recovery"] = json!(recovery);
        }
        // Every terminal failure carries its bounded root-cause class; a
        // driver-provided exact recovery wins over the class hint.
        let root_cause = root_cause::classify_root_cause(&root_cause::FailureEvidence {
            code: &failure.code,
            stage: &failure.stage,
            message: &failure.message,
            turn_status: failure.turn_status.as_deref(),
            status_code,
            output_tail: None,
        });
        error["rootCause"] = json!(root_cause.as_str());
        if failure.recovery.is_none() {
            error["recovery"] = json!(root_cause.recovery());
        }
        error
    });
    let stderr = execution
        .error
        .as_ref()
        .map(|failure| failure.message.clone())
        .unwrap_or_default();
    let effective = if native_session_id.is_some() {
        json!({
            "cwd": execution.effective.cwd,
            "model": execution.effective.model,
            "reasoningEffort": execution.effective.reasoning_effort,
            "permissionMode": execution.effective.permission_mode,
            "mode": execution.effective.mode,
            "runtimeAgent": execution.effective.runtime_agent,
            "allowAll": execution.effective.allow_all,
            "sandbox": execution.effective.sandbox,
            "approvalPolicy": execution.effective.approval_policy
        })
    } else {
        json!({
            "cwd": null,
            "model": null,
            "reasoningEffort": null,
            "permissionMode": null,
            "mode": null,
            "runtimeAgent": null,
            "allowAll": null,
            "sandbox": null,
            "approvalPolicy": null
        })
    };
    json!({
        "ok": execution.ok,
        "schemaVersion": RUNTIME_SCHEMA_VERSION,
        "mode": "runtime-adapter",
        "adapterId": adapter_id,
        "adapterLabel": adapter_label,
        "driverId": driver_id,
        "runtimeProtocol": execution.runtime_protocol,
        "agentId": adapter_id,
        "nativeSessionId": native_session_id,
        "sessionId": native_session_id,
        "threadId": execution.thread_id,
        "turnId": execution.turn_id,
        "turnStatus": execution.turn_status,
        "statusCode": execution.status_code,
        "output": execution.output,
        // Child stderr is never returned. This field preserves the old client
        // contract while containing only the driver's fixed sanitized message.
        "stderr": stderr,
        "error": error,
        "events": transitions,
        "lifecyclePrefix": lifecycle_prefix,
        "terminalTransition": terminal_transition,
        "capabilities": execution.capabilities,
        "stdoutTruncated": execution.stdout_truncated,
        "stderrTruncated": execution.stderr_truncated,
        "startedAt": execution.started_at,
        "completedAt": timestamp(),
        "cwd": effective["cwd"],
        "workingDirectory": effective["cwd"],
        "model": effective["model"],
        "reasoningEffort": effective["reasoningEffort"],
        "permissionMode": effective["permissionMode"],
        "sandbox": effective["sandbox"],
        "approvalPolicy": effective["approvalPolicy"],
        "effective": effective,
        "planner": false,
        "clientOwnedToolLoop": false,
        "approvalOwner": "user"
    })
}

pub fn normalize_codex(execution: DrivenRun) -> NormalizedExecution {
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "interactiveApprovalBridge": false
        }),
        error: execution.error.as_ref().map(normalized_failure),
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: "codex-app-server",
    }
}

pub fn normalize_antigravity(execution: DrivenRun) -> NormalizedExecution {
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "interactiveApprovalBridge": false,
            "promptInArguments": true,
            "continuityIdInArguments": true
        }),
        error: execution.error.as_ref().map(normalized_failure),
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            permission_mode: execution.effective.permission_mode,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: execution.driver_id,
    }
}

pub fn normalize_claude(execution: DrivenRun) -> NormalizedExecution {
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "interactiveApprovalBridge": false,
            "processLocalContinuation": true
        }),
        error: execution.error.as_ref().map(normalized_failure),
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            permission_mode: execution.effective.permission_mode,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: "claude-code-stream-json",
    }
}

pub fn normalize_cursor(execution: DrivenRun) -> NormalizedExecution {
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "interactiveApprovalBridge": false,
            "promptInArguments": true,
            "continuityIdInArguments": true
        }),
        error: execution.error.as_ref().map(normalized_failure),
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            permission_mode: execution.effective.permission_mode,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: execution.driver_id,
    }
}

pub fn normalize_acp(adapter: RuntimeAdapter, execution: DrivenRun) -> NormalizedExecution {
    debug_assert_eq!(execution.driver_id, adapter.driver_id());
    let AcpCapabilityFacts {
        protocol_version,
        load_session,
        resume_session,
        close_session,
        list_sessions,
        delete_session,
        image_prompts,
        audio_prompts,
        embedded_context,
    } = execution.acp_capabilities.unwrap_or_default();
    let capabilities = json!({
        "protocolVersion": protocol_version,
        "loadSession": load_session,
        "resumeSession": resume_session,
        "closeSession": close_session,
        "listSessions": list_sessions,
        "deleteSession": delete_session,
        "imagePrompts": image_prompts,
        "audioPrompts": audio_prompts,
        "embeddedContext": embedded_context
    });
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities,
        error: execution.error.as_ref().map(normalized_failure),
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            mode: execution.effective.mode,
            runtime_agent: execution.effective.runtime_agent,
            allow_all: execution.effective.allow_all,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        // The shared ACP engine reports the canonical driver identity from the
        // inventory. Keep the public response bound to that same identity,
        // which is deliberately distinct from the packaged agent id.
        driver_id: adapter.driver_id(),
    }
}

pub fn normalize_openclaw(execution: DrivenRun) -> NormalizedExecution {
    let error = execution
        .error
        .as_ref()
        .map(normalized_failure_session_as_thread);
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "reasoning": true,
            "tools": true,
            "interactiveApprovalBridge": false,
            "modelOverride": false
        }),
        error,
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: "openclaw-acp",
    }
}

/// Normalize one Hermes execution.
///
/// Hermes reports no transition list of its own, so this Agent's registered
/// parser answers it through the shared, protocol-agnostic query: the host
/// names the outcome and never the Agent. `parser` is the registration the
/// composition supplied for this Agent, so the answer travels with the Agent
/// that owns it.
pub fn normalize_hermes(execution: DrivenRun, parser: ParserRegistration) -> NormalizedExecution {
    let failure = execution.error.as_ref().map(execution_failure);
    let transitions = (parser.execution_transitions)(&ExecutionOutcome {
        output: &execution.output,
        failure,
    });
    let error = execution
        .error
        .as_ref()
        .map(normalized_failure_session_as_thread);
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "tools": true,
            "interactiveApprovalBridge": false,
            "modelOverride": true,
            "reasoningOverride": false
        }),
        error,
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: execution.driver_id,
    }
}

pub fn normalize_pi(execution: DrivenRun) -> NormalizedExecution {
    let error = execution
        .error
        .as_ref()
        .map(normalized_failure_session_as_thread);
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "tools": true,
            "interactiveApprovalBridge": false,
            "modelOverride": true,
            "reasoningOverride": true
        }),
        error,
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            permission_mode: execution.effective.permission_mode,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: "pi-rpc",
    }
}

pub fn normalize_lico_agent(execution: DrivenRun) -> NormalizedExecution {
    let error = execution
        .error
        .as_ref()
        .map(normalized_failure_session_as_thread);
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "tools": true,
            "interactiveApprovalBridge": false,
            "modelOverride": true,
            "reasoningOverride": true
        }),
        error,
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            permission_mode: execution.effective.permission_mode,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: "lico-agent-rpc",
    }
}

pub fn normalize_deepseek_harness(execution: DrivenRun) -> NormalizedExecution {
    let error = execution.error.as_ref().map(normalized_failure);
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: execution.transitions,
        capabilities: json!({
            "newSession": true,
            "resumeSession": true,
            "structuredEvents": true,
            "interactiveApprovalBridge": false,
            "cancel": false,
            "interruptSteer": false,
            "history": false,
            "modelOverride": true,
            "reasoningOverride": false
        }),
        error,
        session_id: execution.session_id,
        thread_id: execution.thread_id,
        turn_id: execution.turn_id,
        turn_status: execution.turn_status,
        effective: NormalizedEffectiveSettings {
            cwd: execution.effective.cwd,
            model: execution.effective.model,
            reasoning_effort: execution.effective.reasoning_effort,
            permission_mode: execution.effective.permission_mode,
            sandbox: execution.effective.sandbox,
            approval_policy: execution.effective.approval_policy,
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: execution.driver_id,
    }
}
