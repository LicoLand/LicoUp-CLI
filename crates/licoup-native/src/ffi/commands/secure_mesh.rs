// secure-mesh commands: status, envelope validate, command policy/evaluate/execute

use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) fn handle_secure_mesh(admitted: AdmittedCommand) -> Result<CliExecution> {
    let route = admitted.path();
    let params = admitted_params(
        &[
            ("ledgerPath", admitted.option_text("ledger-path")),
            ("trustState", admitted.option_text("trust-state")),
            ("approvedRoot", admitted.option_text("approved-root")),
            ("conflictPolicy", admitted.option_text("conflict-policy")),
            ("userConfirmed", admitted.option_text("user-confirmed")),
            (
                "pendingOperationId",
                admitted.option_text("pending-operation-id"),
            ),
            ("decision", admitted.option_text("decision")),
            (
                "respondingEndpointId",
                admitted.option_text("responding-endpoint-id"),
            ),
            ("responseNonce", admitted.option_text("response-nonce")),
        ],
        &[
            ("payload", admitted.option_json("payload")),
            ("context", admitted.option_json("context")),
            ("identity", admitted.option_json("identity")),
            (
                "previousIdentity",
                admitted.option_json("previous-identity"),
            ),
            ("manifest", admitted.option_json("manifest")),
        ],
        &[],
    );
    let (noun, action) = match route {
        ["secure-mesh", "status"] => ("status", ""),
        ["secure-mesh", noun, action] => (*noun, *action),
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    let result = match (noun, action) {
        ("status", "") => {
            let evaluation =
                crate::domain::mobile_relay::selected_mobile_relay_capability_evaluation()?;
            let mut status =
                crate::core::secure_mesh::protocol_status_with_capability_evaluation(&evaluation)?;
            let authorized = params
                .get("authorize")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            match crate::domain::mobile_relay::e2ee_status(&params) {
                Ok(e2ee_status) => merge_pairwise_projection(&mut status, e2ee_status)?,
                Err(error) if authorized => return Err(error),
                Err(_) => {
                    status["mobileRelayE2eeStatus"] = serde_json::json!({
                        "ok": false,
                        "code": "secure_mesh_pairwise_status_unavailable",
                        "detailsRedacted": true
                    });
                    status["capabilityProjectionSource"] = serde_json::json!("local_only");
                }
            }
            status
        }
        ("envelope", "validate") => {
            let envelope = params
                .get("payload")
                .cloned()
                .unwrap_or_else(|| params.clone());
            crate::core::secure_mesh::validate_envelope(&envelope)?
        }
        ("command", "policy") => crate::core::secure_mesh::command_policy(&params),
        ("command", "evaluate") => {
            let payload = params
                .get("payload")
                .or_else(|| params.get("commandPayload"))
                .or_else(|| params.get("body"))
                .cloned()
                .unwrap_or_else(|| params.clone());
            let context = params
                .get("context")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let mut ledger = crate::core::secure_mesh_command::SecureCommandReplayLedger::default();
            crate::core::secure_mesh_command::evaluate_secure_command_json(
                &payload,
                &context,
                &mut ledger,
            )?
        }
        ("command", "execute") => {
            let payload = params
                .get("payload")
                .or_else(|| params.get("commandPayload"))
                .or_else(|| params.get("body"))
                .cloned()
                .unwrap_or_else(|| params.clone());
            let context = params
                .get("context")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let completed_at = params
                .get("completedAt")
                .or_else(|| params.get("completed_at"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(default_completed_at);
            let ledger_path = params
                .get("ledgerPath")
                .or_else(|| params.get("ledger-path"))
                .and_then(serde_json::Value::as_str)
                .map(std::path::PathBuf::from)
                .map(Ok)
                .unwrap_or_else(
                    crate::domain::secure_mesh_command_runtime::default_secure_command_ledger_path,
                )?;
            let mut ledger =
                crate::core::secure_mesh_command::SecureCommandSqliteReplayLedger::open(
                    ledger_path,
                )?;
            let mut executor =
                crate::domain::secure_mesh_command_runtime::SecureCommandRuntimeExecutor;
            crate::core::secure_mesh_command::execute_secure_command_json(
                &payload,
                &context,
                &mut ledger,
                &mut executor,
                completed_at,
            )?
        }
        ("device-trust", "evaluate") => {
            crate::core::secure_mesh_trust::evaluate_device_trust_policy_json(&params)?
        }
        ("file", "route") => crate::core::secure_mesh_file::evaluate_file_route_json(&params)?,
        ("file", "receive-destination") => {
            crate::core::secure_mesh_file::evaluate_file_receive_destination_json(&params)?
        }
        ("file", "receive-confirmation") => {
            crate::core::secure_mesh_file::evaluate_file_receive_confirmation_json(&params)?
        }
        ("approval", "request") => {
            crate::core::secure_mesh_approval::evaluate_approval_request_json(&params)?
        }
        ("approval", "fanout") => {
            crate::core::secure_mesh_approval::evaluate_approval_fanout_json(&params)?
        }
        ("approval", "respond") => {
            let mut result =
                crate::core::secure_mesh_approval::resolve_approval_response_json(&params)?;
            if result.get("ok").and_then(Value::as_bool) == Some(true) {
                let agent_id = result
                    .get("requesterAgentId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let token = result
                    .get("adapterCallbackTokenRef")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let allow = result.get("decision").and_then(Value::as_str) == Some("allow");
                if matches!(agent_id, "hermes" | "claude-code") && !token.is_empty() {
                    match crate::platform::resolve_native_agent_interaction_approval(token, allow) {
                        Ok(resume) => {
                            if let Some(object) = result.as_object_mut() {
                                object.insert("adapterResume".to_string(), resume);
                            }
                        }
                        Err(code) => {
                            if let Some(object) = result.as_object_mut() {
                                object.insert(
                                    "adapterResume".to_string(),
                                    json!({
                                        "ok": false,
                                        "code": code,
                                        "failClosed": true,
                                    }),
                                );
                            }
                        }
                    }
                }
            }
            result
        }
        ("approval", "inbox") => {
            crate::core::secure_mesh_approval::list_approval_inbox_json(&params)?
        }
        ("approval", "adapter-capability") => {
            crate::core::secure_mesh_approval::evaluate_approval_adapter_capability_json(&params)?
        }
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    Ok(CliExecution::Json(result))
}

fn merge_pairwise_projection(
    status: &mut serde_json::Value,
    e2ee_status: serde_json::Value,
) -> Result<()> {
    let verified_projection = e2ee_status
        .get("capabilityProjection")
        .filter(|projection| projection.is_object())
        .cloned();
    let secure_session_established = e2ee_status
        .get("secureSessionEstablished")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    status["mobileRelayE2eeStatus"] = e2ee_status;
    if secure_session_established {
        if let Some(projection) = verified_projection {
            status["capabilityProjection"] = projection;
            status["capabilityProjectionSource"] =
                serde_json::json!("durable_verified_pairwise_session");
            return Ok(());
        }
    }
    status["capabilityProjectionSource"] = serde_json::json!("local_only");
    Ok(())
}

fn default_completed_at() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::commands::CliExecution;
    use base64::{Engine as _, engine::general_purpose};
    use licoup_foundation::platform::paths::set_portable_data_dir_override;
    use serde_json::{Value, json};
    use std::path::PathBuf;

    /// Unwrap a command result in a test. The command surface is host-facing,
    /// so a non-JSON outcome is a test failure via assertion, never a panic
    /// inside the boundary library.
    fn expect_json(result: CliExecution, expectation: &str) -> Value {
        match result {
            CliExecution::Json(value) => value,
            other => {
                assert!(false, "unexpected CLI result for {expectation}: {other:?}");
                serde_json::Value::Null
            }
        }
    }

    #[test]
    fn secure_mesh_desktop_status_promotes_only_durable_verified_pairwise_projection() {
        let local_projection = json!({"schemaVersion": 2, "source": "local"});
        let verified_projection = json!({"schemaVersion": 2, "source": "verified-pairwise"});
        let mut status = json!({"capabilityProjection": local_projection});
        merge_pairwise_projection(
            &mut status,
            json!({
                "secureSessionEstablished": true,
                "capabilityProjection": verified_projection
            }),
        )
        .unwrap();
        assert_eq!(status["capabilityProjection"], verified_projection);
        assert_eq!(
            status["capabilityProjectionSource"],
            "durable_verified_pairwise_session"
        );

        let mut unconfirmed = json!({"capabilityProjection": local_projection});
        merge_pairwise_projection(
            &mut unconfirmed,
            json!({
                "secureSessionEstablished": false,
                "capabilityProjection": verified_projection
            }),
        )
        .unwrap();
        assert_eq!(unconfirmed["capabilityProjection"], local_projection);
        assert_eq!(unconfirmed["capabilityProjectionSource"], "local_only");
    }

    #[test]
    fn secure_mesh_command_execute_cli_requires_confirmation_before_local_effect() {
        let env = SecureMeshCommandCliTestEnv::new("local-effect-confirmation");
        let mut payload = command_fixture("cmd-a", "idem-a");
        payload["commandKind"] = json!("secure_mesh.device.verify");
        payload["riskClass"] = json!("local_effect");
        payload["targetBinding"]["targetAgentId"] = Value::Null;
        payload["body"] = json!({});
        let mut context = context_fixture();
        context["allowedAgentIds"] = json!([]);
        let result = execute_fixture(&env, payload, context);
        assert_eq!(result["evaluation"]["accepted"], true);
        assert_eq!(result["evaluation"]["shouldExecute"], false);
        assert_eq!(result["evaluation"]["code"], "user_confirmation_required");
        assert_eq!(result["execution"]["outcome"], "error");
        assert_eq!(
            result["execution"]["errorCode"],
            "user_confirmation_required"
        );
        assert_eq!(result["bodyRedacted"], true);
        assert!(result.get("body").is_none());
    }

    #[test]
    fn secure_mesh_command_execute_cli_uses_durable_replay_ledger() {
        let env = SecureMeshCommandCliTestEnv::new("execute-replay");
        let first = execute_fixture(
            &env,
            message_command_fixture("cmd-a", "idem-a"),
            message_context_fixture(),
        );
        assert_eq!(first["evaluation"]["shouldExecute"], true);
        assert_eq!(first["execution"]["outcome"], "error");
        assert_eq!(
            first["execution"]["errorCode"],
            "native_agent_runtime_binding_unavailable"
        );

        let replay = execute_fixture(
            &env,
            message_command_fixture("cmd-a", "idem-a"),
            message_context_fixture(),
        );
        assert_eq!(replay, first);
    }

    #[test]
    fn secure_mesh_command_execute_cli_rejects_high_risk_web_limited_before_runtime() {
        let env = SecureMeshCommandCliTestEnv::new("execute-high-risk");
        let mut payload = command_fixture("cmd-risk", "idem-risk");
        payload["riskClass"] = json!("high_risk");
        payload["senderIdentity"]["endpointKind"] = json!("web_limited");
        let mut context = context_fixture();
        context["senderEndpointKind"] = json!("web_limited");

        let rejected = execute_fixture(&env, payload, context);
        assert_eq!(rejected["evaluation"]["accepted"], false);
        assert_eq!(rejected["evaluation"]["shouldExecute"], false);
        assert_eq!(rejected["evaluation"]["code"], "high_risk_sender_rejected");
        assert_eq!(rejected["execution"]["outcome"], "error");
        assert_eq!(
            rejected["execution"]["errorCode"],
            "high_risk_sender_rejected"
        );
    }

    #[test]
    fn secure_mesh_approval_respond_cli_admits_bound_response_fields() {
        let pending_operation_id = format!("approval-cli-{}", uuid::Uuid::new_v4());
        let responding_endpoint_id = "endpoint-cli-responder";
        let response_nonce = "nonce-cli-response";
        crate::core::secure_mesh_approval::evaluate_approval_request_json(&json!({
            "pendingOperationId": pending_operation_id,
            "requesterAgentId": "codex",
            "targetClientId": "local-client",
            "originEndpointId": "endpoint-cli-origin",
            "riskLevel": "local_effect",
            "displaySummary": "Approve a bounded CLI operation",
            "adapterCallbackTokenRef": "callback-cli-response",
            "adapterStyle": "callback",
            "expiresAt": "2099-01-01T00:00:00Z",
            "responseNonce": response_nonce,
            "requestedTools": ["fs.read"],
            "trustedEndpointIds": [responding_endpoint_id],
        }))
        .unwrap();

        let result = super::super::execute_cli(vec![
            "secure-mesh".into(),
            "approval".into(),
            "respond".into(),
            "--pending-operation-id".into(),
            pending_operation_id,
            "--decision".into(),
            "allow".into(),
            "--responding-endpoint-id".into(),
            responding_endpoint_id.into(),
            "--response-nonce".into(),
            response_nonce.into(),
        ])
        .unwrap();
        let value = expect_json(result, "secure mesh approval respond");

        assert_eq!(value["ok"], true);
        assert_eq!(value["response"]["responseNonceBound"], true);
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains(responding_endpoint_id));
        assert!(!serialized.contains(response_nonce));
    }

    #[test]
    fn secure_mesh_device_trust_evaluate_cli_reports_policy_decision() {
        let identity = identity_fixture_json("desktop_gui:alice", 1, 2);
        let previous = identity_fixture_json("desktop_gui:alice", 3, 4);
        let args = vec![
            "secure-mesh".to_string(),
            "device-trust".to_string(),
            "evaluate".to_string(),
            "--identity".to_string(),
            serde_json::to_string(&identity).unwrap(),
            "--previous-identity".to_string(),
            serde_json::to_string(&previous).unwrap(),
            "--trust-state".to_string(),
            "verified".to_string(),
        ];
        let result = super::super::execute_cli(args).unwrap();
        let value = expect_json(result, "secure mesh device-trust evaluate");
        assert_eq!(
            value["protocolVersion"],
            crate::core::secure_mesh_trust::SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION
        );
        assert_eq!(value["keyChangeDetected"], true);
        assert_eq!(value["trustState"], "key_changed");
        assert_eq!(value["decision"]["allowedForPrekey"], false);
        assert_eq!(value["decision"]["code"], "identity_key_changed");
    }

    #[test]
    fn secure_mesh_file_route_cli_reports_default_route_without_metadata() {
        let manifest = json!({
            "fileId": "file-cli-1",
            "fileName": "launch-plan.pdf",
            "mimeType": "application/pdf",
            "relativePath": "workspace/reports",
            "totalSize": 16,
            "chunkSize": 8,
            "chunkCount": 2
        });
        let args = vec![
            "secure-mesh".to_string(),
            "file".to_string(),
            "route".to_string(),
            "--manifest".to_string(),
            serde_json::to_string(&manifest).unwrap(),
        ];
        let result = super::super::execute_cli(args).unwrap();
        let value = expect_json(result, "secure mesh file route");
        assert_eq!(
            value["route"]["uploadOperation"],
            "secure_mesh.file_chunk.upload"
        );
        assert_eq!(
            value["route"]["fetchOperation"],
            "secure_mesh.file_chunk.fetch"
        );
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains("launch-plan.pdf"));
        assert!(!serialized.contains("application/pdf"));
        assert!(!serialized.contains("workspace/reports"));
    }

    #[test]
    fn secure_mesh_file_receive_destination_cli_redacts_destination_paths() {
        let approved_root = std::env::temp_dir()
            .join("lico-cli-approved-root-canary")
            .join(uuid::Uuid::new_v4().to_string());
        let manifest = json!({
            "fileId": "file-cli-receive-canary",
            "fileName": "private-cli-file-canary.pdf",
            "mimeType": "application/x-cli-canary",
            "relativePath": "phone/private-cli-relative-canary",
            "totalSize": 16,
            "chunkSize": 8,
            "chunkCount": 2
        });
        let args = vec![
            "secure-mesh".to_string(),
            "file".to_string(),
            "receive-destination".to_string(),
            "--manifest".to_string(),
            serde_json::to_string(&manifest).unwrap(),
            "--approved-root".to_string(),
            approved_root.to_string_lossy().to_string(),
        ];
        let result = super::super::execute_cli(args).unwrap();
        let value = expect_json(result, "secure mesh file receive-destination");
        assert_eq!(value["receivePolicy"]["destinationApproved"], true);
        assert_eq!(value["receivePolicy"]["destinationPathRedacted"], true);
        let serialized = serde_json::to_string(&value).unwrap();
        for forbidden in [
            "file-cli-receive-canary",
            "private-cli-file-canary.pdf",
            "application/x-cli-canary",
            "private-cli-relative-canary",
            "lico-cli-approved-root-canary",
            approved_root.to_string_lossy().as_ref(),
        ] {
            assert!(
                !serialized.contains(forbidden),
                "receive destination CLI leaked {forbidden}"
            );
        }
    }

    #[test]
    fn secure_mesh_file_receive_confirmation_cli_requires_user_confirmation_without_auto_open() {
        let approved_root = std::env::temp_dir()
            .join("lico-cli-confirmation-root-canary")
            .join(uuid::Uuid::new_v4().to_string());
        let manifest = json!({
            "fileId": "file-cli-confirmation-canary",
            "fileName": "private-cli-confirmation-canary.pdf",
            "mimeType": "application/x-cli-confirmation-canary",
            "relativePath": "phone/private-cli-confirmation-relative-canary",
            "totalSize": 16,
            "chunkSize": 8,
            "chunkCount": 2
        });
        let args = vec![
            "secure-mesh".to_string(),
            "file".to_string(),
            "receive-confirmation".to_string(),
            "--manifest".to_string(),
            serde_json::to_string(&manifest).unwrap(),
            "--approved-root".to_string(),
            approved_root.to_string_lossy().to_string(),
            "--user-confirmed".to_string(),
            "false".to_string(),
        ];
        let result = super::super::execute_cli(args).unwrap();
        let value = expect_json(result, "secure mesh file receive-confirmation");
        assert_eq!(value["receiveConfirmation"]["required"], true);
        assert_eq!(
            value["receiveConfirmation"]["userVisibleConfirmationRequired"],
            true
        );
        assert_eq!(value["receiveConfirmation"]["writeAllowed"], false);
        assert_eq!(value["receiveConfirmation"]["autoPreviewEnabled"], false);
        assert_eq!(value["receiveConfirmation"]["autoIngestionEnabled"], false);
        let serialized = serde_json::to_string(&value).unwrap();
        for forbidden in [
            "file-cli-confirmation-canary",
            "private-cli-confirmation-canary.pdf",
            "application/x-cli-confirmation-canary",
            "private-cli-confirmation-relative-canary",
            "lico-cli-confirmation-root-canary",
            approved_root.to_string_lossy().as_ref(),
        ] {
            assert!(
                !serialized.contains(forbidden),
                "receive confirmation CLI leaked {forbidden}"
            );
        }
    }

    fn execute_fixture(env: &SecureMeshCommandCliTestEnv, payload: Value, context: Value) -> Value {
        let args = vec![
            "secure-mesh".to_string(),
            "command".to_string(),
            "execute".to_string(),
            "--payload".to_string(),
            serde_json::to_string(&payload).unwrap(),
            "--context".to_string(),
            serde_json::to_string(&context).unwrap(),
            "--ledger-path".to_string(),
            env.ledger_path.display().to_string(),
        ];
        let result = super::super::execute_cli(args).unwrap();
        expect_json(result, "secure mesh command execute")
    }

    fn command_fixture(command_id: &str, idempotency_key: &str) -> Value {
        json!({
            "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "commandId": command_id,
            "commandKind": "agent.sessions.list",
            "senderIdentity": {
                "endpointId": "pc-a",
                "identityFingerprint": "fingerprint-a",
                "trustState": "verified",
                "endpointKind": "desktop_sidecar"
            },
            "targetBinding": {
                "targetEndpointId": "pc-b",
                "targetAgentId": "codex",
                "workspaceId": "workspace-a"
            },
            "riskClass": "read_only",
            "requiresUserConfirmation": false,
            "idempotencyKey": idempotency_key,
            "createdAt": "2026-01-01T00:00:00Z",
            "expiresAt": "2026-01-01T00:10:00Z",
            "body": {"limit": 5}
        })
    }

    fn message_command_fixture(command_id: &str, idempotency_key: &str) -> Value {
        let mut payload = command_fixture(command_id, idempotency_key);
        payload["commandKind"] = json!("agent.message.send");
        payload["riskClass"] = json!("safe_write");
        payload["targetBinding"]["targetAgentId"] = json!("unsupported-fixture-agent");
        payload["body"] = json!({
            "text": "fixture message"
        });
        payload
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
            "allowedAgentIds": ["codex"],
            "now": "2026-01-01T00:01:00Z"
        })
    }

    fn message_context_fixture() -> Value {
        let mut context = context_fixture();
        context["allowedAgentIds"] = json!(["unsupported-fixture-agent"]);
        context
    }

    fn identity_fixture_json(endpoint_id: &str, identity_byte: u8, signing_byte: u8) -> Value {
        json!({
            "endpointId": endpoint_id,
            "identityPublicKey": general_purpose::URL_SAFE_NO_PAD.encode([identity_byte; 32]),
            "signingPublicKey": general_purpose::URL_SAFE_NO_PAD.encode([signing_byte; 32]),
            "rotationEpoch": 1
        })
    }

    struct SecureMeshCommandCliTestEnv {
        root: PathBuf,
        ledger_path: PathBuf,
        previous_portable: Option<PathBuf>,
    }

    impl SecureMeshCommandCliTestEnv {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "lico-secure-mesh-command-cli-{}-{}",
                label,
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let ledger_path = root.join("command-ledger.sqlite");
            let previous_portable = set_portable_data_dir_override(Some(root.join("portable")));
            Self {
                root,
                ledger_path,
                previous_portable,
            }
        }
    }

    impl Drop for SecureMeshCommandCliTestEnv {
        fn drop(&mut self) {
            set_portable_data_dir_override(self.previous_portable.take());
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
