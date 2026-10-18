use super::*;
use crate::core::secure_mesh_secret_store::SecretBytes;
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
};

pub(in crate::domain::mobile_relay) fn apply_out_of_band_pairing_response_with_context(
    config: &mut Value,
    response: &Value,
    mut secret_context: Option<&mut RuntimeSecretContext>,
) -> Result<()> {
    let object = response
        .as_object()
        .ok_or_else(|| anyhow!("mobile relay out-of-band pairing response must be an object"))?;
    let actual_fields = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected_fields = ["mobileSecureMesh", "secureMeshClaimProof"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    ensure!(
        actual_fields == expected_fields,
        "mobile relay out-of-band pairing response shape is invalid"
    );
    let pairing_id = config
        .get("pairingId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("mobile relay local pending pairing id is missing"))?;
    let mobile_secure_mesh = object
        .get("mobileSecureMesh")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("mobile relay out-of-band mobile descriptor is missing"))?;
    let claim_proof = object
        .get("secureMeshClaimProof")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("mobile relay out-of-band claim proof is missing"))?;
    let context = secret_context
        .as_deref_mut()
        .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
    let pc_secure_mesh = local_endpoint_state(config, &context.material)?.public_descriptor()?;
    ensure!(
        mobile_relay_claim_proof_matches(
            config,
            &context.material,
            pairing_id,
            mobile_secure_mesh,
            &pc_secure_mesh,
            claim_proof,
        )?,
        "mobile relay out-of-band claim proof is invalid"
    );
    apply_peer_secure_mesh_descriptor_with_context(
        config,
        mobile_secure_mesh,
        true,
        Some(context),
    )?;
    config["paired"] = json!(true);
    Ok(())
}

pub(in crate::domain::mobile_relay) fn with_config(mut response: Value, config: &Value) -> Value {
    let public = public_config(config);
    if let Some(object) = response.as_object_mut() {
        object.insert("config".to_string(), public);
        return response;
    }
    json!({
        "ok": true,
        "response": response,
        "config": public
    })
}

pub(in crate::domain::mobile_relay) fn public_config(config: &Value) -> Value {
    let mut public = config.clone();
    let pc_token_present = secret_present(config.get("pcToken"))
        || config.get("pcTokenPresent").and_then(Value::as_bool) == Some(true);
    let selected_pairing_id = config
        .get("pairingId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let selected_device_credential_present = config.get("secretStorageStatus").is_some()
        && config
            .get("pairedDevices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|device| {
                !selected_pairing_id.is_empty()
                    && device
                        .get("pairingId")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        == Some(selected_pairing_id)
                    && device.get("credentialPresent").and_then(Value::as_bool) == Some(true)
            });
    let mobile_token_present = secret_present(config.get("mobileToken"))
        || config.get("mobileTokenPresent").and_then(Value::as_bool) == Some(true)
        || selected_device_credential_present;
    let secret_storage_backend = public_secret_storage_backend(config);
    public["pcToken"] = json!("");
    public["mobileToken"] = json!("");
    public["lastPairingCode"] = json!("");
    public["lastPairingExpiresAt"] = json!("");
    public["pcTokenPresent"] = json!(pc_token_present);
    public["mobileTokenPresent"] = json!(mobile_token_present);
    if let Some(object) = public.as_object_mut() {
        object.remove("mobileRelayPairingInvite");
    }
    if let Some(state) = public
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        state.remove(crate::domain::mobile_relay::secret_custody::CUSTODY_NAMESPACE_FIELD);
        state.remove("privateKeyBase64url");
        state.remove("signingKeyBase64url");
        state.remove("signedPrekeyPrivateKeyBase64url");
        state.remove("oneTimePrekeyPrivateKeyBase64url");
        state.remove("oneTimeMlKem1024PrekeySeedBase64url");
        state.remove("pairingSecretBase64url");
        state.insert("privateKeyMaterial".to_string(), json!("redacted"));
        state.insert("signingKeyMaterial".to_string(), json!("redacted"));
        state.insert(
            "signedPrekeyPrivateKeyMaterial".to_string(),
            json!("redacted"),
        );
        state.insert(
            "oneTimePrekeyPrivateKeyMaterial".to_string(),
            json!("redacted"),
        );
        state.insert(
            "oneTimeMlKem1024PrekeySeedMaterial".to_string(),
            json!("redacted"),
        );
        state.insert("pairingSecretMaterial".to_string(), json!("redacted"));
        state.insert(
            "secretStorageStatus".to_string(),
            json!(secret_storage_backend.clone()),
        );
    }
    if let Some(devices) = public
        .get_mut("pairedDevices")
        .and_then(Value::as_array_mut)
    {
        for device in devices {
            if let Some(object) = device.as_object_mut() {
                let credential_present = object
                    .get("credentialPresent")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| secret_present(object.get("mobileToken")));
                object.insert("mobileToken".to_string(), json!(""));
                object.insert("credentialPresent".to_string(), json!(credential_present));
            }
        }
    }
    public["secretStorageStatus"] = json!({
        "tokenMaterial": "redacted",
        "mobileRelayPrivateKeyMaterial": "redacted",
        "selectedBackend": secret_storage_backend,
        "unsafePersistenceForbidden": true
    });
    if let Ok(presentation) = public_device_trust_presentation(config) {
        public["deviceTrustPresentation"] = presentation;
    } else if let Some(object) = public.as_object_mut() {
        object.remove("deviceTrustPresentation");
    }
    public
}

