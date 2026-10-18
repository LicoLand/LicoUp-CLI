//! What can be selected, kept apart from what may run now.
//!
//! One Agent name can be in several states at the same time, and this module
//! exists so they are never collapsed into one label:
//!
//! - **support** — is this Agent/provider/model combination usable at all on
//!   this host? A declared identity is supported; a model this host observed
//!   under an Agent the host cannot execute is not; a name nothing declares is
//!   unknown rather than unsupported, because "no declaration" is not a denial.
//! - **availability** — has a live source on this machine reported the model,
//!   and when? Absence of an observation is `Unobserved`: an observation that
//!   could not be taken is never read as "nothing there".
//! - **credentials** — does this host hold a usable credential for the serving
//!   provider? This is read from [`crate::port::CredentialState`] and keeps its
//!   three-valued answer: a host that cannot establish a credential reports
//!   `Unknown`, never `Absent`.
//! - **execution** — may this Agent run under the *effective* policy for one
//!   request scope, right now? Direct requests and workflow turns have separate
//!   scopes, and one Agent may legitimately be allowed in one and blocked in
//!   the other, so every outcome carries the scope it was decided for together
//!   with its own reason.
//!
//! The module judges nothing about policy. The effective admission decision is
//! owned above this crate and arrives through
//! [`crate::port::ModelCatalogPort::agent_scope_admission`]; a caller that
//! composes no owner keeps the fail-closed answer [`ScopeOutcomeState::Undetermined`],
//! which is the honest report that this host cannot say, and never `Allowed`.
//! Readiness evidence in particular is never promoted into an execution claim:
//! only an owner that states `Allowed` produces one.
//!
//! [`selection_matrix_document`] renders the whole matrix into the one JSON
//! document the desktop client reads, so the client renders these dimensions
//! instead of re-deriving a single readiness label from them.

use crate::availability::ObservedAvailability;
use crate::identity::CanonicalModel;
use crate::port::{CredentialState, ModelCatalogPort};
use crate::selection::{SelectionEvidence, SelectionFacts};
use serde_json::{Value, json};

/// Which kind of request the effective policy is currently deciding.
///
/// The two scopes are separate on purpose: a direct request answers one turn,
/// while a workflow turn is admitted into a durable multi-step task, and the
/// same Agent can have a different execution outcome in each.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SelectionScope {
    /// One request answered directly by the Agent.
    Direct,
    /// One turn admitted into a workflow.
    Workflow,
}

impl SelectionScope {
    /// Every scope the matrix reports, in the order a client renders them.
    pub const ALL: [Self; 2] = [Self::Direct, Self::Workflow];

    /// The stable wire value. A client keys its view on this, never on the
    /// variant's position.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Workflow => "workflow",
        }
    }
}

/// Whether this Agent/provider/model combination is usable at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportState {
    /// A declared canonical identity exists for the combination.
    Supported,
    /// Nothing declares the combination, or the host states it cannot serve it.
    /// This is a positive statement about the catalogue, not about policy.
    Unsupported,
    /// The host could not establish whether the combination is usable. It is
    /// never rendered as supported or unsupported.
    Unknown,
}

impl SupportState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether a live source on this machine reported the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityState {
    /// A live source reported it; the matrix records when.
    Observed,
    /// No live source reported it. This is not a negative claim about the model.
    Unobserved,
}

impl AvailabilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unobserved => "unobserved",
        }
    }
}

/// Whether the effective policy admits this Agent for one scope right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeOutcomeState {
    /// The policy owner admits the Agent for this scope now.
    Allowed,
    /// The policy owner refuses the Agent for this scope now.
    Blocked,
    /// No owner stated an outcome: this host cannot say. Fail-closed, and never
    /// rendered as allowed.
    Undetermined,
}

impl ScopeOutcomeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Blocked => "blocked",
            Self::Undetermined => "undetermined",
        }
    }
}

/// The effective policy's answer for one Agent and one scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeOutcome {
    /// The scope this outcome was decided for. An outcome is never reused for
    /// another scope.
    pub scope: SelectionScope,
    pub state: ScopeOutcomeState,
    /// Why the state holds. Never empty: a state without a reason cannot be
    /// inspected, and an unexplained `Allowed` cannot be audited.
    pub reason: String,
}

