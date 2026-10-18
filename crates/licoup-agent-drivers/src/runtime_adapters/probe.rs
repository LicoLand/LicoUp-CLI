use super::adapter::adapter_for_agent;
use serde_json::{Value, json};
use std::path::Path;

/// Probes only the official fixed-argument entrypoint and emits redacted
/// booleans. No command output, paths, account data, or runtime content escapes.
///
/// What each Agent's probe *is* belongs to that Agent, so the thirteen probe
/// arms live in the composition and arrive here through
/// [`super::port::AgentDriverRegistration::probe`]. This function owns the
/// admission around them: an inventory id no packaged adapter answers for is
/// `unknown_adapter`, and an adapter this host composed no driver for is
/// reported unavailable rather than inherited from a neighbour.
pub fn probe_runtime_driver(target: &str, executable: &Path, cwd: &Path) -> Value {
    let executable = executable.to_string_lossy();
    let Some(adapter) = adapter_for_agent(target) else {
        return json!({"available": false, "supported": false, "errorCode": "unknown_adapter"});
    };
    let Some(registration) = super::port::registration_for_adapter(adapter) else {
        return json!({"available": false, "supported": false, "errorCode": "runtime_not_detected"});
    };
    (registration.probe)(executable.as_ref(), cwd)
}
