//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry and host queries, so adding
//! this Agent's crate is one entry rather than thirteen lists that can drift.
//!
//! The two protocol-agnostic queries the SDK declares are answered differently
//! here, and both answers are Hermes' own facts:
//!
//! - [`execution_transitions`] turns one Hermes execution outcome into the
//!   shared transition vocabulary. Hermes reports no transition list with its
//!   execution result, so this is the one Agent whose normalized transitions the
//!   host reads *through* the query rather than from the driver's own result.
//!   The projection is a field copy: the facts already arrived on the outcome.
//! - The durable-identity query stays declared and unanswered
//!   ([`ParserRegistration::unanswered`]'s fail-closed answer, stated by
//!   [`no_identity`]): the Subagent mesh never dispatches Hermes, so it holds no
//!   durable dispatch identity for this Agent to validate, and the query must not
//!   inherit a neighbouring Agent's rule.
//!
//! The adapter id and framing are this Agent's declaration, and the framing is
//! the same string its corpus records, so a recorded transcript cannot pass
//! against another channel.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::port::{
    AdapterParserSet, DurableIdentityRequest, ExecutionOutcome, ParserRegistration,
};
use licoup_agent_adapter_sdk::{Transition, registry};

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "hermes";

/// The framing its parser really speaks, and the channel its corpus records.
pub const FRAMING: &str = "stdio-jsonrpc-acp";

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: AdapterContract = parser::CONTRACT;

/// This Agent's registration, as composition reads it.
///
/// It is a `const` rather than only an element of [`registrations`] because a
/// composing program builds its own parser list at compile time and needs a
/// constant expression to put here.
pub const REGISTRATION: ParserRegistration =
    ParserRegistration::new(CONTRACT, execution_transitions, no_identity);

/// The registrations this package injects into the adapter SDK.
static REGISTRATIONS: [ParserRegistration; 1] = [REGISTRATION];

/// The parser registrations this package publishes.
pub fn registrations() -> &'static [ParserRegistration] {
    &REGISTRATIONS
}

/// The parser set composition injects.
///
/// The replay arm is a test surface, so the production set carries none and a
/// production reader of `AdapterParserSet::replay_for` gets the SDK's
/// fail-closed answer rather than an arm this build does not have. The arm
/// itself is [`crate::replay::replay_arm`], which the host's replay suite drives
/// under the `test-support` feature.
pub const fn parser_set() -> AdapterParserSet {
    AdapterParserSet {
        registrations,
        ..AdapterParserSet::unavailable()
    }
}

/// The adapter declaration of this package's parser, read through the SDK's own
/// registry lookup rather than from the constant, so the lookup and the set
/// cannot disagree.
pub fn contract() -> Option<AdapterContract> {
    registry::parser_for(&parser_set(), ADAPTER_ID)
}

/// Hermes' normalized transitions for one execution outcome.
///
/// A completed execution becomes Hermes' reply transitions at the terminal
/// stage; a failed one becomes the shared failure transition carrying the
/// protocol's own code, stage and redacted message. Hermes' driver reports no
/// transition list of its own, so the host's Hermes normalization reads exactly
/// this query — the answer may not become empty without the host noticing.
pub fn execution_transitions(outcome: &ExecutionOutcome<'_>) -> Vec<Transition> {
    match outcome.failure {
        Some(failure) => parser::failed_transitions(failure.code, failure.stage, failure.message),
        None => parser::completed_transitions(outcome.output),
    }
}

/// Whether one durable native session identity is a Hermes session.
///
/// Fail-closed: the Subagent mesh never dispatches Hermes, so no durable
/// dispatch identity for this Agent exists to validate. The answer is declared
/// rather than omitted, so it can never be inherited from another Agent's rule.
pub fn no_identity(_: &DurableIdentityRequest<'_>) -> bool {
    false
}
