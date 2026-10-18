use super::SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION;
use super::decision::{
    device_trust_decision_code, trusted_for_sensitive_use, usable_for_read_only,
};
use super::identity::detect_identity_key_change;
use super::input::{
    device_identity_from_json, device_identity_param, provided_sas_text, read_bool,
    trust_state_from_json, trust_state_label,
};
use super::model::DeviceTrustState;
use super::verification::{qr_verification_payload, sas_decimal_chunks};
use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};

pub fn evaluate_device_trust_policy_json(params: &Value) -> Result<Value> {
    let identity_value = params
        .get("identity")
        .or_else(|| params.get("deviceIdentity"))
        .or_else(|| params.get("currentIdentity"))
        .ok_or_else(|| anyhow!("secure mesh device trust identity is required"))?;
    let identity = device_identity_from_json(identity_value)?;
    let previous_identity = params
        .get("previousIdentity")
        .and_then(|value| {
            if value.is_null() {
                None
            } else {
                Some(device_identity_from_json(value))
            }
        })
        .transpose()?;
    let requested_trust_state = trust_state_from_json(params)?;
    let key_change_state = previous_identity
        .as_ref()
        .map(|previous| detect_identity_key_change(previous, &identity))
        .transpose()?;
    // This stateless evaluator receives caller-controlled JSON and therefore cannot establish a
    // trust root. Positive authorization is owned by the locally persisted, locally signed trust
    // record consumed by the product route. Caller input may only reduce access here.
    let effective_trust_state = match (key_change_state.as_ref(), &requested_trust_state) {
        (Some(DeviceTrustState::KeyChanged), _) | (_, DeviceTrustState::KeyChanged) => {
            DeviceTrustState::KeyChanged
        }
        (_, DeviceTrustState::Revoked) => DeviceTrustState::Revoked,
        _ => DeviceTrustState::Unverified,
    };
    let require_verified_device = read_bool(params, "requireVerifiedDevice", true);
    let allow_unverified_read_only = read_bool(params, "allowUnverifiedReadOnly", false);
    let allowed_for_prekey = if require_verified_device {
        trusted_for_sensitive_use(&effective_trust_state)
    } else {
        usable_for_read_only(&effective_trust_state, allow_unverified_read_only)
    };
    let allowed_for_high_risk = trusted_for_sensitive_use(&effective_trust_state);
    let allowed_for_read_only =
        usable_for_read_only(&effective_trust_state, allow_unverified_read_only);
    let decision_code = device_trust_decision_code(
        &effective_trust_state,
        require_verified_device,
        allow_unverified_read_only,
    );
    Ok(json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "endpointId": identity.endpoint_id,
        "fingerprint": identity.fingerprint()?,
        "trustState": trust_state_label(&effective_trust_state),
        "requestedTrustState": trust_state_label(&requested_trust_state),
        "keyChangeDetected": key_change_state == Some(DeviceTrustState::KeyChanged),
        "policy": {
            "requireVerifiedDevice": require_verified_device,
            "allowUnverifiedReadOnly": allow_unverified_read_only,
            "positiveAuthorizationSource": "persisted_local_signed_trust_record_only",
            "callerSuppliedTrustStateIsAdvisory": true
        },
        "decision": {
            "allowedForPrekey": allowed_for_prekey,
            "allowedForHighRiskCommand": allowed_for_high_risk,
            "allowedForReadOnlyCommand": allowed_for_read_only,
            "requiresUserVerification": !allowed_for_high_risk,
            "code": decision_code
        }
    }))
}

pub fn evaluate_device_trust_verification_json(
    params: &Value,
    verification_method: &str,
) -> Result<Value> {
    let local = device_identity_param(params, &["localIdentity", "firstIdentity", "leftIdentity"])?;
    let peer = device_identity_param(
        params,
        &["peerIdentity", "remoteIdentity", "secondIdentity"],
    )?;
    let roster_epoch = params
        .get("rosterEpoch")
        .or_else(|| params.get("roster_epoch"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let sas = sas_decimal_chunks(&local, &peer)?;
    let sas_text = sas.join("-");
    let qr_payload = qr_verification_payload(&local, &peer, roster_epoch)?;
    let observation_matched = match verification_method {
        "qr" => params
            .get("qrPayload")
            .or_else(|| params.get("qr_payload"))
            .and_then(Value::as_str)
            .is_some_and(|provided| provided.trim() == qr_payload),
        "sas" => provided_sas_text(params).is_some_and(|provided| provided == sas_text),
        _ => false,
    };
    Ok(json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "method": verification_method,
        "localEndpointId": local.endpoint_id,
        "peerEndpointId": peer.endpoint_id,
        "localFingerprint": local.fingerprint()?,
        "peerFingerprint": peer.fingerprint()?,
        "sas": sas,
        "qrPayload": qr_payload,
        "observationMatched": observation_matched,
        "decision": {
            "allowedForPrekey": false,
            "allowedForHighRiskCommand": false,
            "requiresUserVerification": !observation_matched,
            "requiresPersistedTrustRecord": true,
            "code": if observation_matched {
                "verification_observation_requires_persisted_trust_record"
            } else {
                "verification_required"
            }
        },
        "keyMaterial": "redacted"
    }))
}

pub fn evaluate_device_trust_lifecycle_json(params: &Value, lifecycle: &str) -> Result<Value> {
    let identity =
        device_identity_param(params, &["identity", "currentIdentity", "deviceIdentity"])?;
    let mut policy_params = params.clone();
    let object = policy_params
        .as_object_mut()
        .ok_or_else(|| anyhow!("secure mesh device trust lifecycle params must be an object"))?;
    object.insert("identity".to_string(), json!({
        "endpointId": identity.endpoint_id,
        "identityPublicKey": general_purpose::URL_SAFE_NO_PAD.encode(identity.identity_public_key),
        "signingPublicKey": general_purpose::URL_SAFE_NO_PAD.encode(identity.signing_public_key),
        "rotationEpoch": identity.rotation_epoch
    }));
    let lifecycle_status = match lifecycle {
        "rotate" => {
            object.insert("trustState".to_string(), json!("verified"));
            "rotation_reverification_required"
        }
        "revoke" => {
            object.insert("trustState".to_string(), json!("revoked"));
            "revoked"
        }
        "recover" => {
            let recovery_confirmed = read_bool(params, "recoveryConfirmed", false);
            object.insert(
                "trustState".to_string(),
                json!(if recovery_confirmed {
                    "cross_signed"
                } else {
                    "unverified"
                }),
            );
            if recovery_confirmed {
                "recovery_confirmed"
            } else {
                "recovery_confirmation_required"
            }
        }
        _ => {
            return Err(anyhow!(
                "secure mesh device trust lifecycle action is unsupported"
            ));
        }
    };
    object.insert("requireVerifiedDevice".to_string(), json!(true));
    let policy = evaluate_device_trust_policy_json(&policy_params)?;
    Ok(json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "lifecycle": lifecycle,
        "status": lifecycle_status,
        "fingerprint": policy["fingerprint"].clone(),
        "trustState": policy["trustState"].clone(),
        "keyChangeDetected": policy["keyChangeDetected"].clone(),
        "decision": policy["decision"].clone(),
        "keyMaterial": "redacted"
    }))
}
