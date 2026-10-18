//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry, replay harness and host
//! queries, so adding this Agent's crate is one entry rather than four lists
//! that can drift.
//!
//! Both protocol-agnostic queries the SDK declares are answered here, from this
//! Agent's own recorded evidence, rather than left fail-closed:
//!
//! - [`execution_transitions`] turns one Antigravity execution outcome into the
//!   shared transition vocabulary.
//! - [`valid_identity`] answers whether a durable native session identity is a
//!   real Antigravity conversation, judged by the identity rule this Agent's
//!   own protocol states.
//!
//! The adapter id and framing are this Agent's declaration, and the framing is
//! the same string its fixtures record, so a corpus cannot pass against another
//! channel.

use licoup_agent_adapter_sdk::port::{
    AdapterParserSet, DurableIdentityRequest, ExecutionOutcome, ParserRegistration,
};
use licoup_agent_adapter_sdk::{Transition, registry};

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "antigravity";

/// The framing its parser really speaks, and the channel its fixtures record.
///
/// Antigravity has no line protocol: a turn's vendor facts are the official
/// Agent Hooks receipt, the PTY lane's bytes and the supervised process's own
/// outcome, and this is the channel name the transcript records all three on.
pub const FRAMING: &str = "pty-hook-json";

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: licoup_agent_adapter_sdk::adapters::AdapterContract = parser::CONTRACT;

/// This Agent's registration, as composition reads it.
///
/// It is a `const` rather than only an element of [`registrations`] because a
/// composing program builds its own parser list at compile time and needs a
/// constant expression to put here.
pub const REGISTRATION: ParserRegistration =
    ParserRegistration::new(CONTRACT, execution_transitions, valid_identity);

/// The registrations this package injects into the adapter SDK.
static REGISTRATIONS: [ParserRegistration; 1] = [REGISTRATION];

/// The parser registrations this package publishes.
pub fn registrations() -> &'static [ParserRegistration] {
    &REGISTRATIONS
}

/// The parser set composition injects: one Agent parser, and the replay arm this
/// package builds exactly as its own driver does.
pub const fn parser_set() -> AdapterParserSet {
    AdapterParserSet {
        registrations,
        replay: crate::replay::replay_arm,
    }
}

/// The adapter declaration of this package's parser, read through the SDK's own
/// registry lookup rather than from the constant, so the lookup and the set
/// cannot disagree.
pub fn contract() -> Option<licoup_agent_adapter_sdk::adapters::AdapterContract> {
    registry::parser_for(&parser_set(), ADAPTER_ID)
}

/// This Agent's normalized transitions for one execution outcome.
///
/// A completed execution becomes this Agent's reply transitions over the output
/// the driver reported; a failed one becomes the shared failure transition with
/// the protocol's own code, stage and redacted message. The projection is a
/// field copy: the facts already arrived on the outcome.
pub fn execution_transitions(outcome: &ExecutionOutcome<'_>) -> Vec<Transition> {
    match outcome.failure {
        None => parser::completed_transitions(outcome.output),
        Some(failure) => parser::failure_transitions(failure.code, failure.stage, failure.message),
    }
}

/// Whether one durable native session identity is a real Antigravity
/// conversation.
///
/// Antigravity persists its conversations inside the vendor client, not as a
/// file this package can read, so the identity is judged on its own shape by the
/// same rule the parser applies to a receipt: non-empty, the protocol's bounded
/// length, and only the characters the vendor's conversation ids use. A
/// recorded location, when the binding carries one, must name the identity
/// itself — a path is a locator and never authorizes a resume.
pub fn valid_identity(request: &DurableIdentityRequest<'_>) -> bool {
    match request.location {
        Some(location) => location
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == request.session_id && parser::valid_session_id(stem)),
        None => parser::valid_session_id(request.session_id),
    }
}
