//! The replay arm lookup over the composed Agent parsers.
//!
//! An arm is the only place that knows how to construct one Agent's real
//! parser. The arms are composed above this crate — each Agent's arm lives next
//! to the code it replays — and arrive through [`AdapterParserSet`], so this
//! module is the lookup and it names no Agent.

use super::FrameReplay;
use crate::port::AdapterParserSet;

/// The framing a registered adapter really speaks. A fixture whose recorded
/// channel disagrees with this is not a transcript of that adapter.
pub fn contract_framing(set: &AdapterParserSet, adapter_id: &str) -> Result<&'static str, String> {
    set.framing(adapter_id)
}

/// Build the replay arm for a registered adapter id.
pub fn replay_for(
    set: &AdapterParserSet,
    adapter_id: &str,
) -> Result<Box<dyn FrameReplay>, String> {
    set.replay_for(adapter_id)
}
