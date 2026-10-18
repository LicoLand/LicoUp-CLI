use anyhow::Result;

use super::{
    constants::{AAD_MAGIC, ADDITIONAL_AAD_MAGIC, SECURE_MESH_CONTENT_CIPHER_SUITE},
    length_codec::append_len_prefixed_bytes,
    model::{SecureMeshContentContext, SecureMeshPayloadKind},
    validation::validate_additional_aad,
};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;

pub(super) fn build_aad_with_binding(
    context: &SecureMeshContentContext,
    kind: SecureMeshPayloadKind,
    additional_aad: &[u8],
) -> Result<Vec<u8>> {
    validate_additional_aad(additional_aad)?;
    let mut out = Vec::new();
    out.extend_from_slice(AAD_MAGIC);
    append_len_prefixed_bytes(&mut out, SECURE_MESH_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, SECURE_MESH_CONTENT_CIPHER_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.envelope_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.message_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.opaque_mailbox_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.sender_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.recipient_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, kind.as_str().as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.created_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.expires_at.as_bytes())?;
    if !additional_aad.is_empty() {
        out.extend_from_slice(ADDITIONAL_AAD_MAGIC);
        append_len_prefixed_bytes(&mut out, additional_aad)?;
    }
    Ok(out)
}
