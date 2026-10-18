use super::*;
#[cfg(test)]
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn apply_peer_secure_mesh_descriptor(
    config: &mut Value,
    secret_material: &mut RuntimeSecretMaterial,
    descriptor: &Value,
    verified: bool,
) -> Result<()> {
    let mut context = RuntimeSecretContext::default();
    std::mem::swap(&mut context.material, secret_material);
    let result =
        apply_peer_secure_mesh_descriptor_inner(config, descriptor, verified, &mut context);
    std::mem::swap(&mut context.material, secret_material);
    result
}

pub(in crate::domain::mobile_relay) fn apply_peer_secure_mesh_descriptor_with_context(
    config: &mut Value,
    descriptor: &Value,
    verified: bool,
    mut secret_context: Option<&mut RuntimeSecretContext>,
) -> Result<()> {
    let context = secret_context
        .as_deref_mut()
        .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
    apply_peer_secure_mesh_descriptor_inner(config, descriptor, verified, context)
}

fn apply_peer_secure_mesh_descriptor_inner(
    config: &mut Value,
    descriptor: &Value,
    verified: bool,
    secret_context: &mut RuntimeSecretContext,
) -> Result<()> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let endpoint_id = descriptor_text(descriptor, "endpointId")?;
    let endpoint_kind = descriptor_text(descriptor, "endpointKind")?;
    let public_key = descriptor_text(descriptor, "publicKeyBase64url")?;
    let decoded = decode_key_32(&public_key, "mobile relay peer public key")?;
    let mut candidate = config.clone();
    let local_endpoint_kind = candidate
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("endpointKind"))
        .and_then(Value::as_str)
        .unwrap_or("desktop_sidecar")
        .to_string();
    ensure_mobile_relay_endpoint_descriptor(
        &mut candidate,
        &mut secret_context.material,
        &local_endpoint_kind,
    )?;
    let local_endpoint_id = candidate
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("endpointId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if endpoint_id == local_endpoint_id {
        return Err(anyhow!(
            "mobile relay peer secure mesh descriptor points at the local endpoint"
        ));
    }
    let prior_peer_identity = candidate
        .get("mobileRelayE2ee")
        .and_then(|state| peer_device_identity_from_state(state).ok());
    let prior_peer_verified = candidate
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("peerVerified"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let first_pairing = prior_peer_identity
        .as_ref()
        .is_none_or(|prior| prior.endpoint_id != endpoint_id);
    candidate["mobileRelayE2ee"]["peerEndpointId"] = json!(endpoint_id);
    candidate["mobileRelayE2ee"]["peerEndpointKind"] = json!(endpoint_kind);
    candidate["mobileRelayE2ee"]["peerPublicKeyBase64url"] = json!(public_key);
    candidate["mobileRelayE2ee"]["peerFingerprint"] = json!(public_key_fingerprint(&decoded));
    candidate["mobileRelayE2ee"]["peerVerified"] = json!(verified);
    let peer_mailbox_rotation_epoch = descriptor
        .get("mailboxRotationEpoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("mobile relay peer mailbox rotation epoch is missing"))?;
    candidate["mobileRelayE2ee"]["peerMailboxRotationEpoch"] = json!(peer_mailbox_rotation_epoch);
    let peer_prekey_bundle = pairwise_prekey_bundle_from_descriptor(descriptor)?;
    let peer_identity = peer_prekey_bundle.endpoint_identity.clone();
    ensure!(
        peer_identity.endpoint_id == endpoint_id,
        "mobile relay peer trust identity endpoint mismatch"
    );
    ensure!(
        peer_identity.identity_public_key == decoded,
        "mobile relay peer trust identity key mismatch"
    );
    let identity_changed = prior_peer_identity.as_ref().is_some_and(|prior| {
        prior.endpoint_id == peer_identity.endpoint_id
            && (prior.identity_public_key != peer_identity.identity_public_key
                || prior.signing_public_key != peer_identity.signing_public_key)
    });
    let untrusted_directory_response: UntrustedDirectoryResponse = serde_json::from_value(
        descriptor
            .get("preKeyBundle")
            .and_then(|bundle| bundle.get("keyTransparency"))
            .cloned()
            .ok_or_else(|| anyhow!("mobile relay peer key transparency response is missing"))?,
    )
    .map_err(|_| anyhow!("mobile relay peer key transparency response is invalid"))?;
    let directory_purpose = if untrusted_directory_response.claim.revoked() {
        DirectoryAuthorizationPurpose::Revocation
    } else if identity_changed {
        DirectoryAuthorizationPurpose::IdentityKeyChange
    } else if first_pairing {
        DirectoryAuthorizationPurpose::Pairing
    } else {
        DirectoryAuthorizationPurpose::SelfMonitor
    };
    let peer_directory_authorization = authorize_peer_pairwise_directory_for_purpose(
        &candidate,
        descriptor,
        &peer_prekey_bundle,
        OffsetDateTime::now_utc(),
        directory_purpose,
    )?;
    if let Some(prior) = prior_peer_identity
        .as_ref()
        .filter(|prior| prior.endpoint_id == peer_identity.endpoint_id)
    {
        validate_peer_identity_transition(prior, &peer_identity)?;
    }
    let directory_revoked = peer_directory_authorization.claim().revoked();
    let signed_prekey_directory_authorization = if !directory_revoked && !identity_changed {
        Some(authorize_peer_pairwise_directory_for_purpose(
            &candidate,
            descriptor,
            &peer_prekey_bundle,
            OffsetDateTime::now_utc(),
            DirectoryAuthorizationPurpose::PairwiseSignedPrekey,
        )?)
    } else {
        None
    };
    let one_time_prekey_directory_authorization = if !directory_revoked && !identity_changed {
        Some(authorize_peer_pairwise_directory_for_purpose(
            &candidate,
            descriptor,
            &peer_prekey_bundle,
            OffsetDateTime::now_utc(),
            DirectoryAuthorizationPurpose::PairwiseOneTimePrekey,
        )?)
    } else {
        None
    };
    candidate["mobileRelayE2ee"]["peerSigningPublicKeyBase64url"] =
        json!(general_purpose::URL_SAFE_NO_PAD.encode(peer_identity.signing_public_key));
    candidate["mobileRelayE2ee"]["peerRotationEpoch"] = json!(peer_identity.rotation_epoch);
    candidate["mobileRelayE2ee"]["peerDeviceTrustFingerprint"] =
        json!(peer_identity.fingerprint()?);
    candidate["mobileRelayE2ee"]["peerKeyTransparencyAuthorization"] = json!({
        "purpose": directory_purpose.stable_code(),
        "provenance": peer_directory_authorization.provenance().stable_code(),
        "productionAuthority": peer_directory_authorization
            .provenance()
            .production_service_claim_allowed(),
        "authorizationDigest": peer_directory_authorization.authorization_digest(),
        "signedPrekeyAuthorizationDigest": signed_prekey_directory_authorization
            .as_ref()
            .map(AuthorizedDirectoryLeaf::authorization_digest),
        "oneTimePrekeyAuthorizationDigest": one_time_prekey_directory_authorization
            .as_ref()
            .map(AuthorizedDirectoryLeaf::authorization_digest)
    });
    if let Some(e2ee) = candidate
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        if let Some(session_id) = descriptor
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            e2ee.insert("peerSessionId".to_string(), json!(session_id));
        } else {
            e2ee.remove("peerSessionId");
        }
        e2ee.insert(
            "peerPreKeyBundle".to_string(),
            descriptor
                .get("preKeyBundle")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or(Value::Null),
        );
        if let Some(pairwise_intro) = descriptor
            .get("pairwiseIntro")
            .filter(|value| value.is_object())
            .cloned()
        {
            e2ee.insert("peerPairwiseIntro".to_string(), pairwise_intro);
        } else {
            e2ee.remove("peerPairwiseIntro");
        }
        if let Some(pairwise_accepted) = descriptor
            .get("pairwiseAccepted")
            .filter(|value| value.is_object())
            .cloned()
        {
            e2ee.insert("peerPairwiseAccepted".to_string(), pairwise_accepted);
        } else {
            e2ee.remove("peerPairwiseAccepted");
        }
        if let Some(pairwise_finished) = descriptor
            .get("pairwiseFinished")
            .filter(|value| value.is_object())
            .cloned()
        {
            e2ee.insert("peerPairwiseFinished".to_string(), pairwise_finished);
        } else {
            e2ee.remove("peerPairwiseFinished");
        }
    }
    let directory_trust_state = if directory_revoked {
        DeviceTrustState::Revoked
    } else if identity_changed {
        DeviceTrustState::KeyChanged
    } else if first_pairing && verified {
        DeviceTrustState::Verified
    } else if prior_peer_verified {
        DeviceTrustState::Verified
    } else {
        DeviceTrustState::Unverified
    };
    candidate["mobileRelayE2ee"]["peerVerified"] =
        json!(directory_trust_state == DeviceTrustState::Verified);
    if directory_trust_state == DeviceTrustState::Verified
        && (first_pairing || identity_changed || directory_revoked)
    {
        let local_endpoint = local_endpoint_state(&candidate, &secret_context.material)?;
        let issued_at = mobile_relay_trust_record_now_epoch()?;
        let expires_at = mobile_relay_trust_record_expiry_epoch(issued_at)?;
        let trust_record = sign_device_trust_record(
            &local_endpoint.signing_key()?,
            &local_endpoint.device_identity()?,
            &peer_identity,
            DeviceTrustState::Verified,
            peer_identity.rotation_epoch,
            "pairing_claim_proof_and_key_transparency",
            issued_at,
            expires_at,
        )?;
        candidate["mobileRelayE2ee"]["peerTrustRecord"] =
            device_trust_record_to_json(&trust_record);
    } else if directory_trust_state == DeviceTrustState::Unverified
        && first_pairing
        && let Some(e2ee) = candidate
            .get_mut("mobileRelayE2ee")
            .and_then(Value::as_object_mut)
    {
        e2ee.remove("peerTrustRecord");
    }
    if matches!(
        directory_trust_state,
        DeviceTrustState::KeyChanged | DeviceTrustState::Revoked
    ) {
        let local_identity =
            local_endpoint_state(&candidate, &secret_context.material)?.device_identity()?;
        let terminal_state = match directory_trust_state {
            DeviceTrustState::KeyChanged => "key_changed",
            DeviceTrustState::Revoked => "revoked",
            _ => unreachable!("terminal directory state checked above"),
        };
        candidate["pairingId"] = json!("");
        candidate["pcToken"] = json!("");
        candidate["mobileToken"] = json!("");
        candidate["paired"] = json!(false);
        candidate["relayEnabled"] = json!(false);
        clear_pairing_presentation(&mut candidate);
        if let Some(root) = candidate.as_object_mut() {
            for key in [
                "pairedDevices",
                "pcTokenPresent",
                "mobileTokenPresent",
                "secretStorageStatus",
            ] {
                root.remove(key);
            }
        }
        if let Some(e2ee) = candidate
            .get_mut("mobileRelayE2ee")
            .and_then(Value::as_object_mut)
        {
            for key in [
                "peerEndpointId",
                "peerEndpointKind",
                "peerPublicKeyBase64url",
                "peerFingerprint",
                "peerSessionId",
                "peerPreKeyBundle",
                "peerPairwiseIntro",
                "peerPairwiseAccepted",
                "peerPairwiseFinished",
                "peerSigningPublicKeyBase64url",
                "peerRotationEpoch",
                "peerDeviceTrustFingerprint",
                "peerTrustRecord",
                "peerKeyTransparencyAuthorization",
                "pendingPairwiseIntro",
                "pairwiseAccepted",
                "pairwiseFinished",
                "sessionId",
                "pairingSecretBase64url",
            ] {
                e2ee.remove(key);
            }
            e2ee.insert("peerVerified".to_string(), json!(false));
            e2ee.insert(
                "keyTransparencyTerminalPeerBlock".to_string(),
                json!({
                    "schemaVersion": 1,
                    "state": terminal_state,
                    "stableDirectoryLabel": stable_directory_label(
                        &peer_directory_authorization
                            .claim()
                            .endpoint
                            .directory_scope_commitment,
                        &peer_identity.endpoint_id,
                    ),
                    "directoryVersion": peer_directory_authorization.claim().directory_version,
                    "rotationEpoch": peer_identity.rotation_epoch,
                    "treeSize": peer_directory_authorization.signed_tree_head().tree_size,
                    "authorizationDigest": peer_directory_authorization.authorization_digest(),
                    "redacted": true,
                }),
            );
        }
        save_config_with_runtime_secret_context(&mut candidate, secret_context)?;
        *config = candidate;
        purge_mobile_relay_pairwise_sessions()?;
        {
            let (secret_store, authorization, namespace) = secret_context
                .secret_store_batch
                .authorization()?
                .ok_or_else(|| anyhow!("secure mesh MLS selected custody is unavailable"))?;
            crate::domain::secure_mesh_mls::reset_selected_custody_for_kt_authority_change(
                &local_identity,
                secret_store.as_ref(),
                &authorization,
                &namespace,
            )?;
        }
        crate::domain::secure_mesh_mls::reset_durable_state_for_kt_authority_change()?;
        return Err(anyhow!(
            "mobile relay peer directory trust is terminal ({terminal_state}); re-pairing is required"
        ));
    }
    if candidate["mobileRelayE2ee"]
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        candidate["mobileRelayE2ee"]["sessionId"] =
            json!(format!("mrelay_session_{}", Uuid::new_v4()));
    }
    initialize_mobile_relay_pairwise_session(
        &mut candidate,
        &mut secret_context.material,
        descriptor,
        &peer_identity,
    )?;
    *config = candidate;
    Ok(())
}

