use anyhow::{Context, ensure};
use serde_json::{Value, json};

use super::{
    DeviceTrustLifecycle, apply_device_trust_lifecycle,
    protected_operation::secure_mesh_action_requires_protected_operation_gate,
    redacted_error::unsupported_action_response, request_validation::validate_ffi_json_structure,
};
use crate::ffi::generated::secure_mesh::{
    SecureMeshFailure, SecureMeshFailureCode, SecureMeshRequest, SecureMeshResult,
};

pub fn dispatch_json(request: &Value, unsupported_code: &'static str) -> anyhow::Result<Value> {
    validate_ffi_json_structure(request)?;
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !action.starts_with("secure_mesh.") {
        ensure!(
            request.is_object(),
            "secure mesh native request must be an object"
        );
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        return dispatch_value(action, &params, unsupported_code);
    }
    let request = match SecureMeshRequest::from_value(request.clone()) {
        Ok(request) => request,
        Err(failure) if failure.code == SecureMeshFailureCode::UnsupportedAction => {
            return Ok(unsupported_action_response("", unsupported_code));
        }
        Err(failure) => return Err(anyhow::Error::new(failure)),
    };
    dispatch_request(&request, unsupported_code)
        .map(SecureMeshResult::into_value)
        .map_err(anyhow::Error::new)
}

pub fn dispatch_request(
    request: &SecureMeshRequest,
    unsupported_code: &'static str,
) -> Result<SecureMeshResult, SecureMeshFailure> {
    dispatch_value(request.action.as_str(), &request.params, unsupported_code)
        .map_err(|_| SecureMeshFailure::new(SecureMeshFailureCode::NativeOperationFailed))
        .and_then(SecureMeshResult::from_value)
}

fn dispatch_value(
    action: &str,
    params: &Value,
    unsupported_code: &'static str,
) -> anyhow::Result<Value> {
    if secure_mesh_action_requires_protected_operation_gate(action) {
        crate::domain::mobile_relay::ensure_secure_mesh_protected_operation_allowed()?;
    }
    match action {
        "mobile.relay.config.get" => crate::domain::mobile_relay::config_get(params),
        "mobile.relay.config.set" => crate::domain::mobile_relay::config_set(params),
        "mobile.relay.pairing.claim" => crate::domain::mobile_relay::pairing_claim(params),
        "mobile.relay.pairing.status" => crate::domain::mobile_relay::pairing_status(params),
        "mobile.relay.commands.createSecure" => {
            crate::domain::mobile_relay::command_create_secure(params)
        }
        "mobile.relay.commands.resultSecure" => {
            crate::domain::mobile_relay::command_result_secure(params)
        }
        "mobile.relay.commands.resultReplayProof" => {
            crate::domain::mobile_relay::command_result_replay_proof(params)
        }
        "mobile.relay.e2ee.status" => crate::domain::mobile_relay::e2ee_status(params),
        "secure_mesh.status" => {
            let evaluation =
                crate::domain::mobile_relay::selected_mobile_relay_capability_evaluation()?;
            crate::core::secure_mesh::protocol_status_with_capability_evaluation(&evaluation)
        }
        action if crate::domain::mobile_relay::SECURE_MESH_KT_NATIVE_ACTIONS.contains(&action) => {
            crate::domain::mobile_relay::dispatch_key_transparency_action(action, params)
        }
        action
            if crate::domain::secure_mesh_mls::SECURE_MESH_MLS_NATIVE_ACTIONS.contains(&action) =>
        {
            crate::domain::secure_mesh_mls::dispatch(action, params)
        }
        "secure_mesh.command.execute" => execute_secure_command(params),
        "secure_mesh.deviceTrust.evaluate" => {
            crate::core::secure_mesh_trust::evaluate_device_trust_policy_json(params)
        }
        "secure_mesh.deviceTrust.verifyQr" => {
            crate::core::secure_mesh_trust::evaluate_device_trust_verification_json(params, "qr")
        }
        "secure_mesh.deviceTrust.verifySas" => {
            crate::core::secure_mesh_trust::evaluate_device_trust_verification_json(params, "sas")
        }
        "secure_mesh.deviceTrust.rotate" => {
            dispatch_device_trust_lifecycle(params, super::DeviceTrustLifecycle::Rotate)
        }
        "secure_mesh.deviceTrust.revoke" => {
            dispatch_device_trust_lifecycle(params, super::DeviceTrustLifecycle::Revoke)
        }
        "secure_mesh.deviceTrust.recover" => {
            dispatch_device_trust_lifecycle(params, super::DeviceTrustLifecycle::Recover)
        }
        "secure_mesh.lifecycle.serviceAction" => {
            crate::core::secure_mesh_lifecycle::evaluate_service_action_json(params)
        }
        "secure_mesh.file.route" => crate::core::secure_mesh_file::evaluate_file_route_json(params),
        "secure_mesh.file.receiveDestination" => {
            crate::core::secure_mesh_file::evaluate_file_receive_destination_json(params)
        }
        "secure_mesh.file.receiveConfirmation" => {
            crate::core::secure_mesh_file::evaluate_file_receive_confirmation_json(params)
        }
        "secure_mesh.file.handoffProof" => {
            crate::core::secure_mesh_file::evaluate_file_handoff_proof_json(params)
        }
        "secure_mesh.approval.request" => {
            crate::core::secure_mesh_approval::evaluate_approval_request_json(params)
        }
        "secure_mesh.approval.fanout" => {
            crate::core::secure_mesh_approval::evaluate_approval_fanout_json(params)
        }
        "secure_mesh.approval.respond" => resolve_approval_response(params),
        "secure_mesh.approval.inbox" => {
            crate::core::secure_mesh_approval::list_approval_inbox_json(params)
        }
        "secure_mesh.approval.adapterCapability" => {
            crate::core::secure_mesh_approval::evaluate_approval_adapter_capability_json(params)
        }
        _ => Ok(unsupported_action_response(action, unsupported_code)),
    }
}

