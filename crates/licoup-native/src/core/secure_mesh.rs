use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::core::secure_mesh_acp::SECURE_MESH_ACP_STATUS;
use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvidenceKind, capability_catalog, mandatory_protocol_facts,
};
use crate::core::secure_mesh_capability_proof::ClientCapabilityProjection;
use crate::core::secure_mesh_command::SECURE_MESH_COMMAND_SECURITY_STATUS;
use crate::core::secure_mesh_crypto::{
    SECURE_MESH_CONTENT_CIPHER_SUITE, SECURE_MESH_CONTENT_CRYPTO_STATUS,
};
use crate::core::secure_mesh_file::{
    SECURE_MESH_FILE_CHUNK_CONTENT_TYPE, SECURE_MESH_FILE_CRYPTO_STATUS,
    SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE,
};
use crate::core::secure_mesh_lifecycle::SECURE_MESH_LIFECYCLE_STATUS;
use crate::core::secure_mesh_mls::{SECURE_MESH_MLS_CIPHER_SUITE, SECURE_MESH_MLS_STATUS};
use crate::core::secure_mesh_pairwise::{
    SECURE_MESH_PAIRWISE_CIPHER_SUITE, SECURE_MESH_PAIRWISE_STATUS,
};
use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_CIPHERTEXT_BYTES, ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
    ML_KEM_1024_PRIVATE_KEY_BYTES, ML_KEM_1024_PUBLIC_KEY_BYTES, ML_KEM_1024_SHARED_SECRET_BYTES,
    SECURE_MESH_PQXDH_CIPHER_SUITE,
};
use crate::core::secure_mesh_prekey::{
    SECURE_MESH_KEYPACKAGE_PROTOCOL_VERSION, SECURE_MESH_KEYPACKAGE_WIRE_CIPHER_SUITE,
    SECURE_MESH_PREKEY_PROTOCOL_VERSION, SECURE_MESH_PREKEY_STATUS,
};
use crate::core::secure_mesh_product_readiness::SecureMeshProductReadiness;
use crate::core::secure_mesh_response::{
    SECURE_MESH_ERROR_CONTENT_TYPE, SECURE_MESH_RESPONSE_CRYPTO_STATUS,
    SECURE_MESH_RESULT_CONTENT_TYPE,
};
use crate::core::secure_mesh_transparency::SECURE_MESH_TRANSPARENCY_STATUS;
use crate::core::secure_mesh_trust::{
    SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION, SECURE_MESH_DEVICE_TRUST_STATUS,
};
use licoup_protocol_bindings::licoarc_relay::{
    LICOARC_RELAY_CONTRACT_VERSION, LicoArcRelayEnvelope,
};

pub const SECURE_MESH_PROTOCOL_VERSION: &str = "licomesh.secure-mesh.v1";
/// Stable wire/security compatibility profile revision.
///
/// This changes only when protocol or security semantics become incompatible. Application
/// versions and release-artifact identity are deliberately not part of session negotiation.
pub const SECURE_MESH_PROTOCOL_BUILD_REVISION: u64 = 5;
pub const SECURE_MESH_COMMAND_PROTOCOL_VERSION: &str = "licomesh.secure-mesh.command.v1";
pub const SECURE_MESH_RESULT_PROTOCOL_VERSION: &str = "licomesh.secure-mesh.result.v1";
pub const SECURE_MESH_FILE_PROTOCOL_VERSION: &str = "licomesh.secure-mesh.file.v1";

pub(crate) const ALLOWED_COMMANDS: &[&str] = &[
    "agent.sessions.list",
    "agent.sessions.describe",
    "agent.message.send",
    "secure_mesh.device.verify",
    "secure_mesh.approval.request",
    "secure_mesh.approval.response",
];

pub(crate) const DENIED_PREFIXES: &[&str] = &[
    "shell.",
    "filesystem.",
    "process.spawn.",
    "runtime.raw.",
    "mcp.raw.",
    "settings.write.",
    "secrets.",
    "network.raw.",
    "external.unscoped.",
    "tool.unbounded.",
];

pub fn protocol_status() -> Value {
    let capability_evaluation = mandatory_protocol_facts(CapabilityEvidenceKind::SourceContract)
        .and_then(|facts| capability_catalog()?.evaluate(&facts));
    capability_evaluation
        .as_ref()
        .ok()
        .and_then(|evaluation| protocol_status_with_capability_evaluation(evaluation).ok())
        .unwrap_or_else(|| {
            protocol_status_with_capability_values(
                Value::Null,
                Value::Null,
                &SecureMeshProductReadiness::missing_evidence(),
            )
        })
}

