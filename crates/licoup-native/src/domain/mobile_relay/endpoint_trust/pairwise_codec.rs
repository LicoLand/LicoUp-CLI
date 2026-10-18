use super::*;

pub(in crate::domain::mobile_relay) fn pairwise_prekey_bundle_from_descriptor(
    descriptor: &Value,
) -> Result<SecureMeshPairwisePreKeyBundle> {
    ensure!(
        descriptor_text(descriptor, "protocolVersion")? == MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "mobile relay peer secure mesh descriptor protocol is unsupported; re-pairing is required"
    );
    ensure!(
        descriptor_text(descriptor, "keyAgreement")?
            == "pqxdh-x25519-ed25519-mlkem1024-triple-ratchet",
        "mobile relay peer secure mesh key agreement is unsupported"
    );
    ensure!(
        descriptor_text(descriptor, "payloadCipher")? == SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "mobile relay peer secure mesh payload cipher is unsupported"
    );
    let bundle = descriptor
        .get("preKeyBundle")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("mobile relay peer secure mesh descriptor missing preKeyBundle"))?;
    require_exact_object_fields(
        bundle,
        &[
            "endpointIdentity",
            "keyTransparency",
            "oneTimeMlKem1024Prekey",
            "oneTimePrekey",
            "prekeyPublicationVersion",
            "protocolVersion",
            "signedPrekey",
        ],
        "mobile relay peer prekey bundle",
    )?;
    ensure!(
        descriptor_text(bundle, "protocolVersion")?
            == crate::core::secure_mesh_prekey::SECURE_MESH_PREKEY_PROTOCOL_VERSION,
        "mobile relay peer prekey bundle protocol is unsupported"
    );
    let identity_value = bundle
        .get("endpointIdentity")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("mobile relay peer prekey bundle missing endpointIdentity"))?;
    let endpoint_identity = device_identity_from_descriptor(identity_value)?;
    let signed_prekey = prekey_record_from_descriptor::<MOBILE_RELAY_KEY_BYTES>(
        bundle
            .get("signedPrekey")
            .ok_or_else(|| anyhow!("mobile relay peer prekey bundle missing signedPrekey"))?,
        "signed prekey",
    )?;
    let one_time_prekey = Some(prekey_record_from_descriptor::<MOBILE_RELAY_KEY_BYTES>(
        bundle
            .get("oneTimePrekey")
            .filter(|value| value.is_object())
            .ok_or_else(|| anyhow!("mobile relay peer prekey bundle missing oneTimePrekey"))?,
        "one-time curve prekey",
    )?);
    let one_time_mlkem1024_prekey = prekey_record_from_descriptor::<ML_KEM_1024_PUBLIC_KEY_BYTES>(
        bundle
            .get("oneTimeMlKem1024Prekey")
            .filter(|value| value.is_object())
            .ok_or_else(|| {
                anyhow!(
                    "mobile relay peer prekey bundle missing oneTimeMlKem1024Prekey; re-pairing is required"
                )
            })?,
        "ML-KEM-1024 one-time prekey",
    )?;
    Ok(SecureMeshPairwisePreKeyBundle {
        endpoint_identity,
        // Trust is local state established by the out-of-band pairing proof and the
        // locally signed peer trust record. A relay-provided descriptor cannot assert it.
        trust_state: DeviceTrustState::Unverified,
        signed_prekey,
        one_time_prekey,
        one_time_mlkem1024_prekey,
        prekey_publication_version: bundle
            .get("prekeyPublicationVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("mobile relay peer prekey publication version is missing"))?,
    })
}

pub(in crate::domain::mobile_relay) fn prekey_record_from_descriptor<
    const PUBLIC_KEY_BYTES: usize,
>(
    value: &Value,
    label: &str,
) -> Result<SecureMeshPreKeyRecord> {
    require_exact_object_fields(
        value,
        &[
            "createdAt",
            "expiresAt",
            "prekeyId",
            "publicKeyBase64url",
            "signatureBase64url",
        ],
        &format!("mobile relay peer {label}"),
    )?;
    Ok(SecureMeshPreKeyRecord {
        prekey_id: descriptor_text(value, "prekeyId")
            .map_err(|_| anyhow!("mobile relay peer {label} id is missing"))?,
        public_key: decode_fixed_base64url::<PUBLIC_KEY_BYTES>(
            &descriptor_text(value, "publicKeyBase64url")?,
            &format!("mobile relay peer {label} public key"),
        )?
        .to_vec(),
        signature: descriptor_text(value, "signatureBase64url")?,
        created_at: descriptor_text(value, "createdAt")?,
        expires_at: descriptor_text(value, "expiresAt")?,
    })
}

