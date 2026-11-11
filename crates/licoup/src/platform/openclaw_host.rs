//! The client's answer for the OpenClaw adapter package's ports.
//!
//! The package owns what one OpenClaw turn *is* — the Gateway ACP frames, the
//! attach it names and the events that attach produces. This module owns where
//! its effects go, because the client owns the Gateway lifecycle and the event
//! consumer. Both halves meet exactly here:
//!
//! - [`gateway_port`] is the package's [`GatewayPort`] answered from the
//!   client's own Gateway engine ([`super::openclaw_gateway`]) — the same
//!   reviewed process, port, state and bounded-HTTP primitives that start,
//!   reuse, health-check and stop the Gateway, and the same state document the
//!   client's status route reads. The package supplies the endpoint policy
//!   (its `policy` module owns the vendor-default and preferred ports and the
//!   attach-mode vocabulary); the engine supplies the socket and the process.
//! - [`turn_event_port`] is the package's turn-event emission answered from this
//!   host's own emitters, so an OpenClaw event and a Codex event reach the same
//!   reader through the same path.
//!
//! Nothing here names a vendor field: the engine is reached through the
//! package's own endpoint model, and the failure code it reports is the one the
//! package asked for.

use licoup_agent_openclaw::port::gateway::GatewayPort;
use licoup_agent_openclaw::port::turn_event::TurnEventPort;

use super::openclaw_gateway;
use super::turn_event_emit;

/// This host's answer for the package's Gateway lifecycle port.
pub(crate) fn gateway_port() -> GatewayPort {
    GatewayPort {
        // The engine's failure vocabulary is its own shape and the package maps
        // it onto its typed driver failure, so the code crosses as its own
        // string rather than as a re-worded message.
        ensure_attach_endpoint: |executable| {
            openclaw_gateway::ensure_attach_endpoint(executable).map_err(|error| error.to_string())
        },
    }
}

/// This host's answer for the package's turn-event port.
///
/// The package owns *what* one OpenClaw turn emits as its Gateway frames arrive;
/// this host owns *where* they go, because the host owns the consumer. The
/// answer is the same emitters the host's own drivers and the Codex package
/// reach, so an OpenClaw message chunk and a Codex one arrive at one reader
/// through one path rather than two sinks that can drift.
pub(crate) fn turn_event_port() -> TurnEventPort {
    TurnEventPort {
        emit_turn_event: turn_event_emit::emit_turn_event,
        emit_agent_message_chunk: turn_event_emit::emit_agent_message_chunk,
        emit_agent_processing: turn_event_emit::emit_agent_processing,
    }
}

#[cfg(test)]
mod tests;
