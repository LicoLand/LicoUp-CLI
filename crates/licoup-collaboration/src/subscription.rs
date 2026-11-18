//! Snapshot, catch-up and high-frequency channels for graph consumers.
//!
//! A consumer that joins late needs one consistent reading of the current
//! graph; a consumer that stays connected needs only what changed. The two are
//! separated by channel and by epoch:
//!
//! * [`Channel::Topology`] carries node and edge membership changes, which is
//!   what a layout pass reacts to.
//! * [`Channel::NodeState`] carries value and revision changes that leave the
//!   shape alone, so a state-only change never triggers a full layout.
//! * [`Channel::Relation`] carries association and holding changes without
//!   node membership changes.
//!
//! A reconnecting consumer either resumes its cursor inside the same epoch, or
//! receives a fresh snapshot when the epoch advanced or the retained log no
//! longer reaches its cursor. A catch-up therefore never mixes epochs, and
//! because a delta is only ever recorded from an applied change, it never
//! invents completion that the store did not commit.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

use crate::graph::{
    CollaborationGraph, Edge, EdgeId, EdgeKind, NodeId, NodeKind, ProjectId, Revision,
};

/// Upper bound on retained deltas per epoch.
pub const MAX_RETAINED_DELTAS: usize = 4_096;
/// Upper bound on topology entries one snapshot carries.
pub const MAX_SNAPSHOT_NODES: usize = 4_096;

/// One consistent reading of the graph.
///
/// The epoch changes whenever the retained log is discarded, so a delta from a
/// different epoch can never be applied to this snapshot.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Epoch(pub u64);

impl Epoch {
    pub const INITIAL: Self = Self(1);

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// One monotonic position inside an epoch.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Sequence(pub u64);

/// Which stream one change belongs to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    /// Node or edge membership changed; a layout pass may react.
    Topology,
    /// A node's value or revision changed; the shape is unchanged.
    NodeState,
    /// An association or holding changed; node membership is unchanged.
    Relation,
}

/// One node in a snapshot, without its body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub revision: Revision,
}

/// One edge in a snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEdge {
    pub id: EdgeId,
    pub kind: EdgeKind,
    pub from: NodeId,
    pub to: NodeId,
    pub revision: Revision,
}

/// The topology of one project at one sequence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologySnapshot {
    pub nodes: Vec<SnapshotNode>,
    pub edges: Vec<SnapshotEdge>,
    /// True when the project holds more nodes than one snapshot carries.
    pub truncated: bool,
}

/// A consistent reading of one project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    pub project_id: ProjectId,
    pub epoch: Epoch,
    pub sequence: Sequence,
    pub topology: TopologySnapshot,
    /// Current revisions per node, so a consumer can detect a value change it
    /// missed without re-reading the bodies.
    pub node_revisions: BTreeMap<NodeId, Revision>,
}

/// One recorded change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "camelCase")]
pub enum GraphChange {
    NodeUpserted {
        node: NodeId,
        kind: NodeKind,
        revision: Revision,
    },
    NodeValueChanged {
        node: NodeId,
        revision: Revision,
    },
    NodeRemoved {
        node: NodeId,
        revision: Revision,
    },
    EdgeUpserted {
        edge: EdgeId,
        kind: EdgeKind,
        from: NodeId,
        to: NodeId,
        revision: Revision,
    },
    EdgeRetargeted {
        edge: EdgeId,
        from: NodeId,
        to: NodeId,
        revision: Revision,
    },
    EdgeReleased {
        edge: EdgeId,
        revision: Revision,
    },
}

impl GraphChange {
    /// The channel this change belongs to. A value-only change is never
    /// reported as a topology change, so layout work is not triggered by it.
    pub const fn channel(&self) -> Channel {
        match self {
            Self::NodeUpserted { .. } | Self::NodeRemoved { .. } => Channel::Topology,
            Self::NodeValueChanged { .. } => Channel::NodeState,
            Self::EdgeUpserted { .. } | Self::EdgeRetargeted { .. } | Self::EdgeReleased { .. } => {
                Channel::Relation
            }
        }
    }
}

/// One delivered change with its position.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphDelta {
    pub project_id: ProjectId,
    pub epoch: Epoch,
    pub sequence: Sequence,
    pub change: GraphChange,
}

impl GraphDelta {
    /// The channel this delta belongs to.
    pub const fn channel(&self) -> Channel {
        self.change.channel()
    }
}

/// What a consumer holds between deliveries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCursor {
    pub epoch: Epoch,
    pub sequence: Sequence,
}

/// The answer to one catch-up request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "catchUp", rename_all = "camelCase")]
pub enum CatchUp {
    /// The cursor is still reachable inside the same epoch.
    Replay { deltas: Vec<GraphDelta> },
    /// The epoch advanced or the cursor fell out of the retained log, so the
    /// consumer gets a fresh snapshot and starts a new cursor. It never
    /// receives deltas from two epochs in one answer.
    Resnapshot { snapshot: GraphSnapshot },
}

