use super::*;

pub(in crate::domain::mobile_relay) fn e2ee_secret_store_self_test_in(
    _params: &Value,
) -> Result<Value> {
    let temp_dir = env::temp_dir().join(format!(
        "lico-mobile-relay-secret-store-self-test-{}",
        Uuid::new_v4()
    ));
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(temp_dir.clone()));
    let result = e2ee_secret_store_self_test_in_current_portable_dir();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    let _ = fs::remove_dir_all(&temp_dir);
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            // Classify locally for a stable redacted receipt; never return the
            // formatted error or any platform/runtime detail to the caller.
            let message = format!("{error:#}");
            let failure_category = if message.contains("secure_mesh_authorization_required") {
                "system_authorization_required"
            } else if message.contains("security status -34018") {
                "keychain_entitlement_missing"
            } else if message.contains("security status -25293") {
                "keychain_authentication_failed"
            } else if message.contains("security status -25308") {
                "keychain_interaction_not_allowed"
            } else if message.contains("access control unavailable") {
                "platform_access_control_unavailable"
            } else if message.contains("authorization callback") {
                "system_authorization_callback_unavailable"
            } else if message.contains("system authentication") {
                "system_authentication_unavailable"
            } else if message.contains("operation budget") {
                "authorization_operation_budget_exceeded"
            } else if message.contains("secret-store self-test cleanup") {
                "secret_store_cleanup_failed"
            } else {
                "platform_secret_store_unavailable"
            };
            let failure_operation = if message.contains(" secret store write failed ") {
                "write"
            } else if message.contains(" secret store read failed ") {
                "read"
            } else if message.contains(" secret store delete failed ") {
                "delete"
            } else if message.contains("self-test cleanup") {
                "cleanup"
            } else {
                "authorization-or-access-control"
            };
            let selected_store = selected_mobile_relay_secret_store();
            let capability_report = selected_store
                .capability_evaluation()
                .ok()
                .and_then(|evaluation| serde_json::to_value(evaluation.report()).ok())
                .unwrap_or(Value::Null);
            Ok(json!({
            "ok": false,
            "backend": selected_store.backend(),
            "unverifiedDesktopBackends": NATIVE_SECRET_STORE_UNVERIFIED_DESKTOP_BACKENDS,
            "selfTestPassed": false,
            "redacted": true,
            "rawPrivateMaterialIncluded": false,
            "rawPlaintextIncluded": false,
            "rawPublicWireBytesIncluded": false,
            "reportLeakScan": true,
            "capabilityReport": capability_report,
            "operationFailed": true,
            "failureCategory": failure_category,
            "failureOperation": failure_operation,
            "failureSummary": "selected secret custody operation failed; local details redacted"
            }))
        }
    }
}