/// Projects a caller-selected local custody evaluation into the public client status.
///
/// Callers must obtain the evaluation from the actually selected platform secret store.
/// This function performs no authorization, key access, or platform probing itself.
pub fn protocol_status_with_capability_evaluation(
    evaluation: &CapabilityEvaluation,
) -> Result<Value> {
    protocol_status_with_capability_evaluation_and_readiness(
        evaluation,
        &SecureMeshProductReadiness::missing_evidence(),
    )
}

pub fn protocol_status_with_capability_evaluation_and_readiness(
    evaluation: &CapabilityEvaluation,
    product_readiness: &SecureMeshProductReadiness,
) -> Result<Value> {
    evaluation.require_mandatory_foundation()?;
    let report = serde_json::to_value(evaluation.report())?;
    let projection = serde_json::to_value(ClientCapabilityProjection::local_only(evaluation))?;
    Ok(protocol_status_with_capability_values(
        report,
        projection,
        product_readiness,
    ))
}

fn protocol_status_with_capability_values(
    capability_report: Value,
    capability_projection: Value,
    product_readiness: &SecureMeshProductReadiness,
) -> Value {
    let product_readiness = product_readiness.to_status_json();
    let mut status = json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_PROTOCOL_VERSION,
        "capabilityReport": capability_report,
        "capabilityProjection": capability_projection,
        "commandProtocolVersion": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "resultProtocolVersion": SECURE_MESH_RESULT_PROTOCOL_VERSION,
        "fileProtocolVersion": SECURE_MESH_FILE_PROTOCOL_VERSION,
        "deviceTrustProtocolVersion": SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "supportedEndpointKinds": [
            "desktop_gui",
            "desktop_sidecar",
            "mobile",
            "cli",
            "agent_host",
            "web_limited"
        ],
        "supportedTransports": [
            "cloud_relay",
            "mobile_relay_pairwise",
            "lan_direct",
            "webrtc_data_channel",
            "loopback_local"
        ],
        "productionAdvertisedTransports": [
            "cloud_relay",
            "mobile_relay_pairwise"
        ],
        "productionUnavailableTransports": [
            "lan_direct",
            "webrtc_data_channel",
            "loopback_local"
        ],
        "transportProductionReadiness": {
            "cloud_relay": {
                "status": "partial_until_physical_release_matrix",
                "productionAvailable": true,
                "advertisedForProduction": true,
                "failClosed": true
            },
            "mobile_relay_pairwise": {
                "status": "partial_until_physical_release_matrix",
                "productionAvailable": true,
                "advertisedForProduction": true,
                "failClosed": true
            },
            "lan_direct": {
                "status": "fail_closed_unavailable",
                "productionAvailable": false,
                "advertisedForProduction": false,
                "failClosed": true,
                "reason": "lan_transport_verifier_pending"
            },
            "webrtc_data_channel": {
                "status": "fail_closed_unavailable",
                "productionAvailable": false,
                "advertisedForProduction": false,
                "failClosed": true,
                "reason": "webrtc_transport_verifier_pending"
            },
            "loopback_local": {
                "status": "local_diagnostics_only_not_remote_production",
                "productionAvailable": false,
                "advertisedForProduction": false,
                "failClosed": true,
                "reason": "loopback_local_not_a_remote_client_transport"
            }
        },
        "allowedCommands": ALLOWED_COMMANDS,
        "deniedPrefixes": DENIED_PREFIXES,
        "contentCipherSuite": SECURE_MESH_CONTENT_CIPHER_SUITE,
        "contentCryptoStatus": SECURE_MESH_CONTENT_CRYPTO_STATUS,
        "fileManifestContentType": SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE,
        "fileChunkContentType": SECURE_MESH_FILE_CHUNK_CONTENT_TYPE,
        "fileCryptoStatus": SECURE_MESH_FILE_CRYPTO_STATUS,
        "resultContentType": SECURE_MESH_RESULT_CONTENT_TYPE,
        "errorContentType": SECURE_MESH_ERROR_CONTENT_TYPE,
        "responseCryptoStatus": SECURE_MESH_RESPONSE_CRYPTO_STATUS,
        "mlsCipherSuite": SECURE_MESH_MLS_CIPHER_SUITE,
        "mlsCryptoStatus": SECURE_MESH_MLS_STATUS,
        "mlsLibraryAvailableForDiagnostics": true,
        "pairwiseCipherSuite": SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "pairwiseCryptoStatus": SECURE_MESH_PAIRWISE_STATUS,
        "pairwiseKem": {
            "algorithm": "ML-KEM",
            "parameterSet": "ML-KEM-1024",
            "standard": "FIPS 203",
            "pqxdhCipherSuite": SECURE_MESH_PQXDH_CIPHER_SUITE,
            "keyGenerationSeedBytes": ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
            "publicKeyBytes": ML_KEM_1024_PUBLIC_KEY_BYTES,
            "privateKeyBytes": ML_KEM_1024_PRIVATE_KEY_BYTES,
            "ciphertextBytes": ML_KEM_1024_CIPHERTEXT_BYTES,
            "sharedSecretBytes": ML_KEM_1024_SHARED_SECRET_BYTES
        },
        "prekeyProtocolVersion": SECURE_MESH_PREKEY_PROTOCOL_VERSION,
        "keyPackageProtocolVersion": SECURE_MESH_KEYPACKAGE_PROTOCOL_VERSION,
        "keyPackageWireCipherSuite": SECURE_MESH_KEYPACKAGE_WIRE_CIPHER_SUITE,
        "prekeyStatus": SECURE_MESH_PREKEY_STATUS,
        "deviceTrustStatus": SECURE_MESH_DEVICE_TRUST_STATUS,
        "transparencyStatus": SECURE_MESH_TRANSPARENCY_STATUS,
        "commandSecurityStatus": SECURE_MESH_COMMAND_SECURITY_STATUS,
        "lifecycleStatus": SECURE_MESH_LIFECYCLE_STATUS,
        "acpEnvelopeStatus": SECURE_MESH_ACP_STATUS,
        "cryptoCoreStatus": "pairwise_mlkem1024_triple_ratchet_mls_mlkem1024_epoch_hybrid_payload_identity_bound_authorized_directory_kt"
    });
    if let Some(object) = status.as_object_mut() {
        object.insert(
            "mlsMlKem1024EpochContributionReady".to_string(),
            Value::Bool(true),
        );
        object.insert(
            "mlsHybridPayloadKeyDerivationReady".to_string(),
            Value::Bool(true),
        );
        object.insert(
            "mlsProductMessagingAvailable".to_string(),
            product_readiness["productMessagingAvailable"].clone(),
        );
        object.insert(
            "mlsSelectedTargetReleaseReady".to_string(),
            product_readiness["selectedTargetReleaseReady"].clone(),
        );
        object.insert(
            "mlsProductionClaimReady".to_string(),
            product_readiness["productionClaimReady"].clone(),
        );
        object.insert("mlsProductReadiness".to_string(), product_readiness);
    }
    status
}

