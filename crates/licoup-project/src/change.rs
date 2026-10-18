//! The declared change and the impact it would have.
//!
//! A change is a declaration, never an effect: it says which work items of one
//! project take which declared results from now on. The preview answers the two
//! questions that decision needs, and nothing else:
//!
//! - **what the change would touch**, followed from the declared dependency
//!   edges only: the declared work items, their transitive consumers, and the
//!   references of other projects that start or stop resolving because the
//!   change adds or removes a declaration;
//! - **what the change cannot quietly reuse**, asked of the existing work owner
//!   ([`WorkActivityDirectory`]): a work item with a run in flight or an
//!   accepted result behind the declaration being replaced needs an explicit
//!   handoff, successor or cancel-and-redo, and a work item whose owner does not
//!   answer stays unresolved rather than fresh.
//!
//! Four rules keep the answer trustworthy:
//!
//! - **Nothing is applied.** The preview writes no row, performs no handoff,
//!   cancels no run and rewrites no acceptance. A previous acceptance stays a
//!   historical fact; what the change invalidates is named, not erased.
//! - **Nothing is invented.** A work item is declared by its project exactly
//!   when the dependency index names it — as a consumer or as a local producer
//!   — which is the rule the stored artifact states already answer with. A
//!   reference nothing declares stays explicit rather than assumed.
//! - **Refusal matches admission.** A proposed edge that admission would refuse
//!   is refused here with the same code and stage: a location that escapes its
//!   authorized root, an unauthorized cross-project reference, a cycle (with
//!   its path) and a declaring project that is not registered.
//! - **States stay explicit.** Every input carries the [`ArtifactState`] the
//!   declared reference reports under the change, and whether the change
//!   replaces the result it reads: a materialized file behind a changed
//!   producer is never presented as the input the new contract takes.
//!
//! The only locations read are the declared local inputs, one declared
//! component at a time inside their authorized root, by the same rule admission
//! uses ([`read_local_artifact`]). No directory is listed, no symbolic link is
//! followed and no location outside a declared root is inspected.
//!
//! The preview is a query, not a durable record: it stores nothing, because
//! everything it answers can be recomputed from the declarations and the
//! explicit states that are already durable.

use crate::activity::{WorkActivity, WorkActivityDirectory};
use crate::dependency::{
    ArtifactReference, ArtifactState, WorkRef, render_dependency_path, stays_inside_authorized_root,
};
use crate::failure::ProjectFailure;
use crate::identity::{ProjectId, WorkItemId};
use crate::store::{
    EdgeSnapshot, ProjectIdentityStore, cycle_path, dependency_state, edge_snapshot,
    project_exists, read_project_row, registered_projects,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Largest number of work items one change declaration may name.
pub const MAX_CHANGE_WORK_ITEMS: usize = 512;

/// Largest number of declared inputs one change declaration may carry.
pub const MAX_CHANGE_INPUTS: usize = 4096;

/// One declared change to the work items of one project.
///
/// One request is one project's declaration. A change in another project is a
/// second request, so no caller has to say which project a work item belongs to
/// twice, and a preview can never mix two projects' authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeRequest {
    /// The project the change is declared in. It must already be registered.
    pub project_id: ProjectId,
    /// The work items the change declares, at least one, each named once.
    pub changes: Vec<WorkItemChange>,
}

/// What one change declares about one work item.
///
/// The declaration is total: the work item takes exactly `inputs` afterwards.
/// Dropping an input is expressed by not naming it, and an inserted work item
/// that waits for nothing takes an empty list. Whether the work item already
/// existed is not declared here — the work owner's answer decides what an
/// existing run or acceptance means for it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkItemChange {
    /// The work item whose declared inputs change.
    pub work_item_id: WorkItemId,
    /// The declared results this work item takes afterwards.
    pub inputs: Vec<ArtifactReference>,
}

/// Everything one declared change would touch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePreview {
    /// The project the change was declared in.
    pub project_id: ProjectId,
    /// Every work item the change reaches: the declared ones first, in
    /// declaration order, then the work items reached through their declared
    /// edges in discovery order. A work item appears once, with the first path
    /// that reached it.
    pub affected: Vec<AffectedWorkItem>,
    /// Registered projects this change reaches no work item of, in registration
    /// order. The change does not restart them.
    pub untouched_projects: Vec<ProjectId>,
}

/// One work item the change reaches, and what it means for it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedWorkItem {
    /// The work item the change reaches.
    pub work: WorkRef,
    /// Why it is reached, as a path that starts at a work item this change
    /// declares and ends here. A consumer path is the declared dependency chain
    /// between the two; a path for a reference that starts or stops resolving
    /// names the declaring work item, the referenced work item and the consumer
    /// that references it.
    pub path: Vec<WorkRef>,
    /// How the change reaches it.
    pub impact: ChangeImpact,
    /// The declared inputs the work item takes once the change lands, each with
    /// the explicit state its reference reports under that declaration, and
    /// whether this change replaces the result it reads.
    pub inputs: Vec<DeclaredInputState>,
    /// What the existing work owner knows about this work item's current run
    /// and acceptance. Never inferred here.
    pub activity: WorkActivity,
    /// What the change requires of the previous result of this work item.
    pub handoff: ChangeHandoff,
}

