//! Request normalization, one local target scan, and queued-job creation.

use anyhow::{Result, anyhow};
use rusqlite::params;
use serde_json::{Value, json};

use super::activity::record_activity;
use super::clock::timestamp;
use super::plan::{prepare, request_with_plan, require_matching_binding};
use super::projection::target_scan_summary;
use super::request::{job_id_for, retry_policy_from_request, text_param};
use super::store::ArchiveJobStore;
use crate::domain::conversation::archive_queue::ArchiveJobStatus;

impl ArchiveJobStore {
    pub(super) fn create(&self, params: &Value) -> Result<Value> {
        let prepared = prepare(params)?;
        require_matching_binding(params, &prepared)?;
        let (request, target_scan) = request_with_plan(prepared);
        let policy = retry_policy_from_request(&request, None);
        let job_id = text_param(params, &["jobId"]).unwrap_or_else(|| job_id_for(&request));
        let conn = self.conn()?;
        if let Some(existing) = self.get_job(&conn, &job_id)? {
            return Ok(self.job_response(&conn, existing)?);
        }

        let now = timestamp();
        conn.execute(
            "
            INSERT INTO conversation_archive_jobs (
              job_id, request_json, target_scan_json, status, phase, attempt, max_attempts,
              archive_result_json, validation_result_json, created_at, updated_at, retry_after,
              last_error, completed_at, failed_at, cancelled_at
            )
            VALUES (?1, ?2, ?3, 'queued', 'queued', 0, ?4, '{}', '{}', ?5, ?5, '', '', '', '', '')
            ",
            params![
                job_id,
                serde_json::to_string(&request)?,
                serde_json::to_string(&target_scan)?,
                policy.max_attempts,
                now,
            ],
        )?;
        let job = self
            .get_job(&conn, &job_id)?
            .ok_or_else(|| anyhow!("archive job insert failed"))?;
        self.append_event(
            &conn,
            &job.job_id,
            "archive.scan.completed",
            ArchiveJobStatus::Queued,
            0,
            json!({ "targetScan": target_scan_summary(&job.target_scan) }),
        )?;
        self.append_event(
            &conn,
            &job.job_id,
            "archive.job.queued",
            ArchiveJobStatus::Queued,
            0,
            json!({ "jobId": job.job_id }),
        )?;
        record_activity(
            &self.activity_log,
            "conversation_archive_jobs.created",
            json!({
                "target": "conversation-archive-jobs",
                "jobId": job.job_id,
                "status": job.status,
                "targetScan": target_scan_summary(&job.target_scan)
            }),
        );
        self.job_response(&conn, job)
    }
}
