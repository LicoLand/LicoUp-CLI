//! Mutable current project state for local collaboration.
//!
//! This crate is the functional owner of the **current** collaboration graph:
//! one value per node, the explicit alternatives that coexist with it, the
//! indexed relations between nodes, the temporary holdings an execution takes,
//! and the retention decisions that bound what is kept.
//!
//! It owns no conversation history, no run, no raw evidence, no project
//! document and no long-term memory. Those keep their own owners; this graph
//! keeps values, relations and handles, and answers with provenance intact.
//!
//! ```
//! use licoup_collaboration::{
//!     authority::{GrantTable, Permission, ProjectGrant},
//!     graph::{OpaqueId, ProjectId, PrincipalId},
//! };
//!
//! let project = ProjectId::new("project-1").unwrap();
//! let human = PrincipalId::new("human-1").unwrap();
//! let mut grants = GrantTable::new();
//! grants.grant(ProjectGrant {
//!     project_id: project.clone(),
//!     principal: human.clone(),
//!     permission: Permission::Admin,
//!     granted_by: human.clone(),
//!     granted_at_unix_ms: 1,
//!     revoked: false,
//! });
//! assert_eq!(
//!     grants.effective_permission(&project, &human),
//!     Ok(Permission::Admin)
//! );
//! assert!(grants.readable_projects(&human).contains(&project));
//! let _ = OpaqueId::new("node-1").unwrap();
//! ```
//!
//! The pure core performs no I/O. A caller persists it through its own durable
//! store and reads it through the ports this crate defines.

pub mod authority;
pub mod context;
pub mod graph;
pub mod retention;
pub mod subscription;

pub use authority::{
    AuthorityDenial, GrantTable, GraphSession, GraphTarget, Permission, ProjectGrant, WriteOutcome,
    WriteRefusal,
};
pub use context::{
    ContextEntry, ContextRequest, ContextSlice, CorrectionOutcome, GraphContextPort,
    GraphContextSelection, MAX_CONTEXT_ENTRIES, correct_preference, reclaim_debug_material,
};
pub use graph::{
    Alternative, CollaborationGraph, CurrentNode, Edge, EdgeId, EdgeKind, MAX_BODY_BYTES,
    MAX_GRAPH_EDGES, MAX_GRAPH_NODES, MAX_NODE_ALTERNATIVES, NodeId, NodeKind, NodeValue, OpaqueId,
    PrincipalId, ProjectId, Provenance, ProvenanceSource, Relation, Removal, RetentionReason,
    Revision, Root,
};
pub use retention::{
    CollectionPlan, CollectionRecord, MAX_COLLECTION_BATCH, RetentionNote, Watermarks, collect,
    collectable_nodes, is_collectable_kind, plan_collection, released_holdings, retention_roots,
    rootless_cycle_nodes,
};
pub use subscription::{
    CatchUp, Channel, Epoch, GraphChange, GraphChannelLog, GraphCursor, GraphDelta, GraphSnapshot,
    MAX_RETAINED_DELTAS, MAX_SNAPSHOT_NODES, Sequence, SnapshotEdge, SnapshotNode,
    TopologySnapshot, catch_up, snapshot,
};

/// The persisted-format name of the current collaboration graph.
pub const COLLABORATION_GRAPH_SCHEMA_VERSION: &str = "licoup.collaboration-graph.v1";

#[cfg(test)]
mod tests;
