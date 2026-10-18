//! The composition point for the model catalogue port.
//!
//! `licoup-model-catalog` owns the selection facts — declared canonical
//! identity, observed availability, recorded prices and the planning inputs
//! built on them — and declares [`ModelCatalogPort`] for every fact it reads
//! from a module composed above it. This module is where this host answers that
//! port: each member names the owner that actually holds the fact, so the
//! catalogue never refers to the Agent inventory, the driver engines or the
//! local state roots, and no reference points upward.
//!
//! It sits at the crate root, above the domain and platform layers, exactly
//! like the Agent inventory's port: answering a port is composition, not a
//! domain concern, and a domain module that reached into `crate::platform`
//! would be the coupling the layering exists to prevent.
//!
//! [`selection_matrix_port`] is the one composition function for the effective
//! execution policy, and it answers both consumers of that policy from the same
//! owner: the catalogue's client projection reads
//! [`SelectionMatrixPort::agent_scope_admission`] directly, and
//! [`install_routing_policy_owner`] installs that same admission answer together
//! with the candidate facts for the dispatch entry that routes with it. One
//! owner, one answer, so the scope outcome a client renders and the scope
//! outcome routing obeys cannot disagree.
//!
//! Two members are deliberately answered with less than a caller might wish,
//! and the difference is the point of the port:
//!
//! - **credentials.** No owner publishes a mapping from a catalogue provider id
//!   to the credential records it holds, and inventing one — matching a model
//!   or provider name against a credential label — would fabricate access. The
//!   honest answer is [`CredentialState::Unknown`] for every provider, and the
//!   candidate policy therefore excludes every alternative instead of running
//!   one whose access nothing established.
//! - **the source generation.** It is the revision of the canonical registry
//!   snapshot an observation was taken against. A host with no catalog
//!   publishes no revision, and then no observation is reused.

