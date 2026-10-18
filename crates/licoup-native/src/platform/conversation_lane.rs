//! Conversation dispatch lane operations (open/resume, cancel, capabilities).
//!
//! Extends the existing `runtime_adapters::send_message` path without a parallel
//! executor registry. Protocol families stay selected by `RuntimeAdapter`.

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;

use licoup_agent_drivers::runtime_adapters::{RuntimeAdapter, RuntimeAdapterError};

// The host's own composition of the adapter registry now lives beside this
// module rather than under the moved tree.
use super::runtime_adapters;

#[path = "../domain/client_conversation/settlement.rs"]
mod settlement;
use crate::domain::client_conversation::projection_delta;
use settlement::{
    SettlementDelta, SettlementFailureReason, SettlementOutcome, SettlementSignal,
    TurnSettlementArbiter, send_state_wire, turn_state_wire,
};

// lico-governed-orchestration:start
#[cfg(test)]
mod governed {
    use serde::{Deserialize, Serialize};

    use super::runtime_adapters::protocol_selector::{
        CapabilitySnapshot, PinnedProtocol, ProtocolKind, ProtocolPolicy, SelectionError,
        TargetProtocolRequest, select_pinned_protocol, valid_opaque_evidence, valid_pin,
    };

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    pub struct GovernedConversationRequest {
        pub pin: PinnedProtocol,
        pub input_artifact_handle: String,
        pub input_digest: String,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct GovernedCoordinatorRequest {
        pub target: TargetProtocolRequest,
        pub input_artifact_handle: String,
        pub input_digest: String,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct DispatchBounds {
        pub max_events: usize,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    pub struct SemanticEvent {
        pub sequence: u64,
        pub kind: SemanticEventKind,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum SemanticEventKind {
        Started,
        Progress {
            artifact_handle: String,
            digest: String,
        },
        Completed {
            artifact_handle: String,
            digest: String,
        },
        Failed {
            artifact_handle: String,
            digest: String,
        },
        Cancelled,
    }

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum DispatchDisposition {
        Completed,
        Failed,
        Cancelled,
        Unknown,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AdapterDispatchOutcome {
        pub session_binding: String,
        pub events: Vec<SemanticEvent>,
        pub disposition: DispatchDisposition,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct GovernedDispatchOutcome {
        pub pin: PinnedProtocol,
        pub events: Vec<SemanticEvent>,
        pub disposition: DispatchDisposition,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum AdapterOperationError {
        NoAvailableProtocol,
        PinnedBindingMismatch,
        UnknownOutcome,
        InvalidSemanticEvents,
        EventLimitExceeded,
        SensitiveEvidenceRejected,
    }

    pub trait GovernedConversationAdapter {
        fn adapter_id(&self) -> &str;
        fn protocol(&self) -> ProtocolKind;
        fn driver_id(&self) -> &str;
        fn executable_binding(&self) -> &str;
        fn capability_revision(&self) -> &str;
        fn session_binding(&self) -> &str;
        fn dispatch(
            &mut self,
            request: &GovernedConversationRequest,
        ) -> std::result::Result<AdapterDispatchOutcome, AdapterOperationError>;
        fn cancel(&mut self) -> std::result::Result<(), AdapterOperationError>;
        fn cleanup(&mut self) -> std::result::Result<(), AdapterOperationError>;
    }

    pub struct ResumeCapabilityContext<'a> {
        pub pinned_snapshot: &'a CapabilitySnapshot,
        pub current_snapshot: &'a CapabilitySnapshot,
        pub current_policy: &'a ProtocolPolicy,
    }

    pub fn dispatch_pinned_attempt(
        request: &GovernedConversationRequest,
        adapter: &mut dyn GovernedConversationAdapter,
        bounds: DispatchBounds,
    ) -> std::result::Result<GovernedDispatchOutcome, AdapterOperationError> {
        validate_request(request)?;
        validate_adapter_binding(&request.pin, adapter)?;
        let outcome = adapter.dispatch(request)?;
        if !valid_opaque_evidence(&outcome.session_binding) {
            return Err(AdapterOperationError::SensitiveEvidenceRejected);
        }
        if outcome.session_binding != request.pin.session_binding {
            return Err(AdapterOperationError::PinnedBindingMismatch);
        }
        validate_semantic_events(&outcome.events, outcome.disposition, bounds)?;
        Ok(GovernedDispatchOutcome {
            pin: request.pin.clone(),
            events: outcome.events,
            disposition: outcome.disposition,
        })
    }

    pub fn resume_pinned_attempt(
        request: &GovernedConversationRequest,
        context: &ResumeCapabilityContext<'_>,
        adapter: &mut dyn GovernedConversationAdapter,
        bounds: DispatchBounds,
    ) -> std::result::Result<GovernedDispatchOutcome, AdapterOperationError> {
        // Current evidence and policy are intentionally observed but cannot
        // rewrite a durable attempt binding. Only its original content-bound
        // snapshot can authorize resume.
        let _current_context = (context.current_snapshot.revision(), context.current_policy);
        if !context.pinned_snapshot.contains_pin(&request.pin) {
            return Err(AdapterOperationError::PinnedBindingMismatch);
        }
        dispatch_pinned_attempt(request, adapter, bounds)
    }

    pub fn cancel_pinned_attempt(
        pin: &PinnedProtocol,
        adapter: &mut dyn GovernedConversationAdapter,
    ) -> std::result::Result<(), AdapterOperationError> {
        validate_adapter_binding(pin, adapter)?;
        adapter.cancel()
    }

    pub fn cleanup_pinned_attempt(
        pin: &PinnedProtocol,
        adapter: &mut dyn GovernedConversationAdapter,
    ) -> std::result::Result<(), AdapterOperationError> {
        validate_adapter_binding(pin, adapter)?;
        adapter.cleanup()
    }

    pub fn coordinate_governed_attempt(
        request: &GovernedCoordinatorRequest,
        capabilities: &CapabilitySnapshot,
        policy: &ProtocolPolicy,
        adapters: &mut [&mut dyn GovernedConversationAdapter],
        bounds: DispatchBounds,
    ) -> std::result::Result<GovernedDispatchOutcome, AdapterOperationError> {
        if !valid_opaque_evidence(&request.input_artifact_handle)
            || !valid_opaque_evidence(&request.input_digest)
        {
            return Err(AdapterOperationError::SensitiveEvidenceRejected);
        }
        let pin =
            select_pinned_protocol(&request.target, capabilities, policy).map_err(|error| {
                match error {
                    SelectionError::NoAvailableProtocol => {
                        AdapterOperationError::NoAvailableProtocol
                    }
                    SelectionError::InvalidOpaqueBinding => {
                        AdapterOperationError::SensitiveEvidenceRejected
                    }
                }
            })?;
        let selected = adapters
            .iter_mut()
            .find(|adapter| adapter_matches(&pin, &***adapter))
            .ok_or(AdapterOperationError::PinnedBindingMismatch)?;
        let dispatch_request = GovernedConversationRequest {
            pin: pin.clone(),
            input_artifact_handle: request.input_artifact_handle.clone(),
            input_digest: request.input_digest.clone(),
        };
        match dispatch_pinned_attempt(&dispatch_request, &mut **selected, bounds) {
            Err(AdapterOperationError::UnknownOutcome) => Ok(GovernedDispatchOutcome {
                pin,
                events: Vec::new(),
                disposition: DispatchDisposition::Unknown,
            }),
            result => result,
        }
    }

    fn validate_request(
        request: &GovernedConversationRequest,
    ) -> std::result::Result<(), AdapterOperationError> {
        // A pin is immutable dispatch authority. Any malformed field or failed
        // content binding therefore means the caller no longer holds the exact
        // pinned authority, rather than that its task payload leaked evidence.
        if !valid_pin(&request.pin) {
            return Err(AdapterOperationError::PinnedBindingMismatch);
        }
        if !valid_opaque_evidence(&request.input_artifact_handle)
            || !valid_opaque_evidence(&request.input_digest)
        {
            return Err(AdapterOperationError::SensitiveEvidenceRejected);
        }
        Ok(())
    }

    fn validate_adapter_binding(
        pin: &PinnedProtocol,
        adapter: &dyn GovernedConversationAdapter,
    ) -> std::result::Result<(), AdapterOperationError> {
        if valid_pin(pin) && adapter_matches(pin, adapter) {
            Ok(())
        } else {
            Err(AdapterOperationError::PinnedBindingMismatch)
        }
    }

    fn adapter_matches(pin: &PinnedProtocol, adapter: &dyn GovernedConversationAdapter) -> bool {
        adapter.adapter_id() == pin.adapter_id
            && adapter.protocol() == pin.protocol
            && adapter.driver_id() == pin.driver_id
            && adapter.executable_binding() == pin.executable_binding
            && adapter.capability_revision() == pin.capability_revision
            && adapter.session_binding() == pin.session_binding
    }

    fn validate_semantic_events(
        events: &[SemanticEvent],
        disposition: DispatchDisposition,
        bounds: DispatchBounds,
    ) -> std::result::Result<(), AdapterOperationError> {
        if bounds.max_events == 0 || events.len() > bounds.max_events {
            return Err(AdapterOperationError::EventLimitExceeded);
        }
        if disposition == DispatchDisposition::Unknown {
            return if events.is_empty() {
                Ok(())
            } else {
                Err(AdapterOperationError::InvalidSemanticEvents)
            };
        }
        if events.is_empty()
            || events
                .iter()
                .enumerate()
                .any(|(index, event)| event.sequence != index as u64 + 1)
        {
            return Err(AdapterOperationError::InvalidSemanticEvents);
        }
        let mut terminal = None;
        for (index, event) in events.iter().enumerate() {
            match &event.kind {
                SemanticEventKind::Started => {}
                SemanticEventKind::Progress {
                    artifact_handle,
                    digest,
                }
                | SemanticEventKind::Completed {
                    artifact_handle,
                    digest,
                }
                | SemanticEventKind::Failed {
                    artifact_handle,
                    digest,
                } => {
                    if !valid_opaque_evidence(artifact_handle) || !valid_opaque_evidence(digest) {
                        return Err(AdapterOperationError::SensitiveEvidenceRejected);
                    }
                    if matches!(
                        event.kind,
                        SemanticEventKind::Completed { .. } | SemanticEventKind::Failed { .. }
                    ) {
                        if terminal.replace(index).is_some() {
                            return Err(AdapterOperationError::InvalidSemanticEvents);
                        }
                    }
                }
                SemanticEventKind::Cancelled => {
                    if terminal.replace(index).is_some() {
                        return Err(AdapterOperationError::InvalidSemanticEvents);
                    }
                }
            }
        }
        let Some(terminal_index) = terminal else {
            return Err(AdapterOperationError::InvalidSemanticEvents);
        };
        if terminal_index + 1 != events.len() {
            return Err(AdapterOperationError::InvalidSemanticEvents);
        }
        let disposition_matches = matches!(
            (&events[terminal_index].kind, disposition),
            (
                SemanticEventKind::Completed { .. },
                DispatchDisposition::Completed
            ) | (
                SemanticEventKind::Failed { .. },
                DispatchDisposition::Failed
            ) | (SemanticEventKind::Cancelled, DispatchDisposition::Cancelled)
        );
        if !disposition_matches {
            return Err(AdapterOperationError::InvalidSemanticEvents);
        }
        Ok(())
    }
}
#[cfg(test)]
pub use governed::*;
// lico-governed-orchestration:end

// The driver inventory moved to `licoup-agent-drivers` with the registry that
// embeds it; this reads that crate's embedded copy rather than a second copy of
// the packaged JSON.
const CONVERSATION_DRIVER_INVENTORY_JSON: &str =
    licoup_agent_drivers::runtime_adapters::registry::DRIVER_INVENTORY_JSON;
static CAPABILITY_MATRIX_BY_AGENT: LazyLock<HashMap<String, Value>> = LazyLock::new(|| {
    serde_json::from_str::<Value>(CONVERSATION_DRIVER_INVENTORY_JSON)
        .ok()
        .and_then(|document| document.get("drivers").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|driver| {
            let agent_id = driver.get("agentId")?.as_str()?.to_owned();
            let matrix = driver.get("capabilityMatrix")?.clone();
            Some((agent_id, matrix))
        })
        .collect()
});

/// Official lane family for an adapter (Architecture strategy key).
pub fn lane_family(adapter: RuntimeAdapter) -> &'static str {
    CAPABILITY_MATRIX_BY_AGENT
        .get(adapter.id())
        .and_then(|matrix| matrix.get("laneFamily"))
        .and_then(Value::as_str)
        .unwrap_or("unavailable")
}

/// The packaged driver inventory entry for one Agent's own lane. `None` means
/// the inventory has no readable entry for the Agent, so a fact this owner
/// owns is unknown rather than false.
fn declared_capability_matrix(adapter: RuntimeAdapter) -> Option<&'static Value> {
    CAPABILITY_MATRIX_BY_AGENT.get(adapter.id())
}

/// Read one declared driver-inventory capability flag for an Agent's own lane.
/// Every inventory-owned answer goes through this single reader, so a fact such
/// as `multimodal` has one owner instead of a second copy of the packaged JSON.
pub fn declared_capability_flag(adapter: RuntimeAdapter, flag: &str) -> Option<bool> {
    declared_capability_matrix(adapter)
        .and_then(|matrix| matrix.get(flag))
        .and_then(Value::as_bool)
}

/// Static capability matrix aligned with Evidence.md / drivers inventory.
/// Field names avoid reducer-sensitive fragments (session/path/argv/…).
/// `approvals` means an end-to-end client response bridge, not merely that the
/// native protocol can report and fail closed on an interaction request.
pub fn static_capability_matrix(adapter: RuntimeAdapter) -> Value {
    declared_capability_matrix(adapter)
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "laneFamily": "unavailable",
                "openNew": false,
                "exactResume": false,
                "streaming": false,
                "cancel": false,
                "interruptSteer": false,
                "structuredEvents": false,
                "approvals": false,
                "multimodal": false,
                "usageStatus": false,
                "officialLane": false
            })
        })
}