pub(in crate::domain::mobile_relay) fn validate_peer_identity_transition(
    prior: &DeviceTrustPublicIdentity,
    next: &DeviceTrustPublicIdentity,
) -> Result<()> {
    ensure!(
        prior.endpoint_id == next.endpoint_id,
        "mobile relay peer identity transition endpoint mismatch"
    );
    let key_material_changed = prior.identity_public_key != next.identity_public_key
        || prior.signing_public_key != next.signing_public_key;
    if key_material_changed {
        ensure!(
            next.rotation_epoch > prior.rotation_epoch,
            "mobile relay peer identity key change requires strict rotation epoch advance"
        );
    } else {
        ensure!(
            next.rotation_epoch == prior.rotation_epoch,
            "mobile relay unchanged peer identity cannot change rotation epoch"
        );
    }
    Ok(())
}

pub(in crate::domain::mobile_relay) fn peer_secure_mesh_descriptor(
    config: &Value,
) -> Option<Value> {
    let state = config.get("mobileRelayE2ee")?;
    let endpoint_id = state.get("peerEndpointId")?.as_str()?.trim();
    let endpoint_kind = state.get("peerEndpointKind")?.as_str()?.trim();
    let public_key = state.get("peerPublicKeyBase64url")?.as_str()?.trim();
    if endpoint_id.is_empty() || endpoint_kind.is_empty() || public_key.is_empty() {
        return None;
    }
    let mut descriptor = json!({
        "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "endpointId": endpoint_id,
        "endpointKind": endpoint_kind,
        "publicKeyBase64url": public_key,
        "fingerprint": state.get("peerFingerprint").and_then(Value::as_str).unwrap_or_default(),
        "keyAgreement": "pqxdh-x25519-ed25519-mlkem1024-triple-ratchet",
        "payloadCipher": SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "sessionId": state
            .get("peerSessionId")
            .or_else(|| state.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    });
    if let Some(prekey_bundle) = state
        .get("peerPreKeyBundle")
        .filter(|value| value.is_object())
        .cloned()
    {
        descriptor["preKeyBundle"] = prekey_bundle;
    }
    if let Some(pairwise_intro) = state
        .get("peerPairwiseIntro")
        .filter(|value| value.is_object())
        .cloned()
    {
        descriptor["pairwiseIntro"] = pairwise_intro;
    }
    if let Some(pairwise_accepted) = state
        .get("peerPairwiseAccepted")
        .filter(|value| value.is_object())
        .cloned()
    {
        descriptor["pairwiseAccepted"] = pairwise_accepted;
    }
    if let Some(pairwise_finished) = state
        .get("peerPairwiseFinished")
        .filter(|value| value.is_object())
        .cloned()
    {
        descriptor["pairwiseFinished"] = pairwise_finished;
    }
    Some(descriptor)
}

pub(in crate::domain::mobile_relay) fn ensure_peer_verified(config: &Value) -> Result<()> {
    let _authorization =
        ensure_peer_authorized_for_protected_send(config, ProtectedSendPayloadKind::Command)?;
    Ok(())
}

pub(in crate::domain::mobile_relay) fn ensure_peer_authorized_for_protected_send(
    config: &Value,
    payload_kind: ProtectedSendPayloadKind,
) -> Result<ProtectedSendAuthorization> {
    ensure_secure_mesh_protected_operation_allowed()?;
    require_current_pairwise_directory_authority(
        config,
        current_secure_mesh_kt_gate_epoch_seconds()?,
    )?;
    ensure_peer_trust_authorized_for_protected_send(config, payload_kind)
}

pub(in crate::domain::mobile_relay) fn ensure_peer_trust_authorized_for_protected_send(
    config: &Value,
    payload_kind: ProtectedSendPayloadKind,
) -> Result<ProtectedSendAuthorization> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let Some(state) = config.get("mobileRelayE2ee") else {
        return Err(anyhow!(
            "mobile relay E2EE peer is not verified; refusing to process server-relayed commands"
        ));
    };
    let verified = state
        .get("peerVerified")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !verified {
        return Err(anyhow!(
            "mobile relay E2EE peer is not verified; refusing to process server-relayed commands"
        ));
    }
    let local_identity = local_public_device_identity(config)?;
    let peer_identity = peer_device_identity_from_state(state)?;
    let trust_record = state
        .get("peerTrustRecord")
        .ok_or_else(|| anyhow!("mobile relay E2EE peer trust record is missing"))?;
    let record = crate::core::secure_mesh_trust::device_trust_record_from_json(trust_record)?;
    authorize_protected_send_from_trust_record(
        &local_identity,
        &peer_identity,
        &record,
        mobile_relay_trust_record_now_epoch()?,
        payload_kind,
    )
    .map_err(|failure| {
        let message = format!("{failure}");
        if message.contains("peer trust record") || message.contains("trust record") {
            anyhow!("mobile relay E2EE peer trust record is invalid")
        } else if message.contains("verification_required")
            || message.contains("identity_key_changed")
            || message.contains("device_revoked")
            || message.contains("cross_signature_requires_durable_epoch_validation")
        {
            failure
        } else {
            anyhow!("mobile relay E2EE peer trust record is invalid")
        }
    })
}

