use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use openmls_rust_crypto::{MemoryStorage, RustCrypto};
use zeroize::{Zeroize, Zeroizing};

use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_KEY_GENERATION_SEED_BYTES, SecureMeshMlKem1024PreKeySeed,
};
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle, SecureMeshSecretStore,
};

use super::constants::{
    MLS_EPOCH_SECRET_STORE_CLASS, MLS_PROVIDER_SECRET_SCHEMA_VERSION,
    MLS_RECOVERY_SECRET_STORE_CLASS,
};
use super::provider::SecureMeshOpenMlsProvider;

impl SecureMeshOpenMlsProvider {
    pub fn load_secret_store(
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
    ) -> Result<Self> {
        let session = secret_store.begin_authorized_session(
            &SecretStoreAuthorizationRequest::new("Secure Mesh MLS provider secret-store load", 1),
        )?;
        Self::load_secret_store_optional_with_session(secret_store, handle, &session)?
            .ok_or_else(|| anyhow!("secure mesh MLS provider secret-store entry is missing"))
    }

    pub(super) fn load_secret_store_optional_with_session(
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<Option<Self>> {
        let persisted_json = secret_store
            .get_secret_with_session(session, handle)
            .context("secure mesh MLS provider secret-store read failed")?;
        let Some(persisted_json) = persisted_json else {
            return Ok(None);
        };
        let persisted: PersistedMlsProviderSecrets =
            serde_json::from_slice(persisted_json.expose_bytes())
                .context("secure mesh MLS provider secret-store payload deserialization failed")?;
        ensure!(
            persisted.schema_version == MLS_PROVIDER_SECRET_SCHEMA_VERSION,
            "secure mesh MLS provider secret-store schema is unsupported"
        );
        ensure!(
            persisted.secret_class == MLS_EPOCH_SECRET_STORE_CLASS
                || persisted.secret_class == MLS_RECOVERY_SECRET_STORE_CLASS,
            "secure mesh MLS provider secret-store class is unsupported"
        );
        let storage_bytes = Zeroizing::new(
            general_purpose::URL_SAFE_NO_PAD
                .decode(&persisted.storage_base64url)
                .context("secure mesh MLS provider secret-store payload is not base64url")?,
        );
        let storage = deserialize_storage_from_bytes(&storage_bytes)?;
        let seed_bytes = Zeroizing::new(
            general_purpose::URL_SAFE_NO_PAD
                .decode(&persisted.mlkem1024_seed_base64url)
                .context("secure mesh MLS ML-KEM-1024 custody seed is not base64url")?,
        );
        ensure!(
            seed_bytes.len() == ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
            "secure mesh MLS ML-KEM-1024 custody seed length is invalid"
        );
        let mut fixed_seed = [0u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES];
        fixed_seed.copy_from_slice(&seed_bytes);
        Ok(Some(Self {
            crypto: RustCrypto::default(),
            storage,
            mlkem1024_seed: SecureMeshMlKem1024PreKeySeed::from_bytes(fixed_seed),
        }))
    }

    pub fn save_secret_store(
        &self,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        self.save_secret_store_for_class(secret_store, handle, MLS_EPOCH_SECRET_STORE_CLASS)
    }

    fn save_secret_store_for_class(
        &self,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        secret_class: &str,
    ) -> Result<()> {
        let session = secret_store.begin_authorized_session(
            &SecretStoreAuthorizationRequest::new("Secure Mesh MLS provider secret-store save", 1),
        )?;
        self.save_secret_store_for_class_with_session(secret_store, handle, secret_class, &session)
    }

    pub(super) fn save_secret_store_for_class_with_session(
        &self,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        secret_class: &str,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<()> {
        ensure!(
            secret_class == MLS_EPOCH_SECRET_STORE_CLASS
                || secret_class == MLS_RECOVERY_SECRET_STORE_CLASS,
            "secure mesh MLS provider secret-store class is unsupported"
        );
        let storage_bytes = Zeroizing::new(self.serialize_storage_to_bytes()?);
        let mut persisted = PersistedMlsProviderSecrets {
            schema_version: MLS_PROVIDER_SECRET_SCHEMA_VERSION,
            secret_class: secret_class.to_string(),
            storage_base64url: general_purpose::URL_SAFE_NO_PAD.encode(storage_bytes.as_slice()),
            mlkem1024_seed_base64url: general_purpose::URL_SAFE_NO_PAD
                .encode(self.mlkem1024_seed.expose_for_secret_store()),
        };
        let persisted_json = serde_json::to_string(&persisted)
            .context("secure mesh MLS provider secret-store payload serialization failed")?;
        persisted.secret_class.zeroize();
        persisted.storage_base64url.zeroize();
        persisted.mlkem1024_seed_base64url.zeroize();
        secret_store
            .set_secret_with_session(
                session,
                handle,
                SecretBytes::try_from_string(persisted_json)?,
            )
            .context("secure mesh MLS provider secret-store write failed")?;
        Ok(())
    }

    pub fn delete_secret_store(
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let session =
            secret_store.begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                "Secure Mesh MLS provider secret-store delete",
                1,
            ))?;
        Self::delete_secret_store_with_session(secret_store, handle, &session)
    }

    fn delete_secret_store_with_session(
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<()> {
        secret_store
            .delete_secret_with_session(session, handle)
            .context("secure mesh MLS provider secret-store delete failed")?;
        Ok(())
    }

    fn serialize_storage_to_bytes(&self) -> Result<Vec<u8>> {
        let values = self
            .storage
            .values
            .read()
            .map_err(|_| anyhow!("secure mesh MLS memory storage lock is poisoned"))?;
        let mut serializable = SerializableMlsMemoryStorage::default();
        for (key, value) in values.iter() {
            serializable.values.insert(
                general_purpose::URL_SAFE_NO_PAD.encode(key),
                general_purpose::URL_SAFE_NO_PAD.encode(value),
            );
        }
        serde_json::to_vec(&serializable)
            .context("secure mesh MLS provider memory storage serialization failed")
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PersistedMlsProviderSecrets {
    schema_version: u32,
    secret_class: String,
    storage_base64url: String,
    mlkem1024_seed_base64url: String,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SerializableMlsMemoryStorage {
    values: HashMap<String, String>,
}

fn deserialize_storage_from_bytes(bytes: &[u8]) -> Result<MemoryStorage> {
    let serializable: SerializableMlsMemoryStorage = serde_json::from_slice(bytes)
        .context("secure mesh MLS provider memory storage deserialization failed")?;
    let mut values = HashMap::new();
    for (key, value) in serializable.values {
        values.insert(
            general_purpose::URL_SAFE_NO_PAD
                .decode(key)
                .context("secure mesh MLS provider memory storage key is not base64url")?,
            general_purpose::URL_SAFE_NO_PAD
                .decode(value)
                .context("secure mesh MLS provider memory storage value is not base64url")?,
        );
    }
    Ok(MemoryStorage {
        values: RwLock::new(values),
    })
}
