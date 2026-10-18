//! The host: one catalog owner, one instance registry, one admission path.
//!
//! [`ExtensionHost`] composes the four authorities this slice consumes and never
//! re-implements any of them: the catalog's admission rules
//! ([`licoup_application::DiscoveredCapabilities`]), the wire profile decision
//! ([`licoup_extension_contracts::profile`]), the instance/in-flight machine
//! ([`crate::platform::extension_packages::state`]) and the carrier
//! ([`super::carrier`]).
//!
//! Two locking rules keep it predictable:
//!
//! - One mutex guards every state change. Lifecycle operations and admissions
//!   are short and serialized, which is what makes "the catalog never mixes
//!   generations" a property of the code rather than of timing.
//! - No carrier call happens while the lock is held. A hanging extension
//!   occupies its own call; the catalog, the other instances and every reader
//!   stay responsive, and a fault is noticed by the host rather than by a
//!   blocked mutex.
//!
//! Readers never take the mutex at all: they take the published snapshot, which
//! is one immutable value per epoch.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use licoup_application::{ApplicationFailure, ContractRange, DeclaredAttribute, RecoveryAction};
use licoup_extension_contracts::deployment::InstanceLifecycle;
use serde_json::Value;

use crate::platform::extension_packages::state::{
    InstanceIdentity, InstanceMachine, InstanceRegistry, InstanceReport, Settlement,
};
use crate::state_machines::extension_invocation::Event as InvocationEvent;

use super::carrier::{
    CancelDisposition, CarrierSession, CarrierSpec, DispatchOutcome, ExtensionCarrier, FaultClass,
    Observation, classify_failure, fault_name,
};
use super::catalog::{
    CapabilityCatalog, CatalogAdmission, CatalogEntry, CatalogEpoch, CatalogSnapshot, SessionOwner,
};
use super::identity::HostIncarnation;
use super::invocation::{
    AdmittedInvocation, HookTicket, InvocationBinding, InvocationOutcome, LiveInvocation, LiveState,
};
use super::journal::{
    ActivationRecord, ActivePointer, CatalogJournal, CatalogWatermark, CatalogWriterPermit,
    StopReason, StopRecord,
};
use super::lifecycle::{
    ActivationReceipt, PreparedExtension, StageRequest, StagedExtension, attribute_buckets,
    foreign_prepared, host_component, initialize_request, live_profiles, resolve_profiles,
    served_capabilities, superseded,
};
use super::{actionable, as_uncertain, refusal};

/// How many live carrier sessions one host may hold at once.
const MAX_INSTANCES: usize = 1024;

/// How many unsettled invocations one host may hold at once.
const MAX_IN_FLIGHT: u32 = 4096;

/// How many invocation records are kept before settled ones are pruned.
const MAX_TRACKED_INVOCATIONS: usize = 8192;

/// The host id of the next constructed host.
static NEXT_HOST: AtomicU64 = AtomicU64::new(1);

/// The runtime extension host.
pub struct ExtensionHost {
    /// The identity of this host run. Every handle it issues carries it, and
    /// every handle it receives is checked against it before anything is looked
    /// up.
    incarnation: Arc<HostIncarnation>,
    carrier: Arc<dyn ExtensionCarrier>,
    host_contract_range: ContractRange,
    /// The durable catalogue record, when the composition has one. `None` is
    /// the explicit non-durable fixture mode; see
    /// [`ExtensionHost::without_journal`].
    journal: Option<Arc<dyn CatalogJournal>>,
    /// The exclusive claim on the journal's single writer slot, held for this
    /// host's lifetime and dropped with it. `None` in the process-local mode.
    _writer_permit: Option<CatalogWriterPermit>,
    state: Mutex<HostState>,
    catalog: CapabilityCatalog,
}

/// Every fact the host may change, under one lock.
struct HostState {
    epoch: CatalogEpoch,
    registry: InstanceRegistry,
    /// The published projection of each instance. Entries are kept after an
    /// instance stops, because "which generation served this and why was it
    /// retained" is a catalog fact, not a cache entry.
    entries: BTreeMap<String, CatalogEntry>,
    /// Live carrier sessions, one per instance that has not been shut down.
    sessions: BTreeMap<String, CarrierSession>,
    invocations: BTreeMap<String, LiveInvocation>,
    hooks: BTreeMap<String, HookTicket>,
    next_instance: u64,
    next_invocation: u64,
    next_hook: u64,
    /// The last generation *consumed* per package: an allocation in `prepare`
    /// counts, not only a published activation, so a preparation that never
    /// committed cannot hand its label to later work. Seeded from the durable
    /// watermark when a record is in effect.
    next_generation: BTreeMap<String, u64>,
    /// Stops whose durable record could not be written, newest last. The local
    /// stop happened; the durable active pointer may still name the instance,
    /// and hiding that would be the failure this seam exists to prevent.
    journal_anomalies: Vec<String>,
    /// Pointers a previous run recorded active whose owner has not been
    /// confirmed stopped. They route nothing, stay visible in every snapshot,
    /// and block a new instance of the same package and permission scope until
    /// one of them is resolved: neither adopting nor dropping an unverified
    /// owner is allowed.
    predecessors: BTreeMap<String, ActivePointer>,
}

impl ExtensionHost {
    /// Build a host whose catalogue identity is process-local only.
    ///
    /// No durable record is in effect: after a restart this host hands out
    /// epochs and generations from the beginning again, so generation
    /// references are not stable across runs and the host does not claim
    /// C09 §6 persistence. `identity_is_durable()` reports that plainly. This
    /// constructor exists for fixtures and for a composition with no package
    /// store yet; a production host uses [`ExtensionHost::with_journal`].
    pub fn without_journal(
        carrier: Arc<dyn ExtensionCarrier>,
        host_contract_range: ContractRange,
    ) -> Self {
        Self::assemble(
            carrier,
            host_contract_range,
            None,
            CatalogWatermark::default(),
        )
    }

    /// Build a host over the catalogue record, claiming its single writer slot.
    ///
    /// One managed root has one *writing* host: the claim is taken before
    /// anything is read or prepared, so a second host over the same journal is
    /// refused here — with `runtime_catalog_writer_busy`, before it prepares or
    /// starts a carrier — instead of becoming a second catalogue state machine
    /// that reads watermark zero and publishes its own generation one. Readers
    /// are unrestricted; only the writer is unique. The claim is released when
    /// this host drops, so a later host takes over and inherits the record.
    ///
    /// The epoch and the per-package consumed-generation watermark are seeded
    /// from what a previous run recorded, so a restarted host never hands out a
    /// generation or epoch that was already used; allocations, activations and
    /// stops are recorded as they happen. Active pointers the record still names
    /// become unconfirmed predecessors: visible in every snapshot, routing
    /// nothing, and blocking a new instance of the same package and permission
    /// scope until their owner is confirmed. Whether the identity is *durable*
    /// is the journal's own capability, not the fact that a journal exists.
    pub fn with_journal(
        carrier: Arc<dyn ExtensionCarrier>,
        host_contract_range: ContractRange,
        journal: Arc<dyn CatalogJournal>,
    ) -> Result<Self, ApplicationFailure> {
        let permit = journal.claim_writer()?;
        let watermark: CatalogWatermark = journal.watermark()?;
        let mut host = Self::assemble(carrier, host_contract_range, Some(journal), watermark);
        host._writer_permit = Some(permit);
        Ok(host)
    }

