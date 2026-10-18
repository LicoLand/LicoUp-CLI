//! The installed-package converter inventory.
//!
//! A migration does not know which package converts its data; it asks the store
//! what is installed and reads each package's own declaration. This module is that
//! read, and it is the whole selection rule:
//!
//! 1. Every version the package store records as installed is a candidate for
//!    inspection. A record the store cannot read back is reported, never skipped
//!    silently.
//! 2. A package is a **converter candidate** only when its own manifest declares
//!    the required pair: the required source format is one of the formats it
//!    publishes and the target format is exactly the required target. A package
//!    with no declaration — most packages — is not a candidate, and neither is one
//!    that declares another pair. Which client builds may load a package takes no
//!    part: that is the host's admission question, not the format question.
//! 3. When the caller supplies the signed release index, a candidate must also be
//!    the payload the index publishes: the index signature roles must verify, the
//!    entry must agree with the installed manifest about identity, version, entry
//!    and pair, and the digest and size the host recorded for the installed bytes
//!    must be the digest and size the index publishes. A package that is installed
//!    but not the published payload is reported as unverified rather than run.
//! 4. Selection is deterministic: the greatest installed version of the requested
//!    package, or of every candidate when the caller names none. The report lists
//!    every candidate either way, so an operator sees the alternatives instead of a
//!    silent pick.

use crate::converter::{ConversionOwner, identify_manifest};
use crate::error::{
    CONVERTER_UNAVAILABLE, PACKAGE_INDEX_CONVERTER_MISMATCH, PACKAGE_INDEX_ENTRY_MISSING,
    PACKAGE_INDEX_INVALID, PACKAGE_PAYLOAD_INVALID, PACKAGE_STORE_UNAVAILABLE, ToolError,
    ToolResult,
};
use licoup_extension_contracts::manifest::FrozenEndpoints;
use licoup_native::platform::extension_packages::{
    InstalledPackage, PackageStore, VerifiedPackageIndex, bundled_public_keys, verify_index,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// What the inventory was asked for.
pub struct InventoryRequest<'a> {
    /// The managed root the package store was opened over.
    pub package_store: &'a Path,
    /// A signed release index to check candidates against, when the caller has one.
    pub index: Option<&'a Path>,
    /// The public key catalogue the index is verified with.
    pub index_public_keys: Option<&'a Path>,
    /// One package identity the caller asked for.
    pub requested_package: Option<&'a str>,
}

/// One installed package, as the inventory reports it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackageReport {
    pub package_id: String,
    pub package_version: String,
    /// `verified` or `local-approved`, as the host recorded the install.
    pub trust_channel: String,
    /// Whether the package's own manifest declares any conversion at all.
    pub declares_conversion: bool,
    /// Whether this package declares the required pair.
    pub candidate: bool,
    /// The tool's code for why a package is not a candidate, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_formats: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_format: Option<String>,
    /// Whether the signed index publishes exactly these installed bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_verified: Option<bool>,
}

impl InstalledPackageReport {
    fn not_a_candidate(installed: &InstalledPackage, reason: String) -> Self {
        Self {
            package_id: installed.package_id.clone(),
            package_version: installed.version.clone(),
            trust_channel: channel_name(installed),
            declares_conversion: false,
            candidate: false,
            reason: Some(reason),
            source_formats: Vec::new(),
            target_format: None,
            index_verified: None,
        }
    }
}

/// The signed index the inventory was checked against.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReport {
    pub verified: bool,
    pub release_track: String,
    pub packages: usize,
}

/// The inventory one run concluded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConverterInventoryReport {
    /// Always `inventory`: the read itself never converts anything.
    pub status: &'static str,
    pub source_format: String,
    pub target_format: String,
    pub packages: Vec<InstalledPackageReport>,
    /// `packageId@version` for every candidate, greatest version first.
    pub candidates: Vec<String>,
    /// The candidate a conversion would run, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<SelectedConverterReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexReport>,
    /// The host's own maintenance decision, when the caller named a data root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admission: Option<crate::package_conversion::AdmissionReport>,
}

