use anyhow::{Result, anyhow, ensure};

use crate::core::secure_mesh_crypto::SealedSecureMeshPrivateContextPayload;
use crate::core::secure_mesh_mls_pq_epoch::mlkem1024_epoch_extension_digest;

use super::capability_extension::secure_mesh_mls_capability_extension_digest;
use super::codec::{MlsPayloadReader, append_mls_len_prefixed_bytes};
use super::constants::{
    MLS_PAYLOAD_EXPORT_CONTEXT_MAGIC, MLS_PRIVATE_CONTEXT_PAYLOAD_MAGIC,
    SECURE_MESH_MLS_CIPHER_SUITE,
};
use super::group_model::SecureMeshMlsGroup;

pub(super) fn build_group_payload_export_context(group: &SecureMeshMlsGroup) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(MLS_PAYLOAD_EXPORT_CONTEXT_MAGIC);
    append_mls_len_prefixed_bytes(&mut out, SECURE_MESH_MLS_CIPHER_SUITE.as_bytes())?;
    append_mls_len_prefixed_bytes(&mut out, group.group.group_id().as_slice())?;
    out.extend_from_slice(&group.epoch().to_be_bytes());
    let capability_extension = group.capability_extension()?;
    capability_extension.require_active()?;
    append_mls_len_prefixed_bytes(
        &mut out,
        secure_mesh_mls_capability_extension_digest(&capability_extension)?.as_bytes(),
    )?;
    let pq_epoch_extension = group
        .mlkem1024_epoch_extension
        .as_ref()
        .ok_or_else(|| anyhow!("secure mesh MLS ML-KEM-1024 epoch extension is unavailable"))?;
    append_mls_len_prefixed_bytes(
        &mut out,
        mlkem1024_epoch_extension_digest(pq_epoch_extension)?.as_bytes(),
    )?;
    Ok(out)
}

pub(super) fn encode_mls_private_context_payload(
    sealed: &SealedSecureMeshPrivateContextPayload,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(MLS_PRIVATE_CONTEXT_PAYLOAD_MAGIC);
    append_mls_len_prefixed_bytes(&mut out, sealed.encrypted_header().as_bytes())?;
    out.extend_from_slice(
        &u64::try_from(sealed.ciphertext_size())
            .map_err(|_| anyhow!("secure mesh MLS ciphertext bucket is outside bounds"))?
            .to_be_bytes(),
    );
    append_mls_len_prefixed_bytes(&mut out, sealed.ciphertext().as_bytes())?;
    Ok(out)
}

pub(super) fn decode_mls_private_context_payload(
    bytes: &[u8],
) -> Result<SealedSecureMeshPrivateContextPayload> {
    let mut reader = MlsPayloadReader::new(bytes);
    reader.expect_bytes(MLS_PRIVATE_CONTEXT_PAYLOAD_MAGIC)?;
    let encrypted_header = reader.read_string("encrypted_header")?;
    let ciphertext_size = usize::try_from(reader.read_u64()?)
        .map_err(|_| anyhow!("secure mesh MLS ciphertext bucket is outside platform bounds"))?;
    let ciphertext = reader.read_string("ciphertext")?;
    ensure!(
        reader.is_empty(),
        "secure mesh MLS private-context payload has trailing bytes"
    );
    SealedSecureMeshPrivateContextPayload::from_encoded_parts(
        encrypted_header,
        ciphertext,
        ciphertext_size,
    )
}
