//! The port the Agent inventory declares for the modules composed above it.
//!
//! The inventory owns the declarations — which Agents exist on this machine,
//! where they live and what each one declares — and it stops there. Everything
//! it reports *about* a declaration that is owned by a module above it arrives
//! through [`AgentTargetPort`]: the driver engines answer whether a discovered
//! binary can be driven, project the packaged declaration into
//! [`RuntimeDriverFacts`], and name the local Agent workspace a probe runs in;
//! the model facts answer canonical model identity and agent intelligence;
//! conversation state answers an Agent's recorded model catalog and the token
//! usage of one response; the local model gateway answers its own autostart
//! state and rewrites it after a data-root recovery.
//!
//! The port is declared here and implemented by those crates, and composition
//! supplies it. It is a value of `fn` pointers rather than a trait so that the
//! inventory keeps no state a caller did not hand it, and every member is
//! called only on the branch that needs it — computing one eagerly would run
//! the probe, the cache write or the network-free catalog read on every scan.
//!
//! [`AgentTargetPort::unavailable`] declares every fact as unknown. It is the
//! honest answer for a host that composes no driver engine and no model facts,
//! and it is what this crate's own tests state their expectations against; a
//! production caller always names the real owners instead.

use anyhow::Result;
use licoup_client_state::ClientStateStore;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// One packaged driver declaration, projected for the inventory.
///
/// The fields are the same facts `AdapterCapabilities` reports, so the
/// projection from the declaration document to an inventory record is a field
/// copy and never a second derivation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeDriverFacts {
    pub driver_status: String,
    pub readiness: String,
    pub protocol: String,
    pub blocker: Option<String>,
    pub capability_matrix: Option<Value>,
    pub summary_codes: Vec<String>,
    pub consecutive_passes: usize,
    pub evidence_age_class: String,
}

/// The canonical identity of one model, as the model-facts layer resolves it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalModelFacts {
    pub id: String,
    pub lab_id: String,
}

/// Probe one discovered binary and report whether its conversation runtime is
/// supported. It runs the driver's own bounded probe, so it is called only
/// when discovery already decided to probe.
pub type ProbeRuntimeDriver = fn(agent_id: &str, binary: &Path, working_directory: &Path) -> Value;

/// Project the packaged driver declaration for one Agent. `None` means no
/// packaged driver declares that Agent, which is the admission boundary for a
/// conversation runtime.
pub type RuntimeDriverProfile = fn(agent_id: &str) -> Option<RuntimeDriverFacts>;

/// The private workspace a capability probe for one Agent runs in. The driver
/// engines own the layout; the inventory only needs a directory to hand the
/// probe, and `None` means no workspace can be prepared.
pub type DefaultLocalAgentWorkspace = fn(agent_id: &str) -> Option<PathBuf>;

/// Read the Codex App Server model catalog as a live projection.
pub type CodexAppServerModelCatalog = fn(binary: &Path) -> Result<Value>;

/// Resolve one discovered model name to its canonical identity.
pub type ResolveCanonicalModel = fn(
    raw: &str,
    provider_id: Option<&str>,
    source_agent_id: Option<&str>,
) -> Option<CanonicalModelFacts>;

/// The revision of the canonical registry snapshot a catalog was built from.
pub type ModelRegistryRevision = fn() -> String;

/// Attach the allowlisted public intelligence fields for one model name.
pub type AttachAllowlistedModelFields = fn(model_name: &str, entry: &mut Map<String, Value>);

/// The highest published intelligence score for one Agent and model.
pub type AgentModelMaxIntelligence = fn(agent_id: &str, model_id: &str) -> Option<i64>;

/// One Agent's recorded model catalog, read from local conversation history.
pub type ConversationModelCatalog = fn(params: &Value) -> Result<Value>;

/// Bounded token usage extracted from one recorded response.
pub type ExtractTokenUsage = fn(value: &Value) -> Option<Value>;

/// The local model gateway's autostart state document.
pub type AutostartStatus = fn() -> Result<Value>;

/// Install the local model gateway's login autostart on the given port.
pub type AutostartEnable = fn(port: u16) -> Result<Value>;

/// Remove the local model gateway's login autostart.
pub type AutostartDisable = fn() -> Result<Value>;

/// Rewrite the local model gateway's installed login definitions after a
/// saved-data-root recovery. Nothing is installed on a host that never enabled
/// the gateway, so a caller runs it unconditionally.
pub type AutostartRefresh = fn() -> Result<()>;

/// Append one bounded activity event to a client-state journal. The journal
/// owner sits above the inventory, which records exactly one event: the saved
/// manual target. The store is passed in so the caller writes the event into
/// the same store instance the record was written through.
pub type AppendActivityEvent =
    fn(store: &ClientStateStore, event_type: &str, payload: Value) -> Result<Value>;

/// Every fact the Agent inventory reads from a module composed above it.
#[derive(Clone)]
pub struct AgentTargetPort {
    pub probe_runtime_driver: ProbeRuntimeDriver,
    pub runtime_driver_profile: RuntimeDriverProfile,
    pub append_activity_event: AppendActivityEvent,
    pub default_local_agent_workspace: DefaultLocalAgentWorkspace,
    pub codex_app_server_model_catalog: CodexAppServerModelCatalog,
    pub resolve_canonical_model: ResolveCanonicalModel,
    pub model_registry_revision: ModelRegistryRevision,
    pub attach_allowlisted_model_fields: AttachAllowlistedModelFields,
    pub agent_model_max_intelligence: AgentModelMaxIntelligence,
    pub conversation_model_catalog: ConversationModelCatalog,
    pub extract_token_usage: ExtractTokenUsage,
    pub autostart_status: AutostartStatus,
    pub autostart_enable: AutostartEnable,
    pub autostart_disable: AutostartDisable,
    pub autostart_refresh: AutostartRefresh,
}

