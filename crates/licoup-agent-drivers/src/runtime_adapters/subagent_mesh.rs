use super::runtime_driver_profile;
use licoup_agent_runtime::{
    AdapterFailure, CallerRegistrationPlan, CallerRegistrationReceipt, DurableNativeBinding,
    ExecutionAdmissionEvidence, InstructionPolicy, McpCallerIntegration, NativeResumeIdentity,
    ProviderId, ReadinessEvidence, RegistrationApproval, RuntimeDispatchReceipt, RuntimeEventPart,
    RuntimeObservation, RuntimeTransition, SubagentCapabilities, SubagentContinueRequest,
    SubagentDispatchRequest, SubagentRuntimeAdapter,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// Bind a target Agent process to the exact Membership-scoped caller context
/// that it may later present through the thin MCP connector. Values are
/// validated opaque identifiers and never persisted by this helper.
pub fn apply_subagent_caller_context(command: &mut std::process::Command, params: &Value) {
    for (key, env_key) in [
        ("agentId", "LICOUP_MCP_CALLER_PROVIDER"),
        ("conversationId", "LICOUP_MCP_CONVERSATION_ID"),
        ("membershipId", "LICOUP_MCP_MEMBERSHIP_ID"),
        ("parentDispatchId", "LICOUP_MCP_PARENT_DISPATCH_ID"),
    ] {
        if let Some(value) = params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| valid_context_identifier(value))
        {
            command.env(env_key, value);
        }
    }
}

/// Bind the portable data root so a provider-spawned MCP connector can locate
/// the desktop-owned supervisor. Caller context stays a separate, exact set.
pub fn apply_mcp_runtime_root(command: &mut std::process::Command) {
    if let Ok(root) = licoup_foundation::platform::paths::portable_data_dir() {
        command.env("LICOUP_HOME", root);
    }
}

/// One call on this host's own conversation lane, through the composition.
///
/// The Subagent mesh reaches the lane rather than any Agent's driver, so the
/// lane arrives through the port like every other engine this host composes.
fn host_call(read_only: bool, method: &str, params: &Value) -> Result<Value, String> {
    let Some(port) = super::port::composition().map(|composition| composition.conversation_host)
    else {
        return Err("no conversation host is composed".to_owned());
    };
    if read_only {
        (port.execute_read_only)(method, params)
    } else {
        (port.execute)(method, params)
    }
}

/// The caller-integration manager of the Agent this host manages one for.
fn caller_manager() -> Result<super::port::CallerManagerPort, AdapterFailure> {
    super::port::composition()
        .map(|composition| composition.caller_manager)
        .ok_or_else(|| registration_failure("caller_registration_plan_failed"))
}

/// The user-configuration caller manager this host composed, or a fail-closed
/// registration failure when it composed none.
fn caller_config() -> Result<super::port::CallerConfigPort, AdapterFailure> {
    super::port::caller_config()
        .ok_or_else(|| registration_failure("caller_registration_plan_failed"))
}

fn valid_context_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-'))
}

pub fn production_subagent_runtimes(
    port: licoup_agent_targets::port::AgentTargetPort,
) -> Vec<Arc<dyn SubagentRuntimeAdapter>> {
    vec![
        Arc::new(HostSubagentRuntime::new(
            "codex",
            SubagentCapabilities {
                create: true,
                exact_resume: true,
                observe: true,
                continue_turn: true,
                active_cancel: true,
                native_steer: true,
                instruction_policy: InstructionPolicy::NativeDeveloperInstructions,
            },
            port.clone(),
        )),
        Arc::new(HostSubagentRuntime::new(
            "cursor",
            SubagentCapabilities {
                create: true,
                exact_resume: true,
                observe: true,
                continue_turn: true,
                active_cancel: true,
                native_steer: false,
                instruction_policy: InstructionPolicy::OrdinaryWirePrefix,
            },
            port.clone(),
        )),
        Arc::new(HostSubagentRuntime::new(
            "antigravity",
            SubagentCapabilities {
                create: true,
                exact_resume: true,
                observe: true,
                continue_turn: true,
                active_cancel: true,
                native_steer: false,
                instruction_policy: InstructionPolicy::OrdinaryWirePrefix,
            },
            port.clone(),
        )),
        Arc::new(HostSubagentRuntime::new(
            "claude-code",
            SubagentCapabilities {
                create: true,
                exact_resume: true,
                observe: true,
                continue_turn: true,
                active_cancel: true,
                native_steer: false,
                instruction_policy: InstructionPolicy::OrdinaryWirePrefix,
            },
            port.clone(),
        )),
    ]
}

