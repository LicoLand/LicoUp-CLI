//! The production catalogue query: declared identity, observed availability and
//! unknown evidence, kept apart.
//!
//! One model name can be in exactly one of three catalogue states, and a caller
//! must be able to tell them apart:
//!
//! - **declared**: the canonical identity index resolves the name to one model.
//!   That is a statement about identity and provider aliases, not about whether
//!   this host can use it.
//! - **observed**: a live source on this machine reported the name at a known
//!   time. That is evidence of availability at that time, not a declaration.
//! - **unknown**: neither. The catalogue reports it as unknown and never
//!   synthesizes an entry from a name, a pricing row or an intelligence score.
//!
//! Both dimensions are reported together and separately: an observed name whose
//! identity is unknown is still observed, and a declared name this host never
//! observed is not available.

use crate::availability::{
    ObservedAvailability, ObservedCatalog, ObservedCatalogCache, ObservedModel,
};
use crate::identity::{CanonicalModel, RegistrySnapshot};
use crate::port::ModelCatalogPort;
use serde_json::Value;

/// Which of the three catalogue states one name is in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionEvidence {
    /// Declared by the identity index and not observed on this host.
    DeclaredOnly,
    /// Observed on this host, whether or not its identity is declared.
    Observed,
    /// Neither declared nor observed.
    Unknown,
}

/// The catalogue's answer for one model name.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionFacts {
    /// The name the caller asked about or the source reported.
    pub name: String,
    /// The declared canonical identity, when exactly one resolves.
    pub canonical: Option<CanonicalModel>,
    /// The observed availability, with the time it was taken.
    pub observed: ObservedAvailability,
    /// The provider ids the observing source recorded. Never inferred.
    pub providers: Vec<String>,
}

impl SelectionFacts {
    pub fn evidence(&self) -> SelectionEvidence {
        match (self.canonical.is_some(), self.observed.is_observed()) {
            (_, true) => SelectionEvidence::Observed,
            (true, false) => SelectionEvidence::DeclaredOnly,
            (false, false) => SelectionEvidence::Unknown,
        }
    }

    /// Availability is evidence: only an observation makes a model available.
    pub fn is_available(&self) -> bool {
        self.observed.is_observed()
    }
}

/// One target's observed catalogue, joined with declared identity.
#[derive(Clone, Debug)]
pub struct SelectionReport {
    pub target: String,
    pub generation: Option<String>,
    pub observed_at_unix_ms: u64,
    pub entries: Vec<SelectionFacts>,
}

/// Join one name's declared identity with the evidence recorded for it.
pub fn selection_facts(
    registry: &RegistrySnapshot,
    name: &str,
    provider_id: Option<&str>,
    source_agent_id: Option<&str>,
    observed: ObservedAvailability,
    providers: Vec<String>,
) -> SelectionFacts {
    SelectionFacts {
        name: name.to_owned(),
        canonical: registry
            .resolve_with_provider(name, provider_id, source_agent_id)
            .cloned(),
        observed,
        providers,
    }
}

/// The declared-only answer: identity from the index, availability unknown.
pub fn declared_facts(
    registry: &RegistrySnapshot,
    name: &str,
    provider_id: Option<&str>,
    source_agent_id: Option<&str>,
) -> SelectionFacts {
    selection_facts(
        registry,
        name,
        provider_id,
        source_agent_id,
        ObservedAvailability::Unknown,
        Vec::new(),
    )
}

/// The answer for a name nothing declares and nothing observed.
pub fn unknown_facts(name: &str) -> SelectionFacts {
    selection_facts(
        &RegistrySnapshot::empty(),
        name,
        None,
        None,
        ObservedAvailability::Unknown,
        Vec::new(),
    )
}

/// The production query for one Agent target.
///
/// The observation comes from the probe the composed port answered and is
/// cached by owner generation; the identity comes from the declared index. A
/// probe that cannot answer fails the query instead of yielding an empty model
/// set, so a caller never reads "could not look" as "nothing there".
pub fn selection_report(
    port: &ModelCatalogPort,
    cache: &ObservedCatalogCache,
    registry: &RegistrySnapshot,
    target: &str,
    params: &Value,
) -> anyhow::Result<SelectionReport> {
    let snapshot = cache.observe(port, target, params)?;
    Ok(report_from_observation(registry, target, &snapshot))
}

fn report_from_observation(
    registry: &RegistrySnapshot,
    target: &str,
    snapshot: &ObservedCatalog,
) -> SelectionReport {
    SelectionReport {
        target: target.to_owned(),
        generation: snapshot.generation.clone(),
        observed_at_unix_ms: snapshot.observed_at_unix_ms,
        entries: snapshot
            .models
            .iter()
            .map(|model| facts_for_observed(registry, target, snapshot, model))
            .collect(),
    }
}

