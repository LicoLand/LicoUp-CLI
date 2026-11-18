//! Exact resume and explicit rehydrate stay distinct. No silent fallback.

use super::{NativeWorkContextFailure, NativeWorkContextKey, unsupported_capability};

pub(super) fn exact_resume(_: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
    Err(unsupported_capability())
}

pub(super) fn rehydrate(_: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
    Err(unsupported_capability())
}
