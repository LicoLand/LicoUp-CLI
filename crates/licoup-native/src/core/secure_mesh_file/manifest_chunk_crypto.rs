use super::constants::*;
use super::delivery_projection::ciphertext_hash;
use super::json_input::*;
use super::key_proof::*;
use super::model::*;
use super::primitives::*;
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};

use crate::core::secure_mesh_crypto::{
    ContentKey, SecureMeshPayloadKind, SecureMeshPlaintext, open_payload, seal_payload,
};

pub fn seal_file_manifest(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    manifest: &SecureMeshFileManifest,
) -> Result<EncryptedSecureMeshFileManifest> {
    validate_manifest(manifest)?;
    context.validate()?;
    ensure!(
        context.file_id == manifest.file_id && context.chunk_count == manifest.chunk_count,
        "secure mesh file manifest protection context mismatch"
    );
    let aad = file_authenticated_data(
        context,
        FILE_AAD_MANIFEST_PURPOSE,
        None,
        context.file_hash(),
    )?;
    let derived_key = derive_file_key(root_key.as_bytes(), FILE_HKDF_MANIFEST_DOMAIN, &aad)?;
    let content_key = ContentKey::from_bytes(*derived_key);
    let scoped_context = scoped_file_content_context(context, &aad);
    let sealed = seal_payload(
        &content_key,
        &scoped_context,
        &SecureMeshPlaintext::new(
            SecureMeshPayloadKind::FileManifest,
            encode_manifest(manifest)?,
        )
        .with_content_type(SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE),
    )?;
    let ciphertext_hash = ciphertext_hash(&sealed)?;
    Ok(EncryptedSecureMeshFileManifest {
        file_key_suite: SECURE_MESH_FILE_KEY_SUITE.to_string(),
        file_aad_digest: general_purpose::URL_SAFE_NO_PAD.encode(file_aad_digest(&aad)),
        sealed,
        ciphertext_hash,
    })
}

pub fn open_file_manifest(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    encrypted: &EncryptedSecureMeshFileManifest,
) -> Result<SecureMeshFileManifest> {
    context.validate()?;
    ensure!(
        encrypted.file_key_suite == SECURE_MESH_FILE_KEY_SUITE,
        "secure mesh file manifest key suite is unsupported"
    );
    let aad = file_authenticated_data(
        context,
        FILE_AAD_MANIFEST_PURPOSE,
        None,
        context.file_hash(),
    )?;
    ensure!(
        decode_exact_base64url("file manifest AAD digest", &encrypted.file_aad_digest, 32,)?
            == file_aad_digest(&aad),
        "secure mesh file manifest protection context mismatch"
    );
    ensure!(
        ciphertext_hash(&encrypted.sealed)? == encrypted.ciphertext_hash,
        "secure mesh file manifest ciphertext hash mismatch"
    );
    let derived_key = derive_file_key(root_key.as_bytes(), FILE_HKDF_MANIFEST_DOMAIN, &aad)?;
    let content_key = ContentKey::from_bytes(*derived_key);
    let scoped_context = scoped_file_content_context(context, &aad);
    let opened = open_payload(
        &content_key,
        &scoped_context,
        &encrypted.sealed,
        SecureMeshPayloadKind::FileManifest,
    )?;
    ensure!(
        opened.content_type.as_deref() == Some(SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE),
        "secure mesh file manifest content type mismatch"
    );
    let manifest = decode_manifest(&opened.body)?;
    ensure!(
        manifest.file_id == context.file_id && manifest.chunk_count == context.chunk_count,
        "secure mesh file manifest protection context mismatch"
    );
    Ok(manifest)
}

pub fn seal_file_chunk(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    chunk: &SecureMeshFileChunk,
) -> Result<EncryptedSecureMeshFileChunk> {
    validate_chunk(chunk)?;
    context.validate()?;
    ensure!(
        context.file_id == chunk.file_id,
        "secure mesh file chunk protection context mismatch"
    );
    ensure_file_chunk_context(context, chunk.chunk_index)?;
    let chunk_hash = authenticated_file_chunk_hash(
        root_key,
        context,
        chunk.chunk_index,
        chunk.bytes.as_slice(),
    )?;
    let aad = file_authenticated_data(
        context,
        FILE_AAD_CHUNK_PURPOSE,
        Some(chunk.chunk_index),
        &chunk_hash,
    )?;
    let derived_key = derive_file_key(root_key.as_bytes(), FILE_HKDF_CHUNK_DOMAIN, &aad)?;
    let content_key = ContentKey::from_bytes(*derived_key);
    let scoped_context = scoped_file_content_context(context, &aad);
    let sealed = seal_payload(
        &content_key,
        &scoped_context,
        &SecureMeshPlaintext::new(SecureMeshPayloadKind::FileChunk, encode_chunk(chunk)?)
            .with_content_type(SECURE_MESH_FILE_CHUNK_CONTENT_TYPE),
    )?;
    Ok(EncryptedSecureMeshFileChunk {
        file_key_suite: SECURE_MESH_FILE_KEY_SUITE.to_string(),
        file_aad_digest: general_purpose::URL_SAFE_NO_PAD.encode(file_aad_digest(&aad)),
        file_id_hash: hash_bytes(chunk.file_id.as_bytes()),
        chunk_index: chunk.chunk_index,
        chunk_hash,
        plaintext_size: chunk.bytes.len(),
        ciphertext_hash: ciphertext_hash(&sealed)?,
        sealed,
    })
}

