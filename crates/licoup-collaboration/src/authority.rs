//! Permission-bound writes, current-owner/revision fencing and invalidation.
//!
//! A write to the current graph names the principal that performs it, the
//! project it belongs to, the permission it relies on and the revision and
//! owner it expects to find. The write applies only when all four still hold,
//! so a concurrent writer, a stale handoff result and a replaced dependency
//! cannot corrupt current state.
//!
//! Invalidation is derived, not declared: a write returns the reachable
//! dependents of what it changed, so unrelated subgraphs stay valid.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::graph::{
    Alternative, CollaborationGraph, CurrentNode, Edge, EdgeId, NodeId, NodeValue, OpaqueId,
    PrincipalId, ProjectId, Removal, Revision,
};

/// What a principal may do inside one project.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Read the project's context. Nothing is readable without this.
    Read,
    /// Create and update nodes and edges.
    Contribute,
    /// Contribute plus grant, revoke and remove.
    Admin,
}

/// One durable grant of a permission inside one project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGrant {
    pub project_id: ProjectId,
    pub principal: PrincipalId,
    pub permission: Permission,
    pub granted_by: PrincipalId,
    pub granted_at_unix_ms: i64,
    pub revoked: bool,
}

/// Why a principal may not act.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthorityDenial {
    /// The principal holds no grant for this project, so it receives no
    /// context at all.
    Unauthorized,
    /// The grant was revoked.
    Revoked,
    /// The grant exists but is weaker than the required permission.
    Insufficient,
}

/// The durable grant table.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantTable {
    grants: BTreeMap<(ProjectId, PrincipalId), ProjectGrant>,
}

impl GrantTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record or replace one grant. Replacing an earlier revoked grant clears
    /// the revocation, because the new grant is the current authority.
    pub fn grant(&mut self, grant: ProjectGrant) {
        self.grants
            .insert((grant.project_id.clone(), grant.principal.clone()), grant);
    }

    /// Revoke one grant. A revoked grant denies immediately and is never
    /// silently widened by a later read.
    pub fn revoke(&mut self, project: &ProjectId, principal: &PrincipalId) {
        if let Some(grant) = self.grants.get_mut(&(project.clone(), principal.clone())) {
            grant.revoked = true;
        }
    }

    /// The effective permission of one principal inside one project.
    pub fn effective_permission(
        &self,
        project: &ProjectId,
        principal: &PrincipalId,
    ) -> Result<Permission, AuthorityDenial> {
        match self.grants.get(&(project.clone(), principal.clone())) {
            None => Err(AuthorityDenial::Unauthorized),
            Some(grant) if grant.revoked => Err(AuthorityDenial::Revoked),
            Some(grant) => Ok(grant.permission),
        }
    }

    /// Require at least `required` inside `project`.
    pub fn authorize(
        &self,
        project: &ProjectId,
        principal: &PrincipalId,
        required: Permission,
    ) -> Result<Permission, AuthorityDenial> {
        let effective = self.effective_permission(project, principal)?;
        if effective < required {
            return Err(AuthorityDenial::Insufficient);
        }
        Ok(effective)
    }

    /// Every project this principal may read. An unauthorized project is
    /// absent rather than empty, so a consumer cannot mistake it for a project
    /// that merely has no nodes yet.
    pub fn readable_projects(&self, principal: &PrincipalId) -> BTreeSet<ProjectId> {
        self.grants
            .values()
            .filter(|grant| {
                &grant.principal == principal
                    && !grant.revoked
                    && grant.permission >= Permission::Read
            })
            .map(|grant| grant.project_id.clone())
            .collect()
    }
}

/// Why a fenced write did not apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "refusal", rename_all = "kebab-case")]
pub enum WriteRefusal {
    /// The principal is not permitted in this project.
    Denied { reason: AuthorityDenial },
    /// The target does not exist in this project.
    UnknownTarget,
    /// The target was already removed.
    Removed,
    /// Another writer changed the target, or the owner changed.
    FenceLost {
        current_owner: PrincipalId,
        current_revision: Revision,
    },
    /// The write would violate a structural invariant.
    Rejected { reason: String },
}

/// The result of one fenced write.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub struct WriteOutcome {
    pub applied: bool,
    pub revision: Revision,
    pub previous_owner: Option<PrincipalId>,
    /// The nodes whose derived state this write invalidates.
    pub invalidated: Vec<NodeId>,
    pub refusal: Option<WriteRefusal>,
}