pub(in crate::domain::mobile_relay) fn device_identity_to_json(
    identity: &DeviceTrustPublicIdentity,
) -> Result<Value> {
    Ok(json!({
        "endpointId": identity.endpoint_id,
        "identityPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(identity.identity_public_key),
        "signingPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(identity.signing_public_key),
        "rotationEpoch": identity.rotation_epoch,
        "fingerprint": identity.fingerprint()?
    }))
}

pub(in crate::domain::mobile_relay) fn device_identity_from_descriptor(
    value: &Value,
) -> Result<DeviceTrustPublicIdentity> {
    require_exact_object_fields(
        value,
        &[
            "endpointId",
            "fingerprint",
            "identityPublicKeyBase64url",
            "rotationEpoch",
            "signingPublicKeyBase64url",
        ],
        "mobile relay peer endpoint identity",
    )?;
    let identity = DeviceTrustPublicIdentity::new(
        descriptor_text(value, "endpointId")?,
        decode_key_32(
            &descriptor_text(value, "identityPublicKeyBase64url")?,
            "mobile relay peer identity public key",
        )?,
        decode_key_32(
            &descriptor_text(value, "signingPublicKeyBase64url")?,
            "mobile relay peer signing public key",
        )?,
        value
            .get("rotationEpoch")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("mobile relay peer identity rotation epoch is missing"))?,
    )?;
    ensure!(
        descriptor_text(value, "fingerprint")? == identity.fingerprint()?,
        "mobile relay peer endpoint identity fingerprint mismatch"
    );
    Ok(identity)
}

pub(in crate::domain::mobile_relay) fn require_exact_object_fields(
    value: &Value,
    expected: &[&str],
    label: &str,
) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("{label} must be an object"))?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    ensure!(actual == expected, "{label} shape is invalid");
    Ok(())
}

pub(in crate::domain::mobile_relay) fn validate_pairwise_intro_targets_local_prekeys(
    config: &Value,
    endpoint: &LocalEndpointState,
    local_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
    intro: &SecureMeshPairwiseSessionIntro,
) -> Result<()> {
    ensure!(
        intro.initiator_endpoint_id == peer_identity.endpoint_id,
        "mobile relay pairwise intro initiator endpoint does not match verified peer"
    );
    ensure!(
        intro.initiator_identity_public_key == peer_identity.identity_public_key,
        "mobile relay pairwise intro initiator identity does not match verified peer"
    );
    ensure!(
        intro.responder_endpoint_id == endpoint.endpoint_id,
        "mobile relay pairwise intro responder endpoint does not match local endpoint"
    );
    ensure!(
        intro.responder_signed_prekey_id == endpoint.signed_prekey_id,
        "mobile relay pairwise intro signed prekey id does not match local endpoint"
    );
    let one_time_prekey_id = intro
        .responder_one_time_prekey_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("mobile relay pairwise intro one-time prekey id is required"))?;
    ensure!(
        one_time_prekey_id == endpoint.one_time_prekey_id,
        "mobile relay pairwise intro one-time prekey id does not match local endpoint"
    );
    ensure!(
        intro.responder_one_time_mlkem1024_prekey_id == endpoint.one_time_mlkem1024_prekey_id,
        "mobile relay pairwise intro ML-KEM-1024 one-time prekey id does not match local endpoint"
    );
    let local_directory =
        authorize_local_pairwise_directory(config, endpoint, OffsetDateTime::now_utc())?;
    local_directory.require_device_identity(local_identity)?;
    ensure!(
        intro.directory_authorization_digest == local_directory.transcript_binding_digest(),
        "mobile relay pairwise intro directory authorization does not match local endpoint"
    );
    Ok(())
}