pub(in crate::domain::mobile_relay) fn protected_send_kind_from_payload(
    kind: crate::core::secure_mesh_crypto::SecureMeshPayloadKind,
) -> ProtectedSendPayloadKind {
    match kind {
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command => {
            ProtectedSendPayloadKind::Command
        }
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload
        | crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Error => {
            ProtectedSendPayloadKind::Result
        }
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::FileChunk
        | crate::core::secure_mesh_crypto::SecureMeshPayloadKind::FileManifest => {
            ProtectedSendPayloadKind::File
        }
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction
        | crate::core::secure_mesh_crypto::SecureMeshPayloadKind::TypingIndicator
        | crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ReadReceipt => {
            ProtectedSendPayloadKind::Lifecycle
        }
    }
}

pub(in crate::domain::mobile_relay) fn peer_device_identity_from_state(
    state: &Value,
) -> Result<DeviceTrustPublicIdentity> {
    DeviceTrustPublicIdentity::new(
        descriptor_text(state, "peerEndpointId")?,
        decode_key_32(
            &descriptor_text(state, "peerPublicKeyBase64url")?,
            "mobile relay peer trust identity public key",
        )?,
        decode_key_32(
            &descriptor_text(state, "peerSigningPublicKeyBase64url")?,
            "mobile relay peer trust signing public key",
        )?,
        state
            .get("peerRotationEpoch")
            .and_then(Value::as_u64)
            .unwrap_or(1),
    )
}

