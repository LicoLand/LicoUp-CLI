//! Subagent workspace isolation and working directory binding leaves.
//!
//! Subagents operate within an explicit working directory (`cwd`).
//! Isolation of `cwd` provides filesystem working-directory isolation only.
//! It explicitly does NOT isolate:
//! - Quota (shared conversation and account quota)
//! - Database (shared SQLite/WAL store)
//! - Generated directories (shared build, cache, and platform artifact roots)
//! - Tool side effects (external process execution, network calls, filesystem mutations)

use super::{MAX_WORKING_DIRECTORY_BYTES, SubagentError, permanent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Isolation facts for a subagent working directory.
///
/// Isolation of cwd provides process-local filesystem path scoping only.
/// It explicitly does NOT isolate:
/// - Quota (usage is charged to the shared conversation and account quota)
/// - Database (shared SQLite/WAL store)
/// - Generated directories (shared build, cache, and platform artifact roots)
/// - Tool side effects (external system operations, network access, or external files)
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentWorkspaceIsolation {
    pub working_directory: String,
    pub isolates_quota: bool,
    pub isolates_database: bool,
    pub isolates_generated_dirs: bool,
    pub isolates_tool_effects: bool,
}

impl SubagentWorkspaceIsolation {
    pub fn new(cwd: impl Into<String>) -> Result<Self, SubagentError> {
        let cwd = cwd.into();
        let trimmed = cwd.trim();
        if trimmed.is_empty()
            || trimmed.len() > MAX_WORKING_DIRECTORY_BYTES
            || trimmed.contains('\0')
            || !std::path::Path::new(trimmed).is_absolute()
        {
            return Err(permanent("invalid_working_directory", "schema/validate"));
        }
        Ok(Self {
            working_directory: trimmed.to_owned(),
            isolates_quota: false,
            isolates_database: false,
            isolates_generated_dirs: false,
            isolates_tool_effects: false,
        })
    }

    /// Assert runtime invariants: cwd isolation does NOT isolate quota, database, generated dirs, or tool effects.
    pub fn assert_unisolated_invariants(&self) {
        assert!(!self.isolates_quota, "CWD isolation must not isolate quota");
        assert!(
            !self.isolates_database,
            "CWD isolation must not isolate database"
        );
        assert!(
            !self.isolates_generated_dirs,
            "CWD isolation must not isolate generated directories"
        );
        assert!(
            !self.isolates_tool_effects,
            "CWD isolation must not isolate tool effects"
        );
    }
}

/// Durable record of one workspace bound to a conversation membership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentWorkspaceBinding {
    pub conversation_id: String,
    pub membership_id: String,
    pub isolation: SubagentWorkspaceIsolation,
    pub bound_at_unix_ms: i64,
}

/// Subagent workspace management port.
pub trait SubagentWorkspacePort: Send + Sync {
    /// Bind a working directory to a membership atomically during admission.
    fn bind_workspace(
        &self,
        conversation_id: &str,
        membership_id: &str,
        working_directory: &str,
    ) -> Result<SubagentWorkspaceBinding, SubagentError>;

    /// Read the currently bound workspace for a membership.
    fn get_workspace(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> Result<Option<SubagentWorkspaceBinding>, SubagentError>;

    /// Verify compatibility of a resume request with the recorded working directory.
    ///
    /// Outcome: "Resume must not bind the same request to a different working directory."
    /// - If recorded is `Some(recorded)`:
    ///   - requested is `Some(req)`: if req != recorded, returns `conversation_working_directory_mismatch`.
    ///   - requested is `None`: returns `Ok(Some(recorded))` (preserves bound directory).
    /// - If recorded is `None`:
    ///   - requested is `Some(_)`: returns `conversation_working_directory_mismatch` (cannot rebind on resume).
    ///   - requested is `None`: returns `Ok(None)`.
    fn verify_resume_workspace(
        &self,
        requested_directory: Option<&str>,
        recorded_directory: Option<&str>,
    ) -> Result<Option<String>, SubagentError> {
        verify_resume_working_directory(requested_directory, recorded_directory)
    }
}

/// Pure helper to enforce that resume does not bind to a different working directory.
pub fn verify_resume_working_directory(
    requested_directory: Option<&str>,
    recorded_directory: Option<&str>,
) -> Result<Option<String>, SubagentError> {
    match (
        requested_directory.map(str::trim).filter(|s| !s.is_empty()),
        recorded_directory.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(req), Some(rec)) => {
            if req != rec {
                Err(permanent(
                    "conversation_working_directory_mismatch",
                    "identity/resolve",
                ))
            } else {
                Ok(Some(rec.to_owned()))
            }
        }
        (None, Some(rec)) => Ok(Some(rec.to_owned())),
        (Some(_), None) => Err(permanent(
            "conversation_working_directory_mismatch",
            "identity/resolve",
        )),
        (None, None) => Ok(None),
    }
}

/// In-memory implementation of [`SubagentWorkspacePort`] for testing and standalone operation.
#[derive(Clone, Default)]
pub struct InMemorySubagentWorkspacePort {
    bindings: Arc<Mutex<HashMap<(String, String), SubagentWorkspaceBinding>>>,
}

impl InMemorySubagentWorkspacePort {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SubagentWorkspacePort for InMemorySubagentWorkspacePort {
    fn bind_workspace(
        &self,
        conversation_id: &str,
        membership_id: &str,
        working_directory: &str,
    ) -> Result<SubagentWorkspaceBinding, SubagentError> {
        let isolation = SubagentWorkspaceIsolation::new(working_directory)?;
        let binding = SubagentWorkspaceBinding {
            conversation_id: conversation_id.to_owned(),
            membership_id: membership_id.to_owned(),
            isolation,
            bound_at_unix_ms: 0,
        };
        self.bindings.lock().unwrap().insert(
            (conversation_id.to_owned(), membership_id.to_owned()),
            binding.clone(),
        );
        Ok(binding)
    }

    fn get_workspace(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> Result<Option<SubagentWorkspaceBinding>, SubagentError> {
        Ok(self
            .bindings
            .lock()
            .unwrap()
            .get(&(conversation_id.to_owned(), membership_id.to_owned()))
            .cloned())
    }
}
