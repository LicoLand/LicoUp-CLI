//! GRAPH-CONTEXT-PROFILE: snapshots, channels, context and corrections.

use super::fixtures::*;
use crate::authority::{
    AuthorityDenial, GrantTable, GraphSession, Permission, ProjectGrant, WriteOutcome, WriteRefusal,
};
use crate::context::{
    ContextRequest, GraphContextPort, GraphContextSelection, correct_preference,
    reclaim_debug_material,
};
use crate::graph::{
    Alternative, CollaborationGraph, EdgeKind, NodeKind, Provenance, ProvenanceSource, Relation,
    Revision,
};
use crate::subscription::{
    Channel, Epoch, GraphChange, GraphChannelLog, GraphCursor, Sequence,
    catch_up as graph_catch_up, snapshot as graph_snapshot,
};
use std::collections::BTreeSet;

#[test]
fn a_late_join_obtains_a_consistent_snapshot_and_catch_up() {
    let (graph, _table) = seeded_graph();
    let mut log = GraphChannelLog::new();
    let first = graph_snapshot(&graph, &project(), log.epoch(), log.sequence());
    assert_eq!(first.epoch, Epoch::INITIAL);
    assert_eq!(first.topology.nodes.len(), 7);
    assert_eq!(first.topology.edges.len(), 4);
    assert!(!first.topology.truncated);

    let delta = log.record(
        &project(),
        GraphChange::NodeValueChanged {
            node: id("n-task"),
            revision: Revision(1),
        },
    );
    assert_eq!(delta.channel(), Channel::NodeState);

    let replay = graph_catch_up(
        &graph,
        &log,
        &project(),
        GraphCursor {
            epoch: first.epoch,
            sequence: first.sequence,
        },
    );
    assert_eq!(replay.deltas().len(), 1);
    assert!(replay.snapshot().is_none());

    // A consumer from a previous epoch is resnapshotted, never replayed across
    // the boundary.
    log.rotate_epoch();
    let resnapshot = graph_catch_up(
        &graph,
        &log,
        &project(),
        GraphCursor {
            epoch: first.epoch,
            sequence: first.sequence,
        },
    );
    assert!(resnapshot.deltas().is_empty());
    assert_eq!(
        resnapshot.snapshot().map(|snapshot| snapshot.epoch),
        Some(Epoch(2))
    );
}

#[test]
fn state_only_changes_do_not_appear_on_the_topology_channel() {
    let mut log = GraphChannelLog::new();
    log.record(
        &project(),
        GraphChange::NodeUpserted {
            node: id("n-a"),
            kind: NodeKind::Task,
            revision: Revision::ZERO,
        },
    );
    log.record(
        &project(),
        GraphChange::NodeValueChanged {
            node: id("n-a"),
            revision: Revision(1),
        },
    );
    log.record(
        &project(),
        GraphChange::EdgeUpserted {
            edge: id("e-a"),
            kind: EdgeKind::Association,
            from: id("n-a"),
            to: id("n-a"),
            revision: Revision::ZERO,
        },
    );

    let topology = log.deltas_for_channel(&project(), Sequence(0), Channel::Topology);
    assert_eq!(topology.len(), 1);
    let states = log.deltas_for_channel(&project(), Sequence(0), Channel::NodeState);
    assert_eq!(states.len(), 1);
    let relations = log.deltas_for_channel(&project(), Sequence(0), Channel::Relation);
    assert_eq!(relations.len(), 1);
    // A per-channel cursor only advances that channel.
    let after = log.deltas_for_channel(&project(), Sequence(1), Channel::NodeState);
    assert_eq!(after.len(), 1);
    let after = log.deltas_for_channel(&project(), Sequence(2), Channel::Topology);
    assert!(after.is_empty());
}