    fn assemble(
        carrier: Arc<dyn ExtensionCarrier>,
        host_contract_range: ContractRange,
        journal: Option<Arc<dyn CatalogJournal>>,
        watermark: CatalogWatermark,
    ) -> Self {
        // Instance ids must not collide with an instance a previous run
        // recorded: the catalogue is keyed by instance id, so a reused id would
        // overwrite a predecessor instead of keeping it visible.
        let next_instance = watermark
            .active
            .iter()
            .filter_map(|pointer| instance_suffix(&pointer.instance_id))
            .max()
            .unwrap_or(0);
        let predecessors = watermark
            .active
            .into_iter()
            .map(|pointer| (pointer.instance_id.clone(), pointer))
            .collect();
        let host = Self {
            // The counter is a display label only. Identity is the minted
            // incarnation, which is random, unforgeable and different in every
            // run — a counter restarts at one, and a handle from a previous run
            // must not become valid again.
            incarnation: Arc::new(HostIncarnation::mint(
                NEXT_HOST.fetch_add(1, Ordering::Relaxed),
            )),
            carrier,
            host_contract_range,
            journal,
            _writer_permit: None,
            state: Mutex::new(HostState {
                epoch: CatalogEpoch::from_recorded(watermark.epoch),
                registry: InstanceRegistry::new(),
                entries: BTreeMap::new(),
                sessions: BTreeMap::new(),
                invocations: BTreeMap::new(),
                hooks: BTreeMap::new(),
                next_instance,
                next_invocation: 0,
                next_hook: 0,
                next_generation: watermark.generations,
                journal_anomalies: Vec::new(),
                predecessors,
            }),
            catalog: CapabilityCatalog::new(),
        };
        // The seeded state is the first catalogue: predecessors a previous run
        // recorded are visible before anything else happens, not only after the
        // first mutation.
        host.publish_from_state();
        host
    }

    /// The catalog as every surface reads it: one immutable epoch.
    pub fn catalog(&self) -> Arc<CatalogSnapshot> {
        self.catalog.snapshot()
    }

    pub fn host_contract_range(&self) -> ContractRange {
        self.host_contract_range
    }