impl ScopeOutcome {
    pub fn new(scope: SelectionScope, state: ScopeOutcomeState, reason: &str) -> Self {
        Self {
            scope,
            state,
            reason: reason.to_owned(),
        }
    }
}

/// The fact the port owner states for one Agent and one scope.
///
/// It is deliberately separate from [`ScopeOutcome`]: the owner answers, the
/// catalogue records which scope the answer belongs to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeAdmissionFacts {
    pub state: ScopeOutcomeState,
    pub reason: String,
}

impl ScopeAdmissionFacts {
    pub fn new(state: ScopeOutcomeState, reason: &str) -> Self {
        Self {
            state,
            reason: reason.to_owned(),
        }
    }

    /// The fail-closed answer for a host that composes no policy owner.
    pub fn undetermined() -> Self {
        Self::new(
            ScopeOutcomeState::Undetermined,
            "selection_policy_owner_absent",
        )
    }
}

/// One model of one Agent, with every dimension kept separate.
#[derive(Clone, Debug)]
pub struct SelectionMatrixEntry {
    /// The Agent this entry was observed under. It is not the model.
    pub agent: String,
    /// The name the observing source reported.
    pub model: String,
    /// The declared canonical identity, when exactly one resolves.
    pub canonical: Option<CanonicalModel>,
    /// Which of the catalogue's identity/observation states this name is in.
    pub evidence: SelectionEvidence,
    pub support: SupportState,
    /// Why the support state holds.
    pub support_reason: String,
    pub availability: AvailabilityState,
    pub availability_reason: String,
    /// When the observation was taken, for the entries that have one.
    pub observed_at_unix_ms: Option<u64>,
    pub credentials: CredentialState,
    /// Why the credential state holds.
    pub credential_reason: String,
    /// The provider ids the observing source recorded. Never inferred.
    pub providers: Vec<String>,
    /// One outcome per scope, in [`SelectionScope::ALL`] order.
    pub outcomes: Vec<ScopeOutcome>,
}

impl SelectionMatrixEntry {
    /// The outcome recorded for one scope.
    ///
    /// It answers `None` when the scope was never decided, which is not the
    /// same as a scope that was decided unfavourably.
    pub fn outcome(&self, scope: SelectionScope) -> Option<&ScopeOutcome> {
        self.outcomes.iter().find(|outcome| outcome.scope == scope)
    }

    /// Whether the effective policy admits this Agent for one scope right now.
    /// Only a stated `Allowed` answers `true`.
    pub fn executes(&self, scope: SelectionScope) -> bool {
        self.outcome(scope)
            .is_some_and(|outcome| outcome.state == ScopeOutcomeState::Allowed)
    }
}

/// Every observed model of one Agent, with its dimensions kept apart.
#[derive(Clone, Debug)]
pub struct SelectionMatrix {
    pub agent: String,
    pub generation: Option<String>,
    pub observed_at_unix_ms: u64,
    pub entries: Vec<SelectionMatrixEntry>,
}

/// The effective-policy answer for one Agent and one scope.
///
/// The catalogue calls it once per scope, so a policy owner is never asked for
/// a scope it did not offer and an answer is never reused across scopes.
pub type ScopeAdmission =
    fn(agent: &str, scope: SelectionScope, params: &Value) -> ScopeAdmissionFacts;

/// The facts this module reads from above it.
///
/// It is a separate port from [`ModelCatalogPort`] because the effective policy
/// has a different owner from the probe, the credential claim and the source
/// generation: `licoup-native` composes both, and a process that composes
/// neither keeps every answer fail-closed. [`Self::unavailable`] states no
/// outcome for any scope.
#[derive(Clone, Copy)]
pub struct SelectionMatrixPort {
    pub agent_scope_admission: ScopeAdmission,
}

impl SelectionMatrixPort {
    /// No policy owner composed: every scope stays undetermined.
    pub const fn unavailable() -> Self {
        Self {
            agent_scope_admission: no_scope_admission,
        }
    }
}

impl Default for SelectionMatrixPort {
    fn default() -> Self {
        Self::unavailable()
    }
}

fn no_scope_admission(
    _agent: &str,
    _scope: SelectionScope,
    _params: &Value,
) -> ScopeAdmissionFacts {
    ScopeAdmissionFacts::undetermined()
}

