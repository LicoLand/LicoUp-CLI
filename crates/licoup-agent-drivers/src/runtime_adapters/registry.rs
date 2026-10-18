use super::PACKAGED_RUNTIME_ADAPTER_IDS;
use super::adapter::{
    CapabilityFact, CapabilityFactName, CapabilityFactState, NativeAbilityKind,
    NativeCapabilityKind, NativeDriverStateKind, RuntimeAdapter, adapter_for_agent,
};
use super::live_status::LiveSnapshot;
use super::model::{
    DriverInventoryDocument, DriverInventoryEntry, NativeCapabilityDocument, ReadinessDocument,
    ReadinessEntry, ReadinessSummary, RuntimeDriverProfile, RuntimeDriverRegistry,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{OnceLock, RwLock};

const DRIVER_INVENTORY_SCHEMA_VERSION: &str = "v0.0.1:client-agent-conversation-drivers-1";
const NATIVE_CAPABILITY_SCHEMA_VERSION: &str = "v0.0.1:client-agent-native-capabilities-1";
const READINESS_SCHEMA_VERSION: &str = "v0.0.1:client-agent-conversation-readiness-1";
const CONVERSATION_PARITY_CONTRACT_VERSION: &str = "CL-06";
const MINIMUM_CONSECUTIVE_PASSES: usize = 1;
pub const DRIVER_INVENTORY_JSON: &str =
    include_str!("../../resources/agent-conversation-drivers.json");
pub const NATIVE_CAPABILITY_JSON: &str =
    include_str!("../../resources/agent-native-capabilities.json");
pub const READINESS_JSON: &str = include_str!("../../resources/agent-conversation-readiness.json");
const CORE_CHECK_IDS: &[&str] = &[
    "P-01", "P-02", "P-03", "P-04", "P-05", "P-06", "P-07", "P-08", "P-09", "P-10",
];
const CONDITIONAL_CHECK_IDS: &[&str] = &["C-01", "C-02", "C-03", "C-04", "C-05", "C-06"];
const REQUIRED_EVIDENCE_BOOLEANS: &[&str] = &[
    "officialNativeLane",
    "conversationGatePassed",
    "cleanupPassed",
    "privacyPassed",
];
const REQUIRED_EVIDENCE_COUNTS: &[&str] = &["consecutivePasses"];
const REQUIRED_EVIDENCE_DIGESTS: &[&str] = &[
    "runtimeVersionDigest",
    "capabilitySnapshotDigest",
    "adapterManifestDigest",
    "releaseArtifactDigest",
    "releaseSidecarDigest",
    "productContinuityBindingDigest",
    "registryDigest",
    "driverInventoryDigest",
    "evidenceDigest",
];
const REQUIRED_EVIDENCE_BINDINGS: &[&str] = &[
    "agentId",
    "driverId",
    "runtimeProtocol",
    "harnessVersion",
    "runtimeVersionClass",
    "runtimeSourceClass",
];

/// Logical owners of the capability facts. A source names the owner, never a
/// local path or a runtime value.
pub const SOURCE_AGENT_CLI_PRESENCE: &str = "agent-cli-presence";
pub const SOURCE_AGENT_DESKTOP_PRESENCE: &str = "agent-desktop-presence";
const SOURCE_DRIVER_INVENTORY: &str = "driver-inventory";
const SOURCE_CONVERSATION_READINESS: &str = "conversation-readiness";

/// Facts whose state is read per participant from a live owner instead of the
/// packaged channel inventory. They are the ability and conversation-driver
/// facts every participant is asked about.
const DERIVED_CAPABILITY_FACTS: &[CapabilityFactName] = &[
    CapabilityFactName::Ability(NativeAbilityKind::ImageInput),
    CapabilityFactName::Ability(NativeAbilityKind::RealInterface),
    CapabilityFactName::DriverState(NativeDriverStateKind::ConversationDriverSupported),
    CapabilityFactName::DriverState(NativeDriverStateKind::ConversationDriverReady),
];

static NATIVE_CAPABILITY_REGISTRY: OnceLock<Option<BTreeMap<String, Vec<NativeCapabilityKind>>>> =
    OnceLock::new();
/// Live conversation-driver registry. Starts from the packaged embed and may be
/// hot-reloaded when verified readiness changes (gateway inventory control).
static LIVE_RUNTIME_DRIVER_REGISTRY: RwLock<Option<RuntimeDriverRegistry>> = RwLock::new(None);

pub fn runtime_driver_profile(target: &str) -> Option<RuntimeDriverProfile> {
    let adapter = adapter_for_agent(target)?;
    with_runtime_driver_registry(|registry| registry.profile(adapter.id()))?
}

fn ensure_runtime_driver_registry_loaded() {
    let Ok(guard) = LIVE_RUNTIME_DRIVER_REGISTRY.read() else {
        return;
    };
    if guard.is_some() {
        return;
    }
    drop(guard);
    let Ok(mut guard) = LIVE_RUNTIME_DRIVER_REGISTRY.write() else {
        return;
    };
    if guard.is_none() {
        *guard = parse_runtime_driver_registry(DRIVER_INVENTORY_JSON, READINESS_JSON).ok();
    }
}

fn with_runtime_driver_registry<T>(f: impl FnOnce(&RuntimeDriverRegistry) -> T) -> Option<T> {
    ensure_runtime_driver_registry_loaded();
    let guard = LIVE_RUNTIME_DRIVER_REGISTRY.read().ok()?;
    guard.as_ref().map(f)
}

/// Replace the live readiness projection from a full readiness document.
/// Driver inventory remains the packaged embed; only verified status is hot-swapped.
/// This is a partial reload: it does not touch Telegram bindings, conversation
/// sessions, or any other channel / lane state.
pub fn reload_conversation_readiness_document(
    readiness_json: &str,
) -> std::result::Result<(), &'static str> {
    let parsed = parse_runtime_driver_registry(DRIVER_INVENTORY_JSON, readiness_json)?;
    let mut guard = LIVE_RUNTIME_DRIVER_REGISTRY
        .write()
        .map_err(|_| "runtime_driver_registry_lock_failed")?;
    *guard = Some(parsed);
    Ok(())
}