/// The selected converter, as the report renders it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedConverterReport {
    pub package_id: String,
    pub package_version: String,
    pub trust_channel: String,
    pub entry: String,
    pub source_formats: Vec<String>,
    pub target_format: String,
}

/// The selected converter, with the facts a run needs but a report never prints.
#[derive(Clone, Debug)]
pub struct SelectedConverter {
    pub package_id: String,
    pub package_version: String,
    pub trust_channel: String,
    pub entry: String,
    pub source_formats: Vec<String>,
    pub target_format: String,
    /// The installed package root the entry was resolved inside.
    pub installed_root: PathBuf,
    /// The digest the host recorded for the installed bytes.
    pub record_digest: String,
}

impl SelectedConverter {
    /// The report projection of this selection.
    pub fn report(&self) -> SelectedConverterReport {
        SelectedConverterReport {
            package_id: self.package_id.clone(),
            package_version: self.package_version.clone(),
            trust_channel: self.trust_channel.clone(),
            entry: self.entry.clone(),
            source_formats: self.source_formats.clone(),
            target_format: self.target_format.clone(),
        }
    }
}

/// Read the installed converter inventory, and select the converter to run.
///
/// The two answers come from one read because they are one decision: a caller
/// that runs a converter must run one the inventory named.
pub fn inventory(
    request: &InventoryRequest<'_>,
    required: &FrozenEndpoints,
) -> ToolResult<(ConverterInventoryReport, Option<SelectedConverter>)> {
    // A read never creates the managed root it was pointed at: a wrong path is a
    // refusal, not an empty store.
    if !request.package_store.is_dir() {
        return Err(PACKAGE_STORE_UNAVAILABLE);
    }
    let store = PackageStore::open(request.package_store).map_err(store_unavailable)?;
    let verified_index = read_index(request)?;

    let installed = store.installed().map_err(store_unavailable)?;
    let mut packages = Vec::with_capacity(installed.len());
    let mut candidates: Vec<(ConversionOwner, InstalledPackage)> = Vec::new();

    for package in installed {
        let manifest = match store.installed_manifest(&package.package_id, &package.version) {
            Ok(manifest) => manifest,
            Err(failure) => {
                packages.push(InstalledPackageReport::not_a_candidate(
                    &package,
                    failure.code.clone(),
                ));
                continue;
            }
        };
        let declares_conversion = manifest.conversion.is_some();
        match identify_manifest(&manifest, required) {
            Ok(owner) => {
                packages.push(InstalledPackageReport {
                    package_id: package.package_id.clone(),
                    package_version: package.version.clone(),
                    trust_channel: channel_name(&package),
                    declares_conversion,
                    candidate: true,
                    reason: None,
                    source_formats: owner.source_formats.clone(),
                    target_format: Some(owner.target_format.clone()),
                    index_verified: None,
                });
                candidates.push((owner, package));
            }
            Err(reason) => packages.push(InstalledPackageReport {
                package_id: package.package_id.clone(),
                package_version: package.version.clone(),
                trust_channel: channel_name(&package),
                declares_conversion,
                candidate: false,
                reason: Some(reason.code().to_string()),
                source_formats: manifest
                    .conversion
                    .as_ref()
                    .map(|declaration| declaration.source_formats.clone())
                    .unwrap_or_default(),
                target_format: manifest
                    .conversion
                    .as_ref()
                    .map(|declaration| declaration.target_format.clone()),
                index_verified: None,
            }),
        }
    }

    // The signed index is an authority the caller handed in, so a candidate it does
    // not describe — or describes differently from the installed bytes — is refused
    // here rather than reported as selectable.
    if let Some(index) = verified_index.as_ref() {
        for (owner, package) in &mut candidates {
            let entry = index
                .package(&owner.package_id)
                .ok_or(PACKAGE_INDEX_ENTRY_MISSING)?;
            if entry.package_version != owner.package_version {
                return Err(PACKAGE_INDEX_ENTRY_MISSING);
            }
            let manifest = store
                .installed_manifest(&package.package_id, &package.version)
                .map_err(store_unavailable)?;
            entry
                .reconcile(&manifest, required)
                .map_err(|_| PACKAGE_INDEX_CONVERTER_MISMATCH)?;
            if entry.payload.sha256 != package.digest
                || entry.payload.byte_size != package.compressed_bytes
            {
                return Err(PACKAGE_PAYLOAD_INVALID);
            }
            if let Some(report) = packages.iter_mut().find(|report| {
                report.package_id == package.package_id && report.package_version == package.version
            }) {
                report.index_verified = Some(true);
            }
        }
    }

    // Greatest version first, then by identity, so two runs of the same inventory
    // select the same converter without depending on directory order.
    candidates.sort_by(|(left, left_package), (right, right_package)| {
        let left_version = semver::Version::parse(&left.package_version).ok();
        let right_version = semver::Version::parse(&right.package_version).ok();
        right_version
            .cmp(&left_version)
            .then_with(|| left.package_id.cmp(&right.package_id))
            .then_with(|| left_package.version.cmp(&right_package.version))
    });

    let selected = match request.requested_package {
        Some(requested) => candidates
            .iter()
            .find(|(owner, _)| owner.package_id == requested),
        None => candidates.first(),
    };
    if request.requested_package.is_some() && selected.is_none() {
        return Err(CONVERTER_UNAVAILABLE);
    }

    let candidate_names = candidates
        .iter()
        .map(|(owner, _)| format!("{}@{}", owner.package_id, owner.package_version))
        .collect();

    let selected = match selected {
        Some((owner, package)) => Some(SelectedConverter {
            package_id: owner.package_id.clone(),
            package_version: owner.package_version.clone(),
            trust_channel: channel_name(package),
            entry: owner.entry.clone(),
            source_formats: owner.source_formats.clone(),
            target_format: owner.target_format.clone(),
            installed_root: store.installed_path(&owner.package_id, &owner.package_version),
            record_digest: package.digest.clone(),
        }),
        None => None,
    };

    let report = ConverterInventoryReport {
        status: "inventory",
        source_format: required.source_format().to_string(),
        target_format: required.target_format().to_string(),
        packages,
        candidates: candidate_names,
        selected: selected.as_ref().map(SelectedConverter::report),
        index: verified_index.as_ref().map(|index| IndexReport {
            verified: true,
            release_track: index.release_track.clone(),
            packages: index.packages.len(),
        }),
        admission: None,
    };
    Ok((report, selected))
}