/// The scope outcomes for one Agent, each asked for on its own.
fn scope_outcomes(port: &SelectionMatrixPort, agent: &str, params: &Value) -> Vec<ScopeOutcome> {
    SelectionScope::ALL
        .into_iter()
        .map(|scope| {
            let facts = (port.agent_scope_admission)(agent, scope, params);
            let reason = if facts.reason.trim().is_empty() {
                "selection_policy_reason_absent".to_owned()
            } else {
                facts.reason
            };
            ScopeOutcome::new(scope, facts.state, &reason)
        })
        .collect()
}

/// One entry per name the observing source reported, dimension by dimension.
///
/// Support is a judgement about the declared catalogue: a name whose canonical
/// identity resolves is supported, a name nothing declares is unsupported, and
/// an Agent the host does not declare keeps every entry unknown — a host that
/// does not know the Agent cannot claim anything about its models.
pub fn selection_matrix(
    port: &ModelCatalogPort,
    matrix_port: &SelectionMatrixPort,
    agent_declared: bool,
    report: &crate::selection::SelectionReport,
    params: &Value,
) -> SelectionMatrix {
    let outcomes = scope_outcomes(matrix_port, &report.target, params);
    let entries = report
        .entries
        .iter()
        .map(|facts| entry_for_facts(port, agent_declared, &report.target, facts, &outcomes))
        .collect();
    SelectionMatrix {
        agent: report.target.clone(),
        generation: report.generation.clone(),
        observed_at_unix_ms: report.observed_at_unix_ms,
        entries,
    }
}

fn entry_for_facts(
    port: &ModelCatalogPort,
    agent_declared: bool,
    agent: &str,
    facts: &SelectionFacts,
    outcomes: &[ScopeOutcome],
) -> SelectionMatrixEntry {
    let (support, support_reason) = support_for(agent_declared, facts);
    let (availability, availability_reason) = availability_for(facts);
    let provider_id = facts.providers.first().map(String::as_str);
    let credentials = provider_id.map_or(CredentialState::Unknown, |provider| {
        (port.provider_credential)(provider)
    });
    let credential_reason = match (provider_id, credentials) {
        (None, _) => "provider_not_recorded".to_owned(),
        (Some(_), CredentialState::Present) => "provider_credential_present".to_owned(),
        (Some(_), CredentialState::Absent) => "provider_credential_absent".to_owned(),
        (Some(_), CredentialState::Unknown) => "provider_credential_unknown".to_owned(),
    };
    SelectionMatrixEntry {
        // The Agent the observation was taken under — the report's target, not
        // the model. A client reads this field to place the row under its
        // Agent, so filling it with the model name would file every model
        // under a heading of its own.
        agent: agent.to_owned(),
        model: facts.name.clone(),
        canonical: facts.canonical.clone(),
        evidence: facts.evidence(),
        support,
        support_reason,
        availability,
        availability_reason,
        observed_at_unix_ms: facts.observed.observed_at_unix_ms(),
        credentials,
        credential_reason,
        providers: facts.providers.clone(),
        outcomes: outcomes.to_vec(),
    }
}

fn support_for(agent_declared: bool, facts: &SelectionFacts) -> (SupportState, String) {
    if !agent_declared {
        return (
            SupportState::Unknown,
            "agent_not_declared_on_host".to_owned(),
        );
    }
    if facts.canonical.is_some() {
        return (SupportState::Supported, "declared_identity".to_owned());
    }
    match facts.evidence() {
        // A name a live source reported but nothing declares is not a support
        // claim: the catalogue cannot say what it is, so it does not say it can
        // be used either.
        SelectionEvidence::Observed => (
            SupportState::Unknown,
            "observed_without_declared_identity".to_owned(),
        ),
        SelectionEvidence::DeclaredOnly => (
            SupportState::Unsupported,
            "declared_without_identity".to_owned(),
        ),
        SelectionEvidence::Unknown => (
            SupportState::Unsupported,
            "nothing_declares_the_model".to_owned(),
        ),
    }
}

fn availability_for(facts: &SelectionFacts) -> (AvailabilityState, String) {
    match &facts.observed {
        ObservedAvailability::Observed { sources, .. } => (
            AvailabilityState::Observed,
            if sources.is_empty() {
                "observed_without_named_source".to_owned()
            } else {
                "observed_by_live_source".to_owned()
            },
        ),
        ObservedAvailability::Unknown => (
            AvailabilityState::Unobserved,
            "not_observed_on_this_host".to_owned(),
        ),
    }
}

