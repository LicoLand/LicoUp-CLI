use serde_json::{Value, json};

pub(super) fn native_file_manifest_fixture() -> Value {
    json!({
        "fileId": "file_mobile_native_fixture",
        "fileName": "mobile-native-fixture.txt",
        "mimeType": "text/plain",
        "relativePath": "mobile/native",
        "totalSize": 16,
        "chunkSize": 8,
        "chunkCount": 2
    })
}

pub(super) fn native_file_route_fixture() -> anyhow::Result<Value> {
    crate::core::secure_mesh_file::evaluate_file_route_json(&json!({
        "manifest": native_file_manifest_fixture()
    }))
}

pub(super) fn native_file_receive_destination_fixture() -> anyhow::Result<Value> {
    crate::core::secure_mesh_file::evaluate_file_receive_destination_json(&json!({
        "manifest": native_file_manifest_fixture(),
        "approvedRoot": std::env::temp_dir().to_string_lossy()
    }))
}

pub(super) fn native_file_receive_confirmation_fixture() -> anyhow::Result<Value> {
    crate::core::secure_mesh_file::evaluate_file_receive_confirmation_json(&json!({
        "manifest": native_file_manifest_fixture(),
        "approvedRoot": std::env::temp_dir().to_string_lossy()
    }))
}

pub(super) fn native_file_handoff_proof_fixture() -> anyhow::Result<Value> {
    crate::core::secure_mesh_file::evaluate_file_handoff_proof_json(&json!({}))
}