/// Load a readiness overlay file when present (used by gateway sidecar start).
pub fn reload_conversation_readiness_from_path(
    path: &Path,
) -> std::result::Result<(), &'static str> {
    let bytes = std::fs::read(path).map_err(|_| "readiness_overlay_read_failed")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("readiness_overlay_too_large");
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "readiness_overlay_invalid_utf8")?;
    reload_conversation_readiness_document(text)
}

fn native_capability_registry() -> Option<&'static BTreeMap<String, Vec<NativeCapabilityKind>>> {
    NATIVE_CAPABILITY_REGISTRY
        .get_or_init(|| parse_native_capability_registry(NATIVE_CAPABILITY_JSON).ok())
        .as_ref()
}

/// Privacy-safe projection of every capability fact for one participant. Only
/// wire names, states and logical owner labels cross this boundary; callers
/// cannot mutate an inventory or infer local runtime details. This is the only
/// way a consumer learns what an Agent can do, and it returns one value per
/// fact so a consumer cannot collapse an unknown answer into an absent one.
pub fn native_capabilities_for_agent(agent_id: &str) -> Vec<CapabilityFact> {
    let adapter = adapter_for_agent(agent_id);
    let mut names = adapter
        .and_then(|adapter| native_capability_registry()?.get(adapter.id()))
        .map(|kinds| {
            kinds
                .iter()
                .copied()
                .map(CapabilityFactName::Channel)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.extend(DERIVED_CAPABILITY_FACTS.iter().copied());
    names
        .into_iter()
        .map(|name| project_capability_fact(adapter, name))
        .collect()
}

/// Read one projected capability fact for one participant. A requirement name
/// resolves through the same projection the participant list came from.
pub fn projected_capability_fact(agent_id: &str, name: CapabilityFactName) -> CapabilityFact {
    project_capability_fact(adapter_for_agent(agent_id), name)
}

/// The one derivation of a capability fact: every fact name maps to the single
/// owner that knows it, and that owner's answer becomes the projected state.
/// `None` means no owner covers the participant at all, which is unknown rather
/// than absent.
fn project_capability_fact(
    adapter: Option<RuntimeAdapter>,
    name: CapabilityFactName,
) -> CapabilityFact {
    let state = match adapter {
        Some(adapter) => capability_fact_state(adapter, name),
        None => CapabilityFactState::Unknown,
    };
    CapabilityFact {
        name: name.wire_name(),
        state,
        source: capability_fact_source(name),
    }
}

fn capability_fact_state(adapter: RuntimeAdapter, name: CapabilityFactName) -> CapabilityFactState {
    match name {
        // A desktop channel and a real-interface ability share one owner: the
        // desktop surface that is present on this host. The ability only
        // declares presence and never grants an attempt right.
        CapabilityFactName::Channel(NativeCapabilityKind::Desktop)
        | CapabilityFactName::Ability(NativeAbilityKind::RealInterface) => {
            desktop_presence_state(adapter)
        }
        CapabilityFactName::Channel(_) => cli_presence_state(adapter),
        CapabilityFactName::Ability(NativeAbilityKind::ImageInput) => {
            // Whether an Agent's own lane declares a capability is that
            // Agent's fact and the lane's, not this registry's, so it arrives
            // through the composition. A host that composed no lane answers
            // `Unknown` rather than inheriting a guess.
            let declared = super::port::composition().and_then(|composition| {
                (composition.declared_capability_flag)(adapter, "multimodal")
            });
            match declared {
                Some(declared) => CapabilityFactState::from_declaration(declared),
                None => CapabilityFactState::Unknown,
            }
        }
        CapabilityFactName::DriverState(kind) => conversation_driver_state(adapter, kind),
    }
}

fn capability_fact_source(name: CapabilityFactName) -> &'static str {
    match name {
        CapabilityFactName::Channel(NativeCapabilityKind::Desktop)
        | CapabilityFactName::Ability(NativeAbilityKind::RealInterface) => {
            SOURCE_AGENT_DESKTOP_PRESENCE
        }
        CapabilityFactName::Channel(_) => SOURCE_AGENT_CLI_PRESENCE,
        CapabilityFactName::Ability(NativeAbilityKind::ImageInput) => SOURCE_DRIVER_INVENTORY,
        CapabilityFactName::DriverState(_) => SOURCE_CONVERSATION_READINESS,
    }
}

/// The Agent target owner reads desktop application presence. An Agent the
/// target catalog does not define has no presence answer at all.
fn desktop_presence_state(adapter: RuntimeAdapter) -> CapabilityFactState {
    if licoup_agent_targets::domain::targets::target_def(adapter.id()).is_err() {
        return CapabilityFactState::Unknown;
    }
    CapabilityFactState::from_declaration(
        licoup_agent_targets::domain::targets::agent_desktop_app_detected(adapter.id()),
    )
}

/// The Agent target owner reads CLI/runtime executable presence. The protocol
/// and service lanes are capabilities of the runtime that CLI provides, so they
/// follow the same read.
fn cli_presence_state(adapter: RuntimeAdapter) -> CapabilityFactState {
    if licoup_agent_targets::domain::targets::target_def(adapter.id()).is_err() {
        return CapabilityFactState::Unknown;
    }
    CapabilityFactState::from_declaration(
        licoup_agent_targets::domain::targets::agent_cli_executable(adapter.id()).is_some(),
    )
}

/// The conversation-driver readiness owner reports whether the Agent's own lane
/// implements conversation and whether verified readiness reached `ready`.
fn conversation_driver_state(
    adapter: RuntimeAdapter,
    kind: NativeDriverStateKind,
) -> CapabilityFactState {
    let declared = with_runtime_driver_registry(|registry| match kind {
        NativeDriverStateKind::ConversationDriverSupported => registry
            .drivers
            .get(adapter.id())
            .map(|driver| driver_status_for_mode(&driver.driver_mode) == Some("implemented")),
        NativeDriverStateKind::ConversationDriverReady => registry
            .readiness
            .get(adapter.id())
            .map(|readiness| readiness.status == "ready"),
    })
    .flatten();
    match declared {
        Some(declared) => CapabilityFactState::from_declaration(declared),
        None => CapabilityFactState::Unknown,
    }
}

pub fn parse_native_capability_registry(
    inventory_json: &str,
) -> std::result::Result<BTreeMap<String, Vec<NativeCapabilityKind>>, &'static str> {
    let inventory: NativeCapabilityDocument = serde_json::from_str(inventory_json)
        .map_err(|_| "native_capability_inventory_parse_failed")?;
    if inventory.schema_version != NATIVE_CAPABILITY_SCHEMA_VERSION {
        return Err("native_capability_inventory_contract_invalid");
    }
    let expected_ids = PACKAGED_RUNTIME_ADAPTER_IDS
        .iter()
        .map(|id| (*id).to_string())
        .collect::<BTreeSet<_>>();
    let mut agents = BTreeMap::new();
    for entry in inventory.agents {
        if adapter_for_agent(&entry.agent_id).is_none() || entry.capabilities.is_empty() {
            return Err("native_capability_inventory_entry_invalid");
        }
        let capabilities = entry
            .capabilities
            .iter()
            .filter_map(|kind| NativeCapabilityKind::parse(kind))
            .collect::<Vec<_>>();
        if capabilities.len() != entry.capabilities.len()
            || capabilities
                .iter()
                .map(|kind| kind.wire_name())
                .collect::<BTreeSet<_>>()
                .len()
                != capabilities.len()
            || agents.insert(entry.agent_id, capabilities).is_some()
        {
            return Err("native_capability_inventory_entry_invalid");
        }
    }
    if agents.keys().cloned().collect::<BTreeSet<_>>() != expected_ids {
        return Err("native_capability_inventory_set_drift");
    }
    Ok(agents)
}

