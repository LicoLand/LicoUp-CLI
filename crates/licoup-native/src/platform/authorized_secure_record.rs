//! Platform adapter for the generic authorized secure-record authority.
//!
//! macOS uses an append-only Data Protection Keychain ledger. Every ledger
//! generation is installed with `SecItemAdd`, so concurrent writers cannot
//! overwrite one another. Delete and one-shot consumption append terminal
//! tombstones; old generations are never made authoritative again.

mod ledger;
#[cfg(target_os = "macos")]
mod macos_keychain;

use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};

use crate::core::authorized_secure_record::{
    AuthorizedSecureRecordGrant, AuthorizedSecureRecordStore, SecureRecordAuthorizationRequest,
    SecureRecordLocator, SecureRecordOperation, VersionedSecureRecord,
};
use ledger::{LedgerEntry, LedgerEntryKind, LedgerHead};

const BACKEND: &str = "platform-user-presence-append-only-ledger-v1";
const MAX_GENERATIONS: u64 = 4_096;

pub fn store() -> Arc<dyn AuthorizedSecureRecordStore> {
    #[cfg(test)]
    {
        return test_store();
    }
    #[cfg(not(test))]
    {
        static STORE: OnceLock<Arc<PlatformAuthorizedSecureRecordStore>> = OnceLock::new();
        STORE
            .get_or_init(|| Arc::new(PlatformAuthorizedSecureRecordStore::new()))
            .clone()
    }
}

pub struct PlatformAuthorizedSecureRecordStore {
    #[cfg(target_os = "macos")]
    keychain: Option<macos_keychain::MacosKeychainLedger>,
}

impl PlatformAuthorizedSecureRecordStore {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            keychain: macos_keychain::MacosKeychainLedger::new().ok(),
        }
    }

    fn session<'a>(
        &self,
        grant: &'a AuthorizedSecureRecordGrant,
    ) -> Result<&'a crate::platform::user_presence::UserPresenceSession> {
        grant.platform_context()
    }

    fn scan(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        repair_incomplete: bool,
    ) -> Result<LedgerHead> {
        #[cfg(target_os = "macos")]
        {
            let keychain = self
                .keychain
                .as_ref()
                .ok_or_else(|| anyhow!("authorized_secure_record_user_presence_unavailable"))?;
            let session = self.session(grant)?;
            let mut entries = Vec::new();
            for generation in 1..=MAX_GENERATIONS {
                let Some(loaded) =
                    keychain.load_generation(session, locator, generation, repair_incomplete)?
                else {
                    return ledger::reduce(locator, entries);
                };
                ensure!(
                    loaded.proof_complete,
                    "authorized_secure_record_ledger_recovery_required"
                );
                entries.push(loaded.entry);
            }
            ensure!(
                keychain
                    .load_generation(session, locator, MAX_GENERATIONS.saturating_add(1), false,)?
                    .is_none(),
                "authorized_secure_record_ledger_generation_limit"
            );
            ledger::reduce(locator, entries)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (grant, locator, repair_incomplete);
            Err(anyhow!(
                "authorized_secure_record_user_presence_unavailable"
            ))
        }
    }

    fn append(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        entry: &LedgerEntry,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.keychain
                .as_ref()
                .ok_or_else(|| anyhow!("authorized_secure_record_user_presence_unavailable"))?
                .append(self.session(grant)?, locator, entry)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (grant, locator, entry);
            Err(anyhow!(
                "authorized_secure_record_user_presence_unavailable"
            ))
        }
    }
}

