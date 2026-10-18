//! What a cleanup is allowed to touch.
//!
//! A cleanup names exactly one subject, one revoked device and one approved
//! operation, and it carries the frozen inventory of LicoUp-owned entries that
//! the operation may remove. Everything else — another subject's root, an
//! external project reference, a symbolic link into unrelated data — is
//! outside the target by construction.
//!
//! The inventory grammar itself is not restated here. [`CleanupInventory::freeze`]
//! admits only entries that already passed the data-root inventory owner in
//! `licoup-foundation`; the checks in this module are the *effect-time* guard
//! that keeps a caller-named reference from escaping its root, which is why
//! they are applied again immediately before a removal.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Longest accepted subject, device or operation spelling.
pub const MAX_CLEANUP_IDENTIFIER_BYTES: usize = 256;
/// Most entries one frozen inventory may hold.
pub const MAX_CLEANUP_INVENTORY_ENTRIES: usize = 4_096;
/// The one layout tag a frozen inventory declares.
pub const CLEANUP_INVENTORY_LAYOUT: &str = "licoup.endpoint-cleanup-inventory/v1";

fn bounded_identifier(value: &str, code: &'static str) -> Result<String> {
    let trimmed = value.trim();
    ensure!(!trimmed.is_empty(), "{}", code);
    ensure!(trimmed.len() <= MAX_CLEANUP_IDENTIFIER_BYTES, "{}", code);
    ensure!(!trimmed.chars().any(char::is_control), "{}", code);
    Ok(trimmed.to_string())
}

macro_rules! cleanup_identifier {
    ($name:ident, $code:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validate and normalize one spelling of this identifier.
            pub fn new(value: impl Into<String>) -> Result<Self> {
                Ok(Self(bounded_identifier(&value.into(), $code)?))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

cleanup_identifier!(
    CleanupSubject,
    "cleanup_subject_invalid",
    "The authenticated subject that owns the endpoint being cleaned."
);
cleanup_identifier!(
    DeviceId,
    "cleanup_device_invalid",
    "The revoked endpoint whose LicoUp-owned data is being removed."
);

/// The approved cleanup operation.
///
/// The identifier is a fresh UUID minted once per approved confirmation; a
/// repeated operation identifier is how a replayed request is recognized.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = bounded_identifier(&value.into(), "cleanup_operation_invalid")?;
        ensure!(
            uuid::Uuid::parse_str(&value).is_ok(),
            "cleanup_operation_invalid"
        );
        Ok(Self(value))
    }

    /// Mint the operation identifier for one approved cleanup.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Which frozen entry a stage is acting on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupInventoryKind {
    File,
    Directory,
}

/// One LicoUp-owned entry the cleanup may remove.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct CleanupInventoryEntry {
    path: String,
    kind: CleanupInventoryKind,
    #[serde(default)]
    size: u64,
}

impl CleanupInventoryEntry {
    pub fn file(path: impl Into<String>, size: u64) -> Result<Self> {
        Self::new(path.into(), CleanupInventoryKind::File, size)
    }

    pub fn directory(path: impl Into<String>) -> Result<Self> {
        Self::new(path.into(), CleanupInventoryKind::Directory, 0)
    }

    fn new(path: String, kind: CleanupInventoryKind, size: u64) -> Result<Self> {
        ensure!(
            canonical_relative_posix(&path),
            "cleanup_inventory_path_not_portable"
        );
        ensure!(
            kind == CleanupInventoryKind::File || size == 0,
            "cleanup_inventory_size_invalid"
        );
        Ok(Self { path, kind, size })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn kind(&self) -> CleanupInventoryKind {
        self.kind
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

/// Whether a spelling is a canonical, host-independent POSIX relative path.
///
/// The rule matches the data-root inventory owner's own grammar so a frozen
/// cleanup inventory can only name locations its root can actually contain.
pub fn canonical_relative_posix(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') {
        return false;
    }
    path.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.contains('\\')
            && !part.contains(':')
            && !part.chars().any(char::is_control)
    })
}

/// The authority triple one cleanup is bound to.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct CleanupTarget {
    subject: CleanupSubject,
    device: DeviceId,
    operation: OperationId,
}

impl CleanupTarget {
    pub fn new(subject: CleanupSubject, device: DeviceId, operation: OperationId) -> Self {
        Self {
            subject,
            device,
            operation,
        }
    }

