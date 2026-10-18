use super::*;
use crate::core::secure_mesh_secret_store::SecretBytes;

/// Execute a bounded custody cleanup that already passed the authority checks
/// in [`cleanup_authority`].
///
/// The loop below only ever walks the authorized inventory: the subject and the
/// scope come from [`authorize_custody_cleanup`], never from the parameters.
///
/// Settlement is observed, never assumed. Each authorized credential deletion
/// and each authorized durable-store file is recorded as settled or pending, and
/// `complete` is reported only when nothing is left pending. A refusal — a
/// locked platform store, an unavailable backend, a denied file removal — stays
/// `partial` with the exact pending entries, so an incomplete erase is never
/// converted into a finished one.
///
/// After the last observation this function writes nothing: no data root, log,
/// temporary payload or credential is recreated on the cleaned side. The
/// returned document is the caller's; the replacement endpoint learns the
/// outcome only by receiving it, which is why the receipt block reports
/// `replacementEndpointConfirmed: false` until that endpoint says otherwise.
pub(in crate::domain::mobile_relay) fn e2ee_secret_store_cleanup_in(
    params: &Value,
) -> Result<Value> {
    let config = load_config_for_custody_cleanup()?;
    let authorized = authorize_custody_cleanup(&config, params)?;
    let inventory = authorized.inventory;

    let (store, namespace) = custody_cleanup_secret_store()?;
    ensure!(
        namespace == inventory.subject.custody_namespace,
        "mobile relay custody cleanup secret store namespace does not match the authorized subject"
    );
    ensure!(
        store.supported(),
        "mobile relay native secret store backend is unsupported"
    );

    let operation_count = inventory.secret_handles.len();
    let session =
        store.begin_authorized_session(&SecretStoreAuthorizationRequest::noninteractive(
            "Mobile Relay authenticated replacement custody cleanup",
            operation_count,
        ))?;
    let mut pending_secret_handles: Vec<Value> = Vec::new();
    for handle in &inventory.secret_handles {
        if let Err(error) = store.delete_secret_with_session(&session, handle) {
            pending_secret_handles.push(json!({
                "handleKey": handle.key(),
                "reason": custody_cleanup_reason_code(&error.to_string()),
            }));
        }
    }
    let deleted_secret_handle_count = operation_count.saturating_sub(pending_secret_handles.len());
    if pending_secret_handles.is_empty() {
        // No extra key use: the session performed exactly the deletions the
        // authorized inventory declared and nothing else consumed an operation.
        ensure!(
            session.consumed_operation_count() == operation_count
                && session.authorization_batch_within_budget()
                && session.remaining_operation_count() == 0,
            "mobile relay custody cleanup operation budget mismatch"
        );
    }

    let removal = remove_authorized_pairwise_store_files(&inventory.pairwise_store_files)?;
    let pairwise_path = mobile_relay_pairwise_store_path()?;
    // Observation, not assumption: the durable-store files are reported settled
    // only when every authorized path is absent now.
    let pending_pairwise_store_files: Vec<String> = inventory
        .pairwise_store_files
        .iter()
        .filter(|path| path.exists())
        .map(|path| custody_cleanup_file_name(path))
        .collect();
    let complete = pending_secret_handles.is_empty() && pending_pairwise_store_files.is_empty();
    Ok(json!({
        "ok": true,
        "status": if complete { "cleaned" } else { "partial" },
        "complete": complete,
        "authority": {
            "model": "authenticatedReplacementEndpointWithInformedConfirmation",
            "subjectEndpointId": inventory.subject.endpoint_id.clone(),
            "subjectIdentityFingerprint": inventory.subject.identity_fingerprint.clone(),
            "custodyNamespace": inventory.subject.custody_namespace.clone(),
            "replacementEndpointId": authorized.replacement.endpoint_id.clone(),
            "replacementDeviceTrustFingerprint":
                authorized.replacement.device_trust_fingerprint.clone(),
            "replacementVerificationMethod": authorized.replacement.verification_method.clone(),
            "inventoryDigest": inventory.digest.clone(),
            "scopeEntryCount": inventory.scope.len(),
            "confirmationScopeEntryCount": authorized.confirmation_scope_entry_count,
        },
        "settlement": {
            "complete": complete,
            "authorizedSecretHandleCount": operation_count,
            "deletedSecretHandleCount": deleted_secret_handle_count,
            "pendingSecretHandles": pending_secret_handles,
            "authorizedPairwiseStoreFileCount": inventory.pairwise_store_files.len(),
            "removedPairwiseStoreFileCount": removal.removed,
            "pendingPairwiseStoreFiles": pending_pairwise_store_files,
        },
        "receipt": {
            "kind": CUSTODY_CLEANUP_RECEIPT_KIND,
            "issued": true,
            "complete": complete,
            // Only the replacement endpoint can observe arrival, and a lost
            // receipt is never silently converted into success here.
            "replacementEndpointConfirmed": false,
            "confirmation": "pendingReceiptDelivery",
            "subjectEndpointId": inventory.subject.endpoint_id.clone(),
            "replacementEndpointId": authorized.replacement.endpoint_id.clone(),
            "inventoryDigest": inventory.digest.clone(),
        },
        "deletedSecretHandleCount": deleted_secret_handle_count,
        "rootSecretHandleCount": inventory.root_secret_handle_count,
        "pairwiseSnapshotHandleCount": inventory.pairwise_snapshot_handle_count,
        "pairwiseDatabasePresentBefore": inventory.pairwise_database_present,
        "pairwiseDatabaseRemoved": !pairwise_path.exists(),
        "removedPairwiseDatabaseFileCount": removal.removed,
        "secretStoreAuthorization": {
            "backend": session.backend(),
            "allowInteraction": session.allow_interaction(),
            "operationCount": session.operation_count(),
            "consumedOperationCount": session.consumed_operation_count(),
            "remainingOperationCount": session.remaining_operation_count(),
            "authorizationBatchWithinBudget": session.authorization_batch_within_budget()
        }
    }))
}

