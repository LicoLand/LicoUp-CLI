use crate::platform::client_state::ClientStateStore;

use super::super::model::LocalAssemblyRecord;
use super::probe::ProbeIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::collaboration_plugin::assembly) enum RuntimeIdentity {
    Owned,
    Mismatched,
    Unavailable,
}

pub(in crate::domain::collaboration_plugin::assembly) fn runtime_identity(
    store: &ClientStateStore,
    record: &LocalAssemblyRecord,
) -> RuntimeIdentity {
    let Some(pid) = record.runtime_pid else {
        return RuntimeIdentity::Mismatched;
    };
    let Some(expected_process_identity) = record.runtime_process_identity.as_deref() else {
        return RuntimeIdentity::Mismatched;
    };
    match super::process::capture_identity(pid) {
        Ok(actual) if actual == expected_process_identity => {}
        Ok(_) => return RuntimeIdentity::Mismatched,
        Err(_) => return RuntimeIdentity::Unavailable,
    }
    runtime_identity_after_process_binding(store, record)
}

pub(super) fn runtime_identity_after_process_binding(
    store: &ClientStateStore,
    record: &LocalAssemblyRecord,
) -> RuntimeIdentity {
    let endpoint = super::probe::endpoint_identity(record);
    let lease_held = match super::lease::is_held(store, &record.deployment_id) {
        Ok(value) => value,
        Err(_) => return RuntimeIdentity::Unavailable,
    };
    match (endpoint, lease_held) {
        (ProbeIdentity::Owned, true) => RuntimeIdentity::Owned,
        (ProbeIdentity::Unavailable, true) => RuntimeIdentity::Unavailable,
        (ProbeIdentity::Owned | ProbeIdentity::Unavailable, false)
        | (ProbeIdentity::Mismatched, _) => RuntimeIdentity::Mismatched,
    }
}