pub(in crate::domain::mobile_relay) fn public_device_trust_presentation(
    config: &Value,
) -> Result<Value> {
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay device trust state is missing"))?;
    let local_identity = DeviceTrustPublicIdentity::new(
        descriptor_text(state, "endpointId")?,
        decode_key_32(
            &descriptor_text(state, "publicKeyBase64url")?,
            "mobile relay local trust identity public key",
        )?,
        decode_key_32(
            &descriptor_text(state, "signingPublicKeyBase64url")?,
            "mobile relay local trust signing public key",
        )?,
        state
            .get("rotationEpoch")
            .and_then(Value::as_u64)
            .unwrap_or(1),
    )?;
    let peer_identity = peer_device_identity_from_state(state)?;
    let safety_number_groups = sas_decimal_chunks(&local_identity, &peer_identity)?;
    let trust_record = state.get("peerTrustRecord");
    let now_epoch_seconds = mobile_relay_trust_record_now_epoch()?;
    let trust_record_verified = trust_record.is_some_and(|record| {
        verify_device_trust_record_json(&local_identity, &peer_identity, record, now_epoch_seconds)
            .is_ok()
    });
    let trust_state = trust_record
        .and_then(|record| record.get("trustState"))
        .and_then(Value::as_str)
        .unwrap_or("unverified");
    let verification_method = trust_record
        .and_then(|record| record.get("verificationMethod"))
        .and_then(Value::as_str)
        .unwrap_or("unverified");
    Ok(json!({
        "schemaVersion": "licomesh.secure-mesh.device-trust-presentation.v1",
        "protocolVersion": crate::core::secure_mesh_trust::SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "localFingerprint": local_identity.fingerprint()?,
        "peerFingerprint": peer_identity.fingerprint()?,
        "safetyNumberGroups": safety_number_groups,
        "qrPayload": qr_verification_payload(&local_identity, &peer_identity, 0)?,
        "trustState": trust_state,
        "verificationMethod": verification_method,
        "verified": trust_record_verified && trust_state == "verified",
        "keyMaterial": "redacted"
    }))
}

pub(in crate::domain::mobile_relay) fn redacted_pairing_invite(invite: Option<&Value>) -> Value {
    let Some(invite) = invite else {
        return Value::Null;
    };
    let mut public = invite.clone();
    if let Some(object) = public.as_object_mut() {
        if secret_present(object.get("e2eePairingSecret")) {
            object.remove("e2eePairingSecret");
            object.insert("e2eePairingSecretMaterial".to_string(), json!("redacted"));
        }
    }
    public
}

pub(in crate::domain::mobile_relay) fn clear_pairing_presentation(config: &mut Value) {
    config["lastPairingCode"] = json!("");
    config["lastPairingExpiresAt"] = json!("");
    if let Some(object) = config.as_object_mut() {
        object.remove("mobileRelayPairingInvite");
    }
}

pub(in crate::domain::mobile_relay) fn clear_mobile_relay_pairing_state(
    config: &mut Value,
) -> Result<()> {
    config["pairingId"] = json!("");
    config["pcToken"] = json!("");
    config["mobileToken"] = json!("");
    clear_pairing_presentation(config);
    config["paired"] = json!(false);
    config["relayEnabled"] = json!(false);
    if let Some(object) = config.as_object_mut() {
        object.remove("pairedDevices");
        object.remove("pcTokenPresent");
        object.remove("mobileTokenPresent");
        object.remove("secretStorageStatus");
    }
    if let Some(state) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        let next_mailbox_rotation_epoch = state
            .get("mailboxRotationEpoch")
            .and_then(Value::as_u64)
            .unwrap_or(current_mailbox_rotation_epoch()?)
            .checked_add(1)
            .ok_or_else(|| anyhow!("Lico Arc mailbox rotation epoch overflow"))?;
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
            "peerMailboxRotationEpoch",
            "peerDeviceTrustFingerprint",
            "peerTrustRecord",
            "pendingPairwiseIntro",
            "pairwiseAccepted",
            "pairwiseFinished",
        ] {
            state.remove(key);
        }
        state.insert("peerVerified".to_string(), json!(false));
        state.insert(
            "sessionId".to_string(),
            json!(format!("mrelay_session_{}", Uuid::new_v4())),
        );
        state.insert(
            "pairingSecretBase64url".to_string(),
            json!(random_base64url(MOBILE_RELAY_KEY_BYTES)),
        );
        state.insert(
            "mailboxRotationEpoch".to_string(),
            json!(next_mailbox_rotation_epoch),
        );
    }
    purge_mobile_relay_pairwise_sessions()?;
    Ok(())
}

