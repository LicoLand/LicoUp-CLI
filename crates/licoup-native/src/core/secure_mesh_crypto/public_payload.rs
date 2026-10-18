use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload as AeadPayload},
};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    aad_binding::build_aad_with_binding,
    constants::{CONTENT_NONCE_LEN, MAX_SEALED_CONTENT_BYTES, SECURE_MESH_CONTENT_CIPHER_SUITE},
    content_key::ContentKey,
    frame_codec::{decode_plaintext, encode_plaintext},
    header_codec::{decode_header, encode_header, encoded_len_limit},
    key_derivation::derive_aead_key,
    model::{
        OpenedSecureMeshPayload, SealedSecureMeshPayload, SecureMeshContentContext,
        SecureMeshPayloadKind, SecureMeshPlaintext,
    },
    padding::{add_bucket_padding, remove_authenticated_padding},
    validation::{validate_additional_aad, validate_plaintext},
};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;

pub fn seal_payload(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
) -> Result<SealedSecureMeshPayload> {
    seal_payload_with_aad_binding(key, context, plaintext, &[])
}

pub fn seal_payload_with_aad_binding(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
    additional_aad: &[u8],
) -> Result<SealedSecureMeshPayload> {
    let mut nonce = [0u8; CONTENT_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    seal_payload_with_nonce_and_aad_binding(key, context, plaintext, nonce, additional_aad)
}

pub fn open_payload(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    sealed: &SealedSecureMeshPayload,
    expected_kind: SecureMeshPayloadKind,
) -> Result<OpenedSecureMeshPayload> {
    open_payload_with_aad_binding(key, context, sealed, expected_kind, &[])
}

pub fn open_payload_with_aad_binding(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    sealed: &SealedSecureMeshPayload,
    expected_kind: SecureMeshPayloadKind,
    additional_aad: &[u8],
) -> Result<OpenedSecureMeshPayload> {
    context.validate()?;
    validate_additional_aad(additional_aad)?;
    ensure!(
        sealed.protocol_version == SECURE_MESH_PROTOCOL_VERSION,
        "secure mesh payload protocol version is unsupported"
    );
    ensure!(
        sealed.cipher_suite == SECURE_MESH_CONTENT_CIPHER_SUITE,
        "secure mesh payload cipher suite is unsupported"
    );
    ensure!(
        sealed.ciphertext_size > 0 && sealed.ciphertext_size <= MAX_SEALED_CONTENT_BYTES,
        "secure mesh payload ciphertext size is outside bounds"
    );
    ensure!(
        sealed.ciphertext.len() <= encoded_len_limit(MAX_SEALED_CONTENT_BYTES),
        "secure mesh payload encoded ciphertext is too large"
    );
    let (nonce, aad_hash) = decode_header(&sealed.encrypted_header)?;
    let aad = build_aad_with_binding(context, expected_kind, additional_aad)?;
    ensure!(
        Sha256::digest(&aad).as_slice() == aad_hash,
        "secure mesh payload AAD hash mismatch"
    );
    let ciphertext = general_purpose::URL_SAFE_NO_PAD
        .decode(&sealed.ciphertext)
        .context("secure mesh payload ciphertext is not base64url")?;
    ensure!(
        ciphertext.len() == sealed.ciphertext_size,
        "secure mesh payload ciphertext size mismatch"
    );
    let derived_key = derive_aead_key(key, context, expected_kind, &aad)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(derived_key.as_slice()));
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                AeadPayload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow!("secure mesh payload authentication failed"))?,
    );
    let unpadded = remove_authenticated_padding(&plaintext)?;
    let opened = decode_plaintext(unpadded)?;
    ensure!(
        opened.kind == expected_kind,
        "secure mesh payload kind mismatch"
    );
    Ok(opened)
}

#[cfg(test)]
pub(in crate::core::secure_mesh_crypto) fn seal_payload_with_nonce(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
    nonce: [u8; CONTENT_NONCE_LEN],
) -> Result<SealedSecureMeshPayload> {
    seal_payload_with_nonce_and_aad_binding(key, context, plaintext, nonce, &[])
}

fn seal_payload_with_nonce_and_aad_binding(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
    nonce: [u8; CONTENT_NONCE_LEN],
    additional_aad: &[u8],
) -> Result<SealedSecureMeshPayload> {
    context.validate()?;
    validate_plaintext(plaintext)?;
    validate_additional_aad(additional_aad)?;
    let aad = build_aad_with_binding(context, plaintext.kind, additional_aad)?;
    let derived_key = derive_aead_key(key, context, plaintext.kind, &aad)?;
    let encoded_plaintext = encode_plaintext(context, plaintext)?;
    let padded_plaintext = add_bucket_padding(&encoded_plaintext)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(derived_key.as_slice()));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            AeadPayload {
                msg: padded_plaintext.as_slice(),
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("secure mesh payload encryption failed"))?;
    let encrypted_header = encode_header(&nonce, &Sha256::digest(&aad));
    Ok(SealedSecureMeshPayload {
        protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
        cipher_suite: SECURE_MESH_CONTENT_CIPHER_SUITE.to_string(),
        encrypted_header,
        ciphertext_size: ciphertext.len(),
        ciphertext: general_purpose::URL_SAFE_NO_PAD.encode(ciphertext),
    })
}
