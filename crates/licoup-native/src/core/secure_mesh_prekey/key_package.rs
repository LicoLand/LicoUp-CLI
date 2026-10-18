use anyhow::{Result, ensure};
use ed25519_dalek::SigningKey;
use time::OffsetDateTime;

use crate::core::secure_mesh_mls::SECURE_MESH_MLS_CIPHER_SUITE;
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

use super::validation::{
    append_len_prefixed_bytes, ensure_active_trust_state, ensure_not_expired,
    ensure_signature_shape, parse_rfc3339, sign_payload, verify_signature,
};

pub const SECURE_MESH_KEYPACKAGE_PROTOCOL_VERSION: &str =
    "licomesh.secure-mesh.group-mls.mlkem1024-epoch-payload-hybrid.v1";
pub const SECURE_MESH_KEYPACKAGE_WIRE_CIPHER_SUITE: &str =
    "licomesh.mls-rfc9420.v1.aes128gcmsha256ed25519x25519";

const KEYPACKAGE_MAGIC: &[u8] = b"LCOSM-KEYPACKAGE-v1";
const MAX_KEYPACKAGE_ID_LEN: usize = 128;
const MAX_CREDENTIAL_LEN: usize = 2048;
const MAX_KEYPACKAGE_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshKeyPackageRecord {
    pub key_package_id: String,
    pub cipher_suite: String,
    pub credential: String,
    pub public_key_package: Vec<u8>,
    pub signature: String,
    pub created_at: String,
    pub expires_at: String,
}

pub fn sign_key_package_record(
    signer_key: &SigningKey,
    endpoint_identity: &DeviceTrustPublicIdentity,
    key_package_id: impl Into<String>,
    cipher_suite: impl Into<String>,
    credential: impl Into<String>,
    public_key_package: impl Into<Vec<u8>>,
    created_at: impl Into<String>,
    expires_at: impl Into<String>,
) -> Result<SecureMeshKeyPackageRecord> {
    let mut record = SecureMeshKeyPackageRecord {
        key_package_id: key_package_id.into(),
        cipher_suite: cipher_suite.into(),
        credential: credential.into(),
        public_key_package: public_key_package.into(),
        signature: String::new(),
        created_at: created_at.into(),
        expires_at: expires_at.into(),
    };
    validate_key_package_shape(&record, false)?;
    let payload = key_package_signature_payload(endpoint_identity, &record)?;
    record.signature = sign_payload(signer_key, &payload);
    Ok(record)
}

pub fn verify_key_package_record(
    endpoint_identity: &DeviceTrustPublicIdentity,
    trust_state: DeviceTrustState,
    record: &SecureMeshKeyPackageRecord,
    require_verified_device: bool,
    now: OffsetDateTime,
) -> Result<()> {
    ensure_active_trust_state(trust_state, require_verified_device)?;
    validate_key_package_shape(record, true)?;
    ensure_not_expired(
        &record.created_at,
        &record.expires_at,
        now,
        "MLS KeyPackage",
    )?;
    verify_signature(
        endpoint_identity,
        &key_package_signature_payload(endpoint_identity, record)?,
        &record.signature,
        "MLS KeyPackage",
    )
}

fn validate_key_package_shape(
    record: &SecureMeshKeyPackageRecord,
    require_signature: bool,
) -> Result<()> {
    ensure!(
        !record.key_package_id.trim().is_empty(),
        "secure mesh MLS KeyPackage id is required"
    );
    ensure!(
        record.key_package_id.len() <= MAX_KEYPACKAGE_ID_LEN,
        "secure mesh MLS KeyPackage id is too large"
    );
    ensure!(
        record.cipher_suite == SECURE_MESH_KEYPACKAGE_WIRE_CIPHER_SUITE
            || record.cipher_suite == SECURE_MESH_MLS_CIPHER_SUITE,
        "secure mesh MLS KeyPackage cipher suite is unsupported"
    );
    ensure!(
        !record.credential.trim().is_empty() && record.credential.len() <= MAX_CREDENTIAL_LEN,
        "secure mesh MLS KeyPackage credential is outside bounds"
    );
    ensure!(
        !record.public_key_package.is_empty()
            && record.public_key_package.len() <= MAX_KEYPACKAGE_BYTES,
        "secure mesh MLS KeyPackage public bytes are outside bounds"
    );
    if require_signature {
        ensure_signature_shape(&record.signature, "MLS KeyPackage")?;
    }
    parse_rfc3339(&record.created_at, "MLS KeyPackage")?;
    parse_rfc3339(&record.expires_at, "MLS KeyPackage")?;
    Ok(())
}

pub(super) fn key_package_signature_payload(
    endpoint_identity: &DeviceTrustPublicIdentity,
    record: &SecureMeshKeyPackageRecord,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(KEYPACKAGE_MAGIC);
    append_len_prefixed_bytes(&mut out, SECURE_MESH_KEYPACKAGE_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, endpoint_identity.endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, endpoint_identity.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.key_package_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.cipher_suite.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.credential.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.created_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.expires_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, &record.public_key_package)?;
    Ok(out)
}
