//! The process-level half of the catalogue port: what an unanswered process
//! reports, and that an installed answer is the only one the port gives.
//!
//! This lives in its own test binary because installation is process-wide: a
//! unit test that installed a port would change what every other test in the
//! same binary resolves, and the identity suites state their expectations
//! against the declarations only a real composition supplies.

use licoup_model_catalog::availability::ObservedModel;
use licoup_model_catalog::port::{
    CatalogSourceGeneration, CredentialState, ModelCatalogPort, install_model_catalog_port,
    model_catalog_port,
};
use licoup_model_catalog::selection::{
    SelectionEvidence, declared_facts, selection_report, unknown_facts,
};
use licoup_model_catalog::{ObservedCatalogCache, RegistrySnapshot};
use serde_json::{Value, json};

fn labels() -> Vec<String> {
    vec!["codex".to_owned()]
}

fn observed(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
    Ok(vec![ObservedModel {
        name: "gpt-5.6-sol".to_owned(),
        provider_id: Some("openai-chatgpt".to_owned()),
        provider: None,
        sources: vec!["codex-app-server".to_owned()],
    }])
}

fn refused(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
    Err(anyhow::anyhow!("model_catalog_observation_unavailable"))
}

fn credential(provider_id: &str) -> CredentialState {
    if provider_id == "openai-chatgpt" {
        CredentialState::Unknown
    } else {
        CredentialState::Absent
    }
}

fn generation() -> Option<String> {
    Some("generation-1".to_owned())
}

fn registry() -> RegistrySnapshot {
    RegistrySnapshot::from_catalog(json!({
        "models": { "openai/gpt-5.6-sol": { "name": "GPT-5.6 Sol" } },
        "providers": {}
    }))
    .expect("synthetic registry")
}

#[test]
fn an_unanswered_process_fails_closed_and_the_installed_answer_is_the_only_one() {
    // A process that composes nothing keeps the fail-closed declaration: no
    // wrapper labels, no observation, no credential claim, no generation.
    assert!((model_catalog_port().agent_wrapper_labels)().is_empty());
    assert!((model_catalog_port().observe_target_models)("codex", &json!({})).is_err());
    assert_eq!(
        (model_catalog_port().provider_credential)("openai-chatgpt"),
        CredentialState::Unknown
    );
    assert_eq!((model_catalog_port().source_generation)(), None);

    let composed = ModelCatalogPort {
        agent_wrapper_labels: labels,
        observe_target_models: observed,
        provider_credential: credential,
        source_generation: generation,
    };
    install_model_catalog_port(composed).expect("the first installation is accepted");
    assert_eq!(
        (model_catalog_port().agent_wrapper_labels)(),
        vec!["codex".to_owned()]
    );
    assert_eq!(
        (model_catalog_port().source_generation)(),
        Some("generation-1".to_owned())
    );
    assert_eq!(
        install_model_catalog_port(ModelCatalogPort::unavailable()),
        Err("model_catalog_port_already_installed"),
        "a second installation cannot replace the composed answer"
    );

    // The composed port answers the production query, and the three states stay
    // distinguishable through it.
    let cache = ObservedCatalogCache::default();
    let report = selection_report(
        model_catalog_port(),
        &cache,
        &registry(),
        "codex",
        &json!({}),
    )
    .expect("the composed probe answers");
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].evidence(), SelectionEvidence::Observed);
    assert!(report.entries[0].is_available());
    assert_eq!(
        report.entries[0]
            .canonical
            .as_ref()
            .map(|model| model.id.as_str()),
        Some("openai/gpt-5.6-sol")
    );
    assert_eq!(
        report.entries[0].observed.observed_at_unix_ms().is_some(),
        true
    );
    assert_eq!(
        declared_facts(&registry(), "openai/gpt-5.6-sol", None, None).evidence(),
        SelectionEvidence::DeclaredOnly
    );
    assert_eq!(
        unknown_facts("deepseek-v4-flash").evidence(),
        SelectionEvidence::Unknown
    );
}

#[test]
fn a_composed_probe_that_refuses_is_never_read_as_an_empty_available_catalog() {
    fn no_generation() -> Option<String> {
        None
    }
    let port = ModelCatalogPort {
        agent_wrapper_labels: labels,
        observe_target_models: refused,
        provider_credential: credential,
        source_generation: no_generation as CatalogSourceGeneration,
    };
    let cache = ObservedCatalogCache::default();
    assert!(selection_report(&port, &cache, &registry(), "codex", &json!({})).is_err());
}