/// How one declared change reaches one work item.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeImpact {
    /// The change declares this work item's inputs.
    Declared,
    /// A declared dependency edge makes this work item wait on a work item the
    /// change reaches, so the result it takes is replaced.
    Consumer,
    /// A cross-project reference this work item declares starts resolving,
    /// because the change makes its referenced work item declared.
    Unblocked,
    /// A cross-project reference this work item declares stops resolving,
    /// because the change removes the declaration of its referenced work item.
    Blocked,
}

/// One declared input of an affected work item, under the change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredInputState {
    /// The work item the input is taken from.
    pub producer: WorkRef,
    /// The declared reference the input is taken by, unchanged.
    pub artifact: ArtifactReference,
    /// The explicit state the declared reference reports under the change.
    ///
    /// A local location is read from its authorized root exactly as admission
    /// reads it. A cross-project reference is answered by the declared index
    /// the change would leave behind, so a reference the change resolves is
    /// materialized and one it orphans is missing.
    pub state: ArtifactState,
    /// Whether this change replaces the result this input reads, because its
    /// producer is a work item the change reaches. True means the result
    /// readable now is not the result the new contract takes, whatever
    /// [`Self::state`] reports about the old one.
    pub pending: bool,
}

/// What one change requires of a work item's previous result.
///
/// The preview never performs the handoff: it names what the caller must settle
/// through the existing work authority before the previous result may be
/// replaced.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeHandoff {
    /// Nothing is in flight and no result was accepted behind the declaration
    /// the change replaces.
    None,
    /// A run in flight or an accepted result exists behind the declaration the
    /// change replaces: the caller must hand it off, name a successor or cancel
    /// and redo it through the existing authority. An accepted result never
    /// satisfies the changed contract on its own.
    Required,
    /// The work owner did not answer for this work item, so the preview claims
    /// neither freshness nor a handoff.
    Unresolved,
}

