use crate::engine::CatalogConvergenceEngine;
use crate::model::{CatalogFetchedSnapshot, CatalogToolEntry, InvalidationNotification};
use crate::receipt::{ReceiptContext, build_official_client_receipt};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::sync::OnceLock;

static ENGINE: OnceLock<CatalogConvergenceEngine> = OnceLock::new();

pub fn dispatch(args: &[String], params: &Value) -> Result<Value> {
    dispatch_with_engine(engine(), args, params)
}

pub fn dispatch_with_engine(
    engine: &CatalogConvergenceEngine,
    args: &[String],
    params: &Value,
) -> Result<Value> {
    match args.get(1).map(String::as_str).unwrap_or("status") {
        "status" => Ok(engine.state()),
        "invalidate" => handle_invalidate(engine, params),
        "refresh" => handle_refresh(engine, params),
        "receipt" => handle_receipt(params),
        "purge" => handle_purge(engine, params),
        "reconnect" => Ok(engine.begin_reconnect()),
        "list" => handle_list(engine, params),
        "observe" => handle_observe(engine, params),
        other => bail!("catalog command is unsupported: {other}"),
    }
}

fn handle_invalidate(engine: &CatalogConvergenceEngine, params: &Value) -> Result<Value> {
    let notification = InvalidationNotification {
        affected_partitions: string_list(params, "affectedPartitions"),
        partition_key: optional_string(params, "partitionKey"),
        source_revision: required_i64(params, "sourceRevision")?,
        catalog_revision: required_string(params, "catalogRevision")?,
        audience_revision: required_i64(params, "audienceRevision")?,
        reason_code: optional_string(params, "reasonCode").unwrap_or_default(),
    };
    let result = engine.apply_invalidation(notification);
    Ok(serde_json::to_value(result)?)
}

fn handle_refresh(engine: &CatalogConvergenceEngine, params: &Value) -> Result<Value> {
    let partition_key = required_string(params, "partitionKey")?;
    let fetched = CatalogFetchedSnapshot {
        source_revision: required_i64(params, "sourceRevision")?,
        catalog_revision: required_string(params, "catalogRevision")?,
        audience_revision: required_i64(params, "audienceRevision")?,
        tools: tool_list(params),
    };
    let result = engine.replace_partition(&partition_key, fetched);
    Ok(serde_json::to_value(result)?)
}

fn handle_receipt(params: &Value) -> Result<Value> {
    let context = ReceiptContext {
        target: required_string(params, "target")?,
        platform: required_string(params, "platform")?,
        runtime: required_string(params, "runtime")?,
        source_digest: required_string(params, "sourceDigest")?,
        negotiated_capability: required_string(params, "negotiatedCapability")?,
        opaque_partition_key: required_string(params, "opaquePartitionKey")?,
        source_revision: required_i64(params, "sourceRevision")?,
        catalog_revision: required_string(params, "catalogRevision")?,
        audience_revision: required_i64(params, "audienceRevision")?,
        applied_revision: required_i64(params, "appliedRevision")?,
        cache_digest: required_string(params, "cacheDigest")?,
        cohort_outcome: required_string(params, "cohortOutcome")?,
        ui_observed_revision: required_i64(params, "uiObservedRevision")?,
        restart_ok: required_bool(params, "restartOk")?,
        restart_reason_code: required_string(params, "restartReasonCode")?,
        observed_at: optional_string(params, "observedAt"),
    };
    let receipt = build_official_client_receipt(context)?;
    Ok(serde_json::to_value(receipt)?)
}

fn handle_purge(engine: &CatalogConvergenceEngine, params: &Value) -> Result<Value> {
    if let Some(partition_key) = optional_string(params, "partitionKey") {
        let removed = engine.remove_partition(&partition_key);
        return Ok(json!({ "removed": removed, "partitionKey": partition_key }));
    }
    engine.purge_all();
    Ok(json!({ "purged": true }))
}

fn handle_list(engine: &CatalogConvergenceEngine, params: &Value) -> Result<Value> {
    let partition_key = required_string(params, "partitionKey")?;
    Ok(serde_json::to_value(engine.list_tools(&partition_key))?)
}

fn handle_observe(engine: &CatalogConvergenceEngine, params: &Value) -> Result<Value> {
    let partition_key = required_string(params, "partitionKey")?;
    let revision = engine.observe_ui_revision(&partition_key);
    Ok(json!({
        "observed": revision.is_some(),
        "uiObservedRevision": revision.unwrap_or(-1),
    }))
}

fn engine() -> &'static CatalogConvergenceEngine {
    ENGINE.get_or_init(CatalogConvergenceEngine::default)
}

fn required_string(params: &Value, key: &str) -> Result<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("catalog_missing_field:{key}"))
}

fn optional_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn required_i64(params: &Value, key: &str) -> Result<i64> {
    params
        .get(key)
        .and_then(read_i64)
        .ok_or_else(|| anyhow::anyhow!("catalog_missing_field:{key}"))
}

fn required_bool(params: &Value, key: &str) -> Result<bool> {
    params
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("catalog_missing_field:{key}"))
}

fn read_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|raw| i64::try_from(raw).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
}

fn string_list(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn tool_list(params: &Value) -> Vec<CatalogToolEntry> {
    params
        .get("tools")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let object = item.as_object()?;
                    let name = object.get("name").and_then(Value::as_str)?.trim();
                    if name.is_empty() {
                        None
                    } else {
                        Some(CatalogToolEntry {
                            name: name.to_string(),
                            descriptor: object
                                .iter()
                                .filter(|(key, _)| key.as_str() != "name")
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect(),
                        })
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