    /// The host component name, for a caller that asserts the error chain.
    pub const fn component() -> &'static str {
        host_component()
    }

    /// Stage one declaration: validate it and decide every profile and
    /// attribute against the catalog as it is now, without committing anything.
    pub fn stage(&self, request: StageRequest) -> Result<StagedExtension, ApplicationFailure> {
        request.descriptor.validate()?;
        for profile in &request.profiles {
            profile.validate()?;
        }
        if let Some(failure) = request
            .descriptor
            .compatible_with(self.host_contract_range)
            .refusal(&request.descriptor.plugin_id)
        {
            return Err(failure);
        }
        let snapshot = self.catalog.snapshot();
        let discovered = snapshot.discovered();
        // A required attribute the catalog cannot serve refuses this extension
        // and only this extension; an unknown optional one is preserved.
        let adopted = request.descriptor.admit(&discovered)?;
        let resolved = resolve_profiles(
            &request.profiles,
            self.host_contract_range,
            &request.methods,
            None,
        );
        let staged_capabilities = served_capabilities(&request.descriptor, &resolved);
        let declared_attributes: Vec<DeclaredAttribute> = request.descriptor.attributes.clone();
        let mut permission_scope = request.permission_scope.clone();
        permission_scope.sort();
        permission_scope.dedup();
        Ok(StagedExtension {
            package_id: request.descriptor.plugin_id.clone(),
            package_version: request.descriptor.implementation_version.clone(),
            implementation_version: request.descriptor.implementation_version.clone(),
            descriptor: request.descriptor,
            profiles: request.profiles,
            activation: request.activation,
            permission_scope,
            host_contract_range: self.host_contract_range,
            staged_epoch: snapshot.epoch(),
            resolved_profiles: resolved,
            staged_capabilities,
            declared_attributes,
            adopted_attributes: adopted,
        })
    }

    /// Prepare one staged extension: allocate its generation, start the carrier,
    /// handshake it and wait for `extension.ready`.
    ///
    /// Nothing here touches the published catalog. A failure releases the
    /// session and leaves the host exactly as it was.
    pub fn prepare(
        &self,
        staged: StagedExtension,
    ) -> Result<PreparedExtension, ApplicationFailure> {
        let expected_epoch = self.catalog.snapshot().epoch();
        let (instance_id, generation) = {
            let mut state = self.lock();
            if state.sessions.len() >= MAX_INSTANCES {
                return Err(refusal(
                    "extension_host_instance_limit",
                    "extension/prepare",
                ));
            }
            let instance_id = state.allocate_instance_id();
            let generation = state.allocate_generation(&staged.package_id);
            (instance_id, generation)
        };
        // A generation is consumed by preparation, before any carrier exists.
        // Recording it here means a crash between preparation and activation
        // burns the label instead of handing it to later work; the watermark is
        // about consumed generations, not only published ones.
        if let Some(journal) = &self.journal {
            journal.record_allocation(&staged.package_id, generation)?;
        }
        let spec = CarrierSpec {
            package_id: staged.package_id.clone(),
            package_version: staged.package_version.clone(),
            instance_id: instance_id.clone(),
            generation,
            profiles: staged
                .profiles
                .iter()
                .map(|profile| profile.id.clone())
                .collect(),
        };
        let session = self.carrier.start(&spec)?;
        let initialized = match self
            .carrier
            .initialize(&session, &initialize_request(&staged))
        {
            Ok(initialized) => initialized,
            Err(failure) => {
                let _ = self.carrier.shutdown(&session);
                return Err(failure);
            }
        };
        if let Err(failure) = self.carrier.ready(&session) {
            let _ = self.carrier.shutdown(&session);
            return Err(failure);
        }
        // The handshake's answer replaces the manifest's claim for the live
        // decision. The attribute decision is re-checked against the catalog as
        // it is now: a required capability that disappeared refuses the
        // preparation instead of committing an instance that cannot be served.
        let live = live_profiles(&staged, &initialized);
        let live_capabilities = served_capabilities(&staged.descriptor, &live);
        if let Err(failure) = staged
            .descriptor
            .admit(&self.catalog.snapshot().discovered())
        {
            let _ = self.carrier.shutdown(&session);
            return Err(failure);
        }
        Ok(PreparedExtension {
            staged,
            host: self.incarnation.as_ref().clone(),
            carrier: Arc::clone(&self.carrier),
            instance_id,
            generation,
            expected_epoch,
            session,
            live_profiles: live,
            live_capabilities,
            live_methods: initialized.methods,
            accepted_profiles: initialized.accepted_profiles,
        })
    }

    /// Commit one prepared extension by compare-and-swap against the epoch it
    /// observed.
    ///
    /// An unrelated concurrent commit is re-based onto. A preparation that a
    /// newer generation for the same package and permission scope has already
    /// replaced is refused and its session released: the catalog never holds
    /// two active generations of the same scope.
    pub fn activate(
        &self,
        prepared: PreparedExtension,
    ) -> Result<ActivationReceipt, ApplicationFailure> {
        let PreparedExtension {
            staged,
            host,
            carrier,
            instance_id,
            generation,
            expected_epoch,
            session,
            live_profiles,
            live_capabilities,
            live_methods: _,
            accepted_profiles: _,
        } = prepared;
        if !self.incarnation.same_as(&host) {
            // The session belongs to the carrier that started it, not to this
            // host: release it there, and never through a carrier that never
            // saw it.
            let _ = carrier.shutdown(&session);
            return Err(foreign_prepared(&instance_id));
        }
        let mut state = self.lock();
        if state.epoch != expected_epoch
            && state.superseded_by(&staged.package_id, &staged.permission_scope, generation)
        {
            drop(state);
            let _ = self.carrier.shutdown(&session);
            return Err(superseded(&staged.package_id, generation));
        }
        // An unconfirmed predecessor of the same package and permission scope
        // blocks the new instance: its owner may still be running, and replacing
        // it silently is exactly the re-dispatch across an unknown owner that
        // the durable record exists to prevent.
        if let Some(predecessor) = state
            .predecessors
            .values()
            .find(|pointer| {
                pointer.package_id == staged.package_id
                    && pointer.permission_scope == staged.permission_scope
            })
            .cloned()
        {
            drop(state);
            let _ = carrier.shutdown(&session);
            return Err(predecessor_unreconciled(&predecessor));
        }
        // The required-attribute decision must hold against the catalog being
        // replaced, not only against the one staging saw.
        let adopted = match staged.descriptor.admit(&state.snapshot().discovered()) {
            Ok(adopted) => adopted,
            Err(failure) => {
                drop(state);
                let _ = self.carrier.shutdown(&session);
                return Err(failure);
            }
        };
        let new_epoch = state.epoch.next();
        let identity = match InstanceIdentity::new(
            instance_id.clone(),
            staged.package_id.clone(),
            staged.package_version.clone(),
            generation,
            new_epoch.get(),
            staged.permission_scope.clone(),
        ) {
            Ok(identity) => identity,
            Err(failure) => {
                drop(state);
                let _ = self.carrier.shutdown(&session);
                return Err(failure);
            }
        };
        let machine = {
            let mut machine = match InstanceMachine::discovered(identity.clone()) {
                Ok(machine) => machine,
                Err(failure) => {
                    drop(state);
                    let _ = self.carrier.shutdown(&session);
                    return Err(failure);
                }
            };
            let prepared = machine
                .advance(InstanceLifecycle::Preparing)
                .and_then(|()| machine.advance(InstanceLifecycle::Active));
            match prepared {
                Ok(()) => machine,
                Err(failure) => {
                    drop(state);
                    let _ = self.carrier.shutdown(&session);
                    return Err(failure);
                }
            }
        };
        let (bound_attributes, preserved_attributes) = attribute_buckets(&adopted);
        let entry = CatalogEntry {
            identity,
            implementation_version: staged.implementation_version.clone(),
            supported_contract_range: staged.descriptor.supported_contract_range,
            profiles: live_profiles.clone(),
            capabilities: live_capabilities.clone(),
            activation: staged.activation,
            state: InstanceLifecycle::Active,
            admission: CatalogAdmission::Open,
            session_owner: SessionOwner::Held,
            attributes: staged.declared_attributes.clone(),
            bound_attributes,
            preserved_attributes,
        };
        // The generation reference is recorded before the epoch is published:
        // an activation whose pointer could not be recorded must not commit,
        // and nothing local has changed yet at this point.
        if let Some(journal) = &self.journal {
            let record = ActivationRecord {
                pointer: ActivePointer {
                    package_id: staged.package_id.clone(),
                    package_version: staged.package_version.clone(),
                    permission_scope: staged.permission_scope.clone(),
                    instance_id: instance_id.clone(),
                    generation,
                    registry_epoch: new_epoch.get(),
                },
            };
            if let Err(failure) = journal.record_activation(&record) {
                drop(state);
                let _ = carrier.shutdown(&session);
                return Err(failure);
            }
        }
        let drained =
            state.supersede_older(&staged.package_id, &staged.permission_scope, &instance_id);
        state.registry.insert(machine);
        state.sessions.insert(instance_id.clone(), session);
        state.entries.insert(instance_id.clone(), entry);
        state.epoch = new_epoch;
        let published = state.snapshot();
        drop(state);
        self.catalog.publish(published);
        self.finalize_empty_drains(&drained, StopReason::Drained);
        Ok(ActivationReceipt {
            instance_id,
            package_id: staged.package_id,
            generation,
            registry_epoch: new_epoch,
            drained,
            profiles: live_profiles,
            capabilities: live_capabilities,
        })
    }

    /// Admit one new call for a namespaced capability.
    ///
    /// The admission is atomic: the route is chosen and the in-flight slot
    /// reserved under the same lock, so a revoke or an activation that lands
    /// while a call is admitted either happens before it (and the call is
    /// refused or routed to the newer generation) or after it (and the binding
    /// stays on the generation that admitted it).
    pub fn begin(
        &self,
        capability: &str,
        request: &Value,
    ) -> Result<AdmittedInvocation, ApplicationFailure> {
        self.admit(capability, request, None)
    }

    /// The one admission path, optionally pinned to the instance generation a
    /// caller already holds.
    ///
    /// A pin is what makes a hook's request atomic rather than
    /// check-then-dispatch: the route, the pin and the in-flight reservation are
    /// resolved under one lock, so a hook whose generation was replaced while it
    /// was thinking is refused instead of being re-routed to the new generation.
    fn admit(
        &self,
        capability: &str,
        request: &Value,
        pin: Option<(&str, u64)>,
    ) -> Result<AdmittedInvocation, ApplicationFailure> {
        let (binding, session) = {
            let mut state = self.lock();
            let entry = match state.route(capability) {
                Some(entry) => {
                    if let Some((instance_id, generation)) = pin
                        && (entry.instance_id() != instance_id || entry.generation() != generation)
                    {
                        return Err(hook_generation_superseded(instance_id, generation));
                    }
                    entry.clone()
                }
                None => {
                    return Err(match pin {
                        Some((instance_id, generation)) => {
                            hook_generation_superseded(instance_id, generation)
                        }
                        None => state.capability_unavailable(capability),
                    });
                }
            };
            let instance_id = entry.instance_id().to_owned();
            if state.in_flight() >= MAX_IN_FLIGHT {
                return Err(actionable(
                    "extension_host_invocation_limit",
                    "extension/admit",
                    capability,
                )
                .with_recovery(RecoveryAction::RetryAfterRecovery));
            }
            let session = state
                .sessions
                .get(&instance_id)
                .cloned()
                .ok_or_else(|| state.capability_unavailable(capability))?;
            match state.registry.get_mut(&instance_id) {
                Some(machine) => machine.begin_in_flight()?,
                None => {
                    return Err(refusal("extension_instance_unknown", "extension/admit")
                        .with_field("instanceId"));
                }
            }
            let invocation_id = state.allocate_invocation_id();
            let binding = InvocationBinding {
                host: self.incarnation.as_ref().clone(),
                invocation_id: invocation_id.clone(),
                capability: capability.to_owned(),
                profile: entry.profile_serving(capability),
                package_id: entry.package_id().to_owned(),
                instance_id: instance_id.clone(),
                generation: entry.generation(),
                registry_epoch: entry.registry_epoch(),
            };
            state
                .invocations
                .insert(invocation_id, LiveInvocation::new(binding.clone()));
            state.prune_settled();
            (binding, session)
        };
        match self.carrier.dispatch(&session, &binding, request) {
            Ok(DispatchOutcome::Admitted) => Ok(AdmittedInvocation {
                binding,
                outcome: InvocationOutcome::Admitted,
            }),
            Ok(DispatchOutcome::Completed { payload }) => {
                self.settle_binding(&binding, Settlement::Completed, false)?;
                Ok(AdmittedInvocation {
                    binding,
                    outcome: InvocationOutcome::Finished { payload },
                })
            }
            Ok(DispatchOutcome::Natural(output)) => {
                self.settle_binding(&binding, Settlement::Completed, false)?;
                Ok(AdmittedInvocation {
                    binding,
                    outcome: InvocationOutcome::Natural(output),
                })
            }
            Err(failure) => Err(self.carrier_failure(&binding, &failure)),
        }
    }

    /// Look at an invocation, pinned to the generation that admitted it.
    pub fn observe(&self, binding: &InvocationBinding) -> Result<Observation, ApplicationFailure> {
        let session = self.session_for(binding)?;
        match self.carrier.observe(&session, binding) {
            Ok(Observation::Running) => Ok(Observation::Running),
            Ok(Observation::Completed { payload }) => {
                self.settle_binding(binding, Settlement::Completed, false)?;
                Ok(Observation::Completed { payload })
            }
            Ok(Observation::Natural(output)) => {
                self.settle_binding(binding, Settlement::Completed, false)?;
                Ok(Observation::Natural(output))
            }
            Ok(Observation::Unknown { code }) => {
                self.settle_binding(binding, Settlement::Unknown, false)?;
                Ok(Observation::Unknown { code })
            }
            Err(failure) => Err(self.carrier_failure(binding, &failure)),
        }
    }

    /// Request cancellation, pinned to the original generation.
    ///
    /// A cancellation is a request: `Acknowledged` is not a settlement, and
    /// only the result decides what happened to the work.
    pub fn cancel(
        &self,
        binding: &InvocationBinding,
    ) -> Result<CancelDisposition, ApplicationFailure> {
        let session = self.session_for(binding)?;
        match self.carrier.cancel(&session, binding) {
            Ok(CancelDisposition::Unknown) => {
                self.settle_binding(binding, Settlement::Unknown, false)?;
                Ok(CancelDisposition::Unknown)
            }
            Ok(disposition) => Ok(disposition),
            Err(failure) => Err(self.carrier_failure(binding, &failure)),
        }
    }

    /// The result of an invocation, pinned to the generation that admitted it.
    pub fn result(
        &self,
        binding: &InvocationBinding,
    ) -> Result<InvocationOutcome, ApplicationFailure> {
        let session = self.session_for(binding)?;
        match self.carrier.result(&session, binding) {
            Ok(Observation::Running) => {
                Err(refusal("extension_result_not_ready", "extension/result")
                    .with_field("invocationId"))
            }
            Ok(Observation::Completed { payload }) => {
                self.settle_binding(binding, Settlement::Completed, false)?;
                Ok(InvocationOutcome::Finished { payload })
            }
            Ok(Observation::Natural(output)) => {
                self.settle_binding(binding, Settlement::Completed, false)?;
                Ok(InvocationOutcome::Natural(output))
            }
            Ok(Observation::Unknown { code }) => {
                self.settle_binding(binding, Settlement::Unknown, false)?;
                Ok(InvocationOutcome::Unknown { code })
            }
            Err(failure) => Err(self.carrier_failure(binding, &failure)),
        }
    }

    /// Register a hook against one active instance.
    ///
    /// The ticket names the generation the hook was registered under. It is the
    /// only handle a hook gets, and it cannot dispatch anything.
    pub fn register_hook(
        &self,
        instance_id: &str,
        capability: &str,
    ) -> Result<HookTicket, ApplicationFailure> {
        let mut state = self.lock();
        let (package_id, generation, registered_at) = {
            let entry = state.entries.get(instance_id).ok_or_else(|| {
                refusal("extension_instance_unknown", "extension/hook").with_field("instanceId")
            })?;
            if !entry.routable() {
                return Err(actionable(
                    "extension_hook_instance_inactive",
                    "extension/hook",
                    "instanceId",
                )
                .with_recovery(RecoveryAction::RetryAfterRecovery));
            }
            if !entry.serves(capability) {
                return Err(actionable(
                    "extension_hook_capability_unserved",
                    "extension/hook",
                    capability,
                ));
            }
            (
                entry.package_id().to_owned(),
                entry.generation(),
                state.epoch,
            )
        };
        state.next_hook += 1;
        let ticket = HookTicket {
            host: self.incarnation.as_ref().clone(),
            ticket_id: format!("hook-{}", state.next_hook),
            package_id,
            instance_id: instance_id.to_owned(),
            generation,
            capability: capability.to_owned(),
            registered_at,
        };
        state.hooks.insert(ticket.ticket_id.clone(), ticket.clone());
        Ok(ticket)
    }

    /// Request one effect from a hook.
    ///
    /// The hook gets a *fresh* admission under the current catalog — never a
    /// re-dispatch of its old invocation — and only while the instance it was
    /// registered against is still the one serving the capability. A hook whose
    /// generation has been superseded or revoked is refused.
    pub fn hook_request_effect(
        &self,
        ticket: &HookTicket,
        capability: &str,
        request: &Value,
    ) -> Result<AdmittedInvocation, ApplicationFailure> {
        {
            let state = self.lock();
            let known = state.hooks.get(ticket.ticket_id()).ok_or_else(|| {
                refusal("extension_hook_unknown", "extension/hook").with_field("ticketId")
            })?;
            if !known.host.same_as(&ticket.host) {
                return Err(foreign_hook(ticket.host_display_id()));
            }
            if known != ticket {
                return Err(
                    refusal("extension_hook_forged", "extension/hook").with_field("ticketId")
                );
            }
        }
        // A fresh admission, under the current epoch, through the one admission
        // path, pinned to the generation the hook was registered against. The
        // hook never dispatches directly, and a superseded generation is refused
        // rather than silently re-routed.
        self.admit(
            capability,
            request,
            Some((ticket.instance_id(), ticket.generation())),
        )
    }

    /// Withdraw new admission from every running instance of one package.
    ///
    /// In-flight work keeps its binding and settles through the original
    /// generation; what is withdrawn is the right to accept *new* calls.
    /// Instances with nothing outstanding finalize immediately. Returns how
    /// many instances were affected.
    pub fn revoke(&self, package_id: &str) -> usize {
        let affected: Vec<String> = {
            let mut state = self.lock();
            let ids: Vec<String> = state
                .registry
                .instances()
                .filter(|machine| {
                    machine.identity().package_id == package_id && machine.is_running()
                })
                .map(|machine| machine.identity().instance_id.clone())
                .collect();
            for id in &ids {
                if let Some(machine) = state.registry.get_mut(id) {
                    let _ = machine.withdraw_admission();
                    if machine.state() == InstanceLifecycle::Active {
                        let _ = machine.drain();
                    }
                }
                state.sync_entry(id);
            }
            if !ids.is_empty() {
                state.epoch = state.epoch.next();
            }
            ids
        };
        if !affected.is_empty() {
            self.publish_from_state();
            self.finalize_empty_drains(&affected, StopReason::Revoked);
        }
        affected.len()
    }

    /// Stop one instance after its in-flight work has settled.
    ///
    /// Refused while work is still outstanding: a stopped instance with
    /// unpinned work is how an effect disappears without a record.
    pub fn finish_drain(&self, instance_id: &str) -> Result<(), ApplicationFailure> {
        {
            let mut state = self.lock();
            let machine = state.registry.get_mut(instance_id).ok_or_else(|| {
                refusal("extension_instance_unknown", "extension/drain").with_field("instanceId")
            })?;
            if machine.state() == InstanceLifecycle::Active {
                machine.drain()?;
            }
            if machine.in_flight() > 0 {
                return Err(actionable(
                    "extension_instance_work_unsettled",
                    "extension/drain",
                    "instanceId",
                )
                .with_presentation_arg("inFlight", &machine.in_flight().to_string())
                .with_recovery(RecoveryAction::RetryOrReviewRequest));
            }
            if machine.state() == InstanceLifecycle::Stopped {
                // A catalogue stop is not a confirmed release: the owner fact
                // decides whether the process is known to be gone.
                let verified = state
                    .entries
                    .get(instance_id)
                    .map(|entry| entry.session_owner.is_verified_stop())
                    .unwrap_or(true);
                if verified {
                    return Ok(());
                }
                return Err(session_owner_unverified(instance_id));
            }
        }
        self.finalize_drained(instance_id, StopReason::Drained)
    }

    /// Confirm that the process behind a stopped instance is gone.
    ///
    /// `Stopped` is a catalogue state; this is the separate verification H1
    /// requires before a stop may be treated as cleanup evidence. When this host
    /// still holds the session (the in-process case), the carrier's release is
    /// the confirmation and must succeed; when it does not hold one, the caller
    /// is the original session owner stating the fact.
    pub fn confirm_session_owner_stopped(
        &self,
        instance_id: &str,
    ) -> Result<(), ApplicationFailure> {
        // A predecessor from an earlier run has no catalogue entry and no
        // session here: confirming it clears the durable pointer it left behind,
        // and only that clearing makes its package and permission scope
        // activatable again.
        let predecessor = {
            let state = self.lock();
            state.predecessors.get(instance_id).cloned()
        };
        if let Some(predecessor) = predecessor {
            let mut state = self.lock();
            let published_epoch = state.epoch.next().get();
            if let Some(journal) = &self.journal
                && let Err(failure) = journal.record_stop(&StopRecord {
                    pointer: predecessor.clone(),
                    reason: StopReason::PredecessorConfirmed,
                    epoch: published_epoch,
                })
            {
                state
                    .journal_anomalies
                    .push(format!("{instance_id}: {}", failure.code));
                return Err(failure);
            }
            state.predecessors.remove(instance_id);
            state.epoch = CatalogEpoch::from_recorded(published_epoch);
            let snapshot = state.snapshot();
            drop(state);
            self.catalog.publish(snapshot);
            return Ok(());
        }
        let (session, pointer) = {
            let mut state = self.lock();
            let entry = state
                .entries
                .get(instance_id)
                .ok_or_else(|| {
                    refusal("extension_instance_unknown", "extension/drain")
                        .with_field("instanceId")
                })?
                .clone();
            match entry.session_owner {
                SessionOwner::StoppedVerified => return Ok(()),
                SessionOwner::Held => {
                    return Err(refusal("extension_session_owner_held", "extension/drain")
                        .with_field("instanceId")
                        .with_presentation_arg("instanceId", instance_id));
                }
                SessionOwner::StoppedUnverified => {}
            }
            let pointer = ActivePointer {
                package_id: entry.package_id().to_owned(),
                package_version: entry.identity.package_version.clone(),
                permission_scope: entry.identity.permission_scope.clone(),
                instance_id: instance_id.to_owned(),
                generation: entry.generation(),
                registry_epoch: entry.registry_epoch().get(),
            };
            (state.sessions.remove(instance_id), pointer)
        };
        if let Some(session) = session
            && let Err(failure) = self.carrier.shutdown(&session)
        {
            let mut state = self.lock();
            // A release that failed keeps its handle: the owner may still be
            // asked again, and the record must keep naming it.
            state.sessions.insert(instance_id.to_owned(), session);
            state
                .journal_anomalies
                .push(format!("{instance_id}: release {}", failure.code));
            return Err(
                refusal("extension_instance_shutdown_failed", "extension/drain")
                    .with_field("instanceId")
                    .with_presentation_arg("code", &failure.code),
            );
        }
        let snapshot = {
            let mut state = self.lock();
            let published_epoch = state.epoch.next().get();
            // Only a confirmed owner clears the durable pointer: this is the
            // record's own evidence that the slot is free.
            if let Some(journal) = &self.journal
                && let Err(failure) = journal.record_stop(&StopRecord {
                    pointer,
                    reason: StopReason::PredecessorConfirmed,
                    epoch: published_epoch,
                })
            {
                state
                    .journal_anomalies
                    .push(format!("{instance_id}: {}", failure.code));
                return Err(failure);
            }
            if let Some(entry) = state.entries.get_mut(instance_id) {
                entry.session_owner = SessionOwner::StoppedVerified;
            }
            state.epoch = CatalogEpoch::from_recorded(published_epoch);
            state.snapshot()
        };
        self.catalog.publish(snapshot);
        Ok(())
    }

    /// Instances the catalogue stopped without an owner confirming the process
    /// is gone. They are not cleanup evidence.
    pub fn unverified_session_owners(&self) -> Vec<String> {
        let state = self.lock();
        let mut owners: Vec<String> = state
            .entries
            .values()
            .filter(|entry| entry.session_owner == SessionOwner::StoppedUnverified)
            .map(|entry| entry.instance_id().to_owned())
            .collect();
        // A predecessor is unverified by definition: no owner in this run ever
        // confirmed it, and dropping it would be exactly the silence this rule
        // forbids.
        owners.extend(state.predecessors.keys().cloned());
        owners.sort();
        owners.dedup();
        owners
    }

    /// Stops whose durable record could not be written. The durable active
    /// pointer may still name these instances; the local stop happened.
    pub fn journal_anomalies(&self) -> Vec<String> {
        self.lock().journal_anomalies.clone()
    }

    /// Whether the journal this host was built over can promise durability.
    ///
    /// `false` covers both no journal at all and an in-memory fixture: identity
    /// is then process-local, generations and epochs are not claimed stable
    /// across runs, and the host does not claim C09 §6 persistence.
    pub fn identity_is_durable(&self) -> bool {
        self.journal
            .as_ref()
            .map(|journal| journal.durability().is_durable())
            .unwrap_or(false)
    }

    /// The active pointers a previous run recorded that no owner has confirmed
    /// stopped, newest first by instance id. They are visible in every snapshot
    /// and route nothing.
    pub fn pending_predecessors(&self) -> Vec<ActivePointer> {
        self.catalog
            .snapshot()
            .predecessors()
            .into_iter()
            .cloned()
            .collect()
    }

    /// What the durable record currently says is active, read back from the
    /// journal rather than from this run's memory.
    pub fn durable_active_pointers(&self) -> Vec<ActivePointer> {
        self.journal
            .as_ref()
            .and_then(|journal| journal.watermark().ok())
            .map(|watermark| watermark.active)
            .unwrap_or_default()
    }

    /// Reconcile the catalog against what is actually running after a restart.
    ///
    /// An instance the host held but that is no longer there is `Stopped` — a
    /// *catalogue* state, not evidence that the operating-system process exited.
    /// Nothing here calls the carrier: a session nobody observed is not a
    /// writer that stopped, so the instance is recorded with
    /// [`SessionOwner::StoppedUnverified`] until its owner confirms, and its
    /// unsettled work is recorded `unknown` — never re-dispatched. The returned
    /// ids are the instances that were missing.
    pub fn reconcile_after_restart(&self, observed_instance_ids: &[String]) -> Vec<String> {
        let missing: Vec<String> = {
            let mut state = self.lock();
            let reconcile_epoch = state.epoch.next().get();
            let seen: BTreeSet<&str> = observed_instance_ids.iter().map(String::as_str).collect();
            let missing: Vec<String> = state
                .registry
                .instances()
                .filter(|machine| {
                    machine.is_running() && !seen.contains(machine.identity().instance_id.as_str())
                })
                .map(|machine| machine.identity().instance_id.clone())
                .collect();
            for instance_id in &missing {
                let pointer = state.entries.get(instance_id).map(|entry| ActivePointer {
                    package_id: entry.package_id().to_owned(),
                    package_version: entry.identity.package_version.clone(),
                    permission_scope: entry.identity.permission_scope.clone(),
                    instance_id: instance_id.clone(),
                    generation: entry.generation(),
                    registry_epoch: entry.registry_epoch().get(),
                });
                let outstanding: Vec<String> = state
                    .invocations
                    .values()
                    .filter(|live| {
                        live.binding.instance_id == *instance_id
                            && live.state == LiveState::Outstanding
                    })
                    .map(|live| live.binding.invocation_id.clone())
                    .collect();
                for invocation_id in outstanding {
                    if let Some(live) = state.invocations.get_mut(&invocation_id) {
                        live.settle(InvocationEvent::SettleUnknown);
                    }
                    if let Some(machine) = state.registry.get_mut(instance_id) {
                        let _ = machine.settle(Settlement::Unknown);
                    }
                }
                if let Some(machine) = state.registry.get_mut(instance_id) {
                    let _ = machine.withdraw_admission();
                    if machine.state() == InstanceLifecycle::Active {
                        let _ = machine.drain();
                    }
                    let _ = machine.stop();
                }
                // The session handle is deliberately *kept*: this host did not
                // shut the instance down and no owner has confirmed the process
                // is gone, so dropping the only means of releasing it later
                // would be exactly the equation of "unobserved" with "stopped"
                // that this rule forbids.
                if let Some(entry) = state.entries.get_mut(instance_id) {
                    entry.session_owner = SessionOwner::StoppedUnverified;
                }
                if let (Some(journal), Some(pointer)) = (&self.journal, pointer)
                    && let Err(failure) = journal.record_stop(&StopRecord {
                        pointer,
                        reason: StopReason::ReconciledAfterRestart,
                        epoch: reconcile_epoch,
                    })
                {
                    state
                        .journal_anomalies
                        .push(format!("{instance_id}: {}", failure.code));
                }
                state.sync_entry(instance_id);
            }
            if !missing.is_empty() {
                state.epoch = CatalogEpoch::from_recorded(reconcile_epoch);
            }
            missing
        };
        if !missing.is_empty() {
            self.publish_from_state();
        }
        missing
    }

    /// Every instance the host holds, as the instance authority reports it.
    pub fn instance_reports(&self) -> Vec<InstanceReport> {
        self.lock().registry.reports()
    }

    /// One instance, if the host holds it.
    pub fn instance_report(&self, instance_id: &str) -> Option<InstanceReport> {
        self.lock()
            .registry
            .get(instance_id)
            .map(InstanceMachine::report)
    }

    // -----------------------------------------------------------------------
    // internals
    // -----------------------------------------------------------------------

    fn lock(&self) -> MutexGuard<'_, HostState> {
        match self.state.lock() {
            Ok(guard) => guard,
            // A panic elsewhere must not turn every later call into a second
            // failure: the host keeps answering from the last committed state.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Rebuild and publish the snapshot from the current state.
    fn publish_from_state(&self) {
        let snapshot = self.lock().snapshot();
        self.catalog.publish(snapshot);
    }

    /// The live session of one in-flight invocation, or a refusal that says why
    /// the binding cannot be followed.
    fn session_for(
        &self,
        binding: &InvocationBinding,
    ) -> Result<CarrierSession, ApplicationFailure> {
        if !self.incarnation.same_as(&binding.host) {
            return Err(foreign_invocation_binding(binding.host_display_id()));
        }
        let state = self.lock();
        let live = state
            .invocations
            .get(binding.invocation_id())
            .ok_or_else(|| {
                refusal("extension_invocation_unknown", "extension/invocation")
                    .with_field("invocationId")
            })?;
        if live.binding != *binding {
            return Err(refusal(
                "extension_invocation_binding_mismatch",
                "extension/invocation",
            )
            .with_field("invocationId"));
        }
        if live.state != LiveState::Outstanding {
            return Err(
                refusal("extension_invocation_settled", "extension/invocation")
                    .with_field("invocationId"),
            );
        }
        state
            .sessions
            .get(&binding.instance_id)
            .cloned()
            .ok_or_else(|| {
                refusal("extension_instance_unknown", "extension/invocation")
                    .with_field("instanceId")
            })
    }

    /// Settle one binding through the instance authority.
    ///
    /// `ignore_settled` is for the fault path, where the instance may already
    /// have been quarantined under the same lock and the settlement recorded
    /// there; the caller is not asking whether it won that race.
    fn settle_binding(
        &self,
        binding: &InvocationBinding,
        settlement: Settlement,
        ignore_settled: bool,
    ) -> Result<(), ApplicationFailure> {
        if !self.incarnation.same_as(&binding.host) {
            return Err(foreign_invocation_binding(binding.host_display_id()));
        }
        let drained_empty: Vec<String> = {
            let mut state = self.lock();
            let live_state = match state.invocations.get(binding.invocation_id()) {
                Some(live) if live.binding == *binding => live.state,
                Some(_) => {
                    return Err(refusal(
                        "extension_invocation_binding_mismatch",
                        "extension/settle",
                    )
                    .with_field("invocationId"));
                }
                None => {
                    return Err(refusal("extension_invocation_unknown", "extension/settle")
                        .with_field("invocationId"));
                }
            };
            if live_state != LiveState::Outstanding {
                if ignore_settled {
                    return Ok(());
                }
                return Err(refusal("extension_invocation_settled", "extension/settle")
                    .with_field("invocationId"));
            }
            let drained = {
                let machine = state
                    .registry
                    .get_mut(&binding.instance_id)
                    .ok_or_else(|| {
                        refusal("extension_instance_unknown", "extension/settle")
                            .with_field("instanceId")
                    })?;
                machine.settle(settlement)?;
                machine.state() == InstanceLifecycle::Draining && machine.in_flight() == 0
            };
            if let Some(live) = state.invocations.get_mut(binding.invocation_id()) {
                let event = match settlement {
                    Settlement::Completed => InvocationEvent::SettleCompleted,
                    Settlement::Unknown => InvocationEvent::SettleUnknown,
                };
                live.settle(event);
            }
            if drained {
                vec![binding.instance_id().to_owned()]
            } else {
                Vec::new()
            }
        };
        self.finalize_empty_drains(&drained_empty, StopReason::Drained);
        Ok(())
    }

    /// Turn a carrier failure into a host answer: a classified fault isolates
    /// the instance and settles its work unknown; an ordinary refusal only
    /// accounts for this call.
    fn carrier_failure(
        &self,
        binding: &InvocationBinding,
        failure: &ApplicationFailure,
    ) -> ApplicationFailure {
        match classify_failure(failure) {
            Some(class) => {
                let isolated = self.isolate_instance(binding.instance_id(), class);
                if !isolated {
                    let _ = self.settle_binding(binding, Settlement::Unknown, true);
                }
                as_uncertain(failure.clone())
            }
            None => {
                // The extension answered "no" to this call. The call is
                // accounted for and nothing is outstanding, so the in-flight
                // slot is released; it is not recorded unknown, which would
                // claim the effect might have happened.
                let _ = self.settle_binding(binding, Settlement::Completed, true);
                failure.clone()
            }
        }
    }

    /// Stop routing one instance after a carrier fault.
    ///
    /// The instance is `Failed` for a crash, an unresponsive call or a protocol
    /// violation, and `Quarantined` for a budget overrun or a revocation: both
    /// are visible facts, and neither is allowed to take the rest of the host
    /// down. Everything outstanding on the instance is recorded unknown, once.
    fn isolate_instance(&self, instance_id: &str, class: FaultClass) -> bool {
        let published = {
            let mut state = self.lock();
            if state.registry.get(instance_id).is_none() {
                return false;
            }
            let pointer = state.entries.get(instance_id).map(|entry| ActivePointer {
                package_id: entry.package_id().to_owned(),
                package_version: entry.identity.package_version.clone(),
                permission_scope: entry.identity.permission_scope.clone(),
                instance_id: instance_id.to_owned(),
                generation: entry.generation(),
                registry_epoch: entry.registry_epoch().get(),
            });
            let quarantined = matches!(class, FaultClass::OverBudget | FaultClass::Revoked);
            let note = format!("carrier {}", fault_name(class));
            if let Some(machine) = state.registry.get_mut(instance_id) {
                let _ = if quarantined {
                    machine.quarantine(note)
                } else {
                    machine.fail(note)
                };
            }
            let outstanding: Vec<String> = state
                .invocations
                .values()
                .filter(|live| {
                    live.binding.instance_id == instance_id && live.state == LiveState::Outstanding
                })
                .map(|live| live.binding.invocation_id.clone())
                .collect();
            for invocation_id in outstanding {
                if let Some(live) = state.invocations.get_mut(&invocation_id) {
                    live.settle(InvocationEvent::SettleUnknown);
                }
                if let Some(machine) = state.registry.get_mut(instance_id) {
                    let _ = machine.settle(Settlement::Unknown);
                }
            }
            state
                .hooks
                .retain(|_, hook| hook.instance_id() != instance_id);
            state.sync_entry(instance_id);
            state.epoch = state.epoch.next();
            let published_epoch = state.epoch.get();
            let session = state.sessions.remove(instance_id);
            (state.snapshot(), session, pointer, published_epoch)
        };
        let (snapshot, session, pointer, published_epoch) = published;
        self.catalog.publish(snapshot);
        let released = match &session {
            Some(session) => {
                // A faulted instance's transport is unusable; releasing it is
                // what keeps a quarantined extension from holding resources.
                // The result decides whether the owner is verified, not whether
                // the instance is isolated.
                self.carrier.shutdown(session).is_ok()
            }
            None => false,
        };
        {
            let mut state = self.lock();
            if !released && let Some(session) = session {
                // The process may still be running; keep the handle so its
                // owner can still be asked to release it.
                state.sessions.insert(instance_id.to_owned(), session);
            }
            if let Some(entry) = state.entries.get_mut(instance_id) {
                entry.session_owner = if released {
                    SessionOwner::StoppedVerified
                } else {
                    SessionOwner::StoppedUnverified
                };
            }
            if let (Some(journal), Some(pointer)) = (&self.journal, pointer)
                && let Err(failure) = journal.record_stop(&StopRecord {
                    pointer,
                    reason: if released {
                        StopReason::Faulted
                    } else {
                        StopReason::ReleaseFailed
                    },
                    epoch: published_epoch,
                })
            {
                state
                    .journal_anomalies
                    .push(format!("{instance_id}: {}", failure.code));
            }
        }
        true
    }

    /// Finalize every drained instance in `instance_ids` that has nothing
    /// outstanding. Automatic callers keep going and surface a durable-record
    /// failure through [`ExtensionHost::journal_anomalies`]; an explicit caller
    /// gets it from [`ExtensionHost::finish_drain`].
    fn finalize_empty_drains(&self, instance_ids: &[String], reason: StopReason) {
        for instance_id in instance_ids {
            let _ = self.finalize_drained(instance_id, reason);
        }
    }

    /// Stop one instance whose admission is already withdrawn and whose work is
    /// settled, releasing its carrier session.
    ///
    /// Two facts come out of this, and neither is inferred from the other: the
    /// catalogue state is `Stopped` only when the release was confirmed, and
    /// [`SessionOwner`] records whether that confirmation exists. A session that
    /// cannot be shut down leaves the instance `Failed`, its owner
    /// `StoppedUnverified`, and the caller gets a refusal — the process may
    /// still exist, and reporting a clean stop would hide that.
    fn finalize_drained(
        &self,
        instance_id: &str,
        reason: StopReason,
    ) -> Result<(), ApplicationFailure> {
        let session = {
            let mut state = self.lock();
            let Some(machine) = state.registry.get_mut(instance_id) else {
                return Ok(());
            };
            if machine.state() != InstanceLifecycle::Draining || machine.in_flight() > 0 {
                return Ok(());
            }
            state.sessions.remove(instance_id)
        };
        let (shutdown, session) = match session {
            Some(session) => (self.carrier.shutdown(&session), Some(session)),
            None => (Ok(()), None),
        };
        let (snapshot, outcome, pointer, published_epoch) = {
            let mut state = self.lock();
            let pointer = state.entries.get(instance_id).map(|entry| ActivePointer {
                package_id: entry.package_id().to_owned(),
                package_version: entry.identity.package_version.clone(),
                permission_scope: entry.identity.permission_scope.clone(),
                instance_id: instance_id.to_owned(),
                generation: entry.generation(),
                registry_epoch: entry.registry_epoch().get(),
            });
            let outcome = match shutdown {
                Ok(()) => {
                    if let Some(machine) = state.registry.get_mut(instance_id)
                        && machine.state() == InstanceLifecycle::Draining
                        && machine.in_flight() == 0
                    {
                        let _ = machine.stop();
                    }
                    if let Some(entry) = state.entries.get_mut(instance_id) {
                        entry.session_owner = SessionOwner::StoppedVerified;
                    }
                    Ok(())
                }
                Err(failure) => {
                    if let Some(machine) = state.registry.get_mut(instance_id) {
                        let _ = machine.fail(format!("carrier shutdown: {}", failure.code));
                    }
                    if let Some(entry) = state.entries.get_mut(instance_id) {
                        entry.session_owner = SessionOwner::StoppedUnverified;
                    }
                    // A release that failed keeps its handle: the owner may
                    // still be asked to confirm, and an unverified process is
                    // not one whose handle may be dropped.
                    if let Some(session) = session.clone() {
                        state.sessions.insert(instance_id.to_owned(), session);
                    }
                    Err(
                        refusal("extension_instance_shutdown_failed", "extension/drain")
                            .with_field("instanceId")
                            .with_presentation_arg("code", &failure.code),
                    )
                }
            };
            state.sync_entry(instance_id);
            state.epoch = state.epoch.next();
            let published_epoch = state.epoch.get();
            (state.snapshot(), outcome, pointer, published_epoch)
        };
        self.catalog.publish(snapshot);
        let record_reason = if outcome.is_err() {
            StopReason::ReleaseFailed
        } else {
            reason
        };
        if let (Some(journal), Some(pointer)) = (&self.journal, pointer)
            && let Err(failure) = journal.record_stop(&StopRecord {
                pointer,
                reason: record_reason,
                epoch: published_epoch,
            })
        {
            self.lock()
                .journal_anomalies
                .push(format!("{instance_id}: {}", failure.code));
            return Err(failure);
        }
        outcome
    }
}

