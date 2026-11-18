//! C09: the Agent execution half — describe, execute, event, and the optional
//! capabilities around them.
//!
//! The smallest useful Agent implements three methods. It describes itself, it
//! accepts an invocation, and it emits events. It needs no model catalog, no
//! usage reporting, no tool calls, no session resume and no idempotency support
//! from a remote service, and none of those absences is a defect. Everything
//! beyond the three is a negotiated capability the host asks about through
//! `agent.describe` and then does without.
//!
//! Two of the rules here exist to stop a client from telling a comfortable
//! story:
//!
//! - **`execute` reports admission, never completion.** [`Admission`] cannot
//!   carry [`ReceiptKind::Completed`]; the end of the work is reported by a
//!   terminal *event*, which is a different fact from "the call came back".
//! - **A dropped connection is an unknown effect, not a retry.** With no remote
//!   idempotency, a submission that was sent and never answered may or may not
//!   have started something. Only an extension that says *not started* is a
//!   basis for resubmitting the same invocation
//!   ([`Admission::may_resubmit`]).
//!
//! An event's body is carried verbatim. The envelope is the SDK's business; the
//! content is not. Text that happens to look like JSON, or like a broken
//! envelope, is still text.

use crate::refusal;
use crate::transport::MAX_LOG_LINE_BYTES;
use licoup_application::{
    ApplicationFailure, EffectCertainty, NaturalOutput, OperationState, ReceiptKind, is_namespaced,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The longest invocation reference accepted.
pub const MAX_INVOCATION_REFERENCE_BYTES: usize = 160;

const STAGE: &str = "extension/agent";

/// What an event from an extension means to the host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentEventKind {
    /// Ordinary output. The body is verbatim.
    Text,
    /// A produced file or addressable artefact.
    Artifact,
    /// Progress, which is not an outcome.
    Progress,
    /// A state change of the extension's own session.
    State,
    /// The end of the work, and the only event that reports one.
    Terminal,
}

impl AgentEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Artifact => "artifact",
            Self::Progress => "progress",
            Self::State => "state",
            Self::Terminal => "terminal",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

/// One `agent.event` frame.
///
/// The envelope carries the invocation it belongs to, a monotone sequence, and
/// the kind. It does not carry identity, authority or a version: those are bound
/// once when the stream is established, so an ordinary content event does not
/// repeat them per chunk.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    /// The invocation this event belongs to.
    pub invocation_ref: String,
    /// Monotone within the invocation. The host attaches its own cursor.
    pub sequence: u64,
    pub kind: AgentEventKind,
    /// The body, carried verbatim and never parsed by the host.
    #[serde(default)]
    pub body: Value,
}

impl AgentEvent {
    /// Build an event, refusing one that cannot be attributed.
    pub fn new(
        invocation_ref: impl Into<String>,
        sequence: u64,
        kind: AgentEventKind,
        body: Value,
    ) -> Result<Self, ApplicationFailure> {
        let invocation_ref = invocation_ref.into();
        if invocation_ref.is_empty() || invocation_ref.len() > MAX_INVOCATION_REFERENCE_BYTES {
            return Err(
                refusal::new("agent_event_invocation_missing", STAGE).with_field("invocationRef")
            );
        }
        Ok(Self {
            invocation_ref,
            sequence,
            kind,
            body,
        })
    }

    /// An ordinary text event.
    ///
    /// The text is wrapped in [`NaturalOutput`], which never parses: a reply that
    /// looks like JSON is still a reply.
    pub fn text(
        invocation_ref: impl Into<String>,
        sequence: u64,
        text: impl Into<String>,
    ) -> Result<Self, ApplicationFailure> {
        let output = NaturalOutput::new(text);
        Self::new(
            invocation_ref,
            sequence,
            AgentEventKind::Text,
            Value::String(output.text().to_owned()),
        )
    }

