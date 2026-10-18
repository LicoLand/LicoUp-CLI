use serde_json::{Value, json};

pub(super) fn native_lifecycle_service_action_fixture() -> anyhow::Result<Value> {
    crate::core::secure_mesh_lifecycle::evaluate_service_action_json(&json!({
        "actionKind": "ack_purge",
        "endpointId": "mobile-native-lifecycle-endpoint",
        "fileTransferId": "mobile-native-lifecycle-file-transfer",
        "acknowledged": true,
        "transferComplete": true
    }))
}
