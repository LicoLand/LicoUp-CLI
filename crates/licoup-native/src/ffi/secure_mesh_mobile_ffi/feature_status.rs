use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    fixture_envelope::native_envelope_fixture,
    fixture_file::{
        native_file_handoff_proof_fixture, native_file_receive_confirmation_fixture,
        native_file_receive_destination_fixture, native_file_route_fixture,
    },
    fixture_lifecycle::native_lifecycle_service_action_fixture,
    fixture_payload::native_payload_crypto_fixture,
    fixture_trust::native_device_trust_fixture,
};

pub const FEATURE_PROTOCOL_STATUS: i32 = 1 << 0;
pub const FEATURE_ENVELOPE_VALIDATION: i32 = 1 << 1;
pub const FEATURE_COMMAND_POLICY: i32 = 1 << 2;
pub const FEATURE_CONTENT_CRYPTO: i32 = 1 << 3;
pub const FEATURE_PAIRWISE_RUNTIME: i32 = 1 << 4;
pub const FEATURE_MLS_RUNTIME: i32 = 1 << 5;
pub const FEATURE_DEVICE_TRUST: i32 = 1 << 6;
pub const FEATURE_LIFECYCLE_SERVICE_ACTIONS: i32 = 1 << 7;
pub const EXPECTED_FEATURES: i32 = FEATURE_PROTOCOL_STATUS
    | FEATURE_ENVELOPE_VALIDATION
    | FEATURE_COMMAND_POLICY
    | FEATURE_CONTENT_CRYPTO
    | FEATURE_PAIRWISE_RUNTIME
    | FEATURE_MLS_RUNTIME
    | FEATURE_DEVICE_TRUST
    | FEATURE_LIFECYCLE_SERVICE_ACTIONS;

pub fn runtime_self_test() -> bool {
    runtime_feature_flags() == EXPECTED_FEATURES
}

pub fn runtime_feature_flags() -> i32 {
    let mut flags = 0;
    let status = crate::core::secure_mesh::protocol_status();
    if status.get("protocolVersion").and_then(Value::as_str)
        == Some(crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION)
        && status
            .get("supportedTransports")
            .and_then(Value::as_array)
            .is_some_and(|transports| transports.len() >= 5)
    {
        flags |= FEATURE_PROTOCOL_STATUS;
    }
    let allowed = crate::core::secure_mesh::command_policy(&json!({
        "commandKind": "agent.message.send"
    }));
    let denied = crate::core::secure_mesh::command_policy(&json!({
        "commandKind": "shell.exec"
    }));
    if crate::core::secure_mesh::validate_envelope(&native_envelope_fixture()).is_ok() {
        flags |= FEATURE_ENVELOPE_VALIDATION;
    }
    if allowed.get("allowed").and_then(Value::as_bool) == Some(true)
        && denied.get("allowed").and_then(Value::as_bool) == Some(false)
    {
        flags |= FEATURE_COMMAND_POLICY;
    }
    if crate::core::secure_mesh_crypto::SECURE_MESH_CONTENT_CRYPTO_STATUS
        .contains("content_and_file_aead_available")
        && native_payload_crypto_fixture().is_ok()
        && native_file_route_fixture().is_ok()
        && native_file_receive_destination_fixture().is_ok()
        && native_file_receive_confirmation_fixture().is_ok()
        && native_file_handoff_proof_fixture().is_ok()
    {
        flags |= FEATURE_CONTENT_CRYPTO;
    }
    if crate::core::secure_mesh_pairwise::SECURE_MESH_PAIRWISE_STATUS
        .contains("authenticated_transcript_pqxdh_mlkem1024_triple_ratchet")
        && status["pairwiseKem"]["parameterSet"] == "ML-KEM-1024"
        && status["pairwiseKem"]["standard"] == "FIPS 203"
        && status["pairwiseKem"]["publicKeyBytes"]
            == crate::core::secure_mesh_pqxdh::ML_KEM_1024_PUBLIC_KEY_BYTES
        && status["pairwiseKem"]["ciphertextBytes"]
            == crate::core::secure_mesh_pqxdh::ML_KEM_1024_CIPHERTEXT_BYTES
        && crate::core::secure_mesh_pairwise::runtime_crypto_self_test()
    {
        flags |= FEATURE_PAIRWISE_RUNTIME;
    }
    if crate::domain::secure_mesh_mls::runtime_binding_wired()
        && crate::core::secure_mesh_mls::runtime_crypto_self_test()
    {
        flags |= FEATURE_MLS_RUNTIME;
    }
    if crate::core::secure_mesh_trust::SECURE_MESH_DEVICE_TRUST_STATUS
        .contains("fingerprint_60_digit_safety_number_qr_policy_cli_gui_available")
        && native_device_trust_fixture().is_ok()
    {
        flags |= FEATURE_DEVICE_TRUST;
    }
    if crate::core::secure_mesh_lifecycle::SECURE_MESH_LIFECYCLE_STATUS
        .contains("ttl_delete_screenshot_resend_ack_purge")
        && native_lifecycle_service_action_fixture().is_ok()
    {
        flags |= FEATURE_LIFECYCLE_SERVICE_ACTIONS;
    }
    flags
}

pub fn runtime_protocol_hash() -> i32 {
    let status = crate::core::secure_mesh::protocol_status();
    let digest = Sha256::digest(status.to_string().as_bytes());
    i32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]])
}
