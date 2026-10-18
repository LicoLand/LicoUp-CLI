//! Authority for bounded local custody cleanup.
//!
//! Erasing the mobile-relay secret-store custody is irreversible and removes
//! the material a device needs to keep working. The authority for it can
//! therefore never be a flag that the caller supplies about itself. This module
//! derives the erasure authority from state the caller cannot author in its own
//! invocation:
//!
//! * the **subject** is derived from the persisted local endpoint identity and
//!   the custody locator of the current data home;
//! * the **authenticated replacement endpoint** is the persisted peer identity,
//!   accepted only when the locally signed device trust record for that peer
//!   verifies against the subject's own signing key, is unexpired and reports
//!   the `verified` trust state;
//! * the **bounded inventory** is the exact, deduplicated set of custody
//!   artifacts owned by that subject;
//! * the **informed confirmation** is a record supplied by the operator that
//!   has to name the derived subject, the authenticated replacement endpoint,
//!   the inventory digest and every scope entry. It is not a boolean and it
//!   cannot authorize on its own: without the verified trust record the
//!   operation is refused before the confirmation is even read.
//!
//! Anything the confirmation names that is outside the enumerated inventory is
//! refused rather than erased, and the erasure loop only ever walks the
//! authorized inventory.

use super::*;
use crate::core::secure_mesh_trust::{
    DeviceTrustState, device_trust_record_from_json, verify_device_trust_record_json,
};
use crate::domain::mobile_relay::endpoint_trust::{
    local_public_device_identity, mobile_relay_trust_record_now_epoch,
    peer_device_identity_from_state,
};
use std::collections::BTreeSet;

/// Schema of the bounded cleanup inventory document.
pub(in crate::domain::mobile_relay) const CUSTODY_CLEANUP_INVENTORY_SCHEMA: &str =
    "licoup.custody-cleanup-inventory.v1";

/// Schema of the explicit informed confirmation record.
pub(in crate::domain::mobile_relay) const CUSTODY_CLEANUP_CONFIRMATION_SCHEMA: &str =
    "licoup.custody-cleanup-confirmation.v1";

/// The consequence a confirmation has to name verbatim. Naming the consequence
/// is what makes the record informed rather than a reused acceptance flag.
pub(in crate::domain::mobile_relay) const CUSTODY_CLEANUP_CONSEQUENCE: &str =
    "irreversibleLocalSecretErasure";

/// Upper bound on a confirmation scope so a caller cannot force unbounded work.
const MAX_CUSTODY_CLEANUP_SCOPE_ENTRIES: usize = 4096;

const SECRET_HANDLE_SCOPE_PREFIX: &str = "secret-handle:";
const PAIRWISE_STORE_FILE_SCOPE_PREFIX: &str = "pairwise-store-file:";

/// Files a pairwise durable store owns beside its main database file.
const PAIRWISE_STORE_FILE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// The endpoint whose custody a cleanup would erase.
///
/// Every field is derived from persisted state; none of them is accepted from
/// the caller's parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::domain::mobile_relay) struct CustodyCleanupSubject {
    pub(in crate::domain::mobile_relay) endpoint_id: String,
    pub(in crate::domain::mobile_relay) identity_fingerprint: String,
    pub(in crate::domain::mobile_relay) custody_namespace: String,
}

impl CustodyCleanupSubject {
    fn to_json(&self) -> Value {
        json!({
            "endpointId": self.endpoint_id,
            "identityFingerprint": self.identity_fingerprint,
            "custodyNamespace": self.custody_namespace,
        })
    }

    /// Digest input shared by the inventory digest and any independent
    /// recomputation of it. The parts are length-unambiguous because they are
    /// serialized as a JSON array of strings.
    fn digest_parts(&self) -> Vec<String> {
        vec![
            CUSTODY_CLEANUP_INVENTORY_SCHEMA.to_string(),
            self.endpoint_id.clone(),
            self.identity_fingerprint.clone(),
            self.custody_namespace.clone(),
        ]
    }
}

/// The authenticated counterpart that is taking over the subject's role.
#[derive(Clone, Debug)]
pub(in crate::domain::mobile_relay) struct AuthenticatedReplacementEndpoint {
    pub(in crate::domain::mobile_relay) endpoint_id: String,
    pub(in crate::domain::mobile_relay) device_trust_fingerprint: String,
    pub(in crate::domain::mobile_relay) verification_method: String,
}

