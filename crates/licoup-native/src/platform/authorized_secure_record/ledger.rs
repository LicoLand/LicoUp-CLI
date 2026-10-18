use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::authorized_secure_record::{SecureRecordLocator, VersionedSecureRecord};

const ENTRY_SCHEMA: &str = "licoup.authorized-secure-ledger-entry.v1";
const MAX_GENERATIONS: u64 = 4_096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LedgerEntryKind {
    Record,
    Deleted,
    Consumed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LedgerEntry {
    schema_version: String,
    locator_digest_sha256: String,
    generation: u64,
    previous_entry_digest_sha256: Option<String>,
    kind: LedgerEntryKind,
    record: Option<VersionedSecureRecord>,
    target_record_digest_sha256: String,
    authorization_nonce: String,
    entry_digest_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LedgerHead {
    Missing,
    Active {
        entry: LedgerEntry,
        record: VersionedSecureRecord,
    },
    Deleted {
        entry: LedgerEntry,
    },
    Consumed {
        entry: LedgerEntry,
    },
}

impl LedgerEntry {
    pub(super) fn record(
        locator: &SecureRecordLocator,
        record: VersionedSecureRecord,
        previous_entry_digest_sha256: Option<String>,
        authorization_nonce: &str,
    ) -> Result<Self> {
        record.validate()?;
        let mut entry = Self {
            schema_version: ENTRY_SCHEMA.to_owned(),
            locator_digest_sha256: locator_digest(locator),
            generation: record.version(),
            previous_entry_digest_sha256,
            kind: LedgerEntryKind::Record,
            target_record_digest_sha256: record.record_digest_sha256().to_owned(),
            record: Some(record),
            authorization_nonce: authorization_nonce.to_owned(),
            entry_digest_sha256: String::new(),
        };
        entry.entry_digest_sha256 = entry.expected_digest()?;
        entry.validate(locator)?;
        Ok(entry)
    }

    pub(super) fn tombstone(
        locator: &SecureRecordLocator,
        kind: LedgerEntryKind,
        expected: &VersionedSecureRecord,
        previous_entry_digest_sha256: String,
        authorization_nonce: &str,
    ) -> Result<Self> {
        ensure!(
            matches!(kind, LedgerEntryKind::Deleted | LedgerEntryKind::Consumed),
            "authorized_secure_record_ledger_tombstone_invalid"
        );
        expected.validate()?;
        let mut entry = Self {
            schema_version: ENTRY_SCHEMA.to_owned(),
            locator_digest_sha256: locator_digest(locator),
            generation: expected.version().saturating_add(1),
            previous_entry_digest_sha256: Some(previous_entry_digest_sha256),
            kind,
            record: None,
            target_record_digest_sha256: expected.record_digest_sha256().to_owned(),
            authorization_nonce: authorization_nonce.to_owned(),
            entry_digest_sha256: String::new(),
        };
        entry.entry_digest_sha256 = entry.expected_digest()?;
        entry.validate(locator)?;
        Ok(entry)
    }

    pub(super) fn validate(&self, locator: &SecureRecordLocator) -> Result<()> {
        ensure!(
            self.schema_version == ENTRY_SCHEMA
                && self.locator_digest_sha256 == locator_digest(locator)
                && (1..=MAX_GENERATIONS).contains(&self.generation)
                && self
                    .previous_entry_digest_sha256
                    .as_deref()
                    .is_none_or(is_sha256)
                && is_sha256(&self.target_record_digest_sha256)
                && uuid::Uuid::parse_str(&self.authorization_nonce)
                    .is_ok_and(|value| value.to_string() == self.authorization_nonce)
                && self.entry_digest_sha256 == self.expected_digest()?,
            "authorized_secure_record_ledger_entry_invalid"
        );
        match (&self.kind, &self.record) {
            (LedgerEntryKind::Record, Some(record)) => {
                record.validate()?;
                ensure!(
                    record.version() == self.generation
                        && record.record_digest_sha256() == self.target_record_digest_sha256,
                    "authorized_secure_record_ledger_record_invalid"
                );
            }
            (LedgerEntryKind::Deleted | LedgerEntryKind::Consumed, None) => {
                ensure!(
                    self.generation > 1 && self.previous_entry_digest_sha256.is_some(),
                    "authorized_secure_record_ledger_tombstone_invalid"
                );
            }
            _ => return Err(anyhow!("authorized_secure_record_ledger_entry_invalid")),
        }
        Ok(())
    }

    pub(super) fn from_json(locator: &SecureRecordLocator, value: &str) -> Result<Self> {
        let entry: Self = serde_json::from_str(value)
            .map_err(|_| anyhow!("authorized_secure_record_ledger_entry_invalid"))?;
        entry.validate(locator)?;
        Ok(entry)
    }

    pub(super) fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn entry_digest_sha256(&self) -> &str {
        &self.entry_digest_sha256
    }

    pub(super) fn target_record_digest_sha256(&self) -> &str {
        &self.target_record_digest_sha256
    }

    pub(super) fn kind(&self) -> LedgerEntryKind {
        self.kind
    }

    pub(super) fn record_value(&self) -> Option<&VersionedSecureRecord> {
        self.record.as_ref()
    }

    fn expected_digest(&self) -> Result<String> {
        let canonical = serde_json::to_vec(&(
            &self.schema_version,
            &self.locator_digest_sha256,
            self.generation,
            &self.previous_entry_digest_sha256,
            self.kind,
            &self.record,
            &self.target_record_digest_sha256,
            &self.authorization_nonce,
        ))?;
        let mut hasher = Sha256::new();
        hasher.update(b"LICOUP-AUTHORIZED-SECURE-LEDGER-ENTRY-V1\0");
        hasher.update((canonical.len() as u64).to_be_bytes());
        hasher.update(canonical);
        Ok(format!("{:x}", hasher.finalize()))
    }
}

impl LedgerHead {
    pub(super) fn active(&self) -> Result<(&LedgerEntry, &VersionedSecureRecord)> {
        match self {
            Self::Active { entry, record } => Ok((entry, record)),
            Self::Missing => Err(anyhow!("authorized_secure_record_missing")),
            Self::Deleted { .. } => Err(anyhow!("authorized_secure_record_deleted")),
            Self::Consumed { .. } => Err(anyhow!("authorized_secure_record_consumed")),
        }
    }
}

pub(super) fn reduce(
    locator: &SecureRecordLocator,
    entries: impl IntoIterator<Item = LedgerEntry>,
) -> Result<LedgerHead> {
    let mut head = LedgerHead::Missing;
    let mut previous_entry: Option<LedgerEntry> = None;
    let mut previous_record: Option<VersionedSecureRecord> = None;
    for entry in entries {
        entry.validate(locator)?;
        let expected_generation = previous_entry
            .as_ref()
            .map_or(1, |value| value.generation().saturating_add(1));
        ensure!(
            entry.generation() == expected_generation
                && entry.previous_entry_digest_sha256.as_deref()
                    == previous_entry
                        .as_ref()
                        .map(LedgerEntry::entry_digest_sha256),
            "authorized_secure_record_ledger_chain_invalid"
        );
        match entry.kind() {
            LedgerEntryKind::Record => {
                ensure!(
                    !matches!(
                        head,
                        LedgerHead::Deleted { .. } | LedgerHead::Consumed { .. }
                    ),
                    "authorized_secure_record_ledger_terminal_replay"
                );
                let record = entry
                    .record_value()
                    .cloned()
                    .ok_or_else(|| anyhow!("authorized_secure_record_ledger_record_invalid"))?;
                ensure!(
                    record.previous_record_digest_sha256()
                        == previous_record
                            .as_ref()
                            .map(VersionedSecureRecord::record_digest_sha256),
                    "authorized_secure_record_ledger_record_chain_invalid"
                );
                previous_record = Some(record.clone());
                head = LedgerHead::Active {
                    entry: entry.clone(),
                    record,
                };
            }
            LedgerEntryKind::Deleted | LedgerEntryKind::Consumed => {
                let expected = previous_record
                    .as_ref()
                    .ok_or_else(|| anyhow!("authorized_secure_record_ledger_tombstone_invalid"))?;
                ensure!(
                    entry.target_record_digest_sha256() == expected.record_digest_sha256(),
                    "authorized_secure_record_ledger_tombstone_invalid"
                );
                head = if entry.kind() == LedgerEntryKind::Deleted {
                    LedgerHead::Deleted {
                        entry: entry.clone(),
                    }
                } else {
                    LedgerHead::Consumed {
                        entry: entry.clone(),
                    }
                };
            }
        }
        previous_entry = Some(entry);
    }
    Ok(head)
}

pub(super) fn locator_digest(locator: &SecureRecordLocator) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"LICOUP-AUTHORIZED-SECURE-LEDGER-LOCATOR-V1\0");
    for value in [locator.namespace(), locator.key()] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator() -> SecureRecordLocator {
        SecureRecordLocator::new("test-authority", "profile-default").unwrap()
    }

    #[test]
    fn terminal_tombstone_rejects_old_generation_replay() {
        let locator = locator();
        let first = VersionedSecureRecord::new(1, None, "one".into()).unwrap();
        let first_entry = LedgerEntry::record(
            &locator,
            first.clone(),
            None,
            "11111111-1111-4111-8111-111111111111",
        )
        .unwrap();
        let tombstone = LedgerEntry::tombstone(
            &locator,
            LedgerEntryKind::Consumed,
            &first,
            first_entry.entry_digest_sha256().to_owned(),
            "22222222-2222-4222-8222-222222222222",
        )
        .unwrap();
        let replay = VersionedSecureRecord::new(
            3,
            Some(first.record_digest_sha256().to_owned()),
            "replay".into(),
        )
        .unwrap();
        let replay_entry = LedgerEntry::record(
            &locator,
            replay,
            Some(tombstone.entry_digest_sha256().to_owned()),
            "33333333-3333-4333-8333-333333333333",
        )
        .unwrap();
        assert!(reduce(&locator, [first_entry, tombstone, replay_entry]).is_err());
    }
}