pub fn production_subagent_registry(
    port: licoup_agent_targets::port::AgentTargetPort,
) -> licoup_agent_adapters::AdapterRegistry {
    let mut registry = licoup_agent_adapters::AdapterRegistry::empty();
    let callers = production_caller_integrations();
    let runtimes = production_subagent_runtimes(port);
    for runtime in runtimes {
        let caller = callers
            .iter()
            .find(|caller| caller.provider_id() == runtime.provider_id())
            .cloned()
            .expect("every production runtime has one caller integration");
        registry
            .register_pair(caller, runtime)
            .expect("production provider identities are unique");
    }
    registry
}

fn production_caller_integrations() -> Vec<Arc<dyn McpCallerIntegration>> {
    let connector = super::port::caller_config().and_then(|config| (config.default_connector)());
    vec![
        Arc::new(ManagedCallerIntegration::new(
            "codex",
            CallerManagerKind::CodexPlugin,
            licoup_agent_targets::domain::targets::agent_cli_executable("codex"),
            connector.clone(),
        )),
        Arc::new(ManagedCallerIntegration::new(
            "cursor",
            CallerManagerKind::CursorUserConfig,
            licoup_agent_targets::domain::targets::agent_cli_executable("cursor"),
            connector.clone(),
        )),
        Arc::new(ManagedCallerIntegration::new(
            "antigravity",
            CallerManagerKind::AntigravityUserConfig,
            licoup_agent_targets::domain::targets::agent_cli_executable("antigravity"),
            connector.clone(),
        )),
        Arc::new(ManagedCallerIntegration::new(
            "claude-code",
            CallerManagerKind::ClaudeCodeUserConfig,
            licoup_agent_targets::domain::targets::agent_cli_executable("claude-code"),
            connector,
        )),
    ]
}

/// Which caller registration one managed integration drives.
///
/// The kind is the composition's own vocabulary: the host composes one
/// [`super::port::CallerConfigPort`] answer per kind it ships, and the port
/// carries the kind rather than the registration file.
#[derive(Clone, Copy)]
pub enum CallerManagerKind {
    CodexPlugin,
    CursorUserConfig,
    AntigravityUserConfig,
    ClaudeCodeUserConfig,
}

struct ManagedCallerIntegration {
    provider: ProviderId,
    kind: CallerManagerKind,
    provider_binary: Option<std::path::PathBuf>,
    connector: Option<std::path::PathBuf>,
}

