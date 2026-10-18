//! Real components and controlled carriers for the host contract.
//!
//! What is real here: the production [`ExtensionHost`], the production catalog
//! snapshot types, the contract crate's profile/method decision, the descriptor's
//! admission rules and the real instance/in-flight machine from the package
//! store. The carrier is a controlled adapter implementing the production
//! [`ExtensionCarrier`] port in-process: it records every call it receives and
//! answers what the test scripts, which is how version, revocation and repeated
//! concurrency are exercised against the host's own rules.
//!
//! What is deliberately not here: no subprocess, no OS confinement and no claim
//! of either. A controlled carrier cannot stop an extension from touching what
//! the client can touch, so nothing in these tests is isolation proof; the
//! process-isolation suite owns
//! the real-process fixture and the confinement claims.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use licoup_application::{ActivationMode, CapabilityDescriptor, ContractRange};
use licoup_extension_contracts::profile::{DeclaredMethods, ProfileDeclaration};
use licoup_native::platform::extension_host::{
    ActivationReceipt, CancelDisposition, CarrierSession, CarrierSpec, CatalogJournal,
    DispatchOutcome, ExtensionCarrier, ExtensionHost, FaultClass, InitializeRequest,
    InitializedProfileSet, MemoryCatalogJournal, Observation, StageRequest, carrier_fault,
};
use serde_json::{Value, json};

/// One call a carrier received, with the binding facts the host published.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Call {
    Start {
        package: String,
        generation: u64,
        instance: String,
    },
    Initialize {
        instance: String,
    },
    Ready {
        instance: String,
    },
    Dispatch {
        instance: String,
        invocation: String,
        capability: String,
        generation: u64,
        registry_epoch: u64,
    },
    Observe {
        instance: String,
        invocation: String,
        generation: u64,
    },
    Cancel {
        instance: String,
        invocation: String,
        generation: u64,
    },
    Result {
        instance: String,
        invocation: String,
        generation: u64,
    },
    Shutdown {
        instance: String,
    },
}

/// What one dispatch answers with, unless the test scripts a fault.
#[derive(Clone, Debug)]
pub enum DispatchReply {
    Admitted,
    Completed(Value),
    Natural(String),
    Fault(FaultClass),
}

/// One live session the controlled carrier created.
#[derive(Debug)]
pub struct Session {
    /// The instance this session belongs to. A session is created by one
    /// `start`, so its instance is a fixed fact rather than a lookup.
    pub instance_id: String,
    pub dispatch_count: AtomicUsize,
    pub observe_count: AtomicUsize,
    pub result_count: AtomicUsize,
    pub cancel_count: AtomicUsize,
}

impl Session {
    pub fn dispatches(&self) -> usize {
        self.dispatch_count.load(Ordering::SeqCst)
    }

    pub fn observations(&self) -> usize {
        self.observe_count.load(Ordering::SeqCst)
    }
}

/// The scripted answers, one queue per method kind.
#[derive(Debug)]
struct Replies {
    methods: DeclaredMethods,
    accepted_profiles: Option<Vec<String>>,
    start_fault: Option<FaultClass>,
    initialize_fault: Option<FaultClass>,
    ready_fault: Option<FaultClass>,
    dispatch: DispatchReply,
    observe_fault: Option<FaultClass>,
    observations: VecDeque<Observation>,
    result: Option<Observation>,
    result_fault: Option<FaultClass>,
    cancel: Option<CancelDisposition>,
    cancel_fault: Option<FaultClass>,
    shutdown_fault: Option<FaultClass>,
}

/// A carrier the test drives: it records, it answers from the script, and it
/// holds one real session per started instance.
pub struct ControlledCarrier {
    name: &'static str,
    replies: Mutex<Replies>,
    calls: Mutex<Vec<Call>>,
    sessions: Mutex<BTreeMap<String, Arc<Session>>>,
    pub dropped_sessions: AtomicUsize,
}

