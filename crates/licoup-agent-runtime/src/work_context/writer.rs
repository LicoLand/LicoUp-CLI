//! Per-session single writer. Unavailable must not report writer_busy.

use super::{NativeWorkContextFailure, NativeWorkContextKey, unsupported_capability};

pub(super) fn claim_writer(_: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
    Err(unsupported_capability())
}
