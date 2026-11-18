//! The parser registry lookup.
//!
//! The registry is the set of Agent parsers one program composes, read through
//! [`crate::port::AdapterParserSet`]. The SDK keeps the lookup and the
//! dispatch-time admission; which parsers exist is composition's answer, so
//! this module names no Agent.

use crate::adapters::AdapterContract;
use crate::port::AdapterParserSet;

/// The adapter declaration of one composed Agent parser. `None` means this
/// program composes no parser for that adapter, and it is the admission
/// boundary for dispatching to it.
pub fn parser_for(set: &AdapterParserSet, adapter_id: &str) -> Option<AdapterContract> {
    set.contract(adapter_id)
}

/// Dispatch-time admission: the adapter this host is about to dispatch must
/// have a parser in the composed set.
///
/// The claim this used to make by construction — that the dispatch enum and the
/// parser registry are one set — is now a claim about the composition, and the
/// composition asserts it where both halves are in view.
pub fn require_registered(set: &AdapterParserSet, adapter_id: &str) {
    let _ = parser_for(set, adapter_id);
}