#[test]
fn a_correction_supersedes_a_preference_without_promoting_it() {
    let (mut graph, table) = seeded_graph();
    graph
        .insert_node(node(
            "n-pref",
            &project(),
            NodeKind::GlobalPreference,
            &human(),
            1,
        ))
        .expect("preference inserts");
    let preference_revision_before = graph.node("n-pref").expect("preference exists").revision;
    let preference_provenance = graph
        .node("n-pref")
        .expect("preference exists")
        .value
        .provenance
        .clone();

    let mut session = GraphSession::new(&mut graph, &table);
    let outcome = correct_preference(
        &mut session,
        &project(),
        &human(),
        &id("n-pref"),
        id("n-correction"),
        value("project correction", &human(), 70),
        70,
    );
    assert!(outcome.applied, "{:?}", outcome.reason);
    assert_eq!(outcome.preference, id("n-pref"));

    let correction = session
        .graph()
        .node("n-correction")
        .expect("correction exists");
    assert_eq!(correction.kind, NodeKind::ProjectConstraint);
    assert_eq!(correction.project_id, project());
    assert_eq!(
        correction.value.provenance,
        Provenance::new(human(), ProvenanceSource::HumanStatement, 70)
    );

    // The global preference keeps its value, revision and original provenance.
    let preference = session.graph().node("n-pref").expect("preference exists");
    assert_eq!(preference.kind, NodeKind::GlobalPreference);
    assert_eq!(preference.revision, preference_revision_before);
    assert_eq!(preference.value.provenance, preference_provenance);
    assert_eq!(
        preference.value.label, "n-pref",
        "the preference body is not rewritten by the correction"
    );

    // The correction supersedes the preference through a real association.
    let supersedes = session
        .graph()
        .outgoing_edges(&id("n-correction"))
        .into_iter()
        .find(|edge| edge.relation == Relation::Supersedes)
        .cloned()
        .expect("superseding edge exists");
    assert_eq!(supersedes.to, id("n-pref"));
}

#[test]
fn a_correction_against_a_project_fact_is_refused() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let outcome = correct_preference(
        &mut session,
        &project(),
        &human(),
        &id("n-task"),
        id("n-correction"),
        value("not a correction", &human(), 5),
        5,
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.reason.as_deref(),
        Some("collaboration_correction_target_not_preference")
    );
}

#[test]
fn preferences_and_project_constraints_stay_in_separate_groups() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    assert!(
        session
            .insert_node(
                &project(),
                &human(),
                node(
                    "n-pref",
                    &project(),
                    NodeKind::GlobalPreference,
                    &human(),
                    1
                ),
            )
            .applied
    );
    assert!(
        session
            .insert_edge(
                &project(),
                &human(),
                edge(
                    "e-task-pref",
                    &project(),
                    EdgeKind::Association,
                    Relation::Refines,
                    "n-task",
                    "n-pref",
                    &human(),
                    None,
                ),
            )
            .applied
    );
    let graph = session.graph();

    let selection = GraphContextSelection::new(graph, &table);
    let mut request = ContextRequest::new(project(), human(), id("n-task"));
    request.include_kinds = BTreeSet::from([
        NodeKind::Task,
        NodeKind::GlobalPreference,
        NodeKind::ProjectConstraint,
    ]);
    let slice = selection.task_context(&request);
    assert!(slice.is_authorized());
    assert!(
        slice
            .global_preferences
            .iter()
            .any(|entry| entry.node == id("n-pref"))
    );
    assert!(
        !slice
            .project_values
            .iter()
            .any(|entry| entry.node == id("n-pref")),
        "a global preference is never returned as a project value"
    );
    assert!(
        !slice
            .project_constraints
            .iter()
            .any(|entry| entry.node == id("n-pref"))
    );
}

#[test]
fn context_entries_retain_original_provenance() {
    let (graph, table) = seeded_graph();
    let selection = GraphContextSelection::new(&graph, &table);
    let mut request = ContextRequest::new(project(), human(), id("n-task"));
    request.include_kinds = BTreeSet::from([NodeKind::Task]);
    let slice = selection.task_context(&request);
    let entry = slice
        .project_values
        .iter()
        .find(|entry| entry.node == id("n-task"))
        .expect("the task is in its own context");
    assert_eq!(entry.provenance.author, human());
    assert_eq!(entry.provenance.source, ProvenanceSource::HumanStatement);
    assert_eq!(entry.provenance.recorded_at_unix_ms, 10);
}

#[test]
fn context_selection_is_bounded_and_reports_truncation() {
    let (graph, table) = seeded_graph();
    let selection = GraphContextSelection::new(&graph, &table);
    let mut request = ContextRequest::new(project(), human(), id("n-project"));
    request.max_entries = 1;
    let slice = selection.task_context(&request);
    assert_eq!(slice.entry_count(), 1);
    assert!(slice.truncated);
}

