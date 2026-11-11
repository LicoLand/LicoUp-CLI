//! Activity records remain in the same explicitly selected local client-state root.

use crate::platform::client_state::ActivityLog;
use serde_json::Value;

pub(super) fn record_activity(log: &ActivityLog, event_type: &str, payload: Value) {
    let _ = log.append(event_type, payload);
}
