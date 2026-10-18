//! Shared synthetic fixtures.
//!
//! Every fixture is synthetic. No test reads a real data root, a conversation,
//! a document or a live Agent.

#![allow(dead_code)]

use crate::authority::{GrantTable, Permission, ProjectGrant};
use crate::graph::{
    CollaborationGraph, CurrentNode, Edge, EdgeKind, NodeKind, NodeValue, OpaqueId, PrincipalId,
    ProjectId, Provenance, ProvenanceSource, Relation, Removal, Revision,
};
use serde_json::json;

pub(super) fn id(value: &str) -> OpaqueId {
    OpaqueId::new(value).expect("fixture id is valid")
}

pub(super) fn project() -> ProjectId {
    id("project-1")
}

pub(super) fn other_project() -> ProjectId {
    id("project-2")
}

pub(super) fn human() -> PrincipalId {
    id("human-1")
}

pub(super) fn assistant() -> PrincipalId {
    id("agent-1")
}

pub(super) fn stranger() -> PrincipalId {
    id("stranger-1")
}

pub(super) fn value(label: &str, author: &PrincipalId, at: i64) -> NodeValue {
    NodeValue::new(
        label,
        json!({ "note": label }),
        Provenance::new(author.clone(), ProvenanceSource::HumanStatement, at),
    )
    .expect("fixture value is valid")
}

pub(super) fn node(
    node_id: &str,
    project_id: &ProjectId,
    kind: NodeKind,
    owner: &PrincipalId,
    at: i64,
) -> CurrentNode {
    CurrentNode {
        id: id(node_id),
        project_id: project_id.clone(),
        kind,
        value: value(node_id, owner, at),
        revision: Revision::ZERO,
        owner: owner.clone(),
        selected_alternative: None,
        alternatives: Vec::new(),
        removal: None,
    }
}

pub(super) fn edge(
    edge_id: &str,
    project_id: &ProjectId,
    kind: EdgeKind,
    relation: Relation,
    from: &str,
    to: &str,
    owner: &PrincipalId,
    holder: Option<&PrincipalId>,
) -> Edge {
    Edge {
        id: id(edge_id),
        project_id: project_id.clone(),
        kind,
        relation,
        from: id(from),
        to: id(to),
        revision: Revision::ZERO,
        owner: owner.clone(),
        holder: holder.cloned(),
        removal: None,
    }
}

pub(super) fn grants(entries: &[(&PrincipalId, Permission)]) -> GrantTable {
    let mut table = GrantTable::new();
    for (principal, permission) in entries {
        table.grant(ProjectGrant {
            project_id: project(),
            principal: (*principal).clone(),
            permission: *permission,
            granted_by: human(),
            granted_at_unix_ms: 1,
            revoked: false,
        });
    }
    table
}

pub(super) fn removal(at: i64) -> Removal {
    Removal {
        removed_by: human(),
        removed_at_unix_ms: at,
        reason: "fixture removal".into(),
    }
}

/// A project with a root, a task, a goal and an unrelated sibling subgraph.
pub(super) fn seeded_graph() -> (CollaborationGraph, GrantTable) {
    let mut graph = CollaborationGraph::new();
    let table = grants(&[
        (&human(), Permission::Admin),
        (&assistant(), Permission::Contribute),
    ]);
    for (node_id, kind) in [
        ("n-project", NodeKind::Project),
        ("n-goal", NodeKind::Goal),
        ("n-task", NodeKind::Task),
        ("n-dep", NodeKind::Artifact),
        ("n-transitive", NodeKind::Decision),
        ("n-other", NodeKind::Task),
        ("n-other-dep", NodeKind::Artifact),
    ] {
        graph
            .insert_node(node(node_id, &project(), kind, &human(), 10))
            .expect("fixture node inserts");
    }
    graph
        .insert_edge(edge(
            "e-project-goal",
            &project(),
            EdgeKind::Association,
            Relation::Refines,
            "n-project",
            "n-goal",
            &human(),
            None,
        ))
        .expect("fixture edge inserts");
    graph
        .insert_edge(edge(
            "e-goal-task",
            &project(),
            EdgeKind::Association,
            Relation::Refines,
            "n-goal",
            "n-task",
            &human(),
            None,
        ))
        .expect("fixture edge inserts");
    graph
        .insert_edge(edge(
            "e-dep-transitive",
            &project(),
            EdgeKind::Association,
            Relation::Refines,
            "n-dep",
            "n-transitive",
            &human(),
            None,
        ))
        .expect("fixture edge inserts");
    // The sibling subgraph is deliberately unreachable from the project root.
    graph
        .insert_edge(edge(
            "e-other-dep",
            &project(),
            EdgeKind::Association,
            Relation::DependsOn,
            "n-other",
            "n-other-dep",
            &human(),
            None,
        ))
        .expect("fixture edge inserts");
    (graph, table)
}
