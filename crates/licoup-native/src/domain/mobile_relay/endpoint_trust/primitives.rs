use super::*;
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
};

pub(in crate::domain::mobile_relay) fn session_id(config: &Value) -> Result<String> {
    config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("mobile relay E2EE session id is missing"))
}

pub(in crate::domain::mobile_relay) fn mobile_relay_claim_proof(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    pairing_id: &str,
    descriptor: &Value,
) -> Result<String> {
    mobile_relay_claim_proof_for(config, secret_material, pairing_id, descriptor)
}

pub(in crate::domain::mobile_relay) fn mobile_relay_claim_proof_for(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    pairing_id: &str,
    mobile_descriptor: &Value,
) -> Result<String> {
    let pc_descriptor = peer_secure_mesh_descriptor(config)
        .ok_or_else(|| anyhow!("mobile relay PC secure mesh descriptor is missing"))?;
    mobile_relay_claim_proof_for_pair(
        config,
        secret_material,
        pairing_id,
        mobile_descriptor,
        &pc_descriptor,
    )
}

pub(in crate::domain::mobile_relay) fn mobile_relay_claim_proof_for_pair(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    pairing_id: &str,
    mobile_descriptor: &Value,
    pc_descriptor: &Value,
) -> Result<String> {
    let mac = mobile_relay_claim_proof_mac(
        config,
        secret_material,
        pairing_id,
        mobile_descriptor,
        pc_descriptor,
    )?;
    Ok(general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

pub(in crate::domain::mobile_relay) fn mobile_relay_claim_proof_matches(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    pairing_id: &str,
    mobile_descriptor: &Value,
    pc_descriptor: &Value,
    provided_proof: &str,
) -> Result<bool> {
    let Ok(provided) = general_purpose::URL_SAFE_NO_PAD.decode(provided_proof) else {
        return Ok(false);
    };
    if provided.len() != MOBILE_RELAY_KEY_BYTES {
        return Ok(false);
    }
    let mac = mobile_relay_claim_proof_mac(
        config,
        secret_material,
        pairing_id,
        mobile_descriptor,
        pc_descriptor,
    )?;
    Ok(mac.verify_slice(&provided).is_ok())
}

pub(in crate::domain::mobile_relay) fn mobile_relay_claim_proof_mac(
    _config: &Value,
    secret_material: &RuntimeSecretMaterial,
    pairing_id: &str,
    mobile_descriptor: &Value,
    pc_descriptor: &Value,
) -> Result<MobileRelayClaimMac> {
    let secret = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::PairingSecret)
        .ok_or_else(|| anyhow!("mobile relay E2EE pairing secret is missing"))?;
    let secret_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(secret.expose_bytes())
        .map_err(|_| anyhow!("mobile relay E2EE pairing secret is not base64url"))?;
    ensure!(
        secret_bytes.len() == MOBILE_RELAY_KEY_BYTES,
        "mobile relay E2EE pairing secret length is invalid"
    );
    let mobile_binding =
        serde_json::to_vec(&mobile_relay_claim_descriptor_binding(mobile_descriptor)?)?;
    let pc_binding = serde_json::to_vec(&mobile_relay_claim_descriptor_binding(pc_descriptor)?)?;
    let mut mac = <MobileRelayClaimMac as Mac>::new_from_slice(&secret_bytes)
        .map_err(|_| anyhow!("mobile relay claim proof initialization failed"))?;
    mac.update(b"licomesh.mobile-relay.e2ee.claim-proof.v2");
    update_mobile_relay_claim_mac_field(&mut mac, MOBILE_RELAY_E2EE_PROTOCOL_VERSION.as_bytes())?;
    update_mobile_relay_claim_mac_field(&mut mac, pairing_id.as_bytes())?;
    update_mobile_relay_claim_mac_field(&mut mac, &mobile_binding)?;
    update_mobile_relay_claim_mac_field(&mut mac, &pc_binding)?;
    Ok(mac)
}

pub(in crate::domain::mobile_relay) fn update_mobile_relay_claim_mac_field(
    mac: &mut MobileRelayClaimMac,
    value: &[u8],
) -> Result<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| anyhow!("mobile relay claim proof field is too large"))?;
    mac.update(&length.to_be_bytes());
    mac.update(value);
    Ok(())
}

