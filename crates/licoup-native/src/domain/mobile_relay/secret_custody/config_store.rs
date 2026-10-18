use super::*;

/// Resolve MLS peer trust from the locally persisted Mobile Relay authority.
///
/// The caller supplies an identity to bind the protocol message, but cannot supply or promote
/// its trust state. Until the directory/KT authority is wired, only the single peer represented by
/// the current locally signed and persisted pairing trust record is eligible for MLS operations.
pub(in crate::domain::mobile_relay) fn load_config() -> Result<Value> {
    let parsed = read_persisted_config()?;
    let mut config = normalize_config(parsed.clone().unwrap_or_else(|| json!({})));
    validate_config_generations(&config)?;
    if parsed.as_ref() != Some(&config) || config_contains_native_store_secret_material(&config) {
        save_config(&mut config)?;
        config = normalize_config(read_persisted_config()?.ok_or_else(|| {
            anyhow!("mobile relay config disappeared after durable initialization")
        })?);
        validate_config_generations(&config)?;
    }
    Ok(config)
}

pub(in crate::domain::mobile_relay) fn load_config_without_persistence() -> Result<Value> {
    let config = normalize_config(read_persisted_config()?.unwrap_or_else(|| json!({})));
    validate_config_generations(&config)?;
    Ok(config)
}

pub(in crate::domain::mobile_relay) fn read_persisted_config() -> Result<Option<Value>> {
    let Some(raw) = licoup_foundation::platform::file_security::read_private_text_bounded(
        &config_path()?,
        CONFIG_MAX_BYTES,
    )?
    else {
        return Ok(None);
    };
    ensure!(
        !raw.trim().is_empty(),
        "mobile relay config exists but is empty"
    );
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|_| anyhow!("mobile relay config exists but is invalid"))?;
    ensure!(
        parsed.is_object(),
        "mobile relay config exists but is not an object"
    );
    crate::domain::mobile_relay::validate_current_config_document(&parsed)?;
    validate_config_generations(&parsed)?;
    Ok(Some(parsed))
}

pub(in crate::domain::mobile_relay) fn validate_config_generations(config: &Value) -> Result<()> {
    for field in [CONFIG_GENERATION_FIELD, AUTHORITY_GENERATION_FIELD] {
        let value = config.get(field).and_then(Value::as_u64).unwrap_or(0);
        ensure!(
            value <= KT_JSON_SAFE_INTEGER_MAX,
            "mobile relay config security generation is invalid"
        );
        if config.get(field).is_some() {
            ensure!(
                config.get(field).and_then(Value::as_u64).is_some(),
                "mobile relay config security generation is invalid"
            );
        }
    }
    Ok(())
}

pub(in crate::domain::mobile_relay) fn config_generation(
    config: &Value,
    field: &str,
) -> Result<u64> {
    validate_config_generations(config)?;
    Ok(config.get(field).and_then(Value::as_u64).unwrap_or(0))
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn load_config_with_runtime_secret_overrides(
    params: &Value,
) -> Result<(Value, RuntimeSecretOverrides)> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let mut config = load_config()?;
    let mut overrides = RuntimeSecretOverrides::default();
    let mut material = RuntimeSecretMaterial::new();
    hydrate_runtime_secret_material_from_native_store(&config, &mut material, &mut overrides)?;
    overrides.merge(apply_runtime_secret_overrides(&mut config, params)?);
    apply_selected_paired_device_credentials(&mut config);
    Ok((config, overrides))
}

pub(in crate::domain::mobile_relay) fn load_config_for_read(
    params: &Value,
) -> Result<(Value, RuntimeSecretOverrides)> {
    let authorize_secret_read = should_authorize_secret_read(params);
    let mut config = if authorize_secret_read {
        load_config()?
    } else {
        load_config_without_persistence()?
    };
    let mut overrides = RuntimeSecretOverrides::default();
    if authorize_secret_read {
        let mut material = RuntimeSecretMaterial::new();
        hydrate_runtime_secret_material_from_native_store(&config, &mut material, &mut overrides)?;
    }
    overrides.merge(apply_runtime_secret_overrides(&mut config, params)?);
    if authorize_secret_read {
        apply_selected_paired_device_credentials(&mut config);
    }
    Ok((config, overrides))
}

pub(in crate::domain::mobile_relay) fn should_authorize_secret_read(params: &Value) -> bool {
    bool_param(params, &["authorize"]).unwrap_or(false)
        && bool_param(params, &["hydrateSecrets"]).unwrap_or(true)
}

pub(in crate::domain::mobile_relay) fn load_config_with_runtime_secret_context(
    params: &Value,
) -> Result<(Value, RuntimeSecretContext)> {
    load_config_with_runtime_secret_context_for_operation(
        params,
        "Mobile Relay E2EE secret store authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count(),
    )
}

