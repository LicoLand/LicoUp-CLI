#[cfg(all(feature = "secure-mesh-acceptance-mock-kt", not(debug_assertions)))]
compile_error!(
    "secure-mesh-acceptance-mock-kt is acceptance-only and cannot be compiled in a release profile"
);

pub mod contracts;
pub mod core;
pub mod domain;
pub mod ffi;
pub mod platform;

/// The agent-execution port every caller above the platform layer reaches.
///
/// One port owns execution, cancellation, steering, session resume and history
/// for every Agent. `licoup-agent-drivers` declares it; the platform layer
/// answers it with this host's own conversation lane; this module is the entry
/// a caller names, so no caller reaches the lane, the registry or an Agent's
/// own module directly.
///
/// It stays at the crate root, above both layers, for the same reason
/// [`target_port`] does, and the binaries and suites below reach it through the
/// facade functions re-exported here rather than through a new public module.
pub(crate) mod agent_port;

// The agent-execution port's public entries. A binary or an integration suite
// names these; every caller inside this crate names `crate::agent_port`.
pub use agent_port::{
    cancel as cancel_agent_turn, dispatch as dispatch_agent_operation, send as send_agent_turn,
    steer as steer_agent_turn,
};

/// Every declarative state machine this host compiles from
/// `resources/state-machines`. The JSON configuration is the transition
/// authority; an owner that reads a machine names it through this module.
pub(crate) mod state_machines {
    include!(concat!(env!("OUT_DIR"), "/state_machines.rs"));
}

/// The Agent inventory composition: this host's answers for the port
/// `licoup-agent-targets` declares.
///
/// Like the environment-port and product-version bindings below, it lives at
/// the crate root, above both layers: each fact is answered by the module that
/// owns it, and neither layer has to know the other. The public
/// [`agent_target_port`] accessor is the one path to it.
pub(crate) mod target_port;

/// The driver core's lane composition: this host's own conversation, generic
/// CLI, caller-manager and collaboration lanes, in the shape
/// `licoup-agent-drivers` reads them through its port.
///
/// It lives at the crate root for the same reason [`target_port`] does: two of
/// its answers read domain facts, and the platform layer may not reach the
/// domain layer. The layer that owns each fact stays where it is; this module
/// joins them once, above both.
pub(crate) mod host_lane;
/// The model catalogue composition: this host's answers for the port
/// `licoup-model-catalog` declares.
///
/// It lives at the crate root for the same reason the inventory's does: the
/// catalogue owns the selection facts, this host owns the probe, the
/// declarations and the data roots, and neither layer has to know the other.
pub(crate) mod model_catalog_port;