/// Verify the signed index the caller supplied, when it supplied one.
fn read_index(request: &InventoryRequest<'_>) -> ToolResult<Option<VerifiedPackageIndex>> {
    match request.index {
        Some(path) => verified_index(path, request.index_public_keys).map(Some),
        None => Ok(None),
    }
}

/// Verify one signed index against one public key catalogue.
///
/// The catalogue is the caller's when it supplied one and the client's bundled
/// release catalogue otherwise: the index is signed by the release authority's two
/// roles, and the same catalogue already verifies the client update manifest.
pub fn verified_index(
    index: &Path,
    public_keys: Option<&Path>,
) -> ToolResult<VerifiedPackageIndex> {
    let text = read_bounded_text(index)?;
    let keys = match public_keys {
        Some(path) => read_bounded_text(path)?,
        None => bundled_public_keys().to_string(),
    };
    verify_index(&text, &keys).map_err(|_| PACKAGE_INDEX_INVALID)
}

/// Read one bounded text document, naming no path in the failure.
fn read_bounded_text(path: &Path) -> ToolResult<String> {
    let metadata = std::fs::metadata(path).map_err(|_| PACKAGE_INDEX_INVALID)?;
    if !metadata.is_file()
        || metadata.len() > licoup_native::platform::extension_packages::MAX_INDEX_BYTES as u64
    {
        return Err(PACKAGE_INDEX_INVALID);
    }
    std::fs::read_to_string(path).map_err(|_| PACKAGE_INDEX_INVALID)
}

/// The store's own install channel, as the tool reports it.
fn channel_name(installed: &InstalledPackage) -> String {
    format!("{:?}", installed.trust_channel).to_lowercase()
}

/// The store refused to read the managed root.
fn store_unavailable(_failure: licoup_extension_contracts::ApplicationFailure) -> ToolError {
    PACKAGE_STORE_UNAVAILABLE
}