impl ManagedCallerIntegration {
    fn new(
        provider: &'static str,
        kind: CallerManagerKind,
        provider_binary: Option<std::path::PathBuf>,
        connector: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            provider: ProviderId::parse(provider).expect("static provider identity"),
            kind,
            provider_binary,
            connector,
        }
    }

    fn prepare_digest(&self) -> Result<String, AdapterFailure> {
        match self.kind {
            CallerManagerKind::CodexPlugin => {
                let binary = self.provider_binary.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("provider_binary_unavailable", "registration/plan")
                })?;
                (caller_manager()?.plan_digest)(binary)
                    .map_err(|_| registration_failure("caller_registration_plan_failed"))
            }
            CallerManagerKind::CursorUserConfig => {
                if self.provider_binary.is_none() {
                    return Err(AdapterFailure::permanent(
                        "provider_binary_unavailable",
                        "registration/plan",
                    ));
                }
                let connector = self.connector.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("mcp_connector_unavailable", "registration/plan")
                })?;
                (caller_config()?.plan_digest)(self.kind, connector)
                    .map_err(|_| registration_failure("caller_registration_plan_failed"))
            }
            CallerManagerKind::AntigravityUserConfig => {
                if self.provider_binary.is_none() {
                    return Err(AdapterFailure::permanent(
                        "provider_binary_unavailable",
                        "registration/plan",
                    ));
                }
                let connector = self.connector.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("mcp_connector_unavailable", "registration/plan")
                })?;
                (caller_config()?.plan_digest)(self.kind, connector)
                    .map_err(|_| registration_failure("caller_registration_plan_failed"))
            }
            CallerManagerKind::ClaudeCodeUserConfig => {
                if self.provider_binary.is_none() {
                    return Err(AdapterFailure::permanent(
                        "provider_binary_unavailable",
                        "registration/plan",
                    ));
                }
                let connector = self.connector.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("mcp_connector_unavailable", "registration/plan")
                })?;
                (caller_config()?.plan_digest)(self.kind, connector)
                    .map_err(|_| registration_failure("caller_registration_plan_failed"))
            }
        }
    }

    fn ready(&self) -> bool {
        match self.kind {
            CallerManagerKind::CodexPlugin => {
                self.provider_binary.as_deref().is_some_and(|binary| {
                    caller_manager()
                        .map(|port| (port.ready)(binary))
                        .unwrap_or(false)
                })
            }
            CallerManagerKind::CursorUserConfig
            | CallerManagerKind::AntigravityUserConfig
            | CallerManagerKind::ClaudeCodeUserConfig => {
                self.provider_binary.is_some()
                    && self.connector.as_deref().is_some_and(|connector| {
                        super::port::caller_config()
                            .is_some_and(|config| (config.ready)(self.kind, connector))
                    })
            }
        }
    }

    fn apply(&self, digest: &str, remove: bool) -> Result<(), AdapterFailure> {
        match self.kind {
            CallerManagerKind::CodexPlugin => {
                let binary = self.provider_binary.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("provider_binary_unavailable", "registration/apply")
                })?;
                (caller_manager()?.apply)(binary, digest, remove)
                    .map_err(|_| registration_failure("caller_registration_apply_failed"))
            }
            CallerManagerKind::CursorUserConfig => {
                if self.provider_binary.is_none() {
                    return Err(AdapterFailure::permanent(
                        "provider_binary_unavailable",
                        "registration/apply",
                    ));
                }
                let connector = self.connector.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("mcp_connector_unavailable", "registration/apply")
                })?;
                (caller_config()?.apply)(self.kind, connector, digest, remove)
                    .map_err(|_| registration_failure("caller_registration_apply_failed"))
            }
            CallerManagerKind::AntigravityUserConfig => {
                if self.provider_binary.is_none() {
                    return Err(AdapterFailure::permanent(
                        "provider_binary_unavailable",
                        "registration/apply",
                    ));
                }
                let connector = self.connector.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("mcp_connector_unavailable", "registration/apply")
                })?;
                (caller_config()?.apply)(self.kind, connector, digest, remove)
                    .map_err(|_| registration_failure("caller_registration_apply_failed"))
            }
            CallerManagerKind::ClaudeCodeUserConfig => {
                if self.provider_binary.is_none() {
                    return Err(AdapterFailure::permanent(
                        "provider_binary_unavailable",
                        "registration/apply",
                    ));
                }
                let connector = self.connector.as_deref().ok_or_else(|| {
                    AdapterFailure::permanent("mcp_connector_unavailable", "registration/apply")
                })?;
                (caller_config()?.apply)(self.kind, connector, digest, remove)
                    .map_err(|_| registration_failure("caller_registration_apply_failed"))
            }
        }
    }
}

