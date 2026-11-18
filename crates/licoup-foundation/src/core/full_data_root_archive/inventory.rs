//! The archive inventory.
//!
//! The inventory is derived from the data root itself and from the owners that
//! write into it. It never enumerates unrelated operating-system credentials, and
//! it never treats an opaque platform key handle as a portable secret.
//!
//! Every declared path is a canonical, host-independent POSIX path. Membership,
//! structure and limit facts are validated before a manifest is published or any
//! payload member is written, so a successful export is always importable under the
//! extractor's own policy.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::safe_archive::default_zip_extraction_limits;

/// Archive layout identifier. A restore refuses any other value.
pub const ARCHIVE_LAYOUT: &str = "licoup.full-data-root/v1";

/// One captured entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryEntry {
    /// Canonical POSIX-relative path inside the data root.
    pub path: String,
    pub kind: InventoryKind,
    /// Uncompressed byte length for a file entry.
    #[serde(default)]
    pub size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InventoryKind {
    File,
    Directory,
}

/// What the maintainer is told about recovery completeness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryCoverage {
    /// Every application-owned store was captured and no named domain is limited.
    Complete,
    /// The archive is usable, but a named domain cannot be recovered from it.
    Limited,
}

/// A domain whose secrets cannot travel in a portable archive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoveryLimitation {
    /// Stable domain identifier, for example `gateway-credential-custody`.
    pub domain: String,
    /// Why the domain cannot be recovered from this archive.
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveManifest {
    pub layout: String,
    pub container: String,
    /// The logical data-home spelling the capture was taken from, preserved so an
    /// import into another home can rebase the references its owners rewrite. It is
    /// provenance, not authority: it grants no custody and never replaces the active
    /// selection.
    pub source_home: String,
    pub created_at_unix: u64,
    pub coverage: RecoveryCoverage,
    #[serde(default)]
    pub limitations: Vec<RecoveryLimitation>,
    pub entries: Vec<InventoryEntry>,
    /// Total uncompressed bytes across file entries.
    pub total_bytes: u64,
}

impl ArchiveManifest {
    pub(crate) fn new(
        container: &str,
        source_home: &Path,
        coverage: RecoveryCoverage,
        limitations: Vec<RecoveryLimitation>,
        entries: Vec<InventoryEntry>,
    ) -> Result<Self> {
        let mut total_bytes = 0_u64;
        for entry in &entries {
            if entry.kind == InventoryKind::File {
                total_bytes = total_bytes
                    .checked_add(entry.size)
                    .ok_or_else(|| anyhow!("archive_inventory_size_invalid"))?;
            }
        }
        Ok(Self {
            layout: ARCHIVE_LAYOUT.to_string(),
            container: container.to_string(),
            source_home: source_home.to_string_lossy().into_owned(),
            created_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default(),
            coverage,
            limitations,
            entries,
            total_bytes,
        })
    }

    pub(crate) fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == InventoryKind::File)
            .count()
    }

    /// Validate the manifest against the container the archive actually is and against
    /// the payload limits its own importer must apply. Nothing is written before this.
    pub(crate) fn validate(
        &self,
        expected_container: &str,
        manifest_member_bytes: u64,
    ) -> Result<()> {
        ensure!(self.layout == ARCHIVE_LAYOUT, "archive_layout_unsupported");
        // A source home is provenance an import needs; an archive that does not name
        // its logical origin cannot be rebased and is refused before publication.
        ensure!(
            Path::new(&self.source_home).is_absolute(),
            "archive_source_home_invalid"
        );
        // Custody is never proven by the archive itself. The owner never exports
        // Complete, and until the credential owner can establish non-secret custody
        // facts, a caller-controlled Complete is refused instead of restored as
        // evidence that platform-held material is available.
        ensure!(
            self.coverage == RecoveryCoverage::Limited,
            "archive_coverage_unproven"
        );
        ensure!(
            self.container == expected_container,
            "archive_container_mismatch"
        );
        let facts = validate_inventory_structure(&self.entries)?;
        ensure!(
            facts.file_bytes == self.total_bytes,
            "archive_inventory_size_invalid"
        );
        ensure_within_archive_limits(&facts, manifest_member_bytes, "archive_extraction_refused")
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|_| anyhow!("archive_manifest_unencodable"))
    }

    pub(crate) fn from_bytes(bytes: &[u8], expected_container: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| anyhow!("archive_manifest_invalid"))?;
        manifest.validate(expected_container, bytes.len() as u64)?;
        Ok(manifest)
    }
}

