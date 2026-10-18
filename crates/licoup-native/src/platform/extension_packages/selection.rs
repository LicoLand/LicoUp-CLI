//! Which installed generation of one package the host may start.
//!
//! Installing decides *availability*: which bytes this client holds and what
//! the user approved. Starting decides which of those bytes actually runs, and
//! that decision has exactly one answer for one package:
//!
//! - **One version, never a set.** Several versions of a package may be
//!   installed; only the highest one the user has switched on is selected. Two
//!   selected generations of one package would be two processes answering for
//!   one identity.
//! - **The answer is content-bound.** The generation carries the digest the
//!   operator approved when the version was installed ([`InstalledPackage::digest`])
//!   *and* the digest of the executable entry measured on disk when the
//!   generation was selected. A caller that starts the generation passes the
//!   measured digest to the program, so the process that comes up is the
//!   byte-identical payload the approval was about.
//! - **Absent and disabled are different answers from "the same as before".**
//!   [`GenerationSelection`] names both so a caller reports what is true rather
//!   than falling back to a bundled neighbour or an earlier version.
//!
//! Nothing here starts anything, and nothing here removes anything: selection is
//! a read of the store the installer published, plus one measurement of the
//! entry file it named.

use crate::platform::extension_packages::install::{InstalledPackage, PackageStore};
use crate::platform::extension_packages::{content_digest, read_bounded_text, refusal};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::manifest::Runtime;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The stage every refusal from this module reports.
pub const SELECTION_STAGE: &str = "extension/package-selection";

/// The largest executable entry this client will read to measure it.
///
/// A packaged program is a program, not a data blob: an "entry" larger than this
/// is not something the host starts, and reading it into memory to hash it would
/// be the only reason to.
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

/// The user's switch for one installed version.
///
/// A version with no record was never explicitly switched off, so it is on:
/// installing already decided availability, and forgetting to write a second
/// record must not make an installed package unusable. A record that exists and
/// cannot be read is refused rather than read as "on" — a corrupted switch is
/// not permission to start a service the user may have switched off.
///
/// SEAM(PIPELINE-COMMANDS): the package store is gaining the typed enable/disable
/// preference (`PackageStore::preference` / `set_enabled`, record at
/// `preferences/<packageId>/<version>.json`). When that accessor lands this body
/// becomes one call to it; the record shape below is the one that owner writes.
pub fn switched_on(
    root: &Path,
    package_id: &str,
    version: &str,
) -> Result<bool, ApplicationFailure> {
    let path = root
        .join("preferences")
        .join(package_id)
        .join(format!("{version}.json"));
    let Some(text) = read_bounded_text(&path, 16 * 1024)? else {
        return Ok(true);
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        refusal("package_preference_invalid", SELECTION_STAGE)
            .with_field("enabled")
            .with_presentation_arg("package", package_id)
    })?;
    value
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            refusal("package_preference_invalid", SELECTION_STAGE)
                .with_field("enabled")
                .with_presentation_arg("package", package_id)
        })
}

/// One installed generation, admitted as the thing to start.
///
/// The two digests are different facts and are never collapsed:
/// `approved_digest` is the archive content the operator's decision was bound to
/// (`InstalledPackage::digest`), and `payload_digest` is what the declared
/// executable measures on disk right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledGeneration {
    package_id: String,
    version: String,
    approved_digest: String,
    entry: String,
    entry_path: PathBuf,
    payload_digest: String,
    payload_bytes: u64,
}

impl InstalledGeneration {
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// The digest the operator approved when this version was installed.
    pub fn approved_digest(&self) -> &str {
        &self.approved_digest
    }

    /// The manifest's own `runtime.entry`, relative to the installed version.
    pub fn entry(&self) -> &str {
        &self.entry
    }

    /// The absolute path of the executable this generation starts.
    pub fn entry_path(&self) -> &Path {
        &self.entry_path
    }

    /// The digest of the entry file as it is on disk now.
    pub fn payload_digest(&self) -> &str {
        &self.payload_digest
    }

    pub fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }

    /// The identity one lease and one running process are bound to.
    ///
    /// It names the version and the first bytes of the approved digest, so two
    /// generations of one version installed from different bytes are never the
    /// same generation.
    pub fn generation_id(&self) -> String {
        let digest = self
            .approved_digest
            .strip_prefix("sha256:")
            .unwrap_or(&self.approved_digest);
        let short = digest.get(..12).unwrap_or(digest);
        format!("{}@{}#{short}", self.package_id, self.version)
    }
}

/// What the store says about one package, as one answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GenerationSelection {
    /// Nothing is installed under this package id.
    Absent,
    /// Versions are installed and every one of them is switched off.
    Disabled { installed_versions: Vec<String> },
    /// One generation may be started.
    Selected(Box<InstalledGeneration>),
}

impl GenerationSelection {
    /// The stable name of this answer, for a status surface.
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Disabled { .. } => "disabled",
            Self::Selected(_) => "selected",
        }
    }

    pub fn generation(&self) -> Option<&InstalledGeneration> {
        match self {
            Self::Selected(generation) => Some(generation),
            Self::Absent | Self::Disabled { .. } => None,
        }
    }
}

