//! Explicit cancellation and bounded retry-or-terminal-failure policy.

use anyhow::{Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use super::clock::{timestamp, timestamp_after_seconds};
use super::request::{required_job_id, retry_policy_from_request, text_param};
use super::store::ArchiveJobStore;
use crate::domain::conversation::archive_queue::{ArchiveJob, ArchiveJobStatus};

impl ArchiveJobStore {
    pub(super) fn cancel(&self, params: &Value) -> Result<Value> {
        let conn = self.conn()?;
        let job_id = required_job_id(params)?;
        let job = self
            .get_job(&conn, &job_id)?
            .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job_id))?;
        if ArchiveJobStatus::from_str(&job.status)?.terminal() {
            return self.job_response(&conn, job);
        }
        let now = timestamp();
        conn.execute(
            "
            UPDATE conversation_archive_jobs
            SET status = 'cancelled', phase = 'cancelled', updated_at = ?1,
                cancelled_at = ?1, last_error = 'operator_cancelled'
            WHERE job_id = ?2
            ",
            params![now, job_id],
        )?;
        self.append_event(
            &conn,
            &job_id,
            "archive.cancelled",
            ArchiveJobStatus::Cancelled,
            job.attempt,
            json!({ "reason": text_param(params, &["reason"]).unwrap_or_else(|| "operator_cancelled".to_string()) }),
        )?;
        let job = self
            .get_job(&conn, &job_id)?
            .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job_id))?;
        self.job_response(&conn, job)
    }

    pub(super) fn handle_retry_or_fail(
        &self,
        conn: &Connection,
        job: ArchiveJob,
        error_kind: &str,
        message: &str,
        payload: Value,
    ) -> Result<Value> {
        let policy = retry_policy_from_request(&job.request, Some(job.max_attempts));
        let now = timestamp();
        if policy.should_retry(job.attempt, error_kind) {
            let retry_delay = policy.retry_delay_seconds(job.attempt);
            let retry_after = if retry_delay == 0 {
                String::new()
            } else {
                timestamp_after_seconds(retry_delay)
            };
            conn.execute(
                "
                UPDATE conversation_archive_jobs
                SET status = 'retry_scheduled', phase = 'retry_scheduled',
                    updated_at = ?1, retry_after = ?2, last_error = ?3
                WHERE job_id = ?4
                ",
                params![now, retry_after, message, job.job_id],
            )?;
            self.append_event(
                conn,
                &job.job_id,
                "archive.retry.scheduled",
                ArchiveJobStatus::RetryScheduled,
                job.attempt,
                json!({
                    "errorKind": error_kind,
                    "message": message,
                    "retryAfter": retry_after,
                    "nextAttempt": job.attempt + 1,
                    "payload": payload
                }),
            )?;
        } else {
            conn.execute(
                "
                UPDATE conversation_archive_jobs
                SET status = 'failed', phase = 'failed', updated_at = ?1,
                    failed_at = ?1, last_error = ?2
                WHERE job_id = ?3
                ",
                params![now, message, job.job_id],
            )?;
            self.append_event(
                conn,
                &job.job_id,
                "archive.failed",
                ArchiveJobStatus::Failed,
                job.attempt,
                json!({
                    "errorKind": error_kind,
                    "message": message,
                    "deadLetter": true,
                    "payload": payload
                }),
            )?;
        }
        let updated = self
            .get_job(conn, &job.job_id)?
            .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job.job_id))?;
        self.job_response(conn, updated)
    }

    pub(super) fn is_cancelled(&self, conn: &Connection, job_id: &str) -> Result<bool> {
        let status = conn
            .query_row(
                "SELECT status FROM conversation_archive_jobs WHERE job_id = ?1",
                params![job_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
        Ok(status == "cancelled")
    }
}