pub fn parse_runtime_driver_registry(
    inventory_json: &str,
    readiness_json: &str,
) -> std::result::Result<RuntimeDriverRegistry, &'static str> {
    let inventory: DriverInventoryDocument =
        serde_json::from_str(inventory_json).map_err(|_| "driver_inventory_parse_failed")?;
    let readiness: ReadinessDocument =
        serde_json::from_str(readiness_json).map_err(|_| "readiness_parse_failed")?;

    validate_driver_contract(&inventory)?;
    validate_readiness_document(&readiness)?;

    let expected_ids = PACKAGED_RUNTIME_ADAPTER_IDS
        .iter()
        .map(|id| (*id).to_string())
        .collect::<BTreeSet<_>>();
    let mut drivers = BTreeMap::new();
    for driver in inventory.drivers {
        validate_driver_entry(&driver)?;
        let agent_id = driver.agent_id.clone();
        if drivers.insert(agent_id, driver).is_some() {
            return Err("driver_inventory_duplicate_agent");
        }
    }
    let mut readiness_by_agent = BTreeMap::new();
    for entry in readiness.adapters {
        validate_readiness_entry(&entry)?;
        let agent_id = entry.agent_id.clone();
        if readiness_by_agent.insert(agent_id, entry).is_some() {
            return Err("readiness_duplicate_agent");
        }
    }

    let driver_ids = drivers.keys().cloned().collect::<BTreeSet<_>>();
    let readiness_ids = readiness_by_agent.keys().cloned().collect::<BTreeSet<_>>();
    if driver_ids != expected_ids || readiness_ids != expected_ids || driver_ids != readiness_ids {
        return Err("runtime_driver_registry_set_drift");
    }

    for agent_id in &expected_ids {
        let driver = drivers
            .get(agent_id)
            .ok_or("runtime_driver_registry_set_drift")?;
        let state = readiness_by_agent
            .get(agent_id)
            .ok_or("runtime_driver_registry_set_drift")?;
        match driver.driver_mode.as_str() {
            "blocked" if state.status != "blocked" => {
                return Err("runtime_driver_registry_state_drift");
            }
            "history-only" if state.status != "history-only" => {
                return Err("runtime_driver_registry_state_drift");
            }
            "conversation" if state.status == "history-only" => {
                return Err("runtime_driver_registry_state_drift");
            }
            _ => {}
        }
        if driver.driver_mode == "blocked"
            && !driver
                .blocker_codes
                .iter()
                .any(|code| state.summary_codes.contains(code))
        {
            return Err("runtime_driver_registry_blocker_drift");
        }
        if state.status != "ready"
            && !driver.blocker_codes.is_empty()
            && !driver
                .blocker_codes
                .iter()
                .any(|code| state.summary_codes.contains(code))
        {
            return Err("runtime_driver_registry_blocker_drift");
        }
        if let Some(binding) = state.evidence_binding.as_ref()
            && (binding.agent_id != driver.agent_id
                || binding.driver_id != driver.driver_id
                || binding.runtime_protocol != driver.runtime_protocol)
        {
            return Err("runtime_driver_registry_evidence_binding_drift");
        }
    }

    validate_readiness_summary(&readiness.summary, &readiness_by_agent)?;
    Ok(RuntimeDriverRegistry {
        drivers,
        readiness: readiness_by_agent,
    })
}