/// Answers one lifecycle action, writing its durable half when it carries one.
///
/// The policy result is computed first, exactly as it always was, so a call with
/// no durable evidence keeps the response it always had. Durable evidence is then
/// written through the ledger, and the caller sees both: the policy decision it
/// asked for and what was really persisted. A refused write is reported under
/// `durableActivation` with `persisted: false`, so a refusal can never be read as
/// a completed lifecycle action.
fn dispatch_device_trust_lifecycle(
    params: &Value,
    lifecycle: DeviceTrustLifecycle,
) -> anyhow::Result<Value> {
    let mut response = crate::core::secure_mesh_trust::evaluate_device_trust_lifecycle_json(
        params,
        lifecycle.as_str(),
    )?;
    let Some(outcome) = apply_device_trust_lifecycle(params, lifecycle)? else {
        return Ok(response);
    };
    if let Some(object) = response.as_object_mut() {
        object.insert("durableActivation".to_owned(), outcome);
    }
    Ok(response)
}

fn execute_secure_command(params: &Value) -> anyhow::Result<Value> {
    let payload = params
        .get("payload")
        .context("secure mesh mobile command payload is required")?;
    let context = params
        .get("context")
        .context("secure mesh mobile command context is required")?;
    let completed_at = params
        .get("completedAt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
        });
    let ledger_path =
        crate::domain::secure_mesh_command_runtime::default_secure_command_ledger_path()?;
    let mut ledger =
        crate::core::secure_mesh_command::SecureCommandSqliteReplayLedger::open(ledger_path)?;
    let mut executor = crate::domain::secure_mesh_command_runtime::SecureCommandRuntimeExecutor;
    crate::core::secure_mesh_command::execute_secure_command_json(
        payload,
        context,
        &mut ledger,
        &mut executor,
        completed_at,
    )
}

fn resolve_approval_response(params: &Value) -> anyhow::Result<Value> {
    let mut result = crate::core::secure_mesh_approval::resolve_approval_response_json(params)?;
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(result);
    }
    let agent_id = result
        .get("requesterAgentId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let token = result
        .get("adapterCallbackTokenRef")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let allow = result.get("decision").and_then(Value::as_str) == Some("allow");
    if !matches!(agent_id, "hermes" | "claude-code") || token.is_empty() {
        return Ok(result);
    }
    let adapter_resume =
        match crate::platform::resolve_native_agent_interaction_approval(token, allow) {
            Ok(resume) => resume,
            Err(code) => json!({
                "ok": false,
                "code": code,
                "failClosed": true,
            }),
        };
    if let Some(object) = result.as_object_mut() {
        object.insert("adapterResume".to_string(), adapter_resume);
    }
    Ok(result)
}