impl ControlledCarrier {
    pub fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            replies: Mutex::new(Replies {
                methods: DeclaredMethods::minimal_agent(),
                accepted_profiles: None,
                start_fault: None,
                initialize_fault: None,
                ready_fault: None,
                dispatch: DispatchReply::Admitted,
                observe_fault: None,
                observations: VecDeque::new(),
                result: None,
                result_fault: None,
                cancel: None,
                cancel_fault: None,
                shutdown_fault: None,
            }),
            calls: Mutex::new(Vec::new()),
            sessions: Mutex::new(BTreeMap::new()),
            dropped_sessions: AtomicUsize::new(0),
        })
    }

    pub fn with_start_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").start_fault = Some(class);
    }

    pub fn with_initialize_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").initialize_fault = Some(class);
    }

    pub fn with_ready_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").ready_fault = Some(class);
    }

    pub fn with_dispatch(&self, reply: DispatchReply) {
        self.replies.lock().expect("replies").dispatch = reply;
    }

    pub fn with_observe_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").observe_fault = Some(class);
    }

    pub fn with_result_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").result_fault = Some(class);
    }

    pub fn with_cancel_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").cancel_fault = Some(class);
    }

    pub fn with_observation(&self, observation: Observation) {
        self.replies
            .lock()
            .expect("replies")
            .observations
            .push_back(observation);
    }

    pub fn with_result(&self, observation: Observation) {
        self.replies.lock().expect("replies").result = Some(observation);
    }

    pub fn with_cancel(&self, disposition: CancelDisposition) {
        self.replies.lock().expect("replies").cancel = Some(disposition);
    }

    pub fn with_methods(&self, methods: DeclaredMethods) {
        self.replies.lock().expect("replies").methods = methods;
    }

    pub fn accepted_profiles(&self, profiles: Vec<String>) {
        self.replies.lock().expect("replies").accepted_profiles = Some(profiles);
    }

    pub fn with_shutdown_fault(&self, class: FaultClass) {
        self.replies.lock().expect("replies").shutdown_fault = Some(class);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }

    pub fn dispatches(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Dispatch { .. }))
            .count()
    }

    /// The session of one instance, when it was started and not yet dropped.
    pub fn session(&self, instance_id: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions")
            .get(instance_id)
            .cloned()
    }

    fn record(&self, call: Call) {
        self.calls.lock().expect("calls").push(call);
    }
}

impl ExtensionCarrier for ControlledCarrier {
    fn start(
        &self,
        spec: &CarrierSpec,
    ) -> Result<CarrierSession, licoup_application::ApplicationFailure> {
        self.record(Call::Start {
            package: spec.package_id.clone(),
            generation: spec.generation,
            instance: spec.instance_id.clone(),
        });
        if let Some(class) = self.replies.lock().expect("replies").start_fault {
            return Err(carrier_fault(class));
        }
        let session = Arc::new(Session {
            instance_id: spec.instance_id.clone(),
            dispatch_count: AtomicUsize::new(0),
            observe_count: AtomicUsize::new(0),
            result_count: AtomicUsize::new(0),
            cancel_count: AtomicUsize::new(0),
        });
        self.sessions
            .lock()
            .expect("sessions")
            .insert(spec.instance_id.clone(), Arc::clone(&session));
        Ok(CarrierSession::new(Arc::clone(&session)))
    }

    fn initialize(
        &self,
        session: &CarrierSession,
        request: &InitializeRequest,
    ) -> Result<InitializedProfileSet, licoup_application::ApplicationFailure> {
        let instance = instance_of(session);
        self.record(Call::Initialize {
            instance: instance.clone(),
        });
        let replies = self.replies.lock().expect("replies");
        if let Some(class) = replies.initialize_fault {
            return Err(carrier_fault(class));
        }
        Ok(InitializedProfileSet {
            accepted_profiles: replies.accepted_profiles.clone().unwrap_or_else(|| {
                request
                    .profiles
                    .iter()
                    .map(|profile| profile.id.clone())
                    .collect()
            }),
            methods: replies.methods.clone(),
        })
    }