impl McpCallerIntegration for ManagedCallerIntegration {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }

    fn plan_registration(&self) -> Result<CallerRegistrationPlan, AdapterFailure> {
        Ok(CallerRegistrationPlan {
            provider_id: self.provider.clone(),
            content_digest: self.prepare_digest()?,
            requires_approval: true,
        })
    }

    fn readiness(&self) -> ReadinessEvidence {
        let ready = self.ready();
        ReadinessEvidence {
            provider_id: self.provider.as_str().to_owned(),
            installed: ready,
            identity_verified: ready,
            transport_ready: ready,
            permission_ready: ready,
            capability_revision: format!("subagent-mcp:{}:0.11.0", self.provider),
            blocker_code: (!ready).then(|| "caller_registration_unready".to_owned()),
        }
    }

    fn apply_registration(
        &self,
        plan: &CallerRegistrationPlan,
        approval: &mut RegistrationApproval,
    ) -> Result<CallerRegistrationReceipt, AdapterFailure> {
        if plan.provider_id != self.provider || plan.content_digest != self.prepare_digest()? {
            return Err(registration_failure("caller_registration_plan_stale"));
        }
        approval.claim(&plan.content_digest)?;
        self.apply(&plan.content_digest, false)?;
        Ok(CallerRegistrationReceipt {
            provider_id: self.provider.clone(),
            ready_for_fresh_sessions: true,
        })
    }

    fn remove_registration(
        &self,
        plan: &CallerRegistrationPlan,
        approval: &mut RegistrationApproval,
    ) -> Result<(), AdapterFailure> {
        if plan.provider_id != self.provider || plan.content_digest != self.prepare_digest()? {
            return Err(registration_failure("caller_registration_plan_stale"));
        }
        approval.claim(&plan.content_digest)?;
        self.apply(&plan.content_digest, true)
    }
}

fn registration_failure(code: &'static str) -> AdapterFailure {
    AdapterFailure::permanent(code, "registration/apply")
}

struct HostSubagentRuntime {
    provider: ProviderId,
    capabilities: SubagentCapabilities,
    /// The inventory facts this adapter's admission reads, supplied by the
    /// composition that builds the registry.
    port: licoup_agent_targets::port::AgentTargetPort,
}

impl HostSubagentRuntime {
    fn new(
        provider: &'static str,
        capabilities: SubagentCapabilities,
        port: licoup_agent_targets::port::AgentTargetPort,
    ) -> Self {
        Self {
            provider: ProviderId::parse(provider).expect("static provider identity"),
            capabilities,
            port,
        }
    }

    fn dispatch_params(&self, request: &SubagentDispatchRequest) -> Value {
        let mut text = request.prompt.clone();
        let mut params = json!({
            "agent": self.provider.as_str(),
            "agentId": self.provider.as_str(),
            "text": text,
            "streamEvents": true,
            "timeoutMs": request.timeout_ms.unwrap_or(0),
            "timeoutUnbounded": request.timeout_unbounded,
            "conversationId": request.conversation_id,
            "membershipId": request.target_membership_id,
            "callerMembershipId": request.caller_membership_id,
            "causationId": "subagent-mcp",
            "dispatchId": request.dispatch_id,
            "parentDispatchId": request.dispatch_id,
        });
        if let Some(guidance) = request.generated_guidance.as_deref() {
            match self.capabilities.instruction_policy {
                InstructionPolicy::NativeDeveloperInstructions => {
                    params["developerInstructions"] = json!(guidance);
                }
                InstructionPolicy::OrdinaryWirePrefix => {
                    text = format!("{guidance}\n\n{}", request.prompt);
                    params["text"] = json!(text);
                }
                InstructionPolicy::NativePrivateInstructions => {
                    params["privateInstructions"] = json!(guidance);
                }
            }
        }
        for (key, value) in [
            ("model", request.model.as_deref()),
            ("reasoningEffort", request.reasoning_effort.as_deref()),
            ("workingDirectory", request.working_directory.as_deref()),
            ("taskType", request.task_type.as_deref()),
        ] {
            if let Some(value) = value {
                params[key] = json!(value);
            }
        }
        if let Some(value) = request.max_stdout_bytes {
            params["maxStdoutBytes"] = json!(value);
        }
        if let Some(value) = request.max_stderr_bytes {
            params["maxStderrBytes"] = json!(value);
        }
        params
    }