impl Default for PlatformAuthorizedSecureRecordStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthorizedSecureRecordStore for PlatformAuthorizedSecureRecordStore {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn user_presence_available(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.keychain.is_some() && macos_keychain::MacosKeychainLedger::available()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn authorize(
        &self,
        request: SecureRecordAuthorizationRequest,
    ) -> Result<AuthorizedSecureRecordGrant> {
        request.validate()?;
        ensure!(
            self.user_presence_available(),
            "authorized_secure_record_user_presence_unavailable"
        );
        let scope_digest = authorization_scope_digest(&request);
        let reason = format!("{} [{}]", request.reason, request.operation.as_str());
        let session = crate::platform::user_presence::authorize(&reason, &scope_digest)?;
        AuthorizedSecureRecordGrant::issue(request, BACKEND, session)
    }

    fn read(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected_version: u64,
        expected_digest_sha256: &str,
    ) -> Result<VersionedSecureRecord> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::Read,
            expected_digest_sha256,
            expected_version,
            Some(expected_digest_sha256),
        )?;
        let head = self.scan(grant, locator, false)?;
        let (_, record) = head.active()?;
        ensure_exact(record, expected_version, expected_digest_sha256)?;
        Ok(record.clone())
    }

    fn read_current(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        recovery_scope_digest_sha256: &str,
    ) -> Result<VersionedSecureRecord> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::RecoverRead,
            recovery_scope_digest_sha256,
            0,
            None,
        )?;
        let head = self.scan(grant, locator, true)?;
        let (_, record) = head.active()?;
        Ok(record.clone())
    }

    fn compare_and_swap(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: Option<&VersionedSecureRecord>,
        replacement: &VersionedSecureRecord,
    ) -> Result<()> {
        replacement.validate()?;
        let operation = if expected.is_some() {
            SecureRecordOperation::Replace
        } else {
            SecureRecordOperation::Create
        };
        grant.claim(
            BACKEND,
            locator,
            operation,
            replacement.record_digest_sha256(),
            expected.map_or(0, VersionedSecureRecord::version),
            expected.map(VersionedSecureRecord::record_digest_sha256),
        )?;
        let head = self.scan(grant, locator, false)?;
        let previous_entry_digest = validate_replacement(&head, expected, replacement)?;
        let entry = LedgerEntry::record(
            locator,
            replacement.clone(),
            previous_entry_digest,
            grant.nonce(),
        )?;
        self.append(grant, locator, &entry)
    }

    fn delete(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: &VersionedSecureRecord,
    ) -> Result<()> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::Delete,
            expected.record_digest_sha256(),
            expected.version(),
            Some(expected.record_digest_sha256()),
        )?;
        let head = self.scan(grant, locator, false)?;
        let (entry, record) = head.active()?;
        ensure_exact(record, expected.version(), expected.record_digest_sha256())?;
        let tombstone = LedgerEntry::tombstone(
            locator,
            LedgerEntryKind::Deleted,
            expected,
            entry.entry_digest_sha256().to_owned(),
            grant.nonce(),
        )?;
        self.append(grant, locator, &tombstone)
    }

    fn consume_one_shot(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: &VersionedSecureRecord,
    ) -> Result<VersionedSecureRecord> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::ConsumeOneShot,
            expected.record_digest_sha256(),
            expected.version(),
            Some(expected.record_digest_sha256()),
        )?;
        let head = self.scan(grant, locator, false)?;
        let (entry, record) = head.active()?;
        ensure_exact(record, expected.version(), expected.record_digest_sha256())?;
        let tombstone = LedgerEntry::tombstone(
            locator,
            LedgerEntryKind::Consumed,
            expected,
            entry.entry_digest_sha256().to_owned(),
            grant.nonce(),
        )?;
        self.append(grant, locator, &tombstone)?;
        Ok(record.clone())
    }
}

fn validate_replacement(
    head: &LedgerHead,
    expected: Option<&VersionedSecureRecord>,
    replacement: &VersionedSecureRecord,
) -> Result<Option<String>> {
    match (head, expected) {
        (LedgerHead::Missing, None) => ensure!(
            replacement.version() == 1 && replacement.previous_record_digest_sha256().is_none(),
            "authorized_secure_record_version_transition_invalid"
        ),
        (LedgerHead::Active { entry, record }, Some(expected)) => {
            ensure_exact(record, expected.version(), expected.record_digest_sha256())?;
            ensure!(
                replacement.version() == expected.version().saturating_add(1)
                    && replacement.previous_record_digest_sha256()
                        == Some(expected.record_digest_sha256()),
                "authorized_secure_record_version_transition_invalid"
            );
            return Ok(Some(entry.entry_digest_sha256().to_owned()));
        }
        _ => return Err(anyhow!("authorized_secure_record_compare_and_swap_failed")),
    }
    Ok(None)
}

fn ensure_exact(record: &VersionedSecureRecord, version: u64, digest: &str) -> Result<()> {
    record.validate()?;
    ensure!(
        record.version() == version && record.record_digest_sha256() == digest,
        "authorized_secure_record_binding_mismatch"
    );
    Ok(())
}

