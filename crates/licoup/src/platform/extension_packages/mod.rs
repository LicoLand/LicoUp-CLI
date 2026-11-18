//! C09 and C12 on the host side: discovering, importing, installing, updating,
//! disabling, uninstalling and garbage-collecting *optional* extension packages —
//! without restarting the client and without a marketplace.
//!
//! The contract crate [`licoup_extension_contracts`] owns the shapes: the package
//! manifest, the permission request, the install closure, the ownership table, and
//! the two lifecycle enums. This module owns the *transactions* around them, and
//! the five facts the plan separates on purpose:
//!
//! - **Package state** ([`state::PackageMachine`]): `Available → Downloaded →
//!   Verified/LocalApproved → Staged → Installed`. Installing is not activating:
//!   a package may be installed, disabled, and still on disk; activation is a
//!   per-instance decision taken later.
//! - **Instance state** ([`state::InstanceMachine`]): `Discovered → Preparing →
//!   Active → Draining → Stopped`, with `Failed` and `Quarantined` visible rather
//!   than hidden. One package version can produce several instances with
//!   different permissions, so `packageVersion`, `instanceId`, `generation` and
//!   `registryEpoch` are four separate fields and never one "version" token.
//! - **Discovery** ([`discovery`]): matching small declarative rules against
//!   cached metadata and user-allowed probe locations. A match is a
//!   *recommendation*, never an install and never an execution; the scan is
//!   refused on the GUI frame thread and runs on a worker instead.
//! - **Storage** ([`storage`]): core, optional code, shared runtime, cache, user
//!   data and in-flight pins are accounted separately, and a "space saved" claim
//!   that ignores still-used packages or shared dependencies is refused.
//! - **Compatibility and availability** ([`compatibility`]): which package serves
//!   which Agent version, read from the packages' own manifests and from a
//!   declared interval table, remembered in a version-keyed cache, and answered
//!   on request without reading a manifest at all.
//!
//! Five rules are enforced by construction rather than described:
//!
//! 1. **Uninstall withdraws new admission before it drains anything.** Removing a
//!    package is a typestate: [`uninstall::UninstallTransaction`] can only be
//!    collected out of the `Drained` state, which only `drain` produces, and
//!    `drain` is the call that closes admission on every instance. Closing a page
//!    is not on that path at all.
//! 2. **Nothing here runs a package.** There is no `postinstall` executor, no
//!    `--version` probe in the discovery path, and no code load during matching.
//!    Expansion goes through the host's bounded, no-follow extractor
//!    ([`licoup_foundation::core::safe_archive`]), and activation is a single rename.
//! 3. **Nothing here reaches the network.** A download is bytes handed in by the
//!    host's fetcher together with the digest they were fetched against. A local
//!    import hands in a directory or an already-running adapter endpoint, and
//!    needs no directory service, no account and no publication.
//! 4. **A package loads only when its own compatibility list covers the running
//!    client.** [`PackageStore::install`] refuses one that does not before
//!    anything is published, and activation consumes an
//!    [`ActivationAdmission`] the store issued for that exact package version, so
//!    a package the client has outgrown cannot be started either. The package's
//!    own version is not part of the rule, and `hostProtocol` stays the wire
//!    contract range.
//! 5. **Availability is answered from a record, never from a probe.** The Agent
//!    version in an [`compatibility::AvailabilityRecord`] is an observation a
//!    caller already holds; the request path
//!    ([`compatibility::AvailabilityIndex::cached`]) reads one bounded file and
//!    nothing else. A record decided for another Agent version is stale and is
//!    never reused, and an Agent nothing covers stays unavailable rather than
//!    cached as a guess.

use licoup_application::{ApplicationFailure, RecoveryAction};
use std::fs;
use std::io::Write;
use std::path::Path;

