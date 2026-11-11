//! Local-only skill invocation metering.
//!
//! The ledger is fed by two postures:
//! - Runtime live events: the conversation adapter supplies privacy-minimal
//!   `skill.invoked` events; counts are accepted only for approved agent
//!   pairings and well-formed local skill identifiers reported by that agent.
//! - History backfill: an incremental scanner projects the same skill-call
//!   semantics from locally discovered native transcripts and records
//!   aggregate counts for any locally discovered agent and any well-formed
//!   sanitized skill id, matching the agent-usage token scanner posture.
//!
//! Both postures store only one count per UTC day, agent, and skill. This
//! feature accepts no prompt, tool arguments, paths, or tool results.

use super::{ClientStateStore, Result, Value};
use time::OffsetDateTime;

mod backfill;
mod invocation;
mod ledger;
mod report;
#[cfg(test)]
mod tests;

pub(super) fn observe_conversation_result(
    store: &ClientStateStore,
    agent_id: &str,
    result: &Value,
) -> Result<Value> {
    observe_at(store, agent_id, result, OffsetDateTime::now_utc())
}

pub(super) fn report(store: &ClientStateStore, params: &Value) -> Result<Value> {
    report::report(store, params)
}

pub(super) fn scan(store: &ClientStateStore, params: &Value) -> Result<Value> {
    backfill::scan(store, params)
}

fn observe_at(
    store: &ClientStateStore,
    agent_id: &str,
    result: &Value,
    occurred_at: OffsetDateTime,
) -> Result<Value> {
    let counts = invocation::invocation_counts(result);
    ledger::record_counts(
        store,
        agent_id,
        counts,
        occurred_at,
        ledger::RecordSource::Runtime,
    )
}
