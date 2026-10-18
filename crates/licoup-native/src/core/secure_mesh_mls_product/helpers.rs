use super::constants::MLS_CREDENTIAL_MAGIC;
use super::identity_trust::mls_credential_identity_bytes;
use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};

use crate::core::secure_mesh_mls::{SecureMeshMlsKeyPackage, SecureMeshMlsParticipant};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub(super) fn participant_identity_matches(
    participant: &SecureMeshMlsParticipant,
    identity: &DeviceTrustPublicIdentity,
) -> Result<bool> {
    Ok(
        participant.credential_identity_bytes()? == mls_credential_identity_bytes(identity)?
            && participant.signing_public_key() == identity.signing_public_key,
    )
}

pub(super) fn key_package_identity_matches(
    key_package: &SecureMeshMlsKeyPackage,
    identity: &DeviceTrustPublicIdentity,
) -> Result<bool> {
    Ok(
        key_package.credential_identity_bytes()? == mls_credential_identity_bytes(identity)?
            && key_package.signing_public_key() == identity.signing_public_key,
    )
}

pub(super) fn endpoint_id_from_credential_identity(credential: &[u8]) -> Result<String> {
    ensure!(
        credential.starts_with(MLS_CREDENTIAL_MAGIC),
        "secure mesh MLS credential magic mismatch"
    );
    let mut offset = MLS_CREDENTIAL_MAGIC.len();
    let endpoint = read_len_prefixed(credential, &mut offset)?;
    Ok(String::from_utf8(endpoint)
        .map_err(|_| anyhow!("secure mesh MLS credential endpoint is not utf8"))?)
}

pub(super) fn identity_validate(identity: &DeviceTrustPublicIdentity) -> Result<()> {
    ensure!(
        !identity.endpoint_id.trim().is_empty(),
        "secure mesh endpoint id is required"
    );
    Ok(())
}

pub(super) fn append_len_prefixed(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len()).map_err(|_| anyhow!("field too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) fn read_len_prefixed(bytes: &[u8], offset: &mut usize) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() >= *offset + 4,
        "secure mesh MLS credential is truncated"
    );
    let len = u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
    *offset += 4;
    ensure!(
        bytes.len() >= *offset + len,
        "secure mesh MLS credential is truncated"
    );
    let value = bytes[*offset..*offset + len].to_vec();
    *offset += len;
    Ok(value)
}

pub(super) fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