/// The process composition entry: installs this host's answers for the ports
/// its layers ask.
///
/// It lives at the crate root, above both layers, so neither layer has to know
/// the other: a layer declares the port it needs, the other owns the fact, and
/// this function joins them once per process. That covers the environment
/// ports the domain asks, the gateway runtime's ports, the stop control's
/// Subagent-claim dispatcher, which the domain answers, and the ports the Agent
/// adapter packages ask for — the progressive turn-event sinks Codex,
/// Antigravity, Pi, OpenClaw and Cursor emit through, the execution admission
/// the Antigravity, Kimi Code and OpenCode packages ask for, the serve engine
/// and consumer the Kilo Code package reaches through, and the launch
/// environment the DeepSeek Harness package asks for. A process that never
/// calls it keeps every port fail-closed.
pub fn install_environment_ports() -> Result<(), &'static str> {
    install_workflow_host_ports();
    domain::conversation::history::install_open_codex_rollouts(
        licoup_agent_codex::observation::open_rollout_paths,
    )?;
    platform::gateway_composition::install_readiness()?;
    // The statistics engine's product answers — catalog, history readers and
    // target readiness — are composed by the domain facade.
    domain::agent_usage::host::install()?;
    platform::stop_control::install_subagent_claim_stop(stop_subagent_claim)?;
    licoup_model_catalog::install_model_catalog_port(model_catalog_port::model_catalog_port())?;
    // The routing policy owner is composed after the catalogue port it reads,
    // so one dispatch entry asks the policy instead of choosing ad hoc.
    model_catalog_port::install_routing_policy_owner();
    platform::extension_packages::install_maintenance_admission(std::sync::Arc::new(
        PackageGenerationAdmission,
    ))?;
    // The cross-device entry is this host's own inbound door for verified peer
    // units. It is installed here so the native route and the device layer reach
    // one owner; a process that never installs it refuses every admission
    // instead of inventing an entry with no bindings.
    domain::cross_device_entry::install()?;
    // An adapter package owns what one of its turns emits; this host owns where
    // it goes, because the host owns the consumer. Each package is linked here
    // for its registration while its binary route is completed by the
    // agent-execution port, and a host that never installs these ports leaves the
    // packages' emitters silent rather than inventing a consumer.
    licoup_agent_codex::port::turn_event::install(platform::codex_turn_event_port())?;
    // The Pi adapter package answers the same way, for the same reason: one
    // consumer per process, installed once, and the package silent until then.
    licoup_agent_pi::port::turn_event::install(platform::pi_turn_event_port())?;
    licoup_agent_antigravity::port::turn_event::install(platform::antigravity_turn_event_port())?;
    // The Antigravity package asks this host two execution questions. The caller
    // context belongs to the Subagent mesh's own binding, and the admission
    // answer is the host's close-admission barrier, which a launching turn may
    // not bypass.
    licoup_agent_antigravity::port::execution::install(
        licoup_agent_antigravity::port::execution::ExecutionPort {
            subagent_caller_context: subagent_caller_context,
            admits_execution: admits_agent_execution,
        },
    )?;
    // The Kimi Code adapter package owns what one Kimi execution is; the one fact
    // it cannot derive is whether this host currently admits new work, because the
    // idle-admission decision is the host's. Installing the answer is what lets a
    // Kimi turn run at all, and a host that never installs it refuses rather than
    // running.
    licoup_agent_kimi::port::execution::install(
        licoup_agent_kimi::port::execution::ExecutionPort {
            admits_execution: admits_agent_execution,
        },
    )?;
    // The OpenClaw adapter package owns what one OpenClaw turn is — the Gateway
    // ACP frames, the attach it names and the events that attach produces — and
    // this host owns where its effects go: the Gateway lifecycle it starts,
    // reuses and stops, and the consumer its events reach. Both ports are
    // installed together because a package with an engine and no consumer, or a
    // consumer and no engine, is half-wired. Its agent-execution port stays
    // declared and fail-closed: the client drives this package in-process, so no
    // admission is claimed for a binary route that is not completed yet.
    licoup_agent_openclaw::port::gateway::install(platform::openclaw_host::gateway_port())?;
    licoup_agent_openclaw::port::turn_event::install(platform::openclaw_host::turn_event_port())?;
    // The Kilo Code adapter package owns what one Kilo turn is — the request
    // shape, the session protocol, the stream classification, the projection and
    // the turn itself — and this host owns the serve engine it runs on and the
    // consumer its events reach. Both ports are installed together because a
    // package with an engine and no consumer, or a consumer and no engine, is
    // half-wired. The package reaches the engine only through these ports, so no
    // Kilo module and no second description of the endpoint stays in the kernel.
    licoup_agent_kilo::host::install(platform::kilo_code_host::host_ports())?;
    // The DeepSeek Harness adapter package runs its own turn through the
    // foundation's supervisor, the raw-execution record and the turn-event
    // emitters, and the one fact it may not derive is the environment a launch
    // observes: the user's login-shell snapshot belongs to this host, which reads
    // it once per process. Installing the host's own answer is what lets a
    // Harness started from the desktop see the environment its user's terminal
    // would give it; a host that never installs the port leaves the launch with
    // the environment the process inherited rather than inventing a snapshot.
    licoup_agent_deepseek::port::launch_environment::install(
        platform::user_shell_environment::apply_to_command,
    )?;
    // The OpenCode adapter package owns what one OpenCode turn is — the request
    // shape, the session protocol, the stream classification, the projection and
    // the endpoint contract — and this host owns the serve engine it runs on and
    // the consumer its events reach. Both ports are installed together because a
    // package with an engine and no consumer, or a consumer and no engine, is
    // half-wired. The admission answer is installed beside them: it is the one
    // fact a turn may not decide for itself, because a turn started under the
    // close-admission barrier would run an Agent a maintenance switch may be
    // replacing.
    licoup_agent_opencode::port::execution::install(
        licoup_agent_opencode::port::execution::ExecutionPort {
            admits_execution: admits_agent_execution,
        },
    )?;
    licoup_agent_opencode::host::install(platform::opencode_host::host_ports())?;
    // The Lico Agent adapter package owns the whole of one Lico Agent turn — the
    // RPC launch, the stdio JSONL exchange, the session and plan layout and the
    // sealed profile a Plan turn runs under — and this host owns the platform
    // fact it may not decide for itself: how the platform's sandbox primitive
    // turns a profile into a running command. It is answered from this client's
    // own primitive, so Plan mode cannot run unsandboxed. The package's
    // agent-execution seam stays declared and fail-closed for the same reason
    // Cursor's does: the extension host that starts the package's own binary is
    // what answers it, and until then the package refuses rather than claiming
    // an admission this composition never gave it.
    licoup_agent_lico_agent::port::sandbox::install(platform::lico_agent_host::sandbox_port())?;
    // The Cursor adapter package owns one Cursor turn — the launch on the shared
    // pty, the strict-NDJSON stream, the update signals and the outcome — and
    // this host owns the reader its events reach. Its agent-execution port stays
    // declared and fail-closed: the extension host that starts the package's own
    // binary is what answers it.
    licoup_agent_cursor::port::turn_event::install(platform::cursor_turn_event_port())
}

