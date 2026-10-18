use super::super::{SecureMeshTransparencyLeafBody, directory_scope_commitment};

pub(super) fn leaf(
    endpoint_id: &str,
    rotation_epoch: u64,
    directory_state: &str,
) -> SecureMeshTransparencyLeafBody {
    SecureMeshTransparencyLeafBody {
        directory_scope_commitment: directory_scope_commitment(
            "tenant-a",
            "account-a",
            "workspace-a",
        ),
        endpoint_id: endpoint_id.to_string(),
        endpoint_kind: "test".to_string(),
        identity_public_key: format!("{endpoint_id}-identity"),
        signing_public_key: format!("{endpoint_id}-signing"),
        fingerprint: format!("{endpoint_id}-fingerprint"),
        rotation_epoch,
        directory_state: directory_state.to_string(),
        updated_at: "2026-07-12T00:00:00Z".to_string(),
    }
}

pub(super) fn state_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "lico-kt-{label}-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
