//! GRAPH-BOUNDED-GC: roots, reachability, watermarks and holdings.

use super::fixtures::*;
use crate::authority::{GraphSession, Permission, WriteRefusal};
use crate::graph::{
    CollaborationGraph, EdgeKind, NodeId, NodeKind, PrincipalId, Relation, RetentionReason, Root,
};
use crate::retention::{
    MAX_COLLECTION_BATCH, Watermarks, collect, plan_collection, released_holdings,
    rootless_cycle_nodes,
};
use std::collections::BTreeSet;

#[test]
fn a_rootless_association_cycle_is_collectable() {
    let mut graph = CollaborationGraph::new();
    let _table = grants(&[(&human(), Permission::Admin)]);
    for node_id in ["n-project", "n-cycle-a", "n-cycle-b", "n-cycle-c"] {
        graph
            .insert_node(node(
                node_id,
                &project(),
                if node_id == "n-project" {
                    NodeKind::Project
                } else {
                    NodeKind::Task
                },
                &human(),
                1,
            ))
            .expect("fixture node inserts");
    }
    for (edge_id, from, to) in [
        ("e-cycle-1", "n-cycle-a", "n-cycle-b"),
        ("e-cycle-2", "n-cycle-b", "n-cycle-c"),
        ("e-cycle-3", "n-cycle-c", "n-cycle-a"),
    ] {
        graph
            .insert_edge(edge(
                edge_id,
                &project(),
                EdgeKind::Association,
                Relation::DependsOn,
                from,
                to,
                &human(),
                None,
            ))
            .expect("fixture edge inserts");
    }

    assert_eq!(
        rootless_cycle_nodes(&graph, &project(), &[]),
        vec![id("n-cycle-a"), id("n-cycle-b"), id("n-cycle-c")]
    );

    let watermarks = Watermarks { high: 3, low: 3 };
    let plan = plan_collection(&graph, &project(), &[], watermarks);
    assert_eq!(plan.collectable.len(), 3);
    assert_eq!(plan.retained.len(), 1);
    assert_eq!(plan.retained[0].node, id("n-project"));
    assert_eq!(plan.retained[0].reason, RetentionReason::ProjectRoot);
}

#[test]
fn an_owning_holding_does_not_keep_a_node_alive() {
    let mut graph = CollaborationGraph::new();
    let _table = grants(&[(&human(), Permission::Admin)]);
    for node_id in ["n-project", "n-held"] {
        graph
            .insert_node(node(
                node_id,
                &project(),
                if node_id == "n-project" {
                    NodeKind::Project
                } else {
                    NodeKind::Task
                },
                &human(),
                1,
            ))
            .expect("fixture node inserts");
    }
    graph
        .insert_edge(edge(
            "e-holding",
            &project(),
            EdgeKind::Owning,
            Relation::HeldBy,
            "n-project",
            "n-held",
            &human(),
            Some(&assistant()),
        ))
        .expect("holding edge inserts");

    let watermarks = Watermarks { high: 1, low: 1 };
    let plan = plan_collection(&graph, &project(), &[], watermarks);
    assert_eq!(plan.collectable, vec![id("n-held")]);
}

#[test]
fn active_inputs_and_explicitly_retained_content_remain() {
    let (graph, _table) = seeded_graph();
    let roots = vec![
        Root {
            node: id("n-task"),
            reason: RetentionReason::ActiveInput,
        },
        Root {
            node: id("n-dep"),
            reason: RetentionReason::ExplicitlyRetained,
        },
    ];
    let watermarks = Watermarks { high: 1, low: 1 };
    let plan = plan_collection(&graph, &project(), &roots, watermarks);
    assert!(!plan.collectable.contains(&id("n-task")));
    assert!(!plan.collectable.contains(&id("n-dep")));
    assert!(!plan.collectable.contains(&id("n-transitive")));

    let retained: BTreeSet<NodeId> = plan.retained.iter().map(|note| note.node.clone()).collect();
    assert!(retained.contains(&id("n-task")));
    assert!(retained.contains(&id("n-dep")));
    // n-transitive is reachable from n-dep by an association, so it survives too.
    assert!(retained.contains(&id("n-transitive")));
    // The sibling subgraph is still collectable; explicit retention of one
    // subgraph does not retain another.
    assert!(plan.collectable.contains(&id("n-other")));
    assert!(plan.collectable.contains(&id("n-other-dep")));
}

#[test]
fn unfinished_acceptance_and_undelivered_roots_all_retain() {
    let (graph, _table) = seeded_graph();
    let watermarks = Watermarks { high: 1, low: 1 };
    // n-other carries unfinished work, so it and its dependent survive; each
    // durable reason is honoured.
    for reason in [
        RetentionReason::UnfinishedWork,
        RetentionReason::AcceptanceRecord,
        RetentionReason::Undelivered,
    ] {
        let roots = vec![Root {
            node: id("n-other"),
            reason,
        }];
        let plan = plan_collection(&graph, &project(), &roots, watermarks);
        assert!(!plan.collectable.contains(&id("n-other")), "{reason:?}");
        assert!(!plan.collectable.contains(&id("n-other-dep")), "{reason:?}");
        assert_eq!(
            plan.retained
                .iter()
                .find(|note| note.node == id("n-other"))
                .map(|note| note.reason),
            Some(reason)
        );
    }
}