impl WriteOutcome {
    fn applied(
        revision: Revision,
        previous_owner: Option<PrincipalId>,
        invalidated: Vec<NodeId>,
    ) -> Self {
        Self {
            applied: true,
            revision,
            previous_owner,
            invalidated,
            refusal: None,
        }
    }

    /// A refusal: no revision, no owner change and nothing invalidated.
    pub fn refused(refusal: WriteRefusal) -> Self {
        Self {
            applied: false,
            revision: Revision::ZERO,
            previous_owner: None,
            invalidated: Vec::new(),
            refusal: Some(refusal),
        }
    }
}

/// One permission-bound, fenced graph session.
///
/// The session borrows the graph and the grant table so every mutation in one
/// logical operation shares the same authority view.
pub struct GraphSession<'a> {
    graph: &'a mut CollaborationGraph,
    grants: &'a GrantTable,
}

impl<'a> GraphSession<'a> {
    pub fn new(graph: &'a mut CollaborationGraph, grants: &'a GrantTable) -> Self {
        Self { graph, grants }
    }

    pub fn graph(&self) -> &CollaborationGraph {
        self.graph
    }

    /// Read one project's context. Without a readable grant the caller
    /// receives nothing, so an unauthorized project leaks no context.
    pub fn project_view(
        &self,
        project: &ProjectId,
        principal: &PrincipalId,
    ) -> Result<Vec<&CurrentNode>, AuthorityDenial> {
        self.grants
            .authorize(project, principal, Permission::Read)?;
        Ok(self
            .graph
            .nodes_of_project(project)
            .filter(|node| node.is_current())
            .collect())
    }

