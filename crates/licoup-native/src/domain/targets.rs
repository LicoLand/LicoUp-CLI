// The Agent inventory moved to `licoup-agent-targets`, which is the single
// authority for which Agents exist on this machine and where they live: the
// target catalog and its discovery, the per-Agent model catalog, the packaged
// scan-path manifest and the scan-path rules. Every former path stays reachable
// through this re-export for the FFI command layer, the driver engines, the
// model facts, the conversation history readers and the Agent binaries, which
// are the consumers that still live in `licoup-native`. Every call into it
// takes the port `crate::target_port` composes.
pub use licoup_agent_targets::domain::targets::*;

use std::sync::OnceLock;

/// The catalogue facts for one Agent target, through the port this host
/// composes.
///
/// The probe, the declared canonical identity and the observation timestamp are
/// joined by `licoup-model-catalog`; this host owns the answers, not the
/// judgement. A consumer reads this instead of the raw observed document so a
/// declared model, an observed model and an unknown name stay distinguishable,
/// and historical usage identity keeps resolving through the same registry.
///
/// A probe that cannot answer fails the query: "could not look" is never
/// reported as "nothing there".
pub fn model_selection_facts(
    target: &str,
    params: &serde_json::Value,
) -> anyhow::Result<licoup_model_catalog::SelectionReport> {
    licoup_model_catalog::selection_report(
        licoup_model_catalog::model_catalog_port(),
        observation_cache(),
        &licoup_model_catalog::identity::refresh_cached_snapshot(),
        target,
        params,
    )
}

/// The selection matrix for one Agent target: support, availability,
/// credentials and the effective per-scope execution outcome, kept apart.
///
/// The catalogue joins the dimensions; this host composes the facts they are
/// joined from. `agent_declared` is the inventory's own membership answer, so an
/// Agent this host does not declare reports unknown support instead of a claim,
/// and the policy answer arrives from
/// [`crate::model_catalog_port::selection_matrix_port`], which states no outcome
/// until a policy owner is composed.
pub fn model_selection_matrix(
    target: &str,
    params: &serde_json::Value,
) -> anyhow::Result<licoup_model_catalog::SelectionMatrix> {
    let report = model_selection_facts(target, params)?;
    let agent_declared = crate::domain::agent_catalog::contains(target);
    let matrix_port = crate::model_catalog_port::selection_matrix_port();
    Ok(licoup_model_catalog::selection_matrix(
        licoup_model_catalog::model_catalog_port(),
        &matrix_port,
        agent_declared,
        &report,
        params,
    ))
}

/// The same matrix as the one client document the desktop projection reads.
pub fn model_selection_matrix_document(
    target: &str,
    params: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    Ok(licoup_model_catalog::selection_matrix_document(
        &model_selection_matrix(target, params)?,
    ))
}

