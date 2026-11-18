//! The application payload carried inside LicoArc session records.
//!
//! A paired device sends one [`DeviceCommand`] per record and receives one
//! [`DeviceCommandResult`] back. The sender's identity is not a payload field:
//! it is the LicoArc session's own trust fact, resolved by the caller to the
//! paired peer the session belongs to. This module owns the payload shape, the
//! receive policy, the replay ledger keyed by peer and command id (delivery is
//! at-least-once) and the result shaping; the local executor is a port.

use anyhow::{Context, Result, anyhow, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{error::Error as StdError, fmt, path::Path};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const DEVICE_COMMAND_SCHEMA: &str = "licoup.device-command.v1";
pub const DEVICE_RESULT_SCHEMA: &str = "licoup.device-result.v1";
pub const AGENT_MESSAGE_SEND: &str = "agent.message.send";

const ALLOWED_COMMANDS: &[&str] = &[AGENT_MESSAGE_SEND];
const DENIED_PREFIXES: &[&str] = &[
    "shell.",
    "filesystem.",
    "process.spawn.",
    "runtime.raw.",
    "mcp.raw.",
    "settings.write.",
    "secrets.",
    "network.raw.",
    "external.unscoped.",
    "tool.unbounded.",
];
const MAX_ID_BYTES: usize = 255;
const MAX_PARAMS_BYTES: usize = 1024 * 1024;
const AGENT_MESSAGE_SEND_FIELDS: &[&str] = &[
    "agent",
    "text",
    "message",
    "prompt",
    "model",
    "modelId",
    "reasoningEffort",
    "reasoning_effort",
    "sessionId",
    "nativeSessionId",
];
const DENIED_EXECUTION_FIELDS: &[&str] = &[
    "command",
    "args",
    "stdin",
    "executable",
    "binaryPath",
    "commandPath",
    "cwd",
    "workingDirectory",
    "root",
    "historyRoot",
    "env",
    "environment",
    "shell",
    "timeoutMs",
    "maxStdoutBytes",
    "maxStderrBytes",
];

/// One command a paired device asks this device to run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCommand {
    pub command_id: String,
    pub command_kind: String,
    pub created_at: String,
    pub params: Value,
}

impl DeviceCommand {
    pub fn agent_message(
        command_id: String,
        agent: &str,
        session_id: Option<&str>,
        body: &str,
        created_at: String,
    ) -> Self {
        let mut params = Map::new();
        params.insert("agent".to_owned(), json!(agent));
        params.insert("text".to_owned(), json!(body));
        if let Some(session_id) = session_id {
            params.insert("sessionId".to_owned(), json!(session_id));
        }
        Self {
            command_id,
            command_kind: AGENT_MESSAGE_SEND.to_owned(),
            created_at,
            params: Value::Object(params),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "schema": DEVICE_COMMAND_SCHEMA,
            "commandId": self.command_id,
            "commandKind": self.command_kind,
            "createdAt": self.created_at,
            "params": self.params,
        })
    }

    pub fn from_value(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("device_command_invalid"))?;
        for key in object.keys() {
            ensure!(
                ["schema", "commandId", "commandKind", "createdAt", "params"]
                    .contains(&key.as_str()),
                "device_command_invalid"
            );
        }
        ensure!(
            value.get("schema").and_then(Value::as_str) == Some(DEVICE_COMMAND_SCHEMA),
            "device_command_schema_unsupported"
        );
        let created_at = required_text(value, "createdAt")?;
        OffsetDateTime::parse(&created_at, &Rfc3339).context("device_command_invalid")?;
        let params = value
            .get("params")
            .cloned()
            .ok_or_else(|| anyhow!("device_command_invalid"))?;
        ensure!(params.is_object(), "device_command_invalid");
        ensure!(
            serde_json::to_vec(&params)?.len() <= MAX_PARAMS_BYTES,
            "device_command_too_large"
        );
        Ok(Self {
            command_id: required_text(value, "commandId")?,
            command_kind: required_text(value, "commandKind")?,
            created_at,
            params,
        })
    }

    fn fingerprint(&self) -> Result<String> {
        let canonical = json!({
            "commandKind": self.command_kind,
            "params": self.params,
        });
        Ok(hex_sha256(&serde_json::to_vec(&canonical)?))
    }
}

/// The outcome one paired device returns for one command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCommandResult {
    pub command_id: String,
    pub completed: bool,
    pub output: Value,
    pub completed_at: String,
}