fn agent_id_param(params: &Value) -> Result<String> {
    runtime_adapters::text_param_public(params, &["agent", "agentId", "target"])
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("agent conversation request requires an agent identifier"))
}

fn adapter_or_err(agent_id: &str) -> Result<RuntimeAdapter> {
    runtime_adapters::install();
    runtime_adapters::adapter_for_agent_public(agent_id)
        .ok_or_else(|| anyhow!("unsupported runtime adapter: {}", agent_id))
}

/// Open or resume a native conversation binding without sending a prompt.
pub fn open_or_resume(params: &Value) -> Result<Value> {
    let agent_id = agent_id_param(params)?;
    let adapter = adapter_or_err(&agent_id)?;
    let runtime_connection =
        super::virtual_machine::SshRuntimeConnection::from_params(params, adapter.id())
            .map_err(|error| anyhow!(error.code()))?;
    let native_id = runtime_adapters::text_param_public(params, &["sessionId", "nativeSessionId"])
        .unwrap_or_default();
    let uses_hermes_gateway = runtime_connection
        .as_ref()
        .is_some_and(super::virtual_machine::SshRuntimeConnection::is_hermes_tui_gateway);
    let mut matrix = static_capability_matrix(adapter);
    if uses_hermes_gateway && let Some(matrix) = matrix.as_object_mut() {
        matrix.insert("laneFamily".to_string(), json!("rpc"));
        matrix.insert("cancel".to_string(), json!(false));
        matrix.insert("interruptSteer".to_string(), json!(false));
    }
    let effective_lane_family = if uses_hermes_gateway {
        "rpc"
    } else {
        lane_family(adapter)
    };
    let effective_runtime_protocol = if uses_hermes_gateway {
        licoup_agent_hermes::tui_gateway::RUNTIME_PROTOCOL
    } else {
        adapter.runtime_protocol()
    };
    let profile = runtime_adapters::runtime_driver_profile(&agent_id);
    let blocker = profile.as_ref().and_then(|p| p.blocker.clone());
    let driver_mode = profile
        .as_ref()
        .map(|p| p.driver_status.clone())
        .unwrap_or_else(|| "unknown".to_string());

    if effective_lane_family == "unavailable" {
        return Ok(json!({
            "ok": false,
            "agentId": adapter.id(),
            "laneFamily": effective_lane_family,
            "driverId": adapter.driver_id(),
            "runtimeProtocol": effective_runtime_protocol,
            "capabilities": matrix,
            "error": {
                "code": blocker.unwrap_or_else(|| "official_conversation_transport_unavailable".to_string()),
                "stage": "capability/transport",
                "message": "No official public conversation transport is available for this adapter."
            }
        }));
    }

    let mut serve_status = Value::Null;
    let mut gateway_status = Value::Null;
    if adapter == RuntimeAdapter::OpenCode {
        // Init/bootstrap owns auto-start. Open discloses serve state and may
        // refresh attach metadata, but must not fail the binding when the
        // binary is absent — send/execute still fail-closed via ensure_attach.
        serve_status = crate::platform::opencode_serve::status(&json!({}))
            .or_else(|_| crate::platform::opencode_serve::ensure(&json!({})))
            .unwrap_or_else(|_| {
                json!({
                    "ok": false,
                    "status": "unavailable",
                    "running": false,
                    "healthy": false,
                    "errorCode": "opencode_serve_unavailable"
                })
            });
        if serve_status.get("running").and_then(Value::as_bool) != Some(true) {
            if let Ok(ensured) = crate::platform::opencode_serve::ensure(&json!({
                "healthTimeoutMs": 8_000
            })) {
                serve_status = ensured;
            }
        }
    }
    if adapter == RuntimeAdapter::OpenClaw && runtime_connection.is_none() {
        // Prefer vendor Gateway attach/reuse (18789); never steal that port.
        // Disclose gateway state on open; send still fail-closes via ensure.
        gateway_status = crate::platform::openclaw_gateway::ensure(&json!({
            "healthTimeoutMs": 8_000
        }))
        .unwrap_or_else(|_| {
            json!({
                "ok": false,
                "status": "unavailable",
                "running": false,
                "healthy": false,
                "errorCode": "openclaw_gateway_unavailable",
                "vendorDefaultPort": crate::platform::openclaw_gateway::VENDOR_DEFAULT_PORT
            })
        });
    } else if adapter == RuntimeAdapter::OpenClaw {
        gateway_status = json!({
            "ok": true,
            "status": "deferred",
            "running": false,
            "healthy": false,
            "attachMode": "ssh-stdio",
            "hostClass": "virtual-machine"
        });
    }

    if !native_id.is_empty() {
        let exact_resume = matrix.get("exactResume").and_then(Value::as_bool) == Some(true);
        if !exact_resume {
            let code = blocker.unwrap_or_else(|| "exact_session_resume_unavailable".to_string());
            return Ok(json!({
                "ok": false,
                "agentId": adapter.id(),
                "laneFamily": effective_lane_family,
                "driverId": adapter.driver_id(),
                "runtimeProtocol": effective_runtime_protocol,
                "capabilities": matrix,
                "gateway": gateway_status,
                "error": {
                    "code": code,
                    "stage": "session/resume",
                    "message": "Exact native resume is not available on an official lane for this adapter."
                }
            }));
        }
        // Claude Code resumes a native conversation by launching a fresh
        // --resume process when no process-local live transport owns it; send
        // still fails closed on an unknown or diverged conversation.
    }

    Ok(json!({
        "ok": true,
        "agentId": adapter.id(),
        "laneFamily": effective_lane_family,
        "driverId": adapter.driver_id(),
        "runtimeProtocol": effective_runtime_protocol,
        "nativeSessionId": native_id,
        "sessionId": native_id,
        "threadId": native_id,
        "openMode": if native_id.is_empty() { "new" } else { "resume" },
        "driverStatus": driver_mode,
        "capabilities": matrix,
        "serve": serve_status,
        "gateway": gateway_status,
        "events": []
    }))
}

