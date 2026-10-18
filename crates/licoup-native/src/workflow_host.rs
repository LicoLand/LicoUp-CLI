//! The workflow host composition: this host's answers for the ports
//! `licoup-workflow-runtime` declares.
//!
//! It lives at the crate root for the same reason [`crate::target_port`] does:
//! the extracted runtime must not reach into this host, and the host must not
//! answer a port from inside one of its own layers, because two of these
//! answers read domain facts while the platform layer may not reach the domain
//! layer. The layer that owns each fact stays where it is; this module joins
//! them once, above both, and [`install_workflow_host_ports`] installs them
//! once per process.
//!
//! Nothing in this module is a second implementation. Every answer delegates to
//! the owner that already had it: the profile authority to
//! [`crate::domain::client_conversation::production_snapshot_authority`], the
//! model facts to the model registry and the intelligence catalogue, the
//! ledger to the durable workflow usage ledger, and the effects to the existing
//! strategy runtime, lane dispatch and stop control.

use std::path::Path;
use std::sync::Arc;

use licoup_workflow::{BindingValue, RunCommand, RuntimeKind};
use licoup_workflow_runtime::ports::{
    EffectPort, HostPorts, ModelFactsPort, ResolvedRuntime, StrategyEffectPermit, UsageLedgerPort,
    install_host_ports,
};
use serde_json::Value;

/// Install this host's workflow composition. The first installation wins, so
/// repeated calls from any entry point stay one composition.
pub fn install_workflow_host_ports() {
    install_host_ports(workflow_host_ports());
}

/// This host's answers for the workflow runtime's ports.
pub fn workflow_host_ports() -> HostPorts {
    HostPorts {
        profile: crate::domain::client_conversation::production_snapshot_authority(),
        model: Arc::new(ProductionModelFacts),
        usage: Arc::new(ProductionUsageLedger),
        effect: Arc::new(ProductionEffects),
    }
}

/// The model registry's and the intelligence catalogue's own projections.
pub struct ProductionModelFacts;

impl ModelFactsPort for ProductionModelFacts {
    fn model_display_name(&self, model: &str) -> String {
        crate::domain::model_registry::model_display_name(model)
    }

    fn project_allowlisted_model(&self, model: &str) -> Option<Value> {
        crate::domain::agent_intelligence_catalog::project_allowlisted_model(model)
    }

    fn task_tags_for_model(&self, model: &str) -> Vec<String> {
        crate::domain::agent_intelligence_catalog::task_tags_for_model(model)
    }

    fn bundled_guide_skill_id(&self) -> Option<&'static str> {
        (!crate::domain::client_conversation::LICOUP_GUIDE_SKILL_SOURCE
            .trim()
            .is_empty())
        .then_some(licoup_conversation::LICOUP_GUIDE_SKILL_ID)
    }
}

/// The durable numeric Graph usage ledger.
pub struct ProductionUsageLedger;

impl UsageLedgerPort for ProductionUsageLedger {
    fn begin_graph_run(&self, payload: &Value) -> anyhow::Result<()> {
        licoup_statistics::ledger::begin_graph_run(payload)
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!(error.code))
    }

    fn record_graph_command(&self, payload: &Value) -> anyhow::Result<()> {
        licoup_statistics::ledger::record_graph_command(payload)
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!(error.code))
    }

    fn workflow_report(&self, payload: &Value) -> anyhow::Result<Value> {
        licoup_statistics::ledger::workflow_report(payload)
            .map_err(|error| anyhow::anyhow!(error.code))
    }
}

/// The strategy runtime, lane dispatch and stop control this host already owns.
pub struct ProductionEffects;

/// One runtime the strategy runtime resolved for one requirement.
struct HostRuntime(crate::platform::strategy_runtime::VerifiedRuntime);

impl ResolvedRuntime for HostRuntime {
    fn fingerprint(&self) -> &str {
        self.0.fingerprint()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl EffectPort for ProductionEffects {
    fn runtime_descriptors(&self) -> Vec<Value> {
        serde_json::to_value(
            crate::platform::strategy_runtime::RuntimeCatalog::discover().descriptors(),
        )
        .ok()
        .and_then(|value| match value {
            Value::Array(entries) => Some(entries),
            _ => None,
        })
        .unwrap_or_default()
    }

    fn compatible_runtime_id(
        &self,
        kind: RuntimeKind,
        version_requirement: &str,
    ) -> Option<String> {
        crate::platform::strategy_runtime::RuntimeCatalog::discover()
            .compatible_id(kind, version_requirement)
    }

    fn resolve_runtime(
        &self,
        runtime_id: &str,
        kind: RuntimeKind,
        version_requirement: &str,
    ) -> anyhow::Result<Arc<dyn ResolvedRuntime>> {
        let runtime = crate::platform::strategy_runtime::RuntimeCatalog::discover().resolve(
            runtime_id,
            kind,
            version_requirement,
        )?;
        Ok(Arc::new(HostRuntime(runtime)))
    }

    fn actor_fingerprint(
        &self,
        value_id: &str,
        model: &str,
        reasoning_effort: &str,
    ) -> anyhow::Result<String> {
        crate::platform::strategy_runtime::actor_fingerprint(value_id, model, reasoning_effort)
    }

    fn actor_capabilities(&self, value_id: &str) -> anyhow::Result<Value> {
        crate::agent_port::capabilities(value_id)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn admit_strategy_cwd(&self, cwd: &str) -> anyhow::Result<()> {
        crate::platform::strategy_runtime::admit_strategy_cwd(cwd)
    }

    fn execute_script(
        &self,
        command: &RunCommand,
        authorization_digest: &str,
        runtime: &Arc<dyn ResolvedRuntime>,
        revision_content: &Path,
        runtime_state_root: &Path,
        permit: &mut StrategyEffectPermit,
    ) -> anyhow::Result<Value> {
        let runtime = runtime
            .as_ref()
            .as_any()
            .downcast_ref::<HostRuntime>()
            .map(|runtime| &runtime.0)
            .ok_or_else(|| anyhow::anyhow!("strategy_runtime_unavailable"))?;
        crate::platform::strategy_runtime::execute_script(
            command,
            authorization_digest,
            runtime,
            revision_content,
            runtime_state_root,
            permit,
        )
    }

    fn execute_actor(
        &self,
        command: &RunCommand,
        authorization_digest: &str,
        binding: &BindingValue,
        permit: &mut StrategyEffectPermit,
        cwd: Option<&str>,
    ) -> anyhow::Result<Value> {
        crate::platform::strategy_runtime::execute_actor(
            command,
            authorization_digest,
            binding,
            permit,
            cwd,
        )
    }

    fn predecessor_locator(&self, facts: &Value) -> Value {
        crate::platform::strategy_runtime::predecessor_locator(facts)
    }

    fn dispatch_lane_operation(&self, operation: &str, params: &Value) -> anyhow::Result<Value> {
        // The agent-execution port is the one entry a caller above the platform
        // layer names; this host answers the runtime's effect port through it
        // rather than reaching the lane directly.
        crate::agent_port::dispatch(operation, params)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn new_correlation_id(&self) -> String {
        crate::platform::stop_control::new_correlation_id()
    }

    fn record_run_stop(
        &self,
        portable_root: &Path,
        correlation_id: &str,
        confirmed: bool,
    ) -> anyhow::Result<()> {
        crate::platform::stop_control::record_run_stop(portable_root, correlation_id, confirmed);
        Ok(())
    }
}
