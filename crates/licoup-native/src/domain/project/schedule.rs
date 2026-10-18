//! The scheduling answer this project owner publishes to the work owners.
//!
//! The runtime owners — the continuous scheduler, the resource arbiter and the
//! idle guard that gates an update — need three facts this crate owns and they
//! do not: which declared project work may begin, what one stop releases, and
//! whether any admitted responsibility remains. [`crate::domain::project::port`]
//! exposes them through the durable store; this module declares the question so
//! a caller composes them without reaching into the project tables.
//!
//! Two properties are structural rather than promised:
//!
//! - **Readiness is per work item.** The answer names the producers one work
//!   item actually waits for, so an independent branch is never serialized
//!   behind another, and a scheduler that consumes this cannot collapse the
//!   answer into one project-wide state.
//! - **Settlement is read, never set.** [`OutstandingWork`] comes from the
//!   durable rows on every call. Nothing in this module can mark work released,
//!   so a card, a status or a detached view cannot end admitted responsibility.
//!
//! The stop scope is a selection, not a signal. It is taken to the existing
//! owned-child stop and force-stop confirmation owners, which remain the only
//! callers that may signal a process; an owner that does not acknowledge leaves
//! its work unconfirmed, and unconfirmed work is never reported as released.

use super::port::{NativeProjectApplication, failure};
use licoup_application::ApplicationFailure;
use licoup_project::{OutstandingWork, ProjectId, StopScope, WorkItemId, WorkReadiness};

/// What the runtime owners ask the project owner.
///
/// Every method answers from durable rows. A refusal is typed and never a
/// default: a caller that cannot read the answer must not treat project work as
/// settled.
pub trait ProjectWorkDirectory: Send + Sync {
    /// Which declared work of one project may begin now.
    fn readiness(&self, project_id: &ProjectId) -> Result<WorkReadiness, ApplicationFailure>;

    /// The work one stop releases: the selection and its declared consumers.
    fn stop_scope(
        &self,
        project_id: &ProjectId,
        work_item_id: &WorkItemId,
    ) -> Result<StopScope, ApplicationFailure>;

    /// Whether one project still holds admitted responsibility.
    fn outstanding(&self, project_id: &ProjectId) -> Result<OutstandingWork, ApplicationFailure>;
}

/// The project owner composed over the same durable store the commands use.
pub struct NativeProjectWorkDirectory {
    application: NativeProjectApplication,
}

impl NativeProjectWorkDirectory {
    /// Answer over one explicit data root, used by tests and an embedding host.
    pub fn at(portable_root: &std::path::Path) -> Self {
        Self {
            application: NativeProjectApplication::at(portable_root),
        }
    }

    /// Answer over this process's portable data root.
    pub fn portable() -> Self {
        Self {
            application: NativeProjectApplication::portable(),
        }
    }
}

impl ProjectWorkDirectory for NativeProjectWorkDirectory {
    fn readiness(&self, project_id: &ProjectId) -> Result<WorkReadiness, ApplicationFailure> {
        self.application
            .store()
            .map_err(failure)?
            .work_readiness(project_id)
            .map_err(failure)
    }

    fn stop_scope(
        &self,
        project_id: &ProjectId,
        work_item_id: &WorkItemId,
    ) -> Result<StopScope, ApplicationFailure> {
        self.application
            .store()
            .map_err(failure)?
            .stop_scope(project_id, work_item_id)
            .map_err(failure)
    }