    fn ready(
        &self,
        session: &CarrierSession,
    ) -> Result<(), licoup_application::ApplicationFailure> {
        self.record(Call::Ready {
            instance: instance_of(session),
        });
        if let Some(class) = self.replies.lock().expect("replies").ready_fault {
            return Err(carrier_fault(class));
        }
        Ok(())
    }

    fn dispatch(
        &self,
        session: &CarrierSession,
        binding: &licoup_native::platform::extension_host::InvocationBinding,
        _: &Value,
    ) -> Result<DispatchOutcome, licoup_application::ApplicationFailure> {
        let instance = instance_of(session);
        if let Some(session) = self.session(&instance) {
            session.dispatch_count.fetch_add(1, Ordering::SeqCst);
        }
        self.record(Call::Dispatch {
            instance,
            invocation: binding.invocation_id().to_owned(),
            capability: binding.capability().to_owned(),
            generation: binding.generation(),
            registry_epoch: binding.registry_epoch().get(),
        });
        match self.replies.lock().expect("replies").dispatch.clone() {
            DispatchReply::Admitted => Ok(DispatchOutcome::Admitted),
            DispatchReply::Completed(payload) => Ok(DispatchOutcome::Completed { payload }),
            DispatchReply::Natural(text) => Ok(DispatchOutcome::Natural(
                licoup_application::NaturalOutput::new(text),
            )),
            DispatchReply::Fault(class) => Err(carrier_fault(class)),
        }
    }

    fn observe(
        &self,
        session: &CarrierSession,
        binding: &licoup_native::platform::extension_host::InvocationBinding,
    ) -> Result<Observation, licoup_application::ApplicationFailure> {
        let instance = instance_of(session);
        if let Some(session) = self.session(&instance) {
            session.observe_count.fetch_add(1, Ordering::SeqCst);
        }
        self.record(Call::Observe {
            instance,
            invocation: binding.invocation_id().to_owned(),
            generation: binding.generation(),
        });
        let mut replies = self.replies.lock().expect("replies");
        if let Some(class) = replies.observe_fault {
            return Err(carrier_fault(class));
        }
        match replies.observations.pop_front() {
            Some(observation) => Ok(observation),
            None => Ok(Observation::Running),
        }
    }

    fn cancel(
        &self,
        session: &CarrierSession,
        binding: &licoup_native::platform::extension_host::InvocationBinding,
    ) -> Result<CancelDisposition, licoup_application::ApplicationFailure> {
        let instance = instance_of(session);
        if let Some(session) = self.session(&instance) {
            session.cancel_count.fetch_add(1, Ordering::SeqCst);
        }
        self.record(Call::Cancel {
            instance,
            invocation: binding.invocation_id().to_owned(),
            generation: binding.generation(),
        });
        let replies = self.replies.lock().expect("replies");
        if let Some(class) = replies.cancel_fault {
            return Err(carrier_fault(class));
        }
        Ok(replies.cancel.unwrap_or(CancelDisposition::Acknowledged))
    }

    fn result(
        &self,
        session: &CarrierSession,
        binding: &licoup_native::platform::extension_host::InvocationBinding,
    ) -> Result<Observation, licoup_application::ApplicationFailure> {
        let instance = instance_of(session);
        if let Some(session) = self.session(&instance) {
            session.result_count.fetch_add(1, Ordering::SeqCst);
        }
        self.record(Call::Result {
            instance,
            invocation: binding.invocation_id().to_owned(),
            generation: binding.generation(),
        });
        let replies = self.replies.lock().expect("replies");
        if let Some(class) = replies.result_fault {
            return Err(carrier_fault(class));
        }
        Ok(replies
            .result
            .clone()
            .unwrap_or_else(|| Observation::Completed {
                payload: json!({"carrier": self.name}),
            }))
    }