    fn execute_dispatch(
        &self,
        params: Value,
        dispatch_id: &str,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        let response = host_call(false, "agent.conversation.dispatch", &params)
            .map_err(project_host_dispatch_failure)?;
        if response.get("accepted").and_then(Value::as_bool) != Some(true)
            || response.get("turnHandle").and_then(Value::as_str) != Some(dispatch_id)
        {
            return Err(AdapterFailure::uncertain(
                "subagent_dispatch_receipt_invalid",
                "persistent-turn/dispatch",
            ));
        }
        Ok(RuntimeDispatchReceipt {
            dispatch_id: dispatch_id.to_owned(),
            transition: RuntimeTransition::Accepted,
            identity: None,
        })
    }

    fn exact_identity(&self, durable: &DurableNativeBinding) -> Result<String, AdapterFailure> {
        if durable.provider() != &self.provider {
            return Err(AdapterFailure::permanent(
                "native_resume_provider_mismatch",
                "identity/resolve",
            ));
        }
        let session_id = durable.native_session_id();
        // Whether a durable native identity is valid is that Agent's own
        // protocol fact, and it is asked through the shared parser contract in
        // protocol-agnostic form. This host names no Agent and reads no
        // validator of its own.
        let valid = match super::port::registration_for(self.provider.as_str()) {
            Some(registration) => (registration.parser.valid_identity)(
                &licoup_agent_adapter_sdk::port::DurableIdentityRequest {
                    session_id,
                    location: durable.native_location().map(std::path::Path::new),
                },
            ),
            None => false,
        };
        if !valid {
            return Err(AdapterFailure::permanent(
                "native_resume_identity_drift",
                "identity/resolve",
            ));
        }
        Ok(session_id.to_owned())
    }
}

fn project_host_dispatch_failure(error: String) -> AdapterFailure {
    match error.split(':').next().unwrap_or("") {
        "persistent_conversation_transport_required" => AdapterFailure::retryable(
            "persistent_conversation_transport_required",
            "persistent-turn/connect",
        ),
        "subagent_turn_not_found" => {
            AdapterFailure::permanent("subagent_turn_not_found", "session/resolve")
        }
        "subagent_turn_not_active" => {
            AdapterFailure::permanent("subagent_turn_not_active", "session/resolve")
        }
        "subagent_turn_scope_mismatch" => {
            AdapterFailure::permanent("subagent_turn_scope_mismatch", "session/authorize")
        }
        "subagent_capacity_exhausted" => {
            AdapterFailure::retryable("subagent_capacity_exhausted", "dispatch/admit")
        }
        "subagent_transport_timeout" => {
            AdapterFailure::uncertain("subagent_transport_timeout", "persistent-turn/readback")
        }
        "subagent_transport_invalid_response" => AdapterFailure::uncertain(
            "subagent_transport_invalid_response",
            "persistent-turn/readback",
        ),
        _ => AdapterFailure::uncertain("subagent_transport_failed", "persistent-turn/exchange"),
    }
}

