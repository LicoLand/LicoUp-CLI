//! Event-driven workflow execution: durable admission, routing, node
//! execution and the graph driver.
//!
//! The generic execution machinery — capability adapters, the node facade,
//! the continuous driver, and the in-process drive bookkeeping — is owned by
//! `acativa::runtime` and re-exported here under the same module paths. What
//! this crate owns is the LicoUp composition on top of it: the strategy
//! service's `execute` vocabulary, the Assistant preflight and package
//! importer, the Membership-profile port surface, and the evolution seams
//! that read the host's catalog.
//!
//! ```text
//!   licoup-native  ──►  licoup-workflow-runtime  ──►  licoup-workflow-store  ──►  licoup-workflow
//!     (composes)            (executes)                   (persists)               (pure machine)
//! ```
//!
//! Nothing here reaches back into the host. Everything the runtime needs from
//! above arrives through [`ports`]: the host installs its answers once at its
//! crate root, and a process that installs none runs fail-closed.

// Generic execution machinery, re-exported under the established module
// paths so `licoup_workflow_runtime::adapter::*` (and driver/node) keep
// resolving for this crate and its consumers.
pub use acativa::runtime::{adapter, driver, node};

mod assistant;
pub mod evolution;
mod package;
pub mod ports;
mod service;

// Routing and admission control are the durable store's own contracts. They
// are re-exported at this path because the runtime is their first reader.
pub use licoup_workflow_store::{control, routing};

pub use acativa::runtime::adapter::{
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