/// Cancel an in-flight turn when the canonical driver owns a supervised,
/// process-local active-turn handle. Other adapters fail closed.
pub fn cancel_turn(params: &Value) -> Result<Value> {
    let agent_id = agent_id_param(params)?;
    let adapter = adapter_or_err(&agent_id)?;
    let matrix = static_capability_matrix(adapter);

    if lane_family(adapter) == "unavailable" {
        return Ok(json!({
            "ok": false,
            "agentId": adapter.id(),
            "laneFamily": lane_family(adapter),
            "status": "blocked",
            "error": {
                "code": "official_conversation_transport_unavailable",
                "stage": "turn/cancel",
                "message": "Cancel is unavailable because no official conversation transport exists."
            },
            "capabilities": matrix
        }));
    }

    if matches!(
        adapter,
        RuntimeAdapter::Codex
            | RuntimeAdapter::Hermes
            | RuntimeAdapter::ClaudeCode
            | RuntimeAdapter::Cursor
            | RuntimeAdapter::Antigravity
            | RuntimeAdapter::OpenCode
            | RuntimeAdapter::KiloCode
            | RuntimeAdapter::OpenClaw
            | RuntimeAdapter::Copilot
            | RuntimeAdapter::KimiCode
    ) {
        let session_id =
            runtime_adapters::text_param_public(params, &["sessionId", "nativeSessionId"])
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow!("agent cancel requires an exact native session identifier")
                })?;
        let disposition = match adapter {
            RuntimeAdapter::Codex => {
                match licoup_agent_codex::app_server::driver::active_control::interrupt(&session_id) {
                    licoup_agent_codex::app_server::driver::active_control::ControlDisposition::Accepted => 0,
                    licoup_agent_codex::app_server::driver::active_control::ControlDisposition::NoActiveTurn => 1,
                    licoup_agent_codex::app_server::driver::active_control::ControlDisposition::SessionUnavailable => 2,
                    licoup_agent_codex::app_server::driver::active_control::ControlDisposition::TransportUnavailable => 3,
                }
            }
            RuntimeAdapter::ClaudeCode => match licoup_agent_claude_code::driver::cancel(&session_id) {
                licoup_agent_claude_code::driver::ControlDisposition::Accepted => 0,
                licoup_agent_claude_code::driver::ControlDisposition::NoActiveTurn => 1,
                licoup_agent_claude_code::driver::ControlDisposition::SessionUnavailable => 2,
                licoup_agent_claude_code::driver::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::Cursor => match licoup_agent_cursor::driver::cancel(&session_id) {
                licoup_agent_cursor::driver::ControlDisposition::Accepted => 0,
                licoup_agent_cursor::driver::ControlDisposition::NoActiveTurn => 1,
                licoup_agent_cursor::driver::ControlDisposition::NotPersisted
                | licoup_agent_cursor::driver::ControlDisposition::SessionUnavailable => 2,
                licoup_agent_cursor::driver::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::Antigravity => match licoup_agent_antigravity::driver::cancel(&session_id) {
                licoup_agent_antigravity::driver::ControlDisposition::Accepted => 0,
                licoup_agent_antigravity::driver::ControlDisposition::NoActiveTurn => 1,
                licoup_agent_antigravity::driver::ControlDisposition::NotPersisted
                | licoup_agent_antigravity::driver::ControlDisposition::SessionUnavailable => 2,
                licoup_agent_antigravity::driver::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::Hermes => match licoup_agent_hermes::driver::cancel(&session_id) {
                super::acp_session_transport::ControlDisposition::Accepted => 0,
                super::acp_session_transport::ControlDisposition::NoActiveTurn => 1,
                super::acp_session_transport::ControlDisposition::SessionUnavailable => 2,
                super::acp_session_transport::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::OpenCode => match licoup_agent_opencode::driver::cancel(&session_id) {
                super::local_service::turn_control::ControlDisposition::Accepted => 0,
                super::local_service::turn_control::ControlDisposition::NoActiveTurn => 1,
                super::local_service::turn_control::ControlDisposition::SessionUnavailable => 2,
                super::local_service::turn_control::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::KiloCode => match super::kilo_code_host::cancel(&session_id) {
                super::local_service::turn_control::ControlDisposition::Accepted => 0,
                super::local_service::turn_control::ControlDisposition::NoActiveTurn => 1,
                super::local_service::turn_control::ControlDisposition::SessionUnavailable => 2,
                super::local_service::turn_control::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::OpenClaw => match licoup_agent_openclaw::driver::cancel(&session_id) {
                super::acp_driver_runtime::ControlDisposition::Accepted => 0,
                super::acp_driver_runtime::ControlDisposition::NoActiveTurn => 1,
                super::acp_driver_runtime::ControlDisposition::SessionUnavailable => 2,
                super::acp_driver_runtime::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::Copilot => match licoup_agent_copilot::driver::cancel(&session_id) {
                super::acp_driver_runtime::ControlDisposition::Accepted => 0,
                super::acp_driver_runtime::ControlDisposition::NoActiveTurn => 1,
                super::acp_driver_runtime::ControlDisposition::SessionUnavailable => 2,
                super::acp_driver_runtime::ControlDisposition::TransportUnavailable => 3,
            },
            RuntimeAdapter::KimiCode => match licoup_agent_kimi::driver::cancel(&session_id) {
                super::acp_driver_runtime::ControlDisposition::Accepted => 0,
                super::acp_driver_runtime::ControlDisposition::NoActiveTurn => 1,
                super::acp_driver_runtime::ControlDisposition::SessionUnavailable => 2,
                super::acp_driver_runtime::ControlDisposition::TransportUnavailable => 3,
            },
            _ => 3,
        };
        let prefix = match adapter {
            RuntimeAdapter::Codex => "codex",
            RuntimeAdapter::ClaudeCode => "claude_code",
            RuntimeAdapter::Cursor => "cursor",
            RuntimeAdapter::Antigravity => "antigravity",
            RuntimeAdapter::Hermes => "hermes",
            RuntimeAdapter::OpenCode => "opencode",
            RuntimeAdapter::KiloCode => "kilo_code",
            RuntimeAdapter::OpenClaw => "openclaw",
            RuntimeAdapter::Copilot => "copilot",
            RuntimeAdapter::KimiCode => "kimi_code",
            _ => "agent",
        };
        let label = match adapter {
            RuntimeAdapter::Codex => "Codex App Server",
            RuntimeAdapter::ClaudeCode => "Claude Code",
            RuntimeAdapter::Cursor => "Cursor CLI",
            RuntimeAdapter::Antigravity => "Antigravity CLI",
            RuntimeAdapter::Hermes => "Hermes ACP",
            RuntimeAdapter::OpenCode => "OpenCode Local Service",
            RuntimeAdapter::KiloCode => "Kilo Local Service",
            RuntimeAdapter::OpenClaw => "OpenClaw ACP",
            RuntimeAdapter::Copilot => "GitHub Copilot ACP",
            RuntimeAdapter::KimiCode => "Kimi Code ACP",
            _ => "Agent",
        };
        let (ok, status, code, stage, message) = match disposition {
            0 => (
                true,
                "cancel_requested",
                Value::Null,
                "turn/cancel",
                "The official lane accepted cancellation for the active native turn.",
            ),
            1 => (
                false,
                "not_active",
                json!(format!("{prefix}_turn_not_active")),
                "turn/cancel",
                "The selected native session has no active turn.",
            ),
            2 => (
                false,
                "not_found",
                json!(format!("{prefix}_session_unavailable")),
                "turn/cancel",
                "The selected native session is not bound to this client process.",
            ),
            _ if adapter == RuntimeAdapter::OpenCode => (
                false,
                "unavailable",
                json!("opencode_serve_control_failed"),
                "turn/control",
                "The OpenCode control endpoint failed while controlling the turn.",
            ),
            _ => (
                false,
                "unavailable",
                json!(format!("{prefix}_cancel_transport_unavailable")),
                "turn/cancel",
                "The supervised native cancel channel is unavailable.",
            ),
        };
        return Ok(json!({
            "ok": ok,
            "agentId": adapter.id(),
            "laneFamily": lane_family(adapter),
            "status": status,
            "transport": label,
            "error": if ok { Value::Null } else { json!({
                "code": code,
                "stage": stage,
                "message": message
            }) },
            "capabilities": matrix
        }));
    }

    Ok(json!({
        "ok": false,
        "agentId": adapter.id(),
        "laneFamily": lane_family(adapter),
        "status": "unsupported",
        "error": {
            "code": "dispatch_cancel_unsupported",
            "stage": "turn/cancel",
            "message": "This official lane does not expose a supervised product cancel channel."
        },
        "capabilities": matrix
    }))
}

/// Reclaim adapter-owned conversation resources without deleting arbitrary
/// user history. Cleanup stops only the supervised transport bound to the
/// exact native session in this client process.
pub fn cleanup_conversation(params: &Value) -> Result<Value> {
    let agent_id = agent_id_param(params)?;
    let adapter = adapter_or_err(&agent_id)?;
    let matrix = static_capability_matrix(adapter);
    if !matches!(
        adapter,
        RuntimeAdapter::Hermes
            | RuntimeAdapter::ClaudeCode
            | RuntimeAdapter::Cursor
            | RuntimeAdapter::Antigravity
            | RuntimeAdapter::DeepSeekHarness
    ) {
        return Ok(json!({
            "ok": false,
            "agentId": adapter.id(),
            "laneFamily": lane_family(adapter),
            "status": "unsupported",
            "error": {
                "code": "dispatch_cleanup_unsupported",
                "stage": "session/cleanup",
                "message": "This lane has no process-local supervised cleanup channel."
            },
            "capabilities": matrix
        }));
    }
    let session_id = runtime_adapters::text_param_public(params, &["sessionId", "nativeSessionId"])
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("cleanup requires an exact native session identifier"))?;
    let disposition = match adapter {
        RuntimeAdapter::ClaudeCode => {
            match licoup_agent_claude_code::driver::cleanup_session(&session_id) {
                licoup_agent_claude_code::driver::ControlDisposition::Accepted => 0,
                licoup_agent_claude_code::driver::ControlDisposition::SessionUnavailable => 1,
                _ => 2,
            }
        }
        RuntimeAdapter::Cursor => match licoup_agent_cursor::driver::cleanup_session(&session_id) {
            licoup_agent_cursor::driver::ControlDisposition::Accepted => 0,
            licoup_agent_cursor::driver::ControlDisposition::NotPersisted => 3,
            licoup_agent_cursor::driver::ControlDisposition::SessionUnavailable => 1,
            _ => 2,
        },
        RuntimeAdapter::Antigravity => {
            match licoup_agent_antigravity::driver::cleanup_session(&session_id) {
                licoup_agent_antigravity::driver::ControlDisposition::Accepted => 0,
                licoup_agent_antigravity::driver::ControlDisposition::NotPersisted => 3,
                licoup_agent_antigravity::driver::ControlDisposition::SessionUnavailable => 1,
                _ => 2,
            }
        }
        RuntimeAdapter::DeepSeekHarness => {
            match licoup_agent_deepseek::driver::cleanup_session(&session_id) {
                licoup_agent_deepseek::driver::CleanupDisposition::Accepted => 0,
                licoup_agent_deepseek::driver::CleanupDisposition::SessionUnavailable => 1,
                licoup_agent_deepseek::driver::CleanupDisposition::Unavailable => 2,
            }
        }
        _ => match licoup_agent_hermes::driver::cleanup_session(&session_id) {
            super::acp_session_transport::ControlDisposition::Accepted => 0,
            super::acp_session_transport::ControlDisposition::SessionUnavailable => 1,
            _ => 2,
        },
    };
    let prefix = match adapter {
        RuntimeAdapter::ClaudeCode => "claude_code",
        RuntimeAdapter::Cursor => "cursor",
        RuntimeAdapter::Antigravity => "antigravity",
        RuntimeAdapter::DeepSeekHarness => "deepseek_harness",
        _ => "hermes",
    };
    let (ok, status, code) = match disposition {
        0 => (true, "cleaned", Value::Null),
        1 => (
            false,
            "not_found",
            json!(format!("{prefix}_session_unavailable")),
        ),
        3 => (true, "not_persisted", Value::Null),
        _ => (
            false,
            "unavailable",
            json!(format!("{prefix}_cleanup_transport_unavailable")),
        ),
    };
    if adapter == RuntimeAdapter::ClaudeCode && disposition == 2 {
        return Err(anyhow!("process_local_shutdown_failed"));
    }
    Ok(json!({
        "ok": ok,
        "agentId": adapter.id(),
        "laneFamily": lane_family(adapter),
        "status": status,
        "error": if ok { Value::Null } else { json!({
            "code": code,
            "stage": "session/cleanup",
            "message": "The selected supervised transport could not be cleaned."
        }) },
        "capabilities": matrix
    }))
}

/// Return a backward page from the complete transcript owned by the exact live
/// process-local Claude transport. No filesystem history discovery is performed.
pub fn process_local_history(params: &Value) -> Result<Value> {
    let agent_id = agent_id_param(params)?;
    let adapter = adapter_or_err(&agent_id)?;
    if adapter != RuntimeAdapter::ClaudeCode {
        return Ok(json!({
            "ok": false,
            "error": {
                "code": "process_local_history_unsupported",
                "stage": "session/history",
                "message": "This adapter has no process-local transcript projection."
            }
        }));
    }
    let session_id = runtime_adapters::text_param_public(params, &["sessionId", "nativeSessionId"])
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("history requires an exact native session identifier"))?;
    let before = params
        .get("before")
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| anyhow!("history before cursor must be an unsigned integer"))
        })
        .transpose()?;
    let limit = params
        .get("limit")
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| (1..=100).contains(value))
                .ok_or_else(|| anyhow!("history page limit must be between 1 and 100"))
        })
        .transpose()?
        .unwrap_or(50);
    let Some(mut history) = licoup_agent_claude_code::driver::history(&session_id, before, limit)
    else {
        return Ok(json!({
            "ok": false,
            "error": {
                "code": "claude_code_session_unavailable",
                "stage": "session/history",
                "message": "The exact process-local transcript is unavailable."
            }
        }));
    };
    if let Some(object) = history.as_object_mut() {
        object.insert("ok".to_string(), json!(true));
    }
    Ok(history)
}

