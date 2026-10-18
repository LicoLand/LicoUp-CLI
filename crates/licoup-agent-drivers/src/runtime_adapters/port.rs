//! The port the host's adapter registry and adapter execution read one Agent
//! through.
//!
//! This tree owns the host side of Agent execution: which packaged adapters
//! exist, what each of them declares, how a turn is admitted and dispatched,
//! and how an Agent's own report is normalized into this host's execution
//! vocabulary. It owns no Agent's protocol. Everything it needs *about* one
//! Agent — that Agent's driver identity, the protocol name its driver speaks,
//! how its executable is probed, how one of its turns is executed, and whether
//! a durable native identity is valid for it — arrives through
//! [`AgentDriverRegistration`], and the composition above this crate supplies
//! the registrations: `licoup-native` while the Agent crates do not exist,
//! `licoup-agent-<agent>` once they do.
//!
//! The same shape is used one layer down by `licoup-agent-adapter-sdk`, whose
//! `AdapterParserSet` carries one `ParserRegistration` per Agent. A registration
//! here embeds that registration rather than declaring a second one, so an
//! Agent has exactly one declaration and one set of protocol-agnostic answers.
//!
//! Every member is a `fn` pointer rather than a trait, so the host keeps no
//! state a caller did not hand it, and no vendor name is compiled into this
//! crate. The composition is installed once, by [`install`]; a build that
//! installs nothing answers fail-closed rather than inheriting a guess.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use licoup_agent_adapter_sdk::Transition;
use licoup_agent_adapter_sdk::port::{AdapterParserSet, ParserRegistration};
use licoup_agent_targets::domain::cli_registration::CliRegistration;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use licoup_agent_targets::port::AgentTargetPort;
use serde_json::Value;

use super::adapter::RuntimeAdapter;
use super::error::RuntimeAdapterError;
use super::model::NormalizedExecution;
use super::subagent_mesh::CallerManagerKind;

/// One Agent protocol's failure, reduced to the facts the host's response
/// mapping reads.
///
/// The fields are the facts every Agent's driver failure already carries, so
/// projecting one onto this shape is a field copy and never a second
/// derivation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverFailureFacts {
    /// The failure's stable code.
    pub code: String,
    /// The failure's redacted message.
    pub message: String,
    /// The lifecycle stage the failure was observed at.
    pub stage: String,
    /// The component that reported it, when the Agent names one.
    pub component: Option<String>,
    /// Whether repeating the same request could succeed.
    pub retryable: Option<bool>,
    /// The Agent's own recovery hint, when it gives one.
    pub recovery: Option<String>,
    /// Whether the Agent is waiting for a user decision.
    pub user_interaction_required: bool,
    /// The request method the failure was observed on.
    pub request_method: Option<String>,
    /// The native session identity the failure carries.
    pub session_id: Option<String>,
    /// The native thread identity the failure carries.
    pub thread_id: Option<String>,
    /// The native turn identity the failure carries.
    pub turn_id: Option<String>,
    /// The turn status the failure was observed in.
    pub turn_status: Option<String>,
}

/// The effective settings one Agent's driver resolved for a turn.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverEffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_mode: Option<String>,
    pub mode: Option<String>,
    pub runtime_agent: Option<String>,
    pub allow_all: Option<bool>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

/// The capability facts one Agent's ACP engine negotiated for a turn.
///
/// It is `None` for every Agent that is not reached over the shared ACP
/// engine, and the host's normalization reports the ACP capability object only
/// when it is present.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AcpCapabilityFacts {
    pub protocol_version: Option<u64>,
    pub load_session: bool,
    pub resume_session: bool,
    pub close_session: bool,
    pub list_sessions: bool,
    pub delete_session: bool,
    pub image_prompts: bool,
    pub audio_prompts: bool,
    pub embedded_context: bool,
}

