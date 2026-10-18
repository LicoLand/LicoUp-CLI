//! Event-driven workflow execution: durable admission, routing, node
//! execution and the graph driver.
//!
//! The runtime was the kernel host's own module. It is now its own crate, and
//! its dependencies point one way:
//!
//! ```text
//!   licoup-native  ──►  licoup-workflow-runtime  ──►  licoup-workflow-store  ──►  licoup-workflow
//!     (composes)            (executes)                   (persists)               (pure machine)
//! ```
//!
//! Nothing here reaches back into the host. Everything the runtime needs from
//! above arrives through [`ports`]: the host installs its answers once at its
//! crate root, and a process that installs none runs fail-closed.
//!
//! Runtime leaves are intentionally separate from the durable store: adapters
//! may perform external work, while store transactions only persist state,
//! commands, and post-commit intent.

pub mod adapter;
mod assistant;
pub mod driver;
pub mod evolution;
pub mod node;
mod package;
pub mod ports;
mod service;

// Routing and admission control are the durable store's own contracts. They
// are re-exported at this path because the runtime is their first reader.
pub use licoup_workflow_store::{control, routing};

pub use adapter::{
    AdapterError, AdapterExecutionStatus, CancelOutcome, CooperativeDrainAdapter,
    NodeCapabilityAdapter, NodeInvocation, PauseOutcome, ResumeOutcome,
    SingleWriterSessionRegistry, SteerOutcome, SyntheticCapabilityAdapter,
};
pub use assistant::{
    ASSISTANT_TEMPORARY_DEFINITION_PREFIX, AssistantPreflight, PreflightFailure, PreflightReceipt,
    preflight_assistant_graph,
};
pub use control::{
    AdmissionConflict, AdmissionReceipt, AdmissionRequest, ControlOperation, ControlScope,
    ControlledStore, InMemoryControlledStore, InterventionProxy, OperationGrant, VerifiedPrincipal,
};
pub use driver::{
    ContinuousNodeDriver, DispatchedEffectReport, DriverError, DriverStepOutcome, GraphPauseReport,
    GraphStopReport, PerNodePauseOutcome, PerNodeStopOutcome, ResultReentry,
};
pub use evolution::{
    AdoptedPlanningDefaultSeam, AgentModelOptionSeam, CallbackContextFacts, CallbackCostFacts,
    CallbackEvolutionEnricher, CallbackEvolutionPayload, CallbackFacts, CallbackObservationFacts,
    CallbackSuggestions, DefaultEvolutionContextPort, DefaultEvolutionObservationPort,
    DefaultEvolutionStrategyPort, EffectRecheckContext, EffectRecheckFailure, EffectRecheckReceipt,
    EvolutionContextPort, EvolutionCostPort, EvolutionObservationPort, EvolutionStrategyPort,
    GroupBGapReport, GroupBIntegrationGaps, LedgerEvolutionCostPort, NodeObservationSummary,
    PlanningScopeSeam, StrategySourceSeam, StrategySuggestion, StrategyVersionSeam,
    recheck_before_effect,
};
pub use node::{
    CancellationFacts, InvocationRecord, NodeExecutionError, NodeExecutionOutcome,
    NodeExecutionReceipt, NodeFacade, NodeObservation, PauseResult, ResumeResult, SteerResult,
    StopResult,
};
pub use package::{PreparedPackage, StrategyPackageImporter, synthetic_fixture_package_bytes};
pub use ports::{
    ActorTurnError, CandidateFilters, EffectPort, HostPorts, ModelFactsPort, PriceFacts,
    ProfileSnapshotAuthority, ResolvedRuntime, SharedSnapshotAuthority, StrategyEffectPermit,
    TargetFacts, UsageLedgerPort, host_ports, install_host_ports, project_profile_snapshot,
    project_profile_snapshots, rank_candidates, route_receipt,
};
pub use routing::{
    ActivationRule, ChannelKind, DeliveryMode, FairDispatchQueue, FrozenTargetRecipient,
    FrozenTargets, NodeCapability, NodeFeatureIndex, NodeLifecycleState, NodeMetadata, QueueBounds,
    QueueCapacityExceeded, QueuedItem, RecipientEffectStatus, Subscription, SubscriptionPredicate,
    SubscriptionRegistry, SubscriptionScope, TargetSelector,
};
pub use service::{ActorTurnPort, AssistantWakePort, StrategyService, TurnCancelDisposition};

pub use licoup_workflow_store::{
    BindingCandidate, BindingValue, STRATEGY_SCHEMA_VERSION, StrategyAuthorization,
    StrategyDefinition, StrategyDefinitionSummary, StrategyDiagnostic, StrategyError,
    StrategyErrorCode, StrategyProjection, StrategyStore, WorkflowStore,
};
