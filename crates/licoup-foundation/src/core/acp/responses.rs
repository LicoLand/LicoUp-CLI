use super::types::validate_implementation_value;
use super::validation::{
    MAX_CURSOR_BYTES, MAX_DISPLAY_TEXT_BYTES, MAX_SESSION_ID_BYTES, ensure_message_limit,
    normalized_text, optional_object, validate_optional_meta, validated_session_id,
};
use super::{
    AcpAgentCapabilities, AcpError, AcpInitializeResponse, AcpPromptResponse, AcpRequestId,
    AcpSessionInfo, AcpSessionListResponse, AcpSessionMethod, AcpSessionResponse, AcpSessionUpdate,
    AcpSessionUpdateKind, AcpStopReason, DEFAULT_MAX_MESSAGE_BYTES, JSON_RPC_VERSION,
    MAX_ADDITIONAL_DIRECTORIES, MAX_SESSION_LIST_ITEMS, PROTOCOL_VERSION, SESSION_UPDATE_METHOD,
};
use serde_json::{Map, Value};

pub fn validate_initialize_response(
    message: &Value,
    expected_id: impl Into<AcpRequestId>,
) -> Result<AcpInitializeResponse, AcpError> {
    let result = validated_result(message, expected_id.into())?
        .as_object()
        .ok_or(AcpError::ResultInvalid)?;
    let protocol_version = result
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .and_then(|version| u16::try_from(version).ok())
        .ok_or(AcpError::ProtocolVersionInvalid)?;
    if protocol_version != PROTOCOL_VERSION {
        return Err(AcpError::UnsupportedProtocolVersion {
            received: protocol_version,
        });
    }
    if let Some(auth_methods) = result.get("authMethods")
        && !auth_methods.is_array()
    {
        return Err(AcpError::CapabilityInvalid);
    }
    if let Some(agent_info) = result.get("agentInfo")
        && !agent_info.is_null()
    {
        validate_implementation_value(agent_info)?;
    }

    let empty_capabilities = Map::new();
    let capabilities =
        optional_object(result.get("agentCapabilities"), AcpError::CapabilityInvalid)?
            .unwrap_or(&empty_capabilities);
    let empty_session_capabilities = Map::new();
    let session_capabilities = optional_object(
        capabilities.get("sessionCapabilities"),
        AcpError::CapabilityInvalid,
    )?
    .unwrap_or(&empty_session_capabilities);
    let empty_prompt_capabilities = Map::new();
    let prompt_capabilities = optional_object(
        capabilities.get("promptCapabilities"),
        AcpError::CapabilityInvalid,
    )?
    .unwrap_or(&empty_prompt_capabilities);
    let empty_mcp_capabilities = Map::new();
    let mcp_capabilities = optional_object(
        capabilities.get("mcpCapabilities"),
        AcpError::CapabilityInvalid,
    )?
    .unwrap_or(&empty_mcp_capabilities);

    Ok(AcpInitializeResponse {
        protocol_version,
        capabilities: AcpAgentCapabilities {
            load_session: optional_bool(capabilities, "loadSession")?,
            resume_session: capability_marker(session_capabilities, "resume")?,
            close_session: capability_marker(session_capabilities, "close")?,
            list_sessions: capability_marker(session_capabilities, "list")?,
            delete_session: capability_marker(session_capabilities, "delete")?,
            additional_directories: capability_marker(
                session_capabilities,
                "additionalDirectories",
            )?,
            image_prompts: optional_bool(prompt_capabilities, "image")?,
            audio_prompts: optional_bool(prompt_capabilities, "audio")?,
            embedded_context: optional_bool(prompt_capabilities, "embeddedContext")?,
            mcp_http: optional_bool(mcp_capabilities, "http")?,
            mcp_sse: optional_bool(mcp_capabilities, "sse")?,
        },
    })
}

