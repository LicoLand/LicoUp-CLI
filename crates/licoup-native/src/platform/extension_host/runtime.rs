//! The production composition: one managed root, the package store, the
//! isolation carrier and the host, serving the `agent-execution` profile.
//!
//! Everything below the catalog has an owner already: the package store owns
//! the bytes and the record, [`PackagePrograms`] resolves an installed
//! manifest's own entry point, [`IsolatedProcessCarrier`] owns the process and
//! its operating-system envelope, and [`ExtensionHost`] owns the catalog, the
//! generations, admission and settlement. This module is the one place that
//! composes them, and it adds no second answer to any of those questions.
//!
//! What it does own:
//!
//! - **One layout.** The managed root holds the store (`packages/`, `records/`,
//!   `staging/`, `cache/`), one directory per instance (`instances/`), and the
//!   two records (`journal/`). Every path the carrier confines is derived from
//!   that one root, so the store and the confinement plan cannot disagree.
//! - **One declaration per package.** An installed manifest is read from the
//!   store and turned into the [`StageRequest`] the host validates. The
//!   capability set comes from the manifest's own `agent-execution` profile, the
//!   activation mode from the manifest, the permission scope from the record the
//!   install committed, and the method claim is the published baseline — the
//!   handshake's own answer replaces it for the live decision.
//! - **One call shape.** `begin`, `observe`, `cancel` and `result` are the
//!   host's; [`ExtensionRuntime::serve_agent_execution`] is the composition's
//!   projection of one `agent-execution` call onto them, and it is the call the
//!   production interface makes.
//!
//! The composition does not decide confinement for the user. [`IsolationMode`]
//! is a parameter: `restricted` is refused on a host that cannot enforce it, and
//! `trusted_local` is an explicitly selected unconfined run whose record says so.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use licoup_application::{
    ApplicationFailure, CapabilityDescriptor, ContractRange, LifecycleSupport, QuotaShape,
};
use licoup_extension_contracts::manifest::PackageManifest;
use licoup_extension_contracts::profile::{DeclaredMethods, ExtensionProfile};
use serde_json::{Value, json};

use crate::platform::extension_packages::{InstanceReport, PackageStore, ensure_private_directory};

use super::carrier::{CancelDisposition, Observation};
use super::catalog::{CatalogDocument, CatalogSnapshot};
use super::host::ExtensionHost;
use super::invocation::{AdmittedInvocation, InvocationBinding, InvocationOutcome};
use super::isolation::{
    IsolatedProcessCarrier, IsolationMode, IsolationPolicy, PackagePrograms, program_unavailable,
};
use super::journal::RuntimeCatalogJournal;
use super::lifecycle::{ActivationReceipt, StageRequest};
use super::refusal;

/// The host protocol range this client speaks.
pub const HOST_CONTRACT_RANGE: ContractRange = ContractRange {
    major: 1,
    minimum_minor: 0,
};

/// The published capability the `agent-execution` profile contributes.
///
/// A package declares the capabilities its profile serves in its own manifest,
/// so this is the default distribution's own name rather than a rule: the
/// composition reads what the committed instance serves and only falls back to
/// this name when the profile contributes none.
pub const AGENT_EXECUTION_CAPABILITY: &str = "agent-execution.v1";

/// The profile id every Agent-execution call is admitted for.
const AGENT_EXECUTION_PROFILE: &str = "agent-execution";

/// The polling interval one `serve` call uses while it waits for settlement.
const SETTLEMENT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The longest a caller may wait for one call to settle.
const MAX_SETTLEMENT_WAIT: Duration = Duration::from_secs(10 * 60);

/// One composed extension runtime over a managed root.
pub struct ExtensionRuntime {
    root: PathBuf,
    programs: Arc<PackagePrograms>,
    carrier: Arc<IsolatedProcessCarrier>,
    host: ExtensionHost,
}