fn facts_for_observed(
    registry: &RegistrySnapshot,
    target: &str,
    snapshot: &ObservedCatalog,
    model: &ObservedModel,
) -> SelectionFacts {
    selection_facts(
        registry,
        &model.name,
        model.provider_id.as_deref(),
        Some(target),
        ObservedAvailability::Observed {
            at_unix_ms: snapshot.observed_at_unix_ms,
            sources: model.sources.clone(),
        },
        model
            .provider_id
            .iter()
            .chain(model.provider.iter())
            .cloned()
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{CatalogSourceGeneration, CredentialState, ModelCatalogPort};
    use serde_json::json;

    fn registry() -> RegistrySnapshot {
        RegistrySnapshot::from_catalog(json!({
            "models": {
                "moonshotai/kimi-k3": { "name": "Kimi K3" },
                "openai/gpt-5.6-sol": { "name": "GPT-5.6 Sol" }
            },
            "providers": {
                "kimi-for-coding": {
                    "name": "Kimi Code",
                    "models": { "k3-256k": { "name": "Kimi Code K3 256K", "base_model": "moonshotai/kimi-k3" } }
                },
                "relay-for-coding": {
                    "name": "Relay Code",
                    "models": { "k3-256k": { "name": "Relay K3 256K", "base_model": "openai/gpt-5.6-sol" } }
                }
            }
        }))
        .expect("synthetic registry")
    }

    fn observed_model(name: &str, provider_id: Option<&str>) -> ObservedModel {
        ObservedModel {
            name: name.to_owned(),
            provider_id: provider_id.map(str::to_owned),
            provider: None,
            sources: vec!["codex-app-server".to_owned()],
        }
    }

    fn snapshot(models: Vec<ObservedModel>) -> ObservedCatalog {
        ObservedCatalog {
            target: "codex".to_owned(),
            generation: Some("generation-1".to_owned()),
            observed_at_unix_ms: 1_726_000_000_000,
            models,
        }
    }

    fn declared(name: &str) -> SelectionFacts {
        declared_facts(&registry(), name, None, None)
    }

    #[test]
    fn a_declared_model_this_host_never_observed_is_not_available() {
        let facts = declared("moonshotai/kimi-k3");
        assert_eq!(facts.evidence(), SelectionEvidence::DeclaredOnly);
        assert_eq!(
            facts.canonical.as_ref().map(|model| model.id.as_str()),
            Some("moonshotai/kimi-k3")
        );
        assert_eq!(facts.observed, ObservedAvailability::Unknown);
        assert!(!facts.is_available());
    }

    #[test]
    fn a_shared_provider_selector_is_declared_only_with_its_serving_provider() {
        let provider_qualified = declared_facts(
            &registry(),
            "k3-256k",
            Some("kimi-for-coding"),
            Some("kimi-code"),
        );
        assert_eq!(
            provider_qualified
                .canonical
                .as_ref()
                .map(|model| model.id.as_str()),
            Some("moonshotai/kimi-k3")
        );
        assert_eq!(
            provider_qualified.evidence(),
            SelectionEvidence::DeclaredOnly
        );

        // Two providers expose the same native selector for different models.
        // Without its serving provider the name is ambiguous, and the catalogue
        // leaves it unclassified instead of guessing a destination.
        assert_eq!(declared("k3-256k").canonical, None);
        assert_eq!(declared("k3-256k").evidence(), SelectionEvidence::Unknown);
    }

    #[test]
    fn an_observed_model_records_its_time_and_stays_observed_without_identity() {
        let report = report_from_observation(
            &registry(),
            "codex",
            &snapshot(vec![
                observed_model("gpt-5.6-sol", Some("openai-chatgpt")),
                observed_model("mystery-preview", None),
            ]),
        );
        assert_eq!(report.observed_at_unix_ms, 1_726_000_000_000);
        assert_eq!(report.generation.as_deref(), Some("generation-1"));

        let known = &report.entries[0];
        assert_eq!(known.evidence(), SelectionEvidence::Observed);
        assert!(known.is_available());
        assert_eq!(
            known.canonical.as_ref().map(|model| model.id.as_str()),
            Some("openai/gpt-5.6-sol")
        );
        assert_eq!(
            known.observed.observed_at_unix_ms(),
            Some(1_726_000_000_000)
        );
        assert_eq!(known.providers, vec!["openai-chatgpt".to_owned()]);

        let unidentified = &report.entries[1];
        assert_eq!(unidentified.evidence(), SelectionEvidence::Observed);
        assert!(unidentified.is_available());
        assert_eq!(unidentified.canonical, None);
        assert!(unidentified.providers.is_empty());
    }

    #[test]
    fn nothing_declared_and_nothing_observed_stays_unknown() {
        // `deepseek-v4-flash` has a pricing row in the maintained catalog; a
        // price is not availability evidence and must not create an entry.
        let facts = unknown_facts("deepseek-v4-flash");
        assert_eq!(facts.evidence(), SelectionEvidence::Unknown);
        assert_eq!(facts.canonical, None);
        assert!(!facts.is_available());
        assert_eq!(facts.observed, ObservedAvailability::Unknown);

        let report = report_from_observation(&registry(), "codex", &snapshot(Vec::new()));
        assert!(report.entries.is_empty());
    }

    #[test]
    fn an_unanswered_probe_is_an_error_and_never_an_empty_available_catalog() {
        fn no_generation() -> Option<String> {
            None
        }
        fn no_credential(_provider_id: &str) -> CredentialState {
            CredentialState::Unknown
        }
        fn no_labels() -> Vec<String> {
            Vec::new()
        }
        fn refuse(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
            Err(anyhow::anyhow!("model_catalog_observation_unavailable"))
        }
        let port = ModelCatalogPort {
            agent_wrapper_labels: no_labels,
            observe_target_models: refuse,
            provider_credential: no_credential,
            source_generation: no_generation as CatalogSourceGeneration,
        };
        let cache = ObservedCatalogCache::default();
        assert!(
            selection_report(&port, &cache, &registry(), "codex", &json!({})).is_err(),
            "a refused probe must not read as an empty catalog"
        );
    }
}
