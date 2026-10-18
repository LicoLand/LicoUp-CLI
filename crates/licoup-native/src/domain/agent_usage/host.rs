//! Product answers the statistics engine consumes through its host port.
//!
//! Which agents carry usage capability is decided by the agent catalog and the
//! conversation source catalog; how one agent's history summarizes is owned by
//! the agent readers beside this module; target readiness comes from the target
//! scan. The generic engine asks; this composition answers.

use super::{agent_usage_codex, agent_usage_native};
use agent_usage_native::runtime::CacheRuntime;
use anyhow::Result;
use licoup_statistics::contract::{AgentDef, HistoryUsageSummary};
use licoup_statistics::host::StatisticsHost;
use licoup_statistics::window::UsageWindow;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

static CACHE_RUNTIME: OnceLock<CacheRuntime> = OnceLock::new();

fn cache_runtime() -> &'static CacheRuntime {
    CACHE_RUNTIME.get_or_init(CacheRuntime::new)
}

/// Install the product answers once per process. A process that never installs
/// leaves the statistics engine reporting an empty catalog rather than
/// inventing one.
pub fn install() -> Result<(), &'static str> {
    licoup_statistics::install_host(StatisticsHost {
        supported_agents,
        summarize_agent_history,
        target_status: target_status_map,
        model_registry_snapshot:
            crate::domain::model_registry::refresh_cached_snapshot_for_state_root,
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
    Ok(
        agent_usage_native::summarize(def, params, window, warnings, cache_runtime())
            .unwrap_or_default(),
    )
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
