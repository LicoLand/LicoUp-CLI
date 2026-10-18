use super::*;
use licoup_native::ffi::generated::client_error::ClientError;

// RPC frames preserve ClientError.code, stage, component, retryable, recovery,
// and presentationArgs by serializing the generated value directly.
pub(crate) fn recover_stdio_rpc_writer<W: Write>(writer: Arc<StdioRpcWriter<W>>) -> Result<W> {
    Arc::try_unwrap(writer)
        .map_err(|_| anyhow::anyhow!("stdio RPC writer is still in use"))?
        .into_writer()
        .map_err(anyhow::Error::from)
}

pub(crate) fn write_stdio_rpc_success_shared<W: Write>(
    target: StdioRpcFrameTarget<'_, W>,
    id: &str,
    workflow_id: &str,
    result: Value,
) -> io::Result<()> {
    let response = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "ok": true,
        "result": result,
    });
    if target.write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &response)? {
        return Ok(());
    }
    let error = stdio_rpc_client_error("response_too_large");
    write_stdio_rpc_client_error_shared(target, Some(id), Some(workflow_id), &error)
}

pub(crate) fn write_stdio_rpc_error_shared<W: Write>(
    target: StdioRpcFrameTarget<'_, W>,
    id: Option<&str>,
    workflow_id: Option<&str>,
    code: &'static str,
) -> io::Result<()> {
    let error = stdio_rpc_client_error(code);
    write_stdio_rpc_client_error_shared(target, id, workflow_id, &error)
}

pub(crate) fn write_stdio_rpc_client_error_shared<W: Write>(
    target: StdioRpcFrameTarget<'_, W>,
    id: Option<&str>,
    workflow_id: Option<&str>,
    error: &ClientError,
) -> io::Result<()> {
    let response = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "ok": false,
        "error": error,
    });
    if target.write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &response)? {
        Ok(())
    } else {
        Err(io::Error::other("stdio RPC error response exceeds limit"))
    }
}

pub(crate) fn write_stdio_rpc_event<W: Write>(
    target: StdioRpcFrameTarget<'_, W>,
    id: &str,
    workflow_id: &str,
    sequence: u64,
    event: Value,
) -> io::Result<()> {
    let session_id = event.get("sessionId").and_then(Value::as_str);
    let turn_id = event.get("turnId").and_then(Value::as_str);
    let event_name = event.get("event").and_then(Value::as_str);
    let execution = matches!(
        event_name,
        Some("agent.execution.record" | "agent.execution.ready")
    ) && event
        .get("membershipId")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let persistent = event
        .get("turnHandle")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        && event
            .get("conversationId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        && event
            .get("cursor")
            .and_then(Value::as_u64)
            .is_some_and(|cursor| cursor > 0 || execution);
    if event_name.is_none_or(str::is_empty)
        || (!persistent
            && (session_id.is_none_or(str::is_empty) || turn_id.is_none_or(str::is_empty)))
    {
        return Err(io::Error::other("invalid stdio RPC stream event"));
    }
    let frame = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "kind": "event",
        "sequence": sequence,
        "event": event,
    });
    if target.write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame)? {
        Ok(())
    } else {
        Err(io::Error::other("stdio RPC event exceeds limit"))
    }
}

pub(crate) fn write_stdio_rpc_terminal_success<W: Write>(
    target: StdioRpcFrameTarget<'_, W>,
    id: &str,
    workflow_id: &str,
    sequence: u64,
    result: Value,
) -> io::Result<()> {
    let frame = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "kind": "terminal",
        "sequence": sequence,
        "ok": true,
        "result": result,
    });
    if target.write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame)? {
        return Ok(());
    }
    let error = stdio_rpc_client_error("response_too_large");
    let bounded_error = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "kind": "terminal",
        "sequence": sequence,
        "ok": false,
        "error": error,
    });
    if target.write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &bounded_error)? {
        Ok(())
    } else {
        Err(io::Error::other("stdio RPC terminal exceeds limit"))
    }
}

pub(crate) fn write_stdio_rpc_terminal_error<W: Write>(
    target: StdioRpcFrameTarget<'_, W>,
    id: &str,
    workflow_id: &str,
    sequence: u64,
    error: &ClientError,
) -> io::Result<()> {
    let frame = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "kind": "terminal",
        "sequence": sequence,
        "ok": false,
        "error": error,
    });
    if target.write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame)? {
        Ok(())
    } else {
        Err(io::Error::other("stdio RPC terminal exceeds limit"))
    }
}

#[cfg(test)]
pub(crate) fn write_stdio_rpc_success_with_limit(
    writer: &mut impl Write,
    id: &str,
    workflow_id: &str,
    result: Value,
    max_response_bytes: usize,
) -> io::Result<()> {
    let response = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "ok": true,
        "result": result,
    });
    if try_write_stdio_rpc_response(writer, &response, max_response_bytes)? {
        return Ok(());
    }
    let error = stdio_rpc_client_error("response_too_large");
    write_stdio_rpc_error(writer, Some(id), Some(workflow_id), &error)
}

#[cfg(test)]
pub(crate) fn write_stdio_rpc_error(
    writer: &mut impl Write,
    id: Option<&str>,
    workflow_id: Option<&str>,
    error: &ClientError,
) -> io::Result<()> {
    let response = json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "ok": false,
        "error": error,
    });
    if try_write_stdio_rpc_response(writer, &response, STDIO_RPC_MAX_RESPONSE_BYTES)? {
        Ok(())
    } else {
        Err(io::Error::other("stdio RPC error response exceeds limit"))
    }
}

/// Serialize one frame, or report that it exceeds its declared bound.
///
/// Every response crosses this single encoder, so the lane-aware writer and the
/// plain-writer helpers agree on the frame bound and on the wire shape.
pub(crate) fn encode_stdio_rpc_frame(
    response: &Value,
    max_response_bytes: usize,
) -> io::Result<Option<Vec<u8>>> {
    let encoded = serde_json::to_vec(response).map_err(io::Error::other)?;
    if encoded.len().saturating_add(1) > max_response_bytes {
        return Ok(None);
    }
    Ok(Some(encoded))
}

// Serialize before touching stdout so every response is one bounded, atomic JSON line.
#[cfg(test)]
pub(crate) fn try_write_stdio_rpc_response(
    writer: &mut impl Write,
    response: &Value,
    max_response_bytes: usize,
) -> io::Result<bool> {
    let Some(encoded) = encode_stdio_rpc_frame(response, max_response_bytes)? else {
        return Ok(false);
    };
    writer.write_all(&encoded)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(true)
}
