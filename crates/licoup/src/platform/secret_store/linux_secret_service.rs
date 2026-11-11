use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(target_os = "linux", test))]
use super::capability::{LinuxSecretServiceProbeSnapshot, PlatformSecretStoreRuntimeState};

#[derive(Debug)]
pub(super) struct RuntimeFailureMarker {
    failed: AtomicBool,
}

impl RuntimeFailureMarker {
    pub(super) const fn new() -> Self {
        Self {
            failed: AtomicBool::new(false),
        }
    }

    pub(super) fn record(&self) {
        self.failed.store(true, Ordering::SeqCst);
    }

    pub(super) fn take(&self) -> bool {
        self.failed.swap(false, Ordering::SeqCst)
    }
}

#[cfg(target_os = "linux")]
use dbus_secret_service::{EncryptionType, SecretService};

#[cfg(target_os = "linux")]
static RUNTIME_OPERATION_FAILURE: RuntimeFailureMarker = RuntimeFailureMarker::new();

#[cfg(target_os = "linux")]
pub(super) fn record_runtime_operation_failure() {
    RUNTIME_OPERATION_FAILURE.record();
}

#[cfg(target_os = "linux")]
pub(super) fn take_runtime_operation_failure() -> bool {
    RUNTIME_OPERATION_FAILURE.take()
}

#[cfg(target_os = "linux")]
pub(super) fn snapshot() -> LinuxSecretServiceProbeSnapshot {
    let Ok(service) = SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0) else {
        return LinuxSecretServiceProbeSnapshot {
            schema_version: 1,
            interaction: "noninteractive",
            api: "absent",
            session: "failed",
            default_collection: "absent",
            collection: "unverified",
            prompt: "not_attempted",
            read: "unverified",
            write: "unverified",
            delete: "unverified",
            service: "temporarily_unavailable",
            ordinary_file_persistence: "unverified",
        };
    };
    let Ok(collection) = service.get_default_collection() else {
        return LinuxSecretServiceProbeSnapshot {
            schema_version: 1,
            interaction: "noninteractive",
            api: "available",
            session: "established",
            default_collection: "absent",
            collection: "unverified",
            prompt: "not_attempted",
            read: "unverified",
            write: "unverified",
            delete: "unverified",
            service: "stable",
            ordinary_file_persistence: "unverified",
        };
    };
    let collection_state = match collection.is_locked() {
        Ok(false) => "unlocked",
        Ok(true) => "locked",
        Err(_) => "unverified",
    };
    let service_state = if take_runtime_operation_failure() {
        "disappeared"
    } else {
        "stable"
    };
    LinuxSecretServiceProbeSnapshot {
        schema_version: 1,
        interaction: "noninteractive",
        api: "available",
        session: "established",
        default_collection: "available",
        collection: collection_state,
        prompt: if collection_state == "locked" {
            "required"
        } else {
            "not_required"
        },
        read: "unverified",
        write: "unverified",
        delete: "unverified",
        service: service_state,
        ordinary_file_persistence: "unverified",
    }
}

#[cfg(target_os = "linux")]
pub(super) fn runtime_state() -> PlatformSecretStoreRuntimeState {
    runtime_state_from_snapshot(&snapshot())
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn runtime_state_from_snapshot(
    snapshot: &LinuxSecretServiceProbeSnapshot,
) -> PlatformSecretStoreRuntimeState {
    if snapshot.collection == "locked" {
        PlatformSecretStoreRuntimeState::Locked
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
        PlatformSecretStoreRuntimeState::Available
    } else if snapshot.api == "available"
        && snapshot.session == "established"
        && snapshot.default_collection == "available"
        && snapshot.collection == "unlocked"
        && snapshot.service == "stable"
    {
        PlatformSecretStoreRuntimeState::Unverified
    } else {
        PlatformSecretStoreRuntimeState::Unavailable
    }
}
