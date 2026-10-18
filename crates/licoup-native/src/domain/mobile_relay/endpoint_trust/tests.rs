use super::persistence::persist_peer_trust_authority_entry;
use super::*;

#[cfg(test)]
pub(crate) fn initialize_secure_mesh_mls_test_endpoint(endpoint_kind: &str) -> Result<()> {
    let mut config = default_config();
    let mut context = crate::domain::mobile_relay::secret_custody::RuntimeSecretContext::default();
    ensure_mobile_relay_endpoint_material(&mut config, &mut context.material, endpoint_kind)?;
    ensure_mobile_relay_key_transparency(&mut config)?;
    crate::domain::mobile_relay::secret_custody::save_config_with_runtime_secret_context(
        &mut config,
        &mut context,
    )
}

#[cfg(test)]
pub(crate) fn initialize_secure_mesh_mls_test_peer(
    peer_identity: &DeviceTrustPublicIdentity,
) -> Result<()> {
    let (mut config, mut context) = load_config_with_runtime_secret_context_for_operation(
        &json!({"allowInteraction": true}),
        "Secure Mesh MLS test peer authority",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count(),
    )?;
    let local_endpoint = local_endpoint_state(&config, &context.material)?;
    let local_identity = local_endpoint.device_identity()?;
    let issued_at = mobile_relay_trust_record_now_epoch()?;
    let trust_record = sign_device_trust_record(
        &local_endpoint.signing_key()?,
        &local_identity,
        peer_identity,
        DeviceTrustState::Verified,
        peer_identity.rotation_epoch,
        "test_persisted_pairing_authority",
        issued_at,
        mobile_relay_trust_record_expiry_epoch(issued_at)?,
    )?;
    let trust_record_json = device_trust_record_to_json(&trust_record);
    persist_peer_trust_authority_entry(
        &mut config,
        &local_identity,
        peer_identity,
        &trust_record_json,
    )?;
    config["mobileRelayE2ee"]["peerEndpointId"] = json!(peer_identity.endpoint_id);
    config["mobileRelayE2ee"]["peerEndpointKind"] = json!("secure_mesh_mls_test_peer");
    config["mobileRelayE2ee"]["peerPublicKeyBase64url"] =
        json!(general_purpose::URL_SAFE_NO_PAD.encode(peer_identity.identity_public_key));
    config["mobileRelayE2ee"]["peerSigningPublicKeyBase64url"] =
        json!(general_purpose::URL_SAFE_NO_PAD.encode(peer_identity.signing_public_key));
    config["mobileRelayE2ee"]["peerRotationEpoch"] = json!(peer_identity.rotation_epoch);
    config["mobileRelayE2ee"]["peerVerified"] = json!(true);
    config["mobileRelayE2ee"]["peerTrustRecord"] = trust_record_json;
    refresh_secure_mesh_mls_test_directory_authority(&mut config)?;
    crate::domain::mobile_relay::secret_custody::save_config_with_runtime_secret_context(
        &mut config,
        &mut context,
    )
}

#[cfg(test)]
pub(crate) fn secure_mesh_mls_test_directory_response(
    member_identity: &DeviceTrustPublicIdentity,
    member_key_package: &[u8],
    directory_version: u64,
    key_package_version: u64,
) -> Result<Value> {
    let config = load_config()?;
    let local_endpoint_id = descriptor_text(
        config
            .get("mobileRelayE2ee")
            .ok_or_else(|| anyhow!("secure mesh MLS test local endpoint is missing"))?,
        "endpointId",
    )?;
    let authority = open_mobile_relay_directory_authority(&config, &local_endpoint_id)?;
    let previous_tree_size = authority
        .latest_checkpoint()?
        .map(|checkpoint| checkpoint.tree_size);
    let claim = SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment(
                "local-test-tenant",
                "local-test-account",
                "local-test-workspace",
            ),
            endpoint_id: member_identity.endpoint_id.clone(),
            endpoint_kind: "mls-test-member".to_string(),
            identity_public_key: hex_encode_bytes(&member_identity.identity_public_key),
            signing_public_key: hex_encode_bytes(&member_identity.signing_public_key),
            fingerprint: member_identity.fingerprint()?,
            rotation_epoch: member_identity.rotation_epoch,
            directory_state: "active".to_string(),
            updated_at: now_iso(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: sha256_hex(b"mls-test-signed-prekey"),
            one_time_prekey_batch_digest: sha256_hex(b"mls-test-one-time-prekeys"),
            pairwise_prekey_version: 1,
            mls_key_package_digest: sha256_hex(member_key_package),
            mls_key_package_version: key_package_version,
        },
        directory_version,
    };
    let now_epoch_seconds = mobile_relay_trust_record_now_epoch()?;
    let response = with_mobile_relay_test_kt_log(|log| {
        let index = log.append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash()?,
        )?;
        Ok(UntrustedDirectoryResponse {
            claim: claim.clone(),
            inclusion: log.inclusion_proof_at(index, now_epoch_seconds)?,
            latest_map: log.map_proof_at(&claim.stable_label(), now_epoch_seconds)?,
            consistency: previous_tree_size
                .filter(|size| *size < log.tree_size())
                .map(|size| log.consistency_proof_at(size, now_epoch_seconds))
                .transpose()?,
        })
    })?;
    serde_json::to_value(response).map_err(Into::into)
}

#[cfg(test)]
pub(crate) fn refresh_secure_mesh_mls_test_directory_authority(config: &mut Value) -> Result<()> {
    if config
        .get("secureMeshKeyTransparency")
        .and_then(|settings| settings.get("pin"))
        .and_then(|pin| pin.get("provenance"))
        .and_then(Value::as_str)
        != Some("local-acceptance-mock")
    {
        return Ok(());
    }
    ensure_mobile_relay_key_transparency(config)
}

#[test]
fn endpoint_trust_module_public_projection_redacts_secret_material() {
    let mut config = default_config();
    config["pcToken"] = json!("private-pc-token");
    config["mobileToken"] = json!("private-mobile-token");

    let public = public_config(&config);

    assert_eq!(public["pcToken"], "");
    assert_eq!(public["mobileToken"], "");
    assert_eq!(public["pcTokenPresent"], true);
    assert_eq!(public["mobileTokenPresent"], true);
}
