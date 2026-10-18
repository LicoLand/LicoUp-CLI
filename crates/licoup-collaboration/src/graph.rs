//! Mutable current values of the project collaboration graph.
//!
//! The graph holds what a project team currently believes and how its work
//! items relate. It is deliberately *not* a history ledger: the Canonical
//! Conversation owns conversation events, the workflow store owns runs and
//! commands, and each evidence/document/preference owner keeps its own durable
//! record. This owner keeps one *current* value per node, the explicit
//! alternatives that coexist with it, and the indexed relations between nodes
//! so a change can invalidate exactly the reachable dependents.
//!
//! Every mutation is fenced by the current owner and revision of the node or
//! edge it targets, so a concurrent write, a stale handoff result and a
//! replacement of a dependency cannot silently overwrite current state.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Upper bound on nodes one in-memory graph holds.
pub const MAX_GRAPH_NODES: usize = 4_096;
/// Upper bound on edges one in-memory graph holds.
pub const MAX_GRAPH_EDGES: usize = 16_384;
/// Upper bound on coexisting alternatives per node.
pub const MAX_NODE_ALTERNATIVES: usize = 16;
/// Upper bound on one node's serialized body.
pub const MAX_BODY_BYTES: usize = 262_144;

/// A validated opaque identifier. Identifiers are never parsed for meaning.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OpaqueId(String);

impl OpaqueId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        ensure!(
            !value.trim().is_empty()
                && value == value.trim()
                && value.len() <= 160
                && !value.chars().any(char::is_control),
            "collaboration_id_invalid"
        );
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OpaqueId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A project is the authorization and collection unit of the graph.
pub type ProjectId = OpaqueId;
/// A graph node.
pub type NodeId = OpaqueId;
/// A graph edge.
pub type EdgeId = OpaqueId;
/// The principal that authors and owns values.
pub type PrincipalId = OpaqueId;

/// A monotonic revision of one node or edge.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Revision(pub u64);

impl Revision {
    pub const ZERO: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// What a node is about. The kind drives context selection and retention.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    /// The project root itself.
    Project,
    /// A durable goal the project committed to.
    Goal,
    /// A unit of assigned work.
    Task,
    /// A durable decision record.
    Decision,
    /// A produced or referenced artifact handle.
    Artifact,
    /// A project-scoped constraint.
    ProjectConstraint,
    /// A global preference. Global preferences never become project facts.
    GlobalPreference,
    /// Temporary debug material. Always reclaimable.
    DebugNote,
}

impl NodeKind {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Goal => "goal",
            Self::Task => "task",
            Self::Decision => "decision",
            Self::Artifact => "artifact",
            Self::ProjectConstraint => "project-constraint",
            Self::GlobalPreference => "global-preference",
            Self::DebugNote => "debug-note",
        }
    }

    /// True when the kind is reconstructable or explicitly temporary, so a
    /// collection pass may reclaim it once nothing reachable holds it.
    pub const fn is_temporary(self) -> bool {
        matches!(self, Self::DebugNote)
    }

    /// True when the value is a global preference rather than a project fact.
    /// Context selection keeps the two apart instead of promoting one into the
    /// other.
    pub const fn is_global_preference(self) -> bool {
        matches!(self, Self::GlobalPreference)
    }
}

/// Where a value came from. Provenance is retained across corrections so a
/// superseded preference can still say who stated it and when.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceSource {
    /// A human stated it in conversation.
    HumanStatement,
    /// An assistant proposed it.
    AgentInference,
    /// It was imported from a project document owned elsewhere.
    ImportedDocument,
    /// It was captured as temporary debug material.
    DebugCapture,
}

/// The author and origin of one value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub author: PrincipalId,
    pub source: ProvenanceSource,
    pub recorded_at_unix_ms: i64,
}

impl Provenance {
    pub fn new(author: PrincipalId, source: ProvenanceSource, recorded_at_unix_ms: i64) -> Self {
        Self {
            author,
            source,
            recorded_at_unix_ms,
        }
    }
}

/// One current value with its provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeValue {
    pub label: String,
    pub body: Value,
    pub provenance: Provenance,
}

impl NodeValue {
    pub fn new(label: impl Into<String>, body: Value, provenance: Provenance) -> Result<Self> {
        let label = label.into();
        ensure!(
            !label.trim().is_empty() && label.len() <= 240,
            "collaboration_label_invalid"
        );
        ensure!(
            serde_json::to_vec(&body)?.len() <= MAX_BODY_BYTES,
            "collaboration_body_too_large"
        );
        Ok(Self {
            label,
            body,
            provenance,
        })
    }
}

