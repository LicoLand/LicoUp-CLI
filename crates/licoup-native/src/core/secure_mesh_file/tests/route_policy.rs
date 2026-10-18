use super::support::*;

#[test]
fn secure_mesh_file_route_json_uses_default_route_without_metadata_leak() {
    let manifest = manifest_fixture();
    let route = evaluate_file_route_json(&json!({
        "manifest": manifest_json(&manifest)
    }))
    .unwrap();
    assert_eq!(
        route["route"]["uploadOperation"],
        "secure_mesh.file_chunk.upload"
    );
    assert_eq!(
        route["route"]["fetchOperation"],
        "secure_mesh.file_chunk.fetch"
    );
    assert_eq!(route["route"]["metadataEncrypted"], true);
    assert_eq!(route["transfer"]["chunkCount"], manifest.chunk_count);
    let serialized = serde_json::to_string(&route).unwrap();
    assert!(!serialized.contains(&manifest.file_name));
    assert!(!serialized.contains(&manifest.mime_type));
    assert!(!serialized.contains(&manifest.relative_path));
}
