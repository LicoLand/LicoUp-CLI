use super::*;

pub trait SecureCommandLocalExecutor {
    fn execute_secure_command(&mut self, payload: &SecureCommandPayload) -> Result<Value>;
}

#[derive(Debug)]
pub(crate) struct SecureAgentDispatchFailure {
    pub(crate) code: &'static str,
    pub(crate) retryable: bool,
}

impl SecureAgentDispatchFailure {
    pub(crate) const fn new(code: &'static str, retryable: bool) -> Self {
        Self { code, retryable }
    }

    pub(crate) fn from_adapter_code(code: &str) -> Self {
        let code = code.to_ascii_lowercase();
        if code.contains("timeout") {
            Self::new("native_agent_timeout", true)
        } else if code.contains("cancel") || code.contains("interrupt") {
            Self::new("native_agent_cancelled", true)
        } else if code.contains("approval")
            || code.contains("permission")
            || code.contains("interaction")
        {
            Self::new("native_agent_user_interaction_required", true)
        } else if code.contains("login") || code.contains("auth") || code.contains("credential") {
            Self::new("native_agent_authentication_required", true)
        } else if code.contains("session") || code.contains("resume") || code.contains("thread") {
            Self::new("native_agent_session_rejected", false)
        } else if code.contains("model") || code.contains("setting") || code.contains("effort") {
            Self::new("native_agent_configuration_rejected", false)
        } else if code.contains("output_limit") || code.contains("stdout_limit") {
            Self::new("native_agent_output_limit", false)
        } else if code.contains("process")
            || code.contains("executable")
            || code.contains("unavailable")
        {
            Self::new("native_agent_runtime_unavailable", true)
        } else {
            Self::new("native_agent_dispatch_failed", false)
        }
    }
}

impl fmt::Display for SecureAgentDispatchFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl StdError for SecureAgentDispatchFailure {}

pub(crate) fn dispatch_ready_agent_message<F>(params: &Value, dispatch_lane: F) -> Result<Value>
where
    F: FnOnce(&str, &Value) -> Result<Value>,
{
    successful_agent_dispatch(dispatch_lane("send", params))
}