fn e2ee_secret_store_self_test_in_current_portable_dir() -> Result<Value> {
    let mut config = default_config();
    let mut material = RuntimeSecretMaterial::new();
    let mut secret_store_batch = MobileRelaySecretStoreAuthBatch::new(
        "Mobile Relay E2EE secret store self-test authorization batch",
        mobile_relay_secret_store_self_test_authorization_batch_operation_count(),
    );
    ensure_mobile_relay_endpoint_descriptor(&mut config, &mut material, "desktop_sidecar")?;
    persist_config_secret_material_to_native_store_with_batch(
        &mut config,
        &mut secret_store_batch,
    )?;
    persist_runtime_secret_material_to_native_store_with_batch(
        &mut material,
        &mut secret_store_batch,
    )?;
    save_config_raw(&mut config)?;

    let persisted = fs::read_to_string(config_path()?).unwrap_or_default();
    let persisted_private_fields: Vec<&str> = MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS
        .iter()
        .filter_map(|(field, _)| {
            if persisted.contains(&format!("\"{}\"", field)) {
                Some(*field)
            } else {
                None
            }
        })
        .collect();
    let loaded = normalize_config(config.clone());
    material = RuntimeSecretMaterial::new();
    let mut overrides = RuntimeSecretOverrides::default();
    hydrate_runtime_secret_material_from_native_store_with_batch(
        &loaded,
        &mut material,
        &mut overrides,
        &mut secret_store_batch,
    )?;
    let local_rehydrated = local_endpoint_state(&loaded, &material).is_ok();
    let secret_store = mobile_relay_e2ee_secret_store_status(&loaded, &overrides);
    let (store, authorization_session, namespace) =
        secret_store_batch.authorization()?.ok_or_else(|| {
            anyhow!("mobile relay native secret-store self-test authorization batch is unavailable")
        })?;
    let shared_secret_class_round_trip = verify_secret_class_round_trip_with_session(
        store.as_ref(),
        &authorization_session,
        native_secret_store_shared_secret_classes_namespace()?,
        NATIVE_SECRET_STORE_SHARED_SECRET_CLASSES,
    )?;
    let shared_secret_class_round_trip_passed = shared_secret_class_round_trip
        .all_classes_persisted
        && shared_secret_class_round_trip.all_classes_deleted
        && !shared_secret_class_round_trip.raw_secret_material_included;
    let all_private_keys_in_selected_custody = secret_store
        .get("allPrivateKeysInSelectedCustody")
        .and_then(Value::as_bool)
        == Some(true);
    let authorization_claim_consistent = secret_store
        .get("authorization")
        .and_then(|authorization| authorization.get("claimConsistent"))
        .and_then(Value::as_bool)
        == Some(true);
    let pairing_secret_in_selected_custody = secret_store
        .get("pairingSecretInSelectedCustody")
        .and_then(Value::as_bool)
        == Some(true);
    let capability_report = authorization_session
        .capability_report()
        .cloned()
        .or_else(|| {
            store
                .capability_evaluation()
                .ok()
                .map(|evaluation| evaluation.report())
        })
        .ok_or_else(|| anyhow!("mobile relay capability report is unavailable"))?;
    let custody_strategy = capability_report
        .custody
        .as_ref()
        .map(|selection| selection.strategy)
        .ok_or_else(|| anyhow!("mobile relay safe custody strategy is unavailable"))?;
    let restart_semantics = capability_report
        .custody
        .as_ref()
        .map(|selection| selection.restart_semantics)
        .ok_or_else(|| anyhow!("mobile relay custody restart semantics are unavailable"))?;
    let persistent_custody = custody_strategy == SecretCustodyStrategy::OsSecureStore;
    let shared_secret_class_persistence_ready =
        persistent_custody && shared_secret_class_round_trip_passed;
    let restart_requires_re_pair_rekey =
        restart_semantics == CustodyRestartSemantics::RePairRekeyAfterRestart;
    let stale_session_restoration_rejected = if restart_requires_re_pair_rekey {
        let fresh_store = EphemeralSecretStore::new();
        let bundle_handle = native_e2ee_secret_bundle_handle_for_namespace(&namespace)?;
        fresh_store.get_secret(&bundle_handle)?.is_none()
    } else {
        true
    };
    let self_test_passed = local_rehydrated
        && all_private_keys_in_selected_custody
        && pairing_secret_in_selected_custody
        && authorization_claim_consistent
        && shared_secret_class_round_trip_passed
        && stale_session_restoration_rejected
        && persisted_private_fields.is_empty();
    cleanup_native_secret_store_fields_for_store_with_session(
        &config,
        store.as_ref(),
        &authorization_session,
        &namespace,
    )
    .context("mobile relay secret-store self-test cleanup failed")?;
    let capability_report_value = serde_json::to_value(&capability_report)?;
    let secret_service_probe = platform_linux_secret_service_probe_snapshot(
        persistent_custody && shared_secret_class_round_trip_passed,
        persisted_private_fields.is_empty(),
    );
    Ok(json!({
        "ok": self_test_passed,
        "backend": store.backend(),
        "selectedBackend": store.backend(),
        "unverifiedDesktopBackends": NATIVE_SECRET_STORE_UNVERIFIED_DESKTOP_BACKENDS,
        "selfTestPassed": self_test_passed,
        "redacted": true,
        "rawPrivateMaterialIncluded": false,
        "rawPlaintextIncluded": false,
        "rawPublicWireBytesIncluded": false,
        "reportLeakScan": true,
        "localEndpointRehydrated": local_rehydrated,
        "capabilityReport": capability_report_value,
        "secretServiceProbe": secret_service_probe,
        "secretStore": secret_store,
        "sharedSecretClassRoundTrip": {
            "backend": shared_secret_class_round_trip.backend,
            "secretClasses": shared_secret_class_round_trip.secret_classes,
            "requestedClassCount": shared_secret_class_round_trip.requested_class_count,
            "storedClassCount": shared_secret_class_round_trip.persisted_class_count,
            "deletedClassCount": shared_secret_class_round_trip.deleted_class_count,
            "allClassesStored": shared_secret_class_round_trip.all_classes_persisted,
            "allClassesDeleted": shared_secret_class_round_trip.all_classes_deleted,
            "rawSecretMaterialIncluded": shared_secret_class_round_trip.raw_secret_material_included
        },
        "secretStoreAuthorization": secret_store_authorization_report(&authorization_session),
        "sharedSecretClassRoundTripPassed": shared_secret_class_round_trip_passed,
        "sharedSecretClassPersistenceReady": shared_secret_class_persistence_ready,
        "ordinaryFileSecretArtifactCount": persisted_private_fields.len(),
        "restartProof": {
            "staleSessionRestorationRejected": stale_session_restoration_rejected,
            "rePairRekeyRequired": restart_requires_re_pair_rekey
        },
        "portableConfigPrivateFieldsPresent": persisted_private_fields,
        "portableConfigPrivateMaterialRedacted": persisted_private_fields.is_empty()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_test_reports_redacted_memory_only_restart_semantics() {
        let report = e2ee_secret_store_self_test_in(&json!({})).unwrap();

        assert_eq!(report["ok"], true);
        assert_eq!(report["selfTestPassed"], true);
        assert_eq!(report["selectedBackend"], "memory-only-ephemeral");
        assert!(report.get("supportedBackends").is_none());
        assert_eq!(
            report["unverifiedDesktopBackends"],
            json!([
                "macos-keychain",
                "linux-secret-service-keyring",
                "windows-credential-manager"
            ])
        );
        assert_eq!(report["ordinaryFileSecretArtifactCount"], 0);
        assert_eq!(report["portableConfigPrivateMaterialRedacted"], true);
    }
}