impl HostState {
    fn snapshot(&self) -> CatalogSnapshot {
        CatalogSnapshot::from_entries(
            self.epoch,
            self.entries.values().cloned().collect(),
            self.predecessors.values().cloned().collect(),
        )
    }

    fn in_flight(&self) -> u32 {
        self.registry
            .instances()
            .map(|machine| machine.in_flight())
            .sum()
    }

    fn allocate_instance_id(&mut self) -> String {
        loop {
            self.next_instance += 1;
            let candidate = format!("instance-{}", self.next_instance);
            // A recorded predecessor may use the same shape; the catalogue is
            // keyed by instance id, so the id must be free before it is reused.
            if !self.entries.contains_key(&candidate) && !self.predecessors.contains_key(&candidate)
            {
                return candidate;
            }
        }
    }

    fn allocate_invocation_id(&mut self) -> String {
        self.next_invocation += 1;
        format!("invocation-{}", self.next_invocation)
    }

    /// The next generation for one package.
    ///
    /// Allocation is monotonic across *preparations*, not only across commits:
    /// a preparation that later loses the race has still consumed its number,
    /// so two concurrent preparations of one package never collide on one
    /// generation. The committed entries are also consulted, which keeps the
    /// counter sane if a catalog is reconstructed around existing instances.
    fn allocate_generation(&mut self, package_id: &str) -> u64 {
        let committed = self
            .entries
            .values()
            .filter(|entry| entry.package_id() == package_id)
            .map(CatalogEntry::generation)
            .max()
            .unwrap_or(0);
        let counter = self
            .next_generation
            .entry(package_id.to_owned())
            .or_insert(0);
        let next = (*counter).max(committed).saturating_add(1);
        *counter = next;
        next
    }

