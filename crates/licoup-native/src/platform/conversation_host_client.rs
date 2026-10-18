//! Native command client for the private PersistentTurn host.
//!
//! This is not MCP framing and carries no provider policy. It is the one
//! transport used by all Subagent runtime adapters to reach Canonical
//! Conversation execution.

use super::conversation_host_transport;
use crate::contracts::conversation_protocol::{
    CONVERSATION_PROTOCOL_MAX_FRAME_BYTES, CONVERSATION_PROTOCOL_MAX_RESPONSE_BYTES,
    ConversationCommand, ConversationProtocolMethod,
};
use anyhow::{Result, anyhow, ensure};
use interprocess::local_socket::{Stream, traits::Stream as _};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    thread,
    time::Duration,
};

const FRAME_LIMIT: usize = 64 * 1024;

pub(crate) fn execute(method: &str, params: &Value) -> Result<Value> {
    execute_with_connector(method, params, conversation_host_transport::connect)
}

pub(crate) fn execute_read_only(method: &str, params: &Value) -> Result<Value> {
    execute_existing(method, params)
}

/// Execute through an already-running Conversation host without creating an
/// endpoint or retrying through a process-local ConversationService.
pub fn execute_existing(method: &str, params: &Value) -> Result<Value> {
    execute_with_connector(
        method,
        params,
        conversation_host_transport::connect_existing,
    )
}

/// Stop the current root's host and wait for its owner lock to become
/// available. The RPC acknowledgment closes admission; the OS lock release is
/// the proof that the host process has stopped writing.
pub fn stop_existing_and_wait() -> Result<()> {
    let root = licoup_foundation::platform::paths::portable_data_dir_read_only()?;
    if let Some(owner) = super::conversation_host_transport::try_acquire_host_owner_lock(&root)? {
        drop(owner);
        return Ok(());
    }
    execute_existing("shutdown", &json!({ "host": true }))?;
    loop {
        if let Some(owner) = super::conversation_host_transport::try_acquire_host_owner_lock(&root)?
        {
            drop(owner);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn execute_with_connector(
    method: &str,
    params: &Value,
    connect: fn() -> io::Result<Stream>,
) -> Result<Value> {
    let mut stream =
        connect().map_err(|_| anyhow!("persistent_conversation_transport_required"))?;
    stream
        .set_nonblocking(false)
        .map_err(|_| anyhow!("subagent_transport_failed"))?;
    let request_id = format!("subagent-{}", uuid::Uuid::new_v4().simple());
    let mut encoded = serde_json::to_vec(&json!({
        "protocol": conversation_host_transport::STDIO_RPC_PROTOCOL,
        "id": request_id,
        "workflowId": request_id,
        "method": method,
        "params": params,
    }))
    .map_err(|_| anyhow!("subagent_transport_failed"))?;
    encoded.push(b'\n');
    if encoded.len() > FRAME_LIMIT {
        return Err(anyhow!("subagent_request_too_large"));
    }
    stream
        .write_all(&encoded)
        .map_err(|_| anyhow!("subagent_transport_failed"))?;
    stream
        .flush()
        .map_err(|_| anyhow!("subagent_transport_failed"))?;
    // There is no turn deadline. A transport closure reports loss of the
    // observer and never cancels the separately owned PersistentTurn.
    let mut response = Vec::with_capacity(1024);
    BufReader::new(stream)
        .take(FRAME_LIMIT as u64 + 1)
        .read_until(b'\n', &mut response)
        .map_err(|_| anyhow!("subagent_transport_failed"))?;
    if response.last() != Some(&b'\n') || response.len() > FRAME_LIMIT {
        return Err(anyhow!("subagent_transport_invalid_response"));
    }
    let frame: Value = serde_json::from_slice(&response)
        .map_err(|_| anyhow!("subagent_transport_invalid_response"))?;
    if frame.get("protocol").and_then(Value::as_str)
        != Some(conversation_host_transport::STDIO_RPC_PROTOCOL)
        || frame.get("id").and_then(Value::as_str) != Some(request_id.as_str())
        || frame.get("workflowId").and_then(Value::as_str) != Some(request_id.as_str())
    {
        return Err(anyhow!("subagent_transport_invalid_response"));
    }
    if frame.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(stable_error_code(
            frame.pointer("/error/code").and_then(Value::as_str)
        )));
    }
    let result = frame
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("subagent_transport_invalid_response"))?;
    if result.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(anyhow!(stable_error_code(
            result.pointer("/error/code").and_then(Value::as_str)
        )));
    }
    Ok(result)
}