    pub fn subject(&self) -> &CleanupSubject {
        &self.subject
    }

    pub fn device(&self) -> &DeviceId {
        &self.device
    }

    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    /// Stable spelling used both in the journal and in a receipt.
    pub fn binding(&self) -> String {
        format!(
            "{}|{}|{}",
            self.subject.as_str(),
            self.device.as_str(),
            self.operation.as_str()
        )
    }
}

/// The frozen set of entries one approved cleanup may remove.
#[derive(Clone, Debug)]
pub struct CleanupInventory {
    layout: &'static str,
    target: CleanupTarget,
    entries: Vec<CleanupInventoryEntry>,
    digest: String,
    total_file_bytes: u64,
}

impl CleanupInventory {
    /// Freeze an already-admitted entry set for one target.
    ///
    /// Freezing is the admission decision, not a discovery step: entries come
    /// from the data-root inventory owner, and this owner refuses duplicates,
    /// case collisions, structural conflicts, a file that is an ancestor of
    /// another entry, and any inventory larger than the bound. A refused
    /// freeze removes nothing and records nothing.
    pub fn freeze(
        target: CleanupTarget,
        entries: impl IntoIterator<Item = CleanupInventoryEntry>,
    ) -> Result<Self> {
        let mut entries: Vec<CleanupInventoryEntry> = entries.into_iter().collect();
        ensure!(
            entries.len() <= MAX_CLEANUP_INVENTORY_ENTRIES,
            "cleanup_inventory_capacity_exceeded"
        );
        entries.sort();
        for pair in entries.windows(2) {
            ensure!(
                pair[0].path() != pair[1].path(),
                "cleanup_inventory_duplicate"
            );
            ensure!(
                pair[0].path().to_lowercase() != pair[1].path().to_lowercase(),
                "cleanup_inventory_case_collision"
            );
        }
        let by_path: BTreeMap<&str, CleanupInventoryKind> = entries
            .iter()
            .map(|entry| (entry.path(), entry.kind()))
            .collect();
        for entry in &entries {
            // A directory may hold other declared members; only a *file* that is
            // an ancestor of another declared member is structurally impossible.
            if entry.kind() != CleanupInventoryKind::File {
                continue;
            }
            let mut prefix = String::from(entry.path());
            prefix.push('/');
            let position = entries.partition_point(|candidate| candidate.path() < prefix.as_str());
            if let Some(nested) = entries.get(position) {
                ensure!(
                    !nested.path().starts_with(&prefix),
                    "cleanup_inventory_conflict"
                );
            }
            ensure!(
                by_path.get(entry.path()) == Some(&entry.kind()),
                "cleanup_inventory_conflict"
            );
        }
        let total_file_bytes = entries
            .iter()
            .filter(|entry| entry.kind() == CleanupInventoryKind::File)
            .try_fold(0_u64, |total, entry| total.checked_add(entry.size()))
            .ok_or_else(|| anyhow!("cleanup_inventory_size_invalid"))?;
        let digest = inventory_digest(&target, &entries);
        Ok(Self {
            layout: CLEANUP_INVENTORY_LAYOUT,
            target,
            entries,
            digest,
            total_file_bytes,
        })
    }

    pub fn layout(&self) -> &'static str {
        self.layout
    }

    pub fn target(&self) -> &CleanupTarget {
        &self.target
    }

    pub fn entries(&self) -> &[CleanupInventoryEntry] {
        &self.entries
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn total_file_bytes(&self) -> u64 {
        self.total_file_bytes
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind() == CleanupInventoryKind::File)
            .count()
    }

    /// Whether this inventory may be used for exactly this target.
    pub fn admits(&self, target: &CleanupTarget) -> bool {
        self.target == *target
    }

    /// Whether the inventory names this exact entry.
    pub fn names(&self, entry: &CleanupInventoryEntry) -> bool {
        self.entries.binary_search(entry).is_ok()
    }
}

