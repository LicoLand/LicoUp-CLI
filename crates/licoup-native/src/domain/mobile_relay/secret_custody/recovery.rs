use super::*;
use zeroize::{Zeroize, Zeroizing};

struct RecoveryDocument(Value);

impl Drop for RecoveryDocument {
    fn drop(&mut self) {
        fn wipe(value: &mut Value) {
            match std::mem::take(value) {
                Value::String(mut value) => value.zeroize(),
                Value::Array(mut values) => values.iter_mut().for_each(wipe),
                Value::Object(values) => {
                    for (mut key, mut value) in values {
                        key.zeroize();
                        wipe(&mut value);
                    }
                }
                _ => {}
            }
        }
        wipe(&mut self.0);
    }
}

/// Prepare only portable metadata in an isolated recovery candidate. This never
/// opens platform custody, imports identity material or grants authorization.
pub(crate) fn prepare_recovered_custody_metadata(
    candidate_root: &Path,
    logical_source_home: &Path,
) -> Result<()> {
    ensure!(
        logical_source_home.is_absolute(),
        "recovery_custody_origin_invalid"
    );
    let path = config_path_for_data_home(candidate_root);
    let Some(raw) = licoup_foundation::platform::file_security::read_private_text_bounded(
        &path,
        CONFIG_MAX_BYTES,
    )?
    else {
        return Ok(());
    };
    let raw = Zeroizing::new(raw);
    let mut document = RecoveryDocument(
        serde_json::from_str(&raw).map_err(|_| anyhow!("recovery_custody_metadata_invalid"))?,
    );
    let config = &mut document.0;
    // Recognition is read-only; importing an archive does not advance the
    // application format or manufacture an admission/custody receipt.
    crate::domain::mobile_relay::config::validate_config_for_migration(config)
        .map_err(|_| anyhow!("recovery_custody_metadata_invalid"))?;
    ensure!(
        !config_contains_native_store_secret_material(&config),
        "recovery_source_identity_not_portable"
    );
    let mut changed = false;
    if let Some(e2ee) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        if let Some(value) = e2ee.get(CUSTODY_NAMESPACE_FIELD) {
            validate_custody_namespace(
                value
                    .as_str()
                    .ok_or_else(|| anyhow!("recovery_custody_locator_invalid"))?,
            )
            .map_err(|_| anyhow!("recovery_custody_locator_invalid"))?;
        } else if crate::domain::mobile_relay::endpoint_trust::local_identity_metadata_present(e2ee)
        {
            // Retain the original owner's locator. Actual local material and
            // native authorization are still required before it can be used.
            let origin = native_secret_store_namespace_for_config_path(&config_path_for_data_home(
                logical_source_home,
            ));
            e2ee.insert(CUSTODY_NAMESPACE_FIELD.to_string(), json!(origin));
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let bytes = Zeroizing::new(
        serde_json::to_string(&config).map_err(|_| anyhow!("recovery_custody_metadata_invalid"))?,
    );
    licoup_foundation::platform::file_security::atomic_write_private_text_bounded(
        &path,
        &bytes,
        CONFIG_MAX_BYTES,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_foundation::platform::file_security::atomic_write_private_text_bounded;

    #[test]
    fn both_shared_containers_preserve_local_identity_and_do_not_activate_replacement_custody() {
        let base = env::temp_dir().join(format!("licoup-custody-containers-{}", Uuid::new_v4()));
        let source = base.join("source");
        let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            source.clone(),
        ));
        let store = Arc::new(EphemeralSecretStore::new());
        let io: Arc<dyn SecureMeshSecretStore> = store.clone();
        with_native_namespace_store_test_port(io, || {
            let params = json!({"allowInteraction": true});
            let (mut original, mut context) = load_config_with_runtime_secret_context(&params)?;
            ensure_mobile_relay_endpoint_material(
                &mut original,
                &mut context.material,
                "desktop_sidecar",
            )?;
            save_config_with_runtime_secret_context(&mut original, &mut context)?;
            let identity =
                crate::domain::mobile_relay::endpoint_trust::local_public_device_identity(
                    &original,
                )?;
            let source_bytes = fs::read(config_path()?)?;
            for extension in ["zip", "tar.gz"] {
                let archive = base.join(format!("snapshot.{extension}"));
                crate::domain::local_recovery::export_data_home(Some(&source), &archive, true)?;
                let target = base.join(format!("same-device-{extension}"));
                let before = store.authorization_session_count();
                crate::domain::local_recovery::import_archive(&archive, &target)?;
                assert_eq!(
                    store.authorization_session_count(),
                    before,
                    "archive metadata cannot authorize custody"
                );
                licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
                    target.clone(),
                ));
                let (mut restored, mut restored_context) =
                    load_config_with_runtime_secret_context(&params)?;
                ensure_mobile_relay_endpoint_material(
                    &mut restored,
                    &mut restored_context.material,
                    "desktop_sidecar",
                )?;
                save_config_with_runtime_secret_context(&mut restored, &mut restored_context)?;
                assert_eq!(
                    crate::domain::mobile_relay::endpoint_trust::local_public_device_identity(
                        &restored
                    )?,
                    identity
                );

                let replacement = base.join(format!("replacement-{extension}"));
                crate::domain::local_recovery::import_archive(&archive, &replacement)?;
                licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
                    replacement.clone(),
                ));
                let fresh = Arc::new(EphemeralSecretStore::new());
                let fresh_io: Arc<dyn SecureMeshSecretStore> = fresh.clone();
                with_native_namespace_store_test_port(fresh_io, || {
                    let (mut absent, mut absent_context) =
                        load_config_with_runtime_secret_context(&params)?;
                    let before = absent.clone();
                    assert!(
                        ensure_mobile_relay_endpoint_material(
                            &mut absent,
                            &mut absent_context.material,
                            "desktop_sidecar"
                        )
                        .is_err()
                    );
                    assert_eq!(absent, before);
                    assert!(absent_context.material.is_empty());
                    Ok(())
                })?;

                let active = base.join(format!("active-target-{extension}"));
                licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
                    active.clone(),
                ));
                let (mut active_config, mut active_context) =
                    load_config_with_runtime_secret_context(&params)?;
                ensure_mobile_relay_endpoint_material(
                    &mut active_config,
                    &mut active_context.material,
                    "desktop_sidecar",
                )?;
                save_config_with_runtime_secret_context(&mut active_config, &mut active_context)?;
                let active_bytes = fs::read(config_path()?)?;
                assert!(crate::domain::local_recovery::import_archive(&archive, &active).is_err());
                assert_eq!(fs::read(config_path()?)?, active_bytes);
                licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
                    source.clone(),
                ));
                assert_eq!(fs::read(config_path()?)?, source_bytes);
            }
            Ok(())
        })
        .unwrap();
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn recovered_metadata_retains_origin_without_authorizing_or_importing_identity() {
        let base = env::temp_dir().join(format!("licoup-custody-recovery-{}", Uuid::new_v4()));
        let source = base.join("source");
        let candidate = base.join("candidate");
        let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            source.clone(),
        ));
        let mut config = default_config();
        let mut material = RuntimeSecretMaterial::new();
        ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar")
            .unwrap();
        config["mobileRelayE2ee"]
            .as_object_mut()
            .unwrap()
            .remove(CUSTODY_NAMESPACE_FIELD);
        // A supported pre-current schema is recognized, not silently converted.
        config["schemaVersion"] = json!(1);
        let raw = serde_json::to_string(&config).unwrap();
        let path = config_path_for_data_home(&candidate);
        atomic_write_private_text_bounded(&path, &raw, CONFIG_MAX_BYTES).unwrap();
        let store = Arc::new(EphemeralSecretStore::new());
        let io_port: Arc<dyn SecureMeshSecretStore> = store.clone();
        with_native_namespace_store_test_port(io_port, || {
            prepare_recovered_custody_metadata(&candidate, &source)
        })
        .unwrap();
        assert_eq!(store.authorization_session_count(), 0);
        let prepared: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(prepared["schemaVersion"], 1);
        assert_eq!(
            prepared["mobileRelayE2ee"][CUSTODY_NAMESPACE_FIELD],
            native_secret_store_namespace_for_config_path(&config_path_for_data_home(&source))
        );
        assert_eq!(
            prepared["mobileRelayE2ee"]["fingerprint"],
            config["mobileRelayE2ee"]["fingerprint"]
        );

        config["mobileRelayE2ee"]["privateKeyBase64url"] =
            json!("synthetic-nonportable-identity-value");
        let untrusted = serde_json::to_string(&config).unwrap();
        atomic_write_private_text_bounded(&path, &untrusted, CONFIG_MAX_BYTES).unwrap();
        assert_eq!(
            prepare_recovered_custody_metadata(&candidate, &source)
                .unwrap_err()
                .to_string(),
            "recovery_source_identity_not_portable"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), untrusted);
        assert_eq!(store.authorization_session_count(), 0);
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        fs::remove_dir_all(base).unwrap();
    }
}
