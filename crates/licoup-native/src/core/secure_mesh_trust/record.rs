use super::identity::DeviceTrustPublicIdentity;
use super::input::{read_text_field, read_u64_field, trust_state_from_label, trust_state_label};
use super::model::{DeviceTrustRecord, DeviceTrustState};
use super::signature::verify_device_trust_record;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub fn device_trust_record_to_json(record: &DeviceTrustRecord) -> Value {
    json!({
        "protocolVersion": record.protocol_version,
        "signerEndpointId": record.signer_endpoint_id,
        "peerEndpointId": record.peer_endpoint_id,
        "peerFingerprint": record.peer_fingerprint,
        "trustState": trust_state_label(&record.trust_state),
        "rosterEpoch": record.roster_epoch,
        "verificationMethod": record.verification_method,
        "issuedAtEpochSeconds": record.issued_at_epoch_seconds,
        "expiresAtEpochSeconds": record.expires_at_epoch_seconds,
        "signatureBase64url": record.signature
    })
}

pub fn device_trust_record_from_json(value: &Value) -> Result<DeviceTrustRecord> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh device trust record must be an object"))?;
    Ok(DeviceTrustRecord {
        protocol_version: read_text_field(object, &["protocolVersion", "protocol_version"])?,
        signer_endpoint_id: read_text_field(object, &["signerEndpointId", "signer_endpoint_id"])?,
        peer_endpoint_id: read_text_field(object, &["peerEndpointId", "peer_endpoint_id"])?,
        peer_fingerprint: read_text_field(object, &["peerFingerprint", "peer_fingerprint"])?,
        trust_state: trust_state_from_label(
            object
                .get("trustState")
                .or_else(|| object.get("trust_state"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?,
        roster_epoch: read_u64_field(object, &["rosterEpoch", "roster_epoch"], 0)?,
        verification_method: read_text_field(
            object,
            &["verificationMethod", "verification_method"],
        )?,
        issued_at_epoch_seconds: read_u64_field(
            object,
            &["issuedAtEpochSeconds", "issued_at_epoch_seconds"],
            0,
        )?,
        expires_at_epoch_seconds: read_u64_field(
            object,
            &["expiresAtEpochSeconds", "expires_at_epoch_seconds"],
            0,
        )?,
        signature: read_text_field(object, &["signatureBase64url", "signature"])?,
    })
}

pub fn verify_device_trust_record_json(
    signer_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
    record: &Value,
    now_epoch_seconds: u64,
) -> Result<DeviceTrustState> {
    verify_device_trust_record(
        signer_identity,
        peer_identity,
        &device_trust_record_from_json(record)?,
        now_epoch_seconds,
    )
}
