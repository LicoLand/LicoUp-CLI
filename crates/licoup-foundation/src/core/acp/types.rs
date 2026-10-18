use super::AcpError;
use super::validation::normalized_text;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const MAX_ID_BYTES: usize = 128;
const MAX_IMPLEMENTATION_NAME_BYTES: usize = 128;
const MAX_IMPLEMENTATION_TITLE_BYTES: usize = 256;
const MAX_IMPLEMENTATION_VERSION_BYTES: usize = 128;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum AcpRequestId {
    Number(i64),
    Text(String),
}

impl AcpRequestId {
    pub(super) fn validate(&self) -> Result<(), AcpError> {
        if let Self::Text(value) = self {
            normalized_text(value, MAX_ID_BYTES, AcpError::RequestIdInvalid)?;
        }
        Ok(())
    }

    pub(super) fn from_value(value: &Value) -> Result<Self, AcpError> {
        match value {
            Value::Number(value) => value
                .as_i64()
                .map(Self::Number)
                .ok_or(AcpError::RequestIdInvalid),
            Value::String(value) => {
                let id = Self::Text(value.clone());
                id.validate()?;
                Ok(id)
            }
            _ => Err(AcpError::RequestIdInvalid),
        }
    }

    pub(super) fn to_value(&self) -> Value {
        match self {
            Self::Number(value) => Value::from(*value),
            Self::Text(value) => Value::String(value.clone()),
        }
    }
}

impl From<i64> for AcpRequestId {
    fn from(value: i64) -> Self {
        Self::Number(value)
    }
}