/// Steer is intentionally a distinct lane operation. No packaged driver may
/// alias cancel or a second send into steer; adapters must expose a native,
/// exactly-once in-flight control channel before their inventory capability is
/// promoted.
#[cfg(test)]
fn native_steer_supported(agent_id: &str) -> bool {
    adapter_or_err(agent_id)
        .ok()
        .and_then(|adapter| {
            static_capability_matrix(adapter)
                .get("interruptSteer")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

pub fn steer_turn(params: &Value) -> Result<Value> {
    let agent_id = agent_id_param(params)?;
    let adapter = adapter_or_err(&agent_id)?;
    let matrix = static_capability_matrix(adapter);
    let supported = matrix
        .get("interruptSteer")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !supported {
        return Ok(json!({
            "ok": false,
            "agentId": adapter.id(),
            "laneFamily": lane_family(adapter),
            "status": "unsupported",
            "error": {
                "code": "dispatch_steer_unsupported",
                "stage": "turn/steer",
                "message": "This official lane does not expose a native in-flight steer channel."
            },
            "capabilities": matrix
        }));
    }
    let session_id = runtime_adapters::text_param_public(params, &["sessionId", "nativeSessionId"])
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("steer requires an exact native session identifier"))?;
    let text = runtime_adapters::text_param_public(params, &["text", "message", "prompt"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("steer requires non-empty guidance"))?;
    let disposition = match adapter {
        RuntimeAdapter::ClaudeCode => {
            match licoup_agent_claude_code::driver::steer(&session_id, &text) {
                licoup_agent_claude_code::driver::ControlDisposition::Accepted => "accepted",
                licoup_agent_claude_code::driver::ControlDisposition::NoActiveTurn => {
                    "no_active_turn"
                }
                licoup_agent_claude_code::driver::ControlDisposition::SessionUnavailable => {
                    "session_unavailable"
                }
                licoup_agent_claude_code::driver::ControlDisposition::TransportUnavailable => {
                    "unavailable"
                }
            }
        }
        RuntimeAdapter::Codex => {
            let turn_id = runtime_adapters::text_param_public(params, &["turnId"])
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("Codex steer requires the active native turn identifier"))?;
            match licoup_agent_codex::app_server::driver::active_control::steer(&session_id, &turn_id, &text) {
                licoup_agent_codex::app_server::driver::active_control::ControlDisposition::Accepted => "accepted",
                licoup_agent_codex::app_server::driver::active_control::ControlDisposition::NoActiveTurn => "no_active_turn",
                licoup_agent_codex::app_server::driver::active_control::ControlDisposition::SessionUnavailable => {
                    "session_unavailable"
                }
                licoup_agent_codex::app_server::driver::active_control::ControlDisposition::TransportUnavailable => "unavailable",
            }
        }
        RuntimeAdapter::Pi => {
            let turn_id = runtime_adapters::text_param_public(params, &["turnId"])
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("Pi steer requires the active native turn identifier"))?;
            match licoup_agent_pi::driver::steer(&session_id, &turn_id, &text) {
                licoup_agent_pi::driver::ControlDisposition::Accepted => "accepted",
                licoup_agent_pi::driver::ControlDisposition::NoActiveTurn => "no_active_turn",
                licoup_agent_pi::driver::ControlDisposition::SessionUnavailable => {
                    "session_unavailable"
                }
                licoup_agent_pi::driver::ControlDisposition::TransportUnavailable => "unavailable",
            }
        }
        _ => "unavailable",
    };
    let accepted = disposition == "accepted";
    Ok(json!({
        "ok": accepted,
        "agentId": adapter.id(),
        "laneFamily": lane_family(adapter),
        "status": disposition,
        "deliveryMode": if accepted { "native_steer" } else { "unavailable" },
        "error": if accepted { Value::Null } else { json!({
            "code": "dispatch_steer_transport_unavailable",
            "stage": "turn/steer",
            "message": "The native in-flight steer channel was not available for this turn."
        }) },
        "capabilities": matrix
    }))
}

/// Emit the per-agent capability matrix (inventory + static family matrix).
pub fn lane_capabilities(params: &Value) -> Result<Value> {
    let agent_id = agent_id_param(params)?;
    let adapter = adapter_or_err(&agent_id)?;
    let matrix = runtime_adapters::inventory_capability_matrix(&agent_id)
        .unwrap_or_else(|| static_capability_matrix(adapter));
    let profile = runtime_adapters::runtime_driver_profile(&agent_id);
    let blocker_codes: Vec<String> = profile
        .as_ref()
        .and_then(|p| p.blocker.clone())
        .into_iter()
        .collect();

    Ok(json!({
        "ok": true,
        "agentId": adapter.id(),
        "laneFamily": lane_family(adapter),
        "driverId": adapter.driver_id(),
        "runtimeProtocol": adapter.runtime_protocol(),
        "driverStatus": profile.as_ref().map(|p| p.driver_status.clone()).unwrap_or_default(),
        "readiness": profile.as_ref().map(|p| p.readiness.clone()).unwrap_or_else(|| "unverified".to_string()),
        "blockerCodes": blocker_codes,
        "summaryCodes": profile.as_ref().map(|p| p.summary_codes.clone()).unwrap_or_default(),
        "capabilities": matrix
    }))
}

/// Verified agent discovery for channel admission. Discovery is not admission:
/// callers still apply their own readiness and executable gate.
pub fn lane_target_scan() -> Result<Value> {
    crate::target_port::target_scan(&serde_json::json!({}))
}

/// Bounded conversation listing for one agent, as the lane sees it.
pub fn lane_conversation_list(params: &Value) -> Result<Value> {
    crate::domain::conversations::conversation_list(params)
}

/// Dispatch a conversation lane RPC/CLI operation by name.
pub fn dispatch_lane_operation(
    operation: &str,
    params: &Value,
) -> std::result::Result<Value, RuntimeAdapterError> {
    match operation {
        "open" | "openOrResume" | "resume" => {
            open_or_resume(params).map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)
        }
        "send" => send_and_settle(params),
        "steer" => steer_turn(params).map_err(|_| RuntimeAdapterError::ConversationDispatchFailed),
        "cancel" => {
            cancel_turn(params).map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)
        }
        "cleanup" => cleanup_conversation(params)
            .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed),
        "history" => process_local_history(params)
            .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed),
        "capabilities" | "caps" => {
            lane_capabilities(params).map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)
        }
        "stream" => Ok(json!({
            "ok": true,
            "agentId": agent_id_param(params).unwrap_or_default(),
            "events": [],
            "streamTransport": "stdio_ndjson_on_send",
            "status": "bound_on_send",
            "hint": "Pass streamEvents=true (or --stream-events true) on agent conversation send to receive progressive agent.message.chunk NDJSON lines before the final result."
        })),
        _ => Err(RuntimeAdapterError::ConversationDispatchFailed),
    }
}

