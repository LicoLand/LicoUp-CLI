//! Native host composition for the independent workflow runtime crate.
//!
//! Event-driven workflow admission, routing, node execution and the graph
//! driver are owned by `licoup-workflow-runtime`; durable state is owned by
//! `licoup-workflow-store`. This module keeps the stable
//! `licoup_native::domain::workflow_runtime` path and is the one place that
//! joins the extracted runtime to this host's answers for its ports.

pub use licoup_workflow_runtime::{
    ASSISTANT_TEMPORARY_DEFINITION_PREFIX, BindingCandidate, BindingValue, STRATEGY_SCHEMA_VERSION,
    StrategyAuthorization, StrategyDefinition, StrategyDefinitionSummary, StrategyDiagnostic,
    StrategyError, StrategyErrorCode, StrategyProjection, StrategyStore, WorkflowStore,
};
pub use licoup_workflow_runtime::{
    ActorTurnError, ActorTurnPort, AdapterError, AdapterExecutionStatus, AssistantPreflight,
    AssistantWakePort, CancelOutcome, CooperativeDrainAdapter, NodeCapabilityAdapter,
    NodeInvocation, PauseOutcome, PreflightFailure, PreflightReceipt, ResumeOutcome,
    SingleWriterSessionRegistry, SteerOutcome, StrategyPackageImporter, SyntheticCapabilityAdapter,
    TurnCancelDisposition, adapter, control, driver, evolution, node, ports,
    preflight_assistant_graph, routing, synthetic_fixture_package_bytes,
};

use std::path::Path;

/// The workflow service this host composes.
///
/// Construction installs this host's answers for the runtime's ports once, so
/// every caller of the extracted runtime through this host reads the same
/// composition. Everything else is the extracted service's own API.
pub struct StrategyService {
    inner: licoup_workflow_runtime::StrategyService,
}

impl std::ops::Deref for StrategyService {
    type Target = licoup_workflow_runtime::StrategyService;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl StrategyService {
    pub fn open(portable_root: &Path) -> anyhow::Result<Self> {
        crate::install_workflow_host_ports();
        Ok(Self {
            inner: licoup_workflow_runtime::StrategyService::open(portable_root)?,
        })
    }

    pub fn from_parts(
        portable_root: std::path::PathBuf,
        store: StrategyStore,
        importer: StrategyPackageImporter,
    ) -> Self {
        crate::install_workflow_host_ports();
        Self {
            inner: licoup_workflow_runtime::StrategyService::from_parts(
                portable_root,
                store,
                importer,
            ),
        }
    }

    /// The builders below consume the receiver, so they cannot be reached
    /// through `Deref`: the extracted service would have to move out of the
    /// borrowed dereference. Each one rebinds the inner service and keeps the
    /// host wrapper, so a caller composes ports before running the service.
    pub fn with_host_ports(mut self, ports: ports::HostPorts) -> Self {
        self.inner = self.inner.with_host_ports(ports);
        self
    }

    pub fn with_actor_turn_port(mut self, actor_port: ActorTurnPort) -> Self {
        self.inner = self.inner.with_actor_turn_port(actor_port);
        self
    }

    pub fn with_assistant_wake_port(mut self, assistant_wake: AssistantWakePort) -> Self {
        self.inner = self.inner.with_assistant_wake_port(assistant_wake);
        self
    }

    pub fn with_profile_snapshot_authority(
        mut self,
        authority: ports::SharedSnapshotAuthority,
    ) -> Self {
        self.inner = self.inner.with_profile_snapshot_authority(authority);
        self
    }
}