pub fn open_file_chunk(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    encrypted: &EncryptedSecureMeshFileChunk,
) -> Result<SecureMeshFileChunk> {
    context.validate()?;
    ensure!(
        encrypted.file_key_suite == SECURE_MESH_FILE_KEY_SUITE,
        "secure mesh file chunk key suite is unsupported"
    );
    ensure_file_chunk_context(context, encrypted.chunk_index)?;
    validate_file_chunk_hash("file chunk hash", &encrypted.chunk_hash)?;
    ensure!(
        hash_bytes(context.file_id.as_bytes()) == encrypted.file_id_hash,
        "secure mesh file chunk protection context mismatch"
    );
    let aad = file_authenticated_data(
        context,
        FILE_AAD_CHUNK_PURPOSE,
        Some(encrypted.chunk_index),
        &encrypted.chunk_hash,
    )?;
    ensure!(
        decode_exact_base64url("file chunk AAD digest", &encrypted.file_aad_digest, 32,)?
            == file_aad_digest(&aad),
        "secure mesh file chunk protection context mismatch"
    );
    ensure!(
        ciphertext_hash(&encrypted.sealed)? == encrypted.ciphertext_hash,
        "secure mesh file chunk ciphertext hash mismatch"
    );
    let derived_key = derive_file_key(root_key.as_bytes(), FILE_HKDF_CHUNK_DOMAIN, &aad)?;
    let content_key = ContentKey::from_bytes(*derived_key);
    let scoped_context = scoped_file_content_context(context, &aad);
    let opened = open_payload(
        &content_key,
        &scoped_context,
        &encrypted.sealed,
        SecureMeshPayloadKind::FileChunk,
    )?;
    ensure!(
        opened.content_type.as_deref() == Some(SECURE_MESH_FILE_CHUNK_CONTENT_TYPE),
        "secure mesh file chunk content type mismatch"
    );
    let chunk = decode_chunk(&opened.body)?;
    ensure!(
        chunk.file_id == context.file_id,
        "secure mesh file chunk protection context mismatch"
    );
    ensure!(
        hash_bytes(chunk.file_id.as_bytes()) == encrypted.file_id_hash,
        "secure mesh file chunk file id hash mismatch"
    );
    ensure!(
        chunk.chunk_index == encrypted.chunk_index,
        "secure mesh file chunk index mismatch"
    );
    ensure!(
        chunk.bytes.len() == encrypted.plaintext_size,
        "secure mesh file chunk plaintext size mismatch"
    );
    ensure!(
        authenticated_file_chunk_hash(
            root_key,
            context,
            chunk.chunk_index,
            chunk.bytes.as_slice(),
        )? == encrypted.chunk_hash,
        "secure mesh file chunk hash mismatch"
    );
    Ok(chunk)
}

pub(super) fn manifest_from_json(value: &Value) -> Result<SecureMeshFileManifest> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh file manifest must be an object"))?;
    let manifest = SecureMeshFileManifest {
        file_id: json_text(object, &["fileId", "file_id"])?,
        file_name: json_text(object, &["fileName", "file_name"])?,
        mime_type: json_text(object, &["mimeType", "mime_type"])?,
        relative_path: json_text(object, &["relativePath", "relative_path"])?,
        total_size: json_u64(object, &["totalSize", "total_size"])?,
        chunk_size: json_u32(object, &["chunkSize", "chunk_size"])?,
        chunk_count: json_u32(object, &["chunkCount", "chunk_count"])?,
    };
    validate_manifest_for_transfer(&manifest)?;
    Ok(manifest)
}

pub(super) fn manifest_to_json(manifest: &SecureMeshFileManifest) -> Value {
    json!({
        "fileId": &manifest.file_id,
        "fileName": &manifest.file_name,
        "mimeType": &manifest.mime_type,
        "relativePath": &manifest.relative_path,
        "totalSize": manifest.total_size,
        "chunkSize": manifest.chunk_size,
        "chunkCount": manifest.chunk_count
    })
}