impl From<String> for AcpRequestId {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for AcpRequestId {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcpImplementation {
    name: String,
    title: Option<String>,
    version: String,
}

impl AcpImplementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            version: version.into(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub(super) fn to_value(&self) -> Result<Value, AcpError> {
        normalized_text(
            &self.name,
            MAX_IMPLEMENTATION_NAME_BYTES,
            AcpError::ImplementationInvalid,
        )?;
        normalized_text(
            &self.version,
            MAX_IMPLEMENTATION_VERSION_BYTES,
            AcpError::ImplementationInvalid,
        )?;
        if let Some(title) = &self.title {
            normalized_text(
                title,
                MAX_IMPLEMENTATION_TITLE_BYTES,
                AcpError::ImplementationInvalid,
            )?;
        }
        let mut value = Map::new();
        value.insert("name".into(), Value::String(self.name.clone()));
        if let Some(title) = &self.title {
            value.insert("title".into(), Value::String(title.clone()));
        }
        value.insert("version".into(), Value::String(self.version.clone()));
        Ok(Value::Object(value))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AcpClientCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
    pub terminal: bool,
}

impl AcpClientCapabilities {
    pub(super) fn to_value(self) -> Value {
        json!({
            "fs": {
                "readTextFile": self.read_text_file,
                "writeTextFile": self.write_text_file
            },
            "terminal": self.terminal
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpSessionMethod<'a> {
    New,
    Load(&'a str),
    Resume(&'a str),
}

impl AcpSessionMethod<'_> {
    pub const fn method_name(&self) -> &'static str {
        match self {
            Self::New => super::SESSION_NEW_METHOD,
            Self::Load(_) => super::SESSION_LOAD_METHOD,
            Self::Resume(_) => super::SESSION_RESUME_METHOD,
        }
    }

    pub(super) fn requested_session_id(&self) -> Option<&str> {
        match self {
            Self::New => None,
            Self::Load(session_id) | Self::Resume(session_id) => Some(session_id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AcpSessionOptions<'a> {
    pub(super) cwd: &'a Path,
    pub(super) additional_directories: &'a [PathBuf],
    pub(super) mcp_servers: &'a [Value],
    pub(super) meta: Option<Map<String, Value>>,
}

impl<'a> AcpSessionOptions<'a> {
    pub fn new(cwd: &'a Path) -> Self {
        Self {
            cwd,
            additional_directories: &[],
            mcp_servers: &[],
            meta: None,
        }
    }

    pub fn additional_directories(mut self, directories: &'a [PathBuf]) -> Self {
        self.additional_directories = directories;
        self
    }

    pub fn mcp_servers(mut self, servers: &'a [Value]) -> Self {
        self.mcp_servers = servers;
        self
    }

    pub fn meta(mut self, meta: Map<String, Value>) -> Self {
        self.meta = Some(meta);
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AcpAgentCapabilities {
    pub load_session: bool,
    pub resume_session: bool,
    pub close_session: bool,
    pub list_sessions: bool,
    pub delete_session: bool,
    pub additional_directories: bool,
    pub image_prompts: bool,
    pub audio_prompts: bool,
    pub embedded_context: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcpInitializeResponse {
    pub protocol_version: u16,
    pub capabilities: AcpAgentCapabilities,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcpSessionResponse {
    pub session_id: Option<String>,
    pub modes: Option<Value>,
    pub config_options: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcpSessionInfo {
    pub session_id: String,
    pub cwd: String,
    pub additional_directories: Vec<String>,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub meta: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcpSessionListResponse {
    pub sessions: Vec<AcpSessionInfo>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

impl AcpStopReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::MaxTurnRequests => "max_turn_requests",
            Self::Refusal => "refusal",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpPromptResponse {
    pub stop_reason: AcpStopReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpSessionUpdateKind {
    UserMessageChunk,
    AgentMessageChunk,
    AgentThoughtChunk,
    ToolCall,
    ToolCallUpdate,
    Plan,
    AvailableCommandsUpdate,
    CurrentModeUpdate,
    ConfigOptionUpdate,
    SessionInfoUpdate,
    UsageUpdate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcpSessionUpdate {
    pub session_id: String,
    pub kind: AcpSessionUpdateKind,
    pub(super) update: Value,
}

impl AcpSessionUpdate {
    pub const fn kind(&self) -> AcpSessionUpdateKind {
        self.kind
    }

    pub fn payload(&self) -> &Value {
        &self.update
    }

    pub fn into_payload(self) -> Value {
        self.update
    }

    pub fn agent_message_text(&self) -> Option<&str> {
        (self.kind == AcpSessionUpdateKind::AgentMessageChunk)
            .then(|| self.update.pointer("/content/text").and_then(Value::as_str))
            .flatten()
    }

    pub fn user_message_text(&self) -> Option<&str> {
        (self.kind == AcpSessionUpdateKind::UserMessageChunk)
            .then(|| self.update.pointer("/content/text").and_then(Value::as_str))
            .flatten()
    }

    pub fn current_mode_id(&self) -> Option<&str> {
        (self.kind == AcpSessionUpdateKind::CurrentModeUpdate)
            .then(|| self.update.get("currentModeId").and_then(Value::as_str))
            .flatten()
    }

    pub fn config_options(&self) -> Option<&[Value]> {
        (self.kind == AcpSessionUpdateKind::ConfigOptionUpdate)
            .then(|| self.update.get("configOptions").and_then(Value::as_array))
            .flatten()
            .map(Vec::as_slice)
    }
}

impl AcpSessionUpdateKind {
    pub const fn processing_evidence_kind(self) -> Option<&'static str> {
        match self {
            Self::AgentThoughtChunk => Some("reasoning"),
            Self::ToolCall | Self::ToolCallUpdate => Some("tool"),
            Self::Plan => Some("plan"),
            _ => None,
        }
    }
}

pub(super) fn validate_implementation_value(value: &Value) -> Result<(), AcpError> {
    let implementation = value.as_object().ok_or(AcpError::CapabilityInvalid)?;
    let name = implementation
        .get("name")
        .and_then(Value::as_str)
        .ok_or(AcpError::CapabilityInvalid)?;
    let version = implementation
        .get("version")
        .and_then(Value::as_str)
        .ok_or(AcpError::CapabilityInvalid)?;
    normalized_text(
        name,
        MAX_IMPLEMENTATION_NAME_BYTES,
        AcpError::CapabilityInvalid,
    )?;
    normalized_text(
        version,
        MAX_IMPLEMENTATION_VERSION_BYTES,
        AcpError::CapabilityInvalid,
    )?;
    if let Some(title) = implementation.get("title")
        && !title.is_null()
    {
        let title = title.as_str().ok_or(AcpError::CapabilityInvalid)?;
        normalized_text(
            title,
            MAX_IMPLEMENTATION_TITLE_BYTES,
            AcpError::CapabilityInvalid,
        )?;
    }
    Ok(())
}