fn validate_driver_contract(
    inventory: &DriverInventoryDocument,
) -> std::result::Result<(), &'static str> {
    let contract = &inventory.evidence_contract;
    if inventory.schema_version != DRIVER_INVENTORY_SCHEMA_VERSION
        || inventory.contract_version != CONVERSATION_PARITY_CONTRACT_VERSION
        || contract.minimum_consecutive_passes != MINIMUM_CONSECUTIVE_PASSES
        || !strings_match(&contract.core_checks, CORE_CHECK_IDS)
        || !strings_match(&contract.conditional_checks, CONDITIONAL_CHECK_IDS)
        || !strings_match(&contract.required_booleans, REQUIRED_EVIDENCE_BOOLEANS)
        || !strings_match(&contract.required_counts, REQUIRED_EVIDENCE_COUNTS)
        || !strings_match(&contract.required_digests, REQUIRED_EVIDENCE_DIGESTS)
        || !strings_match(&contract.required_bindings, REQUIRED_EVIDENCE_BINDINGS)
    {
        return Err("driver_inventory_contract_invalid");
    }
    Ok(())
}

fn validate_driver_entry(driver: &DriverInventoryEntry) -> std::result::Result<(), &'static str> {
    let Some(adapter) = adapter_for_agent(&driver.agent_id) else {
        return Err("driver_inventory_unknown_agent");
    };
    let mode_valid = matches!(
        driver.driver_mode.as_str(),
        "conversation" | "blocked" | "history-only"
    );
    let blockers_valid = driver
        .blocker_codes
        .iter()
        .all(|code| is_sanitized_code(code));
    if adapter.id() != driver.agent_id
        || adapter.driver_id() != driver.driver_id
        || driver.runtime_protocol != adapter.runtime_protocol()
        || !is_sanitized_code(&driver.driver_id)
        || !is_sanitized_code(&driver.runtime_protocol)
        || !is_sanitized_code(&driver.official_native_lane_kind)
        || !mode_valid
        || !blockers_valid
        || (driver.driver_mode == "blocked" && driver.blocker_codes.is_empty())
        || (driver.driver_mode == "history-only" && !driver.blocker_codes.is_empty())
        || (driver.driver_mode != "blocked" && driver.official_native_lane_kind == "unavailable")
        || (driver.driver_mode == "history-only" && !driver.history_readable)
    {
        return Err("driver_inventory_entry_invalid");
    }
    Ok(())
}