    /// Drop settled invocation records once enough of them have accumulated.
    ///
    /// The bindings of settled calls have already been answered; keeping every
    /// one forever would let a long-running host grow without bound. Unsettled
    /// records are never dropped: they are the outstanding obligations.
    fn prune_settled(&mut self) {
        if self.invocations.len() < MAX_TRACKED_INVOCATIONS {
            return;
        }
        self.invocations
            .retain(|_, live| live.state == LiveState::Outstanding);
    }

    /// The routable entry for one capability, with the same selection rule the
    /// published snapshot applies.
    fn route(&self, capability: &str) -> Option<&CatalogEntry> {
        self.entries
            .values()
            .filter(|entry| entry.routable() && entry.serves(capability))
            .max_by(|left, right| {
                left.generation()
                    .cmp(&right.generation())
                    .then_with(|| right.instance_id().cmp(left.instance_id()))
            })
    }

    /// Whether a newer-or-equal generation of the same package and permission
    /// scope is already running.
    fn superseded_by(
        &self,
        package_id: &str,
        permission_scope: &[String],
        generation: u64,
    ) -> bool {
        self.registry.instances().any(|machine| {
            machine.identity().package_id == package_id
                && machine.identity().permission_scope == permission_scope
                && machine.identity().generation >= generation
                && machine.is_running()
        })
    }