use licoup_model_catalog::availability::ObservedModel;
use licoup_model_catalog::candidate_policy::{CandidateId, QuotaState};
use licoup_model_catalog::port::{CredentialState, ModelCatalogPort};
use licoup_model_catalog::selection_matrix::{
    ScopeAdmissionFacts, ScopeOutcomeState, SelectionMatrixPort, SelectionScope,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::candidate_routing::{
    CandidateFactSource, CandidateFacts, install_candidate_facts, install_scope_admission,
};

/// The port this host composes: every catalogue fact answered by its owner.
pub fn model_catalog_port() -> ModelCatalogPort {
    ModelCatalogPort {
        agent_wrapper_labels: agent_wrapper_labels,
        observe_target_models: observe_target_models,
        provider_credential: provider_credential,
        source_generation: source_generation,
    }
}

/// The effective per-scope execution policy this host composes.
///
/// ## What this owner decides
///
/// Deciding whether one Agent may run *right now* is policy, and the owner of
/// that policy states its answer for one scope at a time. Readiness evidence is
/// not a substitute — a probed conversation runtime says the Agent can be
/// reached, not that the effective policy permits the request — so this
/// composition states no outcome of its own and never reports `allowed` on the
/// strength of a reachability fact.
///
/// The two scopes stay separate: one Agent may be admitted for a direct request
/// and refused for a workflow turn, and each answer carries the scope it was
/// decided for. The substitution point for a real policy owner is
/// [`agent_scope_admission`]; until one is composed, every scope stays
/// `Undetermined`, which a client renders as such and routing treats as "not
/// allowed" rather than as permission.
pub fn selection_matrix_port() -> SelectionMatrixPort {
    SelectionMatrixPort {
        agent_scope_admission: agent_scope_admission,
    }
}

/// Compose the routing policy owner the dispatch entry asks.
///
/// This is the composition step that makes a routing decision a policy decision
/// instead of an ad-hoc choice: it installs the effective scope admission
/// [`selection_matrix_port`] answers with, and the candidate facts the owners
/// on this host establish. A dispatch entry then asks one question — which
/// allowed alternative may run — and receives the catalogue's ranked answer or
/// an inspectable unavailable result.
///
/// The consequence of an admission owner that states no outcome is deliberate:
/// the policy allows no alternative, so no route and no suggestion is produced.
/// That is the fail-closed direction, and it is what this host reports while the
/// effective per-scope policy has no owner above the catalogue.
pub fn install_routing_policy_owner() {
    install_scope_admission(agent_scope_admission);
    install_candidate_facts(Arc::new(ComposedCandidateFacts));
}

fn agent_scope_admission(
    agent: &str,
    _scope: SelectionScope,
    _params: &Value,
) -> ScopeAdmissionFacts {
    if !crate::domain::agent_catalog::contains(agent) {
        return ScopeAdmissionFacts::new(
            ScopeOutcomeState::Undetermined,
            "agent_not_declared_on_host",
        );
    }
    ScopeAdmissionFacts::undetermined()
}

/// The candidate facts the owners on this host establish.
///
/// ## What is established, and what deliberately is not
///
/// * **what the Agent offers on this host.** The Agent inventory owns which
///   models one Agent offers and which provider serves them, and
///   `inspect_target_read_only` reads exactly that without executing the
///   Agent's binary or touching its history store. Availability is recorded from
///   that read and timed at the read, because the catalogue's availability
///   question is *has a source on this host reported the model*, and this
///   read-only owner is such a source. A candidate whose Agent or model the
///   declaration does not carry is answered `None` — nothing on this host
///   reported it — and no model is ever synthesized from a name, a price row or
///   an intelligence score.
/// * **a credential.** No owner publishes a catalogue-provider to credential
///   mapping, so the answer stays [`CredentialState::Unknown`] rather than a
///   claim that a credential exists.
/// * **a quota window.** The local quota owner publishes per-provider windows,
///   and no mapping from a catalogue provider id to one of those windows is
///   established here; the answer stays [`QuotaState::Unknown`], which the
///   policy records without reading as exhaustion.
///
/// Requirements are answered only where the declaration establishes them, so a
/// requirement nothing establishes stays `Unknown` and the candidate is
/// excluded under its own name instead of being read as satisfying it.
struct ComposedCandidateFacts;

impl CandidateFactSource for ComposedCandidateFacts {
    fn facts(&self, candidate: &CandidateId) -> Option<CandidateFacts> {
        let declared = declared_models(&candidate.agent_id)?;
        let model = declared
            .models
            .iter()
            .find(|model| model.name == candidate.model_id)?;
        Some(CandidateFacts {
            observed_at_unix_ms: Some(declared.observed_at_unix_ms),
            credential: (model_catalog_port().provider_credential)(
                model.provider_id.as_deref().unwrap_or_default(),
            ),
            quota: QuotaState::Unknown,
            requirements: BTreeMap::new(),
        })
    }
}

/// The models one Agent declares, with the time this host read them.
///
/// `None` means this host does not know the Agent, which is not the same as an
/// Agent that declares no model: the first is no evidence at all, and the
/// candidate policy answers it `Unknown`.
struct DeclaredModels {
    models: Vec<ObservedModel>,
    observed_at_unix_ms: u64,
}

fn declared_models(agent: &str) -> Option<DeclaredModels> {
    if !crate::domain::agent_catalog::contains(agent) {
        return None;
    }
    let inspected = crate::domain::targets::inspect_target_read_only(
        &crate::target_port::agent_target_port(),
        agent,
    )
    .ok()?;
    let models = observed_models(agent, &inspected);
    if models.is_empty() {
        return None;
    }
    Some(DeclaredModels {
        models,
        observed_at_unix_ms: licoup_model_catalog::now_unix_ms(),
    })
}

/// The Agent inventory's declaration labels. The inventory owns which Agents
/// exist on this machine; a label that names a source rather than a model is
/// stripped before canonical resolution matches.
fn agent_wrapper_labels() -> Vec<String> {
    crate::domain::agent_catalog::entries()
        .into_iter()
        .flat_map(|agent| [agent.id, agent.label])
        .collect()
}

/// Probe one Agent's live model catalog through the target owner.
///
/// The inventory owns the probe: which binary or app server to ask, which
/// config, cache and history paths to read, and which of them this call is
/// allowed to touch. The catalogue only records what came back and when.
fn observe_target_models(target: &str, params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
    let mut request = params.clone();
    let object = request
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("model_catalog_probe_params_invalid"))?;
    object.insert("target".to_owned(), Value::String(target.to_owned()));
    let inspected = crate::domain::targets::inspect_target_with_params(
        &crate::target_port::agent_target_port(),
        &request,
    )?;
    Ok(observed_models(target, &inspected))
}

