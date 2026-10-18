//! Secure Client Mesh command receive pipeline.
//!
//! The facade intentionally exposes the stable command API while keeping
//! schema decoding, replay persistence, policy evaluation, runtime dispatch,
//! and JSON presentation independently testable.

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::{Engine as _, engine::general_purpose};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error as StdError,
    fmt,
    path::Path,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::core::secure_mesh::{
    ALLOWED_COMMANDS, DENIED_PREFIXES, SECURE_MESH_COMMAND_PROTOCOL_VERSION,
};
use crate::core::secure_mesh_response::{SecureMeshErrorPayload, SecureMeshResultPayload};
use crate::core::secure_mesh_trust::DeviceTrustState;

mod codec;
mod policy;
mod replay;
mod runtime;
mod schema;

pub use codec::{evaluate_secure_command_json, execute_secure_command_json};
pub use policy::{SecureCommandEvaluation, evaluate_secure_command};
pub use replay::{
    SecureCommandPriorExecution, SecureCommandReplayLedger, SecureCommandReplayRecordStatus,
    SecureCommandReplayStore, SecureCommandSqliteReplayLedger,
};
pub use runtime::{
    SecureCommandExecutionOutcome, SecureCommandLocalExecutor, execute_evaluated_secure_command,
};
pub use schema::{
    SecureCommandEvaluationContext, SecureCommandPayload, SecureCommandRiskClass,
    SecureCommandSenderIdentity, SecureCommandTargetBinding,
};

pub(crate) use runtime::{
    SecureAgentDispatchFailure, agent_message_send_params, agent_sessions_describe_params,
    agent_sessions_list_params, dispatch_ready_agent_message,
};

pub const SECURE_MESH_COMMAND_SECURITY_STATUS: &str = "local_schema_risk_replay_idempotency_sqlite_ledger_execution_adapter_runtime_binding_available";

const MAX_COMMAND_ID_BYTES: usize = 255;
const MAX_COMMAND_KIND_BYTES: usize = 255;
const MAX_ENDPOINT_ID_BYTES: usize = 255;
const MAX_FINGERPRINT_BYTES: usize = 255;
const MAX_BINDING_ID_BYTES: usize = 255;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
const MAX_COMMAND_BODY_BYTES: usize = 1024 * 1024;
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;
pub const SECURE_MESH_COMMAND_LEDGER_MAX_ENTRIES: usize = 4096;
const LOCAL_EXECUTION_FAILED_REMOTE_DETAIL: &str =
    "local command execution failed; details are not exposed over secure mesh";
const SECURE_AGENT_MESSAGE_TIMEOUT_MS: u64 = 90_000;
const AGENT_MESSAGE_SEND_PAYLOAD_FIELDS: &[&str] = &[
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
const AGENT_SESSIONS_LIST_PAYLOAD_FIELDS: &[&str] = &["limit", "offset"];
const AGENT_SESSIONS_DESCRIBE_PAYLOAD_FIELDS: &[&str] = &["sessionId", "nativeSessionId"];
const AGENT_RESOURCE_SELECTOR_FIELDS: &[&str] = &["agent", "agentId", "target"];
const COMMAND_BODY_DENIED_EXECUTION_FIELDS: &[&str] = &[
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

#[cfg(test)]
mod tests;