#[test]
fn temporary_debug_material_is_reclaimable() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    assert!(
        session
            .insert_node(
                &project(),
                &human(),
                node("n-debug", &project(), NodeKind::DebugNote, &human(), 1),
            )
            .applied
    );
    assert!(
        session
            .insert_edge(
                &project(),
                &human(),
                edge(
                    "e-debug",
                    &project(),
                    EdgeKind::Association,
                    Relation::DebugOf,
                    "n-debug",
                    "n-task",
                    &human(),
                    None,
                ),
            )
            .applied
    );

    let removed = reclaim_debug_material(&mut session, &project(), &human(), &id("n-task"), 90);
    assert_eq!(removed, vec![id("n-debug")]);
    assert!(
        session
            .graph()
            .node("n-debug")
            .is_some_and(|node| !node.is_current())
    );
    assert!(
        session
            .graph()
            .node("n-task")
            .is_some_and(|node| node.is_current()),
        "reclaiming debug material does not remove its subject"
    );
}

#[test]
fn alternatives_coexist_and_selection_is_fenced() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let first = session.select_alternative(
        &project(),
        &human(),
        &id("n-task"),
        Revision::ZERO,
        Alternative {
            id: id("alt-a"),
            value: value("reading a", &human(), 1),
        },
    );
    assert!(first.applied);

    let stale = session.select_alternative(
        &project(),
        &human(),
        &id("n-task"),
        Revision::ZERO,
        Alternative {
            id: id("alt-b"),
            value: value("reading b", &human(), 2),
        },
    );
    assert!(!stale.applied);

    let second = session.select_alternative(
        &project(),
        &human(),
        &id("n-task"),
        Revision(1),
        Alternative {
            id: id("alt-b"),
            value: value("reading b", &human(), 2),
        },
    );
    assert!(second.applied);

    let node = session.graph().node("n-task").expect("task exists");
    assert_eq!(node.alternatives.len(), 2);
    assert_eq!(node.effective_value().label, "reading b");
    assert_eq!(
        node.alternatives
            .iter()
            .find(|alternative| alternative.id == id("alt-a"))
            .expect("the earlier reading is still readable")
            .value
            .label,
        "reading a"
    );
}

#[test]
fn an_edge_across_projects_is_refused() {
    let (mut graph, table) = seeded_graph();
    let mut session = GraphSession::new(&mut graph, &table);
    let foreign = session.insert_node(
        &project(),
        &human(),
        node("n-foreign", &other_project(), NodeKind::Task, &human(), 1),
    );
    assert!(
        !foreign.applied,
        "a project session never creates a node in another project"
    );
    let outcome = session.insert_edge(
        &project(),
        &human(),
        edge(
            "e-cross",
            &project(),
            EdgeKind::Association,
            Relation::DependsOn,
            "n-task",
            "n-foreign",
            &human(),
            None,
        ),
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.refusal,
        Some(WriteRefusal::Rejected {
            reason: "collaboration_edge_endpoint_unknown".into(),
        })
    );
}

#[test]
fn a_foreign_outcome_never_reports_success() {
    let outcome = WriteOutcome::refused(WriteRefusal::UnknownTarget);
    assert!(!outcome.applied);
    assert_eq!(outcome.revision, Revision::ZERO);
    assert!(outcome.invalidated.is_empty());
}

#[test]
fn an_unauthorized_principal_receives_no_context() {
    let (graph, _table) = seeded_graph();
    let selection = GraphContextSelection::new(&graph, &_table);
    let mut request = ContextRequest::new(project(), stranger(), id("n-task"));
    request.include_kinds = BTreeSet::from([NodeKind::Task]);

    let slice = selection.task_context(&request);
    assert!(!slice.is_authorized());
    assert_eq!(slice.denial, Some(AuthorityDenial::Unauthorized));
    assert!(slice.entries().is_empty());

    // A read-only grant is enough for context.
    let reader = id("reader-1");
    let mut read_only = GrantTable::new();
    read_only.grant(ProjectGrant {
        project_id: project(),
        principal: reader.clone(),
        permission: Permission::Read,
        granted_by: human(),
        granted_at_unix_ms: 1,
        revoked: false,
    });
    let selection = GraphContextSelection::new(&graph, &read_only);
    let readable = selection.task_context(&ContextRequest::new(
        project(),
        reader.clone(),
        id("n-task"),
    ));
    assert!(readable.is_authorized());
    assert!(!readable.entries().is_empty());
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
