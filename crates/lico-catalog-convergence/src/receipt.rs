use crate::model::{
    ALLOWED_CLIENT_TARGETS, OFFICIAL_CLIENT_RECEIPT_SCHEMA, OfficialClientReceipt, OutcomeRecord,
    is_hex_digest, is_opaque_partition_key, revision_number, sha256_hex,
};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const PRIVACY_FORBIDDEN_KEYS: &[&str] = &[
    "tools",
    "catalog",
    "catalogEntries",
    "catalogTools",
    "tag",
    "tags",
    "tagRows",
    "tagPolicy",
    "credential",
    "credentials",
    "secret",
    "secrets",
    "token",
    "tokens",
    "bearer",
    "bearerToken",
    "certificate",
    "certificates",
    "cert",
    "certPem",
    "pem",
    "filePath",
    "filesystemPath",
    "absolutePath",
    "cwd",
    "workingDirectory",
    "grant",
    "grants",
    "manifest",
    "upstreamUrl",
    "baseUrl",
];

#[derive(Debug, Clone)]
pub struct ReceiptContext {
    pub target: String,
    pub platform: String,
    pub runtime: String,
    pub source_digest: String,
    pub negotiated_capability: String,
    pub opaque_partition_key: String,
    pub source_revision: i64,
    pub catalog_revision: String,
    pub audience_revision: i64,
    pub applied_revision: i64,
    pub cache_digest: String,
    pub cohort_outcome: String,
    pub ui_observed_revision: i64,
    pub restart_ok: bool,
    pub restart_reason_code: String,
    pub observed_at: Option<String>,
}

impl ReceiptContext {
    pub fn validate_inputs(&self) -> Result<()> {
        let target = self.target.trim().to_ascii_lowercase();
        ensure!(
            ALLOWED_CLIENT_TARGETS.contains(&target.as_str()),
            "official_client_receipt_unknown_target"
        );
        ensure!(
            is_hex_digest(&self.source_digest),
            "official_client_receipt_missing_field"
        );
        ensure!(
            is_hex_digest(&self.cache_digest),
            "official_client_receipt_missing_field"
        );
        ensure!(
            is_opaque_partition_key(&self.opaque_partition_key),
            "official_client_receipt_missing_field"
        );
        ensure!(
            revision_number(self.source_revision) >= 0
                && revision_number(self.audience_revision) >= 0
                && revision_number(self.applied_revision) >= 0
                && revision_number(self.ui_observed_revision) >= 0
                && !self.catalog_revision.trim().is_empty(),
            "official_client_receipt_missing_field"
        );
        ensure!(
            self.applied_revision >= self.audience_revision
                && self.ui_observed_revision >= self.applied_revision,
            "official_client_receipt_stale_revision"
        );
        ensure!(
            matches!(
                self.cohort_outcome.as_str(),
                "applied" | "disconnected" | "fenced"
            ),
            "official_client_receipt_cohort_invalid"
        );
        Ok(())
    }
}

pub fn build_summary_digest(context: &ReceiptContext) -> Result<String> {
    context.validate_inputs()?;
    let payload = canonical_json(&BTreeMap::from([
        (
            "appliedRevision".to_string(),
            Value::from(revision_number(context.applied_revision)),
        ),
        (
            "audienceRevision".to_string(),
            Value::from(revision_number(context.audience_revision)),
        ),
        (
            "cacheDigest".to_string(),
            Value::String(context.cache_digest.trim().to_string()),
        ),
        (
            "catalogRevision".to_string(),
            Value::String(context.catalog_revision.trim().to_string()),
        ),
        (
            "cohortOutcome".to_string(),
            Value::String(context.cohort_outcome.trim().to_string()),
        ),
        (
            "opaquePartitionKey".to_string(),
            Value::String(context.opaque_partition_key.trim().to_string()),
        ),
        (
            "sourceDigest".to_string(),
            Value::String(context.source_digest.trim().to_string()),
        ),
        (
            "sourceRevision".to_string(),
            Value::from(revision_number(context.source_revision)),
        ),
        (
            "target".to_string(),
            Value::String(context.target.trim().to_ascii_lowercase()),
        ),
        (
            "uiObservedRevision".to_string(),
            Value::from(revision_number(context.ui_observed_revision)),
        ),
    ]));
    Ok(sha256_hex(&payload))
}

