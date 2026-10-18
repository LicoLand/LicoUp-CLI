//! The port this SDK declares for the Agent parsers composed above it.
//!
//! The SDK owns what every adapter program shares — the byte-line ingress
//! contract, the adapter declaration, the framing and envelope helpers, the
//! registry lookup, the replay harness and the parser lifecycle machine — and
//! it owns no Agent's protocol. Everything it reports *about* one Agent arrives
//! through [`AdapterParserSet`], the collection of Agent parsers that the
//! composition above the SDK injects: `licoup-native` today, the driver and
//! per-Agent crates once those exist.
//!
//! The port is a value of `fn` pointers rather than a trait, so the SDK keeps
//! no state a caller did not hand it, and every member is read only on the
//! branch that needs it. [`AdapterParserSet::unavailable`] composes no Agent
//! parser at all: that is the honest answer for a program that carries none,
//! and it is what this crate's own tests state their expectations against.
//!
//! Two of the members are protocol-agnostic queries the host reads *about* one
//! Agent without naming any Agent's protocol, and each composing parser answers
//! for itself:
//!
//! - [`ParserRegistration::execution_transitions`] turns one execution outcome
//!   into the shared transition vocabulary. The host's normalization reads it
//!   where it used to call one Agent's `completed_transitions` /
//!   `failed_transitions` directly.
//! - [`ParserRegistration::valid_identity`] answers whether a durable native
//!   session identity is valid for that Agent, from the Agent's own recorded
//!   evidence. The host's exact-identity resolution reads it where it used to
//!   call one Agent's session-id or rollout-record validator directly.
//!
//! Both are declared here and answered by the composing parser; the SDK
//! implements neither.

use std::path::Path;

use crate::adapters::AdapterContract;
use crate::lifecycle::Transition;
use crate::replay::FrameReplay;

/// One execution outcome of one Agent's driver run.
///
/// The fields are the facts every Agent's normalized execution result already
/// carries, so projecting an outcome onto this shape is a field copy and never
/// a second derivation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionOutcome<'a> {
    /// The execution's output, as the Agent's driver reported it.
    pub output: &'a str,
    /// The protocol failure, when the execution failed.
    pub failure: Option<ExecutionFailure<'a>>,
}

/// One Agent protocol's reported failure, reduced to the facts the shared
/// transition vocabulary needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionFailure<'a> {
    /// The failure's stable code.
    pub code: &'a str,
    /// The lifecycle stage the failure was observed at.
    pub stage: &'a str,
    /// The failure's redacted message.
    pub message: &'a str,
}

/// The durable evidence one Agent's own records give about a native session
/// identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurableIdentityRequest<'a> {
    /// The native session identity to validate.
    pub session_id: &'a str,
    /// The recorded native location, when this Agent's protocol persists one.
    /// `None` means the binding carries no location, and the Agent answers from
    /// the identity alone.
    pub location: Option<&'a Path>,
}