    pub const fn is_terminal(&self) -> bool {
        self.kind.is_terminal()
    }
}

/// What an extension reports about one submission of `agent.execute`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionOutcome {
    /// The extension took the work. This is not a completion.
    Accepted,
    /// The extension reports it never started the work, which is the only basis
    /// for safely submitting the same invocation again.
    NotStarted,
    /// This invocation reference was already admitted; nothing was started twice.
    Duplicate,
    /// The submission was sent and no receipt was observed. The effect may or may
    /// not have happened.
    Unknown,
}

/// The receipt of an `agent.execute`.
///
/// Constructors are the only way to build one, and none of them can produce a
/// completed state: an extension that has not finished has not finished, and the
/// host re-derives completion from the terminal event and its own durable record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub invocation_ref: String,
    pub outcome: AdmissionOutcome,
}

impl Admission {
    fn of(invocation_ref: impl Into<String>, outcome: AdmissionOutcome) -> Self {
        Self {
            invocation_ref: invocation_ref.into(),
            outcome,
        }
    }

    pub fn accepted(invocation_ref: impl Into<String>) -> Self {
        Self::of(invocation_ref, AdmissionOutcome::Accepted)
    }

    pub fn not_started(invocation_ref: impl Into<String>) -> Self {
        Self::of(invocation_ref, AdmissionOutcome::NotStarted)
    }

    pub fn duplicate(invocation_ref: impl Into<String>) -> Self {
        Self::of(invocation_ref, AdmissionOutcome::Duplicate)
    }

    /// A submission whose outcome was never observed.
    pub fn unknown(invocation_ref: impl Into<String>) -> Self {
        Self::of(invocation_ref, AdmissionOutcome::Unknown)
    }

    pub const fn outcome(&self) -> AdmissionOutcome {
        self.outcome
    }

    /// Never [`ReceiptKind::Completed`]: admission is where a receipt stops
    /// short.
    pub const fn kind(&self) -> ReceiptKind {
        match self.outcome {
            AdmissionOutcome::Unknown => ReceiptKind::Unknown,
            AdmissionOutcome::Accepted
            | AdmissionOutcome::NotStarted
            | AdmissionOutcome::Duplicate => ReceiptKind::Admitted,
        }
    }

    /// The state a caller may follow. An unknown effect reports
    /// reconciliation-required so no caller reads "no answer" as "done".
    pub const fn state(&self) -> OperationState {
        self.kind().state()
    }

    /// How much is known about whether the invocation took effect.
    pub const fn effect_certainty(&self) -> EffectCertainty {
        match self.outcome {
            AdmissionOutcome::NotStarted => EffectCertainty::NotAttempted,
            AdmissionOutcome::Accepted | AdmissionOutcome::Duplicate => EffectCertainty::Applied,
            AdmissionOutcome::Unknown => EffectCertainty::Uncertain,
        }
    }

    /// Whether the same invocation may be submitted again.
    ///
    /// Only a known-not-executed outcome qualifies. An equal outer request id
    /// proves nothing about the remote service, so a submission that was sent and
    /// never answered is reconciled against the durable record instead of being
    /// replayed.
    pub const fn may_resubmit(&self) -> bool {
        matches!(self.outcome, AdmissionOutcome::NotStarted)
    }
}

/// What happened to an `agent.cancel` request.
///
/// The four outcomes are visible separately because they are four different
/// facts. A host that reports `Requested` as `Acknowledged` tells the user the
/// work stopped when it may not have.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CancelOutcome {
    /// The request was delivered; the extension has not answered yet.
    Requested,
    /// The extension confirmed it stopped its own work.
    Acknowledged,
    /// The extension has no cancel at all. A cancellation is still attempted by
    /// dropping what the host owns.
    Unsupported,
    /// The extension answered, and what happened to the work is not known.
    Unknown,
}

