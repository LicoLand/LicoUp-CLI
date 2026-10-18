use super::catalog::{TargetCandidate, TargetDef, normalize_target, target_def, target_defs};
use super::manual::{ManualTarget, manual_targets, manual_targets_read_only};
use super::parameters::target_param;
use super::probe_pool::{run_bounded_target_probes, target_scan_concurrency};
use super::processes::ScanContext;
use super::scan_merge::{scan_target_read_only_with_manual, scan_target_with_manual};
use super::support::{client_state_store, client_state_store_read_only};
use super::target_cache::{
    hydrate_model_catalog, hydrate_model_catalogs, persist_discovery_cache, upsert_discovery_cache,
    upsert_discovery_cache_many,
};
use super::virtual_machine_discovery::{AutomaticVmTarget, discover_virtual_machine_targets};
use crate::port::AgentTargetPort;
use anyhow::{Result, ensure};
use licoup_client_state::ClientStateStore;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_SELECTED_TARGETS: usize = 64;

#[derive(Clone, Debug)]
struct TargetProbe {
    def: TargetDef,
    manual: Option<ManualTarget>,
    automatic_vm: Option<AutomaticVmTarget>,
    scan_context: ScanContext,
    params: Arc<Value>,
}

#[derive(Clone, Debug)]
struct SelectedTarget {
    id: String,
    definition: Option<TargetDef>,
    model_catalog_lookup: bool,
}

pub(super) fn scan_targets(port: &AgentTargetPort) -> Result<Value> {
    scan_targets_with_params(port, &json!({}))
}

pub(super) fn scan_targets_with_params(port: &AgentTargetPort, params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    scan_targets_with_store(port, params, &store)
}

pub(super) fn scan_targets_with_store(
    port: &AgentTargetPort,
    params: &Value,
    store: &ClientStateStore,
) -> Result<Value> {
    let manual_targets = manual_targets(store)?;
    let manual_by_target = manual_targets
        .into_iter()
        .map(|target| (target.target.clone(), target))
        .collect::<BTreeMap<_, _>>();
    let process_snapshot = ScanContext::snapshot_from_params(params);
    let selected = selected_targets(params)?;
    let definitions = selected
        .as_ref()
        .map(|targets| {
            targets
                .iter()
                .filter_map(|target| target.definition.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(target_defs);
    let requested_targets = definitions.iter().map(|def| def.id).collect::<Vec<_>>();
    let automatic_vm = discover_virtual_machine_targets(params, &requested_targets);
    let probes = definitions
        .into_iter()
        .map(|def| {
            let mut target_params = params.clone();
            let model_catalog_lookup = selected.as_ref().is_some_and(|targets| {
                targets
                    .iter()
                    .find(|target| target.id == def.id)
                    .is_some_and(|target| target.model_catalog_lookup)
            });
            if selected.is_some()
                && let Some(object) = target_params.as_object_mut()
            {
                object.insert(
                    "enableAgentCliModelLookup".to_string(),
                    json!(model_catalog_lookup),
                );
            }
            TargetProbe {
                manual: manual_by_target.get(def.id).cloned(),
                automatic_vm: automatic_vm.targets.get(def.id).cloned(),
                def,
                scan_context: process_snapshot.clone(),
                params: Arc::new(target_params),
            }
        })
        .collect::<Vec<_>>();
    let concurrency = target_scan_concurrency(params, probes.len());
    if selected.is_some() {
        let outcomes = run_bounded_target_probes(probes, concurrency, |mut probe| {
            let target_id = probe.def.id.to_string();
            let candidate = scan_target_with_manual(
                port,
                &probe.def,
                probe.manual.as_ref(),
                probe.automatic_vm.as_ref(),
                &mut probe.scan_context,
                probe.params.as_ref(),
            )
            .ok();
            Ok((target_id, candidate))
        })?;
        let mut outcomes = outcomes;
        for (_, candidate) in outcomes.iter_mut() {
            if let Some(candidate) = candidate.as_mut() {
                hydrate_model_catalog(port, store, candidate);
            }
        }
        let successful = outcomes
            .iter()
            .filter_map(|(_, candidate)| candidate.as_ref())
            .collect::<Vec<_>>();
        upsert_discovery_cache_many(store, &successful)?;
        let mut outcomes = outcomes.into_iter().collect::<BTreeMap<_, _>>();
        let results = selected
            .as_ref()
            .expect("selected scan has selected slots")
            .iter()
            .map(|selected| match outcomes.remove(&selected.id).flatten() {
                Some(candidate) => json!({
                    "targetId": selected.id,
                    "ok": true,
                    "candidate": candidate,
                }),
                None => json!({
                    "targetId": selected.id,
                    "ok": false,
                    "error": { "code": "target_scan_failed" },
                }),
            })
            .collect::<Vec<_>>();
        return Ok(json!({
            "ok": true,
            "schemaVersion": 1,
            "source": "target-adapters",
            "diagnostics": automatic_vm.diagnostics,
            "results": results,
        }));
    }
    let mut candidates = run_bounded_target_probes(probes, concurrency, |mut probe| {
        scan_target_with_manual(
            port,
            &probe.def,
            probe.manual.as_ref(),
            probe.automatic_vm.as_ref(),
            &mut probe.scan_context,
            probe.params.as_ref(),
        )
    })?;
    hydrate_model_catalogs(port, store, &mut candidates);
    persist_discovery_cache(store, &candidates)?;
    let mut scan_scopes = vec![
        "application-store",
        "package-manager",
        "executable-path",
        "local-configuration",
        "running-process",
    ];
    if automatic_vm.scope_available {
        scan_scopes.push("virtual-machine-orbstack");
    }
    Ok(json!({
        "ok": true,
        "schemaVersion": 1,
        "source": "target-adapters",
        "scanScopes": scan_scopes,
        "diagnostics": automatic_vm.diagnostics,
        "candidates": candidates,
    }))
}

fn selected_targets(params: &Value) -> Result<Option<Vec<SelectedTarget>>> {
    let Some(values) = params.get("targetIds") else {
        return Ok(None);
    };
    let values = values
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("target_selection_invalid"))?;
    ensure!(
        values.len() <= MAX_SELECTED_TARGETS,
        "target_selection_limit"
    );
    let lookup_values = params
        .get("modelCatalogTargetIds")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("target_selection_invalid"))
        })
        .transpose()?
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    ensure!(
        lookup_values.len() <= MAX_SELECTED_TARGETS,
        "target_selection_limit"
    );
    let lookup_ids = lookup_values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(normalize_target)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| anyhow::anyhow!("target_selection_invalid"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    for value in values {
        let id = value
            .as_str()
            .map(normalize_target)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("target_selection_invalid"))?;
        if seen.insert(id.clone()) {
            selected.push(SelectedTarget {
                model_catalog_lookup: lookup_ids.contains(&id),
                definition: target_def(&id).ok(),
                id,
            });
        }
    }
    Ok(Some(selected))
}