impl CatchUp {
    pub fn deltas(&self) -> &[GraphDelta] {
        match self {
            Self::Replay { deltas } => deltas,
            Self::Resnapshot { .. } => &[],
        }
    }

    pub fn snapshot(&self) -> Option<&GraphSnapshot> {
        match self {
            Self::Replay { .. } => None,
            Self::Resnapshot { snapshot } => Some(snapshot),
        }
    }
}

/// A bounded, epoch-scoped log of applied graph changes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphChannelLog {
    epoch: Epoch,
    sequence: Sequence,
    deltas: VecDeque<GraphDelta>,
}

impl Default for GraphChannelLog {
    fn default() -> Self {
        Self {
            epoch: Epoch::INITIAL,
            sequence: Sequence(0),
            deltas: VecDeque::new(),
        }
    }
}

impl GraphChannelLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn sequence(&self) -> Sequence {
        self.sequence
    }

    pub fn retained(&self) -> usize {
        self.deltas.len()
    }

    /// Record one applied change and return its position.
    ///
    /// Only an applied change may be recorded, so a consumer never receives a
    /// completion the store did not commit.
    pub fn record(&mut self, project_id: &ProjectId, change: GraphChange) -> GraphDelta {
        self.sequence = Sequence(self.sequence.0.saturating_add(1));
        let delta = GraphDelta {
            project_id: project_id.clone(),
            epoch: self.epoch,
            sequence: self.sequence,
            change,
        };
        self.deltas.push_back(delta.clone());
        while self.deltas.len() > MAX_RETAINED_DELTAS {
            self.deltas.pop_front();
        }
        delta
    }

    /// Discard the retained log and start a new epoch. Consumers holding an
    /// older cursor are resnapshotted rather than replayed across the boundary.
    pub fn rotate_epoch(&mut self) {
        self.epoch = self.epoch.next();
        self.sequence = Sequence(0);
        self.deltas.clear();
    }

    /// Deltas for one channel after `cursor`, inside the current epoch.
    pub fn deltas_for_channel(
        &self,
        project: &ProjectId,
        cursor: Sequence,
        channel: Channel,
    ) -> Vec<GraphDelta> {
        self.deltas
            .iter()
            .filter(|delta| {
                &delta.project_id == project
                    && delta.sequence > cursor
                    && delta.change.channel() == channel
            })
            .cloned()
            .collect()
    }

    /// Answer one catch-up request.
    pub fn catch_up(&self, project: &ProjectId, cursor: GraphCursor) -> Option<CatchUp> {
        if cursor.epoch != self.epoch {
            return None;
        }
        let oldest = self
            .deltas
            .front()
            .map(|delta| delta.sequence.0)
            .unwrap_or(0);
        if cursor.sequence.0.saturating_add(1) < oldest {
            // The consumer's cursor fell out of the retained log; only a fresh
            // snapshot can be consistent.
            return None;
        }
        Some(CatchUp::Replay {
            deltas: self
                .deltas
                .iter()
                .filter(|delta| &delta.project_id == project && delta.sequence > cursor.sequence)
                .cloned()
                .collect(),
        })
    }
}

/// Read one consistent snapshot of a project.
pub fn snapshot(
    graph: &CollaborationGraph,
    project: &ProjectId,
    epoch: Epoch,
    sequence: Sequence,
) -> GraphSnapshot {
    let mut nodes: Vec<SnapshotNode> = graph
        .nodes_of_project(project)
        .filter(|node| node.is_current())
        .map(|node| SnapshotNode {
            id: node.id.clone(),
            kind: node.kind,
            revision: node.revision,
        })
        .collect();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let truncated = nodes.len() > MAX_SNAPSHOT_NODES;
    nodes.truncate(MAX_SNAPSHOT_NODES);

    let mut edges: Vec<SnapshotEdge> = graph
        .edges()
        .filter(|edge| &edge.project_id == project && edge.is_current())
        .map(|edge: &Edge| SnapshotEdge {
            id: edge.id.clone(),
            kind: edge.kind,
            from: edge.from.clone(),
            to: edge.to.clone(),
            revision: edge.revision,
        })
        .collect();
    edges.sort_by(|left, right| left.id.cmp(&right.id));

    let node_revisions = nodes
        .iter()
        .map(|node| (node.id.clone(), node.revision))
        .collect();

    GraphSnapshot {
        project_id: project.clone(),
        epoch,
        sequence,
        topology: TopologySnapshot {
            nodes,
            edges,
            truncated,
        },
        node_revisions,
    }
}

/// The full catch-up answer for one request: either a replay or a resnapshot.
pub fn catch_up(
    graph: &CollaborationGraph,
    log: &GraphChannelLog,
    project: &ProjectId,
    cursor: GraphCursor,
) -> CatchUp {
    match log.catch_up(project, cursor) {
        Some(answer) => answer,
        None => CatchUp::Resnapshot {
            snapshot: snapshot(graph, project, log.epoch(), log.sequence()),
        },
    }
}