pub fn build_receipt_digest(context: &ReceiptContext, summary_digest: &str) -> Result<String> {
    context.validate_inputs()?;
    scan_privacy_map(&context_to_map(context)?)?;
    let restart_result =
        outcome_record(context.restart_ok, &context.restart_reason_code, "restart");
    let privacy_result = outcome_record(true, "privacy_safe", "privacy");
    let payload = canonical_json(&BTreeMap::from([
        (
            "appliedRevision".to_string(),
            Value::from(revision_number(context.applied_revision)),
        ),
        (
            "audienceRevision".to_string(),
            Value::from(revision_number(context.audience_revision)),
        ),
        (
            "cacheDigest".to_string(),
            Value::String(context.cache_digest.trim().to_string()),
        ),
        (
            "catalogRevision".to_string(),
            Value::String(context.catalog_revision.trim().to_string()),
        ),
        (
            "cohortOutcome".to_string(),
            Value::String(context.cohort_outcome.trim().to_string()),
        ),
        (
            "negotiatedCapability".to_string(),
            Value::String(context.negotiated_capability.trim().to_string()),
        ),
        (
            "opaquePartitionKey".to_string(),
            Value::String(context.opaque_partition_key.trim().to_string()),
        ),
        (
            "platform".to_string(),
            Value::String(context.platform.trim().to_string()),
        ),
        ("privacyResult".to_string(), privacy_result),
        ("restartResult".to_string(), restart_result),
        (
            "runtime".to_string(),
            Value::String(context.runtime.trim().to_string()),
        ),
        (
            "schemaVersion".to_string(),
            Value::String(OFFICIAL_CLIENT_RECEIPT_SCHEMA.to_string()),
        ),
        (
            "sourceDigest".to_string(),
            Value::String(context.source_digest.trim().to_string()),
        ),
        (
            "sourceRevision".to_string(),
            Value::from(revision_number(context.source_revision)),
        ),
        (
            "summaryDigest".to_string(),
            Value::String(summary_digest.trim().to_string()),
        ),
        (
            "target".to_string(),
            Value::String(context.target.trim().to_ascii_lowercase()),
        ),
        (
            "uiObservedRevision".to_string(),
            Value::from(revision_number(context.ui_observed_revision)),
        ),
    ]));
    Ok(sha256_hex(&payload))
}

pub fn build_official_client_receipt(context: ReceiptContext) -> Result<OfficialClientReceipt> {
    context.validate_inputs()?;
    scan_privacy_map(&context_to_map(&context)?)?;
    let summary_digest = build_summary_digest(&context)?;
    let receipt_digest = build_receipt_digest(&context, &summary_digest)?;
    let restart_reason = if context.restart_reason_code.trim().is_empty() {
        if context.restart_ok {
            "restart_recovered".to_string()
        } else {
            "restart_failed".to_string()
        }
    } else {
        context.restart_reason_code.trim().to_string()
    };
    Ok(OfficialClientReceipt {
        schema_version: OFFICIAL_CLIENT_RECEIPT_SCHEMA.to_string(),
        target: context.target.trim().to_ascii_lowercase(),
        platform: context.platform.trim().to_string(),
        runtime: context.runtime.trim().to_string(),
        source_digest: context.source_digest.trim().to_string(),
        negotiated_capability: context.negotiated_capability.trim().to_string(),
        opaque_partition_key: context.opaque_partition_key.trim().to_string(),
        source_revision: revision_number(context.source_revision),
        catalog_revision: context.catalog_revision.trim().to_string(),
        audience_revision: revision_number(context.audience_revision),
        applied_revision: revision_number(context.applied_revision),
        cache_digest: context.cache_digest.trim().to_string(),
        cohort_outcome: context.cohort_outcome.trim().to_string(),
        ui_observed_revision: revision_number(context.ui_observed_revision),
        restart_result: OutcomeRecord {
            ok: context.restart_ok,
            reason_code: restart_reason,
        },
        privacy_result: OutcomeRecord {
            ok: true,
            reason_code: "privacy_safe".to_string(),
        },
        summary_digest,
        receipt_digest,
        observed_at: context.observed_at,
    })
}