impl SubagentRuntimeAdapter for HostSubagentRuntime {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }

    fn capabilities(&self) -> SubagentCapabilities {
        self.capabilities
    }

    fn execution_admission(&self) -> ExecutionAdmissionEvidence {
        let target = licoup_agent_targets::domain::targets::inspect_target_read_only(
            &self.port,
            self.provider.as_str(),
        )
        .ok()
        .and_then(|value| value.get("target").cloned());
        let installed = target
            .as_ref()
            .and_then(|target| target.get("status"))
            .and_then(Value::as_str)
            .is_some_and(|status| status != "not-detected");
        let message_send_capable = target
            .as_ref()
            .and_then(|target| target.get("supportedActions"))
            .and_then(Value::as_array)
            .is_some_and(|actions| {
                actions
                    .iter()
                    .any(|action| action == "runtime.message.send")
            });
        ExecutionAdmissionEvidence {
            provider_id: self.provider.as_str().to_owned(),
            installed,
            executable_message_send_route: message_send_capable
                && licoup_agent_targets::domain::targets::available_runtime_executable(
                    &self.port,
                    self.provider.as_str(),
                )
                .is_some(),
        }
    }

    fn readiness(&self) -> ReadinessEvidence {
        let profile = runtime_driver_profile(self.provider.as_str());
        let transport_ready = profile
            .as_ref()
            .is_some_and(|profile| profile.driver_status == "implemented");
        ReadinessEvidence {
            provider_id: self.provider.as_str().to_owned(),
            installed: licoup_agent_targets::domain::targets::inspect_target_read_only(
                &self.port,
                self.provider.as_str(),
            )
            .ok()
            .and_then(|value| value.get("target").cloned())
            .and_then(|target| {
                target
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|status| status != "not-detected"),
            identity_verified: profile.is_some(),
            transport_ready,
            // Direct MCP authentication and Membership authorization own
            // permission; observational conversation evidence cannot mint it.
            permission_ready: false,
            capability_revision: profile
                .map(|profile| format!("mesh:{}:{}", self.provider, profile.protocol))
                .unwrap_or_else(|| format!("mesh:{}:unavailable", self.provider)),
            blocker_code: (!transport_ready)
                .then(|| "provider_readiness_evidence_incomplete".to_owned()),
        }
    }

    fn resolve_resume_identity(
        &self,
        durable_binding: &DurableNativeBinding,
    ) -> Result<NativeResumeIdentity, AdapterFailure> {
        let exact = self.exact_identity(durable_binding)?;
        NativeResumeIdentity::from_durable(durable_binding, exact)
    }

    fn send(
        &self,
        request: &SubagentDispatchRequest,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        self.execute_dispatch(self.dispatch_params(request), &request.dispatch_id)
    }

    fn continue_turn(
        &self,
        request: &SubagentContinueRequest,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        let session_id = request
            .identity
            .binding_for_adapter(&self.provider)
            .ok_or_else(|| {
                AdapterFailure::permanent("native_resume_provider_mismatch", "identity/resolve")
            })?;
        if let (Some(requested), Some(recorded)) = (
            request.dispatch.working_directory.as_deref(),
            request
                .identity
                .working_directory_for_adapter(&self.provider),
        ) && requested != recorded
        {
            return Err(AdapterFailure::permanent(
                "conversation_working_directory_mismatch",
                "identity/resolve",
            ));
        }
        let mut params = self.dispatch_params(&request.dispatch);
        params["sessionId"] = json!(session_id);
        if let Some(location) = request.identity.native_location_for_adapter(&self.provider) {
            params["sourcePath"] = json!(location);
        }
        if params.get("workingDirectory").is_none()
            && let Some(working_directory) = request
                .identity
                .working_directory_for_adapter(&self.provider)
        {
            params["workingDirectory"] = json!(working_directory);
        }
        self.execute_dispatch(params, &request.dispatch.dispatch_id)
    }

    fn observe(&self, dispatch_id: &str) -> Result<RuntimeObservation, AdapterFailure> {
        let response = host_call(
            true,
            "agent.conversation.active",
            &json!({"agent": self.provider.as_str(), "waitForChangeMs": 0}),
        )
        .map_err(|_| {
            AdapterFailure::retryable("subagent_observation_failed", "persistent-turn/observe")
        })?;
        let active = response
            .get("turns")
            .and_then(Value::as_array)
            .and_then(|turns| {
                turns.iter().find(|turn| {
                    turn.get("turnHandle").and_then(Value::as_str) == Some(dispatch_id)
                })
            });
        Ok(RuntimeObservation {
            dispatch_id: dispatch_id.to_owned(),
            transition: if active.is_some() {
                RuntimeTransition::Processing
            } else {
                RuntimeTransition::ReconciliationRequired
            },
            parts: active
                .map(|_| {
                    vec![RuntimeEventPart {
                        transition: RuntimeTransition::Processing,
                        visible_text: None,
                    }]
                })
                .unwrap_or_default(),
        })
    }

    fn cancel_active(
        &self,
        dispatch_id: &str,
        identity: &NativeResumeIdentity,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        if identity.binding_for_adapter(&self.provider) != Some(dispatch_id) {
            return Err(AdapterFailure::permanent(
                "active_dispatch_identity_mismatch",
                "persistent-turn/cancel",
            ));
        }
        let active = host_call(
            true,
            "agent.conversation.active",
            &json!({"agent": self.provider.as_str(), "waitForChangeMs": 0}),
        )
        .map_err(|_| {
            AdapterFailure::uncertain("subagent_cancel_uncertain", "persistent-turn/cancel")
        })?;
        let conversation_id = active
            .get("turns")
            .and_then(Value::as_array)
            .and_then(|turns| {
                turns.iter().find(|turn| {
                    turn.get("turnHandle").and_then(Value::as_str) == Some(dispatch_id)
                })
            })
            .and_then(|turn| turn.get("conversationId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AdapterFailure::permanent("subagent_turn_not_active", "persistent-turn/cancel")
            })?;
        let response = host_call(
            false,
            "agent.conversation.cancel",
            &json!({
                "turnHandle": dispatch_id,
                "conversationId": conversation_id,
                "agentId": self.provider.as_str(),
            }),
        )
        .map_err(|_| {
            AdapterFailure::uncertain("subagent_cancel_uncertain", "persistent-turn/cancel")
        })?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(AdapterFailure::uncertain(
                "subagent_cancel_uncertain",
                "persistent-turn/reconcile",
            ));
        }
        Ok(RuntimeDispatchReceipt {
            dispatch_id: dispatch_id.to_owned(),
            transition: RuntimeTransition::CancelRequested,
            identity: None,
        })
    }

    fn cleanup(&self, identity: &NativeResumeIdentity) -> Result<(), AdapterFailure> {
        let session_id = identity
            .binding_for_adapter(&self.provider)
            .ok_or_else(|| {
                AdapterFailure::permanent("native_resume_provider_mismatch", "identity/cleanup")
            })?;
        let cleanup = super::port::composition()
            .map(|composition| composition.cleanup_conversation)
            .ok_or_else(|| {
                AdapterFailure::retryable("subagent_cleanup_failed", "identity/cleanup")
            })?;
        let response = cleanup(&json!({
            "agent": self.provider.as_str(),
            "sessionId": session_id,
        }))
        .map_err(|_| AdapterFailure::retryable("subagent_cleanup_failed", "identity/cleanup"))?;
        if response.get("ok").and_then(Value::as_bool) == Some(true)
            || response.get("status").and_then(Value::as_str) == Some("not_persisted")
        {
            Ok(())
        } else {
            Err(AdapterFailure::retryable(
                "subagent_cleanup_failed",
                "identity/cleanup",
            ))
        }
    }
}