/// One Agent driver's raw result, in the protocol-agnostic shape the host's
/// normalization reads.
///
/// This is the projection of one Agent's own `RunResult`. It is a superset of
/// every Agent's result, so an Agent that reports no ACP capability facts and
/// no transitions leaves them empty and nothing is inferred on its behalf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DrivenRun {
    pub ok: bool,
    pub output: String,
    /// The transitions the Agent's own parser produced, when its engine reports
    /// them directly. An Agent whose transitions are derived from the outcome
    /// by its registered parser leaves this empty and the host asks the parser.
    pub transitions: Vec<Transition>,
    pub error: Option<DriverFailureFacts>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: DriverEffectiveSettings,
    /// The ACP engine's negotiated capability facts, when this Agent is reached
    /// over ACP.
    pub acp_capabilities: Option<AcpCapabilityFacts>,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
    /// The protocol name this Agent's driver spoke for this turn.
    pub runtime_protocol: &'static str,
    /// This Agent's driver identity, as its own engine reports it.
    pub driver_id: &'static str,
}

impl Default for DrivenRun {
    fn default() -> Self {
        Self {
            ok: false,
            output: String::new(),
            transitions: Vec::new(),
            error: None,
            session_id: String::new(),
            thread_id: String::new(),
            turn_id: String::new(),
            turn_status: String::new(),
            effective: DriverEffectiveSettings::default(),
            acp_capabilities: None,
            status_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
            started_at: String::new(),
            runtime_protocol: "",
            driver_id: "",
        }
    }
}

/// One Agent's driver run, in the shape the host's dispatch resolves it.
///
/// The fields are the arguments the host already resolved — the executable, the
/// caller's params, the prompt, the bound native session, the working directory,
/// the resolved dispatch timeout, the two output bounds and the SSH runtime
/// connection when the turn is bound to a guest — so an implementing Agent
/// projects them onto its own execution entry point without a second derivation
/// of any of them.
pub struct AgentRun<'a> {
    /// The executable the driver process is launched from.
    pub executable: &'a str,
    /// The caller's adapter params.
    pub params: &'a Value,
    /// The prompt text, already selected from `text` or `message`.
    pub prompt: &'a str,
    /// The bound native session id, empty when the binding carries none.
    pub session_id: &'a str,
    /// The working directory: the call's own, else the configured one.
    pub cwd: Option<&'a Path>,
    /// The resolved dispatch timeout. Zero means unbounded.
    pub timeout_ms: u64,
    /// The stdout bound, when the transport sets one.
    pub max_stdout: Option<usize>,
    /// The stderr bound.
    pub max_stderr: usize,
    /// The SSH runtime connection this turn is bound to, when it is bound to a
    /// guest host. An Agent whose protocol differs between its local and its
    /// guest lane reads it to pick the lane it must speak.
    pub runtime_connection: Option<&'a SshRuntimeConnection>,
}