    fn outstanding(&self, project_id: &ProjectId) -> Result<OutstandingWork, ApplicationFailure> {
        self.application
            .store()
            .map_err(failure)?
            .outstanding_work(project_id)
            .map_err(failure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_application::{ActorClaim, ProjectCommand, ProjectPort, ProjectRegistrationRequest};
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// One temporary data root, removed when the case ends.
    struct TempRoot {
        path: std::path::PathBuf,
    }

    impl TempRoot {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "licoup-native-project-schedule-{label}-{}-{nanos}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("the temporary root is creatable");
            Self { path }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// One registered project holding one imported slice of two work items.
    fn composed(label: &str) -> (TempRoot, NativeProjectWorkDirectory) {
        let root = TempRoot::new(label);
        let application = NativeProjectApplication::at(&root.path);
        let claim = ActorClaim::local_admin("membership:owner");
        let registered = application.execute(
            &claim,
            &ProjectCommand::Register(ProjectRegistrationRequest {
                project_id: "alpha-project".to_owned(),
                display_name: "Synthetic project alpha-project".to_owned(),
                authorized_root: root.path.join("authorized").to_string_lossy().into_owned(),
                authority_kind: "membership".to_owned(),
                authority_reference: "membership:owner".to_owned(),
                workspace_id: "workspace:shared".to_owned(),
                plan_id: "plan:alpha".to_owned(),
            }),
        );
        assert!(
            matches!(registered, Ok(_)),
            "the synthetic project registers"
        );
        let document = json!({
            "schema": licoup_project::PLAN_DOCUMENT_SCHEMA,
            "projectId": "alpha-project",
            "planId": "plan:alpha",
            "source": {
                "sourceId": "source:roadmap",
                "sourceKind": "markdown",
                "locator": "docs/roadmap.md",
            },
            "workItems": [
                {
                    "workItemId": "work:read",
                    "outcome": "Read the source.",
                    "acceptance": ["The declaration is complete."],
                    "inputs": [],
                    "roles": [{"roleId": "role:maintainer", "scope": "work-item"}],
                    "sourceAnchor": "heading:Read",
                },
                {
                    "workItemId": "work:write",
                    "outcome": "Write the plan.",
                    "acceptance": ["The declaration is complete."],
                    "inputs": [
                        {"kind": "local", "producerWorkItemId": "work:read", "path": "build/plan.json"}
                    ],
                    "roles": [{"roleId": "role:maintainer", "scope": "work-item"}],
                    "sourceAnchor": "heading:Write",
                },
            ],
        });
        let applied = application.execute(
            &claim,
            &ProjectCommand::ImportApply {
                document,
                expected_revision: 0,
            },
        );
        assert!(matches!(applied, Ok(_)), "the synthetic import applies");
        let directory = NativeProjectWorkDirectory::at(&root.path);
        (root, directory)
    }

    #[test]
    fn the_runtime_owners_read_readiness_and_settlement_from_the_store() {
        let (_root, directory) = composed("runtime");
        let alpha = ProjectId::declare("alpha-project").expect("a bounded identity");
        let read = WorkItemId::declare("work:read").expect("a bounded identity");
        let write = WorkItemId::declare("work:write").expect("a bounded identity");

        let readiness = directory.readiness(&alpha).expect("the readiness reads");
        assert!(readiness.is_ready(&read));
        assert!(
            !readiness.is_ready(&write),
            "the declared consumer waits for its own producer"
        );

        let outstanding = directory.outstanding(&alpha).expect("the answer reads");
        assert_eq!(outstanding.admitted_work_items, 2);
        assert_eq!(outstanding.declared_inputs, 1);
        assert!(!outstanding.settled);

        let scope = directory
            .stop_scope(&alpha, &read)
            .expect("the stop scope reads");
        assert_eq!(scope.released.len(), 2);
        assert!(!scope.crosses_projects());
    }

    #[test]
    fn an_unanswerable_project_is_refused_rather_than_reported_settled() {
        let root = TempRoot::new("unauthorized");
        let directory = NativeProjectWorkDirectory::at(&root.path);
        let absent = ProjectId::declare("absent-project").expect("a bounded identity");
        let failure = directory
            .outstanding(&absent)
            .expect_err("an unregistered project is refused");
        assert_eq!(failure.code, "project_schedule_project_unauthorized");
        assert_eq!(failure.stage, "project/schedule");
        assert!(!failure.retryable);
    }
}