pub(in crate::domain::mobile_relay) fn mobile_relay_trust_record_now_epoch() -> Result<u64> {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| anyhow!("mobile relay trust record clock is before unix epoch"))
}

pub(in crate::domain::mobile_relay) fn mobile_relay_trust_record_expiry_epoch(
    issued_at_epoch_seconds: u64,
) -> Result<u64> {
    let expires_at = OffsetDateTime::from_unix_timestamp(
        i64::try_from(issued_at_epoch_seconds)
            .map_err(|_| anyhow!("mobile relay trust record issue time is invalid"))?,
    )
    .map_err(|_| anyhow!("mobile relay trust record issue time is invalid"))?
        + Duration::days(MOBILE_RELAY_TRUST_RECORD_VALIDITY_DAYS);
    u64::try_from(expires_at.unix_timestamp())
        .map_err(|_| anyhow!("mobile relay trust record expiry is invalid"))
}

pub(in crate::domain::mobile_relay) fn is_peer_trust_record_verified(config: &Value) -> bool {
    config
        .get("mobileRelayE2ee")
        .and_then(|state| {
            let local_identity = local_public_device_identity(config).ok()?;
            let peer_identity = peer_device_identity_from_state(state).ok()?;
            let trust_record = state.get("peerTrustRecord")?;
            verify_device_trust_record_json(
                &local_identity,
                &peer_identity,
                trust_record,
                mobile_relay_trust_record_now_epoch().ok()?,
            )
            .ok()
        })
        .is_some()
}