pub(in crate::domain::mobile_relay) fn load_config_with_runtime_secret_context_for_operation(
    params: &Value,
    reason: impl Into<String>,
    operation_count: usize,
) -> Result<(Value, RuntimeSecretContext)> {
    ensure_secure_mesh_protected_operation_allowed()?;
    load_config_with_runtime_secret_context_unchecked(params, reason, operation_count)
}

pub(in crate::domain::mobile_relay) fn load_config_with_runtime_secret_context_for_authority_reset(
    params: &Value,
) -> Result<(Value, RuntimeSecretContext)> {
    load_config_with_runtime_secret_context_unchecked(
        params,
        "Mobile Relay KT authority reset authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count(),
    )
}

fn load_config_with_runtime_secret_context_unchecked(
    params: &Value,
    reason: impl Into<String>,
    operation_count: usize,
) -> Result<(Value, RuntimeSecretContext)> {
    // Explicitly non-interactive callers must never trigger a system
    // authorization prompt: they get the in-memory normalized config without
    // the migration write, and the batch fails closed instead of prompting.
    // Flag-less callers keep the interactive default (user-initiated flows).
    let allow_interaction =
        bool_param(params, &["allowInteraction", "allow-interaction"]).unwrap_or(true);
    let mut config = if allow_interaction {
        load_config()?
    } else {
        load_config_without_persistence()?
    };
    let mut context = RuntimeSecretContext {
        material: RuntimeSecretMaterial::new(),
        overrides: RuntimeSecretOverrides::default(),
        secret_store_batch: MobileRelaySecretStoreAuthBatch::with_interaction(
            reason,
            operation_count,
            allow_interaction,
        ),
    };
    hydrate_runtime_secret_material_from_native_store_with_batch(
        &config,
        &mut context.material,
        &mut context.overrides,
        &mut context.secret_store_batch,
    )?;
    context
        .overrides
        .merge(apply_runtime_secret_overrides(&mut config, params)?);
    apply_selected_paired_device_credentials(&mut config);
    Ok((config, context))
}

pub(in crate::domain::mobile_relay) fn apply_runtime_secret_overrides(
    _config: &mut Value,
    params: &Value,
) -> Result<RuntimeSecretOverrides> {
    let applied = RuntimeSecretOverrides::default();
    if params
        .get("secretOverrideTransport")
        .and_then(Value::as_str)
        .map(str::trim)
        != Some(RUNTIME_SECRET_OVERRIDE_TRANSPORT)
    {
        return Ok(applied);
    }
    let Some(overrides) = params
        .get("secretOverrides")
        .filter(|value| value.is_object())
    else {
        return Ok(applied);
    };
    ensure!(
        !contains_unredacted_token_secret_override(overrides),
        "mobile relay raw token secretOverrides are disabled; use the platform secret-store callback"
    );
    if let Some(e2ee_overrides) = overrides
        .get("mobileRelayE2ee")
        .filter(|value| value.is_object())
    {
        ensure!(
            !contains_unredacted_e2ee_secret_override(e2ee_overrides),
            "mobile relay raw E2EE secretOverrides are disabled; use the platform secret-store callback"
        );
    }
    Ok(applied)
}

pub(in crate::domain::mobile_relay) fn contains_unredacted_token_secret_override(
    value: &Value,
) -> bool {
    MOBILE_RELAY_NATIVE_TOKEN_SECRET_FIELDS.iter().any(|field| {
        value
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(is_unredacted_secret)
    }) || value
        .get("pairedDevices")
        .and_then(Value::as_array)
        .is_some_and(|devices| {
            devices.iter().any(|device| {
                device
                    .get("mobileToken")
                    .and_then(Value::as_str)
                    .is_some_and(is_unredacted_secret)
            })
        })
}

pub(in crate::domain::mobile_relay) fn contains_unredacted_e2ee_secret_override(
    value: &Value,
) -> bool {
    MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS
        .iter()
        .any(|(field, _)| {
            value
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(is_unredacted_secret)
        })
}

pub(in crate::domain::mobile_relay) fn save_config(config: &mut Value) -> Result<()> {
    let overrides = RuntimeSecretOverrides::default();
    save_config_with_runtime_secret_overrides(config, &overrides)
}

pub(in crate::domain::mobile_relay) fn save_config_raw(config: &mut Value) -> Result<()> {
    save_config_raw_with_reset_policy(config, false)
}

static CONFIG_WRITE_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