/// Facts about a declared inventory that decide the archive's supported limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InventoryFacts {
    /// Total bytes across file entries, checked for overflow.
    pub file_bytes: u64,
    /// Declared member count including the manifest member.
    pub member_count: usize,
    /// Deepest member path, counting the `data/` member prefix.
    pub max_member_depth: usize,
    /// Largest single file entry.
    pub max_file_bytes: u64,
}

/// Validate the structure and portable grammar of a declared inventory.
///
/// This is the single authority for what an inventory may contain: every caller that
/// captures or imports an archive runs it, so capture and import refuse the same shapes
/// instead of each keeping a private subset of the rule.
pub(crate) fn validate_inventory_structure(entries: &[InventoryEntry]) -> Result<InventoryFacts> {
    let mut exact = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut file_paths = Vec::new();
    let mut file_bytes = 0_u64;
    let mut max_file_bytes = 0_u64;
    let mut max_member_depth = 1_usize;
    for entry in entries {
        ensure!(
            !portable_path_defect(&entry.path),
            "archive_inventory_path_invalid"
        );
        ensure!(
            exact.insert(entry.path.as_str()),
            "archive_inventory_duplicate"
        );
        ensure!(
            folded.insert(entry.path.to_lowercase()),
            "archive_inventory_case_collision"
        );
        max_member_depth = max_member_depth.max(entry.path.split('/').count() + 1);
        match entry.kind {
            InventoryKind::Directory => {
                ensure!(entry.size == 0, "archive_inventory_size_invalid");
            }
            InventoryKind::File => {
                file_paths.push(entry.path.as_str());
                file_bytes = file_bytes
                    .checked_add(entry.size)
                    .ok_or_else(|| anyhow!("archive_inventory_size_invalid"))?;
                max_file_bytes = max_file_bytes.max(entry.size);
            }
        }
    }
    // A file path may not be an ancestor of another declared member; extraction could not
    // create both without structural aliasing.
    let mut sorted: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    sorted.sort_unstable();
    for file in &file_paths {
        let prefix = format!("{file}/");
        let position = sorted.partition_point(|path| *path < prefix.as_str());
        if let Some(candidate) = sorted.get(position) {
            ensure!(
                !candidate.starts_with(&prefix),
                "archive_inventory_conflict"
            );
        }
    }
    Ok(InventoryFacts {
        file_bytes,
        member_count: entries.len() + 1,
        max_member_depth,
        max_file_bytes,
    })
}

/// The one archive policy, applied before writing members on capture and before
/// extracting them on import. `code` names the caller's own refusal.
pub(crate) fn ensure_within_archive_limits(
    facts: &InventoryFacts,
    manifest_member_bytes: u64,
    code: &'static str,
) -> Result<()> {
    let limits = default_zip_extraction_limits();
    ensure!(facts.member_count <= limits.max_entries, "{}", code);
    ensure!(facts.max_member_depth <= limits.max_depth, "{}", code);
    ensure!(facts.max_file_bytes <= limits.max_file_bytes, "{}", code);
    let total = facts
        .file_bytes
        .checked_add(manifest_member_bytes)
        .ok_or_else(|| anyhow!("{}", code))?;
    ensure!(total <= limits.max_total_bytes, "{}", code);
    Ok(())
}

/// Whether a declared path is not a canonical, host-independent POSIX path.
///
/// Backslashes, drive or UNC separators, control characters, empty components and
/// `.`/`..` components are refused independently of the host platform, so a validated
/// manifest can never describe a location outside its payload root.
fn portable_path_defect(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') {
        return true;
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return true;
        }
        if part.contains('\\') || part.contains(':') {
            return true;
        }
        if part.chars().any(char::is_control) {
            return true;
        }
    }
    false
}