fn validate_readiness_document(
    readiness: &ReadinessDocument,
) -> std::result::Result<(), &'static str> {
    if readiness.schema_version != READINESS_SCHEMA_VERSION
        || readiness.contract_version != CONVERSATION_PARITY_CONTRACT_VERSION
        || readiness.minimum_consecutive_passes != MINIMUM_CONSECUTIVE_PASSES
    {
        return Err("readiness_contract_invalid");
    }
    Ok(())
}

fn validate_readiness_entry(entry: &ReadinessEntry) -> std::result::Result<(), &'static str> {
    let status_valid = matches!(
        entry.status.as_str(),
        "ready" | "partial" | "failed" | "blocked" | "unverified" | "history-only"
    );
    let core = &entry.core_checks;
    let conditional = &entry.conditional_checks;
    let counts_valid = core.required == CORE_CHECK_IDS.len()
        && core.passed <= core.required
        && core.failed <= core.required
        && core.passed + core.failed <= core.required
        && conditional.total == CONDITIONAL_CHECK_IDS.len()
        && conditional.native_supported <= conditional.total
        && conditional.passed <= conditional.native_supported
        && conditional.gaps <= conditional.native_supported
        && conditional.failed <= conditional.native_supported
        && conditional.passed + conditional.gaps + conditional.failed
            <= conditional.native_supported;
    if !status_valid
        || entry.send_enabled != (entry.status == "ready")
        || entry.summary_codes.is_empty()
        || !entry
            .summary_codes
            .iter()
            .all(|code| is_sanitized_code(code))
        || !counts_valid
    {
        return Err("readiness_entry_invalid");
    }
    if entry.status == "ready"
        && (!entry.official_native_lane_proven
            || !entry.conversation_gate_passed
            || !entry.cleanup_passed
            || !entry.privacy_passed
            || entry.consecutive_passes < MINIMUM_CONSECUTIVE_PASSES
            || core.passed != core.required
            || core.failed != 0
            || conditional.passed != conditional.native_supported
            || conditional.gaps != 0
            || conditional.failed != 0
            || !entry
                .summary_codes
                .iter()
                .any(|code| code == "all_required_evidence_passed"))
    {
        return Err("readiness_ready_evidence_invalid");
    }
    if let Some(binding) = entry.evidence_binding.as_ref() {
        if !is_sanitized_code(&binding.driver_id)
            || !is_sanitized_code(&binding.runtime_protocol)
            || !is_sanitized_code(&binding.harness_version)
            || !is_sanitized_code(&binding.runtime_version_class)
            || !is_sanitized_code(&binding.runtime_source_class)
            || !is_sha256_digest(&binding.runtime_version_digest)
            || !is_sha256_digest(&binding.capability_snapshot_digest)
            || !is_sha256_digest(&binding.adapter_manifest_digest)
            || !is_sha256_digest(&binding.release_artifact_digest)
            || !is_sha256_digest(&binding.release_sidecar_digest)
            || !is_sha256_digest(&binding.product_continuity_binding_digest)
            || !is_sha256_digest(&binding.registry_digest)
            || !is_sha256_digest(&binding.driver_inventory_digest)
            || !is_sha256_digest(&binding.evidence_digest)
        {
            return Err("readiness_evidence_binding_invalid");
        }
    } else if entry.status == "ready" {
        return Err("readiness_ready_binding_missing");
    }
    Ok(())
}

