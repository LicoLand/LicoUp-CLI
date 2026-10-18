//! What declared project work may begin, what a stop releases, and what remains.
//!
//! Scheduling asks this owner three questions and gets three explicit answers.
//! None of them is a status: this owner holds declarations, and a declaration
//! says what work takes from what — never that something ran.
//!
//! - **What may begin now.** Readiness is decided per work item from its own
//!   declared inputs. Work whose inputs are all materialized is ready, and so is
//!   work that declares none. An independent branch is therefore never
//!   serialized behind another: `A → C` with an independent `B` makes `C` ready
//!   as soon as `A`'s declared result exists, whether or not `B` has finished.
//! - **What a stop releases.** One selected work item and every declared
//!   consumer of it, transitively — the "descendants" of the declared graph and
//!   nothing else. Work outside that set keeps its own course, in this project
//!   and in every other one.
//! - **What remains.** Admitted plan work is admitted responsibility. A card, a
//!   status or a detached view releases nothing: the project is settled only
//!   when it holds no admitted work and no declared input, and the answer is
//!   read from the durable rows each time rather than stored as a claim.
//!
//! Nothing here starts, stops or confirms anything. A stop scope is a selection
//! a caller takes to the existing owned-child stop and force-stop confirmation
//! ports, which are the only owners that may signal a process; an owner that
//! does not answer leaves its work unconfirmed, and unconfirmed work is never
//! reported as released.

use crate::dependency::WorkRef;
use crate::failure::ProjectFailure;
use crate::identity::{ProjectId, WorkItemId};
use serde::Serialize;

/// The registered project a scheduling question is about is not registered.
pub const SCHEDULE_PROJECT_UNAUTHORIZED: &str = "project_schedule_project_unauthorized";

/// One declared work item that cannot begin yet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedWork {
    /// The work item that waits.
    pub work_item_id: WorkItemId,
    /// The producers whose declared result is not materialized. The reason is
    /// named per producer, never summarized into a whole-project state.
    pub blocked_by: Vec<WorkRef>,
}

/// Which declared work of one project may begin now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkReadiness {
    /// The project the answer is about.
    pub project_id: ProjectId,
    /// Admitted work whose declared inputs are all materialized.
    pub ready: Vec<WorkItemId>,
    /// Admitted work that waits, with the producers it waits for.
    pub blocked: Vec<BlockedWork>,
}

impl WorkReadiness {
    /// Whether one work item may begin now.
    pub fn is_ready(&self, work_item_id: &WorkItemId) -> bool {
        self.ready.contains(work_item_id)
    }

    /// Why one work item waits, when it does.
    pub fn blocked_by(&self, work_item_id: &WorkItemId) -> Option<&[WorkRef]> {
        self.blocked
            .iter()
            .find(|blocked| &blocked.work_item_id == work_item_id)
            .map(|blocked| blocked.blocked_by.as_slice())
    }
}

/// The work one stop releases, and the work it leaves alone.
///
/// The scope is a selection for the existing stop owners. It signals nothing,
/// confirms nothing and settles nothing on its own: a caller takes it to the
/// owned-child stop port, and a release that owner does not acknowledge stays
/// unconfirmed rather than being reported as done.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopScope {
    /// The project that owns the selected work item.
    pub project_id: ProjectId,
    /// The selected work item.
    pub work_item_id: WorkItemId,
    /// The selected work item and every declared consumer of it, transitively,
    /// in the declared order.
    pub released: Vec<WorkRef>,
    /// The projects the released work items belong to. A cross-project
    /// declaration widens this set and is visible here rather than implied.
    pub projects: Vec<ProjectId>,
}

impl StopScope {
    /// Whether one work item is inside this scope.
    pub fn releases(&self, work: &WorkRef) -> bool {
        self.released.contains(work)
    }

    /// Whether the scope reaches beyond the project that owns the selection.
    pub fn crosses_projects(&self) -> bool {
        self.projects.len() > 1
    }
}

/// Whether one project still holds admitted responsibility.
///
/// The answer is read from the durable rows; nothing a view or a card says can
/// change it. `settled` is false while the project holds any admitted work item
/// or any declared input.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutstandingWork {
    /// The project the answer is about.
    pub project_id: ProjectId,
    /// Admitted plan work items the project holds.
    pub admitted_work_items: usize,
    /// How many of them may begin now.
    pub ready: usize,
    /// How many of them wait.
    pub blocked: usize,
    /// Declared inputs the project's work items take.
    pub declared_inputs: usize,
    /// Whether the project may be reported settled.
    pub settled: bool,
}

impl OutstandingWork {
    /// The refusal this owner publishes for a question about another project.
    pub fn unauthorized() -> ProjectFailure {
        ProjectFailure::schedule(SCHEDULE_PROJECT_UNAUTHORIZED)
    }
}
