//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry, replay harness and host
//! queries, so adding this Agent's crate is one entry rather than four lists
//! that can drift.
//!
//! Both protocol-agnostic queries the SDK declares stay declared and unanswered
//! here — [`ParserRegistration::unanswered`], not a guess:
//!
//! - The transitions one OpenCode execution produces travel with the parser's
//!   own execution result, which is how this Agent has always reported them: the
//!   `serve` driver carries the transition list [`crate::parser::message`] built
//!   for the completed message document, so the shared query has no second
//!   answer to give.
//! - The Subagent mesh never dispatches OpenCode, so it holds no durable
//!   dispatch identity for this Agent to validate.
//!
//! The adapter id and framing are this Agent's declaration, and the framing is
//! the same string its replay corpus records, so a transcript cannot pass
//! against another channel.

use licoup_agent_adapter_sdk::port::{AdapterParserSet, ParserRegistration};
use licoup_agent_adapter_sdk::{adapters::AdapterContract, registry};

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "opencode";

/// The framing its parser really speaks, and the channel its corpus records.
pub const FRAMING: &str = "http-sse";

/// The wire format this package's protocol is published as.
///
/// It names the `opencode serve` HTTP and SSE protocol the parser reads, which
/// is the inbound format the release declaration's converter declares — a
/// different fact from [`FRAMING`], which is the channel one recorded transcript
/// is filed under.
pub const PROTOCOL_FORMAT: &str = "opencode.serve-http.v1";

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

/// The parser set this package composes: one Agent parser, and the replay arm
/// this package builds exactly as its own protocol does.
///
/// It is the set the package's own corpus check. The host composes its thirteen
/// parsers in its own order and reaches this package's entry through
/// [`REGISTRATION`]; this set exists so the package can prove its parser and its
/// recorded transcripts against each other without a host.
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
