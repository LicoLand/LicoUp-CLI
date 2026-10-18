use super::super::reserve::{LUNA_RESERVE_MODEL, reserve_model_available};
use super::io::{TransportEvent, drain_stderr, read_protocol_messages, write_message};
use super::launch::CodexLaunchSpec;
use crate::parser::parse_response_line;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, finish_protocol_transport,
};
use serde_json::{Map, Value, json};
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const INITIALIZE_ID: i64 = 91_001;
const MODEL_LIST_ID: i64 = 91_002;
const RATE_LIMITS_ID: i64 = 91_003;
// Product policy: every model-catalog scan waits up to one minute. On timeout
// or failure the Codex catalog still merges config, models_cache.json, and
// model-catalogs; a successful App Server list is merged the same way rather
// than replacing those local sources.
const MODEL_SCAN_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PROTOCOL_BYTES: usize = 2 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 256 * 1024;
const MAX_MODELS: usize = 256;

/// Reads the exact visible model directory exposed by the installed Codex
/// App Server. Only the model projection crosses this boundary; initialize
/// metadata, installation identity, stderr, and notifications are discarded.
pub fn list_models(executable: &Path) -> Result<Value, ()> {
    let executable = executable.to_str().ok_or(())?;
    let mut child = CodexLaunchSpec::new(executable, None)
        .spawn()
        .map_err(|_| ())?;
    let stdout = child.stdout().ok_or(())?;
    let stderr = child.stderr().ok_or(())?;
    let stdin = child.stdin().ok_or(())?;
    let mut stdin = BoundedStdinWriter::new(stdin);
    let (sender, receiver) = mpsc::channel();
    let stdout_handle = thread::spawn(move || {
        read_protocol_messages(BufReader::new(stdout), Some(MAX_PROTOCOL_BYTES), sender)
    });
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_handle = thread::spawn(move || drain_stderr(stderr, MAX_STDERR_BYTES, &stderr_flag));

    let result = (|| {
        write_message(
            &mut stdin,
            &json!({
                "id": INITIALIZE_ID,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "lico-up-model-catalog",
                        "title": "LicoUp",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {"experimentalApi": true}
                }
            }),
        )
        .map_err(|_| ())?;
        let deadline = Instant::now() + MODEL_SCAN_TIMEOUT;
        wait_for_response(&receiver, INITIALIZE_ID, deadline)?;
        write_message(&mut stdin, &json!({"method": "initialized", "params": {}}))
            .map_err(|_| ())?;
        // Read the account capability before projecting the model list. The
        // server response is the only authority that can make the reserve
        // selector visible; a local usage percentage is not sufficient.
        write_message(
            &mut stdin,
            &json!({
                "id": RATE_LIMITS_ID,
                "method": "account/rateLimits/read",
                "params": {"supportsLunaReserve": true}
            }),
        )
        .map_err(|_| ())?;
        write_message(
            &mut stdin,
            &json!({
                "id": MODEL_LIST_ID,
                "method": "model/list",
                "params": {"includeHidden": false, "limit": MAX_MODELS}
            }),
        )
        .map_err(|_| ())?;
        let (response, reserve_available) = wait_for_catalog_responses(&receiver, deadline)?;
        project_model_list_response(&response, reserve_available)
    })();

    let cleanup = finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
    if cleanup.is_err() {
        return Err(());
    }
    result
}

fn wait_for_response(
    receiver: &mpsc::Receiver<TransportEvent>,
    request_id: i64,
    deadline: Instant,
) -> Result<Value, ()> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(());
        }
        match receiver.recv_timeout(deadline - now) {
            Ok(TransportEvent::Line(line)) => {
                if let Some(result) = parse_response_line(&line, request_id)? {
                    return Ok(result);
                }
            }
            Ok(_) | Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {
                return Err(());
            }
        }
    }
}