pub fn validate_session_response(
    message: &Value,
    expected_id: impl Into<AcpRequestId>,
    method: AcpSessionMethod<'_>,
) -> Result<AcpSessionResponse, AcpError> {
    let result = validated_result(message, expected_id.into())?;
    match method {
        AcpSessionMethod::Load(requested) => {
            validated_session_id(requested)?;
            if result.is_null() {
                return Ok(AcpSessionResponse {
                    session_id: None,
                    modes: None,
                    config_options: Vec::new(),
                });
            }
        }
        AcpSessionMethod::Resume(requested) => {
            validated_session_id(requested)?;
        }
        AcpSessionMethod::New => {}
    }
    let result = result.as_object().ok_or(AcpError::SessionResponseInvalid)?;
    let session_id = result
        .get("sessionId")
        .map(|value| {
            value
                .as_str()
                .ok_or(AcpError::SessionResponseInvalid)
                .and_then(validated_session_id)
                .map(str::to_owned)
        })
        .transpose()?;
    if matches!(method, AcpSessionMethod::New) && session_id.is_none() {
        return Err(AcpError::SessionResponseInvalid);
    }
    let modes = match result.get("modes") {
        None | Some(Value::Null) => None,
        Some(value @ Value::Object(_)) => {
            validate_session_modes(value)?;
            Some(value.clone())
        }
        Some(_) => return Err(AcpError::SessionResponseInvalid),
    };
    let config_options = match result.get("configOptions") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(options)) => {
            validate_config_options(options, AcpError::SessionResponseInvalid)?;
            options.clone()
        }
        Some(_) => return Err(AcpError::SessionResponseInvalid),
    };
    Ok(AcpSessionResponse {
        session_id,
        modes,
        config_options,
    })
}

pub fn validate_session_list_response(
    message: &Value,
    expected_id: impl Into<AcpRequestId>,
) -> Result<AcpSessionListResponse, AcpError> {
    let result = validated_result(message, expected_id.into())?
        .as_object()
        .ok_or(AcpError::SessionListResponseInvalid)?;
    if !result
        .keys()
        .all(|key| matches!(key.as_str(), "sessions" | "nextCursor" | "_meta"))
    {
        return Err(AcpError::SessionListResponseInvalid);
    }
    validate_optional_meta(result.get("_meta"), AcpError::SessionListResponseInvalid)?;
    let raw_sessions = result
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or(AcpError::SessionListResponseInvalid)?;
    if raw_sessions.len() > MAX_SESSION_LIST_ITEMS {
        return Err(AcpError::SessionListLimitExceeded);
    }
    let sessions = raw_sessions
        .iter()
        .map(validate_session_info)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = match result.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => {
            normalized_text(
                value,
                MAX_CURSOR_BYTES,
                AcpError::SessionListResponseInvalid,
            )?;
            Some(value.clone())
        }
        Some(_) => return Err(AcpError::SessionListResponseInvalid),
    };
    Ok(AcpSessionListResponse {
        sessions,
        next_cursor,
    })
}