/// Project one inspection response into the observation the catalogue records.
///
/// The projection copies the facts the observing source declared and completes
/// none of them: a row that names no provider keeps no provider, and a row that
/// names no model is not a model. A response for another target is not this
/// observation at all.
fn observed_models(target: &str, inspected: &Value) -> Vec<ObservedModel> {
    let Some(candidate) = inspected.get("target") else {
        return Vec::new();
    };
    let answered = candidate
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if answered != crate::domain::targets::normalize_target(target) {
        return Vec::new();
    }
    let Some(models) = candidate
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|model| {
            let name = model
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())?;
            Some(ObservedModel {
                name: name.to_owned(),
                provider_id: non_empty_string(model.get("providerId")),
                provider: non_empty_string(model.get("provider")),
                sources: model
                    .get("sources")
                    .and_then(Value::as_array)
                    .map(|sources| {
                        sources
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// No owner publishes a catalogue provider to credential mapping, and inferring
/// one from a name would fabricate access. Every provider stays unknown until an
/// owner states otherwise.
fn provider_credential(_provider_id: &str) -> CredentialState {
    CredentialState::Unknown
}

/// The revision of the canonical registry snapshot this host holds. An empty
/// catalog has no revision, and a generation that does not exist is `None`
/// rather than an empty string that would cache an attachment to nothing.
fn source_generation() -> Option<String> {
    let revision = crate::domain::model_registry::refresh_cached_snapshot()
        .revision()
        .to_owned();
    (!revision.is_empty()).then_some(revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// No policy owner is composed, so no scope is ever answered `allowed` —
    /// including for an Agent the inventory does declare and can reach.
    #[test]
    fn the_composed_policy_states_no_execution_outcome_for_any_scope() {
        let port = selection_matrix_port();
        for agent in ["codex", "cursor", "kilo-code"] {
            for scope in SelectionScope::ALL {
                let facts = (port.agent_scope_admission)(agent, scope, &json!({}));
                assert_eq!(
                    facts.state,
                    ScopeOutcomeState::Undetermined,
                    "{agent} {scope:?} must not be admitted by a composition with no owner"
                );
                assert_eq!(facts.reason, "selection_policy_owner_absent");
            }
        }
        // An Agent this host does not declare says so, instead of borrowing the
        // reason a declared Agent reports.
        let unknown = (port.agent_scope_admission)(
            "not-a-declared-agent",
            SelectionScope::Direct,
            &json!({}),
        );
        assert_eq!(unknown.state, ScopeOutcomeState::Undetermined);
        assert_eq!(unknown.reason, "agent_not_declared_on_host");
    }

    /// The routing owner reads its admission answer and its candidate facts from
    /// the owners this composition names: no credential owner and no quota owner
    /// are composed, so a candidate with a declared model is still excluded by
    /// the policy under its own credential name rather than selected on a
    /// declaration alone.
    #[test]
    fn the_routing_owner_reads_the_same_admission_the_projection_reads() {
        use crate::domain::candidate_routing::{CandidateFactSource, CandidateId};

        assert!(
            CandidateFactSource::facts(
                &ComposedCandidateFacts,
                &CandidateId::new("not-a-declared-agent", "any-model", None)
            )
            .is_none(),
            "an Agent this host does not declare establishes no candidate fact"
        );

        for scope in SelectionScope::ALL {
            let decision =
                (selection_matrix_port().agent_scope_admission)("codex", scope, &json!({}));
            assert_eq!(decision.state, ScopeOutcomeState::Undetermined);
            assert_eq!(decision.reason, "selection_policy_owner_absent");
        }
    }

    #[test]
    fn the_composition_answers_every_port_member_from_its_owner() {
        let port = model_catalog_port();

        // The inventory's declarations, not a copied list.
        let labels = (port.agent_wrapper_labels)();
        for agent in ["codex", "cursor", "claude-code", "kilo-code"] {
            assert!(labels.iter().any(|label| label == agent), "{agent}");
        }

        // No owner publishes a credential mapping, so no access is claimed.
        assert_eq!(
            (port.provider_credential)("openai-chatgpt"),
            CredentialState::Unknown
        );
        assert_eq!(
            (port.provider_credential)("not-a-provider"),
            CredentialState::Unknown
        );

        // A test build holds no catalog revision, so this host publishes no
        // generation and nothing it observes may be reused.
        assert_eq!((port.source_generation)(), None);
    }

    #[test]
    fn an_undeclared_target_reports_no_observation_instead_of_an_empty_catalog() {
        let port = model_catalog_port();
        assert!(
            (port.observe_target_models)("not-a-declared-agent", &json!({})).is_err(),
            "a probe that cannot run is not an empty model list"
        );
    }

    #[test]
    fn the_projection_copies_declared_facts_and_invents_no_provider() {
        let inspected = json!({
            "ok": true,
            "target": {
                "target": "codex",
                "modelCatalog": {
                    "status": "available",
                    "models": [
                        {
                            "name": "gpt-5.6-sol",
                            "providerId": "openai-chatgpt",
                            "provider": "ChatGPT",
                            "sources": ["codex-app-server", "config"]
                        },
                        { "name": "  " },
                        { "providerId": "openai-chatgpt" },
                        { "name": "mystery-preview", "sources": [] }
                    ]
                }
            }
        });
        let models = observed_models("codex", &inspected);
        assert_eq!(
            models,
            vec![
                ObservedModel {
                    name: "gpt-5.6-sol".to_owned(),
                    provider_id: Some("openai-chatgpt".to_owned()),
                    provider: Some("ChatGPT".to_owned()),
                    sources: vec!["codex-app-server".to_owned(), "config".to_owned()],
                },
                ObservedModel {
                    name: "mystery-preview".to_owned(),
                    provider_id: None,
                    provider: None,
                    sources: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn a_catalog_that_could_not_be_read_projects_no_observation() {
        assert!(observed_models("codex", &json!({ "target": { "target": "codex" } })).is_empty());
        assert!(observed_models("codex", &json!({})).is_empty());
        assert!(
            observed_models(
                "codex",
                &json!({ "target": { "target": "cursor", "modelCatalog": { "models": [{ "name": "gpt-5.6-sol" }] } } })
            )
            .is_empty(),
            "a response for another target is not this observation"
        );
        assert_eq!(
            observed_models(
                "CODEX",
                &json!({ "target": { "target": "codex", "modelCatalog": { "models": [{ "name": "gpt-5.6-sol" }] } } })
            )
            .len(),
            1,
            "the caller's spelling is normalized before it is compared"
        );
    }
}