pub fn scan_privacy_text(value: &str) -> Result<()> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Ok(());
    }
    if normalized.starts_with("Bearer ") || normalized.starts_with("bearer ") {
        return Err(anyhow!("official_client_receipt_privacy_unsafe"));
    }
    if normalized.contains("-----BEGIN") {
        return Err(anyhow!("official_client_receipt_privacy_unsafe"));
    }
    if is_sensitive_unix_absolute_path(normalized) {
        return Err(anyhow!("official_client_receipt_privacy_unsafe"));
    }
    if is_windows_absolute_path(normalized) {
        return Err(anyhow!("official_client_receipt_privacy_unsafe"));
    }
    Ok(())
}

fn is_sensitive_unix_absolute_path(value: &str) -> bool {
    let Some(relative) = value.strip_prefix('/') else {
        return false;
    };
    let Some((root, _remainder)) = relative.split_once('/') else {
        return false;
    };
    matches!(root, "Users" | "home" | "private")
}

fn is_windows_absolute_path(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(
        (characters.next(), characters.next(), characters.next()),
        (Some(drive), Some(':'), Some('\\')) if drive.is_ascii_alphabetic()
    )
}

pub fn scan_privacy_value(value: &Value) -> Result<()> {
    match value {
        Value::String(text) => scan_privacy_text(text),
        Value::Array(items) => {
            for item in items {
                scan_privacy_value(item)?;
            }
            Ok(())
        }
        Value::Object(map) => scan_privacy_map(map),
        _ => Ok(()),
    }
}

pub fn scan_privacy_map(map: &Map<String, Value>) -> Result<()> {
    for (key, value) in map {
        if PRIVACY_FORBIDDEN_KEYS.contains(&key.as_str()) {
            return Err(anyhow!("official_client_receipt_privacy_unsafe"));
        }
        if key == "path" {
            if let Value::String(text) = value {
                scan_privacy_text(text)?;
            }
        }
        scan_privacy_value(value)?;
    }
    Ok(())
}

fn outcome_record(ok: bool, reason_code: &str, label: &str) -> Value {
    let reason = reason_code.trim();
    let normalized = if reason.is_empty() {
        format!("{label}_unknown")
    } else {
        reason.to_string()
    };
    serde_json::json!({ "ok": ok, "reasonCode": normalized })
}

fn context_to_map(context: &ReceiptContext) -> Result<Map<String, Value>> {
    Ok(Map::from_iter([
        ("target".to_string(), Value::String(context.target.clone())),
        (
            "platform".to_string(),
            Value::String(context.platform.clone()),
        ),
        (
            "runtime".to_string(),
            Value::String(context.runtime.clone()),
        ),
        (
            "sourceDigest".to_string(),
            Value::String(context.source_digest.clone()),
        ),
        (
            "negotiatedCapability".to_string(),
            Value::String(context.negotiated_capability.clone()),
        ),
        (
            "opaquePartitionKey".to_string(),
            Value::String(context.opaque_partition_key.clone()),
        ),
        (
            "catalogRevision".to_string(),
            Value::String(context.catalog_revision.clone()),
        ),
        (
            "cacheDigest".to_string(),
            Value::String(context.cache_digest.clone()),
        ),
        (
            "cohortOutcome".to_string(),
            Value::String(context.cohort_outcome.clone()),
        ),
    ]))
}

fn canonical_json(map: &BTreeMap<String, Value>) -> String {
    serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string())
}
