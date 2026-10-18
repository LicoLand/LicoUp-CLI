//! Cross-process serialization for collaboration capability mutations.
//!
//! The lock is intentionally shared by lifecycle, assembly, registration, and
//! runtime control. Individual JSON files are atomically replaced, but those
//! replacements are not transactions across protected authority and file-tree
//! commits.

use anyhow::{Result, anyhow};
use fs2::FileExt;
use std::fs::File;

use crate::platform::client_state::ClientStateStore;
use licoup_foundation::platform::file_security::{ensure_private_dir, open_private_lock_file};

pub(super) struct CollaborationTransactionGuard {
    lock: File,
}

impl CollaborationTransactionGuard {
    pub(super) fn acquire(store: &ClientStateStore) -> Result<Self> {
        let root = super::lifecycle::collaboration_root(store);
        ensure_private_dir(&root)?;
        let lock = open_private_lock_file(&root.join(".collaboration-transaction.lock"))?;
        lock.lock_exclusive()
            .map_err(|_| anyhow!("collaboration_transaction_lock_unavailable"))?;
        Ok(Self { lock })
    }
}

impl Drop for CollaborationTransactionGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock);
    }
}
