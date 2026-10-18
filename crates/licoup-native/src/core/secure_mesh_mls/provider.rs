use openmls_rust_crypto::{MemoryStorage, RustCrypto};
use openmls_traits::OpenMlsProvider;

use crate::core::secure_mesh_pqxdh::SecureMeshMlKem1024PreKeySeed;

pub struct SecureMeshOpenMlsProvider {
    pub(super) crypto: RustCrypto,
    pub(super) storage: MemoryStorage,
    pub(super) mlkem1024_seed: SecureMeshMlKem1024PreKeySeed,
}

impl Default for SecureMeshOpenMlsProvider {
    fn default() -> Self {
        Self {
            crypto: RustCrypto::default(),
            storage: MemoryStorage::default(),
            mlkem1024_seed: SecureMeshMlKem1024PreKeySeed::generate(),
        }
    }
}

impl OpenMlsProvider for SecureMeshOpenMlsProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}