    /// Withdraw admission from the instances one commit replaces.
    fn supersede_older(
        &mut self,
        package_id: &str,
        permission_scope: &[String],
        keep: &str,
    ) -> Vec<String> {
        let superseded: Vec<String> = self
            .registry
            .instances()
            .filter(|machine| {
                machine.identity().package_id == package_id
                    && machine.identity().permission_scope == permission_scope
                    && machine.identity().instance_id != keep
                    && machine.is_running()
            })
            .map(|machine| machine.identity().instance_id.clone())
            .collect();
        for instance_id in &superseded {
            if let Some(machine) = self.registry.get_mut(instance_id) {
                match machine.state() {
                    InstanceLifecycle::Active | InstanceLifecycle::Draining => {
                        let _ = machine.drain();
                    }
                    _ => {
                        let _ = machine.fail("superseded before activation");
                    }
                }
            }
            self.sync_entry(instance_id);
        }
        superseded
    }

    /// Project the instance authority's state into the published entry.
    fn sync_entry(&mut self, instance_id: &str) {
        let Some(machine) = self.registry.get(instance_id) else {
            return;
        };
        let state = machine.state();
        let admission: CatalogAdmission = machine.admission().into();
        if let Some(entry) = self.entries.get_mut(instance_id) {
            entry.state = state;
            entry.admission = admission;
        }
    }