impl ProjectIdentityStore {
    /// Preview one declared change over the declared dependency edges.
    ///
    /// The preview refuses exactly what admission would refuse, with the same
    /// code and stage, so a caller never receives a preview of a change that
    /// cannot land. What it answers is computed from the registered identities,
    /// the declared edges, the declared roots and the work owner's activity
    /// answers; nothing is persisted and nothing is applied.
    pub fn preview_change(
        &self,
        activity: &dyn WorkActivityDirectory,
        request: &ChangeRequest,
    ) -> Result<ChangePreview, ProjectFailure> {
        let connection = self.connect()?;
        let declaring = read_project_row(&connection, request.project_id.as_str())?
            .ok_or_else(|| ProjectFailure::dependency("project_dependency_project_unauthorized"))?;
        let declared = declare_request(request)?;

        // The declaration the change leaves behind: the stored edges with the
        // re-declared work items' own inputs removed, plus the proposed ones.
        let stored = edge_snapshot(&connection)?;
        let replaced = |edge: &EdgeSnapshot| {
            edge.consumer.project_id == request.project_id
                && declared.contains(&edge.consumer.work_item_id)
        };
        let reconciled: Vec<EdgeSnapshot> = stored
            .iter()
            .filter(|edge| !replaced(edge))
            .cloned()
            .collect();
        let mut proposed: Vec<EdgeSnapshot> = Vec::new();
        let mut graph: Vec<(WorkRef, WorkRef)> = reconciled
            .iter()
            .map(|edge| (edge.consumer.clone(), edge.producer.clone()))
            .collect();
        for change in &request.changes {
            let consumer = WorkRef::new(request.project_id.clone(), change.work_item_id.clone());
            for artifact in &change.inputs {
                let producer = artifact.producer(&request.project_id);
                match artifact {
                    ArtifactReference::Local { path, .. } => {
                        if !stays_inside_authorized_root(&declaring.authorized_root, path) {
                            return Err(ProjectFailure::dependency(
                                "project_artifact_reference_escapes_authorized_root",
                            )
                            .with_detail(format!(
                                "{path} is outside {}",
                                declaring.authorized_root
                            )));
                        }
                    }
                    ArtifactReference::CrossProject { project_id, .. } => {
                        if !project_exists(&connection, project_id.as_str())? {
                            return Err(ProjectFailure::dependency(
                                "project_artifact_reference_unauthorized",
                            )
                            .with_detail(project_id.to_string()));
                        }
                    }
                }
                if let Some(cycle) = cycle_path(&graph, &consumer, &producer) {
                    return Err(ProjectFailure::dependency("project_dependency_cycle")
                        .with_detail(render_dependency_path(&cycle)));
                }
                graph.push((consumer.clone(), producer.clone()));
                proposed.push(EdgeSnapshot {
                    consumer: consumer.clone(),
                    producer,
                    artifact: artifact.clone(),
                });
            }
        }
        let after: Vec<EdgeSnapshot> = reconciled.iter().chain(proposed.iter()).cloned().collect();
        let declared_before = declared_items(&stored);
        let declared_after = declared_items(&after);

        // What the change reaches, before any per-item answer is read.
        let mut plan: Vec<(WorkRef, Vec<WorkRef>, ChangeImpact)> = Vec::new();
        let mut seen: BTreeSet<WorkRef> = BTreeSet::new();
        let mut queue: VecDeque<(WorkRef, Vec<WorkRef>)> = VecDeque::new();
        for change in &request.changes {
            let work = WorkRef::new(request.project_id.clone(), change.work_item_id.clone());
            if seen.insert(work.clone()) {
                let path = vec![work.clone()];
                plan.push((work.clone(), path.clone(), ChangeImpact::Declared));
                queue.push_back((work, path));
            }
        }
        let consumers = consumers_of(&after);
        while let Some((work, path)) = queue.pop_front() {
            for consumer in consumers.get(&work).into_iter().flatten() {
                if seen.insert(consumer.clone()) {
                    let mut reached = path.clone();
                    reached.push(consumer.clone());
                    plan.push((consumer.clone(), reached.clone(), ChangeImpact::Consumer));
                    queue.push_back((consumer.clone(), reached));
                }
            }
        }

        // References that start resolving, then references that stop resolving.
        let declarations = declaration_provenance(request);
        let removals = removal_provenance(&stored, &declared, &request.project_id);
        for edge in &after {
            let ArtifactReference::CrossProject {
                project_id,
                work_item_id,
            } = &edge.artifact
            else {
                continue;
            };
            let referent = WorkRef::new(project_id.clone(), work_item_id.clone());
            if declared_before.contains(&referent)
                || !declared_after.contains(&referent)
                || seen.contains(&edge.consumer)
            {
                continue;
            }
            seen.insert(edge.consumer.clone());
            let seed = declarations
                .get(&referent)
                .cloned()
                .unwrap_or_else(|| referent.clone());
            plan.push((
                edge.consumer.clone(),
                declaration_path(&seed, &referent, &edge.consumer),
                ChangeImpact::Unblocked,
            ));
        }
        for edge in &stored {
            let ArtifactReference::CrossProject {
                project_id,
                work_item_id,
            } = &edge.artifact
            else {
                continue;
            };
            let referent = WorkRef::new(project_id.clone(), work_item_id.clone());
            if !declared_before.contains(&referent)
                || declared_after.contains(&referent)
                || seen.contains(&edge.consumer)
            {
                continue;
            }
            seen.insert(edge.consumer.clone());
            let seed = removals
                .get(&referent)
                .cloned()
                .unwrap_or_else(|| referent.clone());
            plan.push((
                edge.consumer.clone(),
                declaration_path(&seed, &referent, &edge.consumer),
                ChangeImpact::Blocked,
            ));
        }

        let affected_works: BTreeSet<WorkRef> =
            plan.iter().map(|(work, _, _)| work.clone()).collect();
        let mut affected = Vec::with_capacity(plan.len());
        for (work, path, impact) in plan {
            let inputs =
                declared_inputs(&connection, &after, &declared_after, &affected_works, &work)?;
            let observed = activity.activity(&work);
            affected.push(AffectedWorkItem {
                work,
                path,
                impact,
                inputs,
                activity: observed,
                handoff: handoff_of(observed),
            });
        }

        let touched: BTreeSet<&ProjectId> = affected
            .iter()
            .map(|entry| &entry.work.project_id)
            .collect();
        let untouched_projects = registered_projects(&connection)?
            .into_iter()
            .map(|project| project.project_id)
            .filter(|project_id| !touched.contains(project_id))
            .collect();
        Ok(ChangePreview {
            project_id: request.project_id.clone(),
            affected,
            untouched_projects,
        })
    }
}

