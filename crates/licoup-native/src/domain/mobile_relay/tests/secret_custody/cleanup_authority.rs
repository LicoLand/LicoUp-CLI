use super::super::test_support::*;
use crate::core::secure_mesh_secret_store::{SecretBytes, SecretZeroizeProbe};
use std::sync::atomic::{AtomicBool, Ordering};

/// Build the exact informed confirmation the cleanup authority expects, from
/// the bounded inventory the operator can read back.
fn confirmation_for(
    inventory: &CustodyCleanupInventory,
    replacement_endpoint_id: &str,
    replacement_device_trust_fingerprint: &str,
) -> Value {
    json!({
        "schemaVersion": CUSTODY_CLEANUP_CONFIRMATION_SCHEMA,
        "consequence": CUSTODY_CLEANUP_CONSEQUENCE,
        "subjectEndpointId": inventory.subject.endpoint_id.clone(),
        "subjectIdentityFingerprint": inventory.subject.identity_fingerprint.clone(),
        "custodyNamespace": inventory.subject.custody_namespace.clone(),
        "replacementEndpointId": replacement_endpoint_id,
        "replacementDeviceTrustFingerprint": replacement_device_trust_fingerprint,
        "inventoryDigest": inventory.digest.clone(),
        "scope": inventory.scope.clone(),
    })
}

fn cleanup_params(confirmation: Value) -> Value {
    json!({ "cleanupConfirmation": confirmation })
}

fn persisted_cleanup_config() -> Result<Value> {
    load_config_for_custody_cleanup()
}

fn presence_snapshot(
    store: &EphemeralSecretStore,
    handles: &[SecretStoreHandle],
) -> Result<Vec<bool>> {
    handles
        .iter()
        .map(|handle| Ok(store.get_secret(handle)?.is_some()))
        .collect()
}

/// Plant secrets that are deliberately outside the authorized inventory: one
/// sibling key in the custody namespace and one shared secret class owned by
/// the protected store surface. Neither may ever be erased by this cleanup.
fn plant_out_of_scope_canaries(
    store: &EphemeralSecretStore,
    inventory: &CustodyCleanupInventory,
) -> Result<Vec<SecretStoreHandle>> {
    let handles = vec![
        SecretStoreHandle::new(
            format!(
                "{}:{}",
                NATIVE_SECRET_STORE_ACCOUNT_PREFIX, MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE
            ),
            "custodyCleanupBoundaryCanary",
        )?,
        SecretStoreHandle::new(
            native_secret_store_shared_secret_classes_namespace()?,
            "pairwiseSessionSnapshot",
        )?,
    ];
    for handle in &handles {
        assert!(
            inventory
                .secret_handles
                .iter()
                .all(|candidate| candidate != handle),
            "out-of-scope canary must not be part of the authorized inventory"
        );
        store.set_secret(
            handle,
            SecretBytes::try_from_bytes(b"synthetic-out-of-scope-custody-canary".to_vec())?,
        )?;
    }
    Ok(handles)
}