impl DeviceCommandResult {
    fn failed(command_id: &str, code: &str, retryable: bool, completed_at: &str) -> Self {
        Self {
            command_id: command_id.to_owned(),
            completed: false,
            output: json!({ "errorCode": code, "retryable": retryable }),
            completed_at: completed_at.to_owned(),
        }
    }

    pub fn status(&self) -> &'static str {
        if self.completed {
            "completed"
        } else {
            "failed"
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "schema": DEVICE_RESULT_SCHEMA,
            "commandId": self.command_id,
            "status": self.status(),
            "output": self.output,
            "completedAt": self.completed_at,
        })
    }

    pub fn from_value(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("device_result_invalid"))?;
        for key in object.keys() {
            ensure!(
                ["schema", "commandId", "status", "output", "completedAt"].contains(&key.as_str()),
                "device_result_invalid"
            );
        }
        ensure!(
            value.get("schema").and_then(Value::as_str) == Some(DEVICE_RESULT_SCHEMA),
            "device_result_schema_unsupported"
        );
        let completed = match value.get("status").and_then(Value::as_str) {
            Some("completed") => true,
            Some("failed") => false,
            _ => bail!("device_result_invalid"),
        };
        let output = value
            .get("output")
            .cloned()
            .filter(Value::is_object)
            .ok_or_else(|| anyhow!("device_result_invalid"))?;
        let completed_at = required_text(value, "completedAt")?;
        OffsetDateTime::parse(&completed_at, &Rfc3339).context("device_result_invalid")?;
        Ok(Self {
            command_id: required_text(value, "commandId")?,
            completed,
            output,
            completed_at,
        })
    }
}

/// One record payload: a command or the result answering one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceRecord {
    Command(DeviceCommand),
    Result(DeviceCommandResult),
}

impl DeviceRecord {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes).context("device_record_invalid")?;
        match value.get("schema").and_then(Value::as_str) {
            Some(DEVICE_COMMAND_SCHEMA) => Ok(Self::Command(DeviceCommand::from_value(&value)?)),
            Some(DEVICE_RESULT_SCHEMA) => {
                Ok(Self::Result(DeviceCommandResult::from_value(&value)?))
            }
            _ => bail!("device_record_schema_unsupported"),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let value = match self {
            Self::Command(command) => command.to_value(),
            Self::Result(result) => result.to_value(),
        };
        Ok(serde_json::to_vec(&value)?)
    }
}

/// A classified agent-dispatch failure; its code travels in the result.
#[derive(Debug)]
pub(crate) struct AgentDispatchFailure {
    pub(crate) code: &'static str,
    pub(crate) retryable: bool,
}

impl AgentDispatchFailure {
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

impl fmt::Display for AgentDispatchFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl StdError for AgentDispatchFailure {}

/// An adapter's own reply is a success only when it says so.
pub(crate) fn successful_agent_dispatch(result: Result<Value>) -> Result<Value> {
    let result = result?;
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        let code = result
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .or_else(|| result.get("code").and_then(Value::as_str))
            .unwrap_or_default();
        return Err(anyhow::Error::new(AgentDispatchFailure::from_adapter_code(
            code,
        )));
    }
    Ok(result)
}

/// The local effect of one admitted command.
pub trait DeviceCommandExecutor {
    fn execute(&mut self, command: &DeviceCommand, agent_params: Value) -> Result<Value>;
}

/// Why a command is not executed: the stable code the result carries.
fn refusal(command: &DeviceCommand, peer_paired: bool) -> Option<&'static str> {
    if !peer_paired {
        return Some("device_peer_not_paired");
    }
    if command.command_id.is_empty() || command.command_id.len() > MAX_ID_BYTES {
        return Some("device_command_invalid");
    }
    if DENIED_PREFIXES
        .iter()
        .any(|prefix| command.command_kind.starts_with(prefix))
        || !ALLOWED_COMMANDS.contains(&command.command_kind.as_str())
    {
        return Some("command_not_allowlisted");
    }
    None
}

