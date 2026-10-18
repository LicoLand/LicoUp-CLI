use super::*;

pub(in crate::domain::mobile_relay) fn save_config_with_runtime_secret_overrides(
    config: &mut Value,
    overrides: &RuntimeSecretOverrides,
) -> Result<()> {
    prepare_station_fields_for_persistence(config)?;
    let mut persistable = config.clone();
    persist_config_secret_material_to_native_store(&mut persistable)?;
    strip_runtime_secret_overrides(&mut persistable, overrides);
    save_config_raw(&mut persistable)?;
    copy_committed_security_generations(config, &persistable)
}

pub(in crate::domain::mobile_relay) fn save_config_with_runtime_secret_context(
    config: &mut Value,
    context: &mut RuntimeSecretContext,
) -> Result<()> {
    if context
        .material
        .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
        .is_some()
        || context
            .material
            .e2ee_secret(MobileRelayE2eeSecretField::SigningKey)
            .is_some()
    {
        validate_existing_identity_custody(config, &context.material)?;
    }
    prepare_station_fields_for_persistence(config)?;
    let mut persistable = config.clone();
    persist_config_secret_material_to_native_store_with_batch(
        &mut persistable,
        &mut context.secret_store_batch,
    )?;
    persist_runtime_secret_material_to_native_store_with_batch(
        &mut context.material,
        &mut context.secret_store_batch,
    )?;
    context.overrides.identity_custody_verified = Some(
        context
            .material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_some()
            && context
                .material
                .e2ee_secret(MobileRelayE2eeSecretField::SigningKey)
                .is_some()
            && validate_existing_identity_custody(config, &context.material).is_ok(),
    );
    for field in MobileRelayE2eeSecretField::ALL {
        if context.material.e2ee_secret(field).is_some() {
            mark_native_secret_override(&mut context.overrides, field.config_field());
        }
    }
    strip_runtime_secret_overrides(&mut persistable, &context.overrides);
    if let Some(namespace) = context.secret_store_batch.verified_namespace() {
        if let Some(e2ee) = persistable
            .get_mut("mobileRelayE2ee")
            .and_then(Value::as_object_mut)
        {
            e2ee.insert(CUSTODY_NAMESPACE_FIELD.to_string(), json!(namespace));
        }
    }
    save_config_raw(&mut persistable)?;
    copy_committed_security_generations(config, &persistable)
}

pub(in crate::domain::mobile_relay) fn save_config_with_runtime_secret_context_for_authority_reset(
    config: &mut Value,
    context: &mut RuntimeSecretContext,
) -> Result<()> {
    prepare_station_fields_for_persistence(config)?;
    let mut persistable = config.clone();
    persist_config_secret_material_to_native_store_with_batch(
        &mut persistable,
        &mut context.secret_store_batch,
    )?;
    persist_runtime_secret_material_to_native_store_with_batch(
        &mut context.material,
        &mut context.secret_store_batch,
    )?;
    strip_runtime_secret_overrides(&mut persistable, &context.overrides);
    save_config_raw_with_reset_policy(&mut persistable, true)?;
    copy_committed_security_generations(config, &persistable)
}

fn copy_committed_security_generations(target: &mut Value, committed: &Value) -> Result<()> {
    validate_config_generations(committed)?;
    target[CONFIG_GENERATION_FIELD] = committed
        .get(CONFIG_GENERATION_FIELD)
        .cloned()
        .ok_or_else(|| anyhow!("mobile relay committed config generation is missing"))?;
    target[AUTHORITY_GENERATION_FIELD] = committed
        .get(AUTHORITY_GENERATION_FIELD)
        .cloned()
        .ok_or_else(|| anyhow!("mobile relay committed authority generation is missing"))?;
    if let Some(namespace) = recorded_custody_namespace(committed)? {
        if let Some(e2ee) = target
            .get_mut("mobileRelayE2ee")
            .and_then(Value::as_object_mut)
        {
            e2ee.insert(CUSTODY_NAMESPACE_FIELD.to_string(), json!(namespace));
        }
    }
    Ok(())
}