    /// The refusal for a capability that cannot be admitted, with a reason a
    /// surface can show without reading the registry.
    fn capability_unavailable(&self, capability: &str) -> ApplicationFailure {
        let serving: Vec<&CatalogEntry> = self
            .entries
            .values()
            .filter(|entry| entry.serves(capability))
            .collect();
        let reason = if serving.is_empty() {
            "not-installed"
        } else if serving
            .iter()
            .any(|entry| entry.admission == CatalogAdmission::Open)
        {
            "not-active"
        } else {
            "withdrawn"
        };
        actionable(
            "extension_capability_unavailable",
            "extension/admit",
            capability,
        )
        .with_presentation_arg("capability", capability)
        .with_presentation_arg("reason", reason)
    }
}

/// The refusal for a hook whose generation no longer serves the capability.
///
/// It is raised by the pinned admission itself, so it covers both a hook whose
/// generation was superseded and one whose instance stopped routing entirely:
/// either way the hook's effect is not re-dispatched from an old generation.
pub(crate) fn hook_generation_superseded(instance_id: &str, generation: u64) -> ApplicationFailure {
    actionable(
        "extension_hook_generation_superseded",
        "extension/hook",
        "generation",
    )
    .with_presentation_arg("instanceId", instance_id)
    .with_presentation_arg("generation", &generation.to_string())
}

