use crate::domain::mobile_relay::endpoint_trust::{
    current_secure_mesh_kt_gate_epoch_seconds, is_peer_trust_record_verified,
    local_endpoint_public_descriptor, local_endpoint_state, peer_endpoint_state,
    redacted_pairing_invite, require_current_pairwise_directory_authority,
};
use crate::domain::mobile_relay::pairwise_session::{
    AuthorizedPairwiseSessionStatus, authorized_pairwise_session_status,
};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretOverrides, kt_authority_reset_in_progress, load_config_for_read,
    load_config_with_runtime_secret_context_for_operation,
    mobile_relay_e2ee_secret_store_authorization_batch_operation_count,
    mobile_relay_e2ee_secret_store_status, should_authorize_secret_read,
};
use crate::domain::mobile_relay::support::{
    CONFIG_SCHEMA_VERSION, MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
};
use anyhow::Result;
use serde_json::{Value, json};

pub fn e2ee_status(params: &Value) -> Result<Value> {
    let read_authorized = should_authorize_secret_read(params);
    let mut authorized_context = None;
    let mut unauthorized_overrides = RuntimeSecretOverrides::default();
    let config = if read_authorized {
        let (config, context) = load_config_with_runtime_secret_context_for_operation(
            params,
            "Mobile Relay E2EE status authorization batch",
            mobile_relay_e2ee_secret_store_authorization_batch_operation_count().saturating_add(2),
        )?;
        authorized_context = Some(context);
        config
    } else {
        let (config, overrides) = load_config_for_read(params)?;
        unauthorized_overrides = overrides;
        config
    };
    let local = if read_authorized {
        local_endpoint_state(
            &config,
            &authorized_context
                .as_ref()
                .expect("authorized context exists")
                .material,
        )
        .ok()
        .map(|endpoint| endpoint.public_descriptor())
        .transpose()?
    } else {
        local_endpoint_public_descriptor(&config).ok()
    };
    let peer = peer_endpoint_state(&config).ok();
    let peer_verified_flag = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("peerVerified"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let peer_trust_record_verified = peer_verified_flag && is_peer_trust_record_verified(&config);
    let authority_reset_in_progress = kt_authority_reset_in_progress().unwrap_or(true);
    let pairwise_directory_freshness = current_secure_mesh_kt_gate_epoch_seconds()
        .and_then(|now| require_current_pairwise_directory_authority(&config, now));
    let pairwise_directory_fresh = pairwise_directory_freshness.is_ok();
    let pairwise_status = if let Some(context) = authorized_context.as_mut() {
        authorized_pairwise_session_status(&config, context)
    } else {
        AuthorizedPairwiseSessionStatus::blocked(
            "pairwise_session_verification_requires_authorization",
        )
    };
    let secret_overrides = authorized_context
        .as_ref()
        .map(|context| &context.overrides)
        .unwrap_or(&unauthorized_overrides);
    let mut secret_store = mobile_relay_e2ee_secret_store_status(&config, secret_overrides);
    if let Some(object) = secret_store.as_object_mut() {
        object.insert("fullStatusAuthorized".to_string(), json!(read_authorized));
        object.insert(
            "authorizationRequiredForFullStatus".to_string(),
            json!(!read_authorized),
        );
    }
    let mandatory_foundation_complete = secret_store
        .get("capabilityReport")
        .and_then(|report| report.get("mandatoryFoundationComplete"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let custody_operational = secret_store
        .get("custodyOperational")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let secure_session_established = local.is_some()
        && peer.is_some()
        && peer_trust_record_verified
        && mandatory_foundation_complete
        && custody_operational
        && pairwise_directory_fresh
        && !authority_reset_in_progress
        && pairwise_status.established;
    let mut blockers = Vec::new();
    if local.is_none() {
        blockers.push("local_endpoint_unavailable");
    }
    if peer.is_none() {
        blockers.push("peer_endpoint_unavailable");
    }
    if !peer_trust_record_verified {
        blockers.push("peer_trust_not_verified");
    }
    if !mandatory_foundation_complete {
        blockers.push("mandatory_capability_foundation_incomplete");
    }
    if !custody_operational {
        blockers.push("safe_secret_custody_not_operational");
    }
    if !pairwise_directory_fresh {
        blockers.push("key_transparency_label_refresh_required");
    }
    if authority_reset_in_progress {
        blockers.push("key_transparency_authority_reset_incomplete");
    }
    if let Some(blocker) = pairwise_status.blocker {
        blockers.push(blocker);
    }
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "local": local,
        "peer": peer.map(|endpoint| json!({
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "endpointId": endpoint.endpoint_id,
            "endpointKind": endpoint.endpoint_kind,
            "fingerprint": endpoint.fingerprint
        })),
        "peerVerified": peer_trust_record_verified,
        "peerVerifiedFlag": peer_verified_flag,
        "peerTrustRecordVerified": peer_trust_record_verified,
        "secretStore": secret_store,
        "fullStatusAuthorized": read_authorized,
        "authorizationRequiredForFullStatus": !read_authorized,
        "mandatoryFoundationComplete": mandatory_foundation_complete,
        "secureSessionEstablished": secure_session_established,
        "keyTransparencyFresh": pairwise_directory_fresh,
        "keyTransparencyFreshness": pairwise_directory_freshness.ok().map(|freshness| json!({
            "treeSize": freshness.tree_size,
            "expiresAtEpochSeconds": freshness.expires_at_epoch_seconds,
            "labelBound": true,
            "purposeBound": true,
            "proofReverifiedFromAuthorityState": true
        })),
        "keyTransparencyAuthorityResetInProgress": authority_reset_in_progress,
        "capabilityProjection": pairwise_status.capability_projection,
        "blockers": blockers,
        "pairingInvite": redacted_pairing_invite(config.get("mobileRelayPairingInvite"))
    }))
}