/// A fail-closed inventory port for this crate's own suites.
///
/// The composed port belongs to the composition above this crate, so a unit
/// test that needs one answers every member with the unavailable value rather
/// than reaching upward for the host's.
#[cfg(test)]
fn fixture_target_port() -> licoup_agent_targets::port::AgentTargetPort {
    licoup_agent_targets::port::AgentTargetPort::unavailable()
}

pub fn valid_opaque_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_environment(command: &std::process::Command, key: &str) -> Option<String> {
        command
            .get_envs()
            .find(|(name, _)| *name == std::ffi::OsStr::new(key))
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn caller_context_does_not_treat_a_direct_turn_as_subagent_lineage() {
        let mut command = std::process::Command::new("fixture");
        apply_subagent_caller_context(
            &mut command,
            &json!({
                "agentId": "codex",
                "conversationId": "conversation:fixture",
                "membershipId": "membership:codex",
                "dispatchId": "turn:direct"
            }),
        );

        assert_eq!(
            command_environment(&command, "LICOUP_MCP_CONVERSATION_ID").as_deref(),
            Some("conversation:fixture")
        );
        assert_eq!(
            command_environment(&command, "LICOUP_MCP_MEMBERSHIP_ID").as_deref(),
            Some("membership:codex")
        );
        assert_eq!(
            command_environment(&command, "LICOUP_MCP_PARENT_DISPATCH_ID"),
            None
        );
    }

    #[test]
    fn caller_context_forwards_only_an_explicit_subagent_parent() {
        let mut command = std::process::Command::new("fixture");
        apply_subagent_caller_context(
            &mut command,
            &json!({
                "agentId": "cursor",
                "conversationId": "conversation:fixture",
                "membershipId": "membership:cursor",
                "dispatchId": "turn:runtime",
                "parentDispatchId": "subagent:parent"
            }),
        );

        assert_eq!(
            command_environment(&command, "LICOUP_MCP_PARENT_DISPATCH_ID").as_deref(),
            Some("subagent:parent")
        );
    }

    #[test]
    fn mcp_runtime_root_binds_only_the_portable_data_directory() {
        let root = std::env::temp_dir().join(format!(
            "licoup-mcp-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
        let mut command = std::process::Command::new("fixture");
        apply_mcp_runtime_root(&mut command);
        let bound = command_environment(&command, "LICOUP_HOME");
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        let expected = root.to_string_lossy().into_owned();
        assert_eq!(bound.as_deref(), Some(expected.as_str()));
        assert_eq!(
            command_environment(&command, "LICOUP_MCP_CONVERSATION_ID"),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn production_provider_capabilities_are_explicit_and_bidirectional() {
        let runtimes = production_subagent_runtimes(
            crate::runtime_adapters::port::composition()
                .and_then(|composition| (composition.target_port)())
                .unwrap_or_else(fixture_target_port),
        );
        assert_eq!(
            runtimes
                .iter()
                .map(|runtime| runtime.provider_id().as_str())
                .collect::<Vec<_>>(),
            ["codex", "cursor", "antigravity", "claude-code"]
        );
        for runtime in runtimes {
            let caps = runtime.capabilities();
            assert!(caps.create && caps.exact_resume && caps.observe);
            assert!(caps.continue_turn && caps.active_cancel);
        }
    }

    #[test]
    fn host_dispatch_failures_keep_the_first_safe_stage() {
        for (source, code, stage, retryable, uncertain) in [
            (
                "persistent_conversation_transport_required",
                "persistent_conversation_transport_required",
                "persistent-turn/connect",
                true,
                false,
            ),
            (
                "subagent_turn_scope_mismatch",
                "subagent_turn_scope_mismatch",
                "session/authorize",
                false,
                false,
            ),
            (
                "subagent_transport_invalid_response",
                "subagent_transport_invalid_response",
                "persistent-turn/readback",
                true,
                true,
            ),
            (
                "private_unrecognized_failure",
                "subagent_transport_failed",
                "persistent-turn/exchange",
                true,
                true,
            ),
        ] {
            let failure = project_host_dispatch_failure(source.to_owned());
            assert_eq!(failure.code, code);
            assert_eq!(failure.stage, stage);
            assert_eq!(failure.retryable, retryable);
            assert_eq!(failure.uncertain_effect, uncertain);
        }
    }
}