fn validate_session_info(value: &Value) -> Result<AcpSessionInfo, AcpError> {
    let value = value
        .as_object()
        .ok_or(AcpError::SessionListResponseInvalid)?;
    if !value.keys().all(|key| {
        matches!(
            key.as_str(),
            "sessionId" | "cwd" | "additionalDirectories" | "title" | "updatedAt" | "_meta"
        )
    }) {
        return Err(AcpError::SessionListResponseInvalid);
    }
    let session_id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or(AcpError::SessionListResponseInvalid)
        .and_then(validated_session_id)?
        .to_owned();
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or(AcpError::SessionListResponseInvalid)?;
    normalized_text(
        cwd,
        DEFAULT_MAX_MESSAGE_BYTES,
        AcpError::SessionListResponseInvalid,
    )?;
    if !cwd.starts_with('/') && !std::path::Path::new(cwd).is_absolute() {
        return Err(AcpError::SessionListResponseInvalid);
    }
    let additional_directories = match value.get("additionalDirectories") {
        None => Vec::new(),
        Some(Value::Array(directories)) => {
            if directories.len() > MAX_ADDITIONAL_DIRECTORIES {
                return Err(AcpError::AdditionalDirectoryLimitExceeded);
            }
            directories
                .iter()
                .map(|directory| {
                    let directory = directory
                        .as_str()
                        .ok_or(AcpError::SessionListResponseInvalid)?;
                    normalized_text(
                        directory,
                        DEFAULT_MAX_MESSAGE_BYTES,
                        AcpError::SessionListResponseInvalid,
                    )?;
                    if !directory.starts_with('/') && !std::path::Path::new(directory).is_absolute()
                    {
                        return Err(AcpError::SessionListResponseInvalid);
                    }
                    Ok(directory.to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        Some(_) => return Err(AcpError::SessionListResponseInvalid),
    };
    let title = optional_session_info_text(value, "title")?;
    let updated_at = optional_session_info_text(value, "updatedAt")?;
    let meta = match value.get("_meta") {
        None | Some(Value::Null) => None,
        Some(Value::Object(meta)) => Some(meta.clone()),
        Some(_) => return Err(AcpError::SessionListResponseInvalid),
    };
    Ok(AcpSessionInfo {
        session_id,
        cwd: cwd.to_owned(),
        additional_directories,
        title,
        updated_at,
        meta,
    })
}

fn optional_session_info_text(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, AcpError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            normalized_text(
                value,
                DEFAULT_MAX_MESSAGE_BYTES,
                AcpError::SessionListResponseInvalid,
            )?;
            Ok(Some(value.clone()))
        }
        Some(_) => Err(AcpError::SessionListResponseInvalid),
    }
}

pub fn validate_prompt_response(
    message: &Value,
    expected_id: impl Into<AcpRequestId>,
) -> Result<AcpPromptResponse, AcpError> {
    let result = validated_result(message, expected_id.into())?
        .as_object()
        .ok_or(AcpError::ResultInvalid)?;
    let stop_reason = match result.get("stopReason").and_then(Value::as_str) {
        Some("end_turn") => AcpStopReason::EndTurn,
        Some("max_tokens") => AcpStopReason::MaxTokens,
        Some("max_turn_requests") => AcpStopReason::MaxTurnRequests,
        Some("refusal") => AcpStopReason::Refusal,
        Some("cancelled") => AcpStopReason::Cancelled,
        Some(_) => return Err(AcpError::StopReasonInvalid),
        None => return Err(AcpError::PromptResponseInvalid),
    };
    Ok(AcpPromptResponse { stop_reason })
}

pub fn validate_session_update(
    message: &Value,
    expected_session_id: Option<&str>,
) -> Result<AcpSessionUpdate, AcpError> {
    ensure_message_limit(message)?;
    let envelope = message
        .as_object()
        .ok_or(AcpError::NotificationEnvelopeInvalid)?;
    if !envelope
        .keys()
        .all(|key| matches!(key.as_str(), "jsonrpc" | "method" | "params"))
    {
        return Err(AcpError::NotificationEnvelopeInvalid);
    }
    if envelope.get("jsonrpc").and_then(Value::as_str) != Some(JSON_RPC_VERSION) {
        return Err(AcpError::JsonRpcVersionInvalid);
    }
    if envelope.get("method").and_then(Value::as_str) != Some(SESSION_UPDATE_METHOD) {
        return Err(AcpError::NotificationMethodInvalid);
    }
    let params = envelope
        .get("params")
        .and_then(Value::as_object)
        .ok_or(AcpError::SessionUpdateInvalid)?;
    if !params
        .keys()
        .all(|key| matches!(key.as_str(), "sessionId" | "update" | "_meta"))
    {
        return Err(AcpError::SessionUpdateInvalid);
    }
    validate_optional_meta(params.get("_meta"), AcpError::SessionUpdateInvalid)?;
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or(AcpError::SessionUpdateInvalid)
        .and_then(validated_session_id)?;
    if let Some(expected_session_id) = expected_session_id {
        validated_session_id(expected_session_id)?;
        if session_id != expected_session_id {
            return Err(AcpError::SessionMismatch);
        }
    }
    let update = params
        .get("update")
        .and_then(Value::as_object)
        .ok_or(AcpError::SessionUpdateInvalid)?;
    let kind = match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("user_message_chunk") => AcpSessionUpdateKind::UserMessageChunk,
        Some("agent_message_chunk") => AcpSessionUpdateKind::AgentMessageChunk,
        Some("agent_thought_chunk") => AcpSessionUpdateKind::AgentThoughtChunk,
        Some("tool_call") => AcpSessionUpdateKind::ToolCall,
        Some("tool_call_update") => AcpSessionUpdateKind::ToolCallUpdate,
        Some("plan") => AcpSessionUpdateKind::Plan,
        Some("available_commands_update") => AcpSessionUpdateKind::AvailableCommandsUpdate,
        Some("current_mode_update") => AcpSessionUpdateKind::CurrentModeUpdate,
        Some("config_option_update") => AcpSessionUpdateKind::ConfigOptionUpdate,
        Some("session_info_update") => AcpSessionUpdateKind::SessionInfoUpdate,
        Some("usage_update") => AcpSessionUpdateKind::UsageUpdate,
        _ => return Err(AcpError::SessionUpdateInvalid),
    };
    validate_optional_meta(update.get("_meta"), AcpError::SessionUpdateInvalid)?;
    match kind {
        AcpSessionUpdateKind::UserMessageChunk
        | AcpSessionUpdateKind::AgentMessageChunk
        | AcpSessionUpdateKind::AgentThoughtChunk => validate_content_block(
            update
                .get("content")
                .ok_or(AcpError::SessionUpdateInvalid)?,
        )?,
        AcpSessionUpdateKind::ToolCall => validate_tool_call(update, true)?,
        AcpSessionUpdateKind::ToolCallUpdate => validate_tool_call(update, false)?,
        AcpSessionUpdateKind::Plan => validate_plan_update(update)?,
        AcpSessionUpdateKind::AvailableCommandsUpdate => {
            validate_available_commands_update(update)?
        }
        AcpSessionUpdateKind::CurrentModeUpdate => {
            validate_required_text(update, "currentModeId", AcpError::SessionUpdateInvalid)?;
        }
        AcpSessionUpdateKind::ConfigOptionUpdate => {
            let options = update
                .get("configOptions")
                .and_then(Value::as_array)
                .ok_or(AcpError::SessionUpdateInvalid)?;
            validate_config_options(options, AcpError::SessionUpdateInvalid)?;
        }
        AcpSessionUpdateKind::SessionInfoUpdate => validate_session_info_update(update)?,
        AcpSessionUpdateKind::UsageUpdate => validate_usage_update(update)?,
    }
    Ok(AcpSessionUpdate {
        session_id: session_id.to_owned(),
        kind,
        update: Value::Object(update.clone()),
    })
}

pub fn validate_close_session_response(
    message: &Value,
    expected_id: impl Into<AcpRequestId>,
) -> Result<(), AcpError> {
    let result = validated_result(message, expected_id.into())?
        .as_object()
        .ok_or(AcpError::CloseResponseInvalid)?;
    if !result.keys().all(|key| key == "_meta") {
        return Err(AcpError::CloseResponseInvalid);
    }
    validate_optional_meta(result.get("_meta"), AcpError::CloseResponseInvalid)
}

fn validated_result(message: &Value, expected_id: AcpRequestId) -> Result<&Value, AcpError> {
    expected_id.validate()?;
    ensure_message_limit(message)?;
    let envelope = message
        .as_object()
        .ok_or(AcpError::ResponseEnvelopeInvalid)?;
    if !envelope
        .keys()
        .all(|key| matches!(key.as_str(), "jsonrpc" | "id" | "result" | "error"))
    {
        return Err(AcpError::ResponseEnvelopeInvalid);
    }
    if envelope.get("jsonrpc").and_then(Value::as_str) != Some(JSON_RPC_VERSION) {
        return Err(AcpError::JsonRpcVersionInvalid);
    }
    let response_id = envelope
        .get("id")
        .ok_or(AcpError::ResponseIdInvalid)
        .and_then(AcpRequestId::from_value)?;
    if response_id != expected_id {
        return Err(AcpError::ResponseIdMismatch);
    }
    let has_result = envelope.contains_key("result");
    let has_error = envelope.contains_key("error");
    if has_result == has_error {
        return Err(AcpError::ResponseOutcomeInvalid);
    }
    if let Some(error) = envelope.get("error") {
        let error = error.as_object().ok_or(AcpError::ResponseOutcomeInvalid)?;
        if !error
            .keys()
            .all(|key| matches!(key.as_str(), "code" | "message" | "data"))
        {
            return Err(AcpError::ResponseOutcomeInvalid);
        }
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .ok_or(AcpError::ResponseOutcomeInvalid)?;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .ok_or(AcpError::ResponseOutcomeInvalid)?;
        normalized_text(
            message,
            DEFAULT_MAX_MESSAGE_BYTES,
            AcpError::ResponseOutcomeInvalid,
        )?;
        return Err(AcpError::RemoteError { code });
    }
    envelope.get("result").ok_or(AcpError::ResultInvalid)
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> Result<bool, AcpError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(AcpError::CapabilityInvalid),
    }
}