pub(in crate::domain::mobile_relay) fn save_config_raw_with_reset_policy(
    config: &mut Value,
    allow_reset_write: bool,
) -> Result<()> {
    // Reject malformed custody provenance before normalization, generations or I/O.
    recorded_custody_namespace(config)?;
    prepare_station_fields_for_persistence(config)?;
    validate_config_generations(config)?;
    let expected_generation = config_generation(config, CONFIG_GENERATION_FIELD)?;
    let candidate_authority_generation = config_generation(config, AUTHORITY_GENERATION_FIELD)?;
    let _process_guard = CONFIG_WRITE_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow!("mobile relay config writer lock is unavailable"))?;
    let lock_path = config_lock_path()?;
    let lock_file = licoup_foundation::platform::file_security::open_private_lock_file(&lock_path)?;
    fs2::FileExt::lock_exclusive(&lock_file)
        .map_err(|_| anyhow!("mobile relay config writer lock could not be acquired"))?;
    let durable = read_persisted_config()?;
    let durable_generation = durable
        .as_ref()
        .map(|value| config_generation(value, CONFIG_GENERATION_FIELD))
        .transpose()?
        .unwrap_or(0);
    let durable_authority_generation = durable
        .as_ref()
        .map(|value| config_generation(value, AUTHORITY_GENERATION_FIELD))
        .transpose()?
        .unwrap_or(0);
    ensure!(
        expected_generation == durable_generation,
        "mobile relay config snapshot is stale"
    );
    if allow_reset_write {
        ensure!(
            candidate_authority_generation == durable_authority_generation
                || candidate_authority_generation == durable_authority_generation.saturating_add(1),
            "mobile relay authority generation transition is invalid"
        );
    } else {
        ensure!(
            candidate_authority_generation == durable_authority_generation,
            "mobile relay config authority generation is stale"
        );
        ensure!(
            !kt_authority_reset_in_progress()?,
            "mobile relay config write is blocked during KT authority reset"
        );
    }
    let committed_generation = expected_generation
        .checked_add(1)
        .filter(|generation| *generation <= KT_JSON_SAFE_INTEGER_MAX)
        .ok_or_else(|| anyhow!("mobile relay config generation overflow"))?;
    config[CONFIG_GENERATION_FIELD] = json!(committed_generation);
    config[AUTHORITY_GENERATION_FIELD] = json!(candidate_authority_generation);
    // Preserve the owning locator. Normalization is not custody verification
    // and must not erase an origin reference after a data-home restore.
    if let Some(e2ee) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        if !e2ee.contains_key(CUSTODY_NAMESPACE_FIELD) {
            e2ee.insert(
                CUSTODY_NAMESPACE_FIELD.to_string(),
                json!(current_custody_namespace()?),
            );
        }
    }
    let encoded = format!("{}\n", serde_json::to_string_pretty(config)?);
    licoup_foundation::platform::file_security::atomic_write_private_text_bounded(
        &config_path()?,
        &encoded,
        CONFIG_MAX_BYTES,
    )?;
    let committed = read_persisted_config()?
        .ok_or_else(|| anyhow!("mobile relay config disappeared after commit"))?;
    ensure!(
        config_generation(&committed, CONFIG_GENERATION_FIELD)? == committed_generation
            && config_generation(&committed, AUTHORITY_GENERATION_FIELD)?
                == candidate_authority_generation,
        "mobile relay config durable generation verification failed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_locator_is_rejected_before_config_normalization_or_io() {
        use licoup_foundation::platform::paths::set_portable_data_dir_override;
        struct RestoreHome {
            previous: Option<PathBuf>,
            root: PathBuf,
        }
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                set_portable_data_dir_override(self.previous.take());
                let _ = fs::remove_dir_all(&self.root);
            }
        }
        let root = env::temp_dir().join(format!("licoup-invalid-locator-{}", Uuid::new_v4()));
        let _home = RestoreHome {
            previous: set_portable_data_dir_override(Some(root.clone())),
            root: root.clone(),
        };
        let mut config = json!({
            "configGeneration": 7,
            "mobileRelayE2ee": {"custodyNamespace": "invalid-locator"}
        });
        let before = config.clone();
        let error = save_config_raw(&mut config).unwrap_err();
        assert_eq!(error.to_string(), "mobile relay custody locator is invalid");
        assert_eq!(config, before);
        assert!(
            !root.exists(),
            "invalid provenance must not create the data home"
        );
    }

    #[test]
    fn raw_runtime_secret_override_detection_is_fail_closed() {
        assert!(contains_unredacted_token_secret_override(
            &json!({"pcToken": "fixture-material"})
        ));
        assert!(contains_unredacted_e2ee_secret_override(
            &json!({"privateKeyBase64url": "fixture-material"})
        ));
        assert!(!contains_unredacted_token_secret_override(
            &json!({"pcToken": "redacted"})
        ));
    }
}
