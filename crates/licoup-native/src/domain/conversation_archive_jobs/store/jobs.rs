//! Typed archive-job row decoding and point lookup.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use super::ArchiveJobStore;
use crate::domain::conversation::archive_queue::ArchiveJob;

impl ArchiveJobStore {
    pub(in crate::domain::conversation_archive_jobs) fn get_job(
        &self,
        conn: &Connection,
        job_id: &str,
    ) -> Result<Option<ArchiveJob>> {
        conn.query_row(
            "SELECT * FROM conversation_archive_jobs WHERE job_id = ?1",
            params![job_id],
            row_to_job,
        )
        .optional()
        .map_err(Into::into)
    }
}

pub(in crate::domain::conversation_archive_jobs) fn row_to_job(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ArchiveJob> {
    let request_raw: String = row.get("request_json")?;
    let target_scan_raw: String = row.get("target_scan_json")?;
    let archive_result_raw: String = row.get("archive_result_json")?;
    let validation_result_raw: String = row.get("validation_result_json")?;
    Ok(ArchiveJob {
        job_id: row.get("job_id")?,
        request: parse_json_field(&request_raw),
        target_scan: parse_json_field(&target_scan_raw),
        status: row.get("status")?,
        phase: row.get("phase")?,
        attempt: row.get("attempt")?,
        max_attempts: row.get("max_attempts")?,
        archive_result: parse_json_field(&archive_result_raw),
        validation_result: parse_json_field(&validation_result_raw),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        retry_after: row.get("retry_after")?,
        last_error: row.get("last_error")?,
        completed_at: row.get("completed_at")?,
        failed_at: row.get("failed_at")?,
        cancelled_at: row.get("cancelled_at")?,
    })
}

pub(super) fn parse_json_field(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({}))
}
