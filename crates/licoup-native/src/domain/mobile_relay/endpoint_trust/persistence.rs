use super::*;

pub(crate) fn persisted_mobile_relay_peer_trust_state(
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
) -> Result<DeviceTrustState> {
    ensure_secure_mesh_protected_operation_allowed()?;
    ensure!(
        local_public_device_identity(config)? == *local_identity,
        "secure mesh MLS persisted local trust identity differs"
    );
    let scope = configured_directory_scope_commitment(config)?;
    let stable_label = stable_directory_label(scope, &peer_identity.endpoint_id);
    let authority = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("peerTrustAuthority"))
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("secure mesh MLS persisted trust authority is unavailable"))?;
    ensure!(
        authority.get("schemaVersion").and_then(Value::as_str)
            == Some(SECURE_MESH_PEER_TRUST_AUTHORITY_SCHEMA),
        "secure mesh MLS persisted trust authority schema is invalid"
    );
    let entries = authority
        .get("entries")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("secure mesh MLS persisted trust authority entries are missing"))?;
    ensure!(
        entries.len() <= MAX_SECURE_MESH_PEER_TRUST_ENTRIES,
        "secure mesh MLS persisted trust authority exceeds its bound"
    );
    let entry = entries
        .get(&stable_label)
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            anyhow!("secure mesh MLS peer is absent from the persisted trust authority")
        })?;
    ensure!(
        entry.get("stableLabel").and_then(Value::as_str) == Some(stable_label.as_str()),
        "secure mesh MLS persisted peer trust label binding is invalid"
    );
    let identity_value = entry
        .get("identity")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("secure mesh MLS persisted peer identity is missing"))?;
    let persisted_identity = DeviceTrustPublicIdentity::new(
        descriptor_text(identity_value, "endpointId")?,
        decode_key_32(
            &descriptor_text(identity_value, "identityPublicKeyBase64url")?,
            "secure mesh persisted peer identity public key",
        )?,
        decode_key_32(
            &descriptor_text(identity_value, "signingPublicKeyBase64url")?,
            "secure mesh persisted peer signing public key",
        )?,
        identity_value
            .get("rotationEpoch")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("secure mesh persisted peer rotation epoch is missing"))?,
    )?;
    ensure!(
        persisted_identity == *peer_identity,
        "secure mesh MLS persisted peer identity binding differs"
    );
    let record = entry
        .get("trustRecord")
        .ok_or_else(|| anyhow!("secure mesh MLS persisted peer trust record is missing"))?;
    let trust_state = verify_device_trust_record_json(
        local_identity,
        peer_identity,
        record,
        mobile_relay_trust_record_now_epoch()?,
    )?;
    ensure!(
        trust_state == DeviceTrustState::Verified,
        "secure mesh MLS persisted peer trust is not verified"
    );
    Ok(trust_state)
}

#[cfg(test)]
pub(super) fn persist_peer_trust_authority_entry(
    config: &mut Value,
    local_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
    trust_record: &Value,
) -> Result<()> {
    ensure!(
        verify_device_trust_record_json(
            local_identity,
            peer_identity,
            trust_record,
            mobile_relay_trust_record_now_epoch()?,
        )? == DeviceTrustState::Verified,
        "secure mesh peer trust authority only accepts verified records"
    );
    let stable_label = stable_directory_label(
        configured_directory_scope_commitment(config)?,
        &peer_identity.endpoint_id,
    );
    if config["mobileRelayE2ee"]
        .get("peerTrustAuthority")
        .is_none()
    {
        config["mobileRelayE2ee"]["peerTrustAuthority"] = json!({
            "schemaVersion": SECURE_MESH_PEER_TRUST_AUTHORITY_SCHEMA,
            "entries": {}
        });
    }
    let authority = config["mobileRelayE2ee"]
        .get_mut("peerTrustAuthority")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("secure mesh peer trust authority is invalid"))?;
    ensure!(
        authority.get("schemaVersion").and_then(Value::as_str)
            == Some(SECURE_MESH_PEER_TRUST_AUTHORITY_SCHEMA),
        "secure mesh peer trust authority schema is invalid"
    );
    let entries = authority
        .get_mut("entries")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("secure mesh peer trust authority entries are invalid"))?;
    ensure!(
        entries.contains_key(&stable_label) || entries.len() < MAX_SECURE_MESH_PEER_TRUST_ENTRIES,
        "secure mesh peer trust authority is at capacity"
    );
    entries.insert(
        stable_label.clone(),
        json!({
            "stableLabel": stable_label,
            "identity": {
                "endpointId": peer_identity.endpoint_id,
                "identityPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(peer_identity.identity_public_key),
                "signingPublicKeyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(peer_identity.signing_public_key),
                "rotationEpoch": peer_identity.rotation_epoch,
            },
            "trustRecord": trust_record,
        }),
    );
    Ok(())
}

