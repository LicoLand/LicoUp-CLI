//! Identify the package that owns one required conversion.
//!
//! This module holds no format table and no conversion algorithm. Two owners
//! declare the facts, and identification is the join of them:
//!
//! - the client's embedded frontier catalogue declares the one source/target
//!   pair this tool is built around ([`required_conversion`]), and
//! - the package's own manifest declares which published formats it reads and
//!   produces and which entry inside its payload performs the move.
//!
//! The tool therefore answers "who converts this" by reading the package, not by
//! recognising a name it was compiled with. A package whose declaration is a
//! different pair is refused even when it looks like a converter, and the same
//! package is identified for the pair it does declare.
//!
//! Nothing here reads the running client version. Which formats a package owns is
//! a claim about its payload; which client builds may load it is a separate claim
//! the host decides ([`licoup_extension_contracts::manifest::PackageManifest::admit_client`]).
//! Source support is never inferred from the version of the installed old client,
//! so a migration is not refused because the client that wrote the source is a
//! different release from this tool.

use crate::error::{
    CONVERTER_ENDPOINT_MISMATCH, CONVERTER_ENTRY_MISSING, CONVERTER_ENTRY_OUTSIDE_PACKAGE,
    CONVERTER_INCOMPLETE, CONVERTER_INVALID, CONVERTER_MANIFEST_INVALID,
    CONVERTER_MANIFEST_UNREADABLE, CONVERTER_MISSING, CONVERTER_NOT_NATIVE, FRONTIER_UNAVAILABLE,
    ToolError, ToolResult,
};
use licoup_extension_contracts::manifest::{FrozenEndpoints, PackageManifest, conversion_code};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The manifest document inside a package payload.
pub const MANIFEST_NAME: &str = "manifest.json";

/// The largest manifest document this tool reads.
///
/// A declaration is a small document; a file larger than this is not one, and
/// reading it anyway would let a malformed payload decide how much memory a
/// maintenance operation uses.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// One package that owns the required conversion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionOwner {
    pub package_id: String,
    pub package_version: String,
    /// Every published source format the package's converter reads.
    pub source_formats: Vec<String>,
    /// The published target format it produces.
    pub target_format: String,
    /// The converter entry, relative to the package root.
    pub entry: String,
}

/// The frozen endpoints the client's embedded catalogue declares.
///
/// The pair is read from the client's own frontier authority rather than
/// restated here, because a second copy of a format name is a second place to
/// disagree with the conversion the client actually performs. It is a pair of
/// format identities: no product version takes part.
pub fn required_conversion() -> ToolResult<FrozenEndpoints> {
    let endpoints = licoup_native::domain::client_state_migration::conversion_endpoints()
        .map_err(|_| FRONTIER_UNAVAILABLE)?;
    Ok(FrozenEndpoints::new(
        endpoints.source_frontier_id,
        endpoints.target_frontier_id,
    ))
}

/// Read one manifest document and report the conversion it owns.
///
/// The document is the package's claim, so a claim that does not answer the
/// required pair is refused here rather than carried into a conversion the
/// package never declared.
pub fn identify_owner(
    manifest_path: &Path,
    required: &FrozenEndpoints,
) -> ToolResult<ConversionOwner> {
    let value = read_manifest(manifest_path)?;
    identify_declared(value, required)
}

/// Identify the owner of a package root: its manifest plus the entry it promises.
///
/// The declaration says where the converter is; this reads the payload to check
/// that the entry is really inside the package. A package whose manifest points
/// outside its own root is refused, and so is one whose entry is absent: the tool
/// must not hand a conversion to a program that is not there.
pub fn identify_owner_in_root(
    package_root: &Path,
    required: &FrozenEndpoints,
) -> ToolResult<ConversionOwner> {
    let root = package_root
        .canonicalize()
        .map_err(|_| CONVERTER_MANIFEST_UNREADABLE)?;
    let owner = identify_owner(&root.join(MANIFEST_NAME), required)?;
    let entry = root.join(&owner.entry);
    let resolved = entry.canonicalize().map_err(|_| CONVERTER_ENTRY_MISSING)?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err(CONVERTER_ENTRY_OUTSIDE_PACKAGE);
    }
    Ok(owner)
}

/// Identify the owner from an already-read manifest document.
pub fn identify_declared(
    value: serde_json::Value,
    required: &FrozenEndpoints,
) -> ToolResult<ConversionOwner> {
    let manifest = PackageManifest::from_value(value).map_err(refusal_of)?;
    identify_manifest(&manifest, required)
}

/// Identify the owner from an already-validated manifest.
///
/// The store reads a manifest back from installed content and validates it, so a
/// caller that already holds one asks the same question here instead of
/// re-serialising the document to go through [`identify_declared`].
pub fn identify_manifest(
    manifest: &PackageManifest,
    required: &FrozenEndpoints,
) -> ToolResult<ConversionOwner> {
    let declaration = manifest.conversion_owner(required).map_err(refusal_of)?;
    Ok(ConversionOwner {
        package_id: manifest.id.clone(),
        package_version: manifest.version.clone(),
        source_formats: declaration.source_formats.clone(),
        target_format: declaration.target_format.clone(),
        entry: declaration.entry.clone(),
    })
}

/// The tool's own stable code for one contract refusal.
///
/// The code a report carries is the tool's vocabulary; the rule it stands for is
/// the contract's. A structural manifest failure that is not about the
/// conversion at all is reported as an invalid manifest rather than as a
/// conversion verdict, so a caller does not read "no converter" where the
/// document itself was unreadable.
fn refusal_of(failure: licoup_extension_contracts::ApplicationFailure) -> ToolError {
    match failure.code.as_str() {
        conversion_code::MISSING => CONVERTER_MISSING,
        conversion_code::NOT_NATIVE => CONVERTER_NOT_NATIVE,
        conversion_code::ENTRY_OUTSIDE_PACKAGE => CONVERTER_ENTRY_OUTSIDE_PACKAGE,
        conversion_code::INCOMPLETE => CONVERTER_INCOMPLETE,
        conversion_code::INVALID => CONVERTER_INVALID,
        conversion_code::ENDPOINT_MISMATCH => CONVERTER_ENDPOINT_MISMATCH,
        _ => CONVERTER_MANIFEST_INVALID,
    }
}

/// Read one bounded manifest document.
fn read_manifest(manifest_path: &Path) -> ToolResult<serde_json::Value> {
    let metadata = std::fs::metadata(manifest_path).map_err(|_| CONVERTER_MANIFEST_UNREADABLE)?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(CONVERTER_MANIFEST_UNREADABLE);
    }
    let text = std::fs::read_to_string(manifest_path).map_err(|_| CONVERTER_MANIFEST_UNREADABLE)?;
    serde_json::from_str(&text).map_err(|_| CONVERTER_MANIFEST_UNREADABLE)
}

/// The manifest path inside one package root, for a caller that reports it.
pub fn manifest_in(package_root: &Path) -> PathBuf {
    package_root.join(MANIFEST_NAME)
}