pub(in crate::domain::mobile_relay) fn pairwise_intro_from_descriptor(
    descriptor: &Value,
) -> Result<Option<SecureMeshPairwiseSessionIntro>> {
    let Some(value) = descriptor
        .get("pairwiseIntro")
        .filter(|value| value.is_object())
    else {
        return Ok(None);
    };
    require_exact_object_fields(
        value,
        &[
            "cipherSuite",
            "directoryAuthorizationDigest",
            "initiatorCapabilityProof",
            "initiatorEphemeralPublicKeyBase64url",
            "initiatorIdentityPublicKeyBase64url",
            "initiatorInitialRatchetPublicKeyBase64url",
            "initiatorEndpointId",
            "initiatorSignatureBase64url",
            "mlkem1024CiphertextBase64url",
            "protocolVersion",
            "responderEndpointId",
            "responderOneTimeMlKem1024PrekeyId",
            "responderOneTimePrekeyId",
            "responderSignedPrekeyId",
            "sessionId",
        ],
        "mobile relay pairwise intro",
    )?;
    Ok(Some(SecureMeshPairwiseSessionIntro {
        protocol_version: descriptor_text(value, "protocolVersion")?,
        cipher_suite: descriptor_text(value, "cipherSuite")?,
        session_id: descriptor_text(value, "sessionId")?,
        initiator_endpoint_id: descriptor_text(value, "initiatorEndpointId")?,
        responder_endpoint_id: descriptor_text(value, "responderEndpointId")?,
        initiator_identity_public_key: decode_key_32(
            &descriptor_text(value, "initiatorIdentityPublicKeyBase64url")?,
            "mobile relay pairwise intro identity public key",
        )?
        .to_vec(),
        initiator_ephemeral_public_key: decode_key_32(
            &descriptor_text(value, "initiatorEphemeralPublicKeyBase64url")?,
            "mobile relay pairwise intro ephemeral public key",
        )?
        .to_vec(),
        initiator_initial_ratchet_public_key: decode_key_32(
            &descriptor_text(value, "initiatorInitialRatchetPublicKeyBase64url")?,
            "mobile relay pairwise intro ratchet public key",
        )?
        .to_vec(),
        responder_signed_prekey_id: descriptor_text(value, "responderSignedPrekeyId")?,
        responder_one_time_prekey_id: value
            .get("responderOneTimePrekeyId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        responder_one_time_mlkem1024_prekey_id: descriptor_text(
            value,
            "responderOneTimeMlKem1024PrekeyId",
        )?,
        mlkem1024_ciphertext: decode_fixed_base64url::<ML_KEM_1024_CIPHERTEXT_BYTES>(
            &descriptor_text(value, "mlkem1024CiphertextBase64url")?,
            "mobile relay pairwise intro ML-KEM-1024 ciphertext",
        )?
        .to_vec(),
        directory_authorization_digest: descriptor_sha256_hex(
            value,
            "directoryAuthorizationDigest",
        )?,
        initiator_capability_proof: serde_json::from_value(
            value
                .get("initiatorCapabilityProof")
                .cloned()
                .ok_or_else(|| {
                    anyhow!("mobile relay pairwise intro capability proof is missing")
                })?,
        )
        .map_err(|_| anyhow!("mobile relay pairwise intro capability proof is invalid"))?,
        initiator_signature: descriptor_text(value, "initiatorSignatureBase64url")?,
    }))
}

pub(in crate::domain::mobile_relay) fn pairwise_intro_to_json(
    intro: &SecureMeshPairwiseSessionIntro,
) -> Value {
    json!({
        "protocolVersion": intro.protocol_version,
        "cipherSuite": intro.cipher_suite,
        "sessionId": intro.session_id,
        "initiatorEndpointId": intro.initiator_endpoint_id,
        "responderEndpointId": intro.responder_endpoint_id,
        "initiatorIdentityPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(&intro.initiator_identity_public_key),
        "initiatorEphemeralPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(&intro.initiator_ephemeral_public_key),
        "initiatorInitialRatchetPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(&intro.initiator_initial_ratchet_public_key),
        "responderSignedPrekeyId": intro.responder_signed_prekey_id,
        "responderOneTimePrekeyId": intro.responder_one_time_prekey_id,
        "responderOneTimeMlKem1024PrekeyId": intro.responder_one_time_mlkem1024_prekey_id,
        "mlkem1024CiphertextBase64url": general_purpose::URL_SAFE_NO_PAD.encode(&intro.mlkem1024_ciphertext),
        "directoryAuthorizationDigest": intro.directory_authorization_digest,
        "initiatorCapabilityProof": intro.initiator_capability_proof,
        "initiatorSignatureBase64url": intro.initiator_signature
    })
}