pub(super) fn successful_agent_dispatch(result: Result<Value>) -> Result<Value> {
    let result = result?;
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        let code = result
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .or_else(|| result.get("code").and_then(Value::as_str))
            .unwrap_or_default();
        return Err(anyhow::Error::new(
            SecureAgentDispatchFailure::from_adapter_code(code),
        ));
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecureCommandExecutionOutcome {
    Result(SecureMeshResultPayload),
    Error(SecureMeshErrorPayload),
}

impl SecureCommandExecutionOutcome {
    pub fn result(self) -> Option<SecureMeshResultPayload> {
        match self {
            Self::Result(result) => Some(result),
            Self::Error(_) => None,
        }
    }

    pub fn error(self) -> Option<SecureMeshErrorPayload> {
        match self {
            Self::Result(_) => None,
            Self::Error(error) => Some(error),
        }
    }
}

pub fn execute_evaluated_secure_command(
    payload: &SecureCommandPayload,
    evaluation: &SecureCommandEvaluation,
    executor: &mut impl SecureCommandLocalExecutor,
    completed_at: impl Into<String>,
) -> Result<SecureCommandExecutionOutcome> {
    let completed_at = completed_at.into();
    ensure!(
        !completed_at.trim().is_empty(),
        "secure mesh command execution completedAt is required"
    );
    ensure!(
        evaluation.command_id == payload.command_id
            && evaluation.command_kind == payload.command_kind
            && evaluation.risk_class == payload.risk_class.as_str(),
        "secure mesh command execution evaluation does not match payload"
    );
    if !evaluation.should_execute {
        return Ok(SecureCommandExecutionOutcome::Error(command_error_payload(
            payload,
            &evaluation.code,
            evaluation.accepted && !evaluation.replayed,
            &completed_at,
            &evaluation.reason,
        )));
    }
    let output = match executor.execute_secure_command(payload) {
        Ok(output) => output,
        Err(error) => {
            let classified = error.downcast_ref::<SecureAgentDispatchFailure>();
            return Ok(SecureCommandExecutionOutcome::Error(command_error_payload(
                payload,
                classified
                    .map(|failure| failure.code)
                    .unwrap_or("local_execution_failed"),
                classified.map(|failure| failure.retryable).unwrap_or(true),
                &completed_at,
                LOCAL_EXECUTION_FAILED_REMOTE_DETAIL,
            )));
        }
    };
    Ok(SecureCommandExecutionOutcome::Result(
        SecureMeshResultPayload {
            command_id: payload.command_id.clone(),
            idempotency_key: payload.idempotency_key.clone(),
            output_content_type: "application/json".to_string(),
            completed_at,
            output: serde_json::to_vec(&json!({
                "ok": true,
                "commandKind": payload.command_kind,
                "output": output,
            }))?,
        },
    ))
}

pub(crate) fn agent_sessions_list_params(payload: &SecureCommandPayload) -> Result<Value> {
    let body = filtered_body(payload.body(), AGENT_SESSIONS_LIST_PAYLOAD_FIELDS)?;
    let agent = bound_agent_id(payload, "agent.sessions.list")?;
    let mut params = Map::new();
    params.insert("agent".to_string(), json!(agent));
    if let Some(limit) = body.get("limit").and_then(Value::as_u64) {
        params.insert("limit".to_string(), json!(limit.min(100)));
    }
    if let Some(offset) = body.get("offset").and_then(Value::as_u64) {
        params.insert("offset".to_string(), json!(offset));
    }
    Ok(Value::Object(params))
}

pub(crate) fn agent_sessions_describe_params(payload: &SecureCommandPayload) -> Result<Value> {
    let mut allowed_fields = AGENT_SESSIONS_DESCRIBE_PAYLOAD_FIELDS.to_vec();
    allowed_fields.extend(["messageBefore", "messageLimit"]);
    let body = filtered_body(payload.body(), &allowed_fields)?;
    let agent = bound_agent_id(payload, "agent.sessions.describe")?;
    let session_id = text_from_any(&body, &["sessionId", "nativeSessionId"]).ok_or_else(|| {
        anyhow!("secure mesh command agent.sessions.describe requires session id")
    })?;
    let message_limit = body
        .get("messageLimit")
        .and_then(Value::as_u64)
        .map(usize::try_from)
        .transpose()
        .map_err(|_| anyhow!("secure mesh command message limit is invalid"))?;
    if let Some(limit) = message_limit {
        ensure!(
            (1..=100).contains(&limit),
            "secure mesh command message limit is invalid"
        );
    }
    Ok(json!({
        "agent": agent,
        "sessionId": session_id,
        "limit": 1,
        "offset": 0,
        "messageBefore": body.get("messageBefore").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()),
        "messageLimit": message_limit,
    }))
}

pub(crate) fn agent_message_send_params(payload: &SecureCommandPayload) -> Result<Value> {
    let mut body = filtered_body(payload.body(), AGENT_MESSAGE_SEND_PAYLOAD_FIELDS)?;
    let agent = bound_agent_id(payload, "agent.message.send")?;
    ensure!(
        text_from_any(&body, &["text", "message", "prompt"]).is_some(),
        "secure mesh command agent.message.send requires message text"
    );
    body["agent"] = json!(agent);
    body["idempotencyKey"] = json!(payload.idempotency_key);
    body["timeoutMs"] = json!(SECURE_AGENT_MESSAGE_TIMEOUT_MS);
    Ok(body)
}

fn bound_agent_id(payload: &SecureCommandPayload, command_kind: &str) -> Result<String> {
    payload
        .target_binding
        .target_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("secure mesh command {command_kind} requires bound agent id"))
}

fn filtered_body(body: &Value, allowed_fields: &[&str]) -> Result<Value> {
    let object = body
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh command body must be a JSON object"))?;
    let mut out = Map::new();
    for (key, value) in object {
        if COMMAND_BODY_DENIED_EXECUTION_FIELDS.contains(&key.as_str()) {
            bail!(
                "secure mesh command body cannot carry unscoped execution field: {}",
                key
            );
        }
        ensure!(
            allowed_fields.contains(&key.as_str()),
            "secure mesh command body field is not enabled for scoped execution: {}",
            key
        );
        out.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(out))
}

fn text_from_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn command_error_payload(
    payload: &SecureCommandPayload,
    error_code: &str,
    retryable: bool,
    occurred_at: &str,
    error_detail: &str,
) -> SecureMeshErrorPayload {
    SecureMeshErrorPayload {
        command_id: payload.command_id.clone(),
        idempotency_key: payload.idempotency_key.clone(),
        error_code: error_code.to_string(),
        retryable,
        occurred_at: occurred_at.to_string(),
        error_detail: error_detail.to_string(),
    }
}
