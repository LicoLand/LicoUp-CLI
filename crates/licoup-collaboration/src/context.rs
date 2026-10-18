//! Task-authorized graph context, preferences and corrections.
//!
//! A consumer asks for the context of one task through a named port. The port
//! answers with a slice of the project's *current* graph values, its project
//! constraints and the global preferences that apply, and it keeps those three
//! apart: a global preference stays global and a project constraint stays
//! project-scoped, so neither is silently promoted into the other.
//!
//! Every entry carries the provenance recorded when the value was written. A
//! correction supersedes a preference for one project without rewriting the
//! global preference, and an unauthorized project yields no context at all.
//!
//! Raw evidence, project documents, durable commitments and optional long-term
//! memory keep their own owners. This port therefore answers with graph values
//! and handles; it never inlines another owner's payload.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::authority::{AuthorityDenial, GraphSession, Permission};
use crate::graph::{
    CollaborationGraph, CurrentNode, Edge, EdgeId, EdgeKind, NodeId, NodeKind, NodeValue, OpaqueId,
    PrincipalId, ProjectId, Provenance, Relation, Revision,
};

/// Upper bound on entries one context slice returns.
pub const MAX_CONTEXT_ENTRIES: usize = 256;

/// One request for the context of a task.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRequest {
    pub project_id: ProjectId,
    pub principal: PrincipalId,
    /// The task the context is for. Selection is authorized against it, so a
    /// task in another project never widens the answer.
    pub task_id: NodeId,
    /// The kinds the consumer wants. An empty set means the default selection.
    pub include_kinds: BTreeSet<NodeKind>,
    pub max_entries: usize,
}

impl ContextRequest {
    pub fn new(project_id: ProjectId, principal: PrincipalId, task_id: NodeId) -> Self {
        Self {
            project_id,
            principal,
            task_id,
            include_kinds: BTreeSet::new(),
            max_entries: MAX_CONTEXT_ENTRIES,
        }
    }

    fn bounded_limit(&self) -> usize {
        self.max_entries.clamp(1, MAX_CONTEXT_ENTRIES)
    }

    fn wants(&self, kind: NodeKind) -> bool {
        if self.include_kinds.is_empty() {
            !kind.is_global_preference()
        } else {
            self.include_kinds.contains(&kind)
        }
    }
}

/// One context entry: a current graph value with its provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEntry {
    pub node: NodeId,
    pub kind: NodeKind,
    pub label: String,
    pub body: serde_json::Value,
    pub provenance: Provenance,
    pub revision: Revision,
}

impl ContextEntry {
    fn from_node(node: &CurrentNode) -> Self {
        let value = node.effective_value();
        Self {
            node: node.id.clone(),
            kind: node.kind,
            label: value.label.clone(),
            body: value.body.clone(),
            provenance: value.provenance.clone(),
            revision: node.revision,
        }
    }
}

/// One authorized answer. The three groups are never merged, so a consumer
/// cannot accidentally treat a global preference as a project fact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSlice {
    pub project_id: ProjectId,
    pub task_id: NodeId,
    /// Project-scoped values: goals, tasks, decisions, artifacts and debug
    /// material.
    pub project_values: Vec<ContextEntry>,
    /// Project constraints, always project-scoped.
    pub project_constraints: Vec<ContextEntry>,
    /// Global preferences that apply to the task. They remain global.
    pub global_preferences: Vec<ContextEntry>,
    /// True when the selection hit [`ContextRequest::max_entries`].
    pub truncated: bool,
    /// Present when no context is authorized at all.
    pub denial: Option<AuthorityDenial>,
}

impl ContextSlice {
    /// An answer that grants nothing, used for an unauthorized request.
    fn denied(project_id: ProjectId, task_id: NodeId, denial: AuthorityDenial) -> Self {
        Self {
            project_id,
            task_id,
            project_values: Vec::new(),
            project_constraints: Vec::new(),
            global_preferences: Vec::new(),
            truncated: false,
            denial: Some(denial),
        }
    }

    /// True when the request was authorized, even if the answer is empty.
    pub fn is_authorized(&self) -> bool {
        self.denial.is_none()
    }

