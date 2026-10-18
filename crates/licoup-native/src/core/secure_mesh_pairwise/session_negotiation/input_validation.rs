use anyhow::{Result, ensure};
use ed25519_dalek::SigningKey;

use super::super::key_ratchet::SecureMeshPairwisePrivateKey;
use super::super::support::{
    SECURE_MESH_PAIRWISE_CIPHER_SUITE, SIGNATURE_LEN, parse_key_bytes, require_sha256_hex,
    require_text, validate_endpoint_id,
};
use super::transcript_codec::{SecureMeshPairwiseSessionIntro, decode_fixed_base64url};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_capability_proof::encode_signed_capability_proof_json;
use crate::core::secure_mesh_pqxdh::ML_KEM_1024_CIPHERTEXT_BYTES;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub(in crate::core::secure_mesh_pairwise) fn ensure_local_identity_key_material(
    identity: &DeviceTrustPublicIdentity,
    identity_secret: &SecureMeshPairwisePrivateKey,
    signing_key: &SigningKey,
) -> Result<()> {
    ensure!(
        identity_secret.public_key() == identity.identity_public_key,
        "secure mesh pairwise identity secret does not match public identity"
    );
    ensure!(
        signing_key.verifying_key().to_bytes() == identity.signing_public_key,
        "secure mesh pairwise signing secret does not match public identity"
    );
    Ok(())
}

pub(in crate::core::secure_mesh_pairwise) fn ensure_intro(
    intro: &SecureMeshPairwiseSessionIntro,
) -> Result<()> {
    ensure!(
        intro.protocol_version == SECURE_MESH_PROTOCOL_VERSION,
        "secure mesh pairwise intro protocol is unsupported"
    );
    ensure!(
        intro.cipher_suite == SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "secure mesh pairwise intro cipher suite is unsupported"
    );
    validate_endpoint_id(&intro.initiator_endpoint_id)?;
    validate_endpoint_id(&intro.responder_endpoint_id)?;
    ensure!(
        intro.initiator_endpoint_id != intro.responder_endpoint_id,
        "secure mesh pairwise intro endpoints must be distinct"
    );
    require_text(intro.session_id.clone(), "session id")?;
    require_text(intro.responder_signed_prekey_id.clone(), "signed prekey id")?;
    require_sha256_hex(
        intro.directory_authorization_digest.clone(),
        "directory authorization digest",
    )?;
    if let Some(one_time_prekey_id) = &intro.responder_one_time_prekey_id {
        require_text(one_time_prekey_id.clone(), "one-time prekey id")?;
    }
    require_text(
        intro.responder_one_time_mlkem1024_prekey_id.clone(),
        "one-time ML-KEM-1024 prekey id",
    )?;
    ensure!(
        intro.mlkem1024_ciphertext.len() == ML_KEM_1024_CIPHERTEXT_BYTES,
        "secure mesh pairwise ML-KEM-1024 ciphertext length is invalid"
    );
    parse_key_bytes(
        &intro.initiator_identity_public_key,
        "initiator identity public key",
    )?;
    parse_key_bytes(
        &intro.initiator_ephemeral_public_key,
        "initiator ephemeral public key",
    )?;
    parse_key_bytes(
        &intro.initiator_initial_ratchet_public_key,
        "initiator ratchet public key",
    )?;
    decode_fixed_base64url::<SIGNATURE_LEN>(&intro.initiator_signature, "intro signature")?;
    encode_signed_capability_proof_json(&intro.initiator_capability_proof)?;
    Ok(())
}