fn authorization_scope_digest(request: &SecureRecordAuthorizationRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"LICOUP-AUTHORIZED-SECURE-RECORD-AUTHORIZATION-SCOPE-V1\0");
    for field in [
        request.locator.namespace(),
        request.locator.key(),
        request.operation.as_str(),
        request.target_digest_sha256.as_str(),
        request
            .expected_prior_digest_sha256
            .as_deref()
            .unwrap_or(""),
        request.nonce.as_str(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.update(request.expected_prior_version.to_be_bytes());
    hasher.update((request.operation_budget as u64).to_be_bytes());
    for (key, value) in &request.scope_bindings {
        for field in [key.as_str(), value.as_str()] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
fn test_store() -> Arc<dyn AuthorizedSecureRecordStore> {
    static STORE: OnceLock<Arc<InMemoryAuthorizedSecureRecordStore>> = OnceLock::new();
    STORE
        .get_or_init(|| Arc::new(InMemoryAuthorizedSecureRecordStore::default()))
        .clone()
}

#[cfg(test)]
#[derive(Default)]
struct InMemoryAuthorizedSecureRecordStore {
    entries: std::sync::Mutex<std::collections::BTreeMap<SecureRecordLocator, Vec<LedgerEntry>>>,
}

#[cfg(test)]
impl InMemoryAuthorizedSecureRecordStore {
    fn head(&self, locator: &SecureRecordLocator) -> Result<LedgerHead> {
        let entries = self.entries.lock().unwrap();
        ledger::reduce(locator, entries.get(locator).cloned().unwrap_or_default())
    }

    fn append_entry(&self, locator: &SecureRecordLocator, entry: LedgerEntry) -> Result<()> {
        let mut entries = self.entries.lock().unwrap();
        let values = entries.entry(locator.clone()).or_default();
        ensure!(
            values.len() as u64 + 1 == entry.generation(),
            "authorized_secure_record_compare_and_swap_failed"
        );
        values.push(entry);
        Ok(())
    }
}

#[cfg(test)]
impl AuthorizedSecureRecordStore for InMemoryAuthorizedSecureRecordStore {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn user_presence_available(&self) -> bool {
        true
    }

    fn authorize(
        &self,
        request: SecureRecordAuthorizationRequest,
    ) -> Result<AuthorizedSecureRecordGrant> {
        AuthorizedSecureRecordGrant::issue(request, BACKEND, ())
    }

    fn read(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected_version: u64,
        expected_digest_sha256: &str,
    ) -> Result<VersionedSecureRecord> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::Read,
            expected_digest_sha256,
            expected_version,
            Some(expected_digest_sha256),
        )?;
        let head = self.head(locator)?;
        let (_, record) = head.active()?;
        ensure_exact(record, expected_version, expected_digest_sha256)?;
        Ok(record.clone())
    }

    fn read_current(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        recovery_scope_digest_sha256: &str,
    ) -> Result<VersionedSecureRecord> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::RecoverRead,
            recovery_scope_digest_sha256,
            0,
            None,
        )?;
        let head = self.head(locator)?;
        let (_, record) = head.active()?;
        Ok(record.clone())
    }

    fn compare_and_swap(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: Option<&VersionedSecureRecord>,
        replacement: &VersionedSecureRecord,
    ) -> Result<()> {
        let operation = if expected.is_some() {
            SecureRecordOperation::Replace
        } else {
            SecureRecordOperation::Create
        };
        grant.claim(
            BACKEND,
            locator,
            operation,
            replacement.record_digest_sha256(),
            expected.map_or(0, VersionedSecureRecord::version),
            expected.map(VersionedSecureRecord::record_digest_sha256),
        )?;
        let previous = validate_replacement(&self.head(locator)?, expected, replacement)?;
        self.append_entry(
            locator,
            LedgerEntry::record(locator, replacement.clone(), previous, grant.nonce())?,
        )
    }

    fn delete(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: &VersionedSecureRecord,
    ) -> Result<()> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::Delete,
            expected.record_digest_sha256(),
            expected.version(),
            Some(expected.record_digest_sha256()),
        )?;
        let head = self.head(locator)?;
        let (entry, record) = head.active()?;
        ensure_exact(record, expected.version(), expected.record_digest_sha256())?;
        self.append_entry(
            locator,
            LedgerEntry::tombstone(
                locator,
                LedgerEntryKind::Deleted,
                expected,
                entry.entry_digest_sha256().to_owned(),
                grant.nonce(),
            )?,
        )
    }

    fn consume_one_shot(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: &VersionedSecureRecord,
    ) -> Result<VersionedSecureRecord> {
        grant.claim(
            BACKEND,
            locator,
            SecureRecordOperation::ConsumeOneShot,
            expected.record_digest_sha256(),
            expected.version(),
            Some(expected.record_digest_sha256()),
        )?;
        let head = self.head(locator)?;
        let (entry, record) = head.active()?;
        ensure_exact(record, expected.version(), expected.record_digest_sha256())?;
        let record = record.clone();
        self.append_entry(
            locator,
            LedgerEntry::tombstone(
                locator,
                LedgerEntryKind::Consumed,
                expected,
                entry.entry_digest_sha256().to_owned(),
                grant.nonce(),
            )?,
        )?;
        Ok(record)
    }
}
