//! Fork, compact, steer, and cancel. Unknown protocol effects are not success.

use super::types::NativeControlRequest;
use super::{NativeWorkContextFailure, NativeWorkContextKey, unsupported_capability};

pub(super) fn fork(_: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
    Err(unsupported_capability())
}

pub(super) fn compact(_: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
    Err(unsupported_capability())
}

pub(super) fn steer(_: &NativeControlRequest) -> Result<(), NativeWorkContextFailure> {
    Err(unsupported_capability())
}

pub(super) fn cancel(_: &NativeControlRequest) -> Result<(), NativeWorkContextFailure> {
    Err(unsupported_capability())
}
