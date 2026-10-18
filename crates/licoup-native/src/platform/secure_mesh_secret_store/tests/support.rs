use crate::core::secure_mesh_capability::{CapabilityFactState, SecurityCapability};

use super::super::capability::{
    LinuxSecretServiceProbeSnapshot, linux_secret_service_capability_facts_from_snapshot,
};

pub(super) fn unlocked_linux_probe_fixture() -> LinuxSecretServiceProbeSnapshot {
    LinuxSecretServiceProbeSnapshot {
        schema_version: 1,
        interaction: "noninteractive",
        api: "available",
        session: "established",
        default_collection: "available",
        collection: "unlocked",
        prompt: "not_required",
        read: "unverified",
        write: "unverified",
        delete: "unverified",
        service: "stable",
        ordinary_file_persistence: "unverified",
    }
}

pub(super) fn assert_linux_probe_unavailable(
    snapshot: LinuxSecretServiceProbeSnapshot,
    state: CapabilityFactState,
    reason_code: &str,
) {
    let facts = linux_secret_service_capability_facts_from_snapshot(&snapshot).unwrap();
    assert_eq!(facts.len(), 2);
    assert!(facts.iter().all(|fact| fact.state == state));
    assert!(
        facts
            .iter()
            .all(|fact| fact.reason_code.as_deref() == Some(reason_code))
    );
    assert!(
        facts
            .iter()
            .any(|fact| { fact.capability == SecurityCapability::OsSecureStore })
    );
    assert!(
        facts
            .iter()
            .any(|fact| { fact.capability == SecurityCapability::LinuxSecretService })
    );
    assert!(
        !facts
            .iter()
            .any(|fact| { fact.capability == SecurityCapability::SoftwareBacked })
    );
}
