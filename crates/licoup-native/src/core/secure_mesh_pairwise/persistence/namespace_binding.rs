use std::path::Path;

use uuid::Uuid;

use super::super::support::{PAIRWISE_SECRET_STORE_CLASS, append_len_prefixed_bytes, sha256_hex};

pub(crate) fn pairwise_secret_store_namespace(path: &Path) -> String {
    format!(
        "{PAIRWISE_SECRET_STORE_CLASS}:{}",
        sha256_hex(path.to_string_lossy().as_bytes())
    )
}

pub(super) fn pairwise_secret_store_key(
    session_id: &str,
    local_endpoint_id: &str,
    state_version: u64,
) -> String {
    format!(
        "{}.{}",
        pairwise_secret_store_key_prefix(session_id, local_endpoint_id, state_version),
        Uuid::new_v4().simple()
    )
}

pub(super) fn pairwise_secret_store_key_prefix(
    session_id: &str,
    local_endpoint_id: &str,
    state_version: u64,
) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(b"LCOSM-PAIRWISE-SECRET-STORE-KEY-v2");
    let _ = append_len_prefixed_bytes(&mut material, session_id.as_bytes());
    let _ = append_len_prefixed_bytes(&mut material, local_endpoint_id.as_bytes());
    material.extend_from_slice(&state_version.to_be_bytes());
    format!("snapshot.v2.{}", sha256_hex(&material))
}

pub(super) fn pairwise_secret_store_key_is_bound(
    key: &str,
    session_id: &str,
    local_endpoint_id: &str,
    state_version: u64,
) -> bool {
    let prefix = format!(
        "{}.",
        pairwise_secret_store_key_prefix(session_id, local_endpoint_id, state_version)
    );
    key.strip_prefix(&prefix).is_some_and(|nonce| {
        nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}
