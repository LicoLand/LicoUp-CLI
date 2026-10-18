//! Verification collection extraction and aggregate health reduction.

use serde_json::{Value, json};

use super::clock::timestamp;
use super::constants::ARCHIVE_JOB_SCHEMA_VERSION;

pub(super) fn archive_collection_paths(archive_result: &Value) -> Vec<String> {
    let mut paths = Vec::<String>::new();
    if let Some(archives) = archive_result.get("archives").and_then(Value::as_array) {
        for archive in archives {
            if let Some(path) = archive
                .get("collectionPath")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                paths.push(path.to_string());
            }
        }
    }
    if paths.is_empty() {
        if let Some(path) = archive_result
            .get("collectionPath")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            paths.push(path.to_string());
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

pub(super) fn aggregate_validations(collections: &[Value]) -> Value {
    if collections.len() == 1 {
        if let Some(validation) = collections[0].get("validation") {
            return validation.clone();
        }
    }
    let mut failed = false;
    let mut error_count = 0_u64;
    let mut warning_count = 0_u64;
    let mut record_count = 0_u64;
    let mut raw_content_bytes = 0_u64;
    let mut issues = Vec::<Value>::new();
    for collection in collections {
        let Some(validation) = collection.get("validation") else {
            failed = true;
            error_count += 1;
            continue;
        };
        failed = failed
            || validation
                .get("healthStatus")
                .and_then(Value::as_str)
                .is_some_and(|status| status == "failed");
        error_count += validation
            .get("errorCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        warning_count += validation
            .get("warningCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        record_count += validation
            .get("recordCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        raw_content_bytes += validation
            .get("rawContentBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if let Some(items) = validation.get("issues").and_then(Value::as_array) {
            for issue in items {
                let mut issue = issue.clone();
                if let Some(object) = issue.as_object_mut() {
                    object.insert(
                        "collectionPath".to_string(),
                        collection
                            .get("collectionPath")
                            .cloned()
                            .unwrap_or_else(|| json!("")),
                    );
                }
                issues.push(issue);
            }
        }
    }
    json!({
        "schemaVersion": ARCHIVE_JOB_SCHEMA_VERSION,
        "healthStatus": if failed { "failed" } else { "ok" },
        "checkedAt": timestamp(),
        "recordCount": record_count,
        "rawContentBytes": raw_content_bytes,
        "errorCount": error_count,
        "warningCount": warning_count,
        "issues": issues
    })
}

pub(super) fn failed_validation(collection_path: &str, message: &str) -> Value {
    json!({
        "schemaVersion": ARCHIVE_JOB_SCHEMA_VERSION,
        "healthStatus": "failed",
        "checkedAt": timestamp(),
        "recordCount": 0,
        "rawContentBytes": 0,
        "errorCount": 1,
        "warningCount": 0,
        "issues": [{
            "type": "archive_job_verification_error",
            "severity": "error",
            "collectionPath": collection_path,
            "message": message
        }]
    })
}
