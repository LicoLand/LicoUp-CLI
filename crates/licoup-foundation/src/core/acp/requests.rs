use super::validation::{
    MAX_CURSOR_BYTES, ensure_message_limit, normalized_text, validated_session_id,
};
use super::{
    AcpClientCapabilities, AcpError, AcpImplementation, AcpRequestId, AcpSessionMethod,
    AcpSessionOptions, DEFAULT_MAX_MESSAGE_BYTES, INITIALIZE_METHOD, JSON_RPC_VERSION,
    MAX_ADDITIONAL_DIRECTORIES, MAX_MCP_SERVERS, PROTOCOL_VERSION, SESSION_CANCEL_METHOD,
    SESSION_CLOSE_METHOD, SESSION_LIST_METHOD, SESSION_PROMPT_METHOD,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::Path;

const MAX_MCP_SERVER_NAME_BYTES: usize = 256;

pub fn initialize_request(
    id: impl Into<AcpRequestId>,
    client: &AcpImplementation,
    capabilities: AcpClientCapabilities,
) -> Result<Value, AcpError> {
    let params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": capabilities.to_value(),
        "clientInfo": client.to_value()?
    });
    request_envelope(id.into(), INITIALIZE_METHOD, params)
}

pub fn session_request(
    id: impl Into<AcpRequestId>,
    method: AcpSessionMethod<'_>,
    options: AcpSessionOptions<'_>,
) -> Result<Value, AcpError> {
    let cwd = absolute_path_text(options.cwd, AcpError::WorkingDirectoryInvalid)?;
    if options.additional_directories.len() > MAX_ADDITIONAL_DIRECTORIES {
        return Err(AcpError::AdditionalDirectoryLimitExceeded);
    }
    let mut seen_directories = HashSet::with_capacity(options.additional_directories.len());
    let mut additional_directories = Vec::with_capacity(options.additional_directories.len());
    for directory in options.additional_directories {
        let path = absolute_path_text(directory, AcpError::AdditionalDirectoryInvalid)?;
        if !seen_directories.insert(path.clone()) {
            return Err(AcpError::AdditionalDirectoryInvalid);
        }
        additional_directories.push(Value::String(path));
    }
    validate_mcp_servers(options.mcp_servers)?;

    let mut params = Map::new();
    params.insert("cwd".into(), Value::String(cwd));
    if !additional_directories.is_empty() {
        params.insert(
            "additionalDirectories".into(),
            Value::Array(additional_directories),
        );
    }
    params.insert(
        "mcpServers".into(),
        Value::Array(options.mcp_servers.to_vec()),
    );
    if let Some(session_id) = method.requested_session_id() {
        params.insert(
            "sessionId".into(),
            Value::String(validated_session_id(session_id)?.to_owned()),
        );
    }
    if let Some(meta) = options.meta {
        params.insert("_meta".into(), Value::Object(meta));
    }
    request_envelope(id.into(), method.method_name(), Value::Object(params))
}

pub fn text_prompt_request(
    id: impl Into<AcpRequestId>,
    session_id: &str,
    prompt: &str,
) -> Result<Value, AcpError> {
    if prompt.is_empty() {
        return Err(AcpError::PromptInvalid);
    }
    let params = json!({
        "sessionId": validated_session_id(session_id)?,
        "prompt": [{"type": "text", "text": prompt}]
    });
    request_envelope(id.into(), SESSION_PROMPT_METHOD, params)
}

pub fn session_list_request(
    id: impl Into<AcpRequestId>,
    cwd: Option<&str>,
    cursor: Option<&str>,
) -> Result<Value, AcpError> {
    let mut params = Map::new();
    if let Some(cwd) = cwd {
        validate_absolute_working_directory(cwd, AcpError::SessionListRequestInvalid)?;
        params.insert("cwd".into(), Value::String(cwd.to_owned()));
    }
    if let Some(cursor) = cursor {
        normalized_text(
            cursor,
            MAX_CURSOR_BYTES,
            AcpError::SessionListRequestInvalid,
        )?;
        params.insert("cursor".into(), Value::String(cursor.to_owned()));
    }
    request_envelope(id.into(), SESSION_LIST_METHOD, Value::Object(params))
}

pub fn cancel_notification(session_id: &str) -> Result<Value, AcpError> {
    notification_envelope(
        SESSION_CANCEL_METHOD,
        json!({"sessionId": validated_session_id(session_id)?}),
    )
}

pub fn close_session_request(
    id: impl Into<AcpRequestId>,
    session_id: &str,
) -> Result<Value, AcpError> {
    request_envelope(
        id.into(),
        SESSION_CLOSE_METHOD,
        json!({"sessionId": validated_session_id(session_id)?}),
    )
}

fn request_envelope(
    id: AcpRequestId,
    method: &'static str,
    params: Value,
) -> Result<Value, AcpError> {
    id.validate()?;
    let message = json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id.to_value(),
        "method": method,
        "params": params
    });
    ensure_message_limit(&message)?;
    Ok(message)
}

fn notification_envelope(method: &'static str, params: Value) -> Result<Value, AcpError> {
    let message = json!({
        "jsonrpc": JSON_RPC_VERSION,
        "method": method,
        "params": params
    });
    ensure_message_limit(&message)?;
    Ok(message)
}

fn validate_mcp_servers(servers: &[Value]) -> Result<(), AcpError> {
    if servers.len() > MAX_MCP_SERVERS {
        return Err(AcpError::McpServerLimitExceeded);
    }
    let mut names = HashSet::with_capacity(servers.len());
    for server in servers {
        let server_object = server.as_object().ok_or(AcpError::McpServerInvalid)?;
        let name = server_object
            .get("name")
            .and_then(Value::as_str)
            .ok_or(AcpError::McpServerInvalid)?;
        normalized_text(name, MAX_MCP_SERVER_NAME_BYTES, AcpError::McpServerInvalid)?;
        if !names.insert(name) {
            return Err(AcpError::McpServerInvalid);
        }
        if let Some(kind) = server_object.get("type")
            && !matches!(kind.as_str(), Some("http" | "sse"))
        {
            return Err(AcpError::McpServerInvalid);
        }
    }
    Ok(())
}

fn absolute_path_text(path: &Path, error: AcpError) -> Result<String, AcpError> {
    let text = path.to_str().ok_or_else(|| error.clone())?;
    validate_absolute_working_directory(text, error)?;
    Ok(text.to_owned())
}

fn validate_absolute_working_directory(value: &str, error: AcpError) -> Result<(), AcpError> {
    if value.starts_with('/') || Path::new(value).is_absolute() {
        normalized_text(value, DEFAULT_MAX_MESSAGE_BYTES, error)
    } else {
        Err(error)
    }
}
