//! The facts and effects the workflow runtime reads but does not own.
//!
//! Authority-backed Membership Profile snapshots are the first member:
//! persistent Profile intent lives in the conversation store, while everything
//! that changes over time (model price, intelligence score, Skill
//! availability, runtime environment and readiness) is derived per request
//! from its existing owner and cached only inside that request. The projection
//! allowlists opaque ids, enums, numbers and booleans; it never carries a
//! prompt, credential, absolute path, machine identity or runtime endpoint.
//!
//! The rest of the module is the port surface itself: the runtime declares
//! what it needs, and the kernel host that composes it supplies the
//! implementations once, at its crate root. Nothing here names a host module.
use anyhow::{Result, anyhow};
use licoup_conversation::{Membership, MembershipAccess, MembershipProfileSnapshot, ProfileIntent};
use licoup_workflow::{BindingValue, RunCommand, RuntimeKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// Bounded, privacy-safe target facts read from the Agent target owner.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetFacts {
    pub status: Option<String>,
    pub model: Option<String>,
    pub environment: Option<String>,
    pub capabilities: Vec<String>,
    pub readiness: Option<String>,
    pub reliability_class: Option<String>,
    pub latency_class: Option<u8>,
}

/// Allowlisted model price facts projected from the pricing owner. Input and
/// output prices are kept separate so the projection never invents a blended
/// single number that the owner does not expose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceFacts {
    pub input: f64,
    pub output: f64,
}

/// One read of one existing owner. Implementations must project allowlisted
/// facts only and are expected to be request-scoped (the caller reads each
/// owner at most once per request/revision).
pub trait ProfileSnapshotAuthority: Send {
    fn target_facts(&mut self, agent_id: &str) -> Option<TargetFacts>;
    fn model_price_usd_per_million_tokens(&mut self, model: &str) -> Option<PriceFacts>;
    fn coding_score(&mut self, agent_id: &str, model: &str) -> Option<i64>;
    fn skill_names(&mut self, agent_id: &str) -> Vec<String>;
}

pub type SharedSnapshotAuthority = Arc<Mutex<Box<dyn ProfileSnapshotAuthority>>>;

/// Request-scoped wrapper that reads each owner at most once per key. The
/// cache lives only for the duration of one projection call.
struct RequestScopedAuthority<'a> {
    inner: &'a mut dyn ProfileSnapshotAuthority,
    targets: BTreeMap<String, Option<TargetFacts>>,
    prices: BTreeMap<String, Option<PriceFacts>>,
    scores: BTreeMap<(String, String), Option<i64>>,
    skills: BTreeMap<String, Vec<String>>,
}

impl<'a> RequestScopedAuthority<'a> {
    fn new(inner: &'a mut dyn ProfileSnapshotAuthority) -> Self {
        Self {
            inner,
            targets: BTreeMap::new(),
            prices: BTreeMap::new(),
            scores: BTreeMap::new(),
            skills: BTreeMap::new(),
        }
    }

    fn target_facts(&mut self, agent_id: &str) -> Option<TargetFacts> {
        if let Some(cached) = self.targets.get(agent_id) {
            return cached.clone();
        }
        let read = self.inner.target_facts(agent_id);
        self.targets.insert(agent_id.to_owned(), read.clone());
        read
    }

    fn model_price(&mut self, model: &str) -> Option<PriceFacts> {
        if let Some(cached) = self.prices.get(model) {
            return *cached;
        }
        let read = self.inner.model_price_usd_per_million_tokens(model);
        self.prices.insert(model.to_owned(), read);
        read
    }

    fn score(&mut self, agent_id: &str, model: &str) -> Option<i64> {
        let key = (agent_id.to_owned(), model.to_owned());
        if let Some(cached) = self.scores.get(&key) {
            return *cached;
        }
        let read = self.inner.coding_score(agent_id, model);
        self.scores.insert(key, read);
        read
    }

    fn skills(&mut self, agent_id: &str) -> Vec<String> {
        if let Some(cached) = self.skills.get(agent_id) {
            return cached.clone();
        }
        let read = self.inner.skill_names(agent_id);
        self.skills.insert(agent_id.to_owned(), read.clone());
        read
    }
}

/// Derive one Membership Profile snapshot from persistent intent plus the
/// existing owners. `authority` is locked for one projection and each owner is
/// read at most once per request through the request-scoped cache.
pub fn project_profile_snapshot(
    conversation_id: &str,
    membership: &Membership,
    intent: &ProfileIntent,
    is_assistant: bool,
    authority: &SharedSnapshotAuthority,
    model_facts: &dyn ModelFactsPort,
) -> MembershipProfileSnapshot {
    let mut guard = authority
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut scoped = RequestScopedAuthority::new(guard.as_mut());
    project_with(
        &mut scoped,
        conversation_id,
        membership,
        intent,
        is_assistant,
        model_facts,
    )
}

/// Project one or more Membership Profiles in one request while sharing one
/// request-scoped authority cache, so each owner is read at most once per
/// request/revision regardless of how many Memberships are projected.
pub fn project_profile_snapshots(
    conversation_id: &str,
    members: &[(Membership, ProfileIntent, bool)],
    authority: &SharedSnapshotAuthority,
    model_facts: &dyn ModelFactsPort,
) -> Vec<MembershipProfileSnapshot> {
    let mut guard = authority
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut scoped = RequestScopedAuthority::new(guard.as_mut());
    members
        .iter()
        .map(|(membership, intent, is_assistant)| {
            project_with(
                &mut scoped,
                conversation_id,
                membership,
                intent,
                *is_assistant,
                model_facts,
            )
        })
        .collect()
}

/// Hard constraints applied before any ordering. Every required fact must be
/// present and equal/contained; a missing membership binding is a hard
/// failure, never a silent drop.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateFilters {
    #[serde(default)]
    pub required_authority: Vec<String>,
    #[serde(default)]
    pub required_skills: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_readiness: Option<String>,
    #[serde(default)]
    pub membership_ids: Vec<String>,
    #[serde(default)]
    pub pinned_membership_ids: Vec<String>,
    #[serde(default)]
    pub preferred_skills: Vec<String>,
    #[serde(default)]
    pub preferred_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_task: Option<String>,
}