/// One immutable observation per target, reused only while the owner publishes
/// the same source generation.
fn observation_cache() -> &'static licoup_model_catalog::ObservedCatalogCache {
    static OBSERVATION_CACHE: OnceLock<licoup_model_catalog::ObservedCatalogCache> =
        OnceLock::new();
    OBSERVATION_CACHE.get_or_init(licoup_model_catalog::ObservedCatalogCache::default)
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use licoup_model_catalog::SelectionEvidence;
    use serde_json::json;

    #[test]
    fn a_target_the_catalogue_cannot_observe_reports_an_error_not_an_empty_catalog() {
        let error = model_selection_facts("not-a-declared-agent", &json!({}))
            .expect_err("an unobservable target is not an empty catalogue");
        assert!(
            error.to_string().contains("model_registry")
                || error.to_string().contains("model_catalog")
                || error.to_string().contains("target")
                || error.to_string().contains("_unavailable"),
            "{error}"
        );
    }

    #[test]
    fn the_declared_unknown_and_observed_states_stay_distinguishable() {
        // Nothing observed, nothing declared: unknown, and no price row or
        // intelligence score changes that.
        assert_eq!(
            licoup_model_catalog::unknown_facts("deepseek-v4-flash").evidence(),
            SelectionEvidence::Unknown
        );
        // The test build holds the synthetic registry: a declared identity is
        // declared only, and availability stays unknown until a source reports
        // the model.
        let registry = licoup_model_catalog::RegistrySnapshot::from_catalog(json!({
            "models": { "moonshotai/kimi-k3": { "name": "Kimi K3" } },
            "providers": {}
        }))
        .expect("synthetic registry");
        let declared = licoup_model_catalog::declared_facts(
            &registry,
            "moonshotai/kimi-k3",
            None,
            Some("kimi-code"),
        );
        assert_eq!(declared.evidence(), SelectionEvidence::DeclaredOnly);
        assert!(!declared.is_available());
    }

    /// The document one Agent produces: every dimension present and separate,
    /// both scopes stated, and no execution claim anywhere.
    #[test]
    fn the_composed_document_states_every_dimension_separately() {
        fn credential(provider_id: &str) -> licoup_model_catalog::CredentialState {
            if provider_id == "kimi-for-coding" {
                licoup_model_catalog::CredentialState::Present
            } else {
                licoup_model_catalog::CredentialState::Unknown
            }
        }
        let port = licoup_model_catalog::ModelCatalogPort {
            provider_credential: credential as licoup_model_catalog::port::ProviderCredential,
            ..licoup_model_catalog::ModelCatalogPort::unavailable()
        };
        let registry = licoup_model_catalog::RegistrySnapshot::from_catalog_with_wrappers(
            json!({
                "models": { "moonshotai/kimi-k3": { "name": "Kimi K3" } },
                "providers": {
                    "kimi-for-coding": {
                        "name": "Kimi Code",
                        "models": {
                            "k3-256k": {
                                "name": "Kimi Code K3 256K",
                                "base_model": "moonshotai/kimi-k3"
                            }
                        }
                    }
                }
            }),
            Vec::<String>::new(),
        )
        .expect("synthetic registry");
        let report = licoup_model_catalog::SelectionReport {
            target: "kimi-code".to_owned(),
            generation: Some("generation-1".to_owned()),
            observed_at_unix_ms: 1_726_000_000_000,
            entries: vec![
                licoup_model_catalog::selection_facts(
                    &registry,
                    "k3-256k",
                    Some("kimi-for-coding"),
                    Some("kimi-code"),
                    licoup_model_catalog::ObservedAvailability::Observed {
                        at_unix_ms: 1_726_000_000_000,
                        sources: vec!["codex-app-server".to_owned()],
                    },
                    vec!["kimi-for-coding".to_owned()],
                ),
                licoup_model_catalog::unknown_facts("mystery-preview"),
            ],
        };
        // A host that declares the Agent and composes no policy owner.
        let matrix = licoup_model_catalog::selection_matrix(
            &port,
            &crate::model_catalog_port::selection_matrix_port(),
            true,
            &report,
            &json!({}),
        );
        assert_eq!(matrix.agent, "kimi-code");

        let document = licoup_model_catalog::selection_matrix_document(&matrix);
        assert_eq!(document["schemaVersion"], 1);
        assert_eq!(document["scopes"], json!(["direct", "workflow"]));

        // A declared, observed, credentialed model: supported and available —
        // and still not executable, because no policy owner admits it.
        let declared = &document["entries"][0];
        assert_eq!(declared["model"], "k3-256k");
        assert_eq!(declared["canonicalId"], "moonshotai/kimi-k3");
        assert_eq!(declared["support"]["state"], "supported");
        assert_eq!(declared["availability"]["state"], "observed");
        assert_eq!(declared["credentials"]["state"], "present");
        assert_eq!(declared["scopes"][0]["state"], "undetermined");
        assert_eq!(declared["scopes"][1]["state"], "undetermined");
        assert_eq!(
            declared["scopes"][0]["reason"],
            "selection_policy_owner_absent"
        );

        // A name nothing declares: neither identity nor availability. Unknown
        // is reported as unknown and never as unsupported or ready.
        let undeclared = &document["entries"][1];
        assert_eq!(undeclared["canonicalId"], serde_json::Value::Null);
        assert_eq!(undeclared["evidence"], "unknown");
        assert_eq!(undeclared["support"]["state"], "unsupported");
        assert_eq!(
            undeclared["support"]["reason"],
            "nothing_declares_the_model"
        );
        assert_eq!(undeclared["availability"]["state"], "unobserved");
        assert_eq!(
            undeclared["availability"]["reason"],
            "not_observed_on_this_host"
        );
        assert_eq!(
            undeclared["availability"]["observedAtUnixMs"],
            serde_json::Value::Null
        );
        assert_eq!(undeclared["credentials"]["state"], "unknown");
        assert_eq!(undeclared["credentials"]["reason"], "provider_not_recorded");
        assert_eq!(undeclared["scopes"][0]["state"], "undetermined");
        assert_eq!(undeclared["scopes"][1]["state"], "undetermined");
    }

    /// An Agent this host does not declare keeps unknown support even when a
    /// name resolves: the host cannot claim a combination it does not know.
    #[test]
    fn an_undeclared_agent_keeps_unknown_support() {
        let registry = licoup_model_catalog::RegistrySnapshot::from_catalog_with_wrappers(
            json!({
                "models": { "moonshotai/kimi-k3": { "name": "Kimi K3" } },
                "providers": {}
            }),
            Vec::<String>::new(),
        )
        .expect("synthetic registry");
        let report = licoup_model_catalog::SelectionReport {
            target: "kimi-code".to_owned(),
            generation: None,
            observed_at_unix_ms: 0,
            entries: vec![licoup_model_catalog::declared_facts(
                &registry,
                "moonshotai/kimi-k3",
                None,
                Some("kimi-code"),
            )],
        };
        let matrix = licoup_model_catalog::selection_matrix(
            &licoup_model_catalog::ModelCatalogPort::unavailable(),
            &crate::model_catalog_port::selection_matrix_port(),
            false,
            &report,
            &json!({}),
        );
        let document = licoup_model_catalog::selection_matrix_document(&matrix);
        assert_eq!(document["entries"][0]["support"]["state"], "unknown");
        assert_eq!(
            document["entries"][0]["support"]["reason"],
            "agent_not_declared_on_host"
        );
        assert_eq!(
            document["entries"][0]["availability"]["state"],
            "unobserved"
        );
    }
}