impl AgentTargetPort {
    /// Every fact unknown. A caller that composes no owner above the inventory
    /// states that explicitly instead of inheriting another host's answer.
    ///
    /// `autostart_refresh` succeeds rather than fails: a host with no local
    /// model gateway has no installed definition to rewrite, so the honest
    /// answer is that there is nothing to do.
    pub const fn unavailable() -> Self {
        Self {
            probe_runtime_driver: |_, _, _| Value::Null,
            runtime_driver_profile: |_| None,
            append_activity_event: |_, _, _| Ok(Value::Null),
            default_local_agent_workspace: |_| None,
            codex_app_server_model_catalog: |_| Ok(Value::Null),
            resolve_canonical_model: |_, _, _| None,
            model_registry_revision: String::new,
            attach_allowlisted_model_fields: |_, _| {},
            agent_model_max_intelligence: |_, _| None,
            conversation_model_catalog: |_| Ok(Value::Null),
            extract_token_usage: |_| None,
            autostart_status: || Ok(Value::Null),
            autostart_enable: |_| Ok(Value::Null),
            autostart_disable: || Ok(Value::Null),
            autostart_refresh: || Ok(()),
        }
    }
}

/// The port a test build states its expectations against.
///
/// The declarations below are the packaged driver documents projected to the
/// shape this crate reads, so a scan fixture asserts the projection it produces
/// against the same facts the packaged inventory declares rather than against a
/// second derivation. They travel behind `test-support` because `cfg(test)` is
/// false for a dependency.
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures {
    use super::{AgentTargetPort, RuntimeDriverFacts};
    use licoup_client_state::ClientStateStore;
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};

    /// `(agent id, runtime protocol, consecutive passes)` from
    /// `resources/agent-conversation-drivers.json` and
    /// `resources/agent-conversation-readiness.json`.
    static DRIVERS: &[(&str, &str, usize)] = &[
        ("openclaw", "openclaw-acp-stdio-jsonrpc", 0),
        ("claude-code", "claude-code-cli-stream-json", 3),
        ("codex", "codex-app-server-stdio-jsonrpc", 1),
        ("antigravity", "antigravity-cli-argv-hook-v1", 3),
        ("opencode", "opencode-serve-http-v1", 3),
        ("copilot", "copilot-acp-v1-stdio-ndjson", 0),
        ("kilo-code", "kilo-code-serve-http-v1", 0),
        ("cursor", "cursor-agent-cli-v1", 3),
        ("hermes", "hermes-acp-stdio-jsonrpc", 0),
        ("kimi-code", "kimi-code-acp-v1-stdio-ndjson", 3),
        ("pi", "pi-rpc-stdio-jsonl", 0),
        ("deepseek-harness", "deepseek-harness-sdk-stdio-jsonrpc", 0),
        ("lico-agent", "lico-agent-rpc-stdio-jsonl", 0),
    ];

    fn profile(agent_id: &str) -> Option<RuntimeDriverFacts> {
        DRIVERS
            .iter()
            .find(|(id, _, _)| *id == agent_id)
            .map(|(_, protocol, passes)| RuntimeDriverFacts {
                driver_status: "implemented".to_string(),
                readiness: "unverified".to_string(),
                protocol: (*protocol).to_string(),
                blocker: Some("release_evidence_incomplete".to_string()),
                // Every packaged declaration carries a capability matrix, and a
                // projection that reports a driver without one would be the
                // defect this fixture exists to catch.
                capability_matrix: Some(json!({
                    "laneFamily": "protocol",
                    "cancel": true,
                    "interruptSteer": true,
                    "imageInput": false,
                })),
                // A declaration with no consecutive pass has no evidence at
                // all; one with passes has stale or incomplete evidence. That
                // is the packaged document's own distinction.
                summary_codes: vec![if *passes == 0 {
                    "evidence_missing".to_string()
                } else {
                    "evidence_stale_or_incomplete".to_string()
                }],
                consecutive_passes: *passes,
                evidence_age_class: "unknown".to_string(),
            })
    }

    /// The activity journal's bounded record — its schema version, generated
    /// event id, timestamp and payload redaction — belongs to the journal owner
    /// and is asserted by that owner's tests. This fixture answers with the
    /// identity the inventory itself supplied, so a scan fixture asserts the
    /// event the inventory asked for and never a second record shape.
    fn append_activity(
        _store: &ClientStateStore,
        event_type: &str,
        payload: Value,
    ) -> anyhow::Result<Value> {
        Ok(json!({ "type": event_type, "payload": payload }))
    }

    fn probe(agent_id: &str, _binary: &Path, _cwd: &Path) -> Value {
        json!({
            "available": true,
            "supported": agent_id == "cursor",
            "errorCode": Value::Null
        })
    }

    fn workspace(agent_id: &str) -> Option<PathBuf> {
        Some(std::env::temp_dir().join(format!("licoup-agent-workspace-{agent_id}")))
    }

    /// The agent ids the packaged driver declarations name.
    pub fn declared_agent_ids() -> Vec<&'static str> {
        DRIVERS.iter().map(|(id, _, _)| *id).collect()
    }

    /// A port that declares the packaged drivers and answers every other fact
    /// as unknown.
    pub fn packaged() -> AgentTargetPort {
        AgentTargetPort {
            probe_runtime_driver: probe,
            runtime_driver_profile: profile,
            default_local_agent_workspace: workspace,
            append_activity_event: append_activity,
            ..AgentTargetPort::unavailable()
        }
    }
}