    /// Every entry in the order a consumer should read it: project values,
    /// then project constraints, then global preferences.
    pub fn entries(&self) -> Vec<&ContextEntry> {
        self.project_values
            .iter()
            .chain(self.project_constraints.iter())
            .chain(self.global_preferences.iter())
            .collect()
    }

    pub fn entry_count(&self) -> usize {
        self.project_values.len() + self.project_constraints.len() + self.global_preferences.len()
    }
}

/// The named port a consumer depends on.
///
/// Implementations read from the current graph; the trait exists so a consumer
/// depends on the port rather than on the store, and so a test can supply a
/// deterministic slice without a database.
pub trait GraphContextPort {
    fn task_context(&self, request: &ContextRequest) -> ContextSlice;
}

/// The production port over one graph and its grant table.
pub struct GraphContextSelection<'a> {
    graph: &'a CollaborationGraph,
    grants: &'a crate::authority::GrantTable,
}

impl<'a> GraphContextSelection<'a> {
    pub fn new(graph: &'a CollaborationGraph, grants: &'a crate::authority::GrantTable) -> Self {
        Self { graph, grants }
    }
}

impl GraphContextPort for GraphContextSelection<'_> {
    fn task_context(&self, request: &ContextRequest) -> ContextSlice {
        // A task in another project must never widen the answer, so the task is
        // resolved before authority is even consulted.
        let Some(task) = self
            .graph
            .node(request.task_id.as_str())
            .filter(|node| node.is_current() && node.project_id == request.project_id)
        else {
            return ContextSlice::denied(
                request.project_id.clone(),
                request.task_id.clone(),
                AuthorityDenial::Unauthorized,
            );
        };
        // The port only reads, so authority is checked directly through the
        // grant table rather than through a mutable session.
        if let Err(denial) =
            self.grants
                .authorize(&request.project_id, &request.principal, Permission::Read)
        {
            return ContextSlice::denied(
                request.project_id.clone(),
                request.task_id.clone(),
                denial,
            );
        }

        let limit = request.bounded_limit();
        let mut project_values = Vec::new();
        let mut project_constraints = Vec::new();
        let mut global_preferences = Vec::new();
        let mut truncated = false;

        // The task itself is always first, then its reachable dependents, so the
        // slice is the task's subgraph rather than the whole project.
        let mut ordered: Vec<&CurrentNode> = vec![task];
        for dependent in self.graph.reachable_dependents(&request.task_id) {
            if let Some(node) = self.graph.node(dependent.as_str())
                && node.is_current()
                && node.project_id == request.project_id
            {
                ordered.push(node);
            }
        }

        for node in ordered {
            if !request.wants(node.kind) {
                continue;
            }
            if project_values.len() + project_constraints.len() + global_preferences.len() >= limit
            {
                truncated = true;
                break;
            }
            let entry = ContextEntry::from_node(node);
            match node.kind {
                NodeKind::ProjectConstraint => project_constraints.push(entry),
                NodeKind::GlobalPreference => global_preferences.push(entry),
                _ => project_values.push(entry),
            }
        }

        ContextSlice {
            project_id: request.project_id.clone(),
            task_id: request.task_id.clone(),
            project_values,
            project_constraints,
            global_preferences,
            truncated,
            denial: None,
        }
    }
}

/// The outcome of applying an explicit preference correction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionOutcome {
    /// The project-scoped correction node that now supersedes the preference.
    pub correction: Option<NodeId>,
    /// The superseding edge, when one was created.
    pub edge: Option<EdgeId>,
    /// The global preference node. It is never modified by a correction.
    pub preference: NodeId,
    pub applied: bool,
    pub reason: Option<String>,
}

