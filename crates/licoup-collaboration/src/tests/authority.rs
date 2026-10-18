//! GRAPH-CURRENT-AUTHORITY: fenced writes, invalidation and authorization.

use super::fixtures::*;
use crate::authority::{AuthorityDenial, GraphSession, Permission, WriteRefusal};
use crate::context::{ContextRequest, GraphContextPort, GraphContextSelection};
use crate::graph::{CollaborationGraph, EdgeKind, NodeId, NodeKind, Relation, Revision};
use std::collections::BTreeSet;

#[test]
fn a_concurrent_writer_cannot_overwrite_current_state() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);

    // The first writer moves the task to revision 1.
    let first = session.replace_value(
        &project(),
        &human(),
        &id("n-task"),
        Revision::ZERO,
        value("task v1", &human(), 11),
    );
    assert!(first.applied);
    assert_eq!(first.revision, Revision(1));

    // A second writer that observed revision 0 loses the fence and learns the
    // current owner and revision instead of overwriting.
    let second = session.replace_value(
        &project(),
        &assistant(),
        &id("n-task"),
        Revision::ZERO,
        value("stale task", &assistant(), 12),
    );
    assert!(!second.applied);
    assert_eq!(
        second.refusal,
        Some(WriteRefusal::FenceLost {
            current_owner: human(),
            current_revision: Revision(1),
        })
    );
    assert_eq!(
        session
            .graph()
            .node("n-task")
            .expect("task exists")
            .effective_value()
            .label,
        "task v1"
    );
}

#[test]
fn a_read_only_grant_cannot_write() {
    let mut empty = CollaborationGraph::new();
    let read_only = grants(&[(&human(), Permission::Read)]);
    let mut writes = GraphSession::new(&mut empty, &read_only);
    let denied = writes.insert_node(
        &project(),
        &human(),
        node("n-new", &project(), NodeKind::Task, &human(), 1),
    );
    assert!(!denied.applied);
    assert_eq!(
        denied.refusal,
        Some(WriteRefusal::Denied {
            reason: AuthorityDenial::Insufficient,
        })
    );
}

#[test]
fn a_stale_handoff_result_from_a_previous_owner_is_refused() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    assert!(
        session
            .replace_value(
                &project(),
                &human(),
                &id("n-task"),
                Revision::ZERO,
                value("task v1", &human(), 11),
            )
            .applied
    );

    // The Assistant hands back a result computed against the old revision while
    // owning the node; the fence rejects it rather than corrupting current
    // state.
    let handoff = session.replace_value(
        &project(),
        &assistant(),
        &id("n-task"),
        Revision::ZERO,
        value("handoff result", &assistant(), 12),
    );
    assert!(!handoff.applied);
    assert!(matches!(
        handoff.refusal,
        Some(WriteRefusal::FenceLost { .. })
    ));
}

#[test]
fn a_removed_node_refuses_further_writes() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let removed = session.remove_node(
        &project(),
        &human(),
        &id("n-transitive"),
        Revision::ZERO,
        "superseded",
        20,
    );
    assert!(removed.applied);

    let after = session.replace_value(
        &project(),
        &human(),
        &id("n-transitive"),
        removed.revision,
        value("resurrected", &human(), 21),
    );
    assert!(!after.applied);
    assert_eq!(after.refusal, Some(WriteRefusal::Removed));
    assert!(
        session
            .graph()
            .node("n-transitive")
            .is_some_and(|node| !node.is_current())
    );
}

#[test]
fn replacing_a_dependency_invalidates_only_the_affected_reachable_nodes() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    // n-task gains a dependency on n-dep, which is refined by n-transitive.
    assert!(
        session
            .insert_edge(
                &project(),
                &human(),
                edge(
                    "e-task-dep",
                    &project(),
                    EdgeKind::Association,
                    Relation::DependsOn,
                    "n-task",
                    "n-dep",
                    &human(),
                    None,
                ),
            )
            .applied
    );

    // Swap the dependency target: n-task now depends on n-other-dep.
    let swapped = session.replace_edge_target(
        &project(),
        &human(),
        &id("e-task-dep"),
        Revision::ZERO,
        id("n-other-dep"),
    );
    assert!(swapped.applied);
    let invalidated: BTreeSet<NodeId> = swapped.invalidated.iter().cloned().collect();
    // Both the old reachable set and the new one are invalidated.
    assert!(invalidated.contains(&id("n-dep")));
    assert!(invalidated.contains(&id("n-transitive")));
    assert!(invalidated.contains(&id("n-other-dep")));
    // The unrelated part of the graph is not invalidated wholesale.
    assert!(!invalidated.contains(&id("n-project")));
    assert!(!invalidated.contains(&id("n-goal")));
    assert_eq!(
        session.graph().edge("e-task-dep").expect("edge exists").to,
        id("n-other-dep")
    );
}

#[test]
fn revoking_a_grant_denies_immediately() {
    let mut table = grants(&[(&human(), Permission::Admin)]);
    assert_eq!(
        table.effective_permission(&project(), &human()),
        Ok(Permission::Admin)
    );
    table.revoke(&project(), &human());
    assert_eq!(
        table.effective_permission(&project(), &human()),
        Err(AuthorityDenial::Revoked)
    );
    assert!(table.readable_projects(&human()).is_empty());
}

#[test]
fn a_task_in_another_project_does_not_widen_the_context() {
    let mut graph = CollaborationGraph::new();
    graph
        .insert_node(node(
            "n-task",
            &other_project(),
            NodeKind::Task,
            &human(),
            1,
        ))
        .expect("foreign task inserts");
    let table = grants(&[(&human(), Permission::Admin)]);
    let selection = GraphContextSelection::new(&graph, &table);
    let slice = selection.task_context(&ContextRequest::new(project(), human(), id("n-task")));
    assert!(!slice.is_authorized());
    assert!(slice.entries().is_empty());
}

#[test]
fn inserting_a_node_with_a_foreign_project_is_refused() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let outcome = session.insert_node(
        &project(),
        &human(),
        node("n-foreign", &other_project(), NodeKind::Task, &human(), 1),
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.refusal,
        Some(WriteRefusal::Rejected {
            reason: "collaboration_node_project_mismatch".into(),
        })
    );
}

#[test]
fn a_second_insert_of_the_same_identity_is_a_conflict() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let outcome = session.insert_node(
        &project(),
        &human(),
        node("n-task", &project(), NodeKind::Task, &human(), 30),
    );
    assert!(!outcome.applied);
    assert!(matches!(
        outcome.refusal,
        Some(WriteRefusal::Rejected { .. })
    ));
}
