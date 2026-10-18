//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry and host queries, so adding
//! this Agent's crate is one entry rather than several lists that can drift.
//!
//! The adapter id and framing are this Agent's declaration, and the framing is
//! the same string its corpus records, so a transcript cannot pass against
//! another channel. Kimi Code reports its transitions with its own execution
//! result, so the SDK's normalized-transition query stays declared and
//! unanswered for this Agent — exactly as
//! [`ParserRegistration::unanswered`] states — and the Subagent mesh never
//! dispatches Kimi Code, so the durable-identity query stays unanswered too
//! rather than inheriting a neighbouring Agent's rule.

use licoup_agent_adapter_sdk::port::{AdapterParserSet, ParserRegistration};

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "kimi-code";

/// The framing its parser really speaks, and the channel its corpus records.
pub const FRAMING: &str = "lf-ndjson-acp";

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: licoup_agent_adapter_sdk::adapters::AdapterContract = parser::CONTRACT;

/// This Agent's registration, as composition reads it.
///
/// It is a `const` rather than only an element of [`registrations`] because a
/// composing program builds its own parser list at compile time and needs a
/// constant expression to put here.
pub const REGISTRATION: ParserRegistration = ParserRegistration::unanswered(CONTRACT);

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
/// itself is [`crate::replay::replay_arm`].
pub const fn parser_set() -> AdapterParserSet {
    AdapterParserSet {
        registrations,
        ..AdapterParserSet::unavailable()
    }
}

/// The adapter declaration of this package's parser, read through the SDK's own
/// registry lookup rather than from the constant, so the lookup and the set
/// cannot disagree.
pub fn contract() -> Option<licoup_agent_adapter_sdk::adapters::AdapterContract> {
    licoup_agent_adapter_sdk::registry::parser_for(&parser_set(), ADAPTER_ID)
}
