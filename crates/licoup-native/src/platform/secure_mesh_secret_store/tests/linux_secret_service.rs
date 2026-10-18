use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, CapabilityFactState, SecurityCapability,
};

use super::super::PlatformSecretStoreRuntimeState;
#[cfg(target_os = "linux")]
use super::super::capability::record_platform_secret_store_runtime_failure;
use super::super::capability::{
    linux_secret_service_capability_facts_from_snapshot,
    linux_secret_service_probe_snapshot_with_evidence,
};
#[cfg(target_os = "linux")]
use super::super::linux_secret_service;
use super::super::linux_secret_service::RuntimeFailureMarker;
use super::super::linux_secret_service::runtime_state_from_snapshot;
use super::support::{assert_linux_probe_unavailable, unlocked_linux_probe_fixture};

#[test]
fn linux_probe_api_missing_is_an_independent_unsupported_fact() {
    let mut snapshot = unlocked_linux_probe_fixture();
    snapshot.api = "absent";
    assert_linux_probe_unavailable(
        snapshot,
        CapabilityFactState::Unsupported,
        "linux_secret_service_api_absent",
    );
}

#[test]
fn linux_probe_session_failure_is_independently_temporarily_unavailable() {
    let mut snapshot = unlocked_linux_probe_fixture();
    snapshot.session = "failed";
    assert_linux_probe_unavailable(
        snapshot,
        CapabilityFactState::TemporarilyUnavailable,
        "linux_secret_service_session_failed",
    );
}

#[test]
fn linux_probe_default_collection_missing_is_independently_unsupported() {
    let mut snapshot = unlocked_linux_probe_fixture();
    snapshot.default_collection = "absent";
    assert_linux_probe_unavailable(
        snapshot,
        CapabilityFactState::Unsupported,
        "linux_secret_service_default_collection_absent",
    );
}

#[test]
fn linux_probe_locked_collection_is_independently_temporarily_unavailable() {
    let mut snapshot = unlocked_linux_probe_fixture();
    snapshot.collection = "locked";
    snapshot.prompt = "not_attempted";
    assert_linux_probe_unavailable(
        snapshot,
        CapabilityFactState::TemporarilyUnavailable,
        "linux_secret_service_collection_locked",
    );
}

#[test]
fn linux_probe_prompt_required_is_independently_temporarily_unavailable() {
    let mut snapshot = unlocked_linux_probe_fixture();
    snapshot.prompt = "required";
    assert_linux_probe_unavailable(
        snapshot,
        CapabilityFactState::TemporarilyUnavailable,
        "linux_secret_service_prompt_required",
    );
}

#[test]
fn linux_probe_unlocked_crud_enables_only_exact_os_store_capabilities() {
    let snapshot = linux_secret_service_probe_snapshot_with_evidence(
        unlocked_linux_probe_fixture(),
        true,
        true,
    );
    assert_eq!(snapshot.read, "supported");
    assert_eq!(snapshot.write, "supported");
    assert_eq!(snapshot.delete, "supported");
    assert_eq!(snapshot.ordinary_file_persistence, "absent");
    let facts = linux_secret_service_capability_facts_from_snapshot(&snapshot).unwrap();
    assert_eq!(facts.len(), 2);
    assert!(
        facts
            .iter()
            .all(|fact| fact.state == CapabilityFactState::Supported)
    );
    assert!(
        !facts
            .iter()
            .any(|fact| { fact.capability == SecurityCapability::SoftwareBacked })
    );
}

#[test]
fn linux_probe_never_enables_persistent_custody_without_crud_and_absence_evidence() {
    let snapshot = unlocked_linux_probe_fixture();
    let facts = linux_secret_service_capability_facts_from_snapshot(&snapshot).unwrap();
    assert!(facts.iter().all(|fact| {
        fact.state == CapabilityFactState::Unverified
            && fact.evidence_kind == CapabilityEvidenceKind::NotMeasured
            && fact.reason_code.as_deref() == Some("linux_secret_service_io_round_trip_unverified")
    }));
    assert_eq!(
        runtime_state_from_snapshot(&snapshot),
        PlatformSecretStoreRuntimeState::Unverified
    );
}

#[test]
fn linux_probe_rejects_ordinary_file_persistence_even_after_crud() {
    let snapshot = linux_secret_service_probe_snapshot_with_evidence(
        unlocked_linux_probe_fixture(),
        true,
        false,
    );
    let facts = linux_secret_service_capability_facts_from_snapshot(&snapshot).unwrap();
    assert!(facts.iter().all(|fact| {
        fact.state == CapabilityFactState::Unsupported
            && fact.reason_code.as_deref()
                == Some("linux_secret_service_ordinary_file_persistence_detected")
    }));
    assert_ne!(
        runtime_state_from_snapshot(&snapshot),
        PlatformSecretStoreRuntimeState::Available
    );
}

#[test]
fn linux_probe_running_service_disappearance_is_independently_unavailable() {
    let mut snapshot = unlocked_linux_probe_fixture();
    snapshot.service = "disappeared";
    assert_linux_probe_unavailable(
        snapshot,
        CapabilityFactState::TemporarilyUnavailable,
        "linux_secret_service_disappeared",
    );
}

#[test]
fn linux_runtime_failure_marker_is_consumed_once_before_service_recovery() {
    let marker = RuntimeFailureMarker::new();
    marker.record();
    assert!(marker.take());
    assert!(!marker.take());

    #[cfg(target_os = "linux")]
    {
        record_platform_secret_store_runtime_failure();
        assert!(linux_secret_service::take_runtime_operation_failure());
        assert!(!linux_secret_service::take_runtime_operation_failure());
    }
}