/// The exact, bounded custody set owned by one subject.
#[derive(Clone, Debug)]
pub(in crate::domain::mobile_relay) struct CustodyCleanupInventory {
    pub(in crate::domain::mobile_relay) subject: CustodyCleanupSubject,
    pub(in crate::domain::mobile_relay) secret_handles: Vec<SecretStoreHandle>,
    pub(in crate::domain::mobile_relay) pairwise_store_files: Vec<PathBuf>,
    pub(in crate::domain::mobile_relay) scope: Vec<String>,
    pub(in crate::domain::mobile_relay) digest: String,
    pub(in crate::domain::mobile_relay) root_secret_handle_count: usize,
    pub(in crate::domain::mobile_relay) pairwise_snapshot_handle_count: usize,
    pub(in crate::domain::mobile_relay) pairwise_database_present: bool,
}

impl CustodyCleanupInventory {
    pub(in crate::domain::mobile_relay) fn to_json(&self) -> Value {
        json!({
            "schemaVersion": CUSTODY_CLEANUP_INVENTORY_SCHEMA,
            "redacted": true,
            "secretValuesIncluded": false,
            "localPathsIncluded": false,
            "subject": self.subject.to_json(),
            "secretHandleCount": self.secret_handles.len(),
            "rootSecretHandleCount": self.root_secret_handle_count,
            "pairwiseSnapshotHandleCount": self.pairwise_snapshot_handle_count,
            "pairwiseDatabasePresent": self.pairwise_database_present,
            "pairwiseStoreFileCount": self.pairwise_store_files.len(),
            "scopeEntryCount": self.scope.len(),
            "scope": self.scope.clone(),
            "inventoryDigest": self.digest.clone(),
        })
    }
}

/// A cleanup that passed every authority check.
pub(in crate::domain::mobile_relay) struct AuthorizedCustodyCleanup {
    pub(in crate::domain::mobile_relay) inventory: CustodyCleanupInventory,
    pub(in crate::domain::mobile_relay) replacement: AuthenticatedReplacementEndpoint,
    pub(in crate::domain::mobile_relay) confirmation_scope_entry_count: usize,
}

/// Derive the cleanup subject from persisted identity and custody state.
pub(in crate::domain::mobile_relay) fn custody_cleanup_subject(
    config: &Value,
) -> Result<CustodyCleanupSubject> {
    let identity = local_public_device_identity(config)?;
    Ok(CustodyCleanupSubject {
        endpoint_id: identity.endpoint_id.clone(),
        identity_fingerprint: identity.fingerprint()?,
        custody_namespace: current_custody_namespace()?,
    })
}

/// Accept a replacement endpoint only against verifiable persisted evidence.
///
/// The evidence is the device trust record the subject itself signed about that
/// peer. Signature, peer identity binding, trust state and validity window are
/// all re-verified here, so a persisted `peerVerified` flag or any other caller
/// supplied claim never authenticates a replacement endpoint.
pub(in crate::domain::mobile_relay) fn authenticated_replacement_endpoint(
    config: &Value,
) -> Result<AuthenticatedReplacementEndpoint> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let state = config
        .get("mobileRelayE2ee")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("mobile relay local endpoint state is missing"))?;
    let local_identity = local_public_device_identity(config)?;
    let peer_identity = peer_device_identity_from_state(state)?;
    ensure!(
        peer_identity.endpoint_id != local_identity.endpoint_id,
        "mobile relay replacement endpoint must differ from the custody subject"
    );
    let trust_record = state
        .get("peerTrustRecord")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("mobile relay replacement endpoint trust record is missing"))?;
    let record = device_trust_record_from_json(trust_record)?;
    let trust_state = verify_device_trust_record_json(
        &local_identity,
        &peer_identity,
        trust_record,
        mobile_relay_trust_record_now_epoch()?,
    )?;
    ensure!(
        trust_state == DeviceTrustState::Verified,
        "mobile relay replacement endpoint trust record is not verified"
    );
    let device_trust_fingerprint = peer_identity.fingerprint()?;
    ensure!(
        record.peer_fingerprint == device_trust_fingerprint,
        "mobile relay replacement endpoint trust record fingerprint mismatch"
    );
    Ok(AuthenticatedReplacementEndpoint {
        endpoint_id: peer_identity.endpoint_id.clone(),
        device_trust_fingerprint,
        verification_method: record.verification_method,
    })
}

