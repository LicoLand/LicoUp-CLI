use crate::core::secure_mesh_pairwise::SecureMeshPairwiseDurableStore;
use crate::domain::mobile_relay::secret_custody::{
    ensure_secure_mesh_protected_operation_allowed, mobile_relay_secret_store_override,
    pairwise_secret_store_override, selected_mobile_relay_secret_store,
};
use crate::platform::client_state::ClientStateStore;
use anyhow::Result;
use std::fs;
use std::path::PathBuf;

pub(in crate::domain::mobile_relay) fn mobile_relay_pairwise_store()
-> Result<SecureMeshPairwiseDurableStore> {
    ensure_secure_mesh_protected_operation_allowed()?;
    mobile_relay_pairwise_store_for_authority_reset()
}

pub(in crate::domain::mobile_relay) fn mobile_relay_pairwise_store_for_authority_reset()
-> Result<SecureMeshPairwiseDurableStore> {
    let path = mobile_relay_pairwise_store_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let secret_store = pairwise_secret_store_override()
        .or_else(mobile_relay_secret_store_override)
        .unwrap_or_else(selected_mobile_relay_secret_store);
    let mut store = SecureMeshPairwiseDurableStore::open_with_secret_store(
        &path,
        secret_store,
        crate::core::secure_mesh_pairwise::pairwise_secret_store_namespace(&path),
    )?;
    store.purge_unrecoverable_memory_only_sessions()?;
    Ok(store)
}

pub(in crate::domain::mobile_relay) fn mobile_relay_pairwise_store_path() -> Result<PathBuf> {
    Ok(ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("pairwise-pqxdh.sqlite3"))
}

pub(in crate::domain::mobile_relay) fn purge_mobile_relay_pairwise_sessions() -> Result<()> {
    let mut store = mobile_relay_pairwise_store_for_authority_reset()?;
    store.purge_sessions_preserving_prekey_history()?;
    Ok(())
}