pub fn validate_envelope(envelope: &Value) -> Result<Value> {
    let wire = serde_json::to_string(envelope)
        .context("secure mesh relay envelope serialization failed")?;
    let canonical = LicoArcRelayEnvelope::from_json(&wire)?;
    Ok(json!({
        "ok": true,
        "contractVersion": LICOARC_RELAY_CONTRACT_VERSION,
        "envelope": serde_json::from_str::<Value>(&canonical.to_json()?)?
    }))
}

pub fn command_policy(params: &Value) -> Value {
    let command_kind = params
        .get("commandKind")
        .or_else(|| params.get("command"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let denied_prefix = DENIED_PREFIXES
        .iter()
        .find(|prefix| command_kind.starts_with(**prefix))
        .copied()
        .unwrap_or_default();
    let allowed = denied_prefix.is_empty() && ALLOWED_COMMANDS.contains(&command_kind);
    let minimum_risk_class = match command_kind {
        "agent.sessions.list" | "agent.sessions.describe" => "read_only",
        "agent.message.send" => "safe_write",
        "secure_mesh.device.verify"
        | "secure_mesh.approval.request"
        | "secure_mesh.approval.response" => "local_effect",
        _ => "",
    };
    let requires_user_confirmation = matches!(
        command_kind,
        "agent.sessions.list"
            | "agent.sessions.describe"
            | "secure_mesh.device.verify"
            | "secure_mesh.approval.request"
            | "secure_mesh.approval.response"
    );
    json!({
        "ok": true,
        "commandKind": command_kind,
        "allowed": allowed,
        "deniedPrefix": denied_prefix,
        "minimumRiskClass": minimum_risk_class,
        "requiresUserConfirmation": requires_user_confirmation,
        "protocolVersion": SECURE_MESH_COMMAND_PROTOCOL_VERSION
    })
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose};

    use super::*;

    #[test]
    fn secure_mesh_envelope_rejects_extra_outer_fields() {
        let mut envelope = envelope_fixture();
        envelope["payloadKind"] = json!("command");
        assert!(validate_envelope(&envelope).is_err());
    }

    #[test]
    fn secure_mesh_envelope_rejects_the_retired_metadata_rich_shape() {
        let retired = json!({
            "protocolVersion": SECURE_MESH_PROTOCOL_VERSION,
            "envelopeId": "env_test",
            "opaqueMailboxId": "mailbox_test",
            "messageId": "msg_test",
            "cipherSuite": SECURE_MESH_PAIRWISE_CIPHER_SUITE,
            "createdAt": "2026-01-01T00:00:00.000Z",
            "expiresAt": "2026-01-01T00:10:00.000Z",
            "ciphertextSize": 32,
            "encryptedHeader": "header",
            "ciphertext": "ciphertext"
        });
        assert!(validate_envelope(&retired).is_err());
    }

    #[test]
    fn secure_mesh_command_policy_allows_only_registered_commands() {
        assert_eq!(
            command_policy(&json!({"commandKind": "agent.message.send"}))["allowed"],
            true
        );
        assert_eq!(
            command_policy(&json!({"commandKind": "agent.message.send"}))["minimumRiskClass"],
            "safe_write"
        );
        assert_eq!(
            command_policy(&json!({"commandKind": "secure_mesh.device.verify"}))["requiresUserConfirmation"],
            true
        );
        let session_list = command_policy(&json!({"commandKind": "agent.sessions.list"}));
        assert_eq!(session_list["minimumRiskClass"], "read_only");
        assert_eq!(session_list["requiresUserConfirmation"], true);
        assert_eq!(
            command_policy(&json!({"commandKind": "shell.exec"}))["allowed"],
            false
        );
    }

    #[test]
    fn secure_mesh_status_projects_the_canonical_exact_capability_report() {
        let status = protocol_status();
        assert_eq!(
            status["capabilityReport"]["mandatoryFoundationComplete"],
            true
        );
        assert!(status["capabilityReport"]["enabled"].is_array());
        assert_eq!(
            status["capabilityReport"]["custody"]["strategy"],
            "memory_only_ephemeral"
        );
        let serialized = serde_json::to_string(&status["capabilityReport"]).unwrap();
        assert!(!serialized.contains("\"tier\""));
        assert!(!serialized.contains("\"level\""));

        assert_eq!(
            status["capabilityProjection"]["schemaVersion"],
            crate::core::secure_mesh_capability_proof::CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION
        );
        assert!(status["capabilityProjection"]["local"]["enabled"].is_array());
        assert!(status["capabilityProjection"]["peer"].is_null());
        assert_eq!(
            status["capabilityProjection"]["negotiatedProtocolCapabilities"],
            json!([])
        );
        assert_eq!(
            status["capabilityProjection"]["reasons"]["peer"],
            "secure_mesh_peer_capability_proof_not_available"
        );
        let projection = serde_json::to_string(&status["capabilityProjection"]).unwrap();
        assert!(!projection.contains("\"tier\""));
        assert!(!projection.contains("\"level\""));
        assert!(!projection.contains("\"ready\""));

        assert_eq!(status["mlsProductMessagingAvailable"], false);
        assert_eq!(status["mlsSelectedTargetReleaseReady"], false);
        assert_eq!(status["mlsProductionClaimReady"], false);
        assert!(status.get("mlsProductionReady").is_none());
        assert_eq!(status["mlsProductReadiness"]["evidenceDerived"], true);
        assert!(status["mlsProductReadiness"]["sourceStateDigest"].is_null());
        assert_eq!(status["pairwiseKem"]["parameterSet"], "ML-KEM-1024");
        assert_eq!(status["pairwiseKem"]["standard"], "FIPS 203");
        assert_eq!(
            status["pairwiseKem"]["publicKeyBytes"],
            ML_KEM_1024_PUBLIC_KEY_BYTES
        );
        assert_eq!(
            status["pairwiseKem"]["ciphertextBytes"],
            ML_KEM_1024_CIPHERTEXT_BYTES
        );
    }

    fn envelope_fixture() -> Value {
        let mailbox =
            licoup_protocol_bindings::licoarc_relay::SecureMeshMailboxToken::from_base64url(
                general_purpose::URL_SAFE_NO_PAD.encode([2u8; 32]),
            )
            .unwrap();
        let envelope = licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelopeDraft::begin(
            &mailbox,
            "2030-01-01T00:00:00Z",
            256,
        )
        .and_then(|draft| {
            draft.finish(
                &[3u8; licoup_protocol_bindings::licoarc_relay::LICOARC_ENCRYPTED_HEADER_BYTES],
                &[4u8; 256],
            )
        })
        .unwrap();
        serde_json::from_str(&envelope.to_json().unwrap()).unwrap()
    }
}