fn validate_readiness_summary(
    summary: &ReadinessSummary,
    entries: &BTreeMap<String, ReadinessEntry>,
) -> std::result::Result<(), &'static str> {
    let count = |status: &str| {
        entries
            .values()
            .filter(|entry| entry.status == status)
            .count()
    };
    if summary.total != entries.len()
        || summary.ready != count("ready")
        || summary.partial != count("partial")
        || summary.failed != count("failed")
        || summary.blocked != count("blocked")
        || summary.unverified != count("unverified")
        || summary.history_only != count("history-only")
        || summary.send_enabled != entries.values().filter(|entry| entry.send_enabled).count()
    {
        return Err("readiness_summary_invalid");
    }
    Ok(())
}

fn strings_match(actual: &[String], expected: &[&str]) -> bool {
    actual
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied())
}

fn is_sanitized_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:+-".contains(&byte)
        })
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn driver_status_for_mode(mode: &str) -> Option<&'static str> {
    match mode {
        "conversation" => Some("implemented"),
        "blocked" => Some("blocked"),
        "history-only" => Some("history-only"),
        _ => None,
    }
}

impl RuntimeDriverRegistry {
    pub fn profile(&self, agent_id: &str) -> Option<RuntimeDriverProfile> {
        let driver = self.drivers.get(agent_id)?;
        let readiness = self.readiness.get(agent_id)?;
        let blocker = if readiness.status != "ready" {
            driver.blocker_codes.first().cloned()
        } else {
            None
        };
        let evidence_age_class = if readiness.evidence_binding.is_some() {
            "current".to_string()
        } else if readiness
            .summary_codes
            .iter()
            .any(|code| code == "evidence_stale_or_incomplete")
        {
            "stale".to_string()
        } else if readiness
            .summary_codes
            .iter()
            .any(|code| code == "evidence_missing" || code == "evidence_incomplete")
        {
            "missing".to_string()
        } else {
            "absent".to_string()
        };
        Some(RuntimeDriverProfile {
            driver_status: driver_status_for_mode(&driver.driver_mode)?.to_string(),
            readiness: readiness.status.clone(),
            protocol: driver.runtime_protocol.clone(),
            blocker,
            capability_matrix: driver.capability_matrix.clone(),
            summary_codes: readiness.summary_codes.clone(),
            consecutive_passes: readiness.consecutive_passes,
            evidence_age_class,
        })
    }
}

