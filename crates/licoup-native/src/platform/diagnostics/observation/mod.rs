//! Segment observation and correlation-id port (contract C07, module M23).
//!
//! This leaf installs the port beside the existing diagnostic owners; it does
//! not create a second history store, log sink, or correlation carrier:
//!
//! * correlation ids are the values other owners already carry, and the trace
//!   sink they land in is the existing `log`/`env_logger` sink
//!   ([`backend::LogObservationTelemetryBackend`]);
//! * nothing here persists, uploads, or reaches a network;
//! * a [`probe::ObservationProbe`] is bounded and can be switched off, and an
//!   off probe reads no clock and touches no buffer.
//!
//! Invariants that callers may rely on:
//!
//! 1. Recording never blocks business code. [`probe::ObservationProbe::submit`]
//!    only tries to push onto a preallocated bounded buffer; contention drops
//!    with a counted reason instead of waiting. The replaceable backend runs from
//!    [`probe::ObservationProbe::drain`], which the telemetry owner calls.
//! 2. Nothing is lost silently. Every refused record increments a counted drop
//!    reason ([`probe::ObservationDropCounts`]).
//! 3. Metrics carry controlled dimensions only. High-cardinality ids live on
//!    [`segment::ObservationSegmentRecord`], which is trace/log data; the crate
//!    exposes no metric type that accepts a correlation id.
//! 4. Waiting and work are separate segments ([`segment::ObservationSegmentKind`]),
//!    and parallel predecessors or asynchronous queues attach
//!    [`segment::ObservationSpanLink`] values instead of faking a synchronous
//!    call stack.

pub mod backend;
pub mod correlation;
pub mod privacy;
pub mod probe;
pub mod segment;

#[cfg(test)]
mod tests;