impl CancelOutcome {
    /// Whether this outcome settles the *external* effect.
    ///
    /// It never does. A cancellation is a request: once an effect left the
    /// machine, the client can stop waiting and stop the local work, and it
    /// cannot claim the remote effect was withdrawn.
    pub const fn settles_external_effect(self) -> bool {
        false
    }

    /// Whether the work has demonstrably stopped.
    pub const fn is_stopped(self) -> bool {
        matches!(self, Self::Acknowledged)
    }
}

/// How well an optional capability is supported, as `agent.describe` reports it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportLevel {
    Supported,
    Unsupported,
}

/// Whether this Agent reports usage at all.
///
/// `Unavailable` is a complete answer, not a gap: a specialist that runs no model
/// has nothing to report, and its absence of usage is not an execution failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageSupport {
    /// The Agent reports usage observations through C11.
    Reported,
    /// The Agent reports none. The host records no zeros on its behalf.
    Unavailable,
}

/// What an Agent says about itself.
///
/// `agent.describe` may be answered straight from the package manifest and costs
/// no inference. There is no field here for a credential, a price or a model
/// endpoint, so describing an Agent cannot bill anyone.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDescription {
    /// The namespaced Agent identity.
    pub id: String,
    /// What kind of instance this is, in the extension's own vocabulary.
    pub instance_kind: String,
    /// Input kinds the Agent accepts.
    #[serde(default)]
    pub input_kinds: Vec<String>,
    /// Namespaced capabilities the Agent offers.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The interface version the Agent implements.
    pub interface_version: String,
    #[serde(default = "unavailable")]
    pub usage: UsageSupport,
    #[serde(default = "unsupported")]
    pub cancel: SupportLevel,
    #[serde(default = "unsupported")]
    pub resume: SupportLevel,
}

fn unavailable() -> UsageSupport {
    UsageSupport::Unavailable
}

fn unsupported() -> SupportLevel {
    SupportLevel::Unsupported
}

impl AgentDescription {
    /// Structural validation: the identity and every capability are namespaced,
    /// and the interface version is a version.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if !is_namespaced(&self.id) {
            return Err(refusal::new("agent_description_invalid", STAGE).with_field("id"));
        }
        if !licoup_application::is_semver(&self.interface_version) {
            return Err(
                refusal::new("agent_description_invalid", STAGE).with_field("interfaceVersion")
            );
        }
        if self.instance_kind.is_empty() {
            return Err(refusal::new("agent_description_invalid", STAGE).with_field("instanceKind"));
        }
        for capability in &self.capabilities {
            if !is_namespaced(capability) {
                return Err(
                    refusal::new("agent_description_invalid", STAGE).with_field("capabilities")
                );
            }
        }
        Ok(())
    }
}

/// A request to continue an invocation that already exists.
///
/// `agent.observe` and `agent.resume` address a specific prior invocation and a
/// cursor into it. A resume that names no prior invocation is not a resume: it is
/// the same task submitted again, and answering it from the old work would
/// invent a continuation that never happened.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeRequest {
    /// The invocation being continued.
    pub prior_invocation_ref: String,
    /// Where the host stopped observing. An empty cursor means "from the start of
    /// the recorded stream", which is still a continuation of *this* invocation.
    #[serde(default)]
    pub cursor: String,
}

impl ResumeRequest {
    pub fn new(prior_invocation_ref: impl Into<String>, cursor: impl Into<String>) -> Self {
        Self {
            prior_invocation_ref: prior_invocation_ref.into(),
            cursor: cursor.into(),
        }
    }

    /// Refuse a continuation that names no prior work.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.prior_invocation_ref.is_empty()
            || self.prior_invocation_ref.len() > MAX_INVOCATION_REFERENCE_BYTES
        {
            return Err(
                refusal::new("agent_resume_requires_prior_invocation", STAGE)
                    .with_field("priorInvocationRef"),
            );
        }
        Ok(())
    }
}