/// The complete client document for one Agent's matrix.
///
/// Every dimension keeps its own state and reason, and both scopes are always
/// present, so a client cannot render one collapsed readiness label by
/// accident: it has to choose which dimension and which scope it is showing.
pub fn selection_matrix_document(matrix: &SelectionMatrix) -> Value {
    json!({
        "schemaVersion": SELECTION_MATRIX_SCHEMA_VERSION,
        "scopes": SelectionScope::ALL
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>(),
        "agent": matrix.agent,
        "generation": matrix.generation,
        "observedAtUnixMs": matrix.observed_at_unix_ms,
        "entries": matrix
            .entries
            .iter()
            .map(entry_document)
            .collect::<Vec<_>>(),
    })
}

/// The document revision a client must agree with.
///
/// A client that does not know this revision refuses the document instead of
/// guessing which dimension a field belongs to.
pub const SELECTION_MATRIX_SCHEMA_VERSION: u64 = 1;

fn entry_document(entry: &SelectionMatrixEntry) -> Value {
    json!({
        "agent": entry.agent,
        "model": entry.model,
        "canonicalId": entry.canonical.as_ref().map(|model| model.id.as_str()),
        "canonicalDisplayName": entry.canonical.as_ref().map(|model| model.display_name.as_str()),
        "evidence": evidence_str(entry.evidence),
        "support": {
            "state": entry.support.as_str(),
            "reason": entry.support_reason,
        },
        "availability": {
            "state": entry.availability.as_str(),
            "reason": entry.availability_reason,
            "observedAtUnixMs": entry.observed_at_unix_ms,
        },
        "credentials": {
            "state": credential_str(entry.credentials),
            "reason": entry.credential_reason,
        },
        "providers": entry.providers,
        "scopes": SelectionScope::ALL
            .iter()
            .map(|scope| scope_document(entry, *scope))
            .collect::<Vec<_>>(),
    })
}

fn scope_document(entry: &SelectionMatrixEntry, scope: SelectionScope) -> Value {
    match entry.outcome(scope) {
        Some(outcome) => json!({
            "scope": scope.as_str(),
            "state": outcome.state.as_str(),
            "reason": outcome.reason,
        }),
        // An entry that recorded no outcome for a scope says so, rather than
        // letting a client render the missing scope as allowed.
        None => json!({
            "scope": scope.as_str(),
            "state": ScopeOutcomeState::Undetermined.as_str(),
            "reason": "scope_outcome_not_recorded",
        }),
    }
}

fn credential_str(state: CredentialState) -> &'static str {
    match state {
        CredentialState::Present => "present",
        CredentialState::Absent => "absent",
        CredentialState::Unknown => "unknown",
    }
}