/// L5 is the sole terminal decision point. Missing or zero `timeoutMs` uses
/// the writable policy, whose default is unbounded (0); a resolved zero is
/// forwarded as `timeoutUnbounded` so no lower layer reintroduces a deadline.
/// Finite deadlines come only from an explicit caller value or a configured
/// policy entry.
fn send_and_settle(params: &Value) -> std::result::Result<Value, RuntimeAdapterError> {
    let resolved_timeout = crate::domain::dispatch_timeout_policy::resolve_dispatch_timeout(params)
        .map_err(|_| RuntimeAdapterError::InvalidRuntimeSetting { field: "timeoutMs" })?;
    let explicit_deadline = resolved_timeout > 0;
    let effective_params = params_with_resolved_timeout(params, resolved_timeout);
    let mut arbiter = TurnSettlementArbiter::new();
    if arbiter.begin_dispatch().is_err() {
        return Err(RuntimeAdapterError::ConversationDispatchFailed);
    }
    let mut projected_deltas = arbiter.drain_deltas();
    emit_settlement_deltas(&projected_deltas, "", "");

    match runtime_adapters::send_message(
        &crate::target_port::agent_target_port(),
        &effective_params,
    ) {
        Ok(mut response) => {
            let signal = settlement_signal(&response, explicit_deadline);
            let outcome = arbiter
                .settle(signal)
                .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)?;
            let deltas = arbiter.drain_deltas();
            let session_id = response
                .get("nativeSessionId")
                .or_else(|| response.get("sessionId"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let turn_id = response
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            emit_settlement_deltas(&deltas, session_id, turn_id);
            emit_user_message_projection(params, &arbiter, session_id, turn_id);
            projected_deltas.extend(deltas);
            project_settlement(&mut response, outcome, &arbiter, &projected_deltas);
            Ok(response)
        }
        Err(error) => {
            let outcome = arbiter
                .settle(SettlementSignal::Error)
                .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)?;
            debug_assert!(matches!(outcome, SettlementOutcome::Failed(_)));
            emit_settlement_deltas(&arbiter.drain_deltas(), "", "");
            Err(error)
        }
    }
}

