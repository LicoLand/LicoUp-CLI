//! Product answers the statistics engine consumes through its host port.
//!
//! Which agents carry usage capability is decided by the agent catalog and the
//! conversation source catalog; how one agent's history summarizes is owned by
//! the agent readers beside this module; target readiness comes from the target
//! scan. The generic engine asks; this composition answers.

use super::{agent_usage_codex, agent_usage_native};
use agent_statistics::contract::{AgentDef, HistoryUsageSummary};
use agent_statistics::host::StatisticsHost;
use agent_statistics::registry::{ModelRegistry, ResolvedModel};
use agent_statistics::window::UsageWindow;
use agent_usage_native::runtime::CacheRuntime;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

static CACHE_RUNTIME: OnceLock<CacheRuntime> = OnceLock::new();

fn cache_runtime() -> &'static CacheRuntime {
    CACHE_RUNTIME.get_or_init(CacheRuntime::new)
}

/// The statistics engine's model-identity port, answered by this host's own
/// catalogue snapshot.
///
/// The catalogue data remains LicoUp's: the snapshot is the same revision the
/// model picker and target scan read, captured once per report so every
/// selector in one report resolves against one catalog. Only the questions are
/// the engine's.
struct CatalogModelRegistry {
    snapshot: Arc<crate::domain::model_registry::RegistrySnapshot>,
}

impl CatalogModelRegistry {
    fn new(snapshot: Arc<crate::domain::model_registry::RegistrySnapshot>) -> Self {
        Self { snapshot }
    }
}

fn resolved(model: &crate::domain::model_registry::CanonicalModel) -> ResolvedModel {
    ResolvedModel {
        id: model.id.clone(),
        display_name: model.display_name.clone(),
    }
}

impl ModelRegistry for CatalogModelRegistry {
    fn revision(&self) -> &str {
        self.snapshot.revision()
    }

    fn resolve_historical(
        &self,
        raw: &str,
        provider_id: Option<&str>,
        source_agent_id: Option<&str>,
    ) -> Option<ResolvedModel> {
        self.snapshot
            .resolve_historical(raw, provider_id, source_agent_id)
            .map(resolved)
    }

    fn canonical_model(&self, id: &str) -> Option<ResolvedModel> {
        self.snapshot.canonical_model(id).map(resolved)
    }

    fn is_provider_label(&self, value: &str) -> bool {
        self.snapshot.is_provider_label(value)
    }

    fn display_name(&self, raw: &str) -> String {
        licoup_foundation::core::model_naming::model_display_name(raw)
    }
}

/// Wrap an already captured catalogue snapshot in the host adapter.
pub(super) fn model_registry_for_snapshot(
    snapshot: Arc<crate::domain::model_registry::RegistrySnapshot>,
) -> Arc<dyn ModelRegistry> {
    Arc::new(CatalogModelRegistry::new(snapshot))
}

/// Capture the catalogue for the state root this report names and answer the
/// engine through the host adapter rather than its fail-closed default.
pub(super) fn model_registry_snapshot(state_root: Option<&Path>) -> Arc<dyn ModelRegistry> {
    model_registry_for_snapshot(
        crate::domain::model_registry::refresh_cached_snapshot_for_state_root(state_root),
    )
}

/// Install the product answers once per process. A process that never installs
/// leaves the statistics engine reporting an empty catalog rather than
/// inventing one.
pub fn install() -> Result<(), &'static str> {
    agent_statistics::install_host(StatisticsHost {
        supported_agents,
        summarize_agent_history,
        target_status: target_status_map,
        model_registry_snapshot,
    })
}

fn supported_agents() -> Vec<AgentDef> {
    crate::domain::agent_catalog::entries()
        .into_iter()
        .filter(|entry| {
            entry.has_adapter
                || crate::domain::conversation::source_catalog::usage_adapter_for_agent(&entry.id)
                    .is_some_and(|adapter| {
                        adapter != crate::domain::conversation::source_catalog::HistoryAdapter::Code
                    })
        })
        .filter_map(|entry| {
            let (id, label) = if let Ok(target) = crate::domain::targets::target_def(&entry.id) {
                (target.id, target.label)
            } else {
                let registration = crate::domain::cli_registration::registrations()
                    .iter()
                    .find(|registration| registration.id == entry.id)?;
                (registration.id.as_str(), registration.label.as_str())
            };
            Some(AgentDef {
                id,
                label: label
                    .strip_suffix(" CLI")
                    .or_else(|| label.strip_suffix(" Desktop"))
                    .unwrap_or(label),
            })
        })
        .collect()
}

fn summarize_agent_history(
    def: &AgentDef,
    params: &Value,
    window: &UsageWindow,
    warnings: &mut Vec<Value>,
) -> Result<HistoryUsageSummary> {
    if def.id == "codex" {
        return Ok(agent_usage_codex::summarize(params, window, warnings).unwrap_or_default());
    }
    if crate::domain::conversation::source_catalog::usage_adapter_for_agent(def.id).is_none() {
        return Ok(HistoryUsageSummary {
            source: Some("native-usage-source-unavailable"),
            skipped: vec![json!({"code": "native_usage_source_unavailable", "agentId": def.id})],
            ..HistoryUsageSummary::default()
        });
    }
    agent_usage_native::summarize(def, params, window, warnings, cache_runtime())
}

fn target_status_map(params: &Value, warnings: &mut Vec<Value>) -> BTreeMap<String, String> {
    let mut scan_params = params.clone();
    if let Some(object) = scan_params.as_object_mut() {
        object.insert("includeHistoryModelCatalog".to_owned(), json!(false));
    }
    match crate::domain::targets::scan_targets_with_params(
        &crate::target_port::agent_target_port(),
        &scan_params,
    ) {
        Ok(scan) => scan
            .get("candidates")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some((
                            item.get("target")?.as_str()?.to_owned(),
                            item.get("status")?.as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => {
            warnings.push(json!({"code": "target_scan_failed"}));
            BTreeMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CatalogModelRegistry {
        let snapshot = crate::domain::model_registry::RegistrySnapshot::from_catalog_with_wrappers(
            json!({
                "models": {
                    "example/model-1": {"name": "Example Model 1"},
                },
                "providers": {
                    "example-provider": {
                        "name": "Example Provider",
                        "models": {
                            "model-1": {"base_model": "example/model-1"},
                        },
                    },
                },
            }),
            ["example-agent"],
        )
        .unwrap();
        CatalogModelRegistry::new(Arc::new(snapshot))
    }

    #[test]
    fn host_adapter_answers_the_registry_port_from_the_catalog_snapshot() {
        let registry = registry();
        assert!(
            !registry.revision().is_empty(),
            "a real catalog has a revision"
        );

        let canonical = registry.canonical_model("example/model-1").unwrap();
        assert_eq!(canonical.id, "example/model-1");
        assert_eq!(canonical.display_name, "Example Model 1");

        let historical = registry
            .resolve_historical("model-1", Some("example-provider"), Some("example-agent"))
            .unwrap();
        assert_eq!(historical.id, "example/model-1");
        assert!(registry.is_provider_label("example-provider"));
        assert!(!registry.is_provider_label("example/model-1"));
    }

    #[test]
    fn host_adapter_reuses_the_foundation_display_projection() {
        let registry = registry();
        assert_eq!(
            registry.display_name("gpt-5-codex"),
            licoup_foundation::core::model_naming::model_display_name("gpt-5-codex")
        );
    }
}