pub mod artifact;
pub mod compatibility;
pub mod discovery;
pub mod endpoint_collaboration;
pub mod install;
pub mod journal;
pub mod maintenance;
pub mod mount_plan;
pub mod registration;
pub mod release_index;
pub mod resources;
pub mod selection;
pub mod state;
pub mod storage;
pub mod surface;
pub mod uninstall;

#[cfg(test)]
mod scenarios;

pub use artifact::{
    ArtifactLimits, ArtifactPreflight, ExpandedPackage, MANIFEST_FILE, content_digest,
    digest_directory, preflight, read_manifest,
};
pub use compatibility::{
    AgentChange, AgentIdentity, AgentNaming, AgentObservation, AgentVersionInterval, Availability,
    AvailabilityCache, AvailabilityIndex, AvailabilityRecord, CachedAvailability, CompatibilityRow,
    CompatibilityTable, DeclaredPackage, DeclaredPackageSource, ManifestSet, PackageOffer,
    StrategyDeclaration, StrategyDeclarations, Unavailable, UnavailableReason,
};
pub use discovery::{
    CatalogEntry, CatalogIndex, Detector, DiscoveryEnvironment, DiscoveryRule, DiscoveryScan,
    OffFrameLane, PendingInstall, Recommendation, RecommendationLog, scan,
};
pub use endpoint_collaboration::{
    ENDPOINT_COLLABORATION_CAPABILITY_ID, ENDPOINT_COLLABORATION_MANIFEST_PATH,
    ENDPOINT_COLLABORATION_PACKAGE_ID, ENDPOINT_COLLABORATION_PROFILE_ID,
    EndpointCollaborationAvailability, EndpointCollaborationBinding, EndpointCollaborationGate,
    EndpointCollaborationLifecycle, EndpointOutboundAuthority, EndpointOutboundRefusal,
    apply_endpoint_collaboration_lifecycle, endpoint_collaboration_gate, resolve_availability,
};
pub use install::{
    ActivationAdmission, FaultPlan, InstallOutcome, InstallPhase, InstallRequest, InstalledPackage,
    PackagePreference, PackageStore, RemovedVersion, StagedPackage, running_client_version,
};
pub use journal::{
    AbandonedStage, InstallJournal, JournalEntry, JournalOperation, RecoveryReport, StagedDirectory,
};
pub use licoup_extension_contracts::deployment::{
    InstanceLifecycle, PackageFacts, PackageLifecycle,
};
pub use mount_plan::{
    HOST_ACTIONS, HOST_RESOURCE_VIEW_FORMATS, MAX_MOUNT_PLAN_BYTES, MOUNT_PLAN_FORMAT,
    MOUNT_PLAN_RECORD, MOUNT_PLAN_VERSION, MountDecision, PlannedContribution, PublishedGeneration,
    ResourceMountPlan, SERVED_PROFILES, mount_plan_path, plan_generation_mount,
    plan_generation_mount_with_actions, plan_published_generations, publish_mount_plan,
    read_published_mount_plan, system_default_name,
};
pub use release_index::{
    IndexedConverter, IndexedPackage, IndexedPayload, MAX_INDEX_BYTES, MAX_INDEX_PACKAGES,
    PACKAGE_INDEX_SCHEMA, VerifiedPackageIndex, bundled_public_keys, canonical_unsigned_bytes,
    verify_index,
};
pub use resources::{
    ADMISSION_BLOCKED, ADMISSION_CLOSED, ADMISSION_RELEASE_FAILED, ADMISSION_UNAVAILABLE,
    AdmissionRelease, AvailableResource, FallbackReason, GenerationReplacement,
    MaintenanceAdmission, MaintenanceAdmissionPort, PreparedGeneration, ReplacementOutcome,
    ResourceBinding, ResourceBindings, ResourceChange, ResourceFallback, ResourceHost,
    SelectionOutcome, SystemDefault, declared_default, install_maintenance_admission,
    maintenance_admission_port,
};
pub use selection::{GenerationSelection, InstalledGeneration, select_generation, switched_on};
pub use state::{
    Admission, InstallActivation, InstanceIdentity, InstanceMachine, InstanceRegistry,
    InstanceReport, PackageMachine, PermissionKey, Settlement, TrustRecord,
    instance_transition_allowed, package_transition_allowed,
};
pub use storage::{
    GcOutcome, GcReport, InFlightPins, RetainReason, SavingsClaim, StorageEntry, StorageKind,
    StorageReport, account_store, plan_gc, reclaim,
};
pub use surface::{
    PackageSurface, RESOURCE_NOT_OWNED, SurfaceResource, SurfaceUninstall, uninstall_package,
};