fn capability_marker(object: &Map<String, Value>, key: &str) -> Result<bool, AcpError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Object(_)) => Ok(true),
        Some(_) => Err(AcpError::CapabilityInvalid),
    }
}

fn validate_session_modes(value: &Value) -> Result<(), AcpError> {
    let modes = value.as_object().ok_or(AcpError::SessionResponseInvalid)?;
    validate_required_text(modes, "currentModeId", AcpError::SessionResponseInvalid)?;
    validate_optional_meta(modes.get("_meta"), AcpError::SessionResponseInvalid)?;
    let available_modes = modes
        .get("availableModes")
        .and_then(Value::as_array)
        .ok_or(AcpError::SessionResponseInvalid)?;
    for mode in available_modes {
        let mode = mode.as_object().ok_or(AcpError::SessionResponseInvalid)?;
        validate_required_text(mode, "id", AcpError::SessionResponseInvalid)?;
        validate_required_text(mode, "name", AcpError::SessionResponseInvalid)?;
        validate_optional_text(mode, "description", AcpError::SessionResponseInvalid)?;
        validate_optional_meta(mode.get("_meta"), AcpError::SessionResponseInvalid)?;
    }
    Ok(())
}

fn validate_config_options(options: &[Value], error: AcpError) -> Result<(), AcpError> {
    for option in options {
        validate_config_option(option, error.clone())?;
    }
    Ok(())
}