#[cfg(test)]
pub(super) fn remove_peer_trust_authority_entry(
    config: &mut Value,
    peer_endpoint_id: &str,
) -> Result<()> {
    let scope = configured_directory_scope_commitment(config)?.to_string();
    let stable_label = stable_directory_label(&scope, peer_endpoint_id);
    if let Some(entries) = config
        .get_mut("mobileRelayE2ee")
        .and_then(|state| state.get_mut("peerTrustAuthority"))
        .and_then(|authority| authority.get_mut("entries"))
        .and_then(Value::as_object_mut)
    {
        entries.remove(&stable_label);
    }
    Ok(())
}

pub(crate) fn secure_mesh_mls_state_dir() -> Result<PathBuf> {
    let directory = ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("secure-mesh-mls");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

pub(crate) fn secure_mesh_mls_public_directory_context()
-> Result<(Value, DeviceTrustPublicIdentity)> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let config = load_config_without_persistence()?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("secure mesh MLS local endpoint state is unavailable"))?;
    let identity = DeviceTrustPublicIdentity::new(
        descriptor_text(state, "endpointId")?,
        decode_key_32(
            &descriptor_text(state, "publicKeyBase64url")?,
            "secure mesh MLS local identity public key",
        )?,
        decode_key_32(
            &descriptor_text(state, "signingPublicKeyBase64url")?,
            "secure mesh MLS local signing public key",
        )?,
        state
            .get("rotationEpoch")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("secure mesh MLS local rotation epoch is unavailable"))?,
    )?;
    Ok((config, identity))
}

pub(crate) fn secure_mesh_kt_authority_path(local_endpoint_id: &str) -> Result<PathBuf> {
    ensure!(
        !local_endpoint_id.trim().is_empty(),
        "secure mesh KT local endpoint id is required"
    );
    let directory = ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("secure-mesh-kt");
    fs::create_dir_all(&directory)?;
    Ok(directory.join(format!(
        "{}.sqlite3",
        sha256_hex(local_endpoint_id.as_bytes())
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_trust_removal_is_scoped_to_the_stable_directory_label() {
        let scope = "a".repeat(64);
        let peer_endpoint_id = "device-a";
        let stable_label = stable_directory_label(&scope, peer_endpoint_id);
        let mut entries = serde_json::Map::new();
        entries.insert(stable_label.clone(), json!({"fixture": true}));
        entries.insert("unrelated".to_string(), json!({"fixture": true}));
        let mut config = json!({
            "secureMeshDirectoryScopeCommitment": scope,
            "mobileRelayE2ee": {
                "peerTrustAuthority": {"entries": Value::Object(entries)}
            }
        });

        remove_peer_trust_authority_entry(&mut config, peer_endpoint_id).unwrap();

        let entries = config["mobileRelayE2ee"]["peerTrustAuthority"]["entries"]
            .as_object()
            .unwrap();
        assert!(!entries.contains_key(&stable_label));
        assert!(entries.contains_key("unrelated"));
    }
}
