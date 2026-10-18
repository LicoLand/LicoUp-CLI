use anyhow::Result;
use serde::Serialize;

use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, CapabilityFact, CapabilityFactState, SecurityCapability,
};

#[cfg(target_os = "linux")]
use super::linux_secret_service;
use super::platform_backends;

pub fn platform_native_secret_store_backend() -> &'static str {
    platform_backends::backend()
}

pub fn platform_native_secret_store_supported() -> bool {
    platform_native_secret_store_runtime_state() == PlatformSecretStoreRuntimeState::Available
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformSecretStoreRuntimeState {
    Available,
    Locked,
    Unverified,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxSecretServiceProbeSnapshot {
    pub schema_version: u32,
    pub interaction: &'static str,
    pub api: &'static str,
    pub session: &'static str,
    pub default_collection: &'static str,
    pub collection: &'static str,
    pub prompt: &'static str,
    pub read: &'static str,
    pub write: &'static str,
    pub delete: &'static str,
    pub service: &'static str,
    pub ordinary_file_persistence: &'static str,
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_secret_service_probe_snapshot_with_evidence(
    mut snapshot: LinuxSecretServiceProbeSnapshot,
    io_round_trip_verified: bool,
    ordinary_file_persistence_absent: bool,
) -> LinuxSecretServiceProbeSnapshot {
    if io_round_trip_verified
        && snapshot.api == "available"
        && snapshot.session == "established"
        && snapshot.default_collection == "available"
        && snapshot.collection == "unlocked"
        && snapshot.prompt == "not_required"
        && snapshot.service == "stable"
    {
        snapshot.read = "supported";
        snapshot.write = "supported";
        snapshot.delete = "supported";
    }
    snapshot.ordinary_file_persistence = if ordinary_file_persistence_absent {
        "absent"
    } else {
        "detected"
    };
    snapshot
}

pub fn platform_linux_secret_service_probe_snapshot(
    io_round_trip_verified: bool,
    ordinary_file_persistence_absent: bool,
) -> Option<LinuxSecretServiceProbeSnapshot> {
    #[cfg(target_os = "linux")]
    {
        Some(linux_secret_service_probe_snapshot_with_evidence(
            linux_secret_service::snapshot(),
            io_round_trip_verified,
            ordinary_file_persistence_absent,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (io_round_trip_verified, ordinary_file_persistence_absent);
        None
    }
}

#[cfg(target_os = "linux")]
pub(super) fn record_platform_secret_store_runtime_failure() {
    linux_secret_service::record_runtime_operation_failure();
}

pub fn platform_native_secret_store_runtime_state() -> PlatformSecretStoreRuntimeState {
    #[cfg(target_os = "linux")]
    {
        return linux_secret_service::runtime_state();
    }
    #[cfg(target_os = "macos")]
    {
        // Unit tests must never touch the real Keychain.
        if cfg!(test) {
            return PlatformSecretStoreRuntimeState::Unverified;
        }
        // Bounded runtime evidence for the selected native Keychain backend:
        // one silent synthetic add/read/delete round trip, cached for the
        // process lifetime.
        use std::sync::OnceLock;
        static PROBE_RESULT: OnceLock<bool> = OnceLock::new();
        let probed = *PROBE_RESULT
            .get_or_init(super::macos_user_presence::adaptive_keychain_roundtrip_probe);
        if probed {
            PlatformSecretStoreRuntimeState::Available
        } else {
            PlatformSecretStoreRuntimeState::Unavailable
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Presence of an OS API is not evidence that the current process can
        // create, read, and delete a protected record under the required user
        // authorization policy. Until a bounded runtime proof is wired to the
        // selected adapter, persistent custody must stay disabled.
        PlatformSecretStoreRuntimeState::Unverified
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        PlatformSecretStoreRuntimeState::Unavailable
    }
}

pub(super) fn platform_secret_store_capability_facts() -> Result<Vec<CapabilityFact>> {
    #[cfg(target_os = "linux")]
    {
        let snapshot = linux_secret_service::snapshot();
        return linux_secret_service_capability_facts_from_snapshot(&snapshot);
    }
    #[cfg(target_os = "macos")]
    let platform_capability = Some(SecurityCapability::AppleKeychain);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let platform_capability = None;
    #[cfg(not(target_os = "linux"))]
    capability_facts_for_runtime_state(
        platform_native_secret_store_runtime_state(),
        platform_capability,
    )
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_secret_service_capability_facts_from_snapshot(
    snapshot: &LinuxSecretServiceProbeSnapshot,
) -> Result<Vec<CapabilityFact>> {
    let unavailable = if snapshot.api == "absent" {
        Some((
            CapabilityFactState::Unsupported,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_api_absent",
        ))
    } else if snapshot.session == "failed" {
        Some((
            CapabilityFactState::TemporarilyUnavailable,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_session_failed",
        ))
    } else if snapshot.default_collection == "absent" {
        Some((
            CapabilityFactState::Unsupported,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_default_collection_absent",
        ))
    } else if snapshot.collection == "locked" {
        Some((
            CapabilityFactState::TemporarilyUnavailable,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_collection_locked",
        ))
    } else if snapshot.prompt == "required" {
        Some((
            CapabilityFactState::TemporarilyUnavailable,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_prompt_required",
        ))
    } else if snapshot.service == "disappeared" {
        Some((
            CapabilityFactState::TemporarilyUnavailable,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_disappeared",
        ))
    } else if snapshot.service == "temporarily_unavailable" {
        Some((
            CapabilityFactState::TemporarilyUnavailable,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_temporarily_unavailable",
        ))
    } else if snapshot.ordinary_file_persistence == "detected" {
        Some((
            CapabilityFactState::Unsupported,
            CapabilityEvidenceKind::RuntimeOperation,
            "linux_secret_service_ordinary_file_persistence_detected",
        ))
    } else if snapshot.api == "available"
        && snapshot.session == "established"
        && snapshot.default_collection == "available"
        && snapshot.collection == "unlocked"
        && snapshot.prompt == "not_required"
        && snapshot.read == "supported"
        && snapshot.write == "supported"
        && snapshot.delete == "supported"
        && snapshot.service == "stable"
        && snapshot.ordinary_file_persistence == "absent"
    {
        None
    } else if snapshot.api == "available"
        && snapshot.session == "established"
        && snapshot.default_collection == "available"
        && snapshot.collection == "unlocked"
        && snapshot.prompt == "not_required"
        && snapshot.service == "stable"
    {
        Some((
            CapabilityFactState::Unverified,
            CapabilityEvidenceKind::NotMeasured,
            "linux_secret_service_io_round_trip_unverified",
        ))
    } else {
        Some((
            CapabilityFactState::Unverified,
            CapabilityEvidenceKind::NotMeasured,
            "linux_secret_service_probe_incomplete",
        ))
    };
    let capabilities = [
        SecurityCapability::OsSecureStore,
        SecurityCapability::LinuxSecretService,
    ];
    let Some((state, evidence_kind, reason_code)) = unavailable else {
        let evidence_kind = CapabilityEvidenceKind::RuntimeOperation;
        return Ok(capabilities
            .into_iter()
            .map(|capability| CapabilityFact::supported(capability, evidence_kind))
            .collect());
    };
    capabilities
        .into_iter()
        .map(|capability| {
            CapabilityFact::unavailable(capability, state, evidence_kind, reason_code)
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn capability_facts_for_runtime_state(
    state: PlatformSecretStoreRuntimeState,
    platform_capability: Option<SecurityCapability>,
) -> Result<Vec<CapabilityFact>> {
    let mut capabilities = vec![SecurityCapability::OsSecureStore];
    if let Some(platform_capability) = platform_capability {
        capabilities.push(platform_capability);
    }
    match state {
        PlatformSecretStoreRuntimeState::Available => Ok(capabilities
            .into_iter()
            .map(|capability| {
                CapabilityFact::supported(capability, CapabilityEvidenceKind::RuntimeOperation)
            })
            .collect()),
        PlatformSecretStoreRuntimeState::Locked => capabilities
            .into_iter()
            .map(|capability| {
                CapabilityFact::unavailable(
                    capability,
                    CapabilityFactState::TemporarilyUnavailable,
                    CapabilityEvidenceKind::RuntimeOperation,
                    "platform_secret_store_locked",
                )
            })
            .collect(),
        PlatformSecretStoreRuntimeState::Unverified => capabilities
            .into_iter()
            .map(|capability| {
                CapabilityFact::unavailable(
                    capability,
                    CapabilityFactState::Unverified,
                    CapabilityEvidenceKind::NotMeasured,
                    "platform_secret_store_runtime_operation_unverified",
                )
            })
            .collect(),
        PlatformSecretStoreRuntimeState::Unavailable => capabilities
            .into_iter()
            .map(|capability| {
                CapabilityFact::unavailable(
                    capability,
                    CapabilityFactState::TemporarilyUnavailable,
                    CapabilityEvidenceKind::RuntimeOperation,
                    "platform_secret_store_unavailable",
                )
            })
            .collect(),
    }
}
