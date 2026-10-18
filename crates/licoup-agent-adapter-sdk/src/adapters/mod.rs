//! The shared adapter declaration and the byte-line ingress contract.
//!
//! Every Agent parser implements [`NativeLineParser`] and reports one
//! [`AdapterContract`]. Neither names an Agent: which parsers exist, and which
//! one an adapter id selects, is the set composition injects through
//! [`crate::port::AdapterParserSet`].

pub mod driver_registry;
pub use driver_registry::{
    registry_get, registry_insert, registry_insert_if_absent, registry_remove, registry_remove_if,
};

/// Shared byte-line ingress contract for native protocols. Implementations
/// classify vendor frames and report facts; the conversation layer remains the
/// sole authority that settles a turn.
pub trait NativeLineParser {
    type Report;
    type Error;

    fn parse_line(&mut self, line: &[u8]) -> Result<Self::Report, Self::Error>;
}

/// The L4 facts every adapter reports. A protocol that cannot report one of
/// these is not a packaged adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolSignalKind {
    ProtocolFinish,
    Eof,
    CancelConfirmed,
}

/// One Agent parser's declaration: how it frames bytes and what it reports.
///
/// The fields are `pub` because the per-Agent parsers that build a contract are
/// one crate away, and `AdapterContract::new` is `const` so a parser can
/// declare its contract as a constant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterContract {
    pub id: &'static str,
    pub framing: &'static str,
    pub reported_signals: [ProtocolSignalKind; 3],
    pub settles_turn: bool,
    pub has_implicit_turn_timeout: bool,
    pub emits_all_content: bool,
}

impl AdapterContract {
    pub const fn new(id: &'static str, framing: &'static str) -> Self {
        Self {
            id,
            framing,
            reported_signals: [
                ProtocolSignalKind::ProtocolFinish,
                ProtocolSignalKind::Eof,
                ProtocolSignalKind::CancelConfirmed,
            ],
            settles_turn: false,
            has_implicit_turn_timeout: false,
            emits_all_content: true,
        }
    }

    /// The declaration as the packaged inventory document reports it.
    ///
    /// It is an ordinary projection, not a test-only seam: the agreement it
    /// feeds — one declaration per packaged adapter, with the id the dispatch
    /// enum names — is asserted where the declarations and the dispatch enum
    /// are both in view, which is composition rather than this crate.
    pub fn inventory_json(self) -> serde_json::Value {
        serde_json::json!({
            "adapterId": self.id,
            "framing": self.framing,
            "settlesTurn": self.settles_turn,
            "hasImplicitTurnTimeout": self.has_implicit_turn_timeout,
            "emitsAllContent": self.emits_all_content,
        })
    }
}