pub fn inventory_capability_matrix(agent_id: &str) -> Option<Value> {
    let adapter = adapter_for_agent(agent_id)?;
    with_runtime_driver_registry(|registry| {
        registry
            .drivers
            .get(adapter.id())
            .and_then(|entry| entry.capability_matrix.clone())
    })?
}

/// Build the bounded, client-facing adapter management catalog from the same
/// packaged registry used for runtime dispatch. Installation lifecycle is
/// exposed only for LicoUp-owned bridges; official native lanes and bundled
/// ACP clients never pretend to require installation into a vendor product.
/// Native capability kinds come from the canonical capability inventory and are
/// annotated with one live on-host evidence snapshot (matched pid, process
/// name, and port only).
pub fn adapter_management_catalog(antigravity_bridge_installed: bool) -> Value {
    if native_capability_registry().is_none() {
        return json!({
            "ok": false,
            "schemaVersion": "lico.adapter-plugin-catalog.v1",
            "adapters": [],
            "error": {"code": "adapter_native_capability_catalog_unavailable"},
        });
    }
    let live_snapshot = LiveSnapshot::capture();
    // Projected outside the registry read guard: the projection reads the same
    // readiness owner, and a nested read lock could deadlock against a pending
    // readiness reload.
    let capability_entries = PACKAGED_RUNTIME_ADAPTER_IDS
        .iter()
        .filter_map(|agent_id| {
            let adapter = adapter_for_agent(agent_id)?;
            Some((
                *agent_id,
                native_capability_catalog_entries(adapter, &live_snapshot),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let Some(adapters) = with_runtime_driver_registry(|registry| {
        PACKAGED_RUNTIME_ADAPTER_IDS
            .iter()
            .filter_map(|agent_id| {
                let adapter = adapter_for_agent(agent_id)?;
                let driver = registry.drivers.get(*agent_id)?;
                let readiness = registry.readiness.get(*agent_id)?;
                let lane_family = driver
                    .capability_matrix
                    .as_ref()
                    .and_then(|matrix| matrix.get("laneFamily"))
                    .and_then(Value::as_str)
                    .unwrap_or("unavailable");
                let managed_bridge = *agent_id == "antigravity";
                let management_kind = if managed_bridge {
                    "managed-bridge"
                } else if lane_family == "acp" {
                    "bundled-acp"
                } else {
                    "native"
                };
                let installation_state = if managed_bridge {
                    if antigravity_bridge_installed {
                        "installed"
                    } else {
                        "not-installed"
                    }
                } else {
                    "not-required"
                };
                let lifecycle_actions = if managed_bridge {
                    if antigravity_bridge_installed {
                        vec!["uninstall"]
                    } else {
                        vec!["install"]
                    }
                } else {
                    Vec::new()
                };
                let native_capabilities = capability_entries
                    .get(*agent_id)
                    .cloned()
                    .unwrap_or_default();
                let adapter_plugins = adapter_plugin_entries(
                    adapter,
                    &driver.runtime_protocol,
                    installation_state,
                    &lifecycle_actions,
                );
                Some(json!({
                    "agentId": adapter.id(),
                    "label": adapter.label(),
                    "driverId": driver.driver_id,
                    "runtimeProtocol": driver.runtime_protocol,
                    "laneFamily": lane_family,
                    "managementKind": management_kind,
                    "installationState": installation_state,
                    "readiness": readiness.status,
                    "lifecycleActions": lifecycle_actions,
                    "nativeCapabilities": native_capabilities,
                    "adapterPlugins": adapter_plugins,
                    "nativePreferred": true,
                }))
            })
            .collect::<Vec<_>>()
    }) else {
        return json!({
            "ok": false,
            "schemaVersion": "lico.adapter-plugin-catalog.v1",
            "adapters": [],
            "error": {"code": "adapter_plugin_catalog_unavailable"},
        });
    };

    json!({
        "ok": adapters.len() == PACKAGED_RUNTIME_ADAPTER_IDS.len(),
        "schemaVersion": "lico.adapter-plugin-catalog.v1",
        "adapters": adapters,
    })
}

/// Publish the declared channel and ability facts for one Agent, the way the
/// catalog has always published its delivery channels. The facts come from the
/// one participant projection, so the catalog holds no capability list of its
/// own. A fact reports `detected` only while its owner answered at all, and
/// reports no detection value while the answer is unknown rather than turning
/// an unreadable owner into a fabricated false.
pub fn native_capability_catalog_entries(
    adapter: RuntimeAdapter,
    live_snapshot: &LiveSnapshot,
) -> Vec<Value> {
    native_capabilities_for_agent(adapter.id())
        .into_iter()
        .filter_map(|fact| native_capability_catalog_entry(adapter, live_snapshot, &fact))
        .collect()
}

/// One catalog entry for one projected fact. A fact whose owner answered
/// reports its detection value; a fact whose state is unknown reports no
/// detection value at all, and the state travels with the entry so a consumer
/// can still tell an unreadable owner from a declared absence.
pub fn native_capability_catalog_entry(
    adapter: RuntimeAdapter,
    live_snapshot: &LiveSnapshot,
    fact: &CapabilityFact,
) -> Option<Value> {
    let live = match CapabilityFactName::parse(fact.name)? {
        CapabilityFactName::Channel(kind) => Some(live_snapshot.status(adapter, kind)),
        CapabilityFactName::Ability(_) => None,
        // Conversation-driver states are readiness facts, not native
        // capabilities; the catalog publishes readiness separately.
        CapabilityFactName::DriverState(_) => return None,
    };
    let mut entry = json!({
        "kind": fact.name,
        "state": fact.state.wire_name(),
        "running": live.as_ref().is_some_and(|live| live.running),
        "pid": live.as_ref().and_then(|live| live.pid),
        "processName": live.as_ref().and_then(|live| live.process_name.clone()),
        "port": live.as_ref().and_then(|live| live.port),
    });
    if fact.state != CapabilityFactState::Unknown {
        entry["detected"] = json!(fact.state == CapabilityFactState::Declared);
    }
    Some(entry)
}

/// Project the LicoUp-managed adapter plugin entries for one agent. Only
/// plugins with real install management appear here; native lanes and bundled
/// ACP clients are capabilities, not installed plugins.
fn adapter_plugin_entries(
    adapter: RuntimeAdapter,
    runtime_protocol: &str,
    installation_state: &str,
    lifecycle_actions: &[&str],
) -> Vec<Value> {
    match adapter.managed_adapter_plugin_id() {
        Some("acp-bridge") => vec![json!({
            "id": "acp-bridge",
            "label": "ACP Bridge",
            "detail": runtime_protocol,
            "installationState": installation_state,
            "lifecycleActions": lifecycle_actions,
        })],
        // A Codex caller integration is carried by a plugin in Codex's own
        // marketplace. This client installs none, so it advertises none.
        _ => Vec::new(),
    }
}