pub use maintenance::{
    ADMISSION_DECISION_UNREADABLE, ADMISSION_WORK_IN_FLIGHT, GUARD_OWNER, IdleVerdict,
    MaintenanceOperation, MaintenancePermit, MaintenanceRequest, PACKAGE_ADMISSION_CLOSED,
    PackageMaintenanceAdmission,
};
pub use registration::{
    RecordedRegistration, RegistrationOwner, RegistrationOwners, ReleasedRegistration,
};
pub use uninstall::{
    DRAINED_RECORD_SCHEMA, DependentsDecision, Drained, DrainedRecord, MAX_DRAINED_RECORD_BYTES,
    PreservedFacts, RemainingWork, SurfaceClosure, UninstallOutcome, UninstallPlan,
    UninstallTransaction, UserDataPurgeRequest, clear_drained_record, close_surface,
    drained_record_path, keeps_user_runtime, preview, purge_user_data, read_drained_record,
    write_drained_record,
};

/// The component every refusal from this module names, so a client can tell a
/// package-management refusal from a business one without reading text.
pub(crate) const COMPONENT: &str = "extension_packages";

/// A package-management refusal that is a fact about the package, the bytes or
/// the installation, not about the shape of a request.
pub(crate) fn refusal(code: &str, stage: &str) -> ApplicationFailure {
    ApplicationFailure::permanent(code, stage).with_component(COMPONENT)
}

/// A refusal whose next step is a real one: install, enable or re-approve the
/// package that would serve this capability.
pub(crate) fn actionable(code: &str, stage: &str, field: &str) -> ApplicationFailure {
    refusal(code, stage)
        .with_field(field)
        .with_recovery(RecoveryAction::InstallOrRetryRuntime)
}

/// Create a private directory below a managed root.
///
/// The production hardening path is [`licoup_foundation::platform::file_security`]; this is
/// the same 0700-on-unix shape it applies, kept local so this module owns its
/// own layout and tests can use an ordinary temporary root.
pub(crate) fn ensure_private_directory(path: &Path) -> Result<(), ApplicationFailure> {
    fs::create_dir_all(path).map_err(|_| storage_failure("package_directory_unavailable"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| storage_failure("package_directory_unavailable"))?;
    }
    Ok(())
}

fn storage_failure(code: &str) -> ApplicationFailure {
    refusal(code, "extension/package-storage")
}

/// Append one JSON line to a managed journal file.
pub(crate) fn append_journal_line(path: &Path, line: &str) -> Result<(), ApplicationFailure> {
    if line.len() > 4096 {
        return Err(refusal(
            "package_journal_entry_too_large",
            "extension/package-journal",
        )
        .with_field("entry"));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| storage_failure("package_journal_unavailable"))?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|_| storage_failure("package_journal_unavailable"))
}

/// Replace a managed file with new content through a rename inside its own
/// directory, so a reader sees the old content or the new one and never a half
/// written one.
pub(crate) fn replace_file_atomically(
    path: &Path,
    content: &str,
) -> Result<(), ApplicationFailure> {
    let directory = path
        .parent()
        .ok_or_else(|| storage_failure("package_directory_unavailable"))?;
    ensure_private_directory(directory)?;
    let temporary = directory.join(format!(".{}.tmp", unique_suffix()));
    fs::write(&temporary, content).map_err(|_| storage_failure("package_write_failed"))?;
    fs::rename(&temporary, path).map_err(|_| storage_failure("package_write_failed"))
}