fn validate_config_option(option: &Value, error: AcpError) -> Result<(), AcpError> {
    let option = option.as_object().ok_or_else(|| error.clone())?;
    validate_required_text(option, "id", error.clone())?;
    validate_required_text(option, "name", error.clone())?;
    validate_optional_text(option, "description", error.clone())?;
    validate_optional_text(option, "category", error.clone())?;
    validate_optional_meta(option.get("_meta"), error.clone())?;
    match option.get("type").and_then(Value::as_str) {
        Some("boolean") if matches!(option.get("currentValue"), Some(Value::Bool(_))) => Ok(()),
        Some("select") => {
            validate_config_select_current_value(option, error.clone())?;
            let values = option
                .get("options")
                .and_then(Value::as_array)
                .ok_or_else(|| error.clone())?;
            let grouped = values.iter().any(|value| {
                value
                    .as_object()
                    .is_some_and(|value| value.contains_key("group"))
            });
            for value in values {
                if grouped {
                    validate_config_select_group(value, error.clone())?;
                } else {
                    validate_config_select_value(value, error.clone())?;
                }
            }
            Ok(())
        }
        _ => Err(error),
    }
}

/// A select current value must be a bounded, normalized string. Vendor
/// agents (real Copilot) advertise an empty string for "unset/default", so
/// empty is accepted where the option id and names remain strictly required.
fn validate_config_select_current_value(
    option: &Map<String, Value>,
    error: AcpError,
) -> Result<(), AcpError> {
    let value = option
        .get("currentValue")
        .and_then(Value::as_str)
        .ok_or_else(|| error.clone())?;
    if value.len() > MAX_SESSION_ID_BYTES || value.trim() != value {
        return Err(error);
    }
    Ok(())
}

fn validate_config_select_value(value: &Value, error: AcpError) -> Result<(), AcpError> {
    let value = value.as_object().ok_or_else(|| error.clone())?;
    // Vendor agents use an empty select value for the "default/unset" entry.
    if value
        .get("value")
        .and_then(Value::as_str)
        .is_none_or(|text| text.len() > MAX_SESSION_ID_BYTES || text.trim() != text)
    {
        return Err(error);
    }
    validate_required_text(value, "name", error.clone())?;
    validate_optional_text(value, "description", error.clone())?;
    validate_optional_meta(value.get("_meta"), error)
}