/// An explicit alternative that coexists with the current value.
///
/// Alternatives are not history: a project may hold several accepted readings
/// at once and switch between them without losing the others.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alternative {
    pub id: OpaqueId,
    pub value: NodeValue,
}

/// Why a node is no longer current. The node is kept as a tombstone so edges
/// that still point at it can be detected instead of silently dangling.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removal {
    pub removed_by: PrincipalId,
    pub removed_at_unix_ms: i64,
    pub reason: String,
}

/// One current graph node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentNode {
    pub id: NodeId,
    pub project_id: ProjectId,
    pub kind: NodeKind,
    pub value: NodeValue,
    pub revision: Revision,
    pub owner: PrincipalId,
    /// The chosen alternative, when the node holds several readings.
    pub selected_alternative: Option<OpaqueId>,
    pub alternatives: Vec<Alternative>,
    pub removal: Option<Removal>,
}

impl CurrentNode {
    pub fn is_current(&self) -> bool {
        self.removal.is_none()
    }

    /// The value this node currently presents: either its own value or the
    /// selected alternative.
    pub fn effective_value(&self) -> &NodeValue {
        match self.selected_alternative.as_ref().and_then(|selected| {
            self.alternatives
                .iter()
                .find(|alternative| &alternative.id == selected)
        }) {
            Some(alternative) => &alternative.value,
            None => &self.value,
        }
    }
}

/// Whether an edge owns temporary execution state or states a durable relation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// A durable relation between two nodes.
    Association,
    /// A temporary holding edge created by one execution. It is released when
    /// the execution settles; it never outlives the work that created it.
    Owning,
}

/// What an association states.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Relation {
    /// `from` depends on `to`.
    DependsOn,
    /// `from` refines `to`.
    Refines,
    /// `from` supersedes `to`.
    Supersedes,
    /// `from` was produced by `to`.
    ProducedBy,
    /// `to` holds `from` for the recorded execution.
    HeldBy,
    /// `from` is temporary debug material attached to `to`.
    DebugOf,
}

/// One graph edge.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub id: EdgeId,
    pub project_id: ProjectId,
    pub kind: EdgeKind,
    pub relation: Relation,
    pub from: NodeId,
    pub to: NodeId,
    pub revision: Revision,
    pub owner: PrincipalId,
    /// The execution that created an owning edge, and therefore the only
    /// authority that may release it.
    pub holder: Option<PrincipalId>,
    pub removal: Option<Removal>,
}

impl Edge {
    pub fn is_current(&self) -> bool {
        self.removal.is_none()
    }
}

/// One value of the graph that reachability marking starts from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub node: NodeId,
    pub reason: RetentionReason,
}

/// Why a node, or something reachable from it, must not be collected.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionReason {
    /// The project root itself.
    ProjectRoot,
    /// A goal or constraint the project still committed to.
    DurableCommitment,
    /// An execution is currently using it.
    ActiveInput,
    /// A human explicitly asked to retain it.
    ExplicitlyRetained,
    /// Unfinished workflow work still refers to it.
    UnfinishedWork,
    /// An acceptance record still refers to it.
    AcceptanceRecord,
    /// Material that has not been delivered yet.
    Undelivered,
}

/// The mutable current graph with indexed adjacency and dependents.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationGraph {
    nodes: BTreeMap<NodeId, CurrentNode>,
    edges: BTreeMap<EdgeId, Edge>,
    /// Outgoing adjacency: node -> edges whose `from` is that node.
    outgoing: BTreeMap<NodeId, BTreeSet<EdgeId>>,
    /// Dependents index: node -> edges whose `to` is that node.
    incoming: BTreeMap<NodeId, BTreeSet<EdgeId>>,
}