pub(super) fn wait_for_catalog_responses(
    receiver: &mpsc::Receiver<TransportEvent>,
    deadline: Instant,
) -> Result<(Value, bool), ()> {
    let mut model_list = None::<Value>;
    let mut reserve_available = false;
    let mut rate_limits_received = false;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(());
        }
        match receiver.recv_timeout(deadline - now) {
            Ok(TransportEvent::Line(line)) => {
                let message: Value = serde_json::from_slice(&line).map_err(|_| ())?;
                let Some(id) = message.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                if id == RATE_LIMITS_ID {
                    if let Some(result) = message.get("result") {
                        reserve_available = reserve_model_available(result);
                    }
                    rate_limits_received = true;
                    if let Some(model_list) = model_list.take() {
                        return Ok((model_list, reserve_available));
                    }
                    continue;
                }
                if id == MODEL_LIST_ID {
                    let result = message.get("result").cloned().ok_or(())?;
                    model_list = Some(result);
                    if rate_limits_received {
                        if let Some(model_list) = model_list.take() {
                            return Ok((model_list, reserve_available));
                        }
                    }
                }
            }
            Ok(_) | Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {
                return Err(());
            }
        }
    }
}

pub(super) fn project_model_list_response(
    result: &Value,
    reserve_available: bool,
) -> Result<Value, ()> {
    let models = result.get("data").and_then(Value::as_array).ok_or(())?;
    let mut projected = models
        .iter()
        .filter_map(|model| project_model(model, reserve_available))
        .take(MAX_MODELS)
        .collect::<Vec<_>>();
    if reserve_available
        && !projected.iter().any(|model| {
            model
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(LUNA_RESERVE_MODEL))
        })
        && projected.len() < MAX_MODELS
    {
        projected.push(json!({
            "name": LUNA_RESERVE_MODEL,
            "displayName": "GPT Reserve",
            "reasoningEfforts": [],
            "isDefault": false,
            "ephemeral": true
        }));
    }
    if projected.is_empty() {
        return Err(());
    }
    let default_model = projected
        .iter()
        .find(|model| model.get("isDefault").and_then(Value::as_bool) == Some(true))
        .and_then(|model| model.get("name"))
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    Ok(json!({
        "schemaVersion": 1,
        "status": "available",
        "source": "codex-app-server",
        "defaultModel": default_model,
        "models": projected,
        "diagnostics": []
    }))
}

fn project_model(model: &Value, reserve_available: bool) -> Option<Value> {
    let name = model.get("model")?.as_str()?.trim();
    if name.is_empty() || name.len() > 256 {
        return None;
    }
    let is_reserve = name.eq_ignore_ascii_case(LUNA_RESERVE_MODEL);
    if is_reserve && !reserve_available {
        return None;
    }
    if model.get("hidden").and_then(Value::as_bool) == Some(true)
        && !(reserve_available && is_reserve)
    {
        return None;
    }
    let display_name = model
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .unwrap_or(name);
    let efforts = model
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|effort| effort.get("reasoningEffort").and_then(Value::as_str))
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && effort.len() <= 32)
        .map(Value::from)
        .collect::<Vec<_>>();
    let default_reasoning_effort = model
        .get("defaultReasoningEffort")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && effort.len() <= 32)
        .map(|effort| Value::String(effort.to_owned()))
        .unwrap_or(Value::Null);
    let mut projected = Map::new();
    projected.insert("name".into(), Value::String(name.to_owned()));
    projected.insert("displayName".into(), Value::String(display_name.to_owned()));
    projected.insert("reasoningEfforts".into(), Value::Array(efforts));
    if !default_reasoning_effort.is_null() {
        projected.insert("defaultReasoningEffort".into(), default_reasoning_effort);
    }
    projected.insert(
        "isDefault".into(),
        Value::Bool(!is_reserve && model.get("isDefault").and_then(Value::as_bool) == Some(true)),
    );
    if reserve_available && is_reserve {
        projected.insert("ephemeral".into(), Value::Bool(true));
    }
    Some(Value::Object(projected))
}