/// Run one Agent's driver and report the host's normalized execution.
///
/// The member is the whole per-Agent pipeline — that Agent's own execution
/// followed by this host's own normalization of the result — because the host
/// owns the normalization vocabulary and the Agent owns the execution, and the
/// composition above is the one place that has both in view.
pub type AgentExecute = fn(&AgentRun<'_>) -> NormalizedExecution;

/// Probe one Agent's executable and report its redacted capability facts.
pub type AgentProbe = fn(&str, &Path) -> Value;

/// One Agent, as the host's adapter registry and adapter execution reach it.
///
/// The six facts are the whole of what the host needs and the whole of what it
/// may not know by name: the inventory id the registry addresses it by, its
/// driver identity, the protocol name its driver speaks, its probe, its
/// execution, and its own parser registration.
#[derive(Clone, Copy)]
pub struct AgentDriverRegistration {
    /// The inventory id the registry and the diagnostics address this Agent by.
    pub agent_id: &'static str,
    /// This Agent's driver identity, as its own engine reports it.
    pub driver_id: &'static str,
    /// The protocol name this Agent's driver speaks on the ordinary lane.
    pub runtime_protocol: &'static str,
    /// Probe this Agent's executable and report its capability facts.
    pub probe: AgentProbe,
    /// Run one turn against this Agent and report the host's normalized
    /// execution.
    pub run: AgentExecute,
    /// This Agent's own parser, as `licoup-agent-adapter-sdk` declares it. The
    /// registration is embedded rather than restated, so an Agent's declaration
    /// and its protocol-agnostic answers cannot drift apart.
    pub parser: ParserRegistration,
}

/// How the host reads the host's own conversation lane on the Subagent mesh's
/// two paths.
///
/// `execute` mutates the lane; `execute_read_only` observes it. They are two
/// members rather than one flag because the lane's own read-only path takes a
/// different route and must not be able to mutate.
#[derive(Clone, Copy)]
pub struct ConversationHostPort {
    /// Dispatch one mutating host-lane call.
    pub execute: fn(&str, &Value) -> Result<Value, String>,
    /// Dispatch one read-only host-lane call.
    pub execute_read_only: fn(&str, &Value) -> Result<Value, String>,
}

/// How the host resolves the optional MCP servers one ACP runtime reaches the
/// host's collaboration surface through.
///
/// The ACP transport reads it to fill in a session's `mcpServers`, and it is the
/// host's answer rather than the transport's: the registration is stored in the
/// host's own client state and canonicalised against the host's own inventory,
/// so a crate below the host cannot read it without depending on the host back.
/// A host that composes none answers with no server, which is what an
/// unconfigured installation reports.
#[derive(Clone, Copy)]
pub struct CollaborationMcpPort {
    /// The MCP server descriptors one runtime reaches the collaboration
    /// surface through, or the reason the registration is not usable.
    pub acp_servers_for_runtime: fn(&str) -> Result<Vec<Value>, String>,
}

/// How the host manages one Agent's caller integration when that Agent ships a
/// plugin LicoUp installs.
///
/// Only the Agent that ships such a plugin has one; the host names no plugin.
#[derive(Clone, Copy)]
pub struct CallerManagerPort {
    /// The digest of the registration this manager would apply.
    pub plan_digest: fn(&Path) -> Result<String, ()>,
    /// Whether the registration is already applied and ready.
    pub ready: fn(&Path) -> bool,
    /// Apply or remove the registration, bound to the approved digest.
    pub apply: fn(&Path, &str, bool) -> Result<(), ()>,
}

/// How the host manages one Agent's user-configuration caller registration.
///
/// Some Agents reach the host's Subagent surface through a registration LicoUp
/// writes into that Agent's own user configuration. The Subagent mesh drives
/// that registration through this port rather than by name, and `kind` is the
/// registration kind the host composed for the Agent it is registering, so the
/// port names no Agent and no registration file.
#[derive(Clone, Copy)]
pub struct CallerConfigPort {
    /// The integration's own default connector path, when it has one.
    pub default_connector: fn() -> Option<PathBuf>,
    /// Whether the registration is already present and ready.
    pub ready: fn(CallerManagerKind, &Path) -> bool,
    /// The digest of the registration the host would apply.
    pub plan_digest: fn(CallerManagerKind, &Path) -> Result<String, ()>,
    /// Apply or remove the registration, bound to the approved digest.
    pub apply: fn(CallerManagerKind, &Path, &str, bool) -> Result<(), ()>,
}

/// One generic-CLI lane execution's raw result, in the shape the host's
/// normalization reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericCliRun {
    pub ok: bool,
    pub timed_out: bool,
    pub output: String,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub started_at: String,
    pub runtime_protocol: &'static str,
}

/// The generic CLI/PTY lane, as the host's fallback dispatch reaches it.
///
/// The lane itself is an engine, not an Agent, but it is composed above this
/// crate like every other driver, so it arrives through the same port rather
/// than by name.
#[derive(Clone, Copy)]
pub struct GenericCliPort {
    /// The lane's stable driver identity.
    pub driver_id: &'static str,
    /// Resolve the executable one CLI registration runs.
    pub resolve_executable:
        fn(&AgentTargetPort, &CliRegistration, &Value) -> Result<String, RuntimeAdapterError>,
    /// Run one turn on the lane.
    pub execute: fn(
        &CliRegistration,
        &str,
        &Value,
        &str,
        Option<&Path>,
        u64,
        Option<usize>,
    ) -> Result<GenericCliRun, RuntimeAdapterError>,
}

