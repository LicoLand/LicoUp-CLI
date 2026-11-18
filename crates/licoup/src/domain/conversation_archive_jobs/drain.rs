//! Bounded local queue draining and oldest-first job selection.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::constants::ARCHIVE_JOB_SCHEMA_VERSION;
use super::request::{bool_param, number_param, text_param};
use super::store::{ArchiveJobStore, row_to_job};
use crate::domain::conversation::archive_queue::ArchiveJob;

impl ArchiveJobStore {
    pub(super) fn drain(&self, params: &Value) -> Result<Value> {
        let conn = self.conn()?;
        let once = bool_param(params, &["once"]).unwrap_or(false);
        let stop_on_error = bool_param(params, &["stopOnError"]).unwrap_or(false);
        let mut processed = Vec::<Value>::new();
        let mut completed = 0_u64;
        let mut failed = 0_u64;
        let mut deferred = 0_u64;

        loop {
            let jobs = self.next_jobs(&conn, params)?;
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                let job_id = job.job_id.clone();
                let outcome = self.advance_job(&conn, job)?;
                let status = outcome
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if status == "completed" {
                    completed += 1;
                } else if status == "failed" {
                    failed += 1;
                } else if status == "retry_scheduled" {
                    deferred += 1;
                }
                processed.push(json!({
                    "jobId": job_id,
                    "outcome": outcome
                }));
                if once || (stop_on_error && status == "failed") {
                    return Ok(json!({
                        "ok": true,
                        "schemaVersion": ARCHIVE_JOB_SCHEMA_VERSION,
                        "status": "drained",
                        "processed": processed.len(),
                        "completed": completed,
                        "failed": failed,
                        "deferred": deferred,
                        "jobs": processed
                    }));
                }
            }
        }

        Ok(json!({
            "ok": true,
            "schemaVersion": ARCHIVE_JOB_SCHEMA_VERSION,
            "status": "drained",
            "processed": processed.len(),
            "completed": completed,
            "failed": failed,
            "deferred": deferred,
            "jobs": processed
        }))
    }

    fn next_jobs(&self, conn: &Connection, params: &Value) -> Result<Vec<ArchiveJob>> {
        let job_id = text_param(params, &["jobId"]);
        let limit = if job_id.is_some() {
            1
        } else {
            number_param(params, &["limit"]).unwrap_or(10).clamp(1, 100) as i64
        };
        let mut stmt = conn.prepare(
            "
            SELECT * FROM conversation_archive_jobs
            WHERE status IN ('queued', 'archiving', 'verifying', 'retry_scheduled')
              AND (?1 = '' OR job_id = ?1)
            ORDER BY created_at ASC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![job_id.unwrap_or_default(), limit], row_to_job)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row?);
        }
        Ok(jobs)
    }
}