/// The composition's answer for the Antigravity adapter package's caller-context
/// query: the exported Subagent caller context this host's own drivers bind.
///
/// It is the same environment contract the launcher applies, read through the
/// port so the package asks the host rather than reading a second copy of the
/// variable names.
fn subagent_caller_context() -> Option<String> {
    let provider = std::env::var("LICOUP_MCP_CALLER_PROVIDER").ok()?;
    if provider.trim().is_empty() {
        return None;
    }
    Some(provider)
}

/// Whether this host admits a new Agent execution right now: admission is open
/// unless a maintenance switch holds the close-admission barrier.
///
/// The barrier is the same record package activation and data conversion hold,
/// and it is read when the question is asked rather than cached, so a turn
/// cannot start from a decision the host has since closed. A data root whose
/// barrier cannot be read admits nothing: an unreadable record is not evidence
/// that admission is open.
fn admits_agent_execution() -> bool {
    let Ok(data_root) = licoup_foundation::platform::paths::portable_data_dir() else {
        return false;
    };
    domain::work_admission::WorkAdmission::open(data_root)
        .barrier()
        .is_ok_and(|barrier| barrier.is_none())
}

/// The workflow composition: this host's answers for the ports
/// `licoup-workflow-runtime` declares.
///
/// It lives at the crate root for the same reason [`target_port`] and
/// [`host_lane`] do: the extracted runtime declares the port, this host owns
/// the fact, and neither layer has to know the other. Construction of the
/// workflow service installs it, so the composition is reached from every
/// entry point and never from inside a platform module.
pub(crate) mod workflow_host;

/// Install this host's answers for the workflow runtime's ports.
///
/// Idempotent: the first installation wins, and a process that never calls it
/// keeps every workflow port fail-closed.
pub fn install_workflow_host_ports() {
    workflow_host::install_workflow_host_ports();
}

/// The composition's answer for the package-generation admission port: the
/// host-wide idle decision and its barrier, which live in the domain layer.
struct PackageGenerationAdmission;

impl platform::extension_packages::MaintenanceAdmission for PackageGenerationAdmission {
    fn hold(&self, data_root: &std::path::Path) -> Result<(), &'static str> {
        domain::work_admission::hold_package_activation_admission(data_root)
    }

    fn release(&self, data_root: &std::path::Path) -> Result<(), &'static str> {
        domain::work_admission::release_maintenance_admission(data_root)
    }
}

/// The composition's answer for the stop control's Subagent-claim port: the
/// claim owner's own dispatcher, which lives in the domain layer.
fn stop_subagent_claim(
    conversation_id: &str,
    caller_membership_id: &str,
    target_membership_id: &str,
) -> Result<serde_json::Value, &'static str> {
    domain::subagents::stop_active_claim(
        conversation_id,
        caller_membership_id,
        target_membership_id,
    )
    .map_err(|error| error.code)
}

/// The product version this binary was built with.
///
/// The build script injects `LICO_CLIENT_PRODUCT_VERSION` from
/// `tools/client-version.json`; a build with no injected version is a
/// development build and reports the documented fallback. The composition owns
/// this process fact, so the platform layer answers a package's compatibility
/// admission through it instead of reaching into the domain layer.
pub fn running_product_version() -> anyhow::Result<&'static str> {
    domain::client_state_migration::running_product_version()
}

/// The Agent inventory port this host composes, for the `lico-agent` binary.
///
/// The composition itself is the crate-root [`target_port`] module; this is the
/// one public path to it, so the domain layer no longer hosts a composition
/// that reaches into the platform layer.
pub fn agent_target_port() -> licoup_agent_targets::port::AgentTargetPort {
    target_port::agent_target_port()
}

// The Agent inventory port: the facts `licoup-agent-targets` reads from the
// layers above it. It is declared by the inventory crate and composed by
// `target_port`; this alias keeps the two naming one path without widening the
// host's public surface.
pub(crate) use licoup_agent_targets::port;
