//! Ownership-aware retention and bounded collection.
//!
//! Retention follows *reachability from a root*, not age or count alone. A root
//! is a durable fact that still needs the node: the project itself, a goal the
//! project committed to, an execution input that is currently in use, an
//! explicit human retain, unfinished workflow work, an acceptance record, or
//! material that has not been delivered yet.
//!
//! Associations carry reachability. Owning edges do **not**: a temporary
//! holding created by one execution never keeps a node alive on its own, so a
//! rootless association cycle is collected while active inputs and explicitly
//! retained content remain.
//!
//! Collection runs on high/low watermarks: nothing happens below the high
//! watermark, and one pass collects at most [`MAX_COLLECTION_BATCH`] nodes,
//! stopping early once the low watermark is reached. Every pass records
//! non-sensitive reasons and counts; no cleanup call leaves this host.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::graph::{
    CollaborationGraph, CurrentNode, EdgeKind, NodeId, NodeKind, ProjectId, RetentionReason, Root,
};

/// Upper bound on nodes one collection pass reclaims.
pub const MAX_COLLECTION_BATCH: usize = 512;

/// The high/low watermark pair that bounds collection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Watermarks {
    /// Collection starts only when the project holds more nodes than this.
    pub high: usize,
    /// One pass stops as soon as the project holds this many nodes or fewer.
    pub low: usize,
}

impl Default for Watermarks {
    fn default() -> Self {
        Self {
            high: 2_048,
            low: 1_536,
        }
    }
}

impl Watermarks {
    /// A watermark pair is usable only when the low mark is strictly below the
    /// high mark, so one pass always makes progress.
    pub fn is_usable(&self) -> bool {
        self.low < self.high
    }
}

/// Why one node was kept. Reasons are non-sensitive: a node identity and a
/// retention class, never content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionNote {
    pub node: NodeId,
    pub reason: RetentionReason,
}

/// The outcome of one collection pass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionRecord {
    /// True when the pass ran at all.
    pub ran: bool,
    /// Nodes reclaimed by this pass.
    pub collected: Vec<NodeId>,
    /// Nodes still held, with the reason that holds them.
    pub retained: Vec<RetentionNote>,
    /// Nodes remaining in the project after the pass.
    pub remaining: usize,
}

impl CollectionRecord {
    /// A pass that ran because the high watermark was crossed.
    fn idle(remaining: usize) -> Self {
        Self {
            ran: false,
            collected: Vec::new(),
            retained: Vec::new(),
            remaining,
        }
    }

    pub fn collected_count(&self) -> usize {
        self.collected.len()
    }
}

/// Every root that holds one project's content, derived from the graph itself.
///
/// The project node is always a root; a goal or a project constraint is a
/// durable commitment. Roots that come from other owners — active inputs,
/// explicit retains, unfinished work, acceptance records, undelivered material
/// — are supplied by the caller through `external` because those owners own
/// them, not this graph.
pub fn retention_roots(graph: &CollaborationGraph, project: &ProjectId) -> Vec<Root> {
    let mut roots = Vec::new();
    for node in graph.nodes_of_project(project) {
        if !node.is_current() {
            continue;
        }
        let reason = match node.kind {
            NodeKind::Project => Some(RetentionReason::ProjectRoot),
            NodeKind::Goal | NodeKind::ProjectConstraint => {
                Some(RetentionReason::DurableCommitment)
            }
            _ => None,
        };
        if let Some(reason) = reason {
            roots.push(Root {
                node: node.id.clone(),
                reason,
            });
        }
    }
    roots.sort_by(|left, right| left.node.cmp(&right.node));
    roots
}

/// The decision for one project, before anything is removed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionPlan {
    /// Nodes no root reaches, in deterministic order.
    pub collectable: Vec<NodeId>,
    /// Retained nodes and the reason that retains them.
    pub retained: Vec<RetentionNote>,
    /// Whether the plan is empty because the project is under the high mark.
    pub below_high_watermark: bool,
}

/// Plan one collection pass without changing the graph.
///
/// Roots are combined from the graph's own durable commitments and the
/// caller-supplied external roots. Reachability follows associations only.
pub fn plan_collection(
    graph: &CollaborationGraph,
    project: &ProjectId,
    external_roots: &[Root],
    watermarks: Watermarks,
) -> CollectionPlan {
    let mut roots = retention_roots(graph, project);
    roots.extend(external_roots.iter().cloned());
    let reachable = graph.reachable_from_roots(&roots);

    let mut collectable = Vec::new();
    let mut retained = Vec::new();
    for node in graph.nodes_of_project(project) {
        if !node.is_current() {
            continue;
        }
        if reachable.contains(&node.id) {
            retained.push(RetentionNote {
                node: node.id.clone(),
                reason: reason_for(&roots, &node.id),
            });
        } else {
            collectable.push(node.id.clone());
        }
    }
    collectable.sort();
    retained.sort_by(|left, right| left.node.cmp(&right.node));

    let held = retained.len() + collectable.len();
    CollectionPlan {
        collectable,
        retained,
        below_high_watermark: held <= watermarks.high,
    }
}

