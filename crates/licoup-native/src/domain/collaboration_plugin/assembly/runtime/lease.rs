use anyhow::{Result, anyhow};
use fs2::FileExt;
use std::io::ErrorKind;
use std::path::PathBuf;

use crate::platform::client_state::ClientStateStore;

pub(super) fn prepare(store: &ClientStateStore, deployment_id: &str) -> Result<PathBuf> {
    let path = path(store, deployment_id);
    let file = licoup_foundation::platform::file_security::open_private_lock_file(&path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok(path)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => {
            Err(anyhow!("collaboration_local_server_runtime_already_owned"))
        }
        Err(_) => Err(anyhow!("collaboration_local_server_runtime_lease_failed")),
    }
}

pub(super) fn is_held(store: &ClientStateStore, deployment_id: &str) -> Result<bool> {
    let file = licoup_foundation::platform::file_security::open_private_lock_file(&path(
        store,
        deployment_id,
    ))?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok(false)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(true),
        Err(_) => Err(anyhow!("collaboration_local_server_runtime_lease_failed")),
    }
}

pub(super) fn path(store: &ClientStateStore, deployment_id: &str) -> PathBuf {
    store
        .root()
        .join(format!(".local-server-runtime-{deployment_id}.lock"))
}
