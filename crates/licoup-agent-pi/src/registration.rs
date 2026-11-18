//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry, replay harness and host
//! queries, so adding this Agent's crate is one entry rather than four lists
//! that can drift.
//!
//! Both protocol-agnostic queries the SDK declares stay *declared and
//! unanswered*, exactly as the client's own composition answered them before the
//! parser moved here:
//!
//! - Pi's driver carries the transition list the parser's own reducer built on
//!   every execution result, so no reader needs the separate transition query.
//! - The Subagent mesh never dispatches Pi, so there is no durable dispatch
//!   identity for the identity query to validate.
//!
//! The adapter id and framing are this Agent's declaration, and the framing is
//! the same string its fixtures record, so a corpus cannot pass against another
//! channel.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::port::{AdapterParserSet, ParserRegistration};
use licoup_agent_adapter_sdk::registry;

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "pi";

/// The framing its parser really speaks, and the channel its fixtures record.
pub const FRAMING: &str = "lf-jsonl-rpc";

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: AdapterContract = parser::CONTRACT;

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
pub fn contract() -> Option<AdapterContract> {
    registry::parser_for(&parser_set(), ADAPTER_ID)
}
