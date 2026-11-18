//! SQLite schema owned only by the local archive-job queue.

use anyhow::Result;

use super::ArchiveJobStore;

impl ArchiveJobStore {
    pub(super) fn ensure_schema(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS conversation_archive_jobs (
              job_id TEXT PRIMARY KEY,
              request_json TEXT NOT NULL,
              target_scan_json TEXT NOT NULL,
              status TEXT NOT NULL,
              phase TEXT NOT NULL,
              attempt INTEGER NOT NULL DEFAULT 0,
              max_attempts INTEGER NOT NULL DEFAULT 2,
              archive_result_json TEXT NOT NULL DEFAULT '{}',
              validation_result_json TEXT NOT NULL DEFAULT '{}',
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              retry_after TEXT NOT NULL DEFAULT '',
              last_error TEXT NOT NULL DEFAULT '',
              completed_at TEXT NOT NULL DEFAULT '',
              failed_at TEXT NOT NULL DEFAULT '',
              cancelled_at TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_conversation_archive_jobs_status
              ON conversation_archive_jobs(status, updated_at);
            CREATE TABLE IF NOT EXISTS conversation_archive_job_events (
              sequence INTEGER PRIMARY KEY AUTOINCREMENT,
              job_id TEXT NOT NULL,
              event_type TEXT NOT NULL,
              phase TEXT NOT NULL,
              status TEXT NOT NULL,
              attempt INTEGER NOT NULL,
              payload_json TEXT NOT NULL,
              created_at TEXT NOT NULL,
              FOREIGN KEY(job_id) REFERENCES conversation_archive_jobs(job_id)
            );
            CREATE INDEX IF NOT EXISTS idx_conversation_archive_job_events_job
              ON conversation_archive_job_events(job_id, sequence);
            ",
        )?;
        Ok(())
    }
}
