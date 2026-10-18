//! Deterministic preview/apply binding for local conversation backups.

use anyhow::{Result, ensure};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::request::{merge_params, normalize_request, text_param};
use crate::domain::{conversation_snapshots, targets};

pub(super) struct PreparedArchivePlan {
    pub(super) request: Value,
    pub(super) target_scan: Value,
    pub(super) plan: Value,
    pub(super) binding: String,
}

pub(super) fn prepare(params: &Value) -> Result<PreparedArchivePlan> {
    let request = normalize_request(params)?;
    let mut scan_params = request.clone();
    if let Some(object) = scan_params.as_object_mut() {
        object.insert("archiveMode".to_string(), json!(true));
    }
    let target_scan =
        targets::scan_targets_with_params(&crate::target_port::agent_target_port(), &scan_params)?;
    prepare_with_target_scan(request, target_scan)
}

pub(super) fn prepare_with_target_scan(
    request: Value,
    target_scan: Value,
) -> Result<PreparedArchivePlan> {
    let preview = conversation_snapshots::archive_selection_preview(&merge_params(
        &request,
        json!({"targetScan": target_scan}),
    ))?;
    let plan = bounded_plan(&preview);
    let binding = binding_for(&plan);
    Ok(PreparedArchivePlan {
        request,
        target_scan,
        plan,
        binding,
    })
}

pub(super) fn preview(params: &Value) -> Result<Value> {
    let prepared = prepare(params)?;
    Ok(json!({
        "ok": true,
        "mode": "conversation-archive-plan",
        "plan": with_binding(prepared.plan, &prepared.binding)
    }))
}

pub(super) fn require_matching_binding(
    params: &Value,
    prepared: &PreparedArchivePlan,
) -> Result<()> {
    let supplied = text_param(params, &["planBinding"])
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    ensure!(
        !supplied.is_empty(),
        "conversation archive create requires --plan-binding"
    );
    ensure!(
        supplied == prepared.binding,
        "conversation archive plan binding no longer matches source, query, destination, count, or conflict"
    );
    Ok(())
}

pub(super) fn validate_stored_plan(request: &Value, target_scan: &Value) -> Result<()> {
    let expected = text_param(request, &["planBinding"]).unwrap_or_default();
    let prepared = prepare_with_target_scan(request.clone(), target_scan.clone())?;
    ensure!(
        !expected.is_empty() && expected == prepared.binding,
        "conversation archive plan changed before execution"
    );
    Ok(())
}

pub(super) fn request_with_plan(prepared: PreparedArchivePlan) -> (Value, Value) {
    let mut request = prepared.request.as_object().cloned().unwrap_or_default();
    request.insert("plan".to_string(), prepared.plan);
    request.insert("planBinding".to_string(), json!(prepared.binding));
    (Value::Object(request), prepared.target_scan)
}

fn bounded_plan(preview: &Value) -> Value {
    let mut plan = Map::new();
    for key in [
        "selectionMode",
        "source",
        "query",
        "destination",
        "count",
        "conflict",
        "conflictPolicy",
        "collectionKey",
    ] {
        plan.insert(
            key.to_string(),
            preview.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(plan)
}

fn with_binding(plan: Value, binding: &str) -> Value {
    let mut object = plan.as_object().cloned().unwrap_or_default();
    object.insert("binding".to_string(), json!(binding));
    Value::Object(object)
}

fn binding_for(plan: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lico-conversation-archive-plan-v1\0");
    hasher.update(serde_json::to_vec(plan).unwrap_or_default());
    format!("sha256:{:x}", hasher.finalize())
}
