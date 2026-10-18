use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::core::secure_mesh_mls::SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION;
use crate::core::secure_mesh_mls_product::sign_mls_keypackage_capability_proof;

use super::group_state::public_local_participant;
use super::input_codec::{encode_base64url, hex_sha256, identity_to_json};
use super::participant_runtime::{ParticipantRequirement, with_local_participant};

pub(super) fn participant_ensure(params: &Value) -> Result<Value> {
    with_local_participant(params, ParticipantRequirement::CreateIfMissing, |runtime| {
        Ok((
            json!({
                "ok": true,
                "participant": public_local_participant(runtime.identity, runtime.participant)?,
                "custodyBackend": runtime.secret_store.backend()
            }),
            true,
        ))
    })
}

pub(super) fn key_package_create(params: &Value) -> Result<Value> {
    with_local_participant(params, ParticipantRequirement::CreateIfMissing, |runtime| {
        let key_package = runtime.participant.generate_key_package()?;
        let now = OffsetDateTime::now_utc();
        let capability_evaluation = runtime.secret_store.capability_evaluation()?;
        let proof = sign_mls_keypackage_capability_proof(
            runtime.identity,
            runtime.signing_key,
            &capability_evaluation,
            &key_package,
            now,
        )?;
        let key_package_id = hex_sha256(key_package.as_public_bytes());
        let previous_version = runtime
            .config
            .get("mobileRelayE2ee")
            .and_then(|state| state.get("mlsKeyPackageVersion"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let key_package_version = previous_version
            .checked_add(1)
            .ok_or_else(|| anyhow!("secure mesh MLS KeyPackage version overflow"))?;
        ensure!(
            key_package_version <= crate::core::secure_mesh_transparency::KT_JSON_SAFE_INTEGER_MAX,
            "secure mesh MLS KeyPackage version exceeds the cross-language safe range"
        );
        runtime.config["mobileRelayE2ee"]["mlsKeyPackageVersion"] = json!(key_package_version);
        runtime.config["mobileRelayE2ee"]["mlsKeyPackageDigest"] = json!(&key_package_id);
        #[cfg(test)]
        crate::domain::mobile_relay::refresh_secure_mesh_mls_test_directory_authority(
            runtime.config,
        )?;
        Ok((
            json!({
                "ok": true,
                "protocolVersion": SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION,
                "keyPackageId": key_package_id,
                "keyPackageBase64url": encode_base64url(key_package.as_public_bytes()),
                "keyPackageVersion": key_package_version,
                "capabilityProof": proof,
                "identity": identity_to_json(runtime.identity),
                "createdAtUnixSeconds": now.unix_timestamp(),
                "directoryPublicationRequired": true
            }),
            true,
        ))
    })
}
