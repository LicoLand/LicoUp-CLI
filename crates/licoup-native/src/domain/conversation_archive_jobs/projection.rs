//! Stable client-facing job and target-scan projections.

use serde_json::{Value, json};

use crate::domain::conversation::archive_queue::ArchiveJob;

pub(super) fn job_to_json(job: ArchiveJob) -> Value {
    let target_scan_summary = target_scan_summary(&job.target_scan);
    json!({
        "jobId": job.job_id,
        "request": job.request,
        "targetScan": job.target_scan,
        "targetScanSummary": target_scan_summary,
        "status": job.status,
        "phase": job.phase,
        "attempt": job.attempt,
        "maxAttempts": job.max_attempts,
        "archiveResult": job.archive_result,
        "validationResult": job.validation_result,
        "createdAt": job.created_at,
        "updatedAt": job.updated_at,
        "retryAfter": job.retry_after,
        "lastError": job.last_error,
        "completedAt": job.completed_at,
        "failedAt": job.failed_at,
        "cancelledAt": job.cancelled_at,
        "workflow": {
            "status": job.status,
            "currentPhase": job.phase,
            "attempt": job.attempt,
            "maxAttempts": job.max_attempts
        },
        "plan": job.request.get("plan").cloned().unwrap_or_else(|| json!({})),
        "mode": "conversation-archive-job",
        "entry": "selection-archive-job"
    })
}

pub(super) fn target_scan_summary(target_scan: &Value) -> Value {
    let candidates = target_scan
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let detected = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status != "not-detected")
        })
        .count();
    json!({
        "source": target_scan.get("source").cloned().unwrap_or_else(|| json!("target-adapters")),
        "clientCount": candidates.len(),
        "detectedCount": detected,
        "clients": candidates.iter().map(|candidate| {
            json!({
                "target": candidate.get("target").cloned().unwrap_or_else(|| json!("")),
                "label": candidate.get("label").cloned().unwrap_or_else(|| json!("")),
                "status": candidate.get("status").cloned().unwrap_or_else(|| json!("")),
                "historyRoots": candidate.get("historyRoots").cloned().unwrap_or_else(|| json!([]))
            })
        }).collect::<Vec<_>>()
    })
}
