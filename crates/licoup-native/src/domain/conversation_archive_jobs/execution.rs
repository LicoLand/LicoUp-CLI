//! Archive and verify state-machine transitions over local snapshot functions.

use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::activity::record_activity;
use super::clock::timestamp;
use super::plan::validate_stored_plan;
use super::request::merge_params;
use super::store::ArchiveJobStore;
use super::validation::{aggregate_validations, archive_collection_paths, failed_validation};
use crate::domain::conversation::archive_queue::{ArchiveJob, ArchiveJobStatus};
use crate::domain::conversation_snapshots;

impl ArchiveJobStore {
    pub(super) fn advance_job(&self, conn: &Connection, job: ArchiveJob) -> Result<Value> {
        match ArchiveJobStatus::from_str(&job.status)? {
            ArchiveJobStatus::Queued | ArchiveJobStatus::RetryScheduled => {
                self.run_archive_step(conn, job)
            }
            ArchiveJobStatus::Archiving => self.run_archive_step(conn, job),
            ArchiveJobStatus::Verifying => self.run_verify_step(conn, job),
            ArchiveJobStatus::Scanning
            | ArchiveJobStatus::Completed
            | ArchiveJobStatus::Failed
            | ArchiveJobStatus::Cancelled => self.job_response(conn, job),
        }
    }

