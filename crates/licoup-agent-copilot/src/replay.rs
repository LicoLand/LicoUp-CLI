//! Replay arm for the `copilot` adapter.
//!
//! The arm drives the *shared* ACP reducer — the same `AcpProtocol` a live
//! Copilot turn drives — and the reducer reads Copilot's frame policy through
//! the installed dialect port, exactly as the production transport does. So a
//! projection here is the shared engine's real report over this Agent's real
//! parser, never a re-derivation of the payload, and the arm can only fail when
//! this Agent's dialect or the shared reducer regresses.
//!
//! The arm travels with the dialect. A composing test build reaches it through
//! [`crate::registration::parser_set`]; the host's own corpus reaches it the
//! same way.
//!
//! The arm receives this package's own dialect directly, so another test's
//! process-wide registration cannot change the parser this corpus exercises.

use licoup_agent_adapter_sdk::replay::FrameReplay;
use licoup_agent_drivers::acp_driver_runtime::replay;

use crate::registration::{ADAPTER_ID, DIALECT};

/// Build the replay arm of this package's parser.
///
/// An adapter this package does not carry is refused rather than defaulted, so a
/// fixture can never pass against a parser that was never constructed.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != ADAPTER_ID {
        return Err(format!(
            "no replayable parser is registered for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(replay::Replay::with_dialect(DIALECT)))
}