fn validate_config_select_group(value: &Value, error: AcpError) -> Result<(), AcpError> {
    let value = value.as_object().ok_or_else(|| error.clone())?;
    validate_required_text(value, "group", error.clone())?;
    validate_required_text(value, "name", error.clone())?;
    validate_optional_meta(value.get("_meta"), error.clone())?;
    let options = value
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| error.clone())?;
    for option in options {
        validate_config_select_value(option, error.clone())?;
    }
    Ok(())
}

fn validate_content_block(content: &Value) -> Result<(), AcpError> {
    let content = content.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
    validate_optional_meta(content.get("_meta"), AcpError::SessionUpdateInvalid)?;
    match content.get("type").and_then(Value::as_str) {
        Some("text") if content.get("text").is_some_and(Value::is_string) => Ok(()),
        Some("image") | Some("audio")
            if content.get("data").is_some_and(Value::is_string)
                && content.get("mimeType").is_some_and(Value::is_string) =>
        {
            Ok(())
        }
        Some("resource_link")
            if content.get("uri").is_some_and(Value::is_string)
                && content.get("name").is_some_and(Value::is_string) =>
        {
            Ok(())
        }
        Some("resource") => {
            let resource = content
                .get("resource")
                .and_then(Value::as_object)
                .ok_or(AcpError::SessionUpdateInvalid)?;
            validate_optional_meta(resource.get("_meta"), AcpError::SessionUpdateInvalid)?;
            if !resource.get("uri").is_some_and(Value::is_string)
                || !(resource.get("text").is_some_and(Value::is_string)
                    || resource.get("blob").is_some_and(Value::is_string))
            {
                return Err(AcpError::SessionUpdateInvalid);
            }
            Ok(())
        }
        _ => Err(AcpError::SessionUpdateInvalid),
    }
}

fn validate_tool_call(update: &Map<String, Value>, title_required: bool) -> Result<(), AcpError> {
    validate_required_text(update, "toolCallId", AcpError::SessionUpdateInvalid)?;
    if title_required {
        validate_required_text(update, "title", AcpError::SessionUpdateInvalid)?;
    } else {
        validate_optional_text(update, "title", AcpError::SessionUpdateInvalid)?;
    }
    validate_optional_enum(
        update,
        "kind",
        &[
            "read",
            "edit",
            "delete",
            "move",
            "search",
            "execute",
            "think",
            "fetch",
            "switch_mode",
            "other",
        ],
    )?;
    validate_optional_enum(
        update,
        "status",
        &["pending", "in_progress", "completed", "failed"],
    )?;
    if let Some(content) = update.get("content") {
        let content = content.as_array().ok_or(AcpError::SessionUpdateInvalid)?;
        for item in content {
            validate_tool_call_content(item)?;
        }
    }
    if let Some(locations) = update.get("locations") {
        let locations = locations.as_array().ok_or(AcpError::SessionUpdateInvalid)?;
        for location in locations {
            validate_tool_call_location(location)?;
        }
    }
    Ok(())
}

fn validate_tool_call_content(value: &Value) -> Result<(), AcpError> {
    let value = value.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
    validate_optional_meta(value.get("_meta"), AcpError::SessionUpdateInvalid)?;
    match value.get("type").and_then(Value::as_str) {
        Some("content") => {
            validate_content_block(value.get("content").ok_or(AcpError::SessionUpdateInvalid)?)
        }
        Some("terminal") => {
            validate_required_text(value, "terminalId", AcpError::SessionUpdateInvalid)
        }
        Some("diff") => {
            validate_required_text(value, "path", AcpError::SessionUpdateInvalid)?;
            if !value.get("newText").is_some_and(Value::is_string) {
                return Err(AcpError::SessionUpdateInvalid);
            }
            match value.get("oldText") {
                None | Some(Value::Null) | Some(Value::String(_)) => Ok(()),
                Some(_) => Err(AcpError::SessionUpdateInvalid),
            }
        }
        _ => Err(AcpError::SessionUpdateInvalid),
    }
}

fn validate_tool_call_location(value: &Value) -> Result<(), AcpError> {
    let value = value.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
    validate_required_text(value, "path", AcpError::SessionUpdateInvalid)?;
    validate_optional_meta(value.get("_meta"), AcpError::SessionUpdateInvalid)?;
    match value.get("line") {
        None | Some(Value::Null) => Ok(()),
        Some(line)
            if line
                .as_u64()
                .is_some_and(|line| u32::try_from(line).is_ok()) =>
        {
            Ok(())
        }
        Some(_) => Err(AcpError::SessionUpdateInvalid),
    }
}