/// Inventory of one data root, in deterministic order.
///
/// Symbolic links are not followed and are not captured: an archive carries the
/// client's own files, not whatever a link happens to point at.
pub(crate) fn inventory_data_root(root: &Path) -> Result<Vec<InventoryEntry>> {
    ensure!(root.is_dir(), "data_root_missing");
    let mut entries = Vec::new();
    walk(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn walk(root: &Path, directory: &Path, entries: &mut Vec<InventoryEntry>) -> Result<()> {
    let mut children: Vec<PathBuf> = Vec::new();
    let reader = std::fs::read_dir(directory).map_err(|_| anyhow!("data_root_unreadable"))?;
    for child in reader {
        let child = child.map_err(|_| anyhow!("data_root_entry_unreadable"))?;
        children.push(child.path());
    }
    children.sort();
    for child in children {
        let metadata =
            std::fs::symlink_metadata(&child).map_err(|_| anyhow!("data_root_entry_unreadable"))?;
        let relative = child
            .strip_prefix(root)
            .map_err(|_| anyhow!("data_root_entry_outside_root"))?;
        let path = posix_relative(relative)?;
        if metadata.is_symlink() {
            // A link is deliberately absent from the inventory; a restore of this
            // archive therefore produces ordinary files and directories only.
            continue;
        }
        if metadata.is_dir() {
            entries.push(InventoryEntry {
                path: path.clone(),
                kind: InventoryKind::Directory,
                size: 0,
            });
            walk(root, &child, entries)?;
        } else if metadata.is_file() {
            entries.push(InventoryEntry {
                path,
                kind: InventoryKind::File,
                size: metadata.len(),
            });
        }
    }
    Ok(())
}

pub(crate) fn posix_relative(relative: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| anyhow!("data_root_path_not_utf8"))?;
                parts.push(name.to_string());
            }
            _ => return Err(anyhow!("data_root_path_unsafe")),
        }
    }
    ensure!(!parts.is_empty(), "data_root_path_empty");
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(path: &str) -> InventoryEntry {
        InventoryEntry {
            path: path.to_string(),
            kind: InventoryKind::Directory,
            size: 0,
        }
    }

    fn file(path: &str, size: u64) -> InventoryEntry {
        InventoryEntry {
            path: path.to_string(),
            kind: InventoryKind::File,
            size,
        }
    }

    #[test]
    fn non_portable_paths_are_refused() {
        for path in [
            "a\\b",
            "..\\escaped.bin",
            "C:\\evil.bin",
            "\\\\server\\share\\evil.bin",
            "a:b",
            "a\u{0}b",
            "/absolute",
            "",
            ".",
            "..",
            "a/./b",
            "a//b",
            "a/",
            "a\nb",
        ] {
            assert!(
                validate_inventory_structure(&[file(path, 0)]).is_err(),
                "path {path:?} must be refused"
            );
        }
    }

    #[test]
    fn duplicate_case_collision_and_structural_conflicts_are_refused() {
        assert!(validate_inventory_structure(&[file("a.txt", 1), file("a.txt", 1)]).is_err());
        assert!(validate_inventory_structure(&[file("A.txt", 1), file("a.txt", 1)]).is_err());
        assert!(validate_inventory_structure(&[file("a", 1), file("a/b", 1)]).is_err());
        assert!(validate_inventory_structure(&[directory("a"), file("a/b", 1)]).is_ok());
    }

    #[test]
    fn facts_include_the_manifest_member_and_the_data_prefix() {
        let facts = validate_inventory_structure(&[directory("a"), file("a/b.bin", 7)]).unwrap();
        assert_eq!(facts.file_bytes, 7);
        assert_eq!(facts.member_count, 3);
        assert_eq!(facts.max_member_depth, 3);
        assert_eq!(facts.max_file_bytes, 7);
    }

    #[test]
    fn limits_include_the_manifest_overhead() {
        let limits = default_zip_extraction_limits();
        let facts = InventoryFacts {
            file_bytes: limits.max_total_bytes - 1,
            member_count: 2,
            max_member_depth: 2,
            max_file_bytes: limits.max_total_bytes - 1,
        };
        assert!(ensure_within_archive_limits(&facts, 1, "fixture_limit").is_ok());
        assert!(ensure_within_archive_limits(&facts, 2, "fixture_limit").is_err());
    }
}