fn inventory_digest(target: &CleanupTarget, entries: &[CleanupInventoryEntry]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CLEANUP_INVENTORY_LAYOUT.as_bytes());
    hasher.update([0]);
    hasher.update(target.binding().as_bytes());
    for entry in entries {
        hasher.update([0]);
        hasher.update(entry.path().as_bytes());
        hasher.update([0]);
        hasher.update(match entry.kind() {
            CleanupInventoryKind::File => b"file".as_slice(),
            CleanupInventoryKind::Directory => b"directory".as_slice(),
        });
        hasher.update([0]);
        hasher.update(entry.size().to_be_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> CleanupTarget {
        CleanupTarget::new(
            CleanupSubject::new("subject-a").unwrap(),
            DeviceId::new("device-a").unwrap(),
            OperationId::generate(),
        )
    }

    #[test]
    fn non_portable_paths_are_refused_by_an_entry() {
        for path in [
            "/absolute",
            "a/../b",
            "..",
            "a//b",
            "a/",
            "a\\b",
            "C:\\evil",
            "a\u{0}b",
            "",
            ".",
        ] {
            assert!(
                CleanupInventoryEntry::file(path, 1).is_err(),
                "{path:?} must be refused"
            );
        }
    }

    #[test]
    fn freeze_refuses_duplicates_case_collisions_and_structural_conflicts() {
        let duplicate = CleanupInventory::freeze(
            target(),
            [
                CleanupInventoryEntry::file("a.txt", 1).unwrap(),
                CleanupInventoryEntry::file("a.txt", 1).unwrap(),
            ],
        );
        assert!(duplicate.unwrap_err().to_string().contains("duplicate"));

        let collision = CleanupInventory::freeze(
            target(),
            [
                CleanupInventoryEntry::file("A.txt", 1).unwrap(),
                CleanupInventoryEntry::file("a.txt", 1).unwrap(),
            ],
        );
        assert!(
            collision
                .unwrap_err()
                .to_string()
                .contains("case_collision")
        );

        let conflict = CleanupInventory::freeze(
            target(),
            [
                CleanupInventoryEntry::file("a", 1).unwrap(),
                CleanupInventoryEntry::file("a/b", 1).unwrap(),
            ],
        );
        assert!(conflict.unwrap_err().to_string().contains("conflict"));

        let nested = CleanupInventory::freeze(
            target(),
            [
                CleanupInventoryEntry::directory("a").unwrap(),
                CleanupInventoryEntry::file("a/b.bin", 7).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(nested.file_count(), 1);
        assert_eq!(nested.total_file_bytes(), 7);
    }

    #[test]
    fn a_frozen_inventory_admits_only_its_own_target_and_entries() {
        let target = target();
        let inventory = CleanupInventory::freeze(
            target.clone(),
            [CleanupInventoryEntry::file("a.txt", 3).unwrap()],
        )
        .unwrap();
        assert!(inventory.admits(&target));
        assert!(inventory.names(&CleanupInventoryEntry::file("a.txt", 3).unwrap()));
        assert!(!inventory.names(&CleanupInventoryEntry::file("b.txt", 3).unwrap()));
        assert_eq!(inventory.layout(), CLEANUP_INVENTORY_LAYOUT);
        assert_eq!(inventory.digest().len(), 64);

        let other = CleanupTarget::new(
            CleanupSubject::new("subject-b").unwrap(),
            target.device().clone(),
            target.operation().clone(),
        );
        assert!(!inventory.admits(&other));
    }

    #[test]
    fn the_digest_binds_the_target_and_every_entry_fact() {
        let target = target();
        let base = CleanupInventory::freeze(
            target.clone(),
            [CleanupInventoryEntry::file("a.txt", 3).unwrap()],
        )
        .unwrap();
        let resized = CleanupInventory::freeze(
            target.clone(),
            [CleanupInventoryEntry::file("a.txt", 4).unwrap()],
        )
        .unwrap();
        let renamed = CleanupInventory::freeze(
            target.clone(),
            [CleanupInventoryEntry::file("b.txt", 3).unwrap()],
        )
        .unwrap();
        let other_target = CleanupInventory::freeze(
            CleanupTarget::new(
                CleanupSubject::new("subject-b").unwrap(),
                target.device().clone(),
                target.operation().clone(),
            ),
            [CleanupInventoryEntry::file("a.txt", 3).unwrap()],
        )
        .unwrap();
        assert_ne!(base.digest(), resized.digest());
        assert_ne!(base.digest(), renamed.digest());
        assert_ne!(base.digest(), other_target.digest());
    }

    #[test]
    fn an_operation_identifier_must_be_a_uuid() {
        assert!(OperationId::new("not-a-uuid").is_err());
        assert!(OperationId::new(OperationId::generate().as_str()).is_ok());
    }
}
