use anyhow::{Result, anyhow, ensure};
use openmls::prelude::{BasicCredential, Capabilities, CredentialWithKey, Extensions, KeyPackage};
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;

use crate::core::secure_mesh_secret_store::{
    SecretStoreAuthorizationSession, SecretStoreHandle, SecureMeshSecretStore,
};

use super::capability_extension::secure_mesh_mls_leaf_capabilities;
use super::config::secure_mesh_mls_ciphersuite;
use super::constants::MLS_EPOCH_SECRET_STORE_CLASS;
#[cfg(test)]
use super::constants::MLS_RECOVERY_SECRET_STORE_CLASS;
use super::group_state::basic_credential_identity;
use super::key_package::SecureMeshMlsKeyPackage;
use super::provider::SecureMeshOpenMlsProvider;

pub struct SecureMeshMlsParticipant {
    pub(super) provider: SecureMeshOpenMlsProvider,
    pub(super) credential_with_key: CredentialWithKey,
    pub(super) signer: SignatureKeyPair,
}

impl SecureMeshMlsParticipant {
    pub fn new(identity: impl Into<Vec<u8>>) -> Result<Self> {
        let provider = SecureMeshOpenMlsProvider::default();
        let signer = SignatureKeyPair::new(secure_mesh_mls_ciphersuite().signature_algorithm())
            .map_err(|error| anyhow!("secure mesh MLS signer generation failed: {error:?}"))?;
        Self::from_provider_credential_parts(provider, identity.into(), signer)
    }

    pub fn from_credential_parts(
        credential_identity: impl Into<Vec<u8>>,
        signer: SignatureKeyPair,
    ) -> Result<Self> {
        let provider = SecureMeshOpenMlsProvider::default();
        Self::from_provider_credential_parts(provider, credential_identity.into(), signer)
    }

    fn from_provider_credential_parts(
        provider: SecureMeshOpenMlsProvider,
        credential_identity: Vec<u8>,
        signer: SignatureKeyPair,
    ) -> Result<Self> {
        signer
            .store(provider.storage())
            .map_err(|_| anyhow!("secure mesh MLS signer storage failed"))?;
        let credential = BasicCredential::new(credential_identity);
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.to_public_vec().into(),
        };
        Ok(Self {
            provider,
            credential_with_key,
            signer,
        })
    }

    pub fn credential_identity_bytes(&self) -> Result<Vec<u8>> {
        basic_credential_identity(&self.credential_with_key.credential)
    }

    pub fn load_from_secret_store(
        identity: impl Into<Vec<u8>>,
        signing_public_key: impl AsRef<[u8]>,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
    ) -> Result<Self> {
        Self::load_from_secret_store_with_optional_session(
            identity,
            signing_public_key,
            secret_store,
            handle,
            None,
        )
    }

    pub(crate) fn load_from_secret_store_with_optional_session(
        identity: impl Into<Vec<u8>>,
        signing_public_key: impl AsRef<[u8]>,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        session: Option<&SecretStoreAuthorizationSession>,
    ) -> Result<Self> {
        let identity = identity.into();
        let signing_public_key = signing_public_key.as_ref().to_vec();
        let provider = match session {
            Some(session) => {
                let provider = SecureMeshOpenMlsProvider::load_secret_store_optional_with_session(
                    secret_store,
                    handle,
                    session,
                )?
                .ok_or_else(|| anyhow!("secure mesh MLS provider secret-store entry is missing"))?;
                provider
            }
            None => SecureMeshOpenMlsProvider::load_secret_store(secret_store, handle)?,
        };
        Self::from_provider_parts(identity, signing_public_key, provider)
    }

    pub(crate) fn load_from_secret_store_optional_with_session(
        identity: impl Into<Vec<u8>>,
        signing_public_key: impl AsRef<[u8]>,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<Option<Self>> {
        let provider = match SecureMeshOpenMlsProvider::load_secret_store_optional_with_session(
            secret_store,
            handle,
            session,
        )? {
            Some(provider) => provider,
            None => return Ok(None),
        };
        Ok(Some(Self::from_provider_parts(
            identity.into(),
            signing_public_key.as_ref().to_vec(),
            provider,
        )?))
    }

    fn from_provider_parts(
        identity: Vec<u8>,
        signing_public_key: Vec<u8>,
        provider: SecureMeshOpenMlsProvider,
    ) -> Result<Self> {
        ensure!(
            !signing_public_key.is_empty(),
            "secure mesh MLS signing public key is required"
        );
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &signing_public_key,
            secure_mesh_mls_ciphersuite().signature_algorithm(),
        )
        .ok_or_else(|| anyhow!("secure mesh MLS signer is missing from secret store"))?;
        let credential = BasicCredential::new(identity.into());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signing_public_key.into(),
        };
        Ok(Self {
            provider,
            credential_with_key,
            signer,
        })
    }

    pub fn signing_public_key(&self) -> Vec<u8> {
        self.signer.to_public_vec()
    }

    pub fn save_secret_store(
        &self,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        self.provider.save_secret_store(secret_store, handle)
    }

    pub(crate) fn save_secret_store_with_session(
        &self,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<()> {
        self.provider.save_secret_store_for_class_with_session(
            secret_store,
            handle,
            MLS_EPOCH_SECRET_STORE_CLASS,
            session,
        )
    }

    #[cfg(test)]
    pub(super) fn save_recovery_secret_store_with_session(
        &self,
        secret_store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<()> {
        self.provider.save_secret_store_for_class_with_session(
            secret_store,
            handle,
            MLS_RECOVERY_SECRET_STORE_CLASS,
            session,
        )
    }

    pub fn generate_key_package(&self) -> Result<SecureMeshMlsKeyPackage> {
        self.generate_key_package_with_leaf_capabilities(secure_mesh_mls_leaf_capabilities())
    }

    pub(crate) fn generate_key_package_with_leaf_capabilities(
        &self,
        capabilities: Capabilities,
    ) -> Result<SecureMeshMlsKeyPackage> {
        let bundle = KeyPackage::builder()
            .key_package_extensions(Extensions::default())
            .leaf_node_capabilities(capabilities)
            .build(
                secure_mesh_mls_ciphersuite(),
                &self.provider,
                &self.signer,
                self.credential_with_key.clone(),
            )
            .map_err(|error| anyhow!("secure mesh MLS key package generation failed: {error:?}"))?;
        SecureMeshMlsKeyPackage::from_bundle(bundle, self.provider.mlkem1024_seed.public_key())
    }
}