fn params_with_resolved_timeout(params: &Value, timeout_ms: u64) -> Value {
    let mut effective = params.clone();
    if let Some(object) = effective.as_object_mut() {
        object.insert("timeoutMs".to_owned(), Value::from(timeout_ms));
        if timeout_ms == 0 {
            object.insert("timeoutUnbounded".to_owned(), Value::from(true));
        }
    }
    effective
}

fn settlement_signal(response: &Value, explicit_deadline: bool) -> SettlementSignal {
    let turn_status = response
        .get("turnStatus")
        .or_else(|| {
            response
                .get("error")
                .and_then(|error| error.get("turnStatus"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default();
    if turn_status == "cancelled" {
        return SettlementSignal::CancelConfirmed;
    }
    let error_code = response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if explicit_deadline
        && (turn_status == "timeout"
            || error_code == "deadline_exceeded"
            || error_code.ends_with("_timeout"))
    {
        return SettlementSignal::ExplicitDeadline;
    }
    if response.get("ok").and_then(Value::as_bool) == Some(false)
        || response.get("error").is_some_and(|error| !error.is_null())
    {
        return SettlementSignal::Error;
    }
    let protocol_finished = response
        .get("events")
        .and_then(Value::as_array)
        .is_some_and(|events| {
            events.iter().any(|event| {
                event.get("kind").and_then(Value::as_str) == Some("lifecycle")
                    && event.get("stage").and_then(Value::as_str) == Some("completed")
            })
        })
        || matches!(turn_status, "completed" | "end_turn" | "end-turn");
    if protocol_finished {
        SettlementSignal::ProtocolFinish
    } else {
        SettlementSignal::Eof {
            has_content: response
                .get("output")
                .and_then(Value::as_str)
                .is_some_and(|output| !output.is_empty()),
        }
    }
}

fn emit_settlement_deltas(deltas: &[SettlementDelta], session_id: &str, turn_id: &str) {
    for delta in deltas {
        super::turn_event_emit::emit_turn_event(
            "conversation.state.delta",
            session_id,
            turn_id,
            delta.to_json(),
        );
    }
}

/// Project the submitted user message (with explicit interaction capability
/// flags) through the send stream. No stream identity means the client could
/// not bind the projection to a turn, so nothing is emitted; the native host
/// never projects content without a groundable scope.
fn emit_user_message_projection(
    params: &Value,
    arbiter: &TurnSettlementArbiter,
    session_id: &str,
    turn_id: &str,
) {
    if session_id.trim().is_empty() && turn_id.trim().is_empty() {
        return;
    }
    let Some(text) = runtime_adapters::text_param_public(params, &["text", "message", "prompt"])
    else {
        return;
    };
    let cancel_supported = agent_id_param(params)
        .ok()
        .and_then(|agent_id| adapter_or_err(&agent_id).ok())
        .and_then(|adapter| {
            static_capability_matrix(adapter)
                .get("cancel")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    let Some(delta) = projection_delta::project_submitted_user_message(
        &text,
        turn_state_wire(arbiter.turn_state()),
        cancel_supported,
    ) else {
        return;
    };
    super::turn_event_emit::emit_turn_event(
        delta.event_kind(),
        session_id,
        turn_id,
        delta.to_event_payload(),
    );
}

fn project_settlement(
    response: &mut Value,
    outcome: SettlementOutcome,
    arbiter: &TurnSettlementArbiter,
    deltas: &[SettlementDelta],
) {
    let Some(object) = response.as_object_mut() else {
        return;
    };
    let mut settlement = json!({
        "outcome": outcome.wire_name(),
        "turnState": turn_state_wire(arbiter.turn_state()),
        "sendState": send_state_wire(arbiter.send_state()),
    });
    if let SettlementOutcome::Failed(reason) = outcome {
        settlement["reason"] = json!(reason.wire_name());
        object.insert("ok".to_owned(), Value::Bool(false));
        if matches!(reason, SettlementFailureReason::TransportLost) {
            object.insert("turnStatus".to_owned(), json!("failed"));
            object.insert(
                "error".to_owned(),
                json!({
                    "code": "transport_lost",
                    "stage": "turn/settlement",
                    "turnStatus": "failed",
                }),
            );
        } else if matches!(reason, SettlementFailureReason::DeadlineExceeded) {
            object.insert("turnStatus".to_owned(), json!("failed"));
            object.insert(
                "error".to_owned(),
                json!({
                    "code": "deadline_exceeded",
                    "stage": "turn/deadline",
                    "turnStatus": "failed",
                }),
            );
        }
    } else if matches!(outcome, SettlementOutcome::Cancelled) {
        object.insert("ok".to_owned(), Value::Bool(false));
        object.insert("turnStatus".to_owned(), json!("cancelled"));
    }
    object.insert("settlement".to_owned(), settlement);
    object.insert(
        "stateDeltas".to_owned(),
        Value::Array(deltas.iter().map(|delta| delta.to_json()).collect()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_settlement_missing_timeout_uses_policy_unless_unbounded() {
        let policy = json!({
            "defaultTimeoutMs": 180_000,
            "agents": {}
        });
        let missing = crate::domain::dispatch_timeout_policy::resolve_dispatch_timeout(&json!({
            "agent": "codex",
            "dispatchTimeoutPolicy": policy,
        }))
        .unwrap();
        assert_eq!(missing, 180_000);
        let resolved = params_with_resolved_timeout(
            &json!({"agent": "codex", "dispatchTimeoutPolicy": policy}),
            missing,
        );
        assert_eq!(resolved["timeoutMs"], 180_000);

        assert!(
            crate::domain::dispatch_timeout_policy::resolve_dispatch_timeout(&json!({
                "agent": "codex",
                "timeoutMs": "120000",
                "dispatchTimeoutPolicy": policy,
            }))
            .is_err()
        );

        let explicit = crate::domain::dispatch_timeout_policy::resolve_dispatch_timeout(&json!({
            "agent": "codex",
            "timeoutMs": 300_000,
            "dispatchTimeoutPolicy": policy,
        }))
        .unwrap();
        assert_eq!(explicit, 300_000);

        let unbounded = crate::domain::dispatch_timeout_policy::resolve_dispatch_timeout(&json!({
            "agent": "codex",
            "timeoutMs": 0,
            "timeoutUnbounded": true,
        }))
        .unwrap();
        assert_eq!(unbounded, 0);
    }

    #[test]
    fn test_settlement_response_classification_uses_protocol_signals() {
        let protocol_finish = json!({
            "ok": true,
            "output": "",
            "turnStatus": "completed",
            "events": [{"kind": "lifecycle", "stage": "completed"}]
        });
        assert_eq!(
            settlement_signal(&protocol_finish, false),
            SettlementSignal::ProtocolFinish
        );

        assert_eq!(
            settlement_signal(&json!({"ok": true, "output": "content"}), false),
            SettlementSignal::Eof { has_content: true }
        );
        assert_eq!(
            settlement_signal(&json!({"ok": true, "output": ""}), false),
            SettlementSignal::Eof { has_content: false }
        );
        assert_eq!(
            settlement_signal(
                &json!({
                    "ok": false,
                    "turnStatus": "timeout",
                    "error": {"code": "acp_protocol_timeout"}
                }),
                true,
            ),
            SettlementSignal::ExplicitDeadline
        );
        assert_eq!(
            settlement_signal(
                &json!({
                    "ok": false,
                    "turnStatus": "timeout",
                    "error": {"code": "acp_protocol_timeout"}
                }),
                false,
            ),
            SettlementSignal::Error
        );
    }

    #[test]
    fn test_cancel_projection_is_distinct_from_failed_projection() {
        let mut arbiter = TurnSettlementArbiter::new();
        let outcome = arbiter.settle(SettlementSignal::CancelConfirmed).unwrap();
        let deltas = arbiter.drain_deltas();
        let mut response = json!({"ok": false, "error": {"turnStatus": "cancelled"}});
        project_settlement(&mut response, outcome, &arbiter, &deltas);

        assert_eq!(response["settlement"]["outcome"], "cancelled");
        assert_eq!(response["settlement"]["turnState"], "cancelled");
        assert_eq!(response["settlement"]["sendState"], "delivered");
        assert_eq!(response["turnStatus"], "cancelled");
        assert_ne!(response["settlement"]["outcome"], "failed");
    }

    #[test]
    fn lane_families_cover_all_packaged_adapters() {
        assert_eq!(lane_family(RuntimeAdapter::Codex), "app-server");
        assert_eq!(lane_family(RuntimeAdapter::ClaudeCode), "stream-json");
        assert_eq!(lane_family(RuntimeAdapter::OpenCode), "serve-http");
        assert_eq!(lane_family(RuntimeAdapter::KiloCode), "serve-http");
        assert_eq!(lane_family(RuntimeAdapter::Pi), "rpc");
        assert_eq!(lane_family(RuntimeAdapter::Cursor), "cli");
        assert_eq!(lane_family(RuntimeAdapter::Antigravity), "cli");
    }

    #[test]
    fn open_new_session_succeeds_for_ready_candidate_families() {
        let result = open_or_resume(&json!({"agent": "codex"})).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["openMode"], "new");
        assert_eq!(result["laneFamily"], "app-server");
        assert_eq!(result["capabilities"]["exactResume"], true);
    }

    #[test]
    fn open_resume_accepts_claude_and_cursor_binding() {
        let claude = open_or_resume(&json!({
            "agent": "claude-code",
            "sessionId": "native-1"
        }))
        .unwrap();
        assert_eq!(claude["ok"], true);
        assert_eq!(claude["openMode"], "resume");
        assert_eq!(claude["laneFamily"], "stream-json");
        assert_eq!(claude["capabilities"]["exactResume"], true);
        assert_eq!(claude["capabilities"]["processLocalContinuation"], true);

        let cursor = open_or_resume(&json!({
            "agent": "cursor",
            "sessionId": "native-2"
        }))
        .unwrap();
        assert_eq!(cursor["ok"], true);
        assert_eq!(cursor["openMode"], "resume");
        assert_eq!(cursor["laneFamily"], "cli");
        assert_eq!(cursor["capabilities"]["exactResume"], true);
    }

    #[test]
    fn antigravity_open_binds_cli_lane_like_cursor() {
        let fresh = open_or_resume(&json!({"agent": "antigravity"})).unwrap();
        assert_eq!(fresh["ok"], true);
        assert_eq!(fresh["openMode"], "new");
        assert_eq!(fresh["laneFamily"], "cli");
        assert_eq!(fresh["capabilities"]["exactResume"], true);

        let resumed = open_or_resume(&json!({
            "agent": "antigravity",
            "sessionId": "native-agy-1"
        }))
        .unwrap();
        assert_eq!(resumed["ok"], true);
        assert_eq!(resumed["openMode"], "resume");
        assert_eq!(resumed["laneFamily"], "cli");
    }

    #[test]
    fn cancel_reports_structured_disposition_per_family() {
        let acp = cancel_turn(&json!({"agent": "opencode", "sessionId": "s1"})).unwrap();
        assert_eq!(acp["ok"], false);
        assert_eq!(acp["status"], "not_active");
        assert_eq!(acp["capabilities"]["cancel"], true);
        assert_eq!(acp["error"]["code"], "opencode_turn_not_active");

        let claude = cancel_turn(&json!({"agent": "claude-code", "sessionId": "missing"})).unwrap();
        assert_eq!(claude["error"]["code"], "claude_code_session_unavailable");
    }

    #[test]
    fn steer_uses_only_verified_native_channels_and_fails_closed_elsewhere() {
        for agent in ["opencode", "hermes", "cursor"] {
            let result = steer_turn(&json!({
                "agent": agent,
                "sessionId": "native-session",
                "text": "follow-up"
            }))
            .unwrap();
            assert_eq!(result["ok"], false);
            assert_eq!(result["status"], "unsupported");
            assert_eq!(result["error"]["code"], "dispatch_steer_unsupported");
        }
        for agent in ["codex", "claude-code", "pi"] {
            assert!(native_steer_supported(agent));
            let result = steer_turn(&json!({
                "agent": agent,
                "sessionId": "native-session",
                "turnId": "native-turn",
                "text": "follow-up"
            }))
            .unwrap();
            assert_eq!(result["ok"], false);
            assert_ne!(result["status"], "unsupported");
            assert_eq!(
                result["error"]["code"],
                "dispatch_steer_transport_unavailable"
            );
        }
    }

    #[test]
    fn hermes_exact_resume_is_available_on_the_persistent_acp_lane() {
        let result = open_or_resume(&json!({
            "agent": "hermes",
            "sessionId": "native-1"
        }))
        .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["capabilities"]["exactResume"], true);
        assert_eq!(result["capabilities"]["cancel"], true);
    }

    #[test]
    fn deepseek_cleanup_uses_exact_process_local_session_while_cancel_stays_unsupported() {
        let cleanup = cleanup_conversation(&json!({
            "agent": "deepseek-harness",
            "sessionId": "missing-deepseek-session"
        }))
        .unwrap();
        assert_eq!(cleanup["ok"], false);
        assert_eq!(cleanup["status"], "not_found");
        assert_eq!(
            cleanup["error"]["code"],
            "deepseek_harness_session_unavailable"
        );

        let cancel = cancel_turn(&json!({
            "agent": "deepseek-harness",
            "sessionId": "missing-deepseek-session"
        }))
        .unwrap();
        assert_eq!(cancel["status"], "unsupported");
        assert_eq!(cancel["error"]["code"], "dispatch_cancel_unsupported");
    }

    #[test]
    fn deepseek_open_preserves_separate_release_readiness_and_native_capabilities() {
        let open = open_or_resume(&json!({"agent": "deepseek-harness"})).unwrap();
        assert_eq!(open["ok"], true);
        assert_eq!(open["openMode"], "new");
        let resumed = open_or_resume(&json!({
            "agent": "deepseek-harness", "sessionId": "synthetic-sdk-session"
        }))
        .unwrap();
        assert_eq!(resumed["ok"], true);
        assert_eq!(resumed["openMode"], "resume");
        assert_eq!(resumed["nativeSessionId"], "synthetic-sdk-session");

        let capabilities = lane_capabilities(&json!({"agent": "deepseek-harness"})).unwrap();
        assert_eq!(capabilities["capabilities"]["streaming"], false);
        assert_eq!(capabilities["capabilities"]["cancel"], false);
        assert_eq!(capabilities["readiness"], "unverified");
        assert_eq!(capabilities["blockerCodes"], json!([]));
        assert_eq!(capabilities["summaryCodes"], json!(["evidence_missing"]));
    }

    #[test]
    fn capabilities_emit_matrix_and_blockers() {
        let caps = lane_capabilities(&json!({"agent": "antigravity"})).unwrap();
        assert_eq!(caps["ok"], true);
        assert_eq!(caps["laneFamily"], "cli");
        assert_eq!(caps["capabilities"]["exactResume"], true);
        assert_eq!(caps["blockerCodes"], json!([]));
        let claude = lane_capabilities(&json!({"agent": "claude-code"})).unwrap();
        assert_eq!(claude["capabilities"]["exactResume"], true);
        assert_eq!(claude["readiness"], "unverified");
        assert_eq!(claude["blockerCodes"], json!([]));
    }

    #[test]
    fn lane_families_are_official_protocol_labels_only() {
        for family in [
            lane_family(RuntimeAdapter::Codex),
            lane_family(RuntimeAdapter::ClaudeCode),
            lane_family(RuntimeAdapter::OpenCode),
            lane_family(RuntimeAdapter::KiloCode),
            lane_family(RuntimeAdapter::Pi),
            lane_family(RuntimeAdapter::Cursor),
            lane_family(RuntimeAdapter::Antigravity),
        ] {
            assert!(matches!(
                family,
                "acp" | "app-server" | "cli" | "rpc" | "serve-http" | "stream-json" | "unavailable"
            ));
        }
    }
}
