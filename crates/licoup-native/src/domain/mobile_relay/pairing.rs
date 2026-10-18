use super::secret_custody::{MobileRelayE2eeSecretField, RuntimeSecretMaterial};
use super::{endpoint_trust::*, support::*};

fn require_pairing_secret(material: &RuntimeSecretMaterial) -> Result<()> {
    let secret = material
        .e2ee_secret(MobileRelayE2eeSecretField::PairingSecret)
        .ok_or_else(|| anyhow!("mobile relay pairing secret material is missing"))?;
    ensure!(
        !secret.expose_bytes().is_empty(),
        "mobile relay pairing secret material is empty"
    );
    Ok(())
}

pub fn pairing_create(params: &Value) -> Result<Value> {
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    effective_station_base_url(&config)?;
    config["relayEnabled"] = json!(true);
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut secret_context.material,
        "desktop_sidecar",
    )?;
    require_pairing_secret(&secret_context.material)?;
    let pairing_id = format!("pair_{}", Uuid::new_v4());
    let pairing_code = random_base64url(12);
    config["pairingId"] = json!(pairing_id);
    config["lastPairingCode"] = json!(pairing_code);
    let response = json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "pairingProtocol": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "relayContract": licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION,
        "pairingId": pairing_id,
        "pairingCode": pairing_code,
        "serverVisiblePairingState": false
    });
    let invite = one_time_pairing_invite(&config, &secret_context.material, &response);
    clear_pairing_presentation(&mut config);
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    let mut output = with_config(response, &config);
    if let (Some(object), Some(invite)) = (output.as_object_mut(), invite) {
        object.insert("mobileRelayPairingInvite".to_string(), invite);
    }
    Ok(output)
}

pub fn pairing_claim(params: &Value) -> Result<Value> {
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    effective_station_base_url(&config)?;
    config["relayEnabled"] = json!(true);
    apply_pairing_invite_params_with_context(&mut config, params, Some(&mut secret_context))?;
    let pairing_id = text_param(params, &["pairingId", "pairing_id"])
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            config
                .get("pairingId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| anyhow!("mobile relay pairing claim requires --pairing-id"))?;
    let code = text_param(params, &["pairingCode", "code"])
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            config
                .get("lastPairingCode")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("mobile relay pairing claim requires private pairingCode input"))?;
    let pc_secure_mesh = pairing_claim_secure_mesh_descriptor_from_params(params)?
        .or_else(|| peer_secure_mesh_descriptor(&config))
        .ok_or_else(|| anyhow!("mobile relay pairing claim requires PC secure mesh invite"))?;
    apply_peer_secure_mesh_descriptor_with_context(
        &mut config,
        &pc_secure_mesh,
        true,
        Some(&mut secret_context),
    )?;
    let expected_code = config
        .get("lastPairingCode")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if let Some(expected_code) = expected_code {
        ensure!(
            expected_code == code,
            "mobile relay pairing code does not match the local one-time invite"
        );
    }
    let mobile_secure_mesh = ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut secret_context.material,
        "mobile",
    )?;
    require_pairing_secret(&secret_context.material)?;
    let claim_proof = mobile_relay_claim_proof(
        &config,
        &secret_context.material,
        &pairing_id,
        &mobile_secure_mesh,
    )?;
    config["paired"] = json!(true);
    let response = json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "pairingProtocol": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "relayContract": licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION,
        "pairingId": pairing_id,
        "outOfBandPairingResponse": {
            "mobileSecureMesh": mobile_secure_mesh,
            "secureMeshClaimProof": claim_proof
        },
        "serverVisiblePairingState": false
    });
    clear_pairing_presentation(&mut config);
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(with_config(response, &config))
}

pub fn pairing_status(params: &Value) -> Result<Value> {
    if let Some(response) = params
        .get("outOfBandPairingResponse")
        .filter(|value| value.is_object())
    {
        let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
        apply_out_of_band_pairing_response_with_context(
            &mut config,
            response,
            Some(&mut secret_context),
        )?;
        save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
        return Ok(pairing_status_response(&config));
    }
    // Status reads are silent by default: secret hydration only runs when the
    // caller explicitly passes `authorize`, so opening the client never prompts.
    let (config, _) = load_config_for_read(params)?;
    Ok(pairing_status_response(&config))
}

pub(super) fn pairing_status_response(config: &Value) -> Value {
    with_config(
        json!({
            "ok": true,
            "schemaVersion": CONFIG_SCHEMA_VERSION,
            "pairingProtocol": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "relayContract": licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION,
            "stationConfigured": effective_station_base_url(config).is_ok(),
            "paired": config.get("paired").and_then(Value::as_bool).unwrap_or(false),
            "serverVisiblePairingState": false
        }),
        config,
    )
}

pub(super) fn refresh_pairing_status_with_context(
    params: &Value,
    config: &mut Value,
    secret_context: &mut RuntimeSecretContext,
) -> Result<Value> {
    let _ = secret_context;
    let response = json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "pairingProtocol": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "relayContract": licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION,
        "stationConfigured": effective_station_base_url(config).is_ok(),
        "paired": config.get("paired").and_then(Value::as_bool).unwrap_or(false),
        "serverVisiblePairingState": false
    });
    let _ = params;
    Ok(response)
}

pub(super) fn refresh_pairwise_acceptance_if_pending(
    params: &Value,
    config: &mut Value,
    secret_context: &mut RuntimeSecretContext,
) -> Result<()> {
    let Some(state) = config.get("mobileRelayE2ee") else {
        return Ok(());
    };
    let endpoint_kind = state
        .get("endpointKind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let paired = config
        .get("paired")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pending_intro = state.get("pendingPairwiseIntro").is_some();
    if endpoint_kind != "mobile" || !paired || !pending_intro {
        return Ok(());
    }
    let _ = refresh_pairing_status_with_context(params, config, secret_context)?;
    Ok(())
}

pub fn pairing_revoke(params: &Value) -> Result<Value> {
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut secret_context.material,
        "desktop_sidecar",
    )?;
    clear_mobile_relay_pairing_state(&mut config)?;
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(with_config(
        json!({
            "ok": true,
            "schemaVersion": CONFIG_SCHEMA_VERSION,
            "mailboxRotated": true,
            "serverVisiblePairingState": false
        }),
        &config,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::config::default_config;
    use super::*;

    #[test]
    fn pairing_module_projects_local_pairing_status() {
        let mut config = default_config();
        config["stationBaseUrl"] = json!("https://station.example.test");
        config["paired"] = json!(true);

        let status = pairing_status_response(&config);

        assert_eq!(status["stationConfigured"], true);
        assert_eq!(status["paired"], true);
        assert_eq!(status["serverVisiblePairingState"], false);
    }
}
