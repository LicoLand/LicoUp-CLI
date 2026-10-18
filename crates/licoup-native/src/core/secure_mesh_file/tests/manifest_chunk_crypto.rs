use super::support::*;

#[test]
fn secure_mesh_file_manifest_and_chunk_round_trip_without_outer_metadata_leak() {
    let key = key_fixture();
    let manifest = manifest_fixture();
    let manifest_context = context_fixture("manifest", "msg_manifest", &manifest);
    let encrypted_manifest = seal_file_manifest(&key, &manifest_context, &manifest).unwrap();
    let serialized_outer = format!("{:?}", encrypted_manifest);
    assert!(!serialized_outer.contains(&manifest.file_name));
    assert!(!serialized_outer.contains(&manifest.mime_type));
    assert!(!serialized_outer.contains(&manifest.relative_path));
    let opened_manifest = open_file_manifest(&key, &manifest_context, &encrypted_manifest).unwrap();
    assert_eq!(opened_manifest, manifest);

    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: b"encrypted file chunk bytes".to_vec(),
    };
    let chunk_context = context_fixture("chunk_0", "msg_chunk_0", &manifest);
    let encrypted_chunk = seal_file_chunk(&key, &chunk_context, &chunk).unwrap();
    assert_ne!(
        encrypted_chunk.ciphertext_hash,
        hash_bytes(chunk.bytes.as_slice())
    );
    assert!(encrypted_chunk.chunk_hash.starts_with("hmac-sha256:"));
    assert_ne!(
        encrypted_chunk.chunk_hash,
        hash_bytes(chunk.bytes.as_slice())
    );
    let opened_chunk = open_file_chunk(&key, &chunk_context, &encrypted_chunk).unwrap();
    assert_eq!(opened_chunk, chunk);
}

#[test]
fn secure_mesh_file_chunk_rejects_corrupted_ciphertext_hash() {
    let key = key_fixture();
    let chunk = SecureMeshFileChunk {
        file_id: "file_test".to_string(),
        chunk_index: 1,
        bytes: b"chunk".to_vec(),
    };
    let manifest = manifest_fixture();
    let context = context_fixture("chunk_1", "msg_chunk_1", &manifest);
    let mut encrypted = seal_file_chunk(&key, &context, &chunk).unwrap();
    encrypted.ciphertext_hash = "sha256:tampered".to_string();
    let error = open_file_chunk(&key, &context, &encrypted).unwrap_err();
    assert!(error.to_string().contains("ciphertext hash mismatch"));
}

#[test]
fn secure_mesh_file_chunk_rejects_tampered_or_legacy_chunk_hash() {
    let key = key_fixture();
    let chunk = SecureMeshFileChunk {
        file_id: "file_test".to_string(),
        chunk_index: 1,
        bytes: b"chunk".to_vec(),
    };
    let manifest = manifest_fixture();
    let context = context_fixture("chunk_hash_1", "msg_chunk_hash_1", &manifest);
    let encrypted = seal_file_chunk(&key, &context, &chunk).unwrap();

    let mut tampered = encrypted.clone();
    tampered.chunk_hash = format!(
        "hmac-sha256:{}",
        general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
    );
    assert!(open_file_chunk(&key, &context, &tampered).is_err());

    let mut legacy = encrypted;
    legacy.chunk_hash = hash_bytes(chunk.bytes.as_slice());
    let error = open_file_chunk(&key, &context, &legacy).unwrap_err();
    assert!(error.to_string().contains("algorithm is unsupported"));
}

#[test]
fn secure_mesh_file_manifest_rejects_path_traversal() {
    let key = key_fixture();
    let mut manifest = manifest_fixture();
    manifest.relative_path = "../secrets".to_string();
    let error = seal_file_manifest(
        &key,
        &context_fixture("manifest", "msg_manifest", &manifest),
        &manifest,
    )
    .unwrap_err();
    assert!(error.to_string().contains("must not traverse"));
}