    /// Create a node under `Contribute`.
    pub fn insert_node(
        &mut self,
        project: &ProjectId,
        principal: &PrincipalId,
        node: CurrentNode,
    ) -> WriteOutcome {
        if let Err(reason) = self
            .grants
            .authorize(project, principal, Permission::Contribute)
        {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        if &node.project_id != project {
            return WriteOutcome::refused(WriteRefusal::Rejected {
                reason: "collaboration_node_project_mismatch".into(),
            });
        }
        match self.graph.insert_node(node) {
            Ok(revision) => WriteOutcome::applied(revision, None, Vec::new()),
            Err(error) => WriteOutcome::refused(WriteRefusal::Rejected {
                reason: error.to_string(),
            }),
        }
    }

    /// Replace one node's current value, fenced by owner and revision.
    pub fn replace_value(
        &mut self,
        project: &ProjectId,
        principal: &PrincipalId,
        id: &NodeId,
        expected: Revision,
        value: NodeValue,
    ) -> WriteOutcome {
        let Some(node) = self.graph.node(id.as_str()) else {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        };
        if &node.project_id != project {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        }
        if let Err(reason) = self
            .grants
            .authorize(project, principal, Permission::Contribute)
        {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        match self
            .graph
            .replace_node_value(id, principal, expected, |_| Ok(value))
        {
            Ok((revision, previous_owner, invalidated)) => {
                WriteOutcome::applied(revision, Some(previous_owner), invalidated)
            }
            Err(error) => WriteOutcome::refused(classify_fence(&error, self.graph, id)),
        }
    }

    /// Select one coexisting alternative, fenced by owner and revision.
    pub fn select_alternative(
        &mut self,
        project: &ProjectId,
        principal: &PrincipalId,
        id: &NodeId,
        expected: Revision,
        alternative: Alternative,
    ) -> WriteOutcome {
        let Some(node) = self.graph.node(id.as_str()) else {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        };
        if &node.project_id != project {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        }
        if let Err(reason) = self
            .grants
            .authorize(project, principal, Permission::Contribute)
        {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        match self.graph.select_alternative(
            id,
            principal,
            expected,
            &alternative.id,
            alternative.value,
        ) {
            Ok((revision, invalidated)) => WriteOutcome::applied(revision, None, invalidated),
            Err(error) => WriteOutcome::refused(classify_fence(&error, self.graph, id)),
        }
    }

    /// Remove a node under `Admin`, keeping a tombstone.
    pub fn remove_node(
        &mut self,
        project: &ProjectId,
        principal: &PrincipalId,
        id: &NodeId,
        expected: Revision,
        reason: &str,
        now_unix_ms: i64,
    ) -> WriteOutcome {
        let Some(node) = self.graph.node(id.as_str()) else {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        };
        if &node.project_id != project {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        }
        if let Err(reason) = self.grants.authorize(project, principal, Permission::Admin) {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        let removal = Removal {
            removed_by: principal.clone(),
            removed_at_unix_ms: now_unix_ms,
            reason: reason.to_owned(),
        };
        match self.graph.remove_node(id, principal, expected, removal) {
            Ok((revision, invalidated)) => WriteOutcome::applied(revision, None, invalidated),
            Err(error) => WriteOutcome::refused(classify_fence(&error, self.graph, id)),
        }
    }

    /// Insert an edge under `Contribute`.
    pub fn insert_edge(
        &mut self,
        project: &ProjectId,
        principal: &PrincipalId,
        edge: Edge,
    ) -> WriteOutcome {
        if let Err(reason) = self
            .grants
            .authorize(project, principal, Permission::Contribute)
        {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        if &edge.project_id != project {
            return WriteOutcome::refused(WriteRefusal::Rejected {
                reason: "collaboration_edge_project_mismatch".into(),
            });
        }
        match self.graph.insert_edge(edge) {
            Ok(revision) => WriteOutcome::applied(revision, None, Vec::new()),
            Err(error) => WriteOutcome::refused(WriteRefusal::Rejected {
                reason: error.to_string(),
            }),
        }
    }

    /// Replace the target of an edge, which is how a dependency is swapped.
    pub fn replace_edge_target(
        &mut self,
        project: &ProjectId,
        principal: &PrincipalId,
        id: &EdgeId,
        expected: Revision,
        to: NodeId,
    ) -> WriteOutcome {
        let Some(edge) = self.graph.edge(id.as_str()) else {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        };
        if &edge.project_id != project {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        }
        if let Err(reason) = self
            .grants
            .authorize(project, principal, Permission::Contribute)
        {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        match self.graph.replace_edge_target(id, principal, expected, to) {
            Ok((revision, invalidated)) => WriteOutcome::applied(revision, None, invalidated),
            Err(error) => WriteOutcome::refused(WriteRefusal::Rejected {
                reason: error.to_string(),
            }),
        }
    }

    /// Release one owning edge. The recorded holder is the only authority, so
    /// an unrelated executor exiting cannot release another execution's hold.
    pub fn release_owning_edge(
        &mut self,
        project: &ProjectId,
        holder: &PrincipalId,
        id: &EdgeId,
        reason: &str,
        now_unix_ms: i64,
    ) -> WriteOutcome {
        let Some(edge) = self.graph.edge(id.as_str()) else {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        };
        if &edge.project_id != project {
            return WriteOutcome::refused(WriteRefusal::UnknownTarget);
        }
        if let Err(reason) = self
            .grants
            .authorize(project, holder, Permission::Contribute)
        {
            return WriteOutcome::refused(WriteRefusal::Denied { reason });
        }
        let removal = Removal {
            removed_by: holder.clone(),
            removed_at_unix_ms: now_unix_ms,
            reason: reason.to_owned(),
        };
        match self.graph.release_owning_edge(id, holder, removal) {
            Ok(()) => {
                let revision = self
                    .graph
                    .edge(id.as_str())
                    .map(|edge| edge.revision)
                    .unwrap_or(Revision::ZERO);
                WriteOutcome::applied(revision, None, Vec::new())
            }
            Err(error) => WriteOutcome::refused(WriteRefusal::Rejected {
                reason: error.to_string(),
            }),
        }
    }
}

/// Turn a graph-layer fence error into the durable refusal it represents.
fn classify_fence(error: &anyhow::Error, graph: &CollaborationGraph, id: &NodeId) -> WriteRefusal {
    let message = error.to_string();
    if message.contains("removed") {
        return WriteRefusal::Removed;
    }
    if message.contains("fence_lost")
        && let Some(node) = graph.node(id.as_str())
    {
        return WriteRefusal::FenceLost {
            current_owner: node.owner.clone(),
            current_revision: node.revision,
        };
    }
    WriteRefusal::Rejected { reason: message }
}

/// One identity a write may address through the graph keyspace.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "target", rename_all = "kebab-case")]
pub enum GraphTarget {
    Node { id: NodeId },
    Edge { id: EdgeId },
}

impl GraphTarget {
    pub fn id(&self) -> &OpaqueId {
        match self {
            Self::Node { id } | Self::Edge { id } => id,
        }
    }
}
