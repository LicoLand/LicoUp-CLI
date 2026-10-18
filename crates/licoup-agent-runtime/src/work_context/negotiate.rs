//! Capability negotiation. Dimensions stay independent; no inferred support.

use super::{
    NativeCapabilitySnapshot, NativeWorkContextFailure, NativeWorkContextKey,
    unsupported_capability,
};

pub(super) fn negotiate(
    _: &NativeWorkContextKey,
) -> Result<NativeCapabilitySnapshot, NativeWorkContextFailure> {
    Err(unsupported_capability())
}

pub fn dimensions_are_independent(snapshot: &NativeCapabilitySnapshot) -> bool {
    let fields = [
        snapshot.exact_resume,
        snapshot.fork,
        snapshot.compact,
        snapshot.steer,
        snapshot.cancel,
        snapshot.tools,
        snapshot.isolated_context,
        snapshot.parallel_contexts,
    ];
    fields.len() == 8
}