pub(super) fn validate_manifest(manifest: &SecureMeshFileManifest) -> Result<()> {
    validate_text("file_id", &manifest.file_id, MAX_FILE_NAME_BYTES)?;
    validate_text("file_name", &manifest.file_name, MAX_FILE_NAME_BYTES)?;
    validate_file_name_segment(&manifest.file_name)?;
    validate_text("mime_type", &manifest.mime_type, MAX_MIME_BYTES)?;
    validate_relative_path(&manifest.relative_path)?;
    ensure!(
        manifest.chunk_size > 0,
        "secure mesh file chunk size is required"
    );
    ensure!(
        manifest.chunk_count > 0 && manifest.chunk_count <= MAX_CHUNK_COUNT,
        "secure mesh file chunk count is outside bounds"
    );
    Ok(())
}

pub(super) fn validate_manifest_for_transfer(manifest: &SecureMeshFileManifest) -> Result<()> {
    validate_manifest(manifest)?;
    ensure!(
        manifest.total_size > 0,
        "secure mesh file total size is required"
    );
    let expected_count = manifest.total_size.div_ceil(u64::from(manifest.chunk_size));
    ensure!(
        expected_count == u64::from(manifest.chunk_count),
        "secure mesh file manifest chunk count does not match total size"
    );
    Ok(())
}

pub(super) fn validate_chunk_plaintext_matches_manifest(
    manifest: &SecureMeshFileManifest,
    chunk: &SecureMeshFileChunk,
) -> Result<()> {
    ensure!(
        chunk.file_id == manifest.file_id,
        "secure mesh file handoff chunk file id mismatch"
    );
    ensure!(
        chunk.chunk_index == 0 && manifest.chunk_count == 1,
        "secure mesh file handoff proof currently requires one chunk"
    );
    ensure!(
        u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX) == manifest.total_size,
        "secure mesh file handoff chunk size does not match manifest"
    );
    Ok(())
}

pub(super) fn validate_chunk(chunk: &SecureMeshFileChunk) -> Result<()> {
    validate_text("file_id", &chunk.file_id, MAX_FILE_NAME_BYTES)?;
    ensure!(
        chunk.bytes.len() <= MAX_CHUNK_BYTES,
        "secure mesh file chunk body is too large"
    );
    Ok(())
}

pub(super) fn encode_manifest(manifest: &SecureMeshFileManifest) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(FILE_MANIFEST_MAGIC);
    append_len_prefixed_bytes(&mut out, manifest.file_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, manifest.file_name.as_bytes())?;
    append_len_prefixed_bytes(&mut out, manifest.mime_type.as_bytes())?;
    append_len_prefixed_bytes(&mut out, manifest.relative_path.as_bytes())?;
    out.extend_from_slice(&manifest.total_size.to_be_bytes());
    out.extend_from_slice(&manifest.chunk_size.to_be_bytes());
    out.extend_from_slice(&manifest.chunk_count.to_be_bytes());
    Ok(out)
}

pub(super) fn decode_manifest(bytes: &[u8]) -> Result<SecureMeshFileManifest> {
    let mut reader = SliceReader::new(bytes);
    reader.expect_bytes(FILE_MANIFEST_MAGIC)?;
    let manifest = SecureMeshFileManifest {
        file_id: read_string(&mut reader, "file_id")?,
        file_name: read_string(&mut reader, "file_name")?,
        mime_type: read_string(&mut reader, "mime_type")?,
        relative_path: read_string(&mut reader, "relative_path")?,
        total_size: reader.read_u64()?,
        chunk_size: reader.read_u32()?,
        chunk_count: reader.read_u32()?,
    };
    ensure!(
        reader.is_empty(),
        "secure mesh file manifest has trailing bytes"
    );
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub(super) fn encode_chunk(chunk: &SecureMeshFileChunk) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(FILE_CHUNK_MAGIC);
    append_len_prefixed_bytes(&mut out, chunk.file_id.as_bytes())?;
    out.extend_from_slice(&chunk.chunk_index.to_be_bytes());
    append_len_prefixed_bytes(&mut out, &chunk.bytes)?;
    Ok(out)
}

pub(super) fn decode_chunk(bytes: &[u8]) -> Result<SecureMeshFileChunk> {
    let mut reader = SliceReader::new(bytes);
    reader.expect_bytes(FILE_CHUNK_MAGIC)?;
    let chunk = SecureMeshFileChunk {
        file_id: read_string(&mut reader, "file_id")?,
        chunk_index: reader.read_u32()?,
        bytes: reader.read_len_prefixed_bytes()?.to_vec(),
    };
    ensure!(
        reader.is_empty(),
        "secure mesh file chunk has trailing bytes"
    );
    validate_chunk(&chunk)?;
    Ok(chunk)
}