fn evidence_str(evidence: SelectionEvidence) -> &'static str {
    match evidence {
        SelectionEvidence::DeclaredOnly => "declaredOnly",
        SelectionEvidence::Observed => "observed",
        SelectionEvidence::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::RegistrySnapshot;
    use crate::selection::selection_report;
    use crate::{
        ObservedCatalogCache,
        availability::ObservedModel,
        port::{CatalogSourceGeneration, ModelCatalogPort},
    };
    use serde_json::json;

    fn registry() -> RegistrySnapshot {
        RegistrySnapshot::from_catalog_with_wrappers(
            json!({
                "models": { "moonshotai/kimi-k3": { "name": "Kimi K3" } },
                "providers": {
                    "kimi-for-coding": {
                        "name": "Kimi Code",
                        "models": {
                            "k3-256k": { "name": "Kimi Code K3 256K", "base_model": "moonshotai/kimi-k3" }
                        }
                    }
                }
            }),
            Vec::<String>::new(),
        )
        .expect("synthetic registry")
    }

    fn observed(name: &str) -> ObservedModel {
        ObservedModel {
            name: name.to_owned(),
            provider_id: Some("kimi-for-coding".to_owned()),
            provider: None,
            sources: vec!["codex-app-server".to_owned()],
        }
    }

    fn observing_port() -> ModelCatalogPort {
        fn labels() -> Vec<String> {
            Vec::new()
        }
        fn probe(_agent: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
            Ok(vec![observed("k3-256k"), observed("mystery-preview")])
        }
        fn credential(_provider_id: &str) -> CredentialState {
            CredentialState::Unknown
        }
        fn generation() -> Option<String> {
            Some("generation-1".to_owned())
        }
        ModelCatalogPort {
            agent_wrapper_labels: labels,
            observe_target_models: probe,
            provider_credential: credential,
            source_generation: generation as CatalogSourceGeneration,
        }
    }

    fn report(port: &ModelCatalogPort) -> crate::selection::SelectionReport {
        selection_report(
            port,
            &ObservedCatalogCache::default(),
            &registry(),
            "kimi-code",
            &json!({}),
        )
        .expect("the probe answers")
    }

    #[test]
    fn one_agent_can_report_differing_direct_and_workflow_outcomes() {
        // The same Agent is admitted for a direct request and refused for a
        // workflow turn: two separate decisions with two separate reasons.
        fn admission(agent: &str, scope: SelectionScope, _params: &Value) -> ScopeAdmissionFacts {
            assert_eq!(agent, "kimi-code");
            match scope {
                SelectionScope::Direct => {
                    ScopeAdmissionFacts::new(ScopeOutcomeState::Allowed, "direct_request_admitted")
                }
                SelectionScope::Workflow => ScopeAdmissionFacts::new(
                    ScopeOutcomeState::Blocked,
                    "workflow_policy_disallows_agent",
                ),
            }
        }
        let port = observing_port();
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort {
                agent_scope_admission: admission,
            },
            true,
            &report(&port),
            &json!({}),
        );
        assert_eq!(matrix.agent, "kimi-code");

        let entry = &matrix.entries[0];
        assert_eq!(entry.model, "k3-256k");
        assert!(entry.executes(SelectionScope::Direct));
        assert!(!entry.executes(SelectionScope::Workflow));
        assert_eq!(
            entry
                .outcome(SelectionScope::Direct)
                .map(|outcome| outcome.reason.as_str()),
            Some("direct_request_admitted")
        );
        assert_eq!(
            entry
                .outcome(SelectionScope::Workflow)
                .map(|outcome| (outcome.state, outcome.reason.as_str())),
            Some((
                ScopeOutcomeState::Blocked,
                "workflow_policy_disallows_agent"
            ))
        );

        let document = selection_matrix_document(&matrix);
        assert_eq!(document["schemaVersion"], 1);
        assert_eq!(document["scopes"], json!(["direct", "workflow"]));
        assert_eq!(document["entries"][0]["scopes"][0]["state"], "allowed");
        assert_eq!(document["entries"][0]["scopes"][1]["state"], "blocked");
    }

    #[test]
    fn an_unanswered_policy_owner_never_reports_execution() {
        let port = observing_port();
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort::unavailable(),
            true,
            &report(&port),
            &json!({}),
        );
        for entry in &matrix.entries {
            for scope in SelectionScope::ALL {
                assert_eq!(
                    entry.outcome(scope).map(|outcome| outcome.state),
                    Some(ScopeOutcomeState::Undetermined),
                    "{scope:?} must stay undetermined without an owner"
                );
                assert!(!entry.executes(scope));
            }
        }
        let document = selection_matrix_document(&matrix);
        assert_eq!(document["entries"][0]["scopes"][0]["state"], "undetermined");
        assert_eq!(
            document["entries"][0]["scopes"][0]["reason"],
            "selection_policy_owner_absent"
        );
    }

    #[test]
    fn readiness_is_never_promoted_into_availability_or_support() {
        // The host observed nothing at all. Support is a declaration fact and
        // availability stays unobserved: neither is ever "ready".
        fn refusing_port() -> ModelCatalogPort {
            fn labels() -> Vec<String> {
                Vec::new()
            }
            fn refuse(_agent: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
                Err(anyhow::anyhow!("model_catalog_observation_unavailable"))
            }
            fn credential(_provider_id: &str) -> CredentialState {
                CredentialState::Unknown
            }
            fn generation() -> Option<String> {
                None
            }
            ModelCatalogPort {
                agent_wrapper_labels: labels,
                observe_target_models: refuse,
                provider_credential: credential,
                source_generation: generation as CatalogSourceGeneration,
            }
        }
        let port = refusing_port();
        let report = crate::selection::SelectionReport {
            target: "kimi-code".to_owned(),
            generation: None,
            observed_at_unix_ms: 0,
            entries: vec![crate::selection::declared_facts(
                &registry(),
                "moonshotai/kimi-k3",
                None,
                Some("kimi-code"),
            )],
        };
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort::unavailable(),
            true,
            &report,
            &json!({}),
        );
        let entry = &matrix.entries[0];
        assert_eq!(entry.support, SupportState::Supported);
        assert_eq!(
            entry.availability,
            AvailabilityState::Unobserved,
            "a declared model this host never observed is not available"
        );
        assert_eq!(entry.observed_at_unix_ms, None);
        assert!(!entry.executes(SelectionScope::Direct));
    }

    #[test]
    fn an_unsupported_combination_is_distinct_from_an_unexecutable_one() {
        // Support is a catalogue judgement; execution is a policy one. The
        // matrix states them separately, so neither hides the other.
        fn blocked(_agent: &str, scope: SelectionScope, _params: &Value) -> ScopeAdmissionFacts {
            let _ = scope;
            ScopeAdmissionFacts::new(ScopeOutcomeState::Blocked, "user_stop")
        }
        let port = observing_port();
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort {
                agent_scope_admission: blocked,
            },
            true,
            &report(&port),
            &json!({}),
        );
        // k3-256k declares a canonical identity through its serving provider,
        // so it is supported even though policy refuses to run it now.
        let supported = &matrix.entries[0];
        assert_eq!(supported.support, SupportState::Supported);
        assert_eq!(
            supported.outcome(SelectionScope::Direct).map(|o| o.state),
            Some(ScopeOutcomeState::Blocked)
        );

        // mystery-preview was observed but nothing declares it: support is
        // unknown, which is not the same as the policy refusal above.
        let undeclared = &matrix.entries[1];
        assert_eq!(undeclared.support, SupportState::Unknown);
        assert_eq!(
            undeclared.support_reason,
            "observed_without_declared_identity"
        );
        assert_eq!(
            undeclared.outcome(SelectionScope::Direct).map(|o| o.state),
            Some(ScopeOutcomeState::Blocked)
        );
        let document = selection_matrix_document(&matrix);
        assert_eq!(document["entries"][1]["support"]["state"], "unknown");
        assert_eq!(
            document["entries"][1]["support"]["reason"],
            "observed_without_declared_identity"
        );
    }

    #[test]
    fn an_undeclared_agent_keeps_unknown_support_instead_of_claiming_unsupported() {
        let port = observing_port();
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort::unavailable(),
            false,
            &report(&port),
            &json!({}),
        );
        for entry in &matrix.entries {
            assert_eq!(entry.support, SupportState::Unknown);
            assert_eq!(entry.support_reason, "agent_not_declared_on_host");
        }
    }

    /// Every entry names the Agent the observation was taken under, not the
    /// model it reports: a client places the row under its Agent.
    #[test]
    fn every_entry_names_its_agent_and_not_its_model() {
        let port = observing_port();
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort::unavailable(),
            true,
            &report(&port),
            &json!({}),
        );
        assert_eq!(matrix.agent, "kimi-code");
        for entry in &matrix.entries {
            assert_eq!(entry.agent, "kimi-code");
            assert_ne!(entry.agent, entry.model);
        }
        let document = selection_matrix_document(&matrix);
        assert_eq!(document["agent"], "kimi-code");
        for entry in document["entries"].as_array().expect("entries") {
            assert_eq!(entry["agent"], "kimi-code");
        }
    }

    #[test]
    fn a_recorded_provider_credential_is_reported_apart_from_support() {
        fn present(provider_id: &str) -> CredentialState {
            if provider_id == "kimi-for-coding" {
                CredentialState::Present
            } else {
                CredentialState::Unknown
            }
        }
        let mut port = observing_port();
        port.provider_credential = present as crate::port::ProviderCredential;
        let matrix = selection_matrix(
            &port,
            &SelectionMatrixPort::unavailable(),
            true,
            &report(&port),
            &json!({}),
        );
        assert_eq!(matrix.entries[0].credentials, CredentialState::Present);
        assert_eq!(
            matrix.entries[0].credential_reason,
            "provider_credential_present"
        );
        let document = selection_matrix_document(&matrix);
        assert_eq!(document["entries"][0]["credentials"]["state"], "present");
        assert_eq!(
            document["entries"][0]["providers"],
            json!(["kimi-for-coding"])
        );
    }
}