/// Apply hard filters, then order by the stable lexicographic tuple frozen by
/// Decision 0004: explicit pin, preference match, verified reliability,
/// coding score, known expected price, observed latency and Membership id.
/// Unknown optional facts remain unknown and sort after known facts.
pub fn rank_candidates(
    snapshots: Vec<MembershipProfileSnapshot>,
    filters: &CandidateFilters,
) -> Result<Vec<MembershipProfileSnapshot>, String> {
    let required_ids = filters.membership_ids.iter().collect::<BTreeSet<_>>();
    let available = snapshots
        .iter()
        .map(|snapshot| &snapshot.membership_id)
        .collect::<BTreeSet<_>>();
    if !required_ids.is_subset(&available) {
        return Err("profile_candidate_rejected".to_owned());
    }
    let mut eligible = snapshots
        .into_iter()
        .filter(|snapshot| {
            (required_ids.is_empty() || required_ids.contains(&snapshot.membership_id))
                && candidate_eligible(snapshot, filters)
        })
        .collect::<Vec<_>>();
    let remaining = eligible
        .iter()
        .map(|snapshot| &snapshot.membership_id)
        .collect::<BTreeSet<_>>();
    if !required_ids.is_subset(&remaining) {
        return Err("profile_candidate_rejected".to_owned());
    }
    let pin_order = filters
        .pinned_membership_ids
        .iter()
        .chain(filters.membership_ids.iter())
        .enumerate()
        .fold(BTreeMap::new(), |mut result, (ordinal, membership_id)| {
            result.entry(membership_id.as_str()).or_insert(ordinal);
            result
        });
    eligible.sort_by(|left, right| {
        pin_order
            .get(left.membership_id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(
                &pin_order
                    .get(right.membership_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
            )
            .then_with(|| preference_misses(left, filters).cmp(&preference_misses(right, filters)))
            .then_with(|| reliability_rank(left).cmp(&reliability_rank(right)))
            .then_with(|| optional_desc(left.intelligence_score, right.intelligence_score))
            .then_with(|| optional_price(left).cmp(&optional_price(right)))
            .then_with(|| optional_asc(left.latency_class, right.latency_class))
            .then_with(|| left.membership_id.cmp(&right.membership_id))
    });
    Ok(eligible)
}

fn candidate_eligible(snapshot: &MembershipProfileSnapshot, filters: &CandidateFilters) -> bool {
    snapshot
        .required_capabilities
        .iter()
        .all(|required| snapshot.capabilities.iter().any(|value| value == required))
        && snapshot
            .skill_references
            .iter()
            .all(|required| snapshot.skills.iter().any(|value| value == required))
        && filters
            .required_authority
            .iter()
            .all(|required| snapshot.authority.iter().any(|value| value == required))
        && filters
            .required_skills
            .iter()
            .all(|required| snapshot.skills.iter().any(|value| value == required))
        && filters
            .required_capabilities
            .iter()
            .all(|required| snapshot.capabilities.iter().any(|value| value == required))
        && filters
            .required_model
            .as_deref()
            .map(|required| snapshot.model.as_deref() == Some(required))
            .unwrap_or(true)
        && filters
            .required_environment
            .as_deref()
            .map(|required| snapshot.environment.as_deref() == Some(required))
            .unwrap_or(true)
        && filters
            .required_readiness
            .as_deref()
            .map(|required| snapshot.readiness.as_deref() == Some(required))
            .unwrap_or(true)
        && filters
            .required_task
            .as_deref()
            .map(|required| snapshot.task_tags.iter().any(|tag| tag == required))
            .unwrap_or(true)
}

fn preference_misses(snapshot: &MembershipProfileSnapshot, filters: &CandidateFilters) -> usize {
    let model = filters
        .preferred_model
        .as_deref()
        .or(snapshot.preferred_model.as_deref());
    let environment = filters
        .preferred_environment
        .as_deref()
        .or(snapshot.preferred_environment.as_deref());
    usize::from(model.is_some_and(|value| snapshot.model.as_deref() != Some(value)))
        + usize::from(
            environment.is_some_and(|value| snapshot.environment.as_deref() != Some(value)),
        )
        + filters
            .preferred_skills
            .iter()
            .filter(|value| !snapshot.skills.contains(value))
            .count()
        + filters
            .preferred_capabilities
            .iter()
            .chain(snapshot.preferred_capabilities.iter())
            .filter(|value| !snapshot.capabilities.contains(value))
            .count()
        + usize::from(
            filters
                .preferred_task
                .as_deref()
                .is_some_and(|task| !snapshot.task_tags.iter().any(|tag| tag == task)),
        )
}

fn reliability_rank(snapshot: &MembershipProfileSnapshot) -> (bool, u8) {
    let Some(value) = snapshot.reliability_class.as_deref() else {
        return (true, u8::MAX);
    };
    let rank = match value {
        "verified" | "high" | "ready" => 0,
        "standard" | "partial" => 1,
        "low" | "unverified" => 2,
        _ => 3,
    };
    (false, rank)
}

fn optional_desc(left: Option<i64>, right: Option<i64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn optional_asc<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn optional_price(snapshot: &MembershipProfileSnapshot) -> (bool, u64) {
    let Some(input) = snapshot.price_input_usd_per_million_tokens else {
        return (true, u64::MAX);
    };
    let Some(output) = snapshot.price_output_usd_per_million_tokens else {
        return (true, u64::MAX);
    };
    if !input.is_finite() || !output.is_finite() || input < 0.0 || output < 0.0 {
        return (true, u64::MAX);
    }
    (false, ((input + output) * 1_000_000.0).round() as u64)
}

fn project_with(
    authority: &mut RequestScopedAuthority<'_>,
    conversation_id: &str,
    membership: &Membership,
    intent: &ProfileIntent,
    is_assistant: bool,
    model_facts: &dyn ModelFactsPort,
) -> MembershipProfileSnapshot {
    let agent_id = membership
        .principal
        .agent_id
        .clone()
        .unwrap_or_else(|| membership.principal.id.clone());
    let target = authority.target_facts(&agent_id);
    let model = target.as_ref().and_then(|facts| facts.model.clone());
    let price = model
        .as_deref()
        .and_then(|model| authority.model_price(model));
    let score = model
        .as_deref()
        .and_then(|model| authority.score(&agent_id, model));
    let mut capabilities = target
        .as_ref()
        .map(|facts| facts.capabilities.clone())
        .unwrap_or_default();
    capabilities.sort();
    capabilities.dedup();
    let mut skills = authority.skills(&agent_id);
    if is_assistant
        && let Some(bundled) = model_facts.bundled_guide_skill_id()
        && intent.skill_references.iter().any(|skill| skill == bundled)
        && !skills.iter().any(|skill| skill == bundled)
    {
        skills.push(bundled.to_owned());
    }
    skills.sort();
    skills.dedup();
    let authority = match membership.access {
        MembershipAccess::Owner => vec![
            "conversation.act".to_owned(),
            "conversation.manage".to_owned(),
            "conversation.read".to_owned(),
        ],
        MembershipAccess::Member => vec![
            "conversation.act".to_owned(),
            "conversation.read".to_owned(),
        ],
    };
    let task_tags = model
        .as_deref()
        .map(|model| model_facts.task_tags_for_model(model))
        .unwrap_or_default();
    MembershipProfileSnapshot {
        conversation_id: conversation_id.to_owned(),
        membership_id: membership.id.clone(),
        agent_id,
        intent_revision: intent.revision,
        responsibility: intent.responsibility,
        required_capabilities: intent.required_capabilities.clone(),
        preferred_capabilities: intent.preferred_capabilities.clone(),
        skill_references: intent.skill_references.clone(),
        preferred_model: intent.preferred_model.clone(),
        preferred_reasoning_effort: intent.preferred_reasoning_effort.clone(),
        preferred_environment: intent.preferred_environment.clone(),
        preferred_permission_mode: intent.preferred_permission_mode.clone(),
        model,
        capabilities,
        skills,
        environment: target.as_ref().and_then(|facts| facts.environment.clone()),
        readiness: target.as_ref().and_then(|facts| facts.readiness.clone()),
        price_input_usd_per_million_tokens: price.map(|price| price.input),
        price_output_usd_per_million_tokens: price.map(|price| price.output),
        intelligence_score: score,
        task_tags,
        reliability_class: target
            .as_ref()
            .and_then(|facts| facts.reliability_class.clone()),
        latency_class: target.as_ref().and_then(|facts| facts.latency_class),
        authority,
    }
}

// ------------------------------------------------------------------- ports

/// The allowlisted model facts the runtime may publish about one binding.
///
/// The model registry, its catalogue and its curated intelligence snapshot stay
/// with the host that owns them; the runtime only asks for the two projections
/// a route receipt and a definition listing already carried.
pub trait ModelFactsPort: Send + Sync {
    /// Human-facing display name for a model id, or the id itself.
    fn model_display_name(&self, model: &str) -> String;
    /// The allowlisted intelligence projection for a model id, when the
    /// catalogue carries one. Unknown models stay absent rather than invented.
    fn project_allowlisted_model(&self, model: &str) -> Option<Value>;
    /// The curated task tags the catalogue carries for a model id.
    fn task_tags_for_model(&self, model: &str) -> Vec<String>;
    /// The Skill id of the product guide this host bundles, when it bundles a
    /// non-empty one. A host that ships no guide answers `None` rather than
    /// claiming a Skill it cannot serve.
    fn bundled_guide_skill_id(&self) -> Option<&'static str>;
}

// The numeric usage ledger, the resolved-runtime handle, the effect seam, the
// effect permit and the turn-dispatch error are the engine's port vocabulary;
// they arrive unchanged so host-side implementors bind the same contract.
pub use acativa::runtime::ports::{
    ActorTurnError, EffectPort, ResolvedRuntime, StrategyEffectPermit, UsageLedgerPort,
};

/// One committed route receipt for a candidate decision.
///
/// The receipt names the sources it was derived from and the ranked
/// Memberships, so a replay can compare the decision instead of re-deriving
/// it.
///
/// The selection policy is one of those sources, and its revision is the one
/// the caller captured for this decision rather than whatever is current when
/// the receipt is read: a durable admission keeps the revision it was admitted
/// under, so a later adoption governs only the next task.
pub fn route_receipt(
    conversation_id: &str,
    snapshots: &[MembershipProfileSnapshot],
    model_facts: &dyn ModelFactsPort,
    selection_policy_revision: &str,
) -> Value {
    json!({
        "conversationId": conversation_id,
        "sourceRevisions": [
            {"source": "selectionPolicy", "revision": selection_policy_revision},
            {"source": "targets", "revision": "read-only-v1"},
            {"source": "nativeCapabilities", "revision": "v0.0.1"},
            {"source": "providerModelPricing", "revision": "catalog-v1"},
            {"source": "agentIntelligenceCatalog", "revision": "catalog-v1"},
            {"source": "skillHub", "revision": "request-snapshot-v1"},
            {"source": "assistantWorkflowAuthoringBundle", "revision": "v1"},
        ],
        "rankedMembershipIds": snapshots
            .iter()
            .map(|snapshot| snapshot.membership_id.clone())
            .collect::<Vec<_>>(),
        "candidates": snapshots.iter().map(|snapshot| json!({
            "membershipId": snapshot.membership_id,
            "profileRevision": snapshot.intent_revision,
            "responsibility": snapshot.responsibility,
            "model": snapshot.model,
            "capabilities": snapshot.capabilities,
            "skills": snapshot.skills,
            "environment": snapshot.environment,
            "readiness": snapshot.readiness,
            "inputPriceUsdPerMillionTokens": snapshot.price_input_usd_per_million_tokens,
            "outputPriceUsdPerMillionTokens": snapshot.price_output_usd_per_million_tokens,
            "codingScore": snapshot.intelligence_score,
            "taskTags": snapshot.task_tags,
            "intelligence": snapshot.model.as_deref().and_then(|model| model_facts.project_allowlisted_model(model)),
            "reliabilityClass": snapshot.reliability_class,
            "latencyClass": snapshot.latency_class,
            "authority": snapshot.authority,
        })).collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------- host composition

/// Everything the runtime reads from the host that composes it.
///
/// The host installs one of these per process at its crate root. A process
/// that installs nothing keeps every port fail-closed: no effect runs, no fact
/// is invented, and the ledger records nothing.
#[derive(Clone)]
pub struct HostPorts {
    /// The request-scoped Membership Profile authority.
    pub profile: SharedSnapshotAuthority,
    /// The allowlisted model facts.
    pub model: Arc<dyn ModelFactsPort>,
    /// The numeric Graph usage ledger.
    pub usage: Arc<dyn UsageLedgerPort>,
    /// The effect seam.
    pub effect: Arc<dyn EffectPort>,
}

impl HostPorts {
    /// The composition a process has when it installed none: every read is
    /// absent and every effect is refused, so a missing installation is a
    /// typed refusal rather than invented work.
    pub fn unavailable() -> Self {
        Self {
            profile: Arc::new(Mutex::new(Box::new(UnavailableProfileAuthority))),
            model: Arc::new(UnavailableModelFacts),
            usage: Arc::new(UnavailableUsageLedger),
            effect: Arc::new(UnavailableEffect),
        }
    }
}

impl std::fmt::Debug for HostPorts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HostPorts").finish_non_exhaustive()
    }
}

static HOST_PORTS: OnceLock<HostPorts> = OnceLock::new();

/// Install the process-wide composition. The first installation wins; a later
/// one is ignored, so a host that composes the same answers twice stays one
/// composition.
pub fn install_host_ports(ports: HostPorts) {
    let _ = HOST_PORTS.set(ports);
}

/// The installed composition, or the refusing default.
pub fn host_ports() -> &'static HostPorts {
    HOST_PORTS.get_or_init(HostPorts::unavailable)
}

struct UnavailableProfileAuthority;

impl ProfileSnapshotAuthority for UnavailableProfileAuthority {
    fn target_facts(&mut self, _agent_id: &str) -> Option<TargetFacts> {
        None
    }

    fn model_price_usd_per_million_tokens(&mut self, _model: &str) -> Option<PriceFacts> {
        None
    }

    fn coding_score(&mut self, _agent_id: &str, _model: &str) -> Option<i64> {
        None
    }

    fn skill_names(&mut self, _agent_id: &str) -> Vec<String> {
        Vec::new()
    }
}

struct UnavailableModelFacts;

impl ModelFactsPort for UnavailableModelFacts {
    fn model_display_name(&self, model: &str) -> String {
        // Display typography is a pure naming projection, not a catalogue
        // fact. The host answers the same function; an uninstalled catalogue
        // still formats the id instead of publishing it raw.
        licoup_foundation::core::model_naming::model_display_name(model)
    }

    fn project_allowlisted_model(&self, _model: &str) -> Option<Value> {
        None
    }

    fn task_tags_for_model(&self, _model: &str) -> Vec<String> {
        Vec::new()
    }

    fn bundled_guide_skill_id(&self) -> Option<&'static str> {
        None
    }
}

struct UnavailableUsageLedger;

impl UsageLedgerPort for UnavailableUsageLedger {
    fn begin_graph_run(&self, _payload: &Value) -> Result<()> {
        Ok(())
    }

    fn record_graph_command(&self, _payload: &Value) -> Result<()> {
        Ok(())
    }

    fn workflow_report(&self, _payload: &Value) -> Result<Value> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }
}

struct UnavailableEffect;

impl EffectPort for UnavailableEffect {
    fn runtime_descriptors(&self) -> Vec<Value> {
        Vec::new()
    }

    fn compatible_runtime_id(
        &self,
        _kind: RuntimeKind,
        _version_requirement: &str,
    ) -> Option<String> {
        None
    }

    fn resolve_runtime(
        &self,
        _runtime_id: &str,
        _kind: RuntimeKind,
        _version_requirement: &str,
    ) -> Result<Arc<dyn ResolvedRuntime>> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn actor_fingerprint(
        &self,
        _value_id: &str,
        _model: &str,
        _reasoning_effort: &str,
    ) -> Result<String> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn actor_capabilities(&self, _value_id: &str) -> Result<Value> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn admit_strategy_cwd(&self, _cwd: &str) -> Result<()> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn execute_script(
        &self,
        _command: &RunCommand,
        _authorization_digest: &str,
        _runtime: &Arc<dyn ResolvedRuntime>,
        _revision_content: &Path,
        _runtime_state_root: &Path,
        _permit: &mut StrategyEffectPermit,
    ) -> Result<Value> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn execute_actor(
        &self,
        _command: &RunCommand,
        _authorization_digest: &str,
        _binding: &BindingValue,
        _permit: &mut StrategyEffectPermit,
        _cwd: Option<&str>,
    ) -> Result<Value> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn predecessor_locator(&self, _facts: &Value) -> Value {
        Value::Null
    }

    fn dispatch_lane_operation(&self, _operation: &str, _params: &Value) -> Result<Value> {
        Err(anyhow!("workflow_host_port_unavailable"))
    }

    fn new_correlation_id(&self) -> String {
        String::new()
    }

    fn record_run_stop(
        &self,
        _portable_root: &Path,
        _correlation_id: &str,
        _confirmed: bool,
    ) -> Result<()> {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use licoup_conversation::{
        LICOUP_GUIDE_SKILL_ID, MembershipAccess, MembershipStatus, Principal, PrincipalKind,
        ProfileResponsibility,
    };

    #[test]
    fn request_scoped_authority_reads_each_owner_once() {
        let calls = Arc::new(Mutex::new(BTreeMap::<&'static str, usize>::new()));
        let mut inner = CountingAuthority {
            calls: Arc::clone(&calls),
        };
        let mut scoped = RequestScopedAuthority::new(&mut inner);
        for _ in 0..3 {
            assert_eq!(
                scoped.target_facts("agent:one").unwrap().status.as_deref(),
                Some("ready")
            );
            assert_eq!(
                scoped.model_price("model-a"),
                Some(PriceFacts {
                    input: 1.0,
                    output: 2.0
                })
            );
            assert_eq!(scoped.score("agent:one", "model-a"), Some(2));
            assert_eq!(scoped.skills("agent:one"), vec!["skill-a".to_owned()]);
        }
        let recorded = calls.lock().unwrap();
        assert_eq!(
            recorded.clone(),
            BTreeMap::from([
                ("target_facts", 1),
                ("model_price", 1),
                ("score", 1),
                ("skills", 1),
            ])
        );
    }

    #[test]
    fn intent_cannot_assert_derived_truth_or_authority() {
        let calls = Arc::new(Mutex::new(BTreeMap::<&'static str, usize>::new()));
        let authority: SharedSnapshotAuthority =
            Arc::new(Mutex::new(Box::new(CountingAuthority {
                calls: Arc::clone(&calls),
            })));
        let membership = membership("membership:one", MembershipAccess::Member);
        let intent = ProfileIntent {
            revision: 3,
            required_capabilities: vec!["caller-asserted-capability".to_owned()],
            preferred_capabilities: vec!["caller-preference".to_owned()],
            skill_references: vec![
                "caller-asserted-skill".to_owned(),
                LICOUP_GUIDE_SKILL_ID.to_owned(),
            ],
            preferred_model: Some("caller-model".to_owned()),
            preferred_reasoning_effort: Some("high".to_owned()),
            preferred_environment: Some("caller-environment".to_owned()),
            preferred_permission_mode: Some("caller-permission-mode".to_owned()),
            preferred_instructions: Some("caller-instructions".to_owned()),
            preferred_context_window: Some(128_000),
            responsibility: ProfileResponsibility::Assistant,
            updated_at_unix_ms: 9,
        };
        let snapshot = project_profile_snapshot(
            "conversation:g",
            &membership,
            &intent,
            true,
            &authority,
            &UnavailableModelFacts,
        );
        assert_eq!(snapshot.membership_id, "membership:one");
        assert_eq!(snapshot.intent_revision, 3);
        assert_eq!(snapshot.model.as_deref(), Some("target-model"));
        assert_eq!(snapshot.price_input_usd_per_million_tokens, Some(1.0));
        assert_eq!(snapshot.price_output_usd_per_million_tokens, Some(2.0));
        assert_eq!(snapshot.intelligence_score, Some(2));
        assert!(
            !snapshot
                .capabilities
                .contains(&"caller-asserted-capability".to_owned())
        );
        assert_eq!(
            snapshot.capabilities,
            vec!["conversationDriver:supported".to_owned()]
        );
        assert!(snapshot.skills.contains(&"skill-a".to_owned()));
        assert!(
            !snapshot
                .skills
                .contains(&"caller-asserted-skill".to_owned())
        );
        // A skill reference in the intent is not a bundled Skill. The guide is
        // present only when this host says it ships that Skill.
        assert!(!snapshot.skills.contains(&LICOUP_GUIDE_SKILL_ID.to_owned()));
        let bundled = project_profile_snapshot(
            "conversation:g",
            &membership,
            &intent,
            true,
            &authority,
            &BundledGuideFacts,
        );
        assert!(bundled.skills.contains(&"skill-a".to_owned()));
        assert!(!bundled.skills.contains(&"caller-asserted-skill".to_owned()));
        assert!(bundled.skills.contains(&LICOUP_GUIDE_SKILL_ID.to_owned()));
        assert_eq!(snapshot.environment.as_deref(), Some("local"));
        assert_eq!(snapshot.readiness.as_deref(), Some("ready"));
        assert_eq!(
            snapshot.authority,
            vec![
                "conversation.act".to_owned(),
                "conversation.read".to_owned()
            ]
        );
        let encoded = serde_json::to_value(&snapshot).unwrap();
        assert!(encoded.get("id").is_none());
        assert!(encoded.get("displayName").is_none());
        assert!(encoded.get("updatedAtUnixMs").is_none());
    }

    #[test]
    fn hard_filters_precede_the_stable_lexicographic_order() {
        let mut first = snapshot("membership:b");
        first.reliability_class = Some("verified".to_owned());
        first.intelligence_score = Some(8);
        first.price_input_usd_per_million_tokens = Some(4.0);
        first.price_output_usd_per_million_tokens = Some(8.0);
        first.latency_class = Some(2);
        let mut pinned = snapshot("membership:c");
        pinned.reliability_class = None;
        pinned.intelligence_score = None;
        let mut cheap = snapshot("membership:a");
        cheap.reliability_class = Some("verified".to_owned());
        cheap.intelligence_score = Some(8);
        cheap.price_input_usd_per_million_tokens = Some(1.0);
        cheap.price_output_usd_per_million_tokens = Some(2.0);
        cheap.latency_class = Some(1);
        let ranked = rank_candidates(
            vec![first, pinned, cheap],
            &CandidateFilters {
                required_capabilities: vec!["conversationDriver:supported".to_owned()],
                pinned_membership_ids: vec!["membership:c".to_owned()],
                ..CandidateFilters::default()
            },
        )
        .unwrap();
        assert_eq!(
            ranked
                .iter()
                .map(|candidate| candidate.membership_id.as_str())
                .collect::<Vec<_>>(),
            vec!["membership:c", "membership:a", "membership:b"]
        );
        let rejected = rank_candidates(
            ranked,
            &CandidateFilters {
                membership_ids: vec!["membership:c".to_owned()],
                required_skills: vec!["missing".to_owned()],
                ..CandidateFilters::default()
            },
        );
        assert_eq!(rejected.unwrap_err(), "profile_candidate_rejected");
    }

    #[test]
    fn task_filters_boost_matching_catalog_tags() {
        let mut frontend = snapshot("membership:frontend");
        frontend.task_tags = vec!["frontend".to_owned()];
        frontend.intelligence_score = Some(4);
        let mut backend = snapshot("membership:backend");
        backend.task_tags = vec!["backend".to_owned()];
        backend.intelligence_score = Some(9);
        let ranked = rank_candidates(
            vec![frontend.clone(), backend.clone()],
            &CandidateFilters {
                preferred_task: Some("frontend".to_owned()),
                ..CandidateFilters::default()
            },
        )
        .unwrap();
        assert_eq!(ranked[0].membership_id, "membership:frontend");
        let required = rank_candidates(
            vec![frontend, backend],
            &CandidateFilters {
                required_task: Some("frontend".to_owned()),
                ..CandidateFilters::default()
            },
        )
        .unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].membership_id, "membership:frontend");
    }

    fn membership(id: &str, access: MembershipAccess) -> Membership {
        Membership {
            id: id.to_owned(),
            conversation_id: "conversation:g".to_owned(),
            principal: Principal {
                id: "agent:one".to_owned(),
                kind: PrincipalKind::Agent,
                display_name: "One".to_owned(),
                agent_id: Some("agent:one".to_owned()),
                created_at_unix_ms: 1,
            },
            access,
            status: MembershipStatus::Active,
            joined_at_unix_ms: 1,
            left_at_unix_ms: None,
        }
    }

    fn snapshot(id: &str) -> MembershipProfileSnapshot {
        MembershipProfileSnapshot {
            conversation_id: "conversation:g".to_owned(),
            membership_id: id.to_owned(),
            agent_id: "agent:one".to_owned(),
            intent_revision: 1,
            responsibility: ProfileResponsibility::Member,
            required_capabilities: Vec::new(),
            preferred_capabilities: Vec::new(),
            skill_references: Vec::new(),
            preferred_model: None,
            preferred_reasoning_effort: None,
            preferred_environment: None,
            preferred_permission_mode: None,
            model: Some("model-a".to_owned()),
            capabilities: vec!["conversationDriver:supported".to_owned()],
            skills: vec!["skill-a".to_owned()],
            environment: Some("local".to_owned()),
            readiness: Some("ready".to_owned()),
            price_input_usd_per_million_tokens: None,
            price_output_usd_per_million_tokens: None,
            intelligence_score: None,
            task_tags: Vec::new(),
            reliability_class: None,
            latency_class: None,
            authority: vec!["conversation.act".to_owned()],
        }
    }

    struct CountingAuthority {
        calls: Arc<Mutex<BTreeMap<&'static str, usize>>>,
    }

    impl ProfileSnapshotAuthority for CountingAuthority {
        fn target_facts(&mut self, _agent_id: &str) -> Option<TargetFacts> {
            *self
                .calls
                .lock()
                .unwrap()
                .entry("target_facts")
                .or_insert(0) += 1;
            Some(TargetFacts {
                status: Some("ready".to_owned()),
                model: Some("target-model".to_owned()),
                environment: Some("local".to_owned()),
                capabilities: vec!["conversationDriver:supported".to_owned()],
                readiness: Some("ready".to_owned()),
                reliability_class: Some("verified".to_owned()),
                latency_class: Some(1),
            })
        }

        fn model_price_usd_per_million_tokens(&mut self, _model: &str) -> Option<PriceFacts> {
            *self.calls.lock().unwrap().entry("model_price").or_insert(0) += 1;
            Some(PriceFacts {
                input: 1.0,
                output: 2.0,
            })
        }

        fn coding_score(&mut self, _agent_id: &str, _model: &str) -> Option<i64> {
            *self.calls.lock().unwrap().entry("score").or_insert(0) += 1;
            Some(2)
        }

        fn skill_names(&mut self, _agent_id: &str) -> Vec<String> {
            *self.calls.lock().unwrap().entry("skills").or_insert(0) += 1;
            vec!["skill-a".to_owned()]
        }
    }

    struct BundledGuideFacts;

    impl ModelFactsPort for BundledGuideFacts {
        fn model_display_name(&self, model: &str) -> String {
            model.to_owned()
        }

        fn project_allowlisted_model(&self, _model: &str) -> Option<Value> {
            None
        }

        fn task_tags_for_model(&self, _model: &str) -> Vec<String> {
            Vec::new()
        }

        fn bundled_guide_skill_id(&self) -> Option<&'static str> {
            Some(LICOUP_GUIDE_SKILL_ID)
        }
    }
}
#[cfg(test)]
mod port_stability {
    //! Stability of the port surface itself.
    //!
    //! The host answers these ports from outside this crate, so the surface is
    //! a published contract: a renamed or retyped method, a different field, a
    //! changed contract constant or a dropped re-export is a breaking change
    //! that has to fail here rather than at the far end of the composition.
    //!
    //! Two independent mechanisms cover the surface. The declarations are read
    //! back from this module's own source, so an added, removed or retyped port
    //! item fails as a difference against the frozen record below. The witness
    //! implementations, the `HostPorts` construction and the receipt/filter
    //! projections pin the same items to the compiler and to the wire format,
    //! so a signature cannot drift while the record is edited to follow it.
    use super::*;
    use licoup_conversation::ProfileResponsibility;

    const PORT_SOURCE: &str = include_str!("ports.rs");
    const CRATE_ROOT: &str = include_str!("lib.rs");

    /// Every public item this module declares, as `kind name`.
    ///
    /// Removing a name is as much a surface change as adding one. The engine's
    /// shared port vocabulary — `EffectPort`, `UsageLedgerPort`,
    /// `ResolvedRuntime`, `StrategyEffectPermit`, `ActorTurnError` — is
    /// declared once in `acativa::runtime::ports` and re-exported here, so the
    /// record below names only what this file itself publishes.
    const PUBLISHED_PORT_ITEMS: &[&str] = &[
        "fn host_ports",
        "fn install_host_ports",
        "fn project_profile_snapshot",
        "fn project_profile_snapshots",
        "fn rank_candidates",
        "fn route_receipt",
        "struct CandidateFilters",
        "struct HostPorts",
        "struct PriceFacts",
        "struct TargetFacts",
        "trait ModelFactsPort",
        "trait ProfileSnapshotAuthority",
        "type SharedSnapshotAuthority",
    ];

    /// The items that deliberately stay internal to this module. They carry no
    /// visibility of their own, so the scanner never sees them as declarations
    /// and no re-export is owed for them.
    const UNPUBLISHED_PORT_ITEMS: &[&str] = &["RequestScopedAuthority"];

    /// Every trait method, in declaration order, for the traits this file
    /// itself declares. The engine-owned traits keep their own frozen record
    /// in `acativa`'s port module.
    const FROZEN_TRAIT_METHODS: &[(&str, &[&str])] = &[
        (
            "ModelFactsPort",
            &[
                "model_display_name",
                "project_allowlisted_model",
                "task_tags_for_model",
                "bundled_guide_skill_id",
            ],
        ),
        (
            "ProfileSnapshotAuthority",
            &[
                "target_facts",
                "model_price_usd_per_million_tokens",
                "coding_score",
                "skill_names",
            ],
        ),
    ];

    /// Every struct field, in declaration order.
    const FROZEN_STRUCT_FIELDS: &[(&str, &[&str])] = &[
        (
            "CandidateFilters",
            &[
                "required_authority",
                "required_skills",
                "required_capabilities",
                "required_model",
                "required_environment",
                "required_readiness",
                "membership_ids",
                "pinned_membership_ids",
                "preferred_skills",
                "preferred_capabilities",
                "preferred_model",
                "preferred_environment",
                "required_task",
                "preferred_task",
            ],
        ),
        ("HostPorts", &["profile", "model", "usage", "effect"]),
        ("PriceFacts", &["input", "output"]),
        (
            "TargetFacts",
            &[
                "status",
                "model",
                "environment",
                "capabilities",
                "readiness",
                "reliability_class",
                "latency_class",
            ],
        ),
    ];

    /// Every port item the crate root re-exports, and nothing else.
    const ROOT_REEXPORTED_PORT_ITEMS: &[&str] = &[
        "ActorTurnError",
        "CandidateFilters",
        "EffectPort",
        "HostPorts",
        "ModelFactsPort",
        "PriceFacts",
        "ProfileSnapshotAuthority",
        "ResolvedRuntime",
        "SharedSnapshotAuthority",
        "StrategyEffectPermit",
        "TargetFacts",
        "UsageLedgerPort",
        "host_ports",
        "install_host_ports",
        "project_profile_snapshot",
        "project_profile_snapshots",
        "rank_candidates",
        "route_receipt",
    ];

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct PortItem {
        kind: String,
        name: String,
    }

    impl PortItem {
        fn render(&self) -> String {
            format!("{} {}", self.kind, self.name)
        }
    }

    /// One declaration of the port module: who it is and what it contains.
    #[derive(Clone, Debug)]
    struct PortDeclaration {
        item: PortItem,
        members: Vec<String>,
    }

    /// Read the port declarations back from this module's own source.
    ///
    /// Documentation, formatting and phrasing stay free: comments are stripped
    /// and whitespace is collapsed, while every keyword, name and type is
    /// compared. A declaration is the structural line at column zero together
    /// with the indented lines that state its signature; a trait or struct
    /// additionally keeps the members one level inside its own block.
    fn scan_port_declarations(source: &str) -> Vec<PortDeclaration> {
        let lines = structural_lines(source);
        let mut declarations = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            if lines[index].indent != 0 {
                index += 1;
                continue;
            }
            let Some(item) = port_item(&lines[index].text, false) else {
                index += 1;
                continue;
            };
            let opened = index;
            signature_end(&lines, &mut index);
            let members = member_names(&lines, opened, index);
            declarations.push(PortDeclaration { item, members });
        }
        declarations
    }

    /// The names one level inside a declaration's own block.
    ///
    /// Only the direct members count: an `impl` block, a nested module or a
    /// member's own body belong to implementation, not to the port surface.
    fn member_names(lines: &[StructuralLine], opened: usize, end: usize) -> Vec<String> {
        if !lines[opened].text.ends_with('{') || end <= opened + 1 {
            return Vec::new();
        }
        let Some(body_indent) = lines[opened + 1..end]
            .iter()
            .find(|line| line.indent > lines[opened].indent)
            .map(|line| line.indent)
        else {
            return Vec::new();
        };
        let mut members = Vec::new();
        for line in &lines[opened + 1..end] {
            if line.indent != body_indent {
                continue;
            }
            if let Some(member) = port_item(&line.text, true)
                .map(|item| item.name)
                .or_else(|| struct_field(&line.text))
            {
                members.push(member);
            }
        }
        members
    }

    struct StructuralLine {
        indent: usize,
        text: String,
    }

    /// The source without comments and blank lines, keeping text and indent.
    fn structural_lines(source: &str) -> Vec<StructuralLine> {
        let mut lines = Vec::new();
        let mut in_block_comment = false;
        for raw in source.lines() {
            let mut text = String::new();
            let mut quotes = 0usize;
            let mut characters = raw.chars().peekable();
            while let Some(character) = characters.next() {
                if in_block_comment {
                    if character == '*' && characters.peek() == Some(&'/') {
                        characters.next();
                        in_block_comment = false;
                    }
                    continue;
                }
                if character == '"' {
                    quotes = 1 - quotes;
                } else if quotes == 0 && character == '/' && characters.peek() == Some(&'/') {
                    break;
                } else if quotes == 0 && character == '/' && characters.peek() == Some(&'*') {
                    characters.next();
                    in_block_comment = true;
                    continue;
                }
                text.push(character);
            }
            if !text.trim().is_empty() {
                lines.push(StructuralLine {
                    indent: text.len() - text.trim_start().len(),
                    text: text.trim().to_owned(),
                });
            }
        }
        lines
    }

    /// The declaration a structural line states, when it is in the port
    /// surface. At module level the item has to be published; inside a trait
    /// the members carry no visibility of their own.
    fn port_item(text: &str, nested: bool) -> Option<PortItem> {
        if !text.starts_with("pub ") && !nested {
            return None;
        }
        if !text.starts_with("pub ")
            && !text.starts_with("fn ")
            && !text.starts_with("type ")
            && !text.starts_with("const ")
            && !text.starts_with("static ")
        {
            return None;
        }
        let declaration = declaration_part(text)?;
        let keyword = declaration.split_whitespace().next()?;
        let kind = match keyword {
            "fn" => "fn",
            "type" => "type",
            "struct" => "struct",
            "enum" => "enum",
            "trait" => "trait",
            "mod" => "mod",
            "const" => "const",
            "static" => "static",
            _ => return None,
        };
        let name = declaration[keyword.len()..]
            .trim_start()
            .split(|character: char| !(character.is_alphanumeric() || character == '_'))
            .next()
            .filter(|name| !name.is_empty())?;
        Some(PortItem {
            kind: kind.to_owned(),
            name: name.to_owned(),
        })
    }

    /// A struct field written as `pub name: Type`.
    fn struct_field(text: &str) -> Option<String> {
        let declaration = text.strip_prefix("pub ")?.split(';').next()?;
        let (name, _) = declaration.split_once(':')?;
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_owned())
    }

    /// The declaration without its visibility marker and without any body.
    fn declaration_part(text: &str) -> Option<String> {
        let declaration = text.strip_prefix("pub ").unwrap_or(text);
        if let Some(opening) = declaration.rfind('{') {
            return Some(declaration[..opening].trim().to_owned());
        }
        let declaration = declaration.split(';').next()?.trim();
        (!declaration.is_empty()).then(|| declaration.to_owned())
    }

    /// Step past the signature lines of one declaration.
    fn signature_end(lines: &[StructuralLine], index: &mut usize) -> usize {
        let mut depth = 0i64;
        let mut opened_block = false;
        while *index < lines.len() {
            let text = lines[*index].text.clone();
            *index += 1;
            depth += balance(&text);
            if depth > 0 {
                opened_block = true;
            }
            if (opened_block && depth == 0) || (!opened_block && text.ends_with(';')) {
                break;
            }
        }
        *index
    }

    fn balance(text: &str) -> i64 {
        text.chars().fold(0, |depth, character| match character {
            '{' => depth + 1,
            '}' => depth - 1,
            _ => depth,
        })
    }

    fn frozen_members<'a>(frozen: &'a [(&'a str, &'a [&'a str])]) -> BTreeSet<&'a str> {
        frozen.iter().map(|(name, _)| *name).collect()
    }

    /// Compare the record against the declarations in both directions: a
    /// record entry that no longer exists and a declaration the record does
    /// not publish are each a contract change.
    fn contract_differences(declared: &[PortItem], frozen: &[&str]) -> Vec<String> {
        let declared = declared
            .iter()
            .map(PortItem::render)
            .collect::<BTreeSet<_>>();
        let frozen = frozen
            .iter()
            .map(|item| (*item).to_owned())
            .collect::<BTreeSet<_>>();
        let mut differences = declared
            .difference(&frozen)
            .map(|item| format!("{item} is declared but not published in the contract"))
            .collect::<Vec<_>>();
        differences.extend(
            frozen
                .difference(&declared)
                .map(|item| format!("{item} is published but no longer declared")),
        );
        differences
    }

    fn member_differences(
        declarations: &[PortDeclaration],
        kind: &str,
        frozen: &[(&str, &[&str])],
    ) -> Vec<String> {
        let declared = declarations
            .iter()
            .filter(|declaration| declaration.item.kind == kind)
            .map(|declaration| (declaration.item.name.clone(), declaration.members.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut differences = Vec::new();
        for (name, expected) in frozen {
            match declared.get(*name) {
                None => differences.push(format!("{kind} {name} is no longer declared")),
                Some(actual) if actual != expected => differences.push(format!(
                    "{kind} {name} changed\n  published: {expected:?}\n  declared:  {actual:?}",
                )),
                Some(_) => {}
            }
        }
        for (name, members) in &declared {
            // A type whose members are private publishes no member set of its
            // own; its public methods are pinned by the signature witnesses.
            if members.is_empty() {
                continue;
            }
            if !frozen_members(frozen).contains(name.as_str()) {
                differences.push(format!(
                    "{kind} {name} is not part of the published contract"
                ));
            }
        }
        differences
    }

    /// Render one member record the way it is written in this module, so a
    /// deliberate surface change can be recorded by copying the printed form
    /// instead of transcribing names by hand.
    fn render_member_record(declarations: &[PortDeclaration], kind: &str) -> String {
        let mut rendered = declarations
            .iter()
            .filter(|declaration| declaration.item.kind == kind && !declaration.members.is_empty())
            .map(|declaration| {
                let members = declaration
                    .members
                    .iter()
                    .map(|member| format!("\"{member}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("        (\"{}\", &[{}]),\n", declaration.item.name, members,)
            })
            .collect::<Vec<_>>();
        rendered.sort();
        rendered.concat()
    }

    fn render_item_list(declarations: &[PortDeclaration], kind: &str) -> String {
        let mut rendered = declarations
            .iter()
            .filter(|declaration| declaration.item.kind == kind)
            .map(|declaration| format!("        \"{}\",\n", declaration.item.render()))
            .collect::<Vec<_>>();
        rendered.sort();
        rendered.concat()
    }

    /// Regenerate the frozen records above from the current declarations.
    ///
    /// Run `cargo test --lib port_stability::print -- --nocapture` and copy the
    /// printed arrays over the constants. This is the only supported way to
    /// change the record: a reviewer sees the exact difference the change
    /// makes to the published surface.
    #[test]
    fn print_the_frozen_records() {
        let declarations = scan_port_declarations(PORT_SOURCE);
        println!("PUBLISHED_PORT_ITEMS = [");
        print!("{}", render_item_list(&declarations, "fn"));
        print!("{}", render_item_list(&declarations, "struct"));
        print!("{}", render_item_list(&declarations, "trait"));
        print!("{}", render_item_list(&declarations, "type"));
        println!("]");
        println!(
            "FROZEN_TRAIT_METHODS = [\n{}]",
            render_member_record(&declarations, "trait"),
        );
        println!(
            "FROZEN_STRUCT_FIELDS = [\n{}]",
            render_member_record(&declarations, "struct"),
        );
    }

    /// One candidate for the wire-contract checks, written out in full so the
    /// projection under test reads the same data a route would produce.
    fn stability_snapshot(id: &str) -> MembershipProfileSnapshot {
        MembershipProfileSnapshot {
            conversation_id: "conversation:g".to_owned(),
            membership_id: id.to_owned(),
            agent_id: "agent:one".to_owned(),
            intent_revision: 1,
            responsibility: ProfileResponsibility::Member,
            required_capabilities: Vec::new(),
            preferred_capabilities: Vec::new(),
            skill_references: Vec::new(),
            preferred_model: None,
            preferred_reasoning_effort: None,
            preferred_environment: None,
            preferred_permission_mode: None,
            model: Some("model-a".to_owned()),
            capabilities: vec!["conversationDriver:supported".to_owned()],
            skills: vec!["skill-a".to_owned()],
            environment: Some("local".to_owned()),
            readiness: Some("ready".to_owned()),
            price_input_usd_per_million_tokens: None,
            price_output_usd_per_million_tokens: None,
            intelligence_score: None,
            task_tags: Vec::new(),
            reliability_class: None,
            latency_class: None,
            authority: vec!["conversation.act".to_owned()],
        }
    }

    /// The port surface as this module declares it: the public items, the
    /// methods of every port trait and the fields of every port struct, each
    /// compared in both directions against the published record.
    #[test]
    fn port_surface_declarations_are_frozen() {
        let declarations = scan_port_declarations(PORT_SOURCE);
        let declared = declarations
            .iter()
            .map(|declaration| declaration.item.clone())
            .collect::<Vec<_>>();
        let mut differences = contract_differences(&declared, PUBLISHED_PORT_ITEMS);
        differences.extend(UNPUBLISHED_PORT_ITEMS.iter().flat_map(|item| {
            let declared = PORT_SOURCE.contains(&format!("struct {item}"));
            let published = PORT_SOURCE.contains(&format!("pub struct {item}"));
            let mut found = Vec::new();
            if !declared {
                found.push(format!("{item} is listed as internal but is not declared"));
            }
            if published {
                found.push(format!("{item} is listed as internal but is published"));
            }
            found
        }));
        differences.extend(member_differences(
            &declarations,
            "trait",
            FROZEN_TRAIT_METHODS,
        ));
        differences.extend(member_differences(
            &declarations,
            "struct",
            FROZEN_STRUCT_FIELDS,
        ));
        assert!(
            differences.is_empty(),
            "the workflow port contract changed; update the host composition and the published record together:\n{}",
            differences.join("\n"),
        );
        assert_eq!(
            declarations.len(),
            PUBLISHED_PORT_ITEMS.len(),
            "the set of declarations this module owns changed",
        );
    }

    /// The compiler-side pin: each port trait keeps exactly these methods with
    /// exactly these signatures. A renamed, retyped, added or removed method
    /// stops this module from compiling.
    ///
    /// The witness calls name the concrete type that implements the trait and
    /// take `&dyn` only afterwards: Rust cannot coerce an unselected trait
    /// method item to a `fn` pointer over `dyn Trait`, and selecting it beside
    /// the methods above is exactly the signature check this test exists for.
    #[test]
    fn port_trait_signatures_are_frozen() {
        let _: fn(&mut UnavailableProfileAuthority, &str) -> Option<TargetFacts> =
            UnavailableProfileAuthority::target_facts;
        let _: fn(&mut UnavailableProfileAuthority, &str) -> Option<PriceFacts> =
            UnavailableProfileAuthority::model_price_usd_per_million_tokens;
        let _: fn(&mut UnavailableProfileAuthority, &str, &str) -> Option<i64> =
            UnavailableProfileAuthority::coding_score;
        let _: fn(&mut UnavailableProfileAuthority, &str) -> Vec<String> =
            UnavailableProfileAuthority::skill_names;
        let _: fn(&mut dyn ProfileSnapshotAuthority, &str, &str) -> Option<i64> =
            |authority, agent_id, model| {
                (authority as &mut dyn ProfileSnapshotAuthority).coding_score(agent_id, model)
            };

        let _: fn(&UnavailableModelFacts, &str) -> String =
            UnavailableModelFacts::model_display_name;
        let _: fn(&UnavailableModelFacts, &str) -> Option<Value> =
            UnavailableModelFacts::project_allowlisted_model;
        let _: fn(&UnavailableModelFacts, &str) -> Vec<String> =
            UnavailableModelFacts::task_tags_for_model;
        let _: fn(&UnavailableModelFacts) -> Option<&'static str> =
            UnavailableModelFacts::bundled_guide_skill_id;
        let _: fn(&dyn ModelFactsPort, &str) -> String =
            |port, model| (port as &dyn ModelFactsPort).model_display_name(model);

        let _: fn(&UnavailableUsageLedger, &Value) -> Result<()> =
            UnavailableUsageLedger::begin_graph_run;
        let _: fn(&UnavailableUsageLedger, &Value) -> Result<()> =
            UnavailableUsageLedger::record_graph_command;
        let _: fn(&UnavailableUsageLedger, &Value) -> Result<Value> =
            UnavailableUsageLedger::workflow_report;
        let _: fn(&dyn UsageLedgerPort, &Value) -> Result<()> =
            |port, payload| (port as &dyn UsageLedgerPort).begin_graph_run(payload);

        struct RuntimeWitness;
        impl ResolvedRuntime for RuntimeWitness {
            fn fingerprint(&self) -> &str {
                "port-signature-witness"
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
        let _: for<'a> fn(&'a RuntimeWitness) -> &'a str = RuntimeWitness::fingerprint;
        let _: for<'a> fn(&'a RuntimeWitness) -> &'a dyn std::any::Any = RuntimeWitness::as_any;
        let _: for<'a> fn(&'a dyn ResolvedRuntime) -> &'a str =
            |runtime| (runtime as &dyn ResolvedRuntime).fingerprint();
        let _: for<'a> fn(&'a dyn ResolvedRuntime) -> &'a dyn std::any::Any =
            |runtime| (runtime as &dyn ResolvedRuntime).as_any();

        let _: fn(&UnavailableEffect) -> Vec<Value> = UnavailableEffect::runtime_descriptors;
        let _: fn(&UnavailableEffect, RuntimeKind, &str) -> Option<String> =
            UnavailableEffect::compatible_runtime_id;
        let _: fn(&UnavailableEffect, &str, RuntimeKind, &str) -> Result<Arc<dyn ResolvedRuntime>> =
            UnavailableEffect::resolve_runtime;
        let _: fn(&UnavailableEffect, &str, &str, &str) -> Result<String> =
            UnavailableEffect::actor_fingerprint;
        let _: fn(&UnavailableEffect, &str) -> Result<()> = UnavailableEffect::admit_strategy_cwd;
        let _: fn(
            &UnavailableEffect,
            &RunCommand,
            &str,
            &Arc<dyn ResolvedRuntime>,
            &Path,
            &Path,
            &mut StrategyEffectPermit,
        ) -> Result<Value> = UnavailableEffect::execute_script;
        let _: fn(
            &UnavailableEffect,
            &RunCommand,
            &str,
            &BindingValue,
            &mut StrategyEffectPermit,
            Option<&str>,
        ) -> Result<Value> = UnavailableEffect::execute_actor;
        let _: fn(&UnavailableEffect, &Value) -> Value = UnavailableEffect::predecessor_locator;
        let _: fn(&UnavailableEffect, &str, &Value) -> Result<Value> =
            UnavailableEffect::dispatch_lane_operation;
        let _: fn(&UnavailableEffect) -> String = UnavailableEffect::new_correlation_id;
        let _: fn(&UnavailableEffect, &Path, &str, bool) -> Result<()> =
            UnavailableEffect::record_run_stop;
        let _: fn(&dyn EffectPort, RuntimeKind, &str) -> Option<String> =
            |port, kind, requirement| {
                (port as &dyn EffectPort).compatible_runtime_id(kind, requirement)
            };

        let error = ActorTurnError::dispatch_failed("port-signature-witness");
        let _: &str = error.detail();
        assert_eq!(
            error.to_string(),
            "port-signature-witness",
            "the port error keeps displaying its owner's detail",
        );
        let _: fn(&mut StrategyEffectPermit, &RunCommand, &str, &str) -> Result<()> =
            StrategyEffectPermit::consume;
        let _: fn(&str, &str, &str) -> Result<StrategyEffectPermit> = StrategyEffectPermit::issue;
    }

    /// The compiler-side pin for the port data: every field, in the exact
    /// shape the host fills. A retyped or renamed field stops this compiling.
    #[test]
    fn port_struct_fields_are_frozen() {
        let target_facts = TargetFacts {
            status: Some("ready".to_owned()),
            model: Some("model-a".to_owned()),
            environment: Some("local".to_owned()),
            capabilities: vec!["conversationDriver:supported".to_owned()],
            readiness: Some("ready".to_owned()),
            reliability_class: Some("verified".to_owned()),
            latency_class: Some(1),
        };
        let price_facts = PriceFacts {
            input: 1.0,
            output: 2.0,
        };
        let filters = CandidateFilters {
            required_authority: vec!["conversation.read".to_owned()],
            required_skills: vec!["skill-a".to_owned()],
            required_capabilities: vec!["conversationDriver:supported".to_owned()],
            required_model: Some("model-a".to_owned()),
            required_environment: Some("local".to_owned()),
            required_readiness: Some("ready".to_owned()),
            membership_ids: vec!["membership:one".to_owned()],
            pinned_membership_ids: Vec::new(),
            preferred_skills: Vec::new(),
            preferred_capabilities: Vec::new(),
            preferred_model: None,
            preferred_environment: None,
            required_task: Some("frontend".to_owned()),
            preferred_task: None,
        };
        let failure = ActorTurnError::dispatch_failed("port-field-witness");
        let ports = HostPorts {
            profile: Arc::new(Mutex::new(Box::new(UnavailableProfileAuthority))),
            model: Arc::new(UnavailableModelFacts),
            usage: Arc::new(UnavailableUsageLedger),
            effect: Arc::new(UnavailableEffect),
        };
        let snapshots =
            project_profile_snapshots("conversation:g", &[], &ports.profile, &*ports.model);
        let receipt = route_receipt("conversation:g", &snapshots, &*ports.model, "policy-v1");
        let permit = StrategyEffectPermit::issue(
            "command",
            "port-stability-authorization-digest-with-exactly-64-characters-x",
            "port-stability-effect-fingerprint-with-exactly-64-characters-len",
        )
        .expect("a complete permit issues");
        assert!(receipt.is_object(), "the receipt keeps its document shape");
        assert_eq!(filters.clone(), filters, "the filter data stays comparable");
        assert_eq!(price_facts.input + price_facts.output, 3.0);
        assert_eq!(target_facts.latency_class, Some(1));
        assert_eq!(failure.detail(), "port-field-witness");
        assert!(format!("{permit:?}").starts_with("StrategyEffectPermit"));
        assert_eq!(
            ports
                .profile
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .coding_score("agent:one", "model-a"),
            None,
        );
        assert_eq!(
            HostPorts::unavailable()
                .usage
                .workflow_report(&Value::Null)
                .unwrap_err()
                .to_string(),
            "workflow_host_port_unavailable",
            "the uninstalled composition stays fail-closed",
        );
    }

    /// The published member contract: the host answers these ports from
    /// outside this crate, so each trait stays object-safe, keeps the
    /// `Send`/`Send + Sync` bound the composition relies on, and keeps every
    /// method the host implements.
    #[test]
    fn port_traits_keep_their_published_bounds_and_members() {
        fn object_safe<T: ?Sized>() {}
        fn sendable<T: Send + ?Sized>() {}
        fn shareable<T: Send + Sync + ?Sized>() {}

        object_safe::<dyn ProfileSnapshotAuthority>();
        object_safe::<dyn ModelFactsPort>();
        object_safe::<dyn UsageLedgerPort>();
        object_safe::<dyn ResolvedRuntime>();
        object_safe::<dyn EffectPort>();
        sendable::<dyn ProfileSnapshotAuthority>();
        shareable::<dyn ModelFactsPort>();
        shareable::<dyn UsageLedgerPort>();
        shareable::<dyn ResolvedRuntime>();
        shareable::<dyn EffectPort>();

        struct WitnessProfileAuthority;
        struct WitnessModelFacts;
        struct WitnessUsageLedger;
        struct WitnessResolvedRuntime;
        struct WitnessEffect;

        impl ProfileSnapshotAuthority for WitnessProfileAuthority {
            fn target_facts(&mut self, _agent_id: &str) -> Option<TargetFacts> {
                None
            }

            fn model_price_usd_per_million_tokens(&mut self, _model: &str) -> Option<PriceFacts> {
                None
            }

            fn coding_score(&mut self, _agent_id: &str, _model: &str) -> Option<i64> {
                None
            }

            fn skill_names(&mut self, _agent_id: &str) -> Vec<String> {
                Vec::new()
            }
        }

        impl ModelFactsPort for WitnessModelFacts {
            fn model_display_name(&self, model: &str) -> String {
                model.to_owned()
            }

            fn project_allowlisted_model(&self, _model: &str) -> Option<Value> {
                None
            }

            fn task_tags_for_model(&self, _model: &str) -> Vec<String> {
                Vec::new()
            }

            fn bundled_guide_skill_id(&self) -> Option<&'static str> {
                None
            }
        }

        impl UsageLedgerPort for WitnessUsageLedger {
            fn begin_graph_run(&self, _payload: &Value) -> Result<()> {
                Ok(())
            }

            fn record_graph_command(&self, _payload: &Value) -> Result<()> {
                Ok(())
            }

            fn workflow_report(&self, _payload: &Value) -> Result<Value> {
                Ok(Value::Null)
            }
        }

        impl ResolvedRuntime for WitnessResolvedRuntime {
            fn fingerprint(&self) -> &str {
                "witness"
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        impl EffectPort for WitnessEffect {
            fn runtime_descriptors(&self) -> Vec<Value> {
                Vec::new()
            }

            fn compatible_runtime_id(
                &self,
                _kind: RuntimeKind,
                _version_requirement: &str,
            ) -> Option<String> {
                None
            }

            fn resolve_runtime(
                &self,
                _runtime_id: &str,
                _kind: RuntimeKind,
                _version_requirement: &str,
            ) -> Result<Arc<dyn ResolvedRuntime>> {
                Ok(Arc::new(WitnessResolvedRuntime))
            }

            fn actor_fingerprint(
                &self,
                _value_id: &str,
                _model: &str,
                _reasoning_effort: &str,
            ) -> Result<String> {
                Ok("witness".to_owned())
            }

            fn actor_capabilities(&self, _value_id: &str) -> Result<Value> {
                Ok(json!({ "ok": true }))
            }

            fn admit_strategy_cwd(&self, _cwd: &str) -> Result<()> {
                Ok(())
            }

            fn execute_script(
                &self,
                _command: &RunCommand,
                _authorization_digest: &str,
                _runtime: &Arc<dyn ResolvedRuntime>,
                _revision_content: &Path,
                _runtime_state_root: &Path,
                _permit: &mut StrategyEffectPermit,
            ) -> Result<Value> {
                Ok(Value::Null)
            }

            fn execute_actor(
                &self,
                _command: &RunCommand,
                _authorization_digest: &str,
                _binding: &BindingValue,
                _permit: &mut StrategyEffectPermit,
                _cwd: Option<&str>,
            ) -> Result<Value> {
                Ok(Value::Null)
            }

            fn predecessor_locator(&self, _facts: &Value) -> Value {
                Value::Null
            }

            fn dispatch_lane_operation(&self, _operation: &str, _params: &Value) -> Result<Value> {
                Ok(Value::Null)
            }

            fn new_correlation_id(&self) -> String {
                String::new()
            }

            fn record_run_stop(
                &self,
                _portable_root: &Path,
                _correlation_id: &str,
                _confirmed: bool,
            ) -> Result<()> {
                Ok(())
            }
        }

        fn assert_host_ports_shape<T: Send + Sync + 'static>() {}
        assert_host_ports_shape::<HostPorts>();
        let ports = HostPorts {
            profile: Arc::new(Mutex::new(Box::new(WitnessProfileAuthority))),
            model: Arc::new(WitnessModelFacts),
            usage: Arc::new(WitnessUsageLedger),
            effect: Arc::new(WitnessEffect),
        };
        assert!(ports.effect.runtime_descriptors().is_empty());
        assert!(
            ports
                .effect
                .resolve_runtime("witness", RuntimeKind::new("node"), ">=0")
                .is_ok(),
        );
        drop(
            ports
                .profile
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
    }

    /// The contract constants the runtime publishes into its wire documents.
    /// The host and a replay both read them, so a changed projection fails
    /// here as a value difference.
    #[test]
    fn port_wire_contracts_are_frozen() {
        let filters = CandidateFilters {
            required_authority: vec!["conversation.read".to_owned()],
            required_skills: vec!["skill-a".to_owned()],
            required_capabilities: vec!["conversationDriver:supported".to_owned()],
            required_model: Some("model-a".to_owned()),
            required_environment: Some("local".to_owned()),
            required_readiness: Some("ready".to_owned()),
            membership_ids: vec!["membership:one".to_owned()],
            pinned_membership_ids: vec!["membership:two".to_owned()],
            preferred_skills: Vec::new(),
            preferred_capabilities: Vec::new(),
            preferred_model: None,
            preferred_environment: None,
            required_task: Some("frontend".to_owned()),
            preferred_task: None,
        };
        let encoded = serde_json::to_value(&filters).expect("candidate filters serialize");
        let keys = encoded
            .as_object()
            .expect("candidate filters serialize as an object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "membershipIds".to_owned(),
                "pinnedMembershipIds".to_owned(),
                "preferredCapabilities".to_owned(),
                "preferredSkills".to_owned(),
                "requiredAuthority".to_owned(),
                "requiredCapabilities".to_owned(),
                "requiredEnvironment".to_owned(),
                "requiredModel".to_owned(),
                "requiredReadiness".to_owned(),
                "requiredSkills".to_owned(),
                "requiredTask".to_owned(),
            ]),
            "the candidate-filter wire contract changed",
        );
        assert!(
            !keys.contains("preferredModel")
                && !keys.contains("preferredEnvironment")
                && !keys.contains("preferredTask"),
            "absent preferences stay absent rather than serialize as null",
        );

        let mut candidate = stability_snapshot("membership:one");
        candidate.capabilities = Vec::new();
        candidate.skills = Vec::new();
        candidate.authority = Vec::new();
        let receipt = route_receipt(
            "conversation:g",
            &[candidate],
            &UnavailableModelFacts,
            "policy-v1",
        );
        let object = receipt.as_object().expect("the route receipt is an object");
        assert_eq!(
            object.keys().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "candidates".to_owned(),
                "conversationId".to_owned(),
                "rankedMembershipIds".to_owned(),
                "sourceRevisions".to_owned(),
            ]),
            "the route-receipt wire contract changed",
        );
        assert_eq!(
            receipt.get("conversationId").and_then(Value::as_str),
            Some("conversation:g"),
        );
        assert_eq!(
            receipt
                .get("rankedMembershipIds")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
        );
        let revisions = receipt
            .get("sourceRevisions")
            .and_then(Value::as_array)
            .expect("the source revisions are a list")
            .iter()
            .map(|entry| {
                (
                    entry
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    entry
                        .get("revision")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            revisions,
            vec![
                ("selectionPolicy", "policy-v1"),
                ("targets", "read-only-v1"),
                ("nativeCapabilities", "v0.0.1"),
                ("providerModelPricing", "catalog-v1"),
                ("agentIntelligenceCatalog", "catalog-v1"),
                ("skillHub", "request-snapshot-v1"),
                ("assistantWorkflowAuthoringBundle", "v1"),
            ],
            "the published source revisions changed",
        );
        let candidates = receipt
            .get("candidates")
            .and_then(Value::as_array)
            .expect("the candidates are a list");
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0]
                .as_object()
                .expect("one candidate is an object")
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "authority".to_owned(),
                "capabilities".to_owned(),
                "codingScore".to_owned(),
                "environment".to_owned(),
                "inputPriceUsdPerMillionTokens".to_owned(),
                "intelligence".to_owned(),
                "latencyClass".to_owned(),
                "membershipId".to_owned(),
                "model".to_owned(),
                "outputPriceUsdPerMillionTokens".to_owned(),
                "profileRevision".to_owned(),
                "readiness".to_owned(),
                "reliabilityClass".to_owned(),
                "responsibility".to_owned(),
                "skills".to_owned(),
                "taskTags".to_owned(),
            ]),
            "the published candidate projection changed",
        );
    }

    /// The crate root publishes the whole host-facing port surface in one
    /// place. A port that exists here but is not reachable from the
    /// composition is a contract break even when it compiles.
    #[test]
    fn port_surface_is_published_from_the_crate_root() {
        let published = CRATE_ROOT
            .lines()
            .skip_while(|line| !line.contains("pub use ports::{"))
            .skip(1)
            .take_while(|line| !line.contains("};"))
            .flat_map(|line| line.split(','))
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            published,
            ROOT_REEXPORTED_PORT_ITEMS
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<BTreeSet<_>>(),
            "the crate root re-exports of the workflow port surface changed",
        );
        let host_facing = scan_port_declarations(PORT_SOURCE)
            .into_iter()
            .filter(|declaration| {
                !UNPUBLISHED_PORT_ITEMS.contains(&declaration.item.render().as_str())
            })
            .map(|declaration| declaration.item.name)
            .collect::<BTreeSet<_>>();
        let mut missing = host_facing
            .difference(&published)
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        assert!(
            missing.is_empty(),
            "these port items are not reachable from the crate root: {missing:?}",
        );
    }
}