/// Read a bounded managed text file, or `None` when it does not exist.
pub(crate) fn read_bounded_text(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<String>, ApplicationFailure> {
    match fs::read(path) {
        Ok(bytes) => {
            if bytes.len() > max_bytes {
                return Err(
                    refusal("package_record_too_large", "extension/package-storage")
                        .with_field("record"),
                );
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| refusal("package_record_invalid", "extension/package-storage"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(storage_failure("package_record_unavailable")),
    }
}

/// The bytes a directory tree occupies, following no symbolic links.
pub(crate) fn directory_bytes(path: &Path) -> Result<u64, ApplicationFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err(storage_failure("package_directory_unavailable")),
    };
    if metadata.file_type().is_symlink() {
        return Err(refusal("package_path_unsafe", "extension/package-storage").with_field("path"));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0;
    for entry in fs::read_dir(path).map_err(|_| storage_failure("package_directory_unavailable"))? {
        let entry = entry.map_err(|_| storage_failure("package_directory_unavailable"))?;
        total += directory_bytes(&entry.path())?;
    }
    Ok(total)
}

/// Remove a managed directory tree and report the bytes it freed.
///
/// Refuses a symbolic link where a managed directory is expected: a link is not
/// this module's artifact to delete, and following it would delete something
/// outside the managed root.
pub(crate) fn remove_managed_tree(path: &Path) -> Result<u64, ApplicationFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err(storage_failure("package_directory_unavailable")),
    };
    if metadata.file_type().is_symlink() {
        return Err(refusal("package_path_unsafe", "extension/package-storage").with_field("path"));
    }
    let bytes = directory_bytes(path)?;
    fs::remove_dir_all(path).map_err(|_| storage_failure("package_remove_failed"))?;
    Ok(bytes)
}

/// A short, collision-resistant suffix for staging directories and temporary
/// files. It is a nonce, not an identity: what identifies content is the digest.
pub(crate) fn unique_suffix() -> String {
    format!("{}-{}", std::process::id(), uuid::Uuid::new_v4().simple())
}

/// Milliseconds since the Unix epoch, for the journal and for previews.
pub(crate) fn now_unix_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_managed_tree_reports_and_frees_its_bytes() {
        let root = std::env::temp_dir().join(format!("licoup-pkg-mod-{}", unique_suffix()));
        ensure_private_directory(&root).expect("root");
        fs::write(root.join("a.bin"), vec![7u8; 32]).expect("write");
        fs::create_dir_all(root.join("nested")).expect("nested");
        fs::write(root.join("nested/b.bin"), vec![1u8; 8]).expect("write");

        assert_eq!(directory_bytes(&root).expect("bytes"), 40);
        assert_eq!(remove_managed_tree(&root).expect("remove"), 40);
        assert!(!root.exists());
        assert_eq!(remove_managed_tree(&root).expect("idempotent"), 0);
    }

    #[test]
    fn a_journal_line_is_bounded_and_appended() {
        let root = std::env::temp_dir().join(format!("licoup-pkg-mod-{}", unique_suffix()));
        ensure_private_directory(&root).expect("root");
        let path = root.join("journal.jsonl");
        append_journal_line(&path, "{\"a\":1}").expect("append");
        append_journal_line(&path, "{\"a\":2}").expect("append");
        let content = read_bounded_text(&path, 4096)
            .expect("read")
            .expect("present");
        assert_eq!(content.lines().count(), 2);

        let oversized = "x".repeat(5000);
        assert_eq!(
            append_journal_line(&path, &oversized)
                .expect_err("bounded")
                .code,
            "package_journal_entry_too_large"
        );
        assert_eq!(
            read_bounded_text(&path, 4).expect_err("bounded read").code,
            "package_record_too_large"
        );
        remove_managed_tree(&root).expect("cleanup");
    }
}