pub(in crate::domain::mobile_relay) struct PeerEndpointState {
    pub(in crate::domain::mobile_relay) endpoint_id: String,
    pub(in crate::domain::mobile_relay) endpoint_kind: String,
    pub(in crate::domain::mobile_relay) fingerprint: String,
}

pub(in crate::domain::mobile_relay) fn peer_endpoint_state(
    config: &Value,
) -> Result<PeerEndpointState> {
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let endpoint_id = descriptor_text(state, "peerEndpointId")?;
    let endpoint_kind = descriptor_text(state, "peerEndpointKind")?;
    let public_key = descriptor_text(state, "peerPublicKeyBase64url")?;
    let _mailbox_rotation_epoch = state
        .get("peerMailboxRotationEpoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("Lico Arc peer mailbox rotation epoch is missing"))?;
    let public_bytes = decode_key_32(&public_key, "mobile relay peer public key")?;
    Ok(PeerEndpointState {
        endpoint_id,
        endpoint_kind,
        fingerprint: public_key_fingerprint(&public_bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_key_change_requires_strict_rotation_epoch_advance() {
        let prior = DeviceTrustPublicIdentity::new("device-a", [1u8; 32], [2u8; 32], 1).unwrap();
        let unchanged_epoch_change =
            DeviceTrustPublicIdentity::new("device-a", [1u8; 32], [2u8; 32], 2).unwrap();
        let stale_key_change =
            DeviceTrustPublicIdentity::new("device-a", [3u8; 32], [4u8; 32], 1).unwrap();
        let rotated_key_change =
            DeviceTrustPublicIdentity::new("device-a", [3u8; 32], [4u8; 32], 2).unwrap();

        assert!(validate_peer_identity_transition(&prior, &unchanged_epoch_change).is_err());
        assert!(validate_peer_identity_transition(&prior, &stale_key_change).is_err());
        assert!(validate_peer_identity_transition(&prior, &rotated_key_change).is_ok());
    }
}