/// The refusal for a binding another host run issued.
pub(crate) fn foreign_invocation_binding(host_display_id: u64) -> ApplicationFailure {
    refusal("extension_invocation_foreign_host", "extension/invocation")
        .with_field("invocationId")
        .with_presentation_arg("hostId", &host_display_id.to_string())
}

/// The refusal for a hook ticket another host run issued.
pub(crate) fn foreign_hook(host_display_id: u64) -> ApplicationFailure {
    refusal("extension_hook_foreign_host", "extension/hook")
        .with_field("ticketId")
        .with_presentation_arg("hostId", &host_display_id.to_string())
}

/// The refusal for treating an unconfirmed stop as cleanup evidence.
pub(crate) fn session_owner_unverified(instance_id: &str) -> ApplicationFailure {
    actionable(
        "extension_session_owner_unverified",
        "extension/drain",
        "instanceId",
    )
    .with_presentation_arg("instanceId", instance_id)
    .with_recovery(RecoveryAction::ReconcileBeforeRetry)
}

/// The numeric part of an `instance-N` id, when the id has that shape.
///
/// Only used to seed the allocator above ids a previous run recorded; an id in
/// another shape contributes nothing and the allocator still skips taken ids.
fn instance_suffix(instance_id: &str) -> Option<u64> {
    instance_id
        .strip_prefix("instance-")
        .and_then(|suffix| suffix.parse::<u64>().ok())
}

/// The refusal for replacing an instance whose owner was never confirmed
/// stopped.
///
/// The durable record says a previous run had this package and permission scope
/// active. Replacing it silently could run two owners of one scope at once, so
/// the new instance is refused until the predecessor is confirmed.
pub(crate) fn predecessor_unreconciled(pointer: &ActivePointer) -> ApplicationFailure {
    actionable(
        "extension_predecessor_unreconciled",
        "extension/activate",
        "packageId",
    )
    .with_presentation_arg("packageId", &pointer.package_id)
    .with_presentation_arg("instanceId", &pointer.instance_id)
    .with_presentation_arg("generation", &pointer.generation.to_string())
    .with_recovery(RecoveryAction::ReconcileBeforeRetry)
}