/// Enumerate the bounded custody inventory owned by the current subject.
pub(in crate::domain::mobile_relay) fn enumerate_custody_cleanup_inventory(
    config: &Value,
) -> Result<CustodyCleanupInventory> {
    ensure_secure_mesh_protected_operation_allowed()?;
    if let Some(origin) = recorded_custody_namespace(config)? {
        ensure!(
            origin == current_custody_namespace()?,
            "relocated custody requires its owning cleanup authority"
        );
    }
    let subject = custody_cleanup_subject(config)?;

    let pairwise_path = mobile_relay_pairwise_store_path()?;
    let pairwise_database_present = pairwise_path.exists();
    let pairwise_handles = if pairwise_database_present {
        let store = mobile_relay_pairwise_store()?;
        let handles = store.referenced_secret_snapshot_handles()?;
        drop(store);
        handles
    } else {
        Vec::new()
    };

    let mut secret_handles =
        custody_cleanup_root_secret_handles(config, &subject.custody_namespace)?;
    let root_secret_handle_count = secret_handles.len();
    let pairwise_snapshot_handle_count = pairwise_handles.len();
    secret_handles.extend(pairwise_handles);
    secret_handles.sort_by(|left, right| {
        left.namespace()
            .cmp(right.namespace())
            .then_with(|| left.key().cmp(right.key()))
    });
    secret_handles.dedup();

    // The authorized file scope is the fixed set of files the pairwise durable
    // store owns, whether or not each one currently exists, so the inventory
    // digest does not change with transient write-ahead-log presence.
    let pairwise_store_files = pairwise_store_file_candidates(&pairwise_path);

    let scope = custody_cleanup_scope(&secret_handles, &pairwise_store_files)?;
    let digest = custody_cleanup_inventory_digest(&subject, &scope)?;
    Ok(CustodyCleanupInventory {
        subject,
        secret_handles,
        pairwise_store_files,
        scope,
        digest,
        root_secret_handle_count,
        pairwise_snapshot_handle_count,
        pairwise_database_present,
    })
}

/// Read-only projection of the bounded inventory and the confirmation contract.
pub(in crate::domain::mobile_relay) fn custody_cleanup_inventory_report() -> Result<Value> {
    let config = load_config_for_custody_cleanup()?;
    let inventory = enumerate_custody_cleanup_inventory(&config)?;
    Ok(json!({
        "ok": true,
        "status": "inventory",
        "requiresAuthenticatedReplacementEndpoint": true,
        "requiresExplicitInformedConfirmation": true,
        "confirmationSchemaVersion": CUSTODY_CLEANUP_CONFIRMATION_SCHEMA,
        "confirmationConsequence": CUSTODY_CLEANUP_CONSEQUENCE,
        "inventory": inventory.to_json(),
    }))
}

/// The single admission point for custody cleanup.
///
/// The authenticated replacement endpoint is required first, so a missing or
/// unverifiable trust record always refuses the operation regardless of what
/// the caller put in the confirmation.
pub(in crate::domain::mobile_relay) fn authorize_custody_cleanup(
    config: &Value,
    params: &Value,
) -> Result<AuthorizedCustodyCleanup> {
    let replacement = authenticated_replacement_endpoint(config)
        .context("custody cleanup requires an authenticated replacement endpoint")?;
    let inventory = enumerate_custody_cleanup_inventory(config)?;
    let confirmation = validate_custody_cleanup_confirmation(
        params,
        &inventory.subject,
        &replacement,
        &inventory,
    )?;
    Ok(AuthorizedCustodyCleanup {
        inventory,
        replacement,
        confirmation_scope_entry_count: confirmation.scope_entry_count,
    })
}

struct CustodyCleanupConfirmation {
    scope_entry_count: usize,
}