pub(in crate::domain::mobile_relay) fn pairwise_accepted_from_descriptor(
    descriptor: &Value,
) -> Result<Option<SecureMeshPairwiseSessionAccepted>> {
    let Some(value) = descriptor
        .get("pairwiseAccepted")
        .filter(|value| value.is_object())
    else {
        return Ok(None);
    };
    require_exact_object_fields(
        value,
        &[
            "capabilityBinding",
            "cipherSuite",
            "handshakeTranscriptHashBase64url",
            "keyConfirmationBase64url",
            "protocolVersion",
            "responderCapabilityProof",
            "responderEndpointId",
            "responderInitialRatchetPublicKeyBase64url",
            "responderSignatureBase64url",
            "sessionId",
        ],
        "mobile relay pairwise accepted message",
    )?;
    Ok(Some(SecureMeshPairwiseSessionAccepted {
        protocol_version: descriptor_text(value, "protocolVersion")?,
        cipher_suite: descriptor_text(value, "cipherSuite")?,
        session_id: descriptor_text(value, "sessionId")?,
        responder_endpoint_id: descriptor_text(value, "responderEndpointId")?,
        responder_initial_ratchet_public_key: decode_key_32(
            &descriptor_text(value, "responderInitialRatchetPublicKeyBase64url")?,
            "mobile relay pairwise accepted ratchet public key",
        )?
        .to_vec(),
        handshake_transcript_hash: descriptor_text(value, "handshakeTranscriptHashBase64url")?,
        responder_capability_proof: serde_json::from_value(
            value
                .get("responderCapabilityProof")
                .cloned()
                .ok_or_else(|| {
                    anyhow!("mobile relay pairwise accepted capability proof is missing")
                })?,
        )
        .map_err(|_| anyhow!("mobile relay pairwise accepted capability proof is invalid"))?,
        capability_binding: serde_json::from_value(
            value.get("capabilityBinding").cloned().ok_or_else(|| {
                anyhow!("mobile relay pairwise accepted capability binding is missing")
            })?,
        )
        .map_err(|_| anyhow!("mobile relay pairwise accepted capability binding is invalid"))?,
        responder_signature: descriptor_text(value, "responderSignatureBase64url")?,
        key_confirmation: descriptor_text(value, "keyConfirmationBase64url")?,
    }))
}

pub(in crate::domain::mobile_relay) fn pairwise_accepted_to_json(
    accepted: &SecureMeshPairwiseSessionAccepted,
) -> Value {
    json!({
        "protocolVersion": accepted.protocol_version,
        "cipherSuite": accepted.cipher_suite,
        "sessionId": accepted.session_id,
        "responderEndpointId": accepted.responder_endpoint_id,
        "responderInitialRatchetPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(&accepted.responder_initial_ratchet_public_key),
        "handshakeTranscriptHashBase64url": accepted.handshake_transcript_hash,
        "responderCapabilityProof": accepted.responder_capability_proof,
        "capabilityBinding": accepted.capability_binding,
        "responderSignatureBase64url": accepted.responder_signature,
        "keyConfirmationBase64url": accepted.key_confirmation
    })
}

pub(in crate::domain::mobile_relay) fn pairwise_finished_from_descriptor(
    descriptor: &Value,
) -> Result<Option<SecureMeshPairwiseSessionFinished>> {
    let Some(value) = descriptor
        .get("pairwiseFinished")
        .filter(|value| value.is_object())
    else {
        return Ok(None);
    };
    require_exact_object_fields(
        value,
        &[
            "capabilityTranscriptDigest",
            "cipherSuite",
            "handshakeTranscriptHashBase64url",
            "initiatorEndpointId",
            "keyConfirmationBase64url",
            "protocolVersion",
            "responderEndpointId",
            "sessionId",
        ],
        "mobile relay pairwise finished message",
    )?;
    Ok(Some(SecureMeshPairwiseSessionFinished {
        protocol_version: descriptor_text(value, "protocolVersion")?,
        cipher_suite: descriptor_text(value, "cipherSuite")?,
        session_id: descriptor_text(value, "sessionId")?,
        initiator_endpoint_id: descriptor_text(value, "initiatorEndpointId")?,
        responder_endpoint_id: descriptor_text(value, "responderEndpointId")?,
        handshake_transcript_hash: descriptor_text(value, "handshakeTranscriptHashBase64url")?,
        capability_transcript_digest: descriptor_text(value, "capabilityTranscriptDigest")?,
        key_confirmation: descriptor_text(value, "keyConfirmationBase64url")?,
    }))
}

pub(in crate::domain::mobile_relay) fn pairwise_finished_to_json(
    finished: &SecureMeshPairwiseSessionFinished,
) -> Value {
    json!({
        "protocolVersion": finished.protocol_version,
        "cipherSuite": finished.cipher_suite,
        "sessionId": finished.session_id,
        "initiatorEndpointId": finished.initiator_endpoint_id,
        "responderEndpointId": finished.responder_endpoint_id,
        "handshakeTranscriptHashBase64url": finished.handshake_transcript_hash,
        "capabilityTranscriptDigest": finished.capability_transcript_digest,
        "keyConfirmationBase64url": finished.key_confirmation
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_identity_codec_round_trip_preserves_exact_binding() {
        let identity = DeviceTrustPublicIdentity::new("device-a", [1u8; 32], [2u8; 32], 7).unwrap();
        let encoded = device_identity_to_json(&identity).unwrap();

        assert_eq!(device_identity_from_descriptor(&encoded).unwrap(), identity);
        let mut extended = encoded;
        extended["unexpected"] = json!(true);
        assert!(device_identity_from_descriptor(&extended).is_err());
    }
}