fn strip_runtime_secret_overrides(config: &mut Value, overrides: &RuntimeSecretOverrides) {
    if overrides.pc_token {
        config["pcToken"] = json!("");
    }
    if overrides.mobile_token || !overrides.paired_device_tokens.is_empty() {
        config["mobileToken"] = json!("");
    }
    let selected_pairing_id = config
        .get("pairingId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let selected_paired_device_credential = config
        .get("pairedDevices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|device| {
            !selected_pairing_id.is_empty()
                && device
                    .get("pairingId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    == Some(selected_pairing_id)
        })
        .any(|device| {
            overrides
                .paired_device_tokens
                .iter()
                .any(|entry| paired_device_override_matches(device, entry))
        });
    if overrides.mobile_token || selected_paired_device_credential {
        config["mobileTokenPresent"] = json!(true);
    }
    if let Some(e2ee) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        if overrides.e2ee_private_key {
            e2ee.remove("privateKeyBase64url");
            e2ee.insert("privateKeyMaterial".to_string(), json!("redacted"));
        }
        if overrides.e2ee_signing_key {
            e2ee.remove("signingKeyBase64url");
            e2ee.insert("signingKeyMaterial".to_string(), json!("redacted"));
        }
        if overrides.e2ee_signed_prekey_private_key {
            e2ee.remove("signedPrekeyPrivateKeyBase64url");
            e2ee.insert(
                "signedPrekeyPrivateKeyMaterial".to_string(),
                json!("redacted"),
            );
        }
        if overrides.e2ee_one_time_prekey_private_key {
            e2ee.remove("oneTimePrekeyPrivateKeyBase64url");
            e2ee.insert(
                "oneTimePrekeyPrivateKeyMaterial".to_string(),
                json!("redacted"),
            );
        }
        if overrides.e2ee_one_time_mlkem1024_prekey_seed {
            e2ee.remove("oneTimeMlKem1024PrekeySeedBase64url");
            e2ee.insert(
                "oneTimeMlKem1024PrekeySeedMaterial".to_string(),
                json!("redacted"),
            );
        }
        if overrides.e2ee_pairing_secret {
            e2ee.remove("pairingSecretBase64url");
            e2ee.insert("pairingSecretMaterial".to_string(), json!("redacted"));
        }
        if overrides.e2ee_private_key
            || overrides.e2ee_signing_key
            || overrides.e2ee_signed_prekey_private_key
            || overrides.e2ee_one_time_prekey_private_key
            || overrides.e2ee_one_time_mlkem1024_prekey_seed
            || overrides.e2ee_pairing_secret
        {
            let backend = secret_storage_backend_for_overrides(overrides);
            e2ee.insert("secretStorageStatus".to_string(), json!(backend));
        }
    }
    if let Some(devices) = config
        .get_mut("pairedDevices")
        .and_then(Value::as_array_mut)
    {
        for device in devices {
            let should_strip = overrides
                .paired_device_tokens
                .iter()
                .any(|entry| paired_device_override_matches(device, entry));
            if should_strip {
                device["mobileToken"] = json!("");
                device["credentialPresent"] = json!(true);
            }
        }
    }
    if has_runtime_secret_overrides(overrides) {
        config["secretStorageStatus"] = json!({
            "tokenMaterial": "redacted",
            "mobileRelayPrivateKeyMaterial": "redacted",
            "selectedBackend": secret_storage_backend_for_overrides(overrides),
            "unsafePersistenceForbidden": true
        });
    }
}

pub(in crate::domain::mobile_relay) fn secret_storage_backend_for_overrides(
    overrides: &RuntimeSecretOverrides,
) -> &'static str {
    overrides
        .secret_storage_backend
        .unwrap_or("memory-only-ephemeral")
}

fn paired_device_override_matches(device: &Value, entry: &PairedDeviceSecretOverride) -> bool {
    let id_matches = !entry.id.is_empty()
        && device
            .get("id")
            .or_else(|| device.get("pcClientId"))
            .and_then(Value::as_str)
            .map(str::trim)
            == Some(entry.id.as_str());
    let pairing_matches = !entry.pairing_id.is_empty()
        && device
            .get("pairingId")
            .and_then(Value::as_str)
            .map(str::trim)
            == Some(entry.pairing_id.as_str());
    id_matches || pairing_matches
}

pub(in crate::domain::mobile_relay) fn paired_device_token_secret_store_key(
    device: &Value,
) -> Option<String> {
    let suffix = first_non_blank(&[
        paired_device_pairing_id(device),
        paired_device_id(device),
        "unknown".to_string(),
    ])?;
    Some(format!(
        "pairedDevices.{}.mobileToken",
        sha256_hex(suffix.as_bytes())
    ))
}

pub(in crate::domain::mobile_relay) fn paired_device_id(device: &Value) -> String {
    device
        .get("id")
        .or_else(|| device.get("pcClientId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub(in crate::domain::mobile_relay) fn paired_device_pairing_id(device: &Value) -> String {
    device
        .get("pairingId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn first_non_blank(values: &[String]) -> Option<String> {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

pub(in crate::domain::mobile_relay) fn has_runtime_secret_overrides(
    overrides: &RuntimeSecretOverrides,
) -> bool {
    overrides.pc_token
        || overrides.mobile_token
        || overrides.e2ee_private_key
        || overrides.e2ee_signing_key
        || overrides.e2ee_signed_prekey_private_key
        || overrides.e2ee_one_time_prekey_private_key
        || overrides.e2ee_one_time_mlkem1024_prekey_seed
        || overrides.e2ee_pairing_secret
        || !overrides.paired_device_tokens.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_device_secret_handle_uses_stable_opaque_pairing_binding() {
        let first = paired_device_token_secret_store_key(
            &json!({"id": "device-a", "pairingId": "pairing-a"}),
        )
        .unwrap();
        let second = paired_device_token_secret_store_key(
            &json!({"id": "device-b", "pairingId": "pairing-a"}),
        )
        .unwrap();

        assert_eq!(first, second);
        assert!(first.starts_with("pairedDevices."));
        assert!(first.ends_with(".mobileToken"));
        assert!(!first.contains("pairing-a"));
    }
}