/// Apply an explicit preference correction for one project.
///
/// The correction becomes a project-scoped constraint with a `Supersedes` edge
/// to the global preference. The preference node keeps its value, its revision
/// and its original provenance, so a project correction never promotes project
/// facts globally and the corrected reading is still attributable.
pub fn correct_preference(
    session: &mut GraphSession<'_>,
    project: &ProjectId,
    principal: &PrincipalId,
    preference: &NodeId,
    correction_id: NodeId,
    correction: NodeValue,
    now_unix_ms: i64,
) -> CorrectionOutcome {
    let Some(node) = session.graph().node(preference.as_str()) else {
        return CorrectionOutcome::missing(preference);
    };
    if node.kind != NodeKind::GlobalPreference {
        return CorrectionOutcome {
            correction: None,
            edge: None,
            preference: preference.clone(),
            applied: false,
            reason: Some("collaboration_correction_target_not_preference".into()),
        };
    }
    if !node.is_current() {
        return CorrectionOutcome {
            correction: None,
            edge: None,
            preference: preference.clone(),
            applied: false,
            reason: Some("collaboration_correction_target_removed".into()),
        };
    }

    // A correction is project-scoped: its own project is the correcting
    // project, never the preference's origin.
    let constraint = CurrentNode {
        id: correction_id.clone(),
        project_id: project.clone(),
        kind: NodeKind::ProjectConstraint,
        value: correction,
        revision: Revision::ZERO,
        owner: principal.clone(),
        selected_alternative: None,
        alternatives: Vec::new(),
        removal: None,
    };
    let inserted = session.insert_node(project, principal, constraint);
    if !inserted.applied {
        return CorrectionOutcome {
            correction: None,
            edge: None,
            preference: preference.clone(),
            applied: false,
            reason: Some(format!("{:?}", inserted.refusal)),
        };
    }

    let edge_id = match OpaqueId::new(format!(
        "correction:{}:{}",
        correction_id.as_str(),
        preference.as_str()
    )) {
        Ok(id) => id,
        Err(error) => {
            return CorrectionOutcome {
                correction: None,
                edge: None,
                preference: preference.clone(),
                applied: false,
                reason: Some(error.to_string()),
            };
        }
    };
    let edge = Edge {
        id: edge_id.clone(),
        project_id: project.clone(),
        kind: EdgeKind::Association,
        relation: Relation::Supersedes,
        from: correction_id.clone(),
        to: preference.clone(),
        revision: Revision::ZERO,
        owner: principal.clone(),
        holder: None,
        removal: None,
    };
    let linked = session.insert_edge(project, principal, edge);
    if !linked.applied {
        return CorrectionOutcome {
            correction: Some(correction_id),
            edge: None,
            preference: preference.clone(),
            applied: false,
            reason: Some(format!("{:?}", linked.refusal)),
        };
    }
    let _ = now_unix_ms;
    CorrectionOutcome {
        correction: Some(correction_id),
        edge: Some(edge_id),
        preference: preference.clone(),
        applied: true,
        reason: None,
    }
}

impl CorrectionOutcome {
    fn missing(preference: &NodeId) -> Self {
        Self {
            correction: None,
            edge: None,
            preference: preference.clone(),
            applied: false,
            reason: Some("collaboration_correction_target_unknown".into()),
        }
    }
}

/// Remove temporary debug material attached to one node.
///
/// Debug notes are always reclaimable, so an explicit cleanup does not need a
/// watermark: it removes exactly the temporary debug nodes that name the target
/// through a `DebugOf` association, following further nested debug notes.
pub fn reclaim_debug_material(
    session: &mut GraphSession<'_>,
    project: &ProjectId,
    principal: &PrincipalId,
    target: &NodeId,
    now_unix_ms: i64,
) -> Vec<NodeId> {
    let mut candidates: Vec<NodeId> = Vec::new();
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    let mut queue = vec![target.clone()];
    while let Some(current) = queue.pop() {
        for edge in session.graph().incoming_edges(&current) {
            if edge.relation != Relation::DebugOf || !seen.insert(edge.from.clone()) {
                continue;
            }
            let Some(node) = session.graph().node(edge.from.as_str()) else {
                continue;
            };
            if node.kind.is_temporary() && node.project_id == *project {
                candidates.push(edge.from.clone());
            }
            queue.push(edge.from.clone());
        }
    }
    candidates.sort();
    candidates.dedup();

    let mut removed = Vec::new();
    for id in candidates {
        let Some(revision) = session.graph().node(id.as_str()).map(|node| node.revision) else {
            continue;
        };
        let outcome = session.remove_node(
            project,
            principal,
            &id,
            revision,
            "temporary debug material reclaimed",
            now_unix_ms,
        );
        if outcome.applied {
            removed.push(id);
        }
    }
    removed
}
