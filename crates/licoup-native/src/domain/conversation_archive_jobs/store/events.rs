//! Ordered local lifecycle events for one archive job.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::super::clock::timestamp;
use super::ArchiveJobStore;
use crate::domain::conversation::archive_queue::ArchiveJobStatus;

impl ArchiveJobStore {
    pub(in crate::domain::conversation_archive_jobs) fn append_event(
        &self,
        conn: &Connection,
        job_id: &str,
        event_type: &str,
        status: ArchiveJobStatus,
        attempt: u64,
        payload: Value,
    ) -> Result<()> {
        conn.execute(
            "
            INSERT INTO conversation_archive_job_events
              (job_id, event_type, phase, status, attempt, payload_json, created_at)
            VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)
            ",
            params![
                job_id,
                event_type,
                status.as_str(),
                attempt,
                serde_json::to_string(&payload)?,
                timestamp(),
            ],
        )?;
        Ok(())
    }

    pub(in crate::domain::conversation_archive_jobs) fn list_events(
        &self,
        conn: &Connection,
        job_id: &str,
        limit: Option<u64>,
    ) -> Result<Vec<Value>> {
        let limit = limit.unwrap_or(1000).clamp(1, 5000) as i64;
        let mut stmt = conn.prepare(
            "
            SELECT sequence, job_id, event_type, phase, status, attempt, payload_json, created_at
            FROM conversation_archive_job_events
            WHERE job_id = ?1
            ORDER BY sequence ASC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![job_id, limit], |row| {
            let payload_raw: String = row.get(6)?;
            let payload = serde_json::from_str::<Value>(&payload_raw).unwrap_or_else(|_| json!({}));
            Ok(json!({
                "sequence": row.get::<_, i64>(0)?,
                "jobId": row.get::<_, String>(1)?,
                "type": row.get::<_, String>(2)?,
                "phase": row.get::<_, String>(3)?,
                "status": row.get::<_, String>(4)?,
                "attempt": row.get::<_, u64>(5)?,
                "payload": payload,
                "createdAt": row.get::<_, String>(7)?
            }))
        })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }
}
