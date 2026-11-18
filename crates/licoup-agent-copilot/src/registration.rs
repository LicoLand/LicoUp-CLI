//! The registration this package publishes to the adapter SDK's ports, and the
//! frame dialect it publishes to the shared ACP engine's.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`REGISTRATION`] and hands it to the SDK's registry, replay harness and host
//! queries, so adding this Agent's crate is one entry rather than four lists
//! that can drift.
//!
//! The shared ACP engine owns its own port
//! ([`licoup_agent_drivers::AcpParserRegistration`]); this package owns the
//! answer there too, because it owns the frame policy. Composition installs
//! [`DIALECT`] by driver identity, so the transport reads Copilot's frames
//! without naming Copilot.
//!
//! Copilot answers both protocol-agnostic SDK queries fail-closed, exactly as
//! it did as a host-held parser: its driver carries the transitions its own
//! reducer built, and the Subagent mesh never dispatches it, so there is no
//! durable dispatch identity for this Agent to validate. Declaring that here is
//! the honest answer; answering with a neighbouring Agent's facts would not be.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
#[cfg(any(test, feature = "test-support"))]
use licoup_agent_adapter_sdk::port::AdapterParserSet;
use licoup_agent_adapter_sdk::port::ParserRegistration;
use licoup_agent_drivers::AcpParserRegistration;

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = "copilot";

/// The framing its parser really speaks, and the channel its fixtures record.
pub const FRAMING: &str = "lf-ndjson-acp";

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: AdapterContract = parser::CONTRACT;

/// This Agent's frame dialect, as the shared ACP engine reads it.
pub const DIALECT: AcpParserRegistration = crate::dialect::DIALECT;

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
///
/// It is declared only where the shared ACP reducer's replay surface is
/// compiled, because the arm drives that reducer. A production build of this
/// package reads its Agent's frames through the engine and carries no arm; the
/// host composes the arms its own corpus runs.
#[cfg(any(test, feature = "test-support"))]
pub const fn parser_set() -> AdapterParserSet {
    AdapterParserSet {
        registrations,
        replay: crate::replay::replay_arm,
    }
}

/// The adapter declaration of this package's parser, read through the SDK's own
/// registry lookup rather than from the constant, so the lookup and the set
/// cannot disagree.
#[cfg(any(test, feature = "test-support"))]
pub fn contract() -> Option<AdapterContract> {
    licoup_agent_adapter_sdk::registry::parser_for(&parser_set(), ADAPTER_ID)
}