impl CollaborationGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node(&self, id: &str) -> Option<&CurrentNode> {
        self.nodes.get(&OpaqueId(id.to_owned()))
    }

    pub fn edge(&self, id: &str) -> Option<&Edge> {
        self.edges.get(&OpaqueId(id.to_owned()))
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &CurrentNode> {
        self.nodes.values()
    }

    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.values()
    }

    pub fn nodes_of_project(&self, project: &ProjectId) -> impl Iterator<Item = &CurrentNode> {
        self.nodes
            .values()
            .filter(move |node| &node.project_id == project)
    }

    /// Insert a node. An existing identifier is an identity conflict rather
    /// than an overwrite, so concurrent creators cannot corrupt each other.
    pub fn insert_node(&mut self, node: CurrentNode) -> Result<Revision> {
        ensure!(
            self.nodes.len() < MAX_GRAPH_NODES,
            "collaboration_graph_node_limit"
        );
        ensure!(
            !self.nodes.contains_key(&node.id),
            "collaboration_node_identity_conflict"
        );
        let revision = node.revision;
        self.nodes.insert(node.id.clone(), node);
        Ok(revision)
    }

    /// Replace a node's value when `expected` still matches the stored
    /// revision and `owner` still owns it.
    ///
    /// Returns the new revision, the previous owner, and the reachable nodes
    /// whose current state a reader must now re-derive.
    pub fn replace_node_value(
        &mut self,
        id: &NodeId,
        owner: &PrincipalId,
        expected: Revision,
        replace: impl FnOnce(&CurrentNode) -> Result<NodeValue>,
    ) -> Result<(Revision, PrincipalId, Vec<NodeId>)> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("collaboration_node_unknown"))?;
        ensure!(node.removal.is_none(), "collaboration_node_removed");
        ensure!(
            node.revision == expected && &node.owner == owner,
            "collaboration_node_fence_lost"
        );
        let previous_owner = node.owner.clone();
        node.value = replace(node)?;
        node.revision = node.revision.next();
        let revision = node.revision;
        let affected = self.reachable_dependents(id);
        Ok((revision, previous_owner, affected))
    }

    /// Attach a coexisting alternative and select it. Other alternatives stay
    /// readable, so a correction supersedes a reading without destroying it.
    pub fn select_alternative(
        &mut self,
        id: &NodeId,
        owner: &PrincipalId,
        expected: Revision,
        alternative_id: &OpaqueId,
        value: NodeValue,
    ) -> Result<(Revision, Vec<NodeId>)> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("collaboration_node_unknown"))?;
        ensure!(node.removal.is_none(), "collaboration_node_removed");
        ensure!(
            node.revision == expected && &node.owner == owner,
            "collaboration_node_fence_lost"
        );
        match node
            .alternatives
            .iter_mut()
            .find(|alternative| &alternative.id == alternative_id)
        {
            Some(existing) => existing.value = value,
            None => {
                ensure!(
                    node.alternatives.len() < MAX_NODE_ALTERNATIVES,
                    "collaboration_alternative_limit"
                );
                node.alternatives.push(Alternative {
                    id: alternative_id.clone(),
                    value,
                });
            }
        }
        node.selected_alternative = Some(alternative_id.clone());
        node.revision = node.revision.next();
        let revision = node.revision;
        let affected = self.reachable_dependents(id);
        Ok((revision, affected))
    }

    /// Remove a node, keeping it as a tombstone so dangling edges are visible
    /// instead of silently reconnecting unrelated subgraphs.
    pub fn remove_node(
        &mut self,
        id: &NodeId,
        owner: &PrincipalId,
        expected: Revision,
        removal: Removal,
    ) -> Result<(Revision, Vec<NodeId>)> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("collaboration_node_unknown"))?;
        ensure!(node.removal.is_none(), "collaboration_node_removed");
        ensure!(
            node.revision == expected && &node.owner == owner,
            "collaboration_node_fence_lost"
        );
        node.removal = Some(removal);
        node.revision = node.revision.next();
        let revision = node.revision;
        let affected = self.reachable_dependents(id);
        Ok((revision, affected))
    }

    /// Insert an edge whose endpoints exist and are current.
    pub fn insert_edge(&mut self, edge: Edge) -> Result<Revision> {
        ensure!(
            self.edges.len() < MAX_GRAPH_EDGES,
            "collaboration_graph_edge_limit"
        );
        ensure!(
            !self.edges.contains_key(&edge.id),
            "collaboration_edge_identity_conflict"
        );
        for endpoint in [&edge.from, &edge.to] {
            let node = self
                .nodes
                .get(endpoint)
                .ok_or_else(|| anyhow::anyhow!("collaboration_edge_endpoint_unknown"))?;
            ensure!(
                node.removal.is_none(),
                "collaboration_edge_endpoint_removed"
            );
            ensure!(
                node.project_id == edge.project_id,
                "collaboration_edge_project_mismatch"
            );
        }
        ensure!(
            edge.kind != EdgeKind::Owning || edge.holder.is_some(),
            "collaboration_owning_edge_needs_holder"
        );
        self.outgoing
            .entry(edge.from.clone())
            .or_default()
            .insert(edge.id.clone());
        self.incoming
            .entry(edge.to.clone())
            .or_default()
            .insert(edge.id.clone());
        let revision = edge.revision;
        self.edges.insert(edge.id.clone(), edge);
        Ok(revision)
    }

    /// Replace the target of an edge, which is how a dependency gets swapped.
    ///
    /// The affected set is the reachable neighbourhood of the edge's source
    /// before **and** after the swap, so a node that only this edge reached is
    /// invalidated as well. Nothing outside that neighbourhood is touched, so
    /// unrelated subgraphs stay valid.
    pub fn replace_edge_target(
        &mut self,
        id: &EdgeId,
        owner: &PrincipalId,
        expected: Revision,
        to: NodeId,
    ) -> Result<(Revision, Vec<NodeId>)> {
        let (previous_to, from, project_id) = {
            let edge = self
                .edges
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("collaboration_edge_unknown"))?;
            ensure!(edge.removal.is_none(), "collaboration_edge_removed");
            ensure!(
                edge.revision == expected && &edge.owner == owner,
                "collaboration_edge_fence_lost"
            );
            (edge.to.clone(), edge.from.clone(), edge.project_id.clone())
        };
        let target = self
            .nodes
            .get(&to)
            .ok_or_else(|| anyhow::anyhow!("collaboration_edge_endpoint_unknown"))?;
        ensure!(
            target.removal.is_none(),
            "collaboration_edge_endpoint_removed"
        );
        ensure!(
            target.project_id == project_id,
            "collaboration_edge_project_mismatch"
        );
        let before = self.reachable_dependents(&from);
        if let Some(bucket) = self.incoming.get_mut(&previous_to) {
            bucket.remove(id);
        }
        self.incoming
            .entry(to.clone())
            .or_default()
            .insert(id.clone());
        let edge = self
            .edges
            .get_mut(id)
            .expect("edge exists after the fence check");
        edge.to = to;
        edge.revision = edge.revision.next();
        let revision = edge.revision;
        let mut affected = before;
        affected.extend(self.reachable_dependents(&from));
        affected.sort();
        affected.dedup();
        Ok((revision, affected))
    }

    /// Release one owning edge. Only its recorded holder, or the node owner who
    /// created it, may release it, so executor exit cannot release a holding
    /// that another execution still needs.
    pub fn release_owning_edge(
        &mut self,
        id: &EdgeId,
        holder: &PrincipalId,
        removal: Removal,
    ) -> Result<()> {
        let edge = self
            .edges
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("collaboration_edge_unknown"))?;
        ensure!(
            edge.kind == EdgeKind::Owning,
            "collaboration_edge_not_owning"
        );
        ensure!(edge.removal.is_none(), "collaboration_edge_removed");
        ensure!(
            edge.holder.as_ref() == Some(holder),
            "collaboration_edge_holder_mismatch"
        );
        edge.removal = Some(removal);
        edge.revision = edge.revision.next();
        Ok(())
    }

    /// Current outgoing edges of one node, read through the adjacency index.
    pub fn outgoing_edges(&self, id: &NodeId) -> Vec<&Edge> {
        self.edges_at(&self.outgoing, id)
    }

    /// Current incoming edges of one node, read through the dependents index.
    pub fn incoming_edges(&self, id: &NodeId) -> Vec<&Edge> {
        self.edges_at(&self.incoming, id)
    }

    fn edges_at<'a>(
        &'a self,
        index: &'a BTreeMap<NodeId, BTreeSet<EdgeId>>,
        id: &NodeId,
    ) -> Vec<&'a Edge> {
        index
            .get(id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|edge_id| self.edges.get(edge_id))
                    .filter(|edge| edge.is_current())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every current node reachable from `start` by following outgoing edges,
    /// excluding `start` itself. Callers use this to invalidate exactly the
    /// affected dependents instead of the whole graph.
    pub fn reachable_dependents(&self, start: &NodeId) -> Vec<NodeId> {
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start.clone());
        while let Some(current) = queue.pop_front() {
            for edge in self.outgoing_edges(&current) {
                if seen.insert(edge.to.clone()) {
                    queue.push_back(edge.to.clone());
                }
            }
        }
        seen.remove(start);
        seen.into_iter().collect()
    }

    /// Every current node reachable from any root. Owning edges are not
    /// traversed, so a temporary holding never keeps a node alive by itself.
    pub fn reachable_from_roots(&self, roots: &[Root]) -> BTreeSet<NodeId> {
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut queue = VecDeque::new();
        for root in roots {
            if self
                .nodes
                .get(&root.node)
                .is_some_and(|node| node.is_current())
                && seen.insert(root.node.clone())
            {
                queue.push_back(root.node.clone());
            }
        }
        while let Some(current) = queue.pop_front() {
            for edge in self.outgoing_edges(&current) {
                if edge.kind == EdgeKind::Owning {
                    continue;
                }
                if seen.insert(edge.to.clone()) {
                    queue.push_back(edge.to.clone());
                }
            }
        }
        seen
    }
}