#[test]
fn collection_is_bounded_and_respects_the_watermarks() {
    let mut graph = CollaborationGraph::new();
    let _table = grants(&[(&human(), Permission::Admin)]);
    graph
        .insert_node(node("n-root", &project(), NodeKind::Project, &human(), 1))
        .expect("root inserts");
    let total = 40usize;
    for index in 0..total {
        graph
            .insert_node(node(
                &format!("n-loose-{index:03}"),
                &project(),
                NodeKind::Task,
                &human(),
                1,
            ))
            .expect("loose node inserts");
    }

    let below = Watermarks { high: 100, low: 50 };
    let (record, removals) = collect(&graph, &project(), &[], below, 5, "unreachable");
    assert!(!record.ran);
    assert!(removals.is_empty());

    let above = Watermarks { high: 10, low: 5 };
    let (record, removals) = collect(&graph, &project(), &[], above, 5, "unreachable");
    assert!(record.ran);
    // 41 held, low watermark 5, so 36 are collectable and the batch bound is
    // not the limit here.
    assert_eq!(removals.len(), 36);
    assert_eq!(record.remaining, 5);
    assert!(record.collected_count() <= MAX_COLLECTION_BATCH);
    assert!(!record.collected.contains(&id("n-root")));

    let unusable = Watermarks { high: 5, low: 5 };
    let (record, removals) = collect(&graph, &project(), &[], unusable, 5, "unreachable");
    assert!(!record.ran);
    assert!(removals.is_empty());
}

#[test]
fn collection_records_non_sensitive_reasons_and_counts() {
    let (graph, _table) = seeded_graph();
    // Seven nodes are held: three reachable and four collectable.
    let watermarks = Watermarks { high: 6, low: 5 };
    let (record, removals) = collect(&graph, &project(), &[], watermarks, 42, "unreachable");
    assert!(record.ran);
    assert_eq!(record.collected.len(), 2);
    assert_eq!(removals.len(), 2);
    for (_, removal) in &removals {
        assert_eq!(removal.reason, "unreachable");
        assert_eq!(removal.removed_at_unix_ms, 42);
    }
    assert!(
        record
            .retained
            .iter()
            .any(|note| note.reason == RetentionReason::DurableCommitment)
    );
}

#[test]
fn holdings_are_released_only_for_settled_holders() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    assert!(
        session
            .insert_edge(
                &project(),
                &human(),
                edge(
                    "e-hold-active",
                    &project(),
                    EdgeKind::Owning,
                    Relation::HeldBy,
                    "n-task",
                    "n-dep",
                    &human(),
                    Some(&assistant()),
                ),
            )
            .applied
    );
    let other_holder = id("agent-2");
    assert!(
        session
            .insert_edge(
                &project(),
                &human(),
                edge(
                    "e-hold-unresolved",
                    &project(),
                    EdgeKind::Owning,
                    Relation::HeldBy,
                    "n-task",
                    "n-transitive",
                    &human(),
                    Some(&other_holder),
                ),
            )
            .applied
    );

    let settled: BTreeSet<PrincipalId> = BTreeSet::from([assistant()]);
    assert_eq!(
        released_holdings(session.graph(), &project(), &settled),
        vec![id("e-hold-active")]
    );

    // An unrelated holder cannot release another execution's holding.
    let refused = session.release_owning_edge(
        &project(),
        &other_holder,
        &id("e-hold-active"),
        "executor exit",
        60,
    );
    assert!(!refused.applied);
    let released = session.release_owning_edge(
        &project(),
        &assistant(),
        &id("e-hold-active"),
        "execution settled",
        60,
    );
    assert!(released.applied);
}

#[test]
fn an_owning_edge_without_a_holder_is_refused() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let outcome = session.insert_edge(
        &project(),
        &human(),
        edge(
            "e-hold-anonymous",
            &project(),
            EdgeKind::Owning,
            Relation::HeldBy,
            "n-task",
            "n-dep",
            &human(),
            None,
        ),
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.refusal,
        Some(WriteRefusal::Rejected {
            reason: "collaboration_owning_edge_needs_holder".into(),
        })
    );
}

#[test]
fn clearing_reconstructable_state_and_restarting_preserves_durable_roots() {
    // A restart is modelled by rebuilding the graph from the same durable
    // values while every reconstructable cache is dropped. Durable commitments
    // still hold their subgraph.
    let (graph, _table) = seeded_graph();
    let watermarks = Watermarks { high: 2, low: 2 };
    let roots = vec![
        Root {
            node: id("n-other"),
            reason: RetentionReason::UnfinishedWork,
        },
        Root {
            node: id("n-dep"),
            reason: RetentionReason::AcceptanceRecord,
        },
        Root {
            node: id("n-task"),
            reason: RetentionReason::Undelivered,
        },
    ];
    let plan = plan_collection(&graph, &project(), &roots, watermarks);
    assert!(plan.collectable.is_empty());
    assert!(plan.retained.len() >= 5);
}
