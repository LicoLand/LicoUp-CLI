//! The workflow definition and run vocabulary.
//!
//! The definition grammar, run status and every shared record is the generic
//! engine vocabulary from `acativa::workflow::ir`, re-exported here so this
//! crate's callers keep the established `licoup_workflow::ir::` paths. What
//! stays LicoUp-owned is the persisted callback-decision spelling: the
//! Assistant-facing [`CallbackDecisionKind`] names the three lawful answers
//! (`advance`/`return`/`terminate`) a master agent may give, and the machine
//! translates them onto the engine's positional decision form internally.

use serde::{Deserialize, Serialize};

pub use acativa::workflow::ir::*;

/// The master agent's decision for one pending callback: enter the declared
/// next node, return to the completed node, or terminate the run.
///
/// This is the persisted event spelling (`strategy_run_events.event_json`
/// stores `"advance"`/`"return"`/`"terminate"`); the generic engine's
/// positional `acativa::CallbackDecision` never crosses this boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallbackDecisionKind {
    Advance,
    Return,
    Terminate,
}
