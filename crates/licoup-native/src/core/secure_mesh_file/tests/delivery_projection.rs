use super::support::*;

#[test]
fn secure_mesh_file_delivery_json_hides_manifest_and_chunk_plaintext() {
    let key = key_fixture();
    let manifest = SecureMeshFileManifest {
        file_id: "file-sensitive-id-canary".to_string(),
        file_name: "user-tax-return-secret-canary.pdf".to_string(),
        mime_type: "application/x-secret-canary".to_string(),
        relative_path: "private/folder/secret-path-canary".to_string(),
        total_size: 40,
        chunk_size: 40,
        chunk_count: 1,
    };
    let encrypted_manifest = seal_file_manifest(
        &key,
        &context_fixture("manifest", "msg_manifest", &manifest),
        &manifest,
    )
    .unwrap();
    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: b"file-body-plaintext-secret-canary-content".to_vec(),
    };
    let encrypted_chunk = seal_file_chunk(
        &key,
        &context_fixture("chunk", "msg_chunk", &manifest),
        &chunk,
    )
    .unwrap();

    let manifest_delivery = file_manifest_delivery_json(&encrypted_manifest);
    let chunk_delivery = file_chunk_delivery_json(&encrypted_chunk);
    assert_eq!(manifest_delivery["metadataEncrypted"], true);
    assert_eq!(manifest_delivery["bodyRedacted"], true);
    assert_eq!(chunk_delivery["metadataEncrypted"], true);
    assert_eq!(chunk_delivery["bodyRedacted"], true);

    let serialized = serde_json::to_string(&json!({
        "manifest": manifest_delivery,
        "chunk": chunk_delivery
    }))
    .unwrap();
    for forbidden in [
        "file-sensitive-id-canary",
        "user-tax-return-secret-canary.pdf",
        "application/x-secret-canary",
        "secret-path-canary",
        "file-body-plaintext-secret-canary-content",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "server-visible file delivery leaked {forbidden}"
        );
    }
    assert!(serialized.contains("fileIdHash"));
    assert!(serialized.contains("ciphertextHash"));
    assert!(serialized.contains("ciphertextSize"));
}
