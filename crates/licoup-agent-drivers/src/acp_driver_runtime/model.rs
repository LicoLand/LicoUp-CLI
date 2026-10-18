use super::errors::ProtocolFailure;
use licoup_foundation::core::acp;
use serde_json::Value;
use std::time::Duration;

pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One ACP driver's immutable launch metadata.
///
/// It is metadata only. Which frame dialect this driver's transport reads is
/// not a field: the dialect arrives through [`super::parser_port`], resolved by
/// [`Self::agent_id`], so the spec cannot name a vendor and an Agent that
/// changes its dialect is not a change to the transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpDriverSpec {
    pub agent_id: &'static str,
    pub error_prefix: &'static str,
    pub runtime_protocol: &'static str,
    pub launch_args: &'static [&'static str],
    pub launch_model_arg: Option<&'static str>,
    pub launch_reasoning_env: Option<&'static str>,
    pub launch_reasoning_values: &'static [&'static str],
    pub launch_allow_all_arg: Option<&'static str>,
    /// Whether this Agent's runtime flags belong to its ACP subcommand rather
    /// than to the binary: when set, the launch arguments are emitted first and
    /// the model and allow-all arguments follow them, so `agent acp --model x`
    /// is spawned rather than `agent --model x acp`.
    pub launch_settings_after_args: bool,
}

impl AcpDriverSpec {
    pub const fn new(runtime_protocol: &'static str, launch_args: &'static [&'static str]) -> Self {
        Self {
            agent_id: "acp",
            error_prefix: "acp",
            runtime_protocol,
            launch_args,
            launch_model_arg: None,
            launch_reasoning_env: None,
            launch_reasoning_values: &[],
            launch_allow_all_arg: None,
            launch_settings_after_args: false,
        }
    }

    pub const fn with_identity(
        mut self,
        agent_id: &'static str,
        error_prefix: &'static str,
    ) -> Self {
        self.agent_id = agent_id;
        self.error_prefix = error_prefix;
        self
    }

    pub const fn with_launch_settings(
        mut self,
        model_arg: &'static str,
        reasoning_env: &'static str,
        reasoning_values: &'static [&'static str],
    ) -> Self {
        self.launch_model_arg = Some(model_arg);
        self.launch_reasoning_env = Some(reasoning_env);
        self.launch_reasoning_values = reasoning_values;
        self
    }

    pub const fn with_allow_all_argument(mut self, allow_all_arg: &'static str) -> Self {
        self.launch_allow_all_arg = Some(allow_all_arg);
        self
    }

    /// Declare only the launch model flag, for an Agent whose launch carries no
    /// reasoning-effort environment.
    pub const fn with_model_argument(mut self, model_arg: &'static str) -> Self {
        self.launch_model_arg = Some(model_arg);
        self
    }

    /// Emit the model and allow-all arguments after this driver's launch
    /// arguments, for an Agent whose runtime flags belong to the subcommand
    /// that opens the ACP surface.
    pub const fn with_settings_after_launch_args(mut self) -> Self {
        self.launch_settings_after_args = true;
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityProbe {
    pub protocol_version: Option<u64>,
    pub load_session: bool,
    pub resume_session: bool,
    pub close_session: bool,
    pub list_sessions: bool,
    pub delete_session: bool,
    pub additional_directories: bool,
    pub image_prompts: bool,
    pub audio_prompts: bool,
    pub embedded_context: bool,
}

impl CapabilityProbe {
    pub fn from_initialize(response: &acp::AcpInitializeResponse) -> Self {
        let capabilities = &response.capabilities;
        Self {
            protocol_version: Some(u64::from(response.protocol_version)),
            load_session: capabilities.load_session,
            resume_session: capabilities.resume_session,
            close_session: capabilities.close_session,
            list_sessions: capabilities.list_sessions,
            delete_session: capabilities.delete_session,
            additional_directories: capabilities.additional_directories,
            image_prompts: capabilities.image_prompts,
            audio_prompts: capabilities.audio_prompts,
            embedded_context: capabilities.embedded_context,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    pub runtime_agent: Option<String>,
    pub allow_all: Option<bool>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

#[derive(Debug)]
pub struct RunResult {
    pub ok: bool,
    pub output: String,
    pub transitions: Vec<licoup_agent_adapter_sdk::Transition>,
    pub error: Option<ProtocolFailure>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub capabilities: CapabilityProbe,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
    pub runtime_protocol: &'static str,
    pub driver_id: &'static str,
}

impl RunResult {
    pub fn failed(
        driver: AcpDriverSpec,
        failure: ProtocolFailure,
        started_at: String,
        status_code: Option<i32>,
        stdout_truncated: bool,
        stderr_truncated: bool,
        capabilities: CapabilityProbe,
        _events: Vec<Value>,
    ) -> Self {
        let failure = failure.namespaced(driver);
        let transitions = (super::parser_port::parser_for(driver.agent_id).failed_transitions)(
            &failure.code,
            &failure.stage,
            &failure.message,
        );
        Self {
            ok: false,
            output: String::new(),
            session_id: failure.session_id.clone().unwrap_or_default(),
            thread_id: failure.thread_id.clone().unwrap_or_default(),
            turn_id: failure.turn_id.clone().unwrap_or_default(),
            turn_status: failure.turn_status.clone().unwrap_or_default(),
            effective: EffectiveSettings::default(),
            error: Some(failure),
            status_code,
            stdout_truncated,
            stderr_truncated,
            started_at,
            runtime_protocol: driver.runtime_protocol,
            driver_id: driver.agent_id,
            capabilities,
            transitions,
        }
    }
}