pub(in crate::domain::mobile_relay) fn stable_json_sha256(value: &Value) -> String {
    sha256_hex(serde_json::to_string(value).unwrap_or_default().as_bytes())
}

pub(in crate::domain::mobile_relay) fn one_time_pairing_invite(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    response: &Value,
) -> Option<Value> {
    let Some(pairing_id) = response.get("pairingId").and_then(Value::as_str) else {
        return None;
    };
    let Some(pairing_code) = response.get("pairingCode").and_then(Value::as_str) else {
        return None;
    };
    let Some(secret) = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::PairingSecret)
        .and_then(|secret| secret.expose_utf8().ok())
    else {
        return None;
    };
    local_endpoint_state(config, secret_material).ok().and_then(|endpoint| {
        let station_base_url = effective_station_base_url(config).ok()?;
        let pc_secure_mesh = endpoint.public_descriptor().ok()?;
        Some(json!({
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "oneTime": true,
            "createdAt": now_iso(),
            "stationBaseUrl": station_base_url,
            "pcClientId": config.get("pcClientId").and_then(Value::as_str).unwrap_or_default(),
            "pcClientName": config.get("pcClientName").and_then(Value::as_str).unwrap_or("LicoUp"),
            "pairingId": pairing_id,
            "pairingCode": pairing_code,
            "pairingCodeHash": sha256_hex(pairing_code.as_bytes()),
            "pcSecureMesh": pc_secure_mesh,
            "e2eePairingSecret": secret
        }))
    })
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn apply_pairing_invite_params(
    config: &mut Value,
    params: &Value,
) -> Result<()> {
    let mut context = RuntimeSecretContext::default();
    apply_pairing_invite_params_with_context(config, params, Some(&mut context))
}