/// The restricted receipt document this cleanup issues to the replacement
/// endpoint. The endpoint consumes it over its own authenticated control path.
pub(in crate::domain::mobile_relay) const CUSTODY_CLEANUP_RECEIPT_KIND: &str =
    "licoup.custody-cleanup-receipt.v1";

fn custody_cleanup_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("custody-cleanup-file")
        .to_string()
}

fn custody_cleanup_reason_code(error: &str) -> String {
    match error.split(':').next() {
        Some(code) if !code.trim().is_empty() => code.trim().to_string(),
        _ => "mobile_relay_custody_cleanup_failed".to_string(),
    }
}

pub(in crate::domain::mobile_relay) fn load_config_for_custody_cleanup() -> Result<Value> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(normalize_config(json!({})));
    }
    let raw =
        fs::read_to_string(&path).context("mobile relay custody cleanup config read failed")?;
    let parsed = serde_json::from_str::<Value>(&raw)
        .context("mobile relay custody cleanup config is invalid")?;
    crate::domain::mobile_relay::validate_current_config_document(&parsed)?;
    Ok(normalize_config(parsed))
}

fn custody_cleanup_secret_store() -> Result<(Arc<dyn SecureMeshSecretStore>, String)> {
    if let Some(store) = mobile_relay_secret_store_override() {
        return Ok((
            store,
            MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE.to_string(),
        ));
    }
    ensure!(
        native_secret_store_enabled(),
        "mobile relay native secret store is required for custody cleanup"
    );
    Ok((
        Arc::new(native_secret_store()),
        native_secret_store_namespace()?,
    ))
}

pub(in crate::domain::mobile_relay) fn custody_cleanup_root_secret_handles(
    config: &Value,
    namespace: &str,
) -> Result<Vec<SecretStoreHandle>> {
    let mut handles = vec![native_e2ee_secret_bundle_handle_for_namespace(namespace)?];
    for field in MOBILE_RELAY_NATIVE_TOKEN_SECRET_FIELDS {
        handles.push(native_secret_store_handle_for_namespace(namespace, field)?);
    }
    for (field, _) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS {
        handles.push(native_secret_store_handle_for_namespace(namespace, field)?);
    }
    if let Some(devices) = config.get("pairedDevices").and_then(Value::as_array) {
        for device in devices {
            if let Some(key) = paired_device_token_secret_store_key(device) {
                handles.push(native_secret_store_handle_for_namespace(namespace, &key)?);
            }
        }
    }
    handles.sort_by(|left, right| {
        left.namespace()
            .cmp(right.namespace())
            .then_with(|| left.key().cmp(right.key()))
    });
    handles.dedup();
    Ok(handles)
}

/// What removing the authorized pairwise durable-store files observed.
struct PairwiseStoreFileRemoval {
    removed: usize,
}

/// Remove exactly the authorized pairwise durable-store files. A file that is
/// already absent is not an error; a removal the platform denies is not an
/// error either, because it is reported as pending by the caller's own
/// observation of the authorized paths and never as a completed erase.
fn remove_authorized_pairwise_store_files(paths: &[PathBuf]) -> Result<PairwiseStoreFileRemoval> {
    let mut removed = 0usize;
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => removed = removed.saturating_add(1),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(error) => {
                return Err(error).context("mobile relay custody pairwise database cleanup failed");
            }
        }
    }
    Ok(PairwiseStoreFileRemoval { removed })
}

