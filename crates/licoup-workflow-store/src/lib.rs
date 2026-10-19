//! Durable workflow state, command, queue, subscription, and commit contracts.
//!
//! This crate is the independent durable owner of the Adaptive Flywheel
//! strategy database. It sits below the workflow runtime and the kernel host:
//!
//! ```text
//!   licoup-native  ──►  licoup-workflow-runtime  ──►  licoup-workflow-store  ──►  licoup-workflow
//!     (composes)            (executes)                   (persists)                 (pure machine)
//! ```
//!
//! Nothing here names the runtime or the host. Routing, subscription, and
//! admission control are the store's own durable contracts, which is why they
//! live with the store rather than above it: the store previously read them
//! back out of the runtime module, and that read is what made the runtime and
//! the store one cycle instead of one direction.
//!
//! The strategy database remains the local workflow authority. Runtime code
//! owns adapters and execution; this crate owns the short SQLite transactions
//! that make accepted state and delivery intent recoverable across a process
//! restart.

// The control contract (operations, scopes, grants, admission verdicts) and
// the routing vocabulary (channels, subscriptions, queues) are the engine's
// durable contracts; this crate keeps the module paths so `crate::control::*`
// and `crate::routing::*` resolve for the welded durable implementations.
pub use acativa::store::{control, routing};

mod admission;
mod commit;
#[cfg(test)]
mod conformance;
mod controlled;
#[cfg(test)]
mod dispatch_composition;
mod queue;
mod store;
mod subscriptions;

pub use admission::{
    MAX_UNFINISHED_WORKFLOW_WORK, UnfinishedWorkflowWork, WorkflowWorkBlocker, WorkflowWorkKind,
    read_unfinished_local_work,
};
pub use commit::{CommittedTransition, TransitionDecorator, TransitionIntent, TransitionObserver};
pub use control::{ControlOperation, ControlScope, ControlledStore};
pub use controlled::DurableControlledStore;
pub use queue::{DurableQueue, DurableQueueLease, DurableQueueStats, QueueReplay, QueueStoreError};
pub use store::StrategyStore;
pub use store::{normalize_legacy_workflow, validate_published_core_layout};
pub use subscriptions::DurableSubscriptionStore;

/// Definition, binding, projection, and error vocabulary the store and the
/// runtime both name. It is defined by the pure machine crate so neither of
/// them has to own the other's identity.
pub use licoup_workflow::{
    ASSISTANT_TEMPORARY_DEFINITION_PREFIX, BindingCandidate, BindingValue, StrategyAuthorization,
    StrategyDefinition, StrategyDefinitionSummary, StrategyDiagnostic, StrategyError,
    StrategyErrorCode, StrategyProjection,
};

pub const STRATEGY_SCHEMA_VERSION: &str = "licoup.adaptive-flywheel.state.v1";

/// Name the store by its domain boundary while preserving the established
/// `StrategyStore` type for callers that still use strategy terminology.
pub type WorkflowStore = StrategyStore;