pub(in crate::domain::mobile_relay) fn apply_pairing_invite_params_with_context(
    config: &mut Value,
    params: &Value,
    mut secret_context: Option<&mut RuntimeSecretContext>,
) -> Result<()> {
    let invite = json_param(params, "invite")
        .or_else(|| json_param(params, "pairingInvite"))
        .or_else(|| json_param(params, "inviteJson"))
        .or(json_file_param(
            params,
            &[
                "inviteFile",
                "invitePath",
                "pairingInviteFile",
                "pairingInvitePath",
                "inviteJsonFile",
                "inviteJsonPath",
            ],
        )?);
    if let Some(invite) = invite {
        if !invite.is_object() {
            return Err(anyhow!("mobile relay pairing invite must be a JSON object"));
        }
        let validated_invite_station = match invite.get("stationBaseUrl") {
            None => None,
            Some(Value::String(value)) => Some(validated_station_base_url(value)?),
            Some(_) => {
                return Err(anyhow!(
                    "mobile relay pairing invite station must be a valid URL"
                ));
            }
        };
        ensure!(
            descriptor_text(&invite, "protocolVersion")? == MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "mobile relay pairing invite protocol is unsupported; a new pairing invite is required"
        );
        ensure!(
            invite.get("oneTime").and_then(Value::as_bool) == Some(true),
            "mobile relay pairing invite must be one-time"
        );
        if pairing_invite_requires_state_reset(config, &invite) {
            clear_mobile_relay_pairing_state(config)?;
        }
        if let Some(pairing_id) = invite.get("pairingId").and_then(Value::as_str) {
            config["pairingId"] = json!(pairing_id);
        }
        if let Some(pairing_code) = invite.get("pairingCode").and_then(Value::as_str) {
            config["lastPairingCode"] = json!(pairing_code);
        }
        if let Some(station_base_url) = validated_invite_station {
            config["stationBaseUrl"] = json!(station_base_url);
            normalize_station_fields(config);
        }
        if let Some(pc_client_id) = invite.get("pcClientId").and_then(Value::as_str) {
            config["pcClientId"] = json!(pc_client_id);
        }
        if let Some(pc_client_name) = invite.get("pcClientName").and_then(Value::as_str) {
            config["pcClientName"] = json!(pc_client_name);
        }
        if let Some(secret) = invite.get("e2eePairingSecret").and_then(Value::as_str) {
            let context = secret_context
                .as_deref_mut()
                .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
            context.material.replace_e2ee_secret(
                MobileRelayE2eeSecretField::PairingSecret,
                SecretBytes::try_from_bytes(secret.trim().as_bytes().to_vec())?,
            )?;
            ensure_mobile_relay_endpoint_descriptor(config, &mut context.material, "mobile")?;
        }
        if let Some(pc_secure_mesh) = invite.get("pcSecureMesh") {
            let context = secret_context
                .as_deref_mut()
                .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
            ensure_mobile_relay_endpoint_descriptor(config, &mut context.material, "mobile")?;
            apply_peer_secure_mesh_descriptor_with_context(
                config,
                pc_secure_mesh,
                true,
                secret_context.as_deref_mut(),
            )?;
        }
    }
    if let Some(secret) = text_param(params, &["e2eePairingSecret", "pairingSecret"]) {
        let context = secret_context
            .as_deref_mut()
            .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
        context.material.replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(secret)?,
        )?;
        ensure_mobile_relay_endpoint_descriptor(config, &mut context.material, "mobile")?;
    }
    if let Some(pc_secure_mesh) = json_param(params, "pcSecureMesh") {
        let context = secret_context
            .as_deref_mut()
            .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
        ensure_mobile_relay_endpoint_descriptor(config, &mut context.material, "mobile")?;
        apply_peer_secure_mesh_descriptor_with_context(
            config,
            &pc_secure_mesh,
            true,
            secret_context.as_deref_mut(),
        )?;
    }
    if let Some(pc_secure_mesh) = json_file_param(
        params,
        &[
            "pcSecureMeshFile",
            "pcSecureMeshPath",
            "pcSecureMeshJsonFile",
            "pcSecureMeshJsonPath",
        ],
    )? {
        let context = secret_context
            .as_deref_mut()
            .ok_or_else(|| anyhow!("mobile relay runtime secret context is required"))?;
        ensure_mobile_relay_endpoint_descriptor(config, &mut context.material, "mobile")?;
        apply_peer_secure_mesh_descriptor_with_context(
            config,
            &pc_secure_mesh,
            true,
            secret_context.as_deref_mut(),
        )?;
    }
    Ok(())
}

pub(in crate::domain::mobile_relay) fn pairing_claim_secure_mesh_descriptor_from_params(
    params: &Value,
) -> Result<Option<Value>> {
    let invite = json_param(params, "invite")
        .or_else(|| json_param(params, "pairingInvite"))
        .or_else(|| json_param(params, "inviteJson"))
        .or(json_file_param(
            params,
            &[
                "inviteFile",
                "invitePath",
                "pairingInviteFile",
                "pairingInvitePath",
                "inviteJsonFile",
                "inviteJsonPath",
            ],
        )?);
    if let Some(pc_secure_mesh) = invite
        .as_ref()
        .and_then(|invite| invite.get("pcSecureMesh"))
        .cloned()
    {
        return Ok(Some(pc_secure_mesh));
    }
    if let Some(pc_secure_mesh) = json_param(params, "pcSecureMesh") {
        return Ok(Some(pc_secure_mesh));
    }
    json_file_param(
        params,
        &[
            "pcSecureMeshFile",
            "pcSecureMeshPath",
            "pcSecureMeshJsonFile",
            "pcSecureMeshJsonPath",
        ],
    )
}

pub(in crate::domain::mobile_relay) fn pairing_invite_requires_state_reset(
    config: &Value,
    invite: &Value,
) -> bool {
    let Some(next_pairing_id) = invite
        .get("pairingId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let current_pairing_id = config
        .get("pairingId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !current_pairing_id.is_empty() {
        return current_pairing_id != next_pairing_id;
    }
    config
        .get("mobileRelayE2ee")
        .and_then(Value::as_object)
        .is_some_and(|state| {
            state
                .get("peerEndpointId")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
                || state.get("pendingPairwiseIntro").is_some()
                || state.get("pairwiseAccepted").is_some()
                || state.get("pairwiseFinished").is_some()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_invite_projection_removes_pairing_secret_material() {
        let projected = redacted_pairing_invite(Some(&json!({
            "pairingId": "fixture-pairing",
            "e2eePairingSecret": "fixture-material"
        })));

        assert_eq!(projected["pairingId"], "fixture-pairing");
        assert!(projected.get("e2eePairingSecret").is_none());
        assert_eq!(projected["e2eePairingSecretMaterial"], "redacted");
    }
}
