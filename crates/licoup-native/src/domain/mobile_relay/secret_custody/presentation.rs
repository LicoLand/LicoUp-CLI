use super::*;

pub(in crate::domain::mobile_relay) fn secret_present(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .map(is_unredacted_secret)
        .unwrap_or(false)
}

pub(in crate::domain::mobile_relay) fn is_unredacted_secret(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed != "redacted" && trimmed != "***" && trimmed != "********"
}

pub(in crate::domain::mobile_relay) fn apply_selected_paired_device_credentials(
    config: &mut Value,
) {
    let pairing_id = config
        .get("pairingId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if pairing_id.is_empty() {
        return;
    }
    let matching_device = config
        .get("pairedDevices")
        .and_then(Value::as_array)
        .and_then(|devices| {
            devices.iter().find(|device| {
                device
                    .get("pairingId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    == Some(pairing_id.as_str())
            })
        })
        .cloned();
    let Some(device) = matching_device else {
        return;
    };
    if let Some(token) = device
        .get("mobileToken")
        .and_then(Value::as_str)
        .filter(|value| is_unredacted_secret(value))
    {
        config["mobileToken"] = json!(token);
    }
    if let Some(pc_client_id) = device
        .get("pcClientId")
        .or_else(|| device.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        config["pcClientId"] = json!(pc_client_id.trim());
    }
    if let Some(pc_client_name) = device
        .get("pcClientName")
        .or_else(|| device.get("label"))
        .or_else(|| device.get("name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        config["pcClientName"] = json!(pc_client_name.trim());
    }
}

pub(in crate::domain::mobile_relay) fn delete_mobile_relay_pairing_token_secrets(
    config: &Value,
    batch: &mut MobileRelaySecretStoreAuthBatch,
) -> Result<()> {
    let Some((store, session, namespace)) = batch.authorization()? else {
        return Ok(());
    };
    let mut handles = MOBILE_RELAY_NATIVE_TOKEN_SECRET_FIELDS
        .iter()
        .map(|field| native_secret_store_handle_for_namespace(&namespace, field))
        .collect::<Result<Vec<_>>>()?;
    if let Some(devices) = config.get("pairedDevices").and_then(Value::as_array) {
        for device in devices {
            if let Some(key) = paired_device_token_secret_store_key(device) {
                handles.push(native_secret_store_handle_for_namespace(&namespace, &key)?);
            }
        }
    }
    handles.sort_by(|left, right| left.key().cmp(right.key()));
    handles.dedup();
    for handle in handles {
        store.delete_secret_with_session(&session, &handle)?;
    }
    Ok(())
}

pub(in crate::domain::mobile_relay) fn public_secret_storage_backend(config: &Value) -> String {
    config
        .get("mobileRelayE2ee")
        .and_then(|value| value.get("secretStorageStatus"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            config
                .get("secretStorageStatus")
                .and_then(|value| value.get("selectedBackend"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("portable_config_pending_platform_secret_store")
        .to_string()
}

pub(in crate::domain::mobile_relay) fn secret_store_authorization_report(
    session: &SecretStoreAuthorizationSession,
) -> Value {
    json!({
        "backend": session.backend(),
        "operationCount": session.operation_count(),
        "consumedOperationCount": session.consumed_operation_count(),
        "remainingOperationCount": session.remaining_operation_count(),
        "authorizationBatchWithinBudget": session.authorization_batch_within_budget(),
        "allowInteraction": session.allow_interaction(),
        "sharedSystemAuthorizationContextRequired": session.shared_system_context_required(),
        "sharedSystemAuthorizationContextAvailable": session.shared_system_context_available(),
        "singleSystemAuthorizationContextVerified": session.single_system_authorization_context_verified(),
        "systemAuthorizationAttemptCount": session.system_authorization_attempt_count(),
        "systemAuthorizationCompleted": session.system_authorization_completed(),
        "authorizationBatchPromptBudgetReady": !session.shared_system_context_required() ||
            (session.system_authorization_attempt_count() == 1 &&
                session.system_authorization_completed()),
        "appCredentialPromptUsed": false,
        "appPasswordPromptUsed": session.app_password_prompt_used(),
        "keyMaterialExported": false
    })
}

pub(in crate::domain::mobile_relay) fn mobile_relay_e2ee_secret_store_status(
    config: &Value,
    overrides: &RuntimeSecretOverrides,
) -> Value {
    let e2ee = config.get("mobileRelayE2ee").unwrap_or(&Value::Null);
    let portable_private_key_present =
        !overrides.e2ee_private_key && secret_present(e2ee.get("privateKeyBase64url"));
    let portable_signing_key_present =
        !overrides.e2ee_signing_key && secret_present(e2ee.get("signingKeyBase64url"));
    let portable_signed_prekey_private_key_present = !overrides.e2ee_signed_prekey_private_key
        && secret_present(e2ee.get("signedPrekeyPrivateKeyBase64url"));
    let portable_one_time_prekey_private_key_present = !overrides.e2ee_one_time_prekey_private_key
        && secret_present(e2ee.get("oneTimePrekeyPrivateKeyBase64url"));
    let portable_one_time_mlkem1024_prekey_seed_present = !overrides
        .e2ee_one_time_mlkem1024_prekey_seed
        && secret_present(e2ee.get("oneTimeMlKem1024PrekeySeedBase64url"));
    let portable_pairing_secret_present =
        !overrides.e2ee_pairing_secret && secret_present(e2ee.get("pairingSecretBase64url"));
    let any_portable_private_key_present = portable_private_key_present
        || portable_signing_key_present
        || portable_signed_prekey_private_key_present
        || portable_one_time_prekey_private_key_present
        || portable_one_time_mlkem1024_prekey_seed_present;
    let any_private_key_missing = (!overrides.e2ee_private_key
        && !secret_present(e2ee.get("privateKeyBase64url")))
        || (!overrides.e2ee_signing_key && !secret_present(e2ee.get("signingKeyBase64url")))
        || (!overrides.e2ee_signed_prekey_private_key
            && !secret_present(e2ee.get("signedPrekeyPrivateKeyBase64url")))
        || (!overrides.e2ee_one_time_prekey_private_key
            && !secret_present(e2ee.get("oneTimePrekeyPrivateKeyBase64url")))
        || (!overrides.e2ee_one_time_mlkem1024_prekey_seed
            && !secret_present(e2ee.get("oneTimeMlKem1024PrekeySeedBase64url")));
    let all_private_keys_in_selected_custody = overrides.e2ee_private_key
        && overrides.e2ee_signing_key
        && overrides.e2ee_signed_prekey_private_key
        && overrides.e2ee_one_time_prekey_private_key
        && overrides.e2ee_one_time_mlkem1024_prekey_seed;
    let any_portable_secret_present =
        any_portable_private_key_present || portable_pairing_secret_present;
    let selected_backend = if all_private_keys_in_selected_custody {
        secret_storage_backend_for_overrides(overrides)
    } else if any_portable_private_key_present {
        "unsafe_portable_config"
    } else {
        "selected_custody_unavailable"
    };
    let custody_reason = if any_portable_secret_present {
        "secret_material_in_portable_config"
    } else if any_portable_private_key_present {
        "secret_material_in_portable_config"
    } else if any_private_key_missing {
        "endpoint_private_key_material_missing"
    } else {
        "custody_operational"
    };
    let authorization = overrides.secret_store_authorization.as_ref();
    let shared_system_context_required = authorization
        .map(|proof| proof.shared_system_context_required)
        .unwrap_or(false);
    let shared_system_context_available = authorization
        .map(|proof| proof.shared_system_context_available)
        .unwrap_or(false);
    let system_authorization_attempt_count = authorization
        .map(|proof| proof.system_authorization_attempt_count)
        .unwrap_or(0);
    let system_authorization_completed = authorization
        .map(|proof| proof.system_authorization_completed)
        .unwrap_or(false);
    let app_password_prompt_used = authorization
        .map(|proof| proof.app_password_prompt_used)
        .unwrap_or(false);
    let app_credential_prompt_used = authorization
        .map(|proof| proof.app_credential_prompt_used)
        .unwrap_or(false);
    let single_system_authorization_context_verified = authorization
        .map(|proof| {
            proof.single_system_authorization_context_verified && !app_credential_prompt_used
        })
        .unwrap_or(
            shared_system_context_required
                && shared_system_context_available
                && system_authorization_attempt_count == 1
                && system_authorization_completed
                && !app_password_prompt_used
                && !app_credential_prompt_used,
        );
    let authorization_batch_within_prompt_budget = !shared_system_context_required
        || (system_authorization_attempt_count == 1
            && system_authorization_completed
            && !app_password_prompt_used
            && !app_credential_prompt_used);
    let authorization_backend = authorization
        .map(|proof| proof.backend)
        .unwrap_or(selected_backend);
    let authorization_batch_operation_count = authorization
        .map(|proof| proof.operation_count)
        .unwrap_or(0);
    let authorization_batch_consumed_operation_count = authorization
        .map(|proof| proof.consumed_operation_count)
        .unwrap_or(0);
    let authorization_batch_remaining_operation_count = authorization
        .map(|proof| proof.remaining_operation_count)
        .unwrap_or(0);
    let authorization_batch_within_budget = authorization
        .map(|proof| proof.authorization_batch_within_budget)
        .unwrap_or(true);
    let authorization_batch_allow_interaction = authorization
        .map(|proof| proof.allow_interaction)
        .unwrap_or(false);
    let capability_report = authorization
        .and_then(|proof| proof.capability_report.clone())
        .or_else(|| {
            selected_mobile_relay_capability_evaluation()
                .ok()
                .map(|evaluation| evaluation.report())
        });
    let user_presence_enabled = capability_report
        .as_ref()
        .is_some_and(|report| report.enabled.contains(&SecurityCapability::OsUserPresence));
    let authorization_claim_consistent = !user_presence_enabled
        || (single_system_authorization_context_verified
            && authorization_batch_within_prompt_budget
            && authorization_batch_within_budget);
    let custody_operational = all_private_keys_in_selected_custody
        && !any_portable_secret_present
        && capability_report
            .as_ref()
            .and_then(|report| report.custody.as_ref())
            .is_some();
    let capability_report_value = capability_report
        .and_then(|report| serde_json::to_value(report).ok())
        .unwrap_or(Value::Null);
    let credential_custody =
        credential_custody_status_projection(config, selected_backend, overrides);
    json!({
        "capabilityReport": capability_report_value,
        "custodyOperational": custody_operational,
        "custodyReason": custody_reason,
        "selectedBackend": selected_backend,
        "privateKeyInSelectedCustody": overrides.e2ee_private_key,
        "signingKeyInSelectedCustody": overrides.e2ee_signing_key,
        "signedPrekeyPrivateKeyInSelectedCustody": overrides.e2ee_signed_prekey_private_key,
        "oneTimePrekeyPrivateKeyInSelectedCustody": overrides.e2ee_one_time_prekey_private_key,
        "oneTimeMlKem1024PrekeySeedInSelectedCustody": overrides.e2ee_one_time_mlkem1024_prekey_seed,
        "allPrivateKeysInSelectedCustody": all_private_keys_in_selected_custody,
        "pairingSecretInSelectedCustody": overrides.e2ee_pairing_secret,
        "unsafePersistenceDetected": any_portable_secret_present,
        "portableConfigPrivateKeyPresent": portable_private_key_present,
        "portableConfigSigningKeyPresent": portable_signing_key_present,
        "portableConfigSignedPrekeyPrivateKeyPresent": portable_signed_prekey_private_key_present,
        "portableConfigOneTimePrekeyPrivateKeyPresent": portable_one_time_prekey_private_key_present,
        "portableConfigOneTimeMlKem1024PrekeySeedPresent": portable_one_time_mlkem1024_prekey_seed_present,
        "portableConfigPairingSecretPresent": portable_pairing_secret_present,
        "authorization": {
            "sharedSystemContextRequired": shared_system_context_required,
            "sharedSystemContextAvailable": shared_system_context_available,
            "singleSystemAuthorizationContextVerified": single_system_authorization_context_verified,
            "systemAuthorizationAttemptCount": system_authorization_attempt_count,
            "systemAuthorizationCompleted": system_authorization_completed,
            "withinPromptBudget": authorization_batch_within_prompt_budget,
            "operationCount": authorization_batch_operation_count,
            "consumedOperationCount": authorization_batch_consumed_operation_count,
            "remainingOperationCount": authorization_batch_remaining_operation_count,
            "withinOperationBudget": authorization_batch_within_budget,
            "allowInteraction": authorization_batch_allow_interaction,
            "backend": authorization_backend,
            "claimConsistent": authorization_claim_consistent,
            "appCredentialPromptUsed": app_credential_prompt_used,
            "appPasswordPromptUsed": app_password_prompt_used
        },
        "keyMaterial": "redacted",
        "credentialCustody": credential_custody,
    })
}

/// Redacted credential classification for the current data home. Provider key
/// metadata is read through its owner without an authorized platform
/// operation; custody that was not observed is never reported as available.
fn credential_custody_status_projection(
    config: &Value,
    selected_backend: &str,
    overrides: &RuntimeSecretOverrides,
) -> Value {
    let mut relay_token_material = std::collections::BTreeMap::new();
    if overrides.secret_store_authorization.is_some() {
        relay_token_material.insert("pcToken".to_string(), overrides.pc_token);
        relay_token_material.insert("mobileToken".to_string(), overrides.mobile_token);
        for device in config
            .get("pairedDevices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(key) = paired_device_token_secret_store_key(device) {
                let observed = overrides.paired_device_tokens.iter().any(|token| {
                    token.id == paired_device_id(device)
                        && token.pairing_id == paired_device_pairing_id(device)
                });
                relay_token_material.insert(key, observed);
            }
        }
    }
    let observations = CredentialCustodyObservations {
        selected_custody_backend: selected_backend.to_string(),
        identity_material_in_selected_custody: overrides.identity_custody_verified,
        provider_key_material_in_selected_custody: None,
        relay_token_material,
        opaque_platform_items: Vec::new(),
    };
    let data_root = licoup_foundation::platform::paths::portable_data_dir_read_only().ok();
    let Some(data_root) = data_root else {
        return json!({
            "available": false,
            "reason": "credential_data_root_unavailable",
            "secretValuesIncluded": false,
        });
    };
    match credential_custody_inventory(&data_root, config, &observations) {
        Ok(inventory) => json!({
            "available": true,
            "inventory": inventory,
        }),
        Err(_) => json!({
            "available": false,
            "reason": "credential_metadata_unavailable",
            "secretValuesIncluded": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_secret_presence_never_treats_redaction_markers_as_material() {
        assert!(secret_present(Some(&json!("fixture-material"))));
        assert!(!secret_present(Some(&json!("redacted"))));
        assert!(!secret_present(Some(&json!("***"))));
        assert!(!secret_present(None));
    }
}
