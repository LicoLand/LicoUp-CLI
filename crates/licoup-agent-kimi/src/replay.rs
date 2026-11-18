//! The recorded-transcript replay arm for Kimi Code.
//!
//! The arm constructs the same shared ACP reducer the production driver
//! constructs, keyed by the same driver identity, so a transcript can only pass
//! if Kimi's real dialect still reports the recorded facts. The projection, the
//! corpus resolution and the fail-closed properties stay in the SDK's harness;
//! this module is the one thing the harness cannot know — how to build *this*
//! Agent's arm.
//!
//! It is behind `test-support` rather than `cfg(test)` because the host's
//! replay suite drives it, and the host links this crate as a dependency:
//! `cfg(test)` is false for a dependency, so a `cfg(test)` arm is compiled out
//! of exactly the build that calls it.

use licoup_agent_adapter_sdk::replay::FrameReplay;

use crate::dialect;

/// Build the replay arm for this package's adapter.
///
/// An adapter id this package does not carry is refused rather than defaulted,
/// so a transcript can never pass against a parser that was never constructed.
/// The dialect is this package's own registration and is handed to the shared
/// reducer directly: the arm reads the same Kimi dialect the production
/// transport resolves from the installed port, and it cannot drift from it.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != crate::registration::ADAPTER_ID {
        return Err(format!(
            "no replayable parser is registered for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(
        licoup_agent_drivers::acp_driver_runtime::replay::Replay::with_dialect(
            dialect::registration(),
        ),
    ))
}