fn validate_custody_cleanup_confirmation(
    params: &Value,
    subject: &CustodyCleanupSubject,
    replacement: &AuthenticatedReplacementEndpoint,
    inventory: &CustodyCleanupInventory,
) -> Result<CustodyCleanupConfirmation> {
    let confirmation = params
        .get("cleanupConfirmation")
        .or_else(|| params.get("cleanup-confirmation"))
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            anyhow!("custody cleanup requires an explicit informed confirmation record")
        })?;
    let object = confirmation
        .as_object()
        .ok_or_else(|| anyhow!("custody cleanup confirmation must be an object"))?;
    ensure!(
        object.get("schemaVersion").and_then(Value::as_str)
            == Some(CUSTODY_CLEANUP_CONFIRMATION_SCHEMA),
        "custody cleanup confirmation schema is unsupported"
    );
    ensure!(
        object.get("consequence").and_then(Value::as_str) == Some(CUSTODY_CLEANUP_CONSEQUENCE),
        "custody cleanup confirmation must name the irreversible local secret erasure consequence"
    );
    ensure!(
        confirmation_text(object, "subjectEndpointId")? == subject.endpoint_id
            && confirmation_text(object, "subjectIdentityFingerprint")?
                == subject.identity_fingerprint
            && confirmation_text(object, "custodyNamespace")? == subject.custody_namespace,
        "custody cleanup confirmation does not match the derived custody subject"
    );
    ensure!(
        confirmation_text(object, "replacementEndpointId")? == replacement.endpoint_id
            && confirmation_text(object, "replacementDeviceTrustFingerprint")?
                == replacement.device_trust_fingerprint,
        "custody cleanup confirmation does not name the authenticated replacement endpoint"
    );
    ensure!(
        confirmation_text(object, "inventoryDigest")? == inventory.digest,
        "custody cleanup confirmation inventory digest does not match the enumerated inventory"
    );

    let declared = object
        .get("scope")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("custody cleanup confirmation scope is required"))?;
    ensure!(
        declared.len() <= MAX_CUSTODY_CLEANUP_SCOPE_ENTRIES,
        "custody cleanup confirmation scope exceeds its bound"
    );
    let authorized: BTreeSet<&str> = inventory.scope.iter().map(String::as_str).collect();
    let mut declared_scope: BTreeSet<&str> = BTreeSet::new();
    for entry in declared {
        let entry = entry
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("custody cleanup confirmation scope entry is invalid"))?;
        ensure!(
            authorized.contains(entry),
            "custody cleanup confirmation scope is outside the bounded custody inventory: {entry}"
        );
        ensure!(
            declared_scope.insert(entry),
            "custody cleanup confirmation scope repeats an entry"
        );
    }
    ensure!(
        declared_scope.len() == authorized.len(),
        "custody cleanup confirmation does not cover the enumerated custody inventory"
    );
    Ok(CustodyCleanupConfirmation {
        scope_entry_count: declared_scope.len(),
    })
}

fn confirmation_text(object: &Map<String, Value>, key: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("custody cleanup confirmation field {key} is required"))
}

fn custody_cleanup_scope(
    handles: &[SecretStoreHandle],
    pairwise_store_files: &[PathBuf],
) -> Result<Vec<String>> {
    let mut scope = Vec::with_capacity(handles.len() + pairwise_store_files.len());
    for handle in handles {
        scope.push(format!(
            "{SECRET_HANDLE_SCOPE_PREFIX}{}:{}",
            handle.namespace(),
            handle.key()
        ));
    }
    for path in pairwise_store_files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("custody cleanup pairwise store file name is invalid"))?;
        scope.push(format!("{PAIRWISE_STORE_FILE_SCOPE_PREFIX}{name}"));
    }
    ensure!(
        !scope.is_empty(),
        "custody cleanup has no bounded secret-store inventory"
    );
    ensure!(
        scope.len() <= MAX_CUSTODY_CLEANUP_SCOPE_ENTRIES,
        "custody cleanup inventory exceeds its bound"
    );
    Ok(scope)
}

fn custody_cleanup_inventory_digest(
    subject: &CustodyCleanupSubject,
    scope: &[String],
) -> Result<String> {
    let mut parts = subject.digest_parts();
    parts.extend(scope.iter().cloned());
    Ok(sha256_hex(serde_json::to_string(&parts)?.as_bytes()))
}

/// The pairwise durable store path plus the side files it owns. Enumerated
/// unconditionally; absence is a runtime fact, not part of the authorized scope.
pub(in crate::domain::mobile_relay) fn pairwise_store_file_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf()];
    for suffix in PAIRWISE_STORE_FILE_SUFFIXES {
        let mut candidate = path.as_os_str().to_os_string();
        candidate.push(suffix);
        candidates.push(PathBuf::from(candidate));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custody_cleanup_inventory_digest_binds_subject_and_scope_order() {
        let subject = CustodyCleanupSubject {
            endpoint_id: "endpoint-fixture".to_string(),
            identity_fingerprint: "sha256:fixture".to_string(),
            custody_namespace: MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE.to_string(),
        };
        let scope = vec![
            "secret-handle:fixture:pcToken".to_string(),
            "pairwise-store-file:pairwise-pqxdh.sqlite3".to_string(),
        ];
        let digest = custody_cleanup_inventory_digest(&subject, &scope).unwrap();
        assert_eq!(digest.len(), 64);
        assert_eq!(
            digest,
            custody_cleanup_inventory_digest(&subject, &scope).unwrap()
        );
        let mut other_subject = subject.clone();
        other_subject.endpoint_id = "endpoint-other".to_string();
        assert_ne!(
            digest,
            custody_cleanup_inventory_digest(&other_subject, &scope).unwrap()
        );
        let mut reordered = scope.clone();
        reordered.reverse();
        assert_ne!(
            digest,
            custody_cleanup_inventory_digest(&subject, &reordered).unwrap()
        );
    }
}
