//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry and replay harness, so
//! adding this Agent's crate is one entry rather than four lists that can drift.
//!
//! # What this registration deliberately leaves unanswered
//!
//! The SDK declares two protocol-agnostic queries beside the declaration. Both
//! stay fail-closed here, and the reasons are this Agent's own facts rather than
//! an omission:
//!
//! - `execution_transitions` is not answered, because the OpenClaw driver
//!   reports the parser's own transition list on the run result it returns
//!   ([`crate::gateway_acp::model::RunResult::transitions`]). The projection the
//!   query would compute is already carried end to end by the caller that owns
//!   it, and answering a second time would be a second path to one fact.
//! - `valid_identity` is not answered, because a resumable OpenClaw identity is
//!   the Gateway `sessionKey` bound during an *attach*, not a location a reader
//!   can verify without one. A validator that guessed from the identifier's
//!   shape alone would claim a check this Agent does not perform. The binding
//!   that does check it lives in [`crate::gateway_acp::continuity`], and the
//!   Subagent mesh never dispatches an OpenClaw turn.
//!
//! Both are the kernel's own documented reading of this Agent, stated here by
//! the crate that now owns the parser rather than retyped by the composition.

use licoup_agent_adapter_sdk::port::{AdapterParserSet, ParserRegistration};
use licoup_agent_adapter_sdk::registry;

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "openclaw";

/// The framing its parser really speaks, and the channel its fixtures record.
pub const FRAMING: &str = "gateway-jsonrpc-acp";

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

/// The parser set composition injects: one Agent parser, and the replay arm this
/// package builds exactly as its own state machine does.
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