/// Normalized transitions for one execution outcome of one Agent.
pub type ExecutionTransitions = fn(&ExecutionOutcome<'_>) -> Vec<Transition>;

/// Whether a durable native session identity is valid for one Agent, judged
/// from that Agent's own recorded evidence.
pub type DurableIdentityValid = fn(&DurableIdentityRequest<'_>) -> bool;

/// One Agent parser, in the shape the SDK's registry, host queries and replay
/// harness read it.
#[derive(Clone, Copy)]
pub struct ParserRegistration {
    /// The adapter declaration this Agent's parser reports.
    pub contract: AdapterContract,
    /// The normalized transitions for one of this Agent's execution outcomes.
    pub execution_transitions: ExecutionTransitions,
    /// Whether a durable native session identity is valid for this Agent.
    pub valid_identity: DurableIdentityValid,
}

impl ParserRegistration {
    /// A registration whose parser answers every query it declares.
    pub const fn new(
        contract: AdapterContract,
        execution_transitions: ExecutionTransitions,
        valid_identity: DurableIdentityValid,
    ) -> Self {
        Self {
            contract,
            execution_transitions,
            valid_identity,
        }
    }

    /// A registration that declares one Agent's adapter declaration and does
    /// not yet answer the two protocol-agnostic queries.
    ///
    /// It answers fail-closed — no transition and no valid identity — so a
    /// reader never inherits an answer the Agent did not give. A composing
    /// parser that owns those facts names its own functions instead of this
    /// constructor.
    pub const fn unanswered(contract: AdapterContract) -> Self {
        Self {
            contract,
            execution_transitions: |_| Vec::new(),
            valid_identity: |_| false,
        }
    }
}

/// Every Agent parser one program composes.
///
/// `registrations` is the single ordered list of the parsers this program
/// carries: the registry lookup, the corpus coverage check and both
/// protocol-agnostic queries read it, so adding an Agent parser is one entry
/// rather than four lists that can drift.
#[derive(Clone, Copy)]
pub struct AdapterParserSet {
    /// Every composed Agent parser, in packaged inventory order.
    pub registrations: fn() -> &'static [ParserRegistration],
    /// Build one composed Agent parser's replay arm, exactly as that Agent's
    /// production driver builds it. The replay harness is a test surface and a
    /// host that compiles no arms answers it with [`AdapterParserSet::unavailable`].
    pub replay: fn(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String>,
}

impl AdapterParserSet {
    /// No Agent parser composed. Every lookup fails closed.
    pub const fn unavailable() -> Self {
        Self {
            registrations: || &[],
            replay: |adapter_id| {
                Err(format!(
                    "no replayable parser is registered for adapter {adapter_id}"
                ))
            },
        }
    }

    /// Every composed Agent parser, in packaged inventory order.
    pub fn all(&self) -> &'static [ParserRegistration] {
        (self.registrations)()
    }

    /// The registration of one composed Agent parser, or `None` when this
    /// program composes no parser for that adapter id.
    pub fn registration(&self, adapter_id: &str) -> Option<ParserRegistration> {
        self.all()
            .iter()
            .copied()
            .find(|registration| registration.contract.id == adapter_id)
    }

    /// The adapter declaration of one composed Agent parser.
    pub fn contract(&self, adapter_id: &str) -> Option<AdapterContract> {
        self.registration(adapter_id)
            .map(|registration| registration.contract)
    }

    /// The framing one composed Agent parser really speaks.
    pub fn framing(&self, adapter_id: &str) -> Result<&'static str, String> {
        self.contract(adapter_id)
            .map(|contract| contract.framing)
            .ok_or_else(|| format!("no registered contract for adapter {adapter_id}"))
    }

    /// Every adapter id this program composes, in packaged inventory order.
    pub fn registered_ids(&self) -> Vec<&'static str> {
        self.all()
            .iter()
            .map(|registration| registration.contract.id)
            .collect()
    }

    /// The normalized transitions for one execution outcome of one composed
    /// Agent. `None` means this program composes no parser for that adapter.
    pub fn execution_transitions(
        &self,
        adapter_id: &str,
        outcome: &ExecutionOutcome<'_>,
    ) -> Option<Vec<Transition>> {
        self.registration(adapter_id)
            .map(|registration| (registration.execution_transitions)(outcome))
    }

    /// Whether a durable native session identity is valid for one composed
    /// Agent. `None` means this program composes no parser for that adapter.
    pub fn valid_identity(
        &self,
        adapter_id: &str,
        request: &DurableIdentityRequest<'_>,
    ) -> Option<bool> {
        self.registration(adapter_id)
            .map(|registration| (registration.valid_identity)(request))
    }

    /// Build the replay arm of one composed Agent parser. An adapter this
    /// program does not compose is refused before the arm is built.
    pub fn replay_for(&self, adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
        if self.registration(adapter_id).is_none() {
            return Err(format!(
                "no replayable parser is registered for adapter {adapter_id}"
            ));
        }
        (self.replay)(adapter_id)
    }
}