/// The bound on one diagnostic line an extension writes, re-exported so an SDK
/// author has one source for it.
pub const MAX_DIAGNOSTIC_LINE_BYTES: usize = MAX_LOG_LINE_BYTES;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_never_reports_completion() {
        for admission in [
            Admission::accepted("run-1"),
            Admission::not_started("run-1"),
            Admission::duplicate("run-1"),
            Admission::unknown("run-1"),
        ] {
            assert_ne!(admission.kind(), ReceiptKind::Completed);
            assert_ne!(admission.state(), OperationState::Completed);
        }
        assert_eq!(Admission::unknown("run-1").kind(), ReceiptKind::Unknown);
        assert_eq!(
            Admission::unknown("run-1").state(),
            OperationState::ReconciliationRequired
        );
    }

    #[test]
    fn only_a_known_not_started_admission_may_be_resubmitted() {
        assert!(Admission::not_started("run-1").may_resubmit());
        for admission in [
            Admission::accepted("run-1"),
            Admission::duplicate("run-1"),
            Admission::unknown("run-1"),
        ] {
            assert!(!admission.may_resubmit(), "{:?}", admission.outcome());
        }
        assert_eq!(
            Admission::unknown("run-1").effect_certainty(),
            EffectCertainty::Uncertain
        );
        assert_eq!(
            Admission::not_started("run-1").effect_certainty(),
            EffectCertainty::NotAttempted
        );
    }

    #[test]
    fn cancel_outcomes_stay_distinct_and_never_settle_an_external_effect() {
        for outcome in [
            CancelOutcome::Requested,
            CancelOutcome::Acknowledged,
            CancelOutcome::Unsupported,
            CancelOutcome::Unknown,
        ] {
            assert!(!outcome.settles_external_effect());
        }
        assert!(CancelOutcome::Acknowledged.is_stopped());
        assert!(!CancelOutcome::Requested.is_stopped());
        assert!(!CancelOutcome::Unsupported.is_stopped());
        assert!(!CancelOutcome::Unknown.is_stopped());
    }

    #[test]
    fn event_bodies_are_verbatim_and_unattributed_events_are_refused() {
        let event = AgentEvent::text("run-1", 1, "{\"not\":\"an envelope\"}").expect("event");
        assert_eq!(
            event.body,
            Value::String("{\"not\":\"an envelope\"}".to_owned())
        );
        assert!(!event.is_terminal());
        assert!(AgentEvent::text("", 1, "x").is_err());
        let terminal =
            AgentEvent::new("run-1", 2, AgentEventKind::Terminal, Value::Null).expect("terminal");
        assert!(terminal.is_terminal());
    }

    #[test]
    fn resume_requires_a_prior_invocation() {
        assert!(ResumeRequest::new("run-1", "").validate().is_ok());
        let failure = ResumeRequest::new("", "").validate().expect_err("no prior");
        assert_eq!(failure.code, "agent_resume_requires_prior_invocation");
    }

    #[test]
    fn description_is_namespaced_and_carries_no_billing_field() {
        let description = AgentDescription {
            id: "example.specialist.echo".to_owned(),
            instance_kind: "executable".to_owned(),
            input_kinds: vec!["text".to_owned()],
            capabilities: vec!["example.specialist/stream".to_owned()],
            interface_version: "1.0.0".to_owned(),
            usage: UsageSupport::Unavailable,
            cancel: SupportLevel::Unsupported,
            resume: SupportLevel::Unsupported,
        };
        assert!(description.validate().is_ok());
        let wire = serde_json::to_value(&description).expect("serialize");
        for forbidden in ["credentialRef", "cost", "price", "apiKey", "model"] {
            assert!(
                wire.get(forbidden).is_none(),
                "{forbidden} must not be here"
            );
        }
        let mut bad = description.clone();
        bad.id = "echo".to_owned();
        assert!(bad.validate().is_err());
    }
}