pub(in crate::domain::mobile_relay) fn native_secret_store() -> PlatformSecretStore {
    PlatformSecretStore::new(
        NATIVE_SECRET_STORE_SERVICE,
        NATIVE_SECRET_STORE_ACCOUNT_PREFIX,
    )
}

pub(in crate::domain::mobile_relay) fn native_secret_store_namespace() -> Result<String> {
    let path = config_path()?;
    Ok(native_secret_store_namespace_for_config_path(&path))
}

pub(in crate::domain::mobile_relay) fn native_secret_store_namespace_for_config_path(
    path: &Path,
) -> String {
    sha256_hex(path.to_string_lossy().as_bytes())
}

pub(in crate::domain::mobile_relay) fn native_secret_store_handle_for_namespace(
    namespace: &str,
    field: &str,
) -> Result<SecretStoreHandle> {
    SecretStoreHandle::new(
        format!("{}:{}", NATIVE_SECRET_STORE_ACCOUNT_PREFIX, namespace),
        field,
    )
}

pub(in crate::domain::mobile_relay) fn native_e2ee_secret_bundle_handle_for_namespace(
    namespace: &str,
) -> Result<SecretStoreHandle> {
    native_secret_store_handle_for_namespace(namespace, MOBILE_RELAY_E2EE_NATIVE_SECRET_BUNDLE_KEY)
}

pub(in crate::domain::mobile_relay) fn native_secret_store_shared_secret_classes_namespace()
-> Result<String> {
    let path = config_path()?;
    Ok(format!(
        "{}:sharedSecretClasses",
        sha256_hex(path.to_string_lossy().as_bytes())
    ))
}

pub(in crate::domain::mobile_relay) fn verify_secret_class_round_trip_with_session(
    store: &dyn SecureMeshSecretStore,
    session: &SecretStoreAuthorizationSession,
    namespace: impl Into<String>,
    secret_classes: &[&str],
) -> Result<SecretClassPersistenceProof> {
    let namespace = namespace.into();
    let mut stored_class_count = 0usize;
    let mut deleted_class_count = 0usize;
    let mut handles = Vec::new();
    for secret_class in secret_classes {
        let handle = SecretStoreHandle::new(&namespace, *secret_class)?;
        let proof_secret = format!("secure-mesh-secret-class-proof:{}", Uuid::new_v4());
        store.set_secret_with_session(
            session,
            &handle,
            SecretBytes::try_from_string(proof_secret.clone())?,
        )?;
        if store
            .get_secret_with_session(session, &handle)?
            .as_ref()
            .map(SecretBytes::expose_bytes)
            == Some(proof_secret.as_bytes())
        {
            stored_class_count = stored_class_count.saturating_add(1);
        }
        handles.push(handle);
    }
    for handle in &handles {
        store.delete_secret_with_session(session, handle)?;
        if store.get_secret_with_session(session, handle)?.is_none() {
            deleted_class_count = deleted_class_count.saturating_add(1);
        }
    }
    Ok(SecretClassPersistenceProof {
        backend: store.backend(),
        secret_classes: secret_classes
            .iter()
            .map(|secret_class| (*secret_class).to_string())
            .collect(),
        requested_class_count: secret_classes.len(),
        persisted_class_count: stored_class_count,
        deleted_class_count,
        all_classes_persisted: stored_class_count == secret_classes.len(),
        all_classes_deleted: deleted_class_count == secret_classes.len(),
        raw_secret_material_included: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custody_cleanup_handle_set_is_deduplicated_and_bounded() {
        let config = json!({
            "pairedDevices": [
                {"id": "device-a", "pairingId": "pairing-a"},
                {"id": "device-b", "pairingId": "pairing-a"}
            ]
        });
        let handles = custody_cleanup_root_secret_handles(&config, "fixture").unwrap();
        let expected = 1
            + MOBILE_RELAY_NATIVE_TOKEN_SECRET_FIELDS.len()
            + MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS.len()
            + 1;

        assert_eq!(handles.len(), expected);
        assert!(handles.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn custody_cleanup_authorized_file_removal_ignores_absent_files_and_reports_removed() {
        let root = std::env::temp_dir().join(format!(
            "licoup-custody-cleanup-scope-files-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let present = root.join("pairwise-pqxdh.sqlite3");
        fs::write(&present, b"synthetic-cleanup-scope-fixture").unwrap();
        let absent = root.join("pairwise-pqxdh.sqlite3-wal");
        assert_eq!(
            remove_authorized_pairwise_store_files(&[present.clone(), absent.clone()])
                .unwrap()
                .removed,
            1
        );
        assert!(!present.exists());
        assert!(!absent.exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