/// The reason that holds one reachable node: its own root reason when it is a
/// root, otherwise the reason of the nearest root that reaches it, reported as
/// the root's own class.
fn reason_for(roots: &[Root], node: &NodeId) -> RetentionReason {
    roots
        .iter()
        .find(|root| &root.node == node)
        .map(|root| root.reason)
        .unwrap_or(RetentionReason::ActiveInput)
}

/// Apply one bounded collection pass.
///
/// A project at or below its high watermark is left untouched. Otherwise up to
/// [`MAX_COLLECTION_BATCH`] collectable nodes are removed, stopping early once
/// the project holds no more than the low watermark. The caller persists the
/// resulting removals through the same fenced store writes as any other
/// mutation; this function only decides and reports.
pub fn collect(
    graph: &CollaborationGraph,
    project: &ProjectId,
    external_roots: &[Root],
    watermarks: Watermarks,
    now_unix_ms: i64,
    removal_reason: &str,
) -> (CollectionRecord, Vec<(NodeId, crate::graph::Removal)>) {
    let plan = plan_collection(graph, project, external_roots, watermarks);
    let remaining = plan.retained.len() + plan.collectable.len();
    if plan.below_high_watermark || !watermarks.is_usable() {
        return (CollectionRecord::idle(remaining), Vec::new());
    }
    let budget = remaining
        .saturating_sub(watermarks.low)
        .min(MAX_COLLECTION_BATCH);
    let mut collected = Vec::new();
    let mut removals = Vec::new();
    for node in plan.collectable.into_iter().take(budget) {
        removals.push((
            node.clone(),
            crate::graph::Removal {
                removed_by: OpaqueRemover::id(),
                removed_at_unix_ms: now_unix_ms,
                reason: removal_reason.to_owned(),
            },
        ));
        collected.push(node);
    }
    let record = CollectionRecord {
        ran: true,
        collected,
        retained: plan.retained,
        remaining: remaining.saturating_sub(removals.len()),
    };
    (record, removals)
}

/// The identity a collection pass records as the remover. Collection is a
/// system maintenance act, so it never impersonates a human principal.
struct OpaqueRemover;

impl OpaqueRemover {
    fn id() -> crate::graph::PrincipalId {
        crate::graph::OpaqueId::new("system:graph-collection")
            .expect("the collection principal is a valid opaque id")
    }
}

/// Rootless association cycles are exactly the nodes no root reaches.
///
/// This helper exists so the collection decision can be asserted directly: it
/// returns every current node that participates in a cycle and is unreachable
/// from any root.
pub fn rootless_cycle_nodes(
    graph: &CollaborationGraph,
    project: &ProjectId,
    external_roots: &[Root],
) -> Vec<NodeId> {
    let mut roots = retention_roots(graph, project);
    roots.extend(external_roots.iter().cloned());
    let reachable = graph.reachable_from_roots(&roots);
    let mut cycle_nodes = BTreeSet::new();
    for node in graph.nodes_of_project(project) {
        if !node.is_current() || reachable.contains(&node.id) {
            continue;
        }
        if participates_in_cycle(graph, &node.id) {
            cycle_nodes.insert(node.id.clone());
        }
    }
    cycle_nodes.into_iter().collect()
}

fn participates_in_cycle(graph: &CollaborationGraph, start: &NodeId) -> bool {
    let mut seen: BTreeMap<NodeId, bool> = BTreeMap::new();
    let mut stack = vec![start.clone()];
    while let Some(current) = stack.pop() {
        for edge in graph.outgoing_edges(&current) {
            if edge.kind == EdgeKind::Owning {
                continue;
            }
            if &edge.to == start {
                return true;
            }
            if seen.insert(edge.to.clone(), true).is_none() {
                stack.push(edge.to.clone());
            }
        }
    }
    false
}

/// Release temporary execution holdings whose holder has settled.
///
/// A holding is released only for a holder the caller reports as settled. An
/// unresolved responsibility keeps its holding, so a restart reconstructs the
/// same view instead of dropping in-flight work.
pub fn released_holdings(
    graph: &CollaborationGraph,
    project: &ProjectId,
    settled_holders: &BTreeSet<crate::graph::PrincipalId>,
) -> Vec<crate::graph::EdgeId> {
    let mut released = Vec::new();
    for edge in graph.edges() {
        if &edge.project_id != project || !edge.is_current() {
            continue;
        }
        if edge.kind != EdgeKind::Owning {
            continue;
        }
        if edge
            .holder
            .as_ref()
            .is_some_and(|holder| settled_holders.contains(holder))
        {
            released.push(edge.id.clone());
        }
    }
    released.sort();
    released
}

/// The kinds a collection pass may reclaim once nothing reaches them.
pub fn is_collectable_kind(kind: NodeKind) -> bool {
    kind.is_temporary()
        || matches!(
            kind,
            NodeKind::Artifact | NodeKind::Decision | NodeKind::Task
        )
}

/// One current node that a collection pass would reclaim.
pub fn collectable_nodes<'a>(
    graph: &'a CollaborationGraph,
    project: &ProjectId,
    external_roots: &[Root],
    watermarks: Watermarks,
) -> Vec<&'a CurrentNode> {
    let plan = plan_collection(graph, project, external_roots, watermarks);
    plan.collectable
        .iter()
        .filter_map(|id| graph.node(id.as_str()))
        .filter(|node| is_collectable_kind(node.kind))
        .collect()
}
