use super::constants::*;
use super::model::*;
use super::primitives::*;
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};

use crate::core::secure_mesh_crypto::SealedSecureMeshPayload;

pub fn file_manifest_delivery_json(encrypted: &EncryptedSecureMeshFileManifest) -> Value {
    json!({
        "kind": "file_manifest",
        "contentType": SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE,
        "fileKeySuite": encrypted.file_key_suite,
        "fileAadDigest": encrypted.file_aad_digest,
        "ciphertextHash": encrypted.ciphertext_hash,
        "sealed": sealed_payload_json(&encrypted.sealed),
        "metadataEncrypted": true,
        "bodyRedacted": true
    })
}

pub fn file_chunk_delivery_json(encrypted: &EncryptedSecureMeshFileChunk) -> Value {
    json!({
        "kind": "file_chunk",
        "contentType": SECURE_MESH_FILE_CHUNK_CONTENT_TYPE,
        "fileKeySuite": encrypted.file_key_suite,
        "fileAadDigest": encrypted.file_aad_digest,
        "fileIdHash": encrypted.file_id_hash,
        "chunkIndex": encrypted.chunk_index,
        "chunkHash": encrypted.chunk_hash,
        "plaintextSize": encrypted.plaintext_size,
        "ciphertextHash": encrypted.ciphertext_hash,
        "sealed": sealed_payload_json(&encrypted.sealed),
        "metadataEncrypted": true,
        "bodyRedacted": true
    })
}

pub(super) fn sealed_payload_json(sealed: &SealedSecureMeshPayload) -> Value {
    json!({
        "protocolVersion": sealed.protocol_version,
        "cipherSuite": sealed.cipher_suite,
        "encryptedHeader": sealed.encrypted_header,
        "ciphertext": sealed.ciphertext,
        "ciphertextSize": sealed.ciphertext_size
    })
}

pub(super) fn ciphertext_hash(sealed: &SealedSecureMeshPayload) -> Result<String> {
    let ciphertext = general_purpose::URL_SAFE_NO_PAD
        .decode(&sealed.ciphertext)
        .context("secure mesh file ciphertext is not base64url")?;
    Ok(hash_bytes(&ciphertext))
}
