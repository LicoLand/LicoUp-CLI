use super::store::mobile_relay_pairwise_store;
use crate::core::secure_mesh_pairwise::{
    SecureMeshLocalPreKeyUse, SecureMeshPairwiseSession, SecureMeshRemotePreKeyUse,
};
use crate::core::secure_mesh_pqxdh::ML_KEM_1024_PUBLIC_KEY_BYTES;
use crate::core::secure_mesh_prekey::SecureMeshPreKeyValidationPolicy;
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
use crate::domain::mobile_relay::endpoint_trust::{
    authorize_peer_pairwise_directory, decode_fixed_base64url, decode_key_32, ensure_peer_verified,
    local_endpoint_state, now_iso, pairwise_accepted_from_descriptor, pairwise_accepted_to_json,
    pairwise_finished_from_descriptor, pairwise_finished_to_json, pairwise_intro_from_descriptor,
    pairwise_intro_to_json, pairwise_prekey_bundle_from_descriptor, peer_endpoint_state,
    prekey_public_key_hash, rotate_mobile_relay_one_time_prekeys, session_id,
    validate_pairwise_intro_targets_local_prekeys,
};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretMaterial, ensure_secure_mesh_protected_operation_allowed,
    selected_mobile_relay_capability_evaluation,
};
use crate::platform::secure_mesh_secret_store::SecretStoreAuthorizationRequest;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn initialize_mobile_relay_pairwise_session(
    config: &mut Value,
    secret_material: &mut RuntimeSecretMaterial,
    peer_descriptor: &Value,
    peer_identity: &DeviceTrustPublicIdentity,
) -> Result<()> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let mut store = mobile_relay_pairwise_store()?;
    let endpoint = local_endpoint_state(config, secret_material)?;
    let peer = peer_endpoint_state(config)?;
    let session_id = session_id(config)?;
    let capability_evaluation = selected_mobile_relay_capability_evaluation()?;
    if let Some(record) = store.read_record(&session_id, &endpoint.endpoint_id)? {
        if let Some(finished) = pairwise_finished_from_descriptor(peer_descriptor)? {
            if finished.responder_endpoint_id == endpoint.endpoint_id
                && finished.initiator_endpoint_id == peer.endpoint_id
            {
                let secret_store_session =
                    store.begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                        "Mobile Relay pairwise finished authorization batch",
                        3,
                    ))?;
                let mut session = store
                    .load_session_with_authorized_session(
                        &session_id,
                        &endpoint.endpoint_id,
                        &secret_store_session,
                    )?
                    .ok_or_else(|| anyhow!("mobile relay pairwise session record is missing"))?;
                session.complete_responder_handshake(&finished)?;
                store.commit_session_with_authorized_session(
                    &record,
                    &session,
                    now_iso(),
                    &secret_store_session,
                )?;
                if let Some(e2ee) = config
                    .get_mut("mobileRelayE2ee")
                    .and_then(Value::as_object_mut)
                {
                    e2ee.remove("pairwiseAccepted");
                }
                return Ok(());
            }
        }
        if let Some(accepted) = pairwise_accepted_from_descriptor(peer_descriptor)? {
            let secret_store_session =
                store.begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                    "Mobile Relay pairwise handshake authorization batch",
                    3,
                ))?;
            let mut session = store
                .load_session_with_authorized_session(
                    &session_id,
                    &endpoint.endpoint_id,
                    &secret_store_session,
                )?
                .ok_or_else(|| anyhow!("mobile relay pairwise session record is missing"))?;
            if session.remote_endpoint_id == accepted.responder_endpoint_id {
                let local_identity = endpoint.device_identity()?;
                let now = OffsetDateTime::now_utc();
                let finished = session.complete_initiator_handshake(
                    &local_identity,
                    peer_identity,
                    &accepted,
                    now,
                    &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(),
                )?;
                store.commit_session_with_authorized_session_and_capability_proofs(
                    &record,
                    &session,
                    session.local_capability_proof(),
                    &accepted.responder_capability_proof,
                    now.unix_timestamp(),
                    now_iso(),
                    &secret_store_session,
                )?;
                config["mobileRelayE2ee"]["pairwiseFinished"] =
                    pairwise_finished_to_json(&finished);
                if let Some(e2ee) = config
                    .get_mut("mobileRelayE2ee")
                    .and_then(Value::as_object_mut)
                {
                    e2ee.remove("pendingPairwiseIntro");
                }
            }
        }
        return Ok(());
    }

    if let Some(intro) = pairwise_intro_from_descriptor(peer_descriptor)? {
        if intro.responder_endpoint_id == endpoint.endpoint_id
            && intro.initiator_endpoint_id == peer.endpoint_id
        {
            let local_identity = endpoint.device_identity()?;
            validate_pairwise_intro_targets_local_prekeys(
                config,
                &endpoint,
                &local_identity,
                peer_identity,
                &intro,
            )?;
            let local_identity_secret = endpoint.identity_secret()?;
            let local_signing_key = endpoint.signing_key()?;
            let signed_prekey_secret = endpoint.signed_prekey_secret()?;
            let one_time_prekey_secret = endpoint
                .one_time_prekey_secret_for(intro.responder_one_time_prekey_id.as_deref())?;
            let one_time_mlkem1024_prekey_seed = endpoint.one_time_mlkem1024_prekey_seed_for(
                &intro.responder_one_time_mlkem1024_prekey_id,
            )?;
            let now = OffsetDateTime::now_utc();
            let (session, accepted) = SecureMeshPairwiseSession::accept(
                &local_identity,
                &local_identity_secret,
                &local_signing_key,
                peer_identity,
                &signed_prekey_secret,
                one_time_prekey_secret.as_ref(),
                &one_time_mlkem1024_prekey_seed,
                &intro,
                &capability_evaluation,
                now,
                &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(),
            )?;
            config["mobileRelayE2ee"]["sessionId"] = json!(session.session_id.clone());
            config["mobileRelayE2ee"]["pairwiseAccepted"] = pairwise_accepted_to_json(&accepted);
            if let Some(e2ee) = config
                .get_mut("mobileRelayE2ee")
                .and_then(Value::as_object_mut)
            {
                e2ee.remove("pendingPairwiseIntro");
                e2ee.remove("pairwiseFinished");
            }
            let local_prekey_use = SecureMeshLocalPreKeyUse {
                local_endpoint_id: endpoint.endpoint_id.clone(),
                local_identity_fingerprint: local_identity.fingerprint()?,
                one_time_prekey_id: endpoint.one_time_prekey_id.clone(),
                one_time_prekey_public_key_hash: prekey_public_key_hash(&decode_key_32(
                    &endpoint.one_time_prekey_public_key,
                    "mobile relay local one-time prekey public key",
                )?),
                one_time_mlkem1024_prekey_id: endpoint.one_time_mlkem1024_prekey_id.clone(),
                one_time_mlkem1024_prekey_public_key_hash: prekey_public_key_hash(
                    &decode_fixed_base64url::<ML_KEM_1024_PUBLIC_KEY_BYTES>(
                        &endpoint.one_time_mlkem1024_prekey_public_key,
                        "mobile relay local ML-KEM-1024 one-time prekey public key",
                    )?,
                ),
            };
            store.upsert_initial_with_local_prekey_claim_and_capability_proofs(
                &session,
                &local_prekey_use,
                &accepted.responder_capability_proof,
                &intro.initiator_capability_proof,
                now.unix_timestamp(),
                now_iso(),
            )?;
            rotate_mobile_relay_one_time_prekeys(config, secret_material)?;
            return Ok(());
        }
    }

    if endpoint.endpoint_kind == "mobile" {
        let local_identity = endpoint.device_identity()?;
        let local_identity_secret = endpoint.identity_secret()?;
        let local_signing_key = endpoint.signing_key()?;
        ensure_peer_verified(config)?;
        let mut remote_bundle = pairwise_prekey_bundle_from_descriptor(peer_descriptor)?;
        ensure!(
            remote_bundle.endpoint_identity == *peer_identity,
            "mobile relay pairwise prekey identity does not match pinned peer"
        );
        remote_bundle.trust_state = DeviceTrustState::Verified;
        let remote_directory_authorization = authorize_peer_pairwise_directory(
            config,
            peer_descriptor,
            &remote_bundle,
            OffsetDateTime::now_utc(),
        )?;
        let (session, intro) = SecureMeshPairwiseSession::initiate(
            &local_identity,
            &local_identity_secret,
            &local_signing_key,
            &remote_bundle,
            &remote_directory_authorization,
            &SecureMeshPreKeyValidationPolicy::default(),
            &capability_evaluation,
            OffsetDateTime::now_utc(),
        )?;
        let one_time_prekey_public_key = remote_bundle
            .one_time_prekey
            .as_ref()
            .ok_or_else(|| anyhow!("mobile relay pairwise one-time prekey is missing"))?
            .public_key
            .as_slice();
        let one_time_mlkem1024_prekey_public_key = remote_bundle
            .one_time_mlkem1024_prekey
            .public_key
            .as_slice();
        let remote_prekey_use = SecureMeshRemotePreKeyUse {
            session_id: session.session_id.clone(),
            local_endpoint_id: session.local_endpoint_id.clone(),
            remote_endpoint_id: remote_bundle.endpoint_identity.endpoint_id.clone(),
            remote_identity_fingerprint: remote_bundle.endpoint_identity.fingerprint()?,
            signed_prekey_id: intro.responder_signed_prekey_id.clone(),
            one_time_prekey_id: intro
                .responder_one_time_prekey_id
                .clone()
                .ok_or_else(|| anyhow!("mobile relay pairwise intro missing one-time prekey id"))?,
            one_time_prekey_public_key_hash: prekey_public_key_hash(one_time_prekey_public_key),
            one_time_mlkem1024_prekey_id: intro.responder_one_time_mlkem1024_prekey_id.clone(),
            one_time_mlkem1024_prekey_public_key_hash: prekey_public_key_hash(
                one_time_mlkem1024_prekey_public_key,
            ),
            directory_authorization_digest: intro.directory_authorization_digest.clone(),
        };
        config["mobileRelayE2ee"]["sessionId"] = json!(session.session_id.clone());
        config["mobileRelayE2ee"]["pendingPairwiseIntro"] = pairwise_intro_to_json(&intro);
        if let Some(e2ee) = config
            .get_mut("mobileRelayE2ee")
            .and_then(Value::as_object_mut)
        {
            e2ee.remove("pairwiseAccepted");
            e2ee.remove("pairwiseFinished");
        }
        store.upsert_initial_with_remote_prekey_claim(&session, &remote_prekey_use, now_iso())?;
        return Ok(());
    }

    Err(anyhow!(
        "mobile relay peer secure mesh descriptor does not contain a PQXDH pairwise intro"
    ))
}
