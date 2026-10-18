use anyhow::{Context, Result, anyhow, ensure};
use openmls::prelude::{
    BasicCredential, KeyPackage, KeyPackageBundle, KeyPackageIn, ProtocolVersion,
    tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize},
};
use openmls_rust_crypto::RustCrypto;

use crate::core::secure_mesh_pqxdh::validate_ml_kem_1024_public_key;

use super::codec::{MlsPayloadReader, append_mls_len_prefixed_bytes};
use super::config::secure_mesh_mls_ciphersuite;
use super::constants::MLS_KEY_PACKAGE_MAGIC;

pub struct SecureMeshMlsKeyPackage {
    pub(super) public_key_package: KeyPackage,
    pub(super) public_bytes: Vec<u8>,
    pub(super) mlkem1024_public_key: Vec<u8>,
}

impl SecureMeshMlsKeyPackage {
    pub fn as_public_bytes(&self) -> &[u8] {
        &self.public_bytes
    }

    pub fn credential_identity_bytes(&self) -> Result<Vec<u8>> {
        let basic =
            BasicCredential::try_from(self.public_key_package.leaf_node().credential().clone())
                .map_err(|error| {
                    anyhow!("secure mesh MLS keypackage credential is not basic: {error:?}")
                })?;
        Ok(basic.identity().to_vec())
    }

    pub fn signing_public_key(&self) -> Vec<u8> {
        self.public_key_package
            .leaf_node()
            .signature_key()
            .as_slice()
            .to_vec()
    }

    pub fn mlkem1024_public_key(&self) -> &[u8] {
        &self.mlkem1024_public_key
    }

    pub(crate) fn from_public_bytes(public_bytes: &[u8]) -> Result<Self> {
        ensure!(
            !public_bytes.is_empty(),
            "secure mesh MLS key package is empty"
        );
        let (mlkem1024_public_key, inner_public_bytes) =
            decode_mlkem1024_key_package(public_bytes)?;
        let key_package_in = KeyPackageIn::tls_deserialize_exact(inner_public_bytes)
            .context("secure mesh MLS key package deserialization failed")?;
        let public_key_package = key_package_in
            .validate(&RustCrypto::default(), ProtocolVersion::Mls10)
            .map_err(|error| {
                anyhow!("secure mesh MLS key package verification failed: {error:?}")
            })?;
        ensure!(
            public_key_package.ciphersuite() == secure_mesh_mls_ciphersuite(),
            "secure mesh MLS key package ciphersuite is unsupported"
        );
        Ok(Self {
            public_key_package,
            public_bytes: public_bytes.to_vec(),
            mlkem1024_public_key,
        })
    }

    pub(super) fn from_bundle(
        bundle: KeyPackageBundle,
        mlkem1024_public_key: Vec<u8>,
    ) -> Result<Self> {
        validate_ml_kem_1024_public_key(&mlkem1024_public_key)?;
        let inner_public_bytes = bundle
            .key_package()
            .tls_serialize_detached()
            .context("secure mesh MLS key package serialization failed")?;
        let public_bytes =
            encode_mlkem1024_key_package(&mlkem1024_public_key, &inner_public_bytes)?;
        Ok(Self {
            public_key_package: bundle.key_package().clone(),
            public_bytes,
            mlkem1024_public_key,
        })
    }
}

fn encode_mlkem1024_key_package(public_key: &[u8], inner: &[u8]) -> Result<Vec<u8>> {
    validate_ml_kem_1024_public_key(public_key)?;
    let mut out =
        Vec::with_capacity(MLS_KEY_PACKAGE_MAGIC.len() + 8 + public_key.len() + inner.len());
    out.extend_from_slice(MLS_KEY_PACKAGE_MAGIC);
    append_mls_len_prefixed_bytes(&mut out, public_key)?;
    append_mls_len_prefixed_bytes(&mut out, inner)?;
    Ok(out)
}

fn decode_mlkem1024_key_package(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut reader = MlsPayloadReader::new(bytes);
    reader.expect_bytes(MLS_KEY_PACKAGE_MAGIC).map_err(|_| {
        anyhow!("secure mesh MLS key package requires ML-KEM-1024 protocol migration")
    })?;
    let public_key = reader.read_len_prefixed_bytes()?.to_vec();
    validate_ml_kem_1024_public_key(&public_key)?;
    let inner = reader.read_len_prefixed_bytes()?.to_vec();
    ensure!(
        !inner.is_empty() && reader.is_empty(),
        "secure mesh MLS key package wrapper is invalid"
    );
    Ok((public_key, inner))
}