impl ExtensionRuntime {
    /// Open the runtime over one managed root.
    ///
    /// The root's single-writer lease is taken here: a second runtime over the
    /// same root is refused rather than allowed to disagree about the catalog.
    pub fn open(root: &Path, mode: IsolationMode) -> Result<Self, ApplicationFailure> {
        ensure_private_directory(root)?;
        let store = PackageStore::open(root)?;
        let root = store.root().to_path_buf();
        let programs = Arc::new(PackagePrograms::new(store));
        let policy = IsolationPolicy::new(mode, &root);
        let carrier = IsolatedProcessCarrier::new(programs.clone(), policy)?;
        let journal = RuntimeCatalogJournal::open(&root)?;
        let host =
            ExtensionHost::with_journal(carrier.clone(), HOST_CONTRACT_RANGE, Arc::new(journal))?;
        Ok(Self {
            root,
            programs,
            carrier,
            host,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The program source over this runtime's package store.
    pub fn programs(&self) -> &Arc<PackagePrograms> {
        &self.programs
    }

    /// The carrier every instance of this runtime runs under.
    pub fn carrier(&self) -> &Arc<IsolatedProcessCarrier> {
        &self.carrier
    }

    pub fn host(&self) -> &ExtensionHost {
        &self.host
    }

    /// The catalog every surface reads, as one serializable document.
    pub fn catalog_document(&self) -> CatalogDocument {
        self.host.catalog().document()
    }

    pub fn catalog(&self) -> Arc<CatalogSnapshot> {
        self.host.catalog()
    }

    pub fn instance_reports(&self) -> Vec<InstanceReport> {
        self.host.instance_reports()
    }

    /// The installed manifest behind one package version.
    pub fn installed_manifest(
        &self,
        package_id: &str,
        package_version: &str,
    ) -> Result<PackageManifest, ApplicationFailure> {
        self.programs
            .installed_manifest(package_id, package_version)
    }

    /// The staged declaration one installed package makes for the
    /// `agent-execution` profile.
    ///
    /// A package whose manifest declares no `agent-execution` profile has not
    /// been enabled for Agent work, and this refuses it by name instead of
    /// staging a declaration that could never be served.
    pub fn agent_execution_request(
        &self,
        package_id: &str,
        package_version: &str,
    ) -> Result<StageRequest, ApplicationFailure> {
        let manifest = self.installed_manifest(package_id, package_version)?;
        let recorded = self
            .programs
            .store()
            .installed_version(package_id, package_version)?
            .ok_or_else(|| program_unavailable(package_id, package_version))?;
        let agent = manifest
            .published_profiles()
            .find(|profile| *profile == ExtensionProfile::AgentExecution)
            .ok_or_else(|| agent_execution_not_declared(package_id))?;
        let declaration = manifest
            .profiles
            .iter()
            .find(|profile| profile.id == agent.id())
            .expect("a published profile came from a declaration");
        let capabilities: Vec<String> = declaration.capabilities.clone();
        if capabilities.is_empty() {
            return Err(agent_execution_not_declared(package_id));
        }
        let permission_scope = recorded
            .permissions
            .iter()
            .map(|permission| format!("{}@{}", permission.capability, permission.scope))
            .collect();
        let descriptor = CapabilityDescriptor {
            plugin_id: manifest.id.clone(),
            implementation_version: manifest.version.clone(),
            supported_contract_range: manifest.host_protocol,
            capabilities: capabilities.clone(),
            required_grants: manifest
                .permissions
                .iter()
                .map(|permission| permission.capability.clone())
                .collect(),
            quota_shape: QuotaShape::default(),
            lifecycle: LifecycleSupport::default(),
            attributes: Vec::new(),
        };
        Ok(StageRequest {
            descriptor,
            // The manifest carries no method catalog: what an extension
            // implements is answered by `extension.initialize`, and that answer
            // is the one the host uses for the live decision. The published
            // baseline is the claim staged here.
            methods: DeclaredMethods::minimal_agent(),
            profiles: vec![declaration.clone()],
            activation: manifest.activation,
            permission_scope,
        })
    }

    /// Commit one installed package as an `agent-execution` extension, or reuse
    /// the instance this catalog already routes.
    ///
    /// Activation is not repeated for the same package version the catalog
    /// already routes: that routing is the fact, so a second call finds the
    /// committed instance instead of burning a generation. A different version
    /// is a different instance and is prepared on its own.
    pub fn activate_agent_execution(
        &self,
        package_id: &str,
        package_version: &str,
    ) -> Result<ActivationReceipt, ApplicationFailure> {
        if let Some(entry) = self.host.catalog().entries().find(|entry| {
            entry.package_id() == package_id
                && entry.identity.package_version == package_version
                && entry.routable()
        }) {
            return Ok(ActivationReceipt {
                instance_id: entry.instance_id().to_owned(),
                package_id: entry.package_id().to_owned(),
                generation: entry.generation(),
                registry_epoch: entry.registry_epoch(),
                drained: Vec::new(),
                profiles: entry.profiles.clone(),
                capabilities: entry.capabilities.clone(),
            });
        }
        let staged = self
            .host
            .stage(self.agent_execution_request(package_id, package_version)?)?;
        let prepared = self.host.prepare(staged)?;
        self.host.activate(prepared)
    }

    /// Admit one call. This is the host's own `begin`.
    pub fn begin(
        &self,
        capability: &str,
        request: &Value,
    ) -> Result<AdmittedInvocation, ApplicationFailure> {
        self.host.begin(capability, request)
    }

    pub fn observe(&self, binding: &InvocationBinding) -> Result<Observation, ApplicationFailure> {
        self.host.observe(binding)
    }

    pub fn cancel(
        &self,
        binding: &InvocationBinding,
    ) -> Result<CancelDisposition, ApplicationFailure> {
        self.host.cancel(binding)
    }

    pub fn result(
        &self,
        binding: &InvocationBinding,
    ) -> Result<InvocationOutcome, ApplicationFailure> {
        self.host.result(binding)
    }

    /// The capability one committed instance's `agent-execution` profile
    /// contributes, as the catalog publishes it.
    ///
    /// This is the manifest's own declaration, not a host-side convention: a
    /// package that names its capability something else is served under that
    /// name.
    pub fn agent_execution_capability(&self, instance_id: &str) -> Option<String> {
        self.host
            .catalog()
            .entry(instance_id)
            .and_then(|entry| {
                entry.profiles.iter().find(|profile| {
                    profile.id == AGENT_EXECUTION_PROFILE && profile.status.serves()
                })
            })
            .and_then(|profile| profile.capabilities.first().cloned())
    }

    /// One `agent-execution` call, from the installed package to settlement.
    ///
    /// The order is the contract's: the package is committed, one capability is
    /// admitted, and the call is then observed until it settles or the caller's
    /// deadline passes. A cancellation is requested only at the caller's own
    /// instant and never settles anything by itself — the observation still
    /// decides what happened, which is why the report carries the outcome, not
    /// the disposition.
    ///
    /// The wait uses [`ExtensionHost::observe`], whose contract is to look at an
    /// in-flight invocation. [`ExtensionHost::result`] is the single read for a
    /// caller that already knows the work settled: it refuses an outstanding
    /// invocation by design, so a waiter must not use it as a poll.
    pub fn serve_agent_execution(
        &self,
        package_id: &str,
        package_version: &str,
        call: &AgentExecutionCall,
    ) -> Result<Value, ApplicationFailure> {
        let receipt = self.activate_agent_execution(package_id, package_version)?;
        let capability = match &call.capability {
            Some(capability) => capability.clone(),
            None => self
                .agent_execution_capability(&receipt.instance_id)
                .unwrap_or_else(|| AGENT_EXECUTION_CAPABILITY.to_owned()),
        };
        if !self.host.catalog().serves(&capability) {
            return Err(
                refusal("extension_capability_not_served", "extension/execute")
                    .with_field("capability")
                    .with_presentation_arg("capability", &capability),
            );
        }
        let admitted = self.begin(&capability, &call.request)?;
        let binding = admitted.binding.clone();
        let deadline = Instant::now() + call.wait.min(MAX_SETTLEMENT_WAIT);
        let cancel_at = call.cancel_after.map(|after| Instant::now() + after);
        let mut cancellation: Option<CancelDisposition> = None;
        let outcome = match &admitted.outcome {
            InvocationOutcome::Admitted => loop {
                if let Some(at) = cancel_at
                    && cancellation.is_none()
                    && Instant::now() >= at
                {
                    cancellation = Some(self.cancel(&binding)?);
                }
                match self.observe(&binding)? {
                    Observation::Running => {}
                    Observation::Completed { payload } => {
                        break InvocationOutcome::Finished { payload };
                    }
                    Observation::Natural(output) => {
                        break InvocationOutcome::Natural(output);
                    }
                    Observation::Unknown { code } => {
                        break InvocationOutcome::Unknown { code };
                    }
                }
                if Instant::now() >= deadline {
                    return Err(refusal("extension_execution_timeout", "extension/execute")
                        .with_field("invocationId")
                        .with_presentation_arg("invocationId", binding.invocation_id()));
                }
                std::thread::sleep(SETTLEMENT_POLL_INTERVAL);
            },
            other => other.clone(),
        };
        Ok(json!({
            "packageId": package_id,
            "packageVersion": package_version,
            "instanceId": receipt.instance_id,
            "generation": receipt.generation,
            "registryEpoch": receipt.registry_epoch.get(),
            "capability": capability,
            "invocationId": binding.invocation_id(),
            "profile": binding.profile(),
            "cancelDisposition": cancellation.map(cancel_name),
            "outcome": outcome_json(&outcome),
        }))
    }
}

/// One `agent-execution` call as a caller describes it.
#[derive(Clone, Debug)]
pub struct AgentExecutionCall {
    /// The capability to admit. The profile's own published capability is the
    /// default; a caller that names another one is refused unless an active
    /// instance serves it.
    pub capability: Option<String>,
    pub request: Value,
    /// How long the caller waits for the work to settle.
    pub wait: Duration,
    /// When to request cancellation, measured from admission. A request never
    /// settles the call: the result still decides.
    pub cancel_after: Option<Duration>,
}

impl AgentExecutionCall {
    pub fn new(request: Value) -> Self {
        Self {
            capability: None,
            request,
            wait: Duration::from_secs(30),
            cancel_after: None,
        }
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.capability = Some(capability.into());
        self
    }

    pub fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    pub fn with_cancel_after(mut self, after: Duration) -> Self {
        self.cancel_after = Some(after);
        self
    }
}

fn cancel_name(disposition: CancelDisposition) -> &'static str {
    match disposition {
        CancelDisposition::Requested => "requested",
        CancelDisposition::Acknowledged => "acknowledged",
        CancelDisposition::Unsupported => "unsupported",
        CancelDisposition::Unknown => "unknown",
    }
}

fn outcome_json(outcome: &InvocationOutcome) -> Value {
    match outcome {
        InvocationOutcome::Admitted => json!({"state": "admitted"}),
        InvocationOutcome::Finished { payload } => {
            json!({"state": "completed", "payload": payload})
        }
        InvocationOutcome::Natural(output) => {
            json!({"state": "natural", "text": output.text()})
        }
        InvocationOutcome::Unknown { code } => {
            json!({"state": "unknown", "code": code})
        }
    }
}

/// The refusal for a package that does not serve Agent work.
fn agent_execution_not_declared(package_id: &str) -> ApplicationFailure {
    refusal(
        "extension_agent_execution_not_declared",
        "extension/activate",
    )
    .with_field("profiles")
    .with_presentation_arg("packageId", package_id)
    .with_presentation_arg("profile", "agent-execution")
}