#[test]
fn mobile_relay_custody_cleanup_admits_only_the_authorized_bounded_inventory() {
    let dir = temp_dir("mobile-relay-custody-cleanup-authority");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            assert!(
                mobile_config["mobileRelayE2ee"]["peerTrustRecord"].is_object(),
                "pairing must persist a signed device trust record for the replacement endpoint"
            );
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(mobile_config),
            )?;

            let config = persisted_cleanup_config()?;
            let inventory = enumerate_custody_cleanup_inventory(&config)?;
            let replacement = authenticated_replacement_endpoint(&config)?;
            assert_eq!(
                replacement.endpoint_id,
                config["mobileRelayE2ee"]["peerEndpointId"]
                    .as_str()
                    .unwrap()
            );
            assert!(
                inventory.secret_handles.len() > inventory.pairwise_snapshot_handle_count,
                "the authorized inventory must contain the subject's own custody handles"
            );
            assert!(inventory.pairwise_snapshot_handle_count > 0);
            let root_handles =
                custody_cleanup_root_secret_handles(&config, &inventory.subject.custody_namespace)?;
            assert_eq!(root_handles.len(), inventory.root_secret_handle_count);

            let mut cleanup_probes = Vec::new();
            for handle in &root_handles {
                let probe = SecretZeroizeProbe::new();
                secret_store.set_secret(
                    handle,
                    SecretBytes::try_from_bytes_with_test_zeroize_probe(
                        b"synthetic-custody-cleanup-canary".to_vec(),
                        probe.clone(),
                    )?,
                )?;
                cleanup_probes.push(probe);
            }
            let out_of_scope = plant_out_of_scope_canaries(&secret_store, &inventory)?;
            let baseline_session_count = secret_store.authorization_session_count();

            let output = e2ee_secret_store_cleanup(&cleanup_params(confirmation_for(
                &inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            )))?;

            let operation_count = inventory.secret_handles.len();
            assert_eq!(output["ok"], true);
            assert_eq!(output["status"], "cleaned");
            assert_eq!(
                output["authority"]["inventoryDigest"],
                inventory.digest.as_str()
            );
            assert_eq!(
                output["authority"]["replacementEndpointId"],
                replacement.endpoint_id.as_str()
            );
            assert_eq!(
                output["authority"]["model"],
                "authenticatedReplacementEndpointWithInformedConfirmation"
            );
            assert_eq!(
                output["authority"]["confirmationScopeEntryCount"],
                inventory.scope.len()
            );
            assert_eq!(output["deletedSecretHandleCount"], operation_count);
            assert_eq!(output["rootSecretHandleCount"], root_handles.len());
            assert_eq!(
                output["pairwiseSnapshotHandleCount"],
                inventory.pairwise_snapshot_handle_count
            );
            assert_eq!(output["pairwiseDatabasePresentBefore"], true);
            assert_eq!(output["pairwiseDatabaseRemoved"], true);
            assert_eq!(
                secret_store.authorization_session_count(),
                baseline_session_count + 1
            );
            assert_eq!(
                secret_store.authorization_session_reasons()[baseline_session_count],
                "Mobile Relay authenticated replacement custody cleanup"
            );
            assert_eq!(
                secret_store.authorization_session_operation_counts()[baseline_session_count],
                operation_count
            );
            assert_eq!(
                secret_store.authorization_session_consumed_operation_counts()
                    [baseline_session_count],
                operation_count
            );
            assert!(
                !secret_store.authorization_session_allow_interactions()[baseline_session_count]
            );
            for handle in &inventory.secret_handles {
                assert!(secret_store.get_secret(handle)?.is_none());
            }
            for handle in &out_of_scope {
                assert!(
                    secret_store.get_secret(handle)?.is_some(),
                    "custody cleanup must not touch secrets outside its authorized inventory"
                );
            }
            for probe in cleanup_probes {
                assert_eq!(
                    probe.observations(),
                    vec![vec![0; b"synthetic-custody-cleanup-canary".len()]],
                    "cleanup must wipe each removed owned secret before releasing its backing"
                );
            }

            // The confirmation is bound to the scope it named: once the
            // pairwise store is gone, the same record must be refused.
            let error = e2ee_secret_store_cleanup(&cleanup_params(confirmation_for(
                &inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            )))
            .unwrap_err()
            .to_string();
            assert!(
                error.contains("inventory digest does not match"),
                "replayed confirmation must be refused: {error}"
            );

            let second_inventory = enumerate_custody_cleanup_inventory(&config)?;
            assert_eq!(second_inventory.pairwise_snapshot_handle_count, 0);
            assert!(!second_inventory.pairwise_database_present);
            let second = e2ee_secret_store_cleanup(&cleanup_params(confirmation_for(
                &second_inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            )))?;
            assert_eq!(second["ok"], true);
            assert_eq!(second["pairwiseSnapshotHandleCount"], 0);
            assert_eq!(second["pairwiseDatabasePresentBefore"], false);
            assert_eq!(second["pairwiseDatabaseRemoved"], true);
            assert_eq!(second["deletedSecretHandleCount"], root_handles.len());
            for handle in &out_of_scope {
                assert!(secret_store.get_secret(handle)?.is_some());
            }
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_custody_cleanup_rejects_without_an_authenticated_replacement_endpoint() {
    let dir = temp_dir("mobile-relay-custody-cleanup-unauthenticated");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
                &mut pc_config,
                &mut test_runtime_secret_material(stringify!(pc_config)),
                "desktop_sidecar",
            )?;
            let mut mobile_config = default_config();
            ensure_mobile_relay_endpoint_descriptor(
                &mut mobile_config,
                &mut test_runtime_secret_material(stringify!(mobile_config)),
                "mobile",
            )?;
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(mobile_config),
            )?;

            let mut out_of_scope: Vec<SecretStoreHandle> = Vec::new();

            for evidence in [
                "absent_replacement_endpoint",
                "tampered_trust_record_signature",
                "caller_asserted_peer_verified_flag",
            ] {
                if evidence != "absent_replacement_endpoint" {
                    apply_peer_secure_mesh_descriptor(
                        &mut mobile_config,
                        &mut test_runtime_secret_material(stringify!(mobile_config)),
                        &pc_descriptor,
                        true,
                    )?;
                }
                match evidence {
                    "tampered_trust_record_signature" => {
                        assert!(
                            mobile_config["mobileRelayE2ee"]["peerTrustRecord"].is_object(),
                            "pairing must produce a trust record to tamper with"
                        );
                        mobile_config["mobileRelayE2ee"]["peerTrustRecord"]["signatureBase64url"] =
                            json!("not-a-valid-detached-signature");
                    }
                    "caller_asserted_peer_verified_flag" => {
                        mobile_config["mobileRelayE2ee"]
                            .as_object_mut()
                            .unwrap()
                            .remove("peerTrustRecord");
                        mobile_config["mobileRelayE2ee"]["peerVerified"] = json!(true);
                    }
                    _ => {}
                }
                save_test_config_with_runtime_secret_context(
                    &mut mobile_config,
                    stringify!(mobile_config),
                )?;

                let config = persisted_cleanup_config()?;
                let inventory = enumerate_custody_cleanup_inventory(&config)?;
                if out_of_scope.is_empty() {
                    out_of_scope = plant_out_of_scope_canaries(&secret_store, &inventory)?;
                }
                // The confirmation names the peer identity exactly as the
                // persisted state records it. Only the verified trust record is
                // missing, so this test fails if that check is removed.
                let replacement_identity = config
                    .get("mobileRelayE2ee")
                    .and_then(|state| peer_device_identity_from_state(state).ok());
                let (replacement_endpoint_id, replacement_device_trust_fingerprint) =
                    match replacement_identity {
                        Some(peer) => {
                            let fingerprint = peer.fingerprint()?;
                            (peer.endpoint_id, fingerprint)
                        }
                        None => (
                            "endpoint-never-authenticated".to_string(),
                            "sha256:endpoint-never-authenticated".to_string(),
                        ),
                    };
                let confirmation = confirmation_for(
                    &inventory,
                    &replacement_endpoint_id,
                    &replacement_device_trust_fingerprint,
                );
                // A refused cleanup must neither erase custody nor open an
                // authorization session: both are measured around this call.
                let session_count_before = secret_store.authorization_session_count();
                let presence_before = presence_snapshot(&secret_store, &inventory.secret_handles)?;
                let error = e2ee_secret_store_cleanup(&cleanup_params(confirmation))
                    .unwrap_err()
                    .to_string();
                assert!(
                    error
                        .contains("custody cleanup requires an authenticated replacement endpoint"),
                    "{evidence} must be refused as unauthenticated: {error}"
                );
                assert_eq!(
                    secret_store.authorization_session_count(),
                    session_count_before,
                    "{evidence}: a refused cleanup must not begin a secret-store authorization session"
                );
                assert_eq!(
                    presence_snapshot(&secret_store, &inventory.secret_handles)?,
                    presence_before,
                    "{evidence}: a refused cleanup must not delete any custody secret"
                );
            }

            for handle in &out_of_scope {
                assert!(secret_store.get_secret(handle)?.is_some());
            }
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_custody_cleanup_rejects_a_confirmation_outside_the_bounded_inventory() {
    let dir = temp_dir("mobile-relay-custody-cleanup-out-of-scope");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(mobile_config),
            )?;

            let config = persisted_cleanup_config()?;
            let inventory = enumerate_custody_cleanup_inventory(&config)?;
            let replacement = authenticated_replacement_endpoint(&config)?;
            let authorized = confirmation_for(
                &inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            );
            let out_of_scope = plant_out_of_scope_canaries(&secret_store, &inventory)?;
            let pairwise_path = mobile_relay_pairwise_store_path()?;
            assert!(pairwise_path.exists());

            let wide_scope = {
                let mut confirmation = authorized.clone();
                confirmation["scope"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("secret-handle:outside-custody:pcToken"));
                confirmation
            };
            let narrow_scope = {
                let mut confirmation = authorized.clone();
                confirmation["scope"].as_array_mut().unwrap().pop();
                confirmation
            };
            let foreign_digest = {
                let mut confirmation = authorized.clone();
                confirmation["inventoryDigest"] = json!("f".repeat(64));
                confirmation
            };
            let foreign_subject = {
                let mut confirmation = authorized.clone();
                confirmation["subjectEndpointId"] = json!("endpoint-other-subject");
                confirmation
            };
            let foreign_replacement = {
                let mut confirmation = authorized.clone();
                confirmation["replacementDeviceTrustFingerprint"] = json!("sha256:other-peer");
                confirmation
            };
            let missing_consequence = {
                let mut confirmation = authorized.clone();
                confirmation["consequence"] = json!("confirmed");
                confirmation
            };
            let baseline_session_count = secret_store.authorization_session_count();
            let baseline_presence = presence_snapshot(&secret_store, &inventory.secret_handles)?;

            for (case, confirmation, expected) in [
                (
                    "scope_wider_than_inventory",
                    wide_scope,
                    "outside the bounded custody inventory",
                ),
                (
                    "scope_narrower_than_inventory",
                    narrow_scope,
                    "does not cover the enumerated custody inventory",
                ),
                (
                    "digest_for_another_scope",
                    foreign_digest,
                    "inventory digest does not match",
                ),
                (
                    "subject_not_the_derived_subject",
                    foreign_subject,
                    "does not match the derived custody subject",
                ),
                (
                    "replacement_not_the_authenticated_endpoint",
                    foreign_replacement,
                    "does not name the authenticated replacement endpoint",
                ),
                (
                    "consequence_not_named",
                    missing_consequence,
                    "irreversible local secret erasure consequence",
                ),
                (
                    "confirmation_absent",
                    Value::Null,
                    "requires an explicit informed confirmation record",
                ),
            ] {
                let params = if confirmation.is_null() {
                    json!({})
                } else {
                    cleanup_params(confirmation)
                };
                let error = e2ee_secret_store_cleanup(&params).unwrap_err().to_string();
                assert!(
                    error.contains(expected),
                    "{case} must be refused with {expected}: {error}"
                );
            }

            assert_eq!(
                secret_store.authorization_session_count(),
                baseline_session_count,
                "out-of-scope confirmations must not begin an authorization session"
            );
            assert_eq!(
                presence_snapshot(&secret_store, &inventory.secret_handles)?,
                baseline_presence,
                "an out-of-scope confirmation must not delete authorized custody"
            );
            for handle in &out_of_scope {
                assert!(secret_store.get_secret(handle)?.is_some());
            }
            assert!(
                pairwise_path.exists(),
                "an out-of-scope confirmation must not remove the pairwise store"
            );
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_custody_cleanup_inventory_report_is_redacted_and_stable() {
    let dir = temp_dir("mobile-relay-custody-cleanup-inventory-report");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(mobile_config),
            )?;

            let config = persisted_cleanup_config()?;
            let inventory = enumerate_custody_cleanup_inventory(&config)?;
            let baseline_presence = presence_snapshot(&secret_store, &inventory.secret_handles)?;
            let runtime_secrets = MobileRelayE2eeSecretField::ALL
                .map(|field| test_runtime_e2ee_secret(stringify!(mobile_config), field));

            let first = e2ee_secret_store_cleanup_inventory()?;
            let second = e2ee_secret_store_cleanup_inventory()?;
            assert_eq!(first["ok"], true);
            assert_eq!(first["status"], "inventory");
            assert_eq!(first["requiresAuthenticatedReplacementEndpoint"], true);
            assert_eq!(first["requiresExplicitInformedConfirmation"], true);
            assert_eq!(
                first["confirmationSchemaVersion"],
                CUSTODY_CLEANUP_CONFIRMATION_SCHEMA
            );
            assert_eq!(
                first["confirmationConsequence"],
                CUSTODY_CLEANUP_CONSEQUENCE
            );
            assert_eq!(first["inventory"]["redacted"], true);
            assert_eq!(first["inventory"]["secretValuesIncluded"], false);
            assert_eq!(first["inventory"]["localPathsIncluded"], false);
            assert_eq!(
                first["inventory"]["inventoryDigest"],
                second["inventory"]["inventoryDigest"]
            );
            assert_eq!(first["inventory"]["scope"], second["inventory"]["scope"]);
            assert!(
                first["inventory"]["scope"].as_array().unwrap().len()
                    > first["inventory"]["secretHandleCount"].as_u64().unwrap() as usize
            );

            let serialized = serde_json::to_string(&first)?;
            let data_root = dir.to_string_lossy().to_string();
            assert!(!serialized.contains(data_root.as_str()));
            for secret in &runtime_secrets {
                assert!(
                    !secret.is_empty() && !serialized.contains(secret.as_str()),
                    "the inventory report must never contain secret material"
                );
            }

            assert_eq!(
                presence_snapshot(&secret_store, &inventory.secret_handles)?,
                baseline_presence,
                "the inventory read path must not erase custody"
            );
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_custody_cleanup_reports_denied_deletion_as_pending() {
    struct DeleteRejectingSecretStore {
        inner: EphemeralSecretStore,
        rejected_key: &'static str,
        reject_deletes: AtomicBool,
    }

    impl SecureMeshSecretStore for DeleteRejectingSecretStore {
        fn backend(&self) -> &'static str {
            self.inner.backend()
        }

        fn supported(&self) -> bool {
            self.inner.supported()
        }

        fn begin_authorized_session(
            &self,
            request: &SecretStoreAuthorizationRequest,
        ) -> Result<SecretStoreAuthorizationSession> {
            self.inner.begin_authorized_session(request)
        }

        fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
            self.inner.set_secret(handle, secret)
        }

        fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            self.inner.get_secret(handle)
        }

        fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
            if self.reject_deletes.load(Ordering::SeqCst) && handle.key() == self.rejected_key {
                return Err(anyhow!("injected custody cleanup delete failure"));
            }
            self.inner.delete_secret(handle)
        }
    }

    let dir = temp_dir("mobile-relay-custody-cleanup-delete-failure");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(DeleteRejectingSecretStore {
        inner: EphemeralSecretStore::new(),
        rejected_key: "mobileToken",
        reject_deletes: AtomicBool::new(false),
    });
    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(store_override.clone(), || {
        with_pairwise_secret_store_override(store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(mobile_config),
            )?;

            let config = persisted_cleanup_config()?;
            let inventory = enumerate_custody_cleanup_inventory(&config)?;
            let replacement = authenticated_replacement_endpoint(&config)?;
            let rejected_handle = native_secret_store_handle_for_namespace(
                &inventory.subject.custody_namespace,
                "mobileToken",
            )?;
            assert!(
                inventory
                    .secret_handles
                    .iter()
                    .any(|handle| handle == &rejected_handle)
            );
            store.set_secret(
                &rejected_handle,
                SecretBytes::try_from_bytes(b"synthetic-delete-failure-canary".to_vec())?,
            )?;
            let baseline_session_count = store.inner.authorization_session_count();
            store.reject_deletes.store(true, Ordering::SeqCst);

            let output = e2ee_secret_store_cleanup(&cleanup_params(confirmation_for(
                &inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            )))?;

            // A denied deletion is not converted into a finished erase: the
            // operation settles as partial, names the pending credential and
            // still reports the replacement endpoint as unconfirmed.
            let operation_count = inventory.secret_handles.len();
            assert_eq!(output["ok"], true);
            assert_eq!(output["status"], "partial");
            assert_eq!(output["complete"], false);
            assert_eq!(output["settlement"]["complete"], false);
            assert_eq!(
                output["settlement"]["deletedSecretHandleCount"],
                operation_count - 1
            );
            let pending = output["settlement"]["pendingSecretHandles"]
                .as_array()
                .expect("pending handles are listed");
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0]["handleKey"], "mobileToken");
            assert!(
                pending[0]["reason"]
                    .as_str()
                    .unwrap()
                    .contains("injected custody cleanup delete failure")
            );
            assert_eq!(output["receipt"]["complete"], false);
            assert_eq!(output["receipt"]["replacementEndpointConfirmed"], false);
            assert_eq!(output["receipt"]["confirmation"], "pendingReceiptDelivery");
            assert!(store.get_secret(&rejected_handle)?.is_some());
            assert_eq!(
                store.inner.authorization_session_count(),
                baseline_session_count + 1
            );
            assert!(
                !store.inner.authorization_session_allow_interactions()[baseline_session_count]
            );
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_custody_cleanup_completes_once_the_denied_deletion_is_allowed() {
    struct DeferredDeleteSecretStore {
        inner: EphemeralSecretStore,
        rejected_key: &'static str,
        reject_deletes: AtomicBool,
    }

    impl SecureMeshSecretStore for DeferredDeleteSecretStore {
        fn backend(&self) -> &'static str {
            self.inner.backend()
        }

        fn supported(&self) -> bool {
            self.inner.supported()
        }

        fn begin_authorized_session(
            &self,
            request: &SecretStoreAuthorizationRequest,
        ) -> Result<SecretStoreAuthorizationSession> {
            self.inner.begin_authorized_session(request)
        }

        fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
            self.inner.set_secret(handle, secret)
        }

        fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            self.inner.get_secret(handle)
        }

        fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
            if self.reject_deletes.load(Ordering::SeqCst) && handle.key() == self.rejected_key {
                return Err(anyhow!("injected custody cleanup delete failure"));
            }
            self.inner.delete_secret(handle)
        }
    }

    let dir = temp_dir("mobile-relay-custody-cleanup-deferred-delete");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(DeferredDeleteSecretStore {
        inner: EphemeralSecretStore::new(),
        rejected_key: "mobileToken",
        reject_deletes: AtomicBool::new(true),
    });
    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(store_override.clone(), || {
        with_pairwise_secret_store_override(store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(mobile_config),
            )?;

            let config = persisted_cleanup_config()?;
            let inventory = enumerate_custody_cleanup_inventory(&config)?;
            let replacement = authenticated_replacement_endpoint(&config)?;
            let rejected_handle = native_secret_store_handle_for_namespace(
                &inventory.subject.custody_namespace,
                "mobileToken",
            )?;
            store.set_secret(
                &rejected_handle,
                SecretBytes::try_from_bytes(b"synthetic-deferred-delete-canary".to_vec())?,
            )?;

            let partial = e2ee_secret_store_cleanup(&cleanup_params(confirmation_for(
                &inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            )))?;
            assert_eq!(partial["status"], "partial");
            assert!(store.get_secret(&rejected_handle)?.is_some());

            // Once the platform allows the deletion, the same authorized
            // inventory settles completely.
            store.reject_deletes.store(false, Ordering::SeqCst);
            let second_inventory = enumerate_custody_cleanup_inventory(&config)?;
            let settled = e2ee_secret_store_cleanup(&cleanup_params(confirmation_for(
                &second_inventory,
                &replacement.endpoint_id,
                &replacement.device_trust_fingerprint,
            )))?;
            assert_eq!(settled["status"], "cleaned");
            assert_eq!(settled["complete"], true);
            assert_eq!(settled["settlement"]["pendingSecretHandles"], json!([]));
            assert_eq!(
                settled["settlement"]["pendingPairwiseStoreFiles"],
                json!([])
            );
            assert_eq!(settled["receipt"]["complete"], true);
            assert_eq!(settled["receipt"]["replacementEndpointConfirmed"], false);
            assert!(store.get_secret(&rejected_handle)?.is_none());
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}