/// The composition above this crate, in the shape the host reads it.
///
/// It is one value rather than six globals, so a program either composes the
/// host completely or composes nothing, and a half-composed host cannot exist.
#[derive(Clone, Copy)]
pub struct HostComposition {
    /// The composed Agent inventory port, read by the Subagent mesh.
    pub target_port: fn() -> Option<AgentTargetPort>,
    /// The parser set this host injects into `licoup-agent-adapter-sdk`.
    pub parser_set: fn() -> AdapterParserSet,
    /// Every Agent this host composes, in packaged inventory order.
    pub drivers: &'static [AgentDriverRegistration],
    /// How this host reaches its own conversation lane.
    pub conversation_host: ConversationHostPort,
    /// The generic CLI/PTY fallback lane.
    pub generic_cli: GenericCliPort,
    /// How this host resolves an ACP runtime's collaboration MCP servers.
    pub collaboration_mcp: CollaborationMcpPort,
    /// How this host manages a plugin-shipping Agent's caller integration.
    pub caller_manager: CallerManagerPort,
    /// How this host manages a user-configuration caller registration.
    pub caller_config: CallerConfigPort,
    /// How this host resolves the dispatch timeout one turn is bounded by. The
    /// policy is the host's own persisted setting, so its reading arrives here
    /// rather than being re-derived below the host.
    pub dispatch_timeout: fn(&Value) -> Result<u64, ()>,
    /// Whether one Agent's own lane declares a capability flag.
    pub declared_capability_flag: fn(RuntimeAdapter, &str) -> Option<bool>,
    /// End one Agent's persisted conversation.
    pub cleanup_conversation: fn(&Value) -> Result<Value, String>,
}

static COMPOSITION: OnceLock<HostComposition> = OnceLock::new();

/// Install the composition this host reads every Agent through.
///
/// The first installation wins: a second one is refused rather than silently
/// replacing the answers a running turn may already be reading.
pub fn install(composition: HostComposition) -> bool {
    COMPOSITION.set(composition).is_ok()
}

/// The composition this host reads, or `None` when nothing installed one.
pub fn composition() -> Option<HostComposition> {
    COMPOSITION.get().copied()
}

/// The parser set this host injects into the adapter SDK. A host that composed
/// no parser answers with the fail-closed set.
pub fn parser_set() -> AdapterParserSet {
    match composition() {
        Some(composition) => (composition.parser_set)(),
        None => AdapterParserSet::unavailable(),
    }
}

/// The dispatch timeout one turn is bounded by, as the host resolved it.
///
/// A host that composed no policy answers fail-closed: an unreadable policy is
/// an invalid request, never an unbounded turn.
pub fn dispatch_timeout(params: &Value) -> Result<u64, ()> {
    match composition() {
        Some(composition) => (composition.dispatch_timeout)(params),
        None => Err(()),
    }
}

/// How the host manages a user-configuration caller registration. A host that
/// composed none answers fail-closed.
pub fn caller_config() -> Option<CallerConfigPort> {
    composition().map(|composition| composition.caller_config)
}

/// The MCP servers one ACP runtime reaches the collaboration surface through.
///
/// A host that composed no answer reports no server, matching the fail-closed
/// shape every other accessor here uses.
pub fn collaboration_acp_servers(runtime_id: &str) -> Result<Vec<Value>, String> {
    match composition() {
        Some(composition) => (composition.collaboration_mcp.acp_servers_for_runtime)(runtime_id),
        None => Ok(Vec::new()),
    }
}

/// Every Agent this host composes, in packaged inventory order.
pub fn drivers() -> &'static [AgentDriverRegistration] {
    composition()
        .map(|composition| composition.drivers)
        .unwrap_or(&[])
}

/// The registration of one composed Agent, or `None` when this host composes
/// no driver for that inventory id.
pub fn registration_for(agent_id: &str) -> Option<AgentDriverRegistration> {
    drivers()
        .iter()
        .copied()
        .find(|registration| registration.agent_id == agent_id)
}

/// The registration of one composed Agent, by the dispatch enum.
pub fn registration_for_adapter(adapter: RuntimeAdapter) -> Option<AgentDriverRegistration> {
    registration_for(adapter.id())
}

impl AgentDriverRegistration {
    /// This Agent's own parser, in the shape the SDK's registry lookup reads.
    pub fn parser(&self) -> ParserRegistration {
        self.parser
    }
}