/// Admit the request's own shape, or refuse it by name.
fn declare_request(request: &ChangeRequest) -> Result<Vec<WorkItemId>, ProjectFailure> {
    if request.changes.is_empty() {
        // An empty change would preview as "nothing is affected", which is a
        // claim about a request that declared nothing at all.
        return Err(ProjectFailure::change("project_change_required"));
    }
    let inputs: usize = request
        .changes
        .iter()
        .map(|change| change.inputs.len())
        .sum();
    if request.changes.len() > MAX_CHANGE_WORK_ITEMS || inputs > MAX_CHANGE_INPUTS {
        return Err(
            ProjectFailure::change("project_change_limit_exceeded").with_detail(format!(
                "{} work items, {inputs} inputs",
                request.changes.len()
            )),
        );
    }
    let mut declared = Vec::with_capacity(request.changes.len());
    for change in &request.changes {
        if declared.contains(&change.work_item_id) {
            return Err(ProjectFailure::change("project_change_work_item_duplicate")
                .with_detail(change.work_item_id.to_string()));
        }
        declared.push(change.work_item_id.clone());
    }
    Ok(declared)
}

/// The work items the dependency index declares, by the rule the stored
/// artifact states answer with: every consumer, and every local producer.
fn declared_items(edges: &[EdgeSnapshot]) -> BTreeSet<WorkRef> {
    let mut declared = BTreeSet::new();
    for edge in edges {
        declared.insert(edge.consumer.clone());
        if matches!(edge.artifact, ArtifactReference::Local { .. }) {
            declared.insert(edge.producer.clone());
        }
    }
    declared
}

/// The consumers of each producer, over one edge set.
fn consumers_of(edges: &[EdgeSnapshot]) -> BTreeMap<WorkRef, BTreeSet<WorkRef>> {
    let mut consumers: BTreeMap<WorkRef, BTreeSet<WorkRef>> = BTreeMap::new();
    for edge in edges {
        consumers
            .entry(edge.producer.clone())
            .or_default()
            .insert(edge.consumer.clone());
    }
    consumers
}

/// Which work item of the change declares each work item the change declares.
fn declaration_provenance(request: &ChangeRequest) -> BTreeMap<WorkRef, WorkRef> {
    let mut declares = BTreeMap::new();
    for change in &request.changes {
        let seed = WorkRef::new(request.project_id.clone(), change.work_item_id.clone());
        if !change.inputs.is_empty() {
            declares.entry(seed.clone()).or_insert(seed.clone());
        }
        for artifact in &change.inputs {
            if matches!(artifact, ArtifactReference::Local { .. }) {
                declares
                    .entry(artifact.producer(&request.project_id))
                    .or_insert(seed.clone());
            }
        }
    }
    declares
}

/// Which work item of the change stops declaring each work item it stops
/// declaring: the re-declared work item whose replaced inputs named it.
fn removal_provenance(
    stored: &[EdgeSnapshot],
    declared: &[WorkItemId],
    project_id: &ProjectId,
) -> BTreeMap<WorkRef, WorkRef> {
    let mut removals = BTreeMap::new();
    for edge in stored {
        if edge.consumer.project_id != *project_id
            || !declared.contains(&edge.consumer.work_item_id)
        {
            continue;
        }
        let seed = edge.consumer.clone();
        removals.entry(seed.clone()).or_insert(seed.clone());
        if matches!(edge.artifact, ArtifactReference::Local { .. }) {
            removals
                .entry(edge.producer.clone())
                .or_insert(seed.clone());
        }
    }
    removals
}

/// The path that explains a reference which starts or stops resolving.
fn declaration_path(seed: &WorkRef, referent: &WorkRef, consumer: &WorkRef) -> Vec<WorkRef> {
    if seed == referent {
        vec![referent.clone(), consumer.clone()]
    } else {
        vec![seed.clone(), referent.clone(), consumer.clone()]
    }
}

/// The declared inputs of one work item under the change.
fn declared_inputs(
    connection: &Connection,
    edges: &[EdgeSnapshot],
    declared_after: &BTreeSet<WorkRef>,
    affected_works: &BTreeSet<WorkRef>,
    consumer: &WorkRef,
) -> Result<Vec<DeclaredInputState>, ProjectFailure> {
    let mut inputs = Vec::new();
    for edge in edges.iter().filter(|edge| &edge.consumer == consumer) {
        let state = match &edge.artifact {
            ArtifactReference::Local { .. } => {
                dependency_state(connection, &edge.consumer.project_id, &edge.artifact)?
            }
            ArtifactReference::CrossProject { .. } => {
                if declared_after.contains(&edge.producer) {
                    ArtifactState::Materialized
                } else {
                    ArtifactState::Missing
                }
            }
        };
        inputs.push(DeclaredInputState {
            producer: edge.producer.clone(),
            artifact: edge.artifact.clone(),
            state,
            pending: affected_works.contains(&edge.producer),
        });
    }
    Ok(inputs)
}

/// What the change requires of the previous result behind one activity answer.
const fn handoff_of(activity: WorkActivity) -> ChangeHandoff {
    match activity {
        WorkActivity::NotStarted => ChangeHandoff::None,
        WorkActivity::InFlight | WorkActivity::Accepted => ChangeHandoff::Required,
        WorkActivity::Unknown => ChangeHandoff::Unresolved,
    }
}