    pub(super) fn run_archive_step(&self, conn: &Connection, mut job: ArchiveJob) -> Result<Value> {
        if self.is_cancelled(conn, &job.job_id)? {
            let updated = self
                .get_job(conn, &job.job_id)?
                .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job.job_id))?;
            return self.job_response(conn, updated);
        }
        if job.attempt == 0 {
            validate_stored_plan(&job.request, &job.target_scan)?;
        }
        let attempt = job.attempt + 1;
        let now = timestamp();
        conn.execute(
            "
            UPDATE conversation_archive_jobs
            SET status = 'archiving', phase = 'archiving', attempt = ?1,
                updated_at = ?2, retry_after = '', last_error = ''
            WHERE job_id = ?3
            ",
            params![attempt, now, job.job_id],
        )?;
        self.append_event(
            conn,
            &job.job_id,
            "archive.run.started",
            ArchiveJobStatus::Archiving,
            attempt,
            json!({ "attempt": attempt }),
        )?;
        job.attempt = attempt;

        let archive_params = merge_params(
            &job.request,
            json!({
                "targetScan": job.target_scan,
                "trigger": "conversation-archive-job",
                "jobId": job.job_id
            }),
        );
        match conversation_snapshots::archive_selection_collect(&archive_params) {
            Ok(result) => {
                let archive_ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
                conn.execute(
                    "
                    UPDATE conversation_archive_jobs
                    SET archive_result_json = ?1, updated_at = ?2
                    WHERE job_id = ?3
                    ",
                    params![serde_json::to_string(&result)?, timestamp(), job.job_id],
                )?;
                self.append_event(
                    conn,
                    &job.job_id,
                    if archive_ok {
                        "archive.run.completed"
                    } else {
                        "archive.run.failed"
                    },
                    ArchiveJobStatus::Archiving,
                    attempt,
                    json!({ "result": result.clone() }),
                )?;
                if archive_ok {
                    conn.execute(
                        "
                        UPDATE conversation_archive_jobs
                        SET status = 'verifying', phase = 'verifying', updated_at = ?1,
                            last_error = ''
                        WHERE job_id = ?2
                        ",
                        params![timestamp(), job.job_id],
                    )?;
                    self.append_event(
                        conn,
                        &job.job_id,
                        "archive.verify.started",
                        ArchiveJobStatus::Verifying,
                        attempt,
                        json!({ "attempt": attempt }),
                    )?;
                    let updated = self.get_job(conn, &job.job_id)?.ok_or_else(|| {
                        anyhow!("unknown conversation archive job: {}", job.job_id)
                    })?;
                    self.job_response(conn, updated)
                } else {
                    self.handle_retry_or_fail(
                        conn,
                        job,
                        "archive_failed",
                        result
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("archive returned ok=false"),
                        json!({ "archiveResult": result }),
                    )
                }
            }
            Err(error) => {
                self.append_event(
                    conn,
                    &job.job_id,
                    "archive.run.failed",
                    ArchiveJobStatus::Archiving,
                    attempt,
                    json!({ "error": error.to_string() }),
                )?;
                self.handle_retry_or_fail(
                    conn,
                    job,
                    "archive_error",
                    &error.to_string(),
                    json!({ "error": error.to_string() }),
                )
            }
        }
    }

    pub(super) fn run_verify_step(&self, conn: &Connection, job: ArchiveJob) -> Result<Value> {
        if self.is_cancelled(conn, &job.job_id)? {
            let updated = self
                .get_job(conn, &job.job_id)?
                .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job.job_id))?;
            return self.job_response(conn, updated);
        }
        let collection_paths = archive_collection_paths(&job.archive_result);
        let mut collections = Vec::<Value>::new();
        let mut had_error = false;
        for collection_path in collection_paths {
            let verify_params = merge_params(
                &job.request,
                json!({
                    "collectionPath": collection_path,
                    "jobId": job.job_id
                }),
            );
            match conversation_snapshots::archive_verify(&verify_params) {
                Ok(verified) => collections.push(verified),
                Err(error) => {
                    had_error = true;
                    collections.push(json!({
                        "ok": false,
                        "collectionPath": collection_path,
                        "validation": failed_validation(&collection_path, &error.to_string())
                    }));
                }
            }
        }
        if collections.is_empty() {
            had_error = true;
            collections.push(json!({
                "ok": false,
                "collectionPath": "",
                "validation": failed_validation("", "archive produced no collection paths")
            }));
        }
        let validation = aggregate_validations(&collections);
        let verification_ok = !had_error
            && validation
                .get("healthStatus")
                .and_then(Value::as_str)
                .is_some_and(|status| status != "failed");
        let validation_result = json!({
            "ok": verification_ok,
            "mode": "conversation-archive-job-verify",
            "collectionCount": collections.len(),
            "collections": collections,
            "validation": validation
        });
        conn.execute(
            "
            UPDATE conversation_archive_jobs
            SET validation_result_json = ?1, updated_at = ?2
            WHERE job_id = ?3
            ",
            params![
                serde_json::to_string(&validation_result)?,
                timestamp(),
                job.job_id
            ],
        )?;
        self.append_event(
            conn,
            &job.job_id,
            if verification_ok {
                "archive.verify.completed"
            } else {
                "archive.verify.failed"
            },
            ArchiveJobStatus::Verifying,
            job.attempt,
            json!({ "validation": validation_result.clone() }),
        )?;
        if verification_ok {
            let now = timestamp();
            conn.execute(
                "
                UPDATE conversation_archive_jobs
                SET status = 'completed', phase = 'completed', updated_at = ?1,
                    completed_at = ?1, last_error = ''
                WHERE job_id = ?2
                ",
                params![now, job.job_id],
            )?;
            self.append_event(
                conn,
                &job.job_id,
                "archive.completed",
                ArchiveJobStatus::Completed,
                job.attempt,
                json!({ "validation": validation_result }),
            )?;
            let updated = self
                .get_job(conn, &job.job_id)?
                .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job.job_id))?;
            record_activity(
                &self.activity_log,
                "conversation_archive_jobs.completed",
                json!({
                    "target": "conversation-archive-jobs",
                    "jobId": updated.job_id,
                    "attempt": updated.attempt,
                    "validation": updated.validation_result.get("validation").cloned().unwrap_or_else(|| json!({}))
                }),
            );
            self.job_response(conn, updated)
        } else {
            self.handle_retry_or_fail(
                conn,
                job,
                if had_error {
                    "verification_error"
                } else {
                    "verification_failed"
                },
                "archive verification failed",
                json!({ "validation": validation_result }),
            )
        }
    }
}
