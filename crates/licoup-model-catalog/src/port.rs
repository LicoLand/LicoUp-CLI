//! The port this catalogue declares for the facts owned above it.
//!
//! The catalogue owns model selection facts: the declared canonical identity of
//! a model and its provider aliases, the observed availability evidence, the
//! pricing facts and the planning inputs built on them. It owns no probe, no
//! credential store and no source revision: those belong to the modules
//! composed above it, and they arrive through [`ModelCatalogPort`].
//!
//! The port is a value of `fn` pointers rather than a trait, exactly like the
//! Agent inventory's port, so the catalogue keeps no state a caller did not
//! hand it and every member is called only on the branch that needs it:
//! probing a target runs a bounded CLI or app-server lookup, and computing it
//! eagerly would run that work on every identity resolution.
//!
//! [`ModelCatalogPort::unavailable`] declares every fact as unknown. It is the
//! fail-closed answer for a host that composes no probe and no declaration
//! owner: no wrapper label is stripped, so alias resolution stays strict; no
//! observation is ever reported, so no model is fabricated as available; no
//! provider credential is claimed; and no source generation exists, so no
//! observed snapshot is reused. Production composition names the real owners
//! instead, from the `licoup-native` crate root.

use crate::availability::ObservedModel;
use serde_json::Value;
use std::sync::OnceLock;

/// Whether this host holds a usable credential for one provider.
///
/// `Unknown` is the fail-closed answer and is never read as `Absent`: a caller
/// that cannot establish a credential reports no support claim rather than a
/// negative one. The catalogue never infers this state from a file's presence
/// or a model name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialState {
    Present,
    Absent,
    Unknown,
}

/// The Agent inventory's declaration labels that name a source rather than a
/// model. Canonical resolution strips them before matching, so a host that
/// cannot answer this list resolves strictly instead of loosely.
pub type AgentWrapperLabels = fn() -> Vec<String>;

/// Probe one installed Agent and report the models it currently offers.
///
/// Failure is a real answer: it means this host holds no observation, and the
/// catalogue reports unknown availability rather than an empty model set.
pub type ObserveTargetModels =
    fn(target: &str, params: &Value) -> anyhow::Result<Vec<ObservedModel>>;

/// Whether this host holds a credential for one provider.
pub type ProviderCredential = fn(provider_id: &str) -> CredentialState;

/// The generation of the catalogue source an observation was taken against.
///
/// An immutable observed snapshot may be reused only while this generation is
/// unchanged. `None` means the owner publishes no generation, and then no
/// observation is ever reused.
pub type CatalogSourceGeneration = fn() -> Option<String>;

/// Every fact the catalogue reads from a module composed above it.
#[derive(Clone, Copy)]
pub struct ModelCatalogPort {
    pub agent_wrapper_labels: AgentWrapperLabels,
    pub observe_target_models: ObserveTargetModels,
    pub provider_credential: ProviderCredential,
    pub source_generation: CatalogSourceGeneration,
}

impl ModelCatalogPort {
    /// Every fact unknown, and every observation refused.
    pub const fn unavailable() -> Self {
        Self {
            agent_wrapper_labels: no_agent_wrapper_labels,
            observe_target_models: no_target_models_observed,
            provider_credential: no_provider_credential,
            source_generation: no_source_generation,
        }
    }
}

impl Default for ModelCatalogPort {
    fn default() -> Self {
        Self::unavailable()
    }
}

const fn no_agent_wrapper_labels() -> Vec<String> {
    Vec::new()
}

fn no_target_models_observed(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
    Err(anyhow::anyhow!("model_catalog_observation_unavailable"))
}

const fn no_provider_credential(_provider_id: &str) -> CredentialState {
    CredentialState::Unknown
}

const fn no_source_generation() -> Option<String> {
    None
}

/// The fail-closed declaration, kept as a value so an unanswered process
/// borrows it instead of installing a second answer.
static FAIL_CLOSED: ModelCatalogPort = ModelCatalogPort::unavailable();
static INSTALLED: OnceLock<ModelCatalogPort> = OnceLock::new();

/// Install this host's answers for the catalogue port, once per process.
///
/// The composition that calls it lives at the `licoup-native` crate root,
/// above both layers. One answer per port: a second installation is refused
/// rather than silently replacing the first, and a process that never calls it
/// keeps every member fail-closed.
pub fn install_model_catalog_port(port: ModelCatalogPort) -> Result<(), &'static str> {
    INSTALLED
        .set(port)
        .map_err(|_| "model_catalog_port_already_installed")
}

/// The installed answers, or the fail-closed declaration when none was
/// installed.
pub fn model_catalog_port() -> &'static ModelCatalogPort {
    INSTALLED.get().unwrap_or(&FAIL_CLOSED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_unavailable_port_fails_closed_on_every_member() {
        let port = ModelCatalogPort::unavailable();
        assert!((port.agent_wrapper_labels)().is_empty());
        assert!((port.observe_target_models)("codex", &json!({})).is_err());
        assert_eq!(
            (port.provider_credential)("openai-chatgpt"),
            CredentialState::Unknown
        );
        assert_eq!((port.source_generation)(), None);
    }

    /// Installing is process-wide, so the answered-port test lives in its own
    /// test binary (`tests/composition_port.rs`) and this module only asserts
    /// what an unanswered process reports.
    #[test]
    fn an_unanswered_process_reports_the_fail_closed_declaration() {
        assert!((model_catalog_port().agent_wrapper_labels)().is_empty());
        assert!((model_catalog_port().observe_target_models)("codex", &json!({})).is_err());
        assert_eq!(
            (model_catalog_port().provider_credential)("openai-chatgpt"),
            CredentialState::Unknown
        );
        assert_eq!((model_catalog_port().source_generation)(), None);
    }
}
