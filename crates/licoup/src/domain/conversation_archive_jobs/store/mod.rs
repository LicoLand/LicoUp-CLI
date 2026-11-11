//! Local-only archive-job state root and SQLite connection boundary.

mod events;
mod jobs;
mod schema;

use anyhow::Result;
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::Duration;

use super::constants::{ARCHIVE_JOB_DB, ARCHIVE_JOB_DIR};
use super::request::optional_local_path_param;
use crate::platform::client_state::{ActivityLog, ClientStateStore};
use licoup_foundation::platform::paths::portable_data_dir;

pub(super) use jobs::row_to_job;

pub(super) struct ArchiveJobStore {
    pub(super) root: PathBuf,
    pub(super) db_path: PathBuf,
    pub(super) activity_log: ActivityLog,
}

impl ArchiveJobStore {
    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self> {
        let client_state = if let Some(state_root) =
            optional_local_path_param(params, &["stateRoot", "clientStateRoot"], "state root")?
        {
            ClientStateStore::new(state_root)?
        } else if let Some(portable_dir) =
            optional_local_path_param(params, &["portableDir"], "portable directory")?
        {
            ClientStateStore::new(portable_dir.join("client-state"))?
        } else {
            ClientStateStore::new(portable_data_dir()?.join("client-state"))?
        };
        let activity_log = client_state.activity_log();
        Self::new(client_state.root().join(ARCHIVE_JOB_DIR), activity_log)
    }

    fn new(root: PathBuf, activity_log: ActivityLog) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        let store = Self {
            db_path: root.join(ARCHIVE_JOB_DB),
            root,
            activity_log,
        };
        store.ensure_schema()?;
        Ok(store)
    }

    pub(super) fn conn(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(conn)
    }
}