    fn shutdown(
        &self,
        session: &CarrierSession,
    ) -> Result<(), licoup_application::ApplicationFailure> {
        let instance = instance_of(session);
        self.record(Call::Shutdown {
            instance: instance.clone(),
        });
        self.sessions.lock().expect("sessions").remove(&instance);
        self.dropped_sessions.fetch_add(1, Ordering::SeqCst);
        if let Some(class) = self.replies.lock().expect("replies").shutdown_fault {
            return Err(carrier_fault(class));
        }
        Ok(())
    }
}

fn instance_of(session: &CarrierSession) -> String {
    session
        .get::<Arc<Session>>()
        .expect("controlled carrier session")
        .instance_id
        .clone()
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A host whose catalogue identity is process-local: the fixture mode of the
/// harness. Persistence behaviour is exercised through
/// [`host_with_journal`], and the difference is a fact the host reports, not a
/// convention.
pub fn host(carrier: Arc<dyn ExtensionCarrier>) -> ExtensionHost {
    ExtensionHost::without_journal(carrier, contract_range())
}

/// A host wired to the in-memory fixture record.
pub fn host_with_journal(
    carrier: Arc<dyn ExtensionCarrier>,
    journal: Arc<MemoryCatalogJournal>,
) -> ExtensionHost {
    ExtensionHost::with_journal(carrier, contract_range(), journal).expect("catalogue journal")
}

/// A host wired to any catalogue record, including the real file-backed one.
pub fn host_with_catalog_journal(
    carrier: Arc<dyn ExtensionCarrier>,
    journal: Arc<dyn CatalogJournal>,
) -> ExtensionHost {
    ExtensionHost::with_journal(carrier, contract_range(), journal).expect("catalogue journal")
}

pub fn contract_range() -> ContractRange {
    ContractRange {
        major: 1,
        minimum_minor: 0,
    }
}

pub fn stage_request(
    package: &str,
    version: &str,
    capabilities: &[&str],
    profiles: &[(&str, &[&str])],
) -> StageRequest {
    let descriptor: CapabilityDescriptor = serde_json::from_value(json!({
        "pluginId": package,
        "implementationVersion": version,
        "supportedContractRange": {"major": 1, "minimumMinor": 0},
        "capabilities": capabilities,
    }))
    .expect("descriptor");
    StageRequest {
        descriptor,
        methods: DeclaredMethods::minimal_agent(),
        profiles: profiles
            .iter()
            .map(|(id, capabilities)| {
                ProfileDeclaration::new(*id, 1).with_capabilities(capabilities.iter().copied())
            })
            .collect(),
        activation: ActivationMode::OnDemand,
        permission_scope: vec!["scope:local".to_owned()],
    }
}

/// One package with one `agent-execution` profile serving its capabilities.
pub fn activate(
    host: &ExtensionHost,
    package: &str,
    capabilities: &[&str],
) -> Result<ActivationReceipt, licoup_application::ApplicationFailure> {
    let staged = host.stage(stage_request(
        package,
        "1.0.0",
        capabilities,
        &[("agent-execution", capabilities)],
    ))?;
    let prepared = host.prepare(staged)?;
    host.activate(prepared)
}

/// A descriptor with namespaced attributes, for the admission tests.
pub fn stage_request_with_attributes(
    package: &str,
    capabilities: &[&str],
    attributes: &[(&str, bool)],
) -> StageRequest {
    let attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, required)| {
            json!({
                "name": name,
                "requirement": if *required { "required" } else { "optional" },
            })
        })
        .collect();
    let descriptor: CapabilityDescriptor = serde_json::from_value(json!({
        "pluginId": package,
        "implementationVersion": "1.0.0",
        "supportedContractRange": {"major": 1, "minimumMinor": 0},
        "capabilities": capabilities,
        "attributes": attributes,
    }))
    .expect("descriptor");
    StageRequest {
        descriptor,
        methods: DeclaredMethods::minimal_agent(),
        profiles: vec![
            ProfileDeclaration::new("agent-execution", 1)
                .with_capabilities(capabilities.iter().copied()),
        ],
        activation: ActivationMode::OnDemand,
        permission_scope: vec!["scope:local".to_owned()],
    }
}