/// The bounded parameters `agent.message.send` hands the agent port.
pub(crate) fn agent_message_params(command: &DeviceCommand) -> Result<Value> {
    let object = command
        .params
        .as_object()
        .ok_or_else(|| anyhow!("device_command_invalid"))?;
    let mut out = Map::new();
    for (key, value) in object {
        ensure!(
            !DENIED_EXECUTION_FIELDS.contains(&key.as_str()),
            "unscoped_execution_field"
        );
        ensure!(
            AGENT_MESSAGE_SEND_FIELDS.contains(&key.as_str()),
            "command_field_not_enabled"
        );
        out.insert(key.clone(), value.clone());
    }
    ensure!(
        text_field(&out, &["agent"]).is_some(),
        "agent_binding_required"
    );
    ensure!(
        text_field(&out, &["text", "message", "prompt"]).is_some(),
        "agent_message_text_required"
    );
    out.insert("idempotencyKey".to_owned(), json!(command.command_id));
    Ok(Value::Object(out))
}

fn text_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// The inbound replay ledger: one row per peer and command id.
pub struct DeviceCommandLedger {
    connection: Connection,
}

enum PriorExecution {
    Missing,
    Reserved,
    Completed(DeviceCommandResult),
    Conflict,
}

impl DeviceCommandLedger {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).context("device_ledger_unavailable")?;
        connection.execute_batch(
            r#"
            PRAGMA secure_delete = ON;
            CREATE TABLE IF NOT EXISTS inbound_command (
                peer_id TEXT NOT NULL,
                command_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                state TEXT NOT NULL CHECK (state IN ('reserved', 'completed')),
                result_json TEXT,
                recorded_at TEXT NOT NULL,
                PRIMARY KEY (peer_id, command_id)
            );
            CREATE TABLE IF NOT EXISTS outbound_command (
                client_intent_id TEXT PRIMARY KEY,
                command_id TEXT NOT NULL UNIQUE,
                peer_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS command_result (
                command_id TEXT PRIMARY KEY,
                peer_id TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('completed', 'failed')),
                output_json TEXT NOT NULL,
                completed_at TEXT NOT NULL
            );
            "#,
        )?;
        Ok(Self { connection })
    }

    fn prior(&self, peer_id: &str, command: &DeviceCommand) -> Result<PriorExecution> {
        let row = self
            .connection
            .query_row(
                "SELECT fingerprint, state, result_json FROM inbound_command WHERE peer_id = ?1 AND command_id = ?2",
                params![peer_id, command.command_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((fingerprint, state, result)) = row else {
            return Ok(PriorExecution::Missing);
        };
        if fingerprint != command.fingerprint()? {
            return Ok(PriorExecution::Conflict);
        }
        match (state.as_str(), result) {
            ("completed", Some(result)) => Ok(PriorExecution::Completed(
                DeviceCommandResult::from_value(&serde_json::from_str(&result)?)?,
            )),
            ("reserved", None) => Ok(PriorExecution::Reserved),
            _ => bail!("device_ledger_invalid"),
        }
    }

    fn reserve(&self, peer_id: &str, command: &DeviceCommand, now: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO inbound_command (peer_id, command_id, fingerprint, state, result_json, recorded_at) VALUES (?1, ?2, ?3, 'reserved', NULL, ?4)",
            params![peer_id, command.command_id, command.fingerprint()?, now],
        )?;
        Ok(())
    }

    fn complete(&self, peer_id: &str, result: &DeviceCommandResult) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE inbound_command SET state = 'completed', result_json = ?1 WHERE peer_id = ?2 AND command_id = ?3 AND state = 'reserved'",
            params![serde_json::to_string(&result.to_value())?, peer_id, result.command_id],
        )?;
        ensure!(changed == 1, "device_ledger_invalid");
        Ok(())
    }

    /// The command id an earlier send of one client intent already chose.
    pub fn outbound_command(&self, client_intent_id: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT command_id, peer_id FROM outbound_command WHERE client_intent_id = ?1",
                params![client_intent_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn record_outbound(
        &self,
        client_intent_id: &str,
        command_id: &str,
        peer_id: &str,
        created_at: &str,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO outbound_command (client_intent_id, command_id, peer_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![client_intent_id, command_id, peer_id, created_at],
        )?;
        Ok(())
    }

    /// Records one result a peer returned for a command this device sent.
    /// A redelivered result is the same row, so recording it again is a no-op.
    pub fn record_result(&self, peer_id: &str, result: &DeviceCommandResult) -> Result<bool> {
        let sent_to = self
            .connection
            .query_row(
                "SELECT peer_id FROM outbound_command WHERE command_id = ?1",
                params![result.command_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        ensure!(
            sent_to.as_deref() == Some(peer_id),
            "device_result_unsolicited"
        );
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO command_result (command_id, peer_id, status, output_json, completed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                result.command_id,
                peer_id,
                result.status(),
                serde_json::to_string(&result.output)?,
                result.completed_at
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn results(&self, peer_id: Option<&str>, since: Option<&str>) -> Result<Vec<Value>> {
        let mut statement = self.connection.prepare(
            "SELECT command_id, peer_id, status, output_json, completed_at FROM command_result ORDER BY completed_at, command_id",
        )?;
        let since = since
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .map_err(|_| anyhow!("devices_since_invalid"))?;
        let mut out = Vec::new();
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (command_id, peer, status, output, completed_at) = row?;
            if peer_id.is_some_and(|wanted| wanted != peer) {
                continue;
            }
            if let Some(since) = since {
                let at = OffsetDateTime::parse(&completed_at, &Rfc3339)
                    .map_err(|_| anyhow!("device_ledger_invalid"))?;
                if at < since {
                    continue;
                }
            }
            out.push(json!({
                "commandId": command_id,
                "peerId": peer,
                "status": status,
                "output": serde_json::from_str::<Value>(&output)?,
                "completedAt": completed_at,
            }));
        }
        Ok(out)
    }

    /// Forgets the outbound intents of one unpaired peer; received results stay.
    pub fn forget_peer(&self, peer_id: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM outbound_command WHERE peer_id = ?1 AND command_id NOT IN (SELECT command_id FROM command_result)",
            params![peer_id],
        )?;
        Ok(())
    }
}

/// Runs one delivered command at most once per peer and command id.
///
/// The result is what the sender receives. A redelivery of a completed
/// command returns the recorded result without executing again; a command
/// whose earlier execution was interrupted reports that its outcome is
/// unknown rather than running twice.
pub fn execute_device_command(
    command: &DeviceCommand,
    peer_id: &str,
    peer_paired: bool,
    ledger: &DeviceCommandLedger,
    executor: &mut impl DeviceCommandExecutor,
    now: &str,
) -> Result<DeviceCommandResult> {
    if let Some(code) = refusal(command, peer_paired) {
        return Ok(DeviceCommandResult::failed(
            &command.command_id,
            code,
            false,
            now,
        ));
    }
    match ledger.prior(peer_id, command)? {
        PriorExecution::Completed(result) => return Ok(result),
        PriorExecution::Reserved => {
            return Ok(DeviceCommandResult::failed(
                &command.command_id,
                "execution_outcome_unknown",
                false,
                now,
            ));
        }
        PriorExecution::Conflict => {
            return Ok(DeviceCommandResult::failed(
                &command.command_id,
                "command_replay_conflict",
                false,
                now,
            ));
        }
        PriorExecution::Missing => {}
    }
    let agent_params = match agent_message_params(command) {
        Ok(params) => params,
        Err(error) => {
            return Ok(DeviceCommandResult::failed(
                &command.command_id,
                stable_code(&error, "device_command_invalid"),
                false,
                now,
            ));
        }
    };
    ledger.reserve(peer_id, command, now)?;
    let result = match executor.execute(command, agent_params) {
        Ok(output) => DeviceCommandResult {
            command_id: command.command_id.clone(),
            completed: true,
            output: json!({ "commandKind": command.command_kind, "output": output }),
            completed_at: now.to_owned(),
        },
        Err(error) => {
            let classified = error.downcast_ref::<AgentDispatchFailure>();
            DeviceCommandResult::failed(
                &command.command_id,
                classified
                    .map(|failure| failure.code)
                    .unwrap_or("local_execution_failed"),
                classified.map(|failure| failure.retryable).unwrap_or(true),
                now,
            )
        }
    };
    ledger.complete(peer_id, &result)?;
    Ok(result)
}

fn stable_code(error: &anyhow::Error, fallback: &'static str) -> &'static str {
    const CODES: &[&str] = &[
        "unscoped_execution_field",
        "command_field_not_enabled",
        "agent_binding_required",
        "agent_message_text_required",
    ];
    let message = error.to_string();
    CODES
        .iter()
        .copied()
        .find(|code| message == *code)
        .unwrap_or(fallback)
}

fn required_text(value: &Value, key: &str) -> Result<String> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= MAX_ID_BYTES)
        .ok_or_else(|| anyhow!("device_command_invalid"))?;
    Ok(text.to_owned())
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