fn stable_error_code(code: Option<&str>) -> &'static str {
    match code {
        Some("conversation_not_found") => "conversation_not_found",
        Some("conversation_state_unavailable") => "conversation_state_unavailable",
        Some("invalid_request") => "invalid_request",
        Some("subagent_self_call_rejected") => "subagent_self_call_rejected",
        Some("subagent_caller_membership_inactive") => "subagent_caller_membership_inactive",
        Some("subagent_target_membership_inactive") => "subagent_target_membership_inactive",
        Some("subagent_target_invalid") => "subagent_target_invalid",
        Some("subagent_duplicate_active_edge") => "subagent_duplicate_active_edge",
        Some("subagent_parent_dispatch_unavailable") => "subagent_parent_dispatch_unavailable",
        Some("subagent_cross_conversation_rejected") => "subagent_cross_conversation_rejected",
        Some("subagent_lineage_caller_mismatch") => "subagent_lineage_caller_mismatch",
        Some("subagent_lineage_cycle") => "subagent_lineage_cycle",
        Some("subagent_depth_exceeded") => "subagent_depth_exceeded",
        Some("subagent_dispatch_not_found") => "subagent_dispatch_not_found",
        Some("subagent_dispatch_transition_invalid") => "subagent_dispatch_transition_invalid",
        Some("turn_not_found") => "subagent_turn_not_found",
        Some("turn_not_active") => "subagent_turn_not_active",
        Some("turn_scope_mismatch") => "subagent_turn_scope_mismatch",
        Some("conversation_capacity_exhausted") => "subagent_capacity_exhausted",
        Some("persistent_conversation_transport_required") => {
            "persistent_conversation_transport_required"
        }
        _ => "subagent_transport_failed",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostOutput {
    Frames,
    Result,
}

#[derive(Debug, PartialEq)]
pub(crate) enum HostResponse {
    Json(Value),
    Streamed,
}

pub(crate) fn request_for_method(
    method: ConversationProtocolMethod,
    params: &Value,
) -> Result<Value> {
    ensure!(params.is_object(), "invalid_params");
    let request_id = format!("cli-{}", uuid::Uuid::new_v4().simple());
    let mut frame = ConversationCommand::frame(&request_id, &request_id, method, params.clone());
    if method == ConversationProtocolMethod::Execute {
        frame["args"] = frame["params"]
            .as_object_mut()
            .and_then(|params| params.remove("args"))
            .ok_or_else(|| anyhow!("invalid_args"))?;
    }
    let encoded = serde_json::to_vec(&frame)?;
    ensure!(
        encoded.len() < CONVERSATION_PROTOCOL_MAX_FRAME_BYTES,
        "request_too_large"
    );
    ConversationCommand::decode(&encoded).map_err(|error| anyhow!(error.code))?;
    Ok(frame)
}

/// Preserve original native frames and their existing individual byte bound.
pub(crate) fn execute_host_call(
    mut stream: impl Read + Write,
    request: Value,
    mut output: impl Write,
    mode: HostOutput,
) -> Result<HostResponse> {
    let mut encoded = serde_json::to_vec(&request)?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .and_then(|_| stream.flush())
        .map_err(|_| anyhow!("native_host_transport_failed"))?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        let read = (&mut reader)
            .take((CONVERSATION_PROTOCOL_MAX_RESPONSE_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|_| anyhow!("native_host_transport_failed"))?;
        ensure!(read > 0, "native_host_disconnected");
        ensure!(
            bytes.len() <= CONVERSATION_PROTOCOL_MAX_RESPONSE_BYTES && bytes.last() == Some(&b'\n'),
            "native_host_response_invalid"
        );
        let frame: Value =
            serde_json::from_slice(&bytes).map_err(|_| anyhow!("native_host_response_invalid"))?;
        ensure!(
            frame.get("protocol") == request.get("protocol")
                && frame.get("id") == request.get("id")
                && frame.get("workflowId") == request.get("workflowId"),
            "native_host_response_invalid"
        );
        let terminal = match frame.get("kind").and_then(Value::as_str) {
            Some("event") => {
                ensure!(
                    frame.get("event").is_some_and(Value::is_object),
                    "native_host_response_invalid"
                );
                false
            }
            None | Some("terminal") => {
                ensure!(
                    frame.get("ok").is_some_and(Value::is_boolean),
                    "native_host_response_invalid"
                );
                true
            }
            _ => return Err(anyhow!("native_host_response_invalid")),
        };
        if mode == HostOutput::Frames || !terminal {
            output
                .write_all(&bytes)
                .and_then(|_| output.flush())
                .map_err(|_| anyhow!("native_cli_output_unavailable"))?;
        }
        if terminal {
            if mode == HostOutput::Frames {
                return Ok(HostResponse::Streamed);
            }
            let result = if frame["ok"] == true {
                frame.get("result").cloned()
            } else {
                frame
                    .get("error")
                    .map(|error| json!({"ok": false, "error": error}))
            }
            .ok_or_else(|| anyhow!("native_host_response_invalid"))?;
            return Ok(HostResponse::Json(result));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct FakeHost(Cursor<Vec<u8>>);
    impl Read for FakeHost {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.read(bytes)
        }
    }
    impl Write for FakeHost {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn native_call_preserves_events_and_terminal_without_truncating_output() {
        let request = request_for_method(
            ConversationProtocolMethod::AgentConversationAttach,
            &json!({}),
        )
        .unwrap();
        let mode = HostOutput::Frames;
        let mut event = request.clone();
        event["kind"] = json!("event");
        event["sequence"] = json!(1);
        event["event"] = json!({"text": "synthetic delta"});
        let mut terminal = request.clone();
        terminal["kind"] = json!("terminal");
        terminal["sequence"] = json!(2);
        terminal["ok"] = json!(true);
        terminal["result"] = json!({"completed": true});
        let frames = format!("{event}\n{terminal}\n").into_bytes();
        let mut output = Vec::new();
        let result = execute_host_call(
            FakeHost(Cursor::new(frames.clone())),
            request,
            &mut output,
            mode,
        )
        .unwrap();
        assert_eq!(result, HostResponse::Streamed);
        assert_eq!(output, frames);
    }

    #[test]
    fn wrong_host_response_identity_is_rejected_without_forwarding() {
        let request =
            request_for_method(ConversationProtocolMethod::CatalogStatus, &json!({})).unwrap();
        let mode = HostOutput::Frames;
        let mut response = request.clone();
        response["id"] = json!("unrelated");
        response["ok"] = json!(true);
        response["result"] = json!({});
        let mut output = Vec::new();
        let error = execute_host_call(
            FakeHost(Cursor::new(format!("{response}\n").into_bytes())),
            request,
            &mut output,
            mode,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "native_host_response_invalid");
        assert!(output.is_empty());
    }
}