fn validate_plan_update(update: &Map<String, Value>) -> Result<(), AcpError> {
    let entries = update
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(AcpError::SessionUpdateInvalid)?;
    for entry in entries {
        let entry = entry.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
        validate_required_text(entry, "content", AcpError::SessionUpdateInvalid)?;
        validate_required_enum(
            entry,
            "priority",
            &["high", "medium", "low"],
            AcpError::SessionUpdateInvalid,
        )?;
        validate_required_enum(
            entry,
            "status",
            &["pending", "in_progress", "completed"],
            AcpError::SessionUpdateInvalid,
        )?;
        validate_optional_meta(entry.get("_meta"), AcpError::SessionUpdateInvalid)?;
    }
    Ok(())
}

fn validate_available_commands_update(update: &Map<String, Value>) -> Result<(), AcpError> {
    let commands = update
        .get("availableCommands")
        .and_then(Value::as_array)
        .ok_or(AcpError::SessionUpdateInvalid)?;
    for command in commands {
        let command = command.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
        validate_required_display_text(command, "name")?;
        validate_required_display_text(command, "description")?;
        validate_optional_meta(command.get("_meta"), AcpError::SessionUpdateInvalid)?;
        if let Some(input) = command.get("input")
            && !input.is_null()
        {
            let input = input.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
            validate_required_display_text(input, "hint")?;
            validate_optional_meta(input.get("_meta"), AcpError::SessionUpdateInvalid)?;
        }
    }
    Ok(())
}

/// Display strings (command names, descriptions, hints) carry third-party
/// text that real agents do not whitespace-normalize: Copilot advertises
/// skill descriptions with surrounding whitespace. Unlike identifiers,
/// surrounding whitespace is tolerated while emptiness and size stay bounded.
fn validate_required_display_text(object: &Map<String, Value>, key: &str) -> Result<(), AcpError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or(AcpError::SessionUpdateInvalid)?;
    if value.trim().is_empty() || value.len() > MAX_DISPLAY_TEXT_BYTES {
        return Err(AcpError::SessionUpdateInvalid);
    }
    Ok(())
}

fn validate_session_info_update(update: &Map<String, Value>) -> Result<(), AcpError> {
    validate_optional_text(update, "title", AcpError::SessionUpdateInvalid)?;
    validate_optional_text(update, "updatedAt", AcpError::SessionUpdateInvalid)
}

fn validate_usage_update(update: &Map<String, Value>) -> Result<(), AcpError> {
    if update.get("used").and_then(Value::as_u64).is_none()
        || update.get("size").and_then(Value::as_u64).is_none()
    {
        return Err(AcpError::SessionUpdateInvalid);
    }
    if let Some(cost) = update.get("cost")
        && !cost.is_null()
    {
        let cost = cost.as_object().ok_or(AcpError::SessionUpdateInvalid)?;
        cost.get("amount")
            .and_then(Value::as_f64)
            .filter(|amount| amount.is_finite() && *amount >= 0.0)
            .ok_or(AcpError::SessionUpdateInvalid)?;
        validate_required_text(cost, "currency", AcpError::SessionUpdateInvalid)?;
        validate_optional_meta(cost.get("_meta"), AcpError::SessionUpdateInvalid)?;
    }
    Ok(())
}

fn validate_required_text(
    object: &Map<String, Value>,
    key: &str,
    error: AcpError,
) -> Result<(), AcpError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error.clone())?;
    normalized_text(value, MAX_SESSION_ID_BYTES, error)
}

fn validate_optional_text(
    object: &Map<String, Value>,
    key: &str,
    error: AcpError,
) -> Result<(), AcpError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) => normalized_text(value, MAX_SESSION_ID_BYTES, error),
        Some(_) => Err(error),
    }
}

fn validate_required_enum(
    object: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
    error: AcpError,
) -> Result<(), AcpError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error.clone())?;
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(error)
    }
}

fn validate_optional_enum(
    object: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Result<(), AcpError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => Ok(()),
        Some(_) => Err(AcpError::SessionUpdateInvalid),
    }
}