pub(in crate::domain::mobile_relay) fn mobile_relay_claim_descriptor_binding(
    descriptor: &Value,
) -> Result<Value> {
    let prekey_bundle = pairwise_prekey_bundle_from_descriptor(descriptor)?;
    let prekey_bundle_json = descriptor
        .get("preKeyBundle")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("mobile relay peer secure mesh descriptor missing preKeyBundle"))?;
    let pairwise_intro = descriptor
        .get("pairwiseIntro")
        .cloned()
        .unwrap_or(Value::Null);
    let pairwise_accepted = descriptor
        .get("pairwiseAccepted")
        .cloned()
        .unwrap_or(Value::Null);
    let pairwise_finished = descriptor
        .get("pairwiseFinished")
        .cloned()
        .unwrap_or(Value::Null);
    let identity = prekey_bundle.endpoint_identity;
    Ok(json!({
        "endpointId": descriptor_text(descriptor, "endpointId")?,
        "endpointKind": descriptor_text(descriptor, "endpointKind")?,
        "publicKeyBase64url": descriptor_text(descriptor, "publicKeyBase64url")?,
        "identityPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(identity.identity_public_key),
        "signingPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(identity.signing_public_key),
        "rotationEpoch": identity.rotation_epoch,
        "deviceTrustFingerprint": identity.fingerprint()?,
        "preKeyBundleHash": stable_json_sha256(prekey_bundle_json),
        "pairwiseIntroHash": stable_json_sha256(&pairwise_intro),
        "pairwiseAcceptedHash": stable_json_sha256(&pairwise_accepted),
        "pairwiseFinishedHash": stable_json_sha256(&pairwise_finished)
    }))
}

pub(in crate::domain::mobile_relay) fn descriptor_text(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("mobile relay secure mesh descriptor missing {}", key))
}

pub(in crate::domain::mobile_relay) fn descriptor_sha256_hex(
    value: &Value,
    key: &str,
) -> Result<String> {
    let digest = descriptor_text(value, key)?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "mobile relay descriptor {key} must be canonical lowercase SHA-256 hex"
    );
    Ok(digest)
}

pub(in crate::domain::mobile_relay) fn decode_key_32(
    value: &str,
    label: &str,
) -> Result<[u8; MOBILE_RELAY_KEY_BYTES]> {
    decode_fixed_base64url(value, label)
}

pub(in crate::domain::mobile_relay) fn decode_fixed_base64url<const BYTES: usize>(
    value: &str,
    label: &str,
) -> Result<[u8; BYTES]> {
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| anyhow!("{} is not base64url", label))?;
    ensure!(
        general_purpose::URL_SAFE_NO_PAD.encode(&bytes) == value,
        "{} must use canonical unpadded base64url",
        label
    );
    let fixed: [u8; BYTES] = bytes
        .try_into()
        .map_err(|_| anyhow!("{} must be {} bytes", label, BYTES))?;
    Ok(fixed)
}

pub(in crate::domain::mobile_relay) fn public_key_fingerprint(
    bytes: &[u8; MOBILE_RELAY_KEY_BYTES],
) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

pub(in crate::domain::mobile_relay) fn random_base64url(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    OsRng.fill_bytes(&mut bytes);
    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(in crate::domain::mobile_relay) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{:02x}", byte)).collect()
}

pub(in crate::domain::mobile_relay) fn prekey_public_key_hash(public_key: &[u8]) -> String {
    let mut material = b"LCOSM-ONE-TIME-PREKEY-PUBLIC-v1".to_vec();
    material.extend_from_slice(public_key);
    format!("sha256:{}", sha256_hex(&material))
}

pub(in crate::domain::mobile_relay) fn now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(in crate::domain::mobile_relay) fn timestamp_after_seconds(seconds: i64) -> Result<String> {
    Ok((OffsetDateTime::now_utc() + Duration::seconds(seconds)).format(&Rfc3339)?)
}

pub(in crate::domain::mobile_relay) fn bounded_time_window(
    lifetime_seconds: i64,
) -> Result<(String, String)> {
    let created_at = OffsetDateTime::now_utc();
    Ok((
        created_at.format(&Rfc3339)?,
        (created_at + Duration::seconds(lifetime_seconds)).format(&Rfc3339)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_digest_and_base64url_decoding_are_canonical() {
        let value = json!({"digest": "b".repeat(64)});
        assert_eq!(
            descriptor_sha256_hex(&value, "digest").unwrap(),
            "b".repeat(64)
        );

        let mut uppercase = value;
        uppercase["digest"] = json!("B".repeat(64));
        assert!(descriptor_sha256_hex(&uppercase, "digest").is_err());
        assert!(
            decode_fixed_base64url::<32>(&general_purpose::URL_SAFE.encode([7u8; 32]), "key")
                .is_err()
        );
    }
}
