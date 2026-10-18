//! Platform composition for the core MLS durable state store.

use anyhow::Result;
use std::path::Path;

use crate::core::secure_mesh_mls::SecureMeshMlsDurableStore;

pub(crate) fn open(path: impl AsRef<Path>) -> Result<SecureMeshMlsDurableStore> {
    SecureMeshMlsDurableStore::open_with_path_hardener(
        path,
        licoup_foundation::platform::file_security::harden_private_path,
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;

    #[test]
    fn durable_store_file_is_hardened_by_platform_adapter() {
        let path = std::env::temp_dir().join(format!("lico-mls-store-{}.sqlite3", Uuid::new_v4()));
        let store = open(&path).unwrap();
        drop(store);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_file(path);
    }
}
