use super::*;

pub(in crate::domain::mobile_relay) const CONFIG_SCHEMA_VERSION: u32 = 2;
pub(in crate::domain::mobile_relay) const CONFIG_MAX_BYTES: usize = 512 * 1024;
pub(in crate::domain::mobile_relay) const CONFIG_GENERATION_FIELD: &str = "configGeneration";
pub(in crate::domain::mobile_relay) const AUTHORITY_GENERATION_FIELD: &str =
    "securityAuthorityGeneration";
/// Local custody locator retained across home changes. This value alone never
/// grants authorization or proves material exists on the current device.
pub(in crate::domain::mobile_relay) const CUSTODY_NAMESPACE_FIELD: &str = "custodyNamespace";

pub(in crate::domain::mobile_relay) fn validate_custody_namespace(namespace: &str) -> Result<()> {
    ensure!(
        namespace == MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE
            || (namespace.len() == 64
                && namespace
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
        "mobile relay custody locator is invalid"
    );
    Ok(())
}

pub(in crate::domain::mobile_relay) fn recorded_custody_namespace(
    config: &Value,
) -> Result<Option<&str>> {
    let Some(value) = config
        .get("mobileRelayE2ee")
        .and_then(|value| value.get(CUSTODY_NAMESPACE_FIELD))
    else {
        return Ok(None);
    };
    let namespace = value
        .as_str()
        .ok_or_else(|| anyhow!("mobile relay custody locator is invalid"))?;
    validate_custody_namespace(namespace)?;
    Ok(Some(namespace))
}

pub(in crate::domain::mobile_relay) fn current_custody_namespace() -> Result<String> {
    if mobile_relay_secret_store_override().is_some() {
        Ok(MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE.to_string())
    } else {
        native_secret_store_namespace()
    }
}

pub(in crate::domain::mobile_relay) fn custody_locator_matches_current_home(
    namespace: &str,
) -> Result<bool> {
    validate_custody_namespace(namespace)?;
    Ok(namespace == current_custody_namespace()? || namespace == native_secret_store_namespace()?)
}
pub(in crate::domain::mobile_relay) const RUNTIME_SECRET_OVERRIDE_TRANSPORT: &str =
    "platform_keyring_to_rust_ffi_memory_override";
pub(in crate::domain::mobile_relay) const NATIVE_SECRET_STORE_MODE_ENV: &str =
    "LICO_MOBILE_RELAY_NATIVE_SECRET_STORE";
pub(in crate::domain::mobile_relay) const NATIVE_SECRET_STORE_SERVICE: &str =
    "app.licomesh.licoup.mobile-relay.pqxdh-mlkem1024.v1";
pub(in crate::domain::mobile_relay) const NATIVE_SECRET_STORE_ACCOUNT_PREFIX: &str =
    "mobileRelayE2ee";
pub(in crate::domain::mobile_relay) const MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE: &str =
    "mobileRelayPqxdhMlKem1024Runtime";
pub(in crate::domain::mobile_relay) const NATIVE_SECRET_STORE_UNVERIFIED_DESKTOP_BACKENDS:
    &[&str] = &[
    "macos-keychain",
    "linux-secret-service-keyring",
    "windows-credential-manager",
];
pub(in crate::domain::mobile_relay) const NATIVE_SECRET_STORE_SHARED_SECRET_CLASSES: &[&str] = &[
    "pairwiseSessionSnapshot",
    "mlsEpochSecret",
    "recoverySecret",
];
pub(in crate::domain::mobile_relay) const KT_AUTHORITY_RESET_GUARD_SCHEMA_VERSION: u64 = 1;
pub(in crate::domain::mobile_relay) const KT_AUTHORITY_RESET_GUARD_STATE: &str =
    "security-blocked-reset-in-progress";

thread_local! {
    static PAIRWISE_SECRET_STORE_OVERRIDE: RefCell<Option<Arc<dyn SecureMeshSecretStore>>> =
        RefCell::new(None);
    static MOBILE_RELAY_SECRET_STORE_OVERRIDE: RefCell<Option<Arc<dyn SecureMeshSecretStore>>> =
        RefCell::new(None);
    #[cfg(test)]
    static NATIVE_NAMESPACE_STORE_TEST_PORT: RefCell<Option<Arc<dyn SecureMeshSecretStore>>> =
        RefCell::new(None);
    #[cfg(test)]
    pub(in crate::domain::mobile_relay) static KT_AUTHORITY_RESET_FAILPOINT: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

static MOBILE_RELAY_EPHEMERAL_SECRET_STORE: OnceLock<Arc<EphemeralSecretStore>> = OnceLock::new();

#[derive(Default)]
pub(in crate::domain::mobile_relay) struct RuntimeSecretOverrides {
    pub(in crate::domain::mobile_relay) pc_token: bool,
    pub(in crate::domain::mobile_relay) mobile_token: bool,
    pub(in crate::domain::mobile_relay) e2ee_private_key: bool,
    pub(in crate::domain::mobile_relay) e2ee_pairing_secret: bool,
    pub(in crate::domain::mobile_relay) e2ee_signing_key: bool,
    pub(in crate::domain::mobile_relay) identity_custody_verified: Option<bool>,
    pub(in crate::domain::mobile_relay) e2ee_signed_prekey_private_key: bool,
    pub(in crate::domain::mobile_relay) e2ee_one_time_prekey_private_key: bool,
    pub(in crate::domain::mobile_relay) e2ee_one_time_mlkem1024_prekey_seed: bool,
    pub(in crate::domain::mobile_relay) secret_storage_backend: Option<&'static str>,
    pub(in crate::domain::mobile_relay) secret_store_authorization:
        Option<RuntimeSecretStoreAuthorizationProof>,
    pub(in crate::domain::mobile_relay) paired_device_tokens: Vec<PairedDeviceSecretOverride>,
}

#[derive(Clone, Debug)]
pub(in crate::domain::mobile_relay) struct RuntimeSecretStoreAuthorizationProof {
    pub(in crate::domain::mobile_relay) backend: &'static str,
    pub(in crate::domain::mobile_relay) operation_count: usize,
    pub(in crate::domain::mobile_relay) consumed_operation_count: usize,
    pub(in crate::domain::mobile_relay) remaining_operation_count: usize,
    pub(in crate::domain::mobile_relay) authorization_batch_within_budget: bool,
    pub(in crate::domain::mobile_relay) allow_interaction: bool,
    pub(in crate::domain::mobile_relay) shared_system_context_required: bool,
    pub(in crate::domain::mobile_relay) shared_system_context_available: bool,
    pub(in crate::domain::mobile_relay) system_authorization_attempt_count: usize,
    pub(in crate::domain::mobile_relay) system_authorization_completed: bool,
    pub(in crate::domain::mobile_relay) single_system_authorization_context_verified: bool,
    pub(in crate::domain::mobile_relay) app_password_prompt_used: bool,
    pub(in crate::domain::mobile_relay) app_credential_prompt_used: bool,
    pub(in crate::domain::mobile_relay) capability_report: Option<CapabilityEvaluationReport>,
}

pub(in crate::domain::mobile_relay) struct PairedDeviceSecretOverride {
    pub(in crate::domain::mobile_relay) id: String,
    pub(in crate::domain::mobile_relay) pairing_id: String,
}

impl RuntimeSecretOverrides {
    pub(in crate::domain::mobile_relay) fn merge(&mut self, other: RuntimeSecretOverrides) {
        self.pc_token |= other.pc_token;
        self.mobile_token |= other.mobile_token;
        self.e2ee_private_key |= other.e2ee_private_key;
        self.e2ee_pairing_secret |= other.e2ee_pairing_secret;
        self.e2ee_signing_key |= other.e2ee_signing_key;
        if other.identity_custody_verified.is_some() {
            self.identity_custody_verified = other.identity_custody_verified;
        }
        self.e2ee_signed_prekey_private_key |= other.e2ee_signed_prekey_private_key;
        self.e2ee_one_time_prekey_private_key |= other.e2ee_one_time_prekey_private_key;
        self.e2ee_one_time_mlkem1024_prekey_seed |= other.e2ee_one_time_mlkem1024_prekey_seed;
        if other.secret_storage_backend.is_some() {
            self.secret_storage_backend = other.secret_storage_backend;
        }
        if other.secret_store_authorization.is_some() {
            self.secret_store_authorization = other.secret_store_authorization;
        }
        self.paired_device_tokens.extend(other.paired_device_tokens);
    }

    pub(in crate::domain::mobile_relay) fn mark_e2ee_secret_store(
        &mut self,
        backend: &'static str,
    ) {
        self.secret_storage_backend = Some(backend);
    }

    pub(in crate::domain::mobile_relay) fn mark_secret_store_authorization(
        &mut self,
        session: &SecretStoreAuthorizationSession,
    ) {
        self.secret_store_authorization = Some(RuntimeSecretStoreAuthorizationProof {
            backend: session.backend(),
            operation_count: session.operation_count(),
            consumed_operation_count: session.consumed_operation_count(),
            remaining_operation_count: session.remaining_operation_count(),
            authorization_batch_within_budget: session.authorization_batch_within_budget(),
            allow_interaction: session.allow_interaction(),
            shared_system_context_required: session.shared_system_context_required(),
            shared_system_context_available: session.shared_system_context_available(),
            system_authorization_attempt_count: session.system_authorization_attempt_count(),
            system_authorization_completed: session.system_authorization_completed(),
            single_system_authorization_context_verified: session
                .single_system_authorization_context_verified(),
            app_password_prompt_used: session.app_password_prompt_used(),
            app_credential_prompt_used: false,
            capability_report: session.capability_report().cloned(),
        });
    }
}

pub(in crate::domain::mobile_relay) struct RuntimeSecretContext {
    pub(in crate::domain::mobile_relay) material: RuntimeSecretMaterial,
    pub(in crate::domain::mobile_relay) overrides: RuntimeSecretOverrides,
    pub(in crate::domain::mobile_relay) secret_store_batch: MobileRelaySecretStoreAuthBatch,
}

impl Default for RuntimeSecretContext {
    fn default() -> Self {
        Self {
            material: RuntimeSecretMaterial::new(),
            overrides: RuntimeSecretOverrides::default(),
            secret_store_batch: MobileRelaySecretStoreAuthBatch::default(),
        }
    }
}

impl RuntimeSecretContext {
    pub(in crate::domain::mobile_relay) fn shared_authorization_session(
        &mut self,
    ) -> Result<Option<SecretStoreAuthorizationSession>> {
        Ok(self
            .secret_store_batch
            .authorization()?
            .map(|(_, session, _)| session))
    }
}

pub(in crate::domain::mobile_relay) struct MobileRelaySecretStoreAuthBatch {
    reason: String,
    operation_count: usize,
    allow_interaction: bool,
    initialized: bool,
    store: Option<Arc<dyn SecureMeshSecretStore>>,
    namespace: Option<String>,
    session: Option<SecretStoreAuthorizationSession>,
    custody_selected: bool,
}

impl Default for MobileRelaySecretStoreAuthBatch {
    fn default() -> Self {
        Self::new(
            "Mobile Relay E2EE secret store authorization batch",
            mobile_relay_e2ee_secret_store_authorization_batch_operation_count(),
        )
    }
}

impl MobileRelaySecretStoreAuthBatch {
    pub(in crate::domain::mobile_relay) fn prepare_custody_lookup(
        &mut self,
        config: &Value,
    ) -> Result<()> {
        self.init()?;
        let cross_home = match recorded_custody_namespace(config)? {
            Some(recorded) => !custody_locator_matches_current_home(recorded)?,
            None => false,
        };
        if self.session.is_none() && cross_home {
            self.operation_count = self
                .operation_count
                .checked_mul(2)
                .ok_or_else(|| anyhow!("mobile relay custody authorization budget overflow"))?;
        }
        Ok(())
    }

    pub(in crate::domain::mobile_relay) fn select_verified_custody_namespace(
        &mut self,
        namespace: String,
    ) -> Result<()> {
        validate_custody_namespace(&namespace)?;
        ensure!(
            self.session.is_some(),
            "mobile relay custody selection requires actual native authorization"
        );
        self.namespace = Some(namespace);
        self.custody_selected = true;
        Ok(())
    }

    pub(in crate::domain::mobile_relay) fn verified_namespace(&self) -> Option<&str> {
        self.custody_selected
            .then_some(self.namespace.as_deref())
            .flatten()
    }
    pub(in crate::domain::mobile_relay) fn new(
        reason: impl Into<String>,
        operation_count: usize,
    ) -> Self {
        Self::with_interaction(reason, operation_count, true)
    }

    pub(in crate::domain::mobile_relay) fn with_interaction(
        reason: impl Into<String>,
        operation_count: usize,
        allow_interaction: bool,
    ) -> Self {
        Self {
            reason: reason.into(),
            operation_count,
            allow_interaction,
            initialized: false,
            store: None,
            namespace: None,
            session: None,
            custody_selected: false,
        }
    }

    fn init(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        self.initialized = true;
        if let Some(store) = mobile_relay_secret_store_override() {
            self.store = Some(store);
            self.namespace = Some(MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE.to_string());
            return Ok(());
        }
        self.store = Some(selected_mobile_relay_secret_store());
        self.namespace = Some(native_secret_store_namespace()?);
        Ok(())
    }

    pub(in crate::domain::mobile_relay) fn authorization(
        &mut self,
    ) -> Result<
        Option<(
            Arc<dyn SecureMeshSecretStore>,
            SecretStoreAuthorizationSession,
            String,
        )>,
    > {
        self.init()?;
        let Some(store) = self.store.as_ref().map(Arc::clone) else {
            return Ok(None);
        };
        ensure!(
            store.supported(),
            "mobile relay native secret store backend is unsupported"
        );
        if self.session.is_none() {
            let request = if self.allow_interaction {
                SecretStoreAuthorizationRequest::new(&self.reason, self.operation_count)
            } else {
                SecretStoreAuthorizationRequest::noninteractive(&self.reason, self.operation_count)
            };
            self.session = Some(store.begin_authorized_session(&request)?);
        }
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow!("mobile relay secret store authorization batch is missing"))?
            .clone();
        let namespace = self
            .namespace
            .as_ref()
            .ok_or_else(|| anyhow!("mobile relay native secret store namespace is missing"))?
            .clone();
        Ok(Some((store, session, namespace)))
    }
}

pub(in crate::domain::mobile_relay) fn with_pairwise_secret_store_override_in<T>(
    store: Arc<dyn SecureMeshSecretStore>,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let previous = PAIRWISE_SECRET_STORE_OVERRIDE.with(|slot| slot.replace(Some(store)));
    let result = f();
    PAIRWISE_SECRET_STORE_OVERRIDE.with(|slot| {
        slot.replace(previous);
    });
    result
}

pub(in crate::domain::mobile_relay) fn with_mobile_relay_secret_store_override_in<T>(
    store: Arc<dyn SecureMeshSecretStore>,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let previous = MOBILE_RELAY_SECRET_STORE_OVERRIDE.with(|slot| slot.replace(Some(store)));
    let result = f();
    MOBILE_RELAY_SECRET_STORE_OVERRIDE.with(|slot| {
        slot.replace(previous);
    });
    result
}

pub(in crate::domain::mobile_relay) fn pairwise_secret_store_override()
-> Option<Arc<dyn SecureMeshSecretStore>> {
    PAIRWISE_SECRET_STORE_OVERRIDE.with(|slot| slot.borrow().as_ref().map(Arc::clone))
}

pub(in crate::domain::mobile_relay) fn mobile_relay_secret_store_override()
-> Option<Arc<dyn SecureMeshSecretStore>> {
    MOBILE_RELAY_SECRET_STORE_OVERRIDE.with(|slot| slot.borrow().as_ref().map(Arc::clone))
}

pub(in crate::domain::mobile_relay) fn selected_mobile_relay_secret_store()
-> Arc<dyn SecureMeshSecretStore> {
    #[cfg(test)]
    if let Some(store) =
        NATIVE_NAMESPACE_STORE_TEST_PORT.with(|slot| slot.borrow().as_ref().map(Arc::clone))
    {
        return store;
    }
    let store =
        MOBILE_RELAY_EPHEMERAL_SECRET_STORE.get_or_init(|| Arc::new(EphemeralSecretStore::new()));
    if native_secret_store_permitted() {
        let platform_store = native_secret_store();
        if platform_store.supported() {
            return Arc::new(platform_store);
        }
        if let Ok(facts) = platform_store.capability_facts() {
            let _ = store.set_unavailable_platform_facts(facts);
        }
    } else {
        let _ = store.set_unavailable_platform_facts(Vec::new());
    }
    Arc::clone(store) as Arc<dyn SecureMeshSecretStore>
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn with_native_namespace_store_test_port<T>(
    store: Arc<dyn SecureMeshSecretStore>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct Restore(Option<Arc<dyn SecureMeshSecretStore>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            NATIVE_NAMESPACE_STORE_TEST_PORT.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let previous = NATIVE_NAMESPACE_STORE_TEST_PORT.with(|slot| slot.replace(Some(store)));
    let _restore = Restore(previous);
    operation()
}

/// Returns the capability evaluation for the secret store selected by the current client.
///
/// Capability evaluation is intentionally non-interactive: it does not begin an
/// authorization session and never reads key material. Platform probe failures are
/// represented by the selected store's conservative capability facts.
pub(in crate::domain::mobile_relay) fn selected_mobile_relay_capability_evaluation_in()
-> Result<CapabilityEvaluation> {
    mobile_relay_secret_store_override()
        .unwrap_or_else(selected_mobile_relay_secret_store)
        .capability_evaluation()
}

/// Execute an MLS operation with the established local device identity and the selected custody
/// backend under one authorization batch. Key material and the authorization session never leave
/// this Rust-only closure.
pub(in crate::domain::mobile_relay) fn with_secure_mesh_mls_participant_in<T>(
    params: &Value,
    additional_secret_store_operations: usize,
    operation: impl FnOnce(
        &mut Value,
        &DeviceTrustPublicIdentity,
        &SigningKey,
        &Arc<dyn SecureMeshSecretStore>,
        &SecretStoreAuthorizationSession,
        &str,
    ) -> Result<T>,
) -> Result<T> {
    ensure_secure_mesh_protected_operation_allowed()?;
    static MLS_RUNTIME_OPERATION_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    let _process_guard = MLS_RUNTIME_OPERATION_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow!("secure mesh MLS participant operation lock is unavailable"))?;
    let operation_lock_path = secure_mesh_mls_state_dir()?.join("participant-operation.lock");
    let operation_lock =
        licoup_foundation::platform::file_security::open_private_lock_file(&operation_lock_path)?;
    fs2::FileExt::lock_exclusive(&operation_lock)
        .map_err(|_| anyhow!("secure mesh MLS participant operation lock could not be acquired"))?;
    ensure!(
        additional_secret_store_operations <= 8,
        "secure mesh MLS secret-store operation budget is invalid"
    );
    let operation_count = mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
        .saturating_add(additional_secret_store_operations);
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context_for_operation(
        params,
        "Secure Mesh MLS selected-custody authorization batch",
        operation_count,
    )?;
    if local_endpoint_state(&config, &secret_context.material).is_err() {
        let endpoint_kind =
            text_param(params, &["endpointKind"]).unwrap_or_else(|| "desktop_sidecar".to_string());
        ensure_mobile_relay_endpoint_material(
            &mut config,
            &mut secret_context.material,
            &endpoint_kind,
        )?;
    }
    let endpoint = local_endpoint_state(&config, &secret_context.material)?;
    let identity = endpoint.device_identity()?;
    let signing_key = endpoint.signing_key()?;
    let (secret_store, authorization, namespace) = secret_context
        .secret_store_batch
        .authorization()?
        .ok_or_else(|| anyhow!("secure mesh MLS selected custody is unavailable"))?;
    ensure!(
        secret_store.supported(),
        "secure mesh MLS selected custody is unsupported"
    );
    let output = operation(
        &mut config,
        &identity,
        &signing_key,
        &secret_store,
        &authorization,
        &namespace,
    )?;
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_secret_overrides_merge_without_losing_authority_state() {
        let mut accumulated = RuntimeSecretOverrides {
            pc_token: true,
            ..RuntimeSecretOverrides::default()
        };
        let incoming = RuntimeSecretOverrides {
            mobile_token: true,
            e2ee_private_key: true,
            secret_storage_backend: Some("memory-only-ephemeral"),
            ..RuntimeSecretOverrides::default()
        };

        accumulated.merge(incoming);

        assert!(accumulated.pc_token);
        assert!(accumulated.mobile_token);
        assert!(accumulated.e2ee_private_key);
        assert_eq!(
            accumulated.secret_storage_backend,
            Some("memory-only-ephemeral")
        );
    }
}