pub(super) fn inspect_target(port: &AgentTargetPort, target: &str) -> Result<Value> {
    inspect_target_with_params(port, &json!({ "target": target }))
}

pub(super) fn inspect_target_read_only(port: &AgentTargetPort, target: &str) -> Result<Value> {
    inspect_target_read_only_with_params(port, &json!({ "target": target }))
}

pub(super) fn inspect_target_read_only_with_params(
    port: &AgentTargetPort,
    params: &Value,
) -> Result<Value> {
    let mut params = params.clone();
    if let Some(object) = params.as_object_mut() {
        object.insert("disableAgentCliModelLookup".to_string(), json!(true));
        object.insert("enableAgentCliModelLookup".to_string(), json!(false));
        object.insert("probeConversationRuntime".to_string(), json!(false));
        object.insert("includeHistoryModelCatalog".to_string(), json!(false));
        object.insert("includeAccessibleEnvironments".to_string(), json!(false));
    }
    inspect_target_inner(port, &params, true)
}

pub(super) fn inspect_target_with_params(port: &AgentTargetPort, params: &Value) -> Result<Value> {
    inspect_target_inner(port, params, false)
}

fn inspect_target_inner(port: &AgentTargetPort, params: &Value, read_only: bool) -> Result<Value> {
    let target = target_param(params)?;
    let def = target_def(&target)?;
    let store = if read_only {
        client_state_store_read_only(params)?
    } else {
        client_state_store(params)?
    };
    let manual_targets = if read_only {
        manual_targets_read_only(&store)?
    } else {
        manual_targets(&store)?
    };
    let manual = manual_targets.iter().find(|item| item.target == def.id);
    let automatic_vm = discover_virtual_machine_targets(params, &[def.id]);
    let mut scan_context = ScanContext::from_params(params);
    let mut candidate = if read_only {
        scan_target_read_only_with_manual(
            port,
            &def,
            manual,
            automatic_vm.targets.get(def.id),
            &mut scan_context,
            params,
        )?
    } else {
        scan_target_with_manual(
            port,
            &def,
            manual,
            automatic_vm.targets.get(def.id),
            &mut scan_context,
            params,
        )?
    };
    if !read_only {
        hydrate_model_catalog(port, &store, &mut candidate);
        upsert_discovery_cache(&store, &candidate)?;
    }
    let target = if read_only {
        read_only_target_projection(&candidate)
    } else {
        serde_json::to_value(candidate)?
    };
    Ok(json!({
        "ok": true,
        "diagnostics": automatic_vm.diagnostics,
        "target": target
    }))
}

fn read_only_target_projection(candidate: &TargetCandidate) -> Value {
    let model = candidate
        .model_catalog
        .as_ref()
        .and_then(|catalog| catalog.get("defaultModel"))
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty());
    json!({
        "target": candidate.target,
        "status": candidate.status,
        "model": model,
        "location": candidate.location,
        "adapterCapabilities": {
            "conversationDriver": candidate.adapter_capabilities.conversation_driver,
            "conversationReadiness": candidate.adapter_capabilities.conversation_readiness,
            "conversationBlocker": candidate.adapter_capabilities.conversation_blocker,
            "conversationConsecutivePasses": candidate.adapter_capabilities.conversation_consecutive_passes,
        },
        "supportedActions": candidate.supported_actions,
    })
}
