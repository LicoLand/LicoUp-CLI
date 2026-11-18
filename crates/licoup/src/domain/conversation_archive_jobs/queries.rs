//! Status, list, event queries, and consistent response projection.

use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::constants::ARCHIVE_JOB_SCHEMA_VERSION;
use super::projection::job_to_json;
use super::request::{display_path, number_param, required_job_id, text_param};
use super::store::{ArchiveJobStore, row_to_job};
use crate::domain::conversation::archive_queue::ArchiveJob;

impl ArchiveJobStore {
    pub(super) fn status(&self, params: &Value) -> Result<Value> {
        let conn = self.conn()?;
        let job_id = required_job_id(params)?;
        let job = self
            .get_job(&conn, &job_id)?
            .ok_or_else(|| anyhow!("unknown conversation archive job: {}", job_id))?;
        self.job_response(&conn, job)
    }

    pub(super) fn list(&self, params: &Value) -> Result<Value> {
        let conn = self.conn()?;
        let status = text_param(params, &["status"]).unwrap_or_default();
        let limit = number_param(params, &["limit"])
            .unwrap_or(100)
            .clamp(1, 1000) as i64;
        let mut stmt = conn.prepare(
            "
            SELECT * FROM conversation_archive_jobs
            WHERE (?1 = '' OR status = ?1)
            ORDER BY created_at DESC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![status, limit], row_to_job)?;
        let mut jobs = Vec::<Value>::new();
        for row in rows {
            jobs.push(job_to_json(row?));
        }
        Ok(json!({
            "ok": true,
            "schemaVersion": ARCHIVE_JOB_SCHEMA_VERSION,
            "jobs": jobs,
            "jobRoot": display_path(&self.root),
            "dbPath": display_path(&self.db_path)
        }))
    }

    pub(super) fn events(&self, params: &Value) -> Result<Value> {
        let conn = self.conn()?;
        let job_id = required_job_id(params)?;
        let events = self.list_events(&conn, &job_id, number_param(params, &["limit"]))?;
        Ok(json!({
            "ok": true,
            "schemaVersion": ARCHIVE_JOB_SCHEMA_VERSION,
            "jobId": job_id,
            "events": events
        }))
    }

    pub(super) fn job_response(&self, conn: &Connection, job: ArchiveJob) -> Result<Value> {
        let job_status = job.status.clone();
        let events = self.list_events(conn, &job.job_id, Some(1000))?;
        let latest_event_status = events
            .last()
            .and_then(|event| event.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let event_consistency = json!({
            "ok": latest_event_status.is_empty() || latest_event_status == job_status,
            "jobStatus": job_status,
            "latestEventStatus": latest_event_status,
            "eventCount": events.len()
        });
        let mut value = job_to_json(job);
        if let Some(object) = value.as_object_mut() {
            object.insert("ok".to_string(), json!(true));
            object.insert(
                "schemaVersion".to_string(),
                json!(ARCHIVE_JOB_SCHEMA_VERSION),
            );
            object.insert("events".to_string(), Value::Array(events));
            object.insert("eventConsistency".to_string(), event_consistency);
            object.insert("jobRoot".to_string(), json!(display_path(&self.root)));
            object.insert("dbPath".to_string(), json!(display_path(&self.db_path)));
        }
        Ok(value)
    }
}