/// Select the one generation of `package_id` this client may start.
pub fn select_generation(
    store: &PackageStore,
    package_id: &str,
) -> Result<GenerationSelection, ApplicationFailure> {
    super::install::checked_package_id(package_id)?;
    let mut installed: Vec<InstalledPackage> = store
        .installed()?
        .into_iter()
        .filter(|package| package.package_id == package_id)
        .collect();
    if installed.is_empty() {
        return Ok(GenerationSelection::Absent);
    }
    // Deterministic: the highest installed version the user has switched on is
    // the generation, and the ordering does not depend on directory iteration.
    installed.sort_by(|left, right| version_key(&right.version).cmp(&version_key(&left.version)));
    let installed_versions = installed
        .iter()
        .map(|package| package.version.clone())
        .collect::<Vec<_>>();
    for package in &installed {
        if switched_on(store.root(), package_id, &package.version)? {
            return Ok(GenerationSelection::Selected(Box::new(resolve_generation(
                store, package,
            )?)));
        }
    }
    Ok(GenerationSelection::Disabled { installed_versions })
}

/// Resolve one installed record into the executable it declares.
fn resolve_generation(
    store: &PackageStore,
    installed: &InstalledPackage,
) -> Result<InstalledGeneration, ApplicationFailure> {
    let manifest = store.installed_manifest(&installed.package_id, &installed.version)?;
    let Runtime::Process { entry, .. } = &manifest.runtime else {
        return Err(refusal("package_runtime_not_process", SELECTION_STAGE)
            .with_field("runtime.mode")
            .with_presentation_arg("package", installed.package_id.as_str())
            .with_presentation_arg("mode", manifest.runtime.mode()));
    };
    let relative = Path::new(entry);
    let safe = !entry.is_empty()
        && !entry.contains('\\')
        && !entry.contains('\0')
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !safe {
        return Err(refusal("package_entry_unsafe", SELECTION_STAGE)
            .with_field("runtime.entry")
            .with_presentation_arg("entry", entry.as_str()));
    }
    let root = store.installed_path(&installed.package_id, &installed.version);
    let entry_path = root.join(relative);
    let metadata = match fs::symlink_metadata(&entry_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => {
            return Err(refusal("package_entry_unsafe", SELECTION_STAGE)
                .with_field("runtime.entry")
                .with_presentation_arg("entry", entry.as_str()));
        }
        Err(_) => {
            return Err(refusal("package_entry_missing", SELECTION_STAGE)
                .with_field("runtime.entry")
                .with_presentation_arg("entry", entry.as_str()));
        }
    };
    if metadata.len() == 0 || metadata.len() > MAX_ENTRY_BYTES {
        return Err(refusal("package_entry_unsafe", SELECTION_STAGE)
            .with_field("runtime.entry")
            .with_presentation_arg("entry", entry.as_str()));
    }
    let bytes = fs::read(&entry_path).map_err(|_| {
        refusal("package_entry_missing", SELECTION_STAGE)
            .with_field("runtime.entry")
            .with_presentation_arg("entry", entry.as_str())
    })?;
    Ok(InstalledGeneration {
        package_id: installed.package_id.clone(),
        version: installed.version.clone(),
        approved_digest: installed.digest.clone(),
        entry: entry.clone(),
        entry_path,
        payload_digest: content_digest(&bytes),
        payload_bytes: bytes.len() as u64,
    })
}

/// The ordering key of one installed version.
///
/// The parser is the one the contract already requires of a manifest, so
/// pre-release and build ordering follow semantic versioning rather than a
/// second, half-correct comparison here. A record the installer published
/// always carries a semantic version; one that does not sorts below every
/// version that does.
fn version_key(version: &str) -> Option<semver::Version> {
    semver::Version::parse(version).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_ordering_is_deterministic_and_semantic() {
        let mut versions = vec!["1.0.0", "0.9.9", "1.0.0-rc.1", "10.0.0"];
        versions.sort_by(|left, right| version_key(right).cmp(&version_key(left)));
        assert_eq!(versions, vec!["10.0.0", "1.0.0", "1.0.0-rc.1", "0.9.9"]);
    }

    #[test]
    fn a_generation_id_names_the_version_and_the_approved_bytes() {
        let generation = InstalledGeneration {
            package_id: "org.licoland.feature.mcp".to_owned(),
            version: "0.14.0".to_owned(),
            approved_digest: "sha256:0123456789abcdef".to_owned(),
            entry: "bin/lico-subagent-mcp".to_owned(),
            entry_path: PathBuf::from("/nowhere"),
            payload_digest: "sha256:ffff".to_owned(),
            payload_bytes: 1,
        };
        assert_eq!(
            generation.generation_id(),
            "org.licoland.feature.mcp@0.14.0#0123456789ab"
        );
        let mut other = generation.clone();
        other.approved_digest = "sha256:fedcba9876543210".to_owned();
        assert_ne!(generation.generation_id(), other.generation_id());
    }
}
