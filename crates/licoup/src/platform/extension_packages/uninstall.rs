//! Uninstall as an ordered transaction: withdraw, drain, then reclaim.
//!
//! The order is the whole feature, so it is the type rather than a comment:
//!
//! 1. [`UninstallTransaction::begin`] closes admission on every instance of the
//!    package. Instances keep running their admitted work; they simply stop
//!    taking new work. Nothing is killed here.
//! 2. [`UninstallTransaction::drain`] moves them to `Draining` and then to
//!    `Stopped`. A caller either waits for in-flight work ([`RemainingWork::Wait`])
//!    or cancels it ([`RemainingWork::Cancel`]) — cancelling records the outcome
//!    as `Unknown` and never re-dispatches it.
//! 3. Only a [`Drained`] value can [`Drained::collect`], and it
//!    refuses if any instance of the package is still running. There is no path
//!    from `begin` to `collect` that skips the drain, so "uninstall while a
//!    generation is still serving" is not representable.
//!
//! What uninstall does **not** do is as important:
//!
//! - It removes the selected package only. A package other packages depend on is
//!   reported with its dependents and refused until the user decides what happens
//!   to them; there is no silent cascade.
//! - It removes this module's managed bytes only. History, credentials and
//!   protocol state survive, and clearing them is a separate explicit operation
//!   ([`purge_user_data`]).
//! - It never removes an interpreter or tool the user installed themselves
//!   (`user:` runtime references belong to the user), and a shared runtime another
//!   package still uses is retained rather than deleted.
//! - Closing a page is not uninstalling a package: [`close_surface`] touches
//!   neither the package nor its instances.

use crate::platform::extension_packages::install::{InstalledPackage, PackageStore};
use crate::platform::extension_packages::registration::{
    RecordedRegistration, RegistrationOwners, ReleasedRegistration,
};
use crate::platform::extension_packages::state::{InstanceRegistry, Settlement};
use crate::platform::extension_packages::{
    read_bounded_text, refusal, remove_managed_tree, replace_file_atomically,
};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::{InstanceLifecycle, LocalCatalogue};
use licoup_extension_contracts::manifest::USER_RUNTIME_PREFIX;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const UNINSTALL_STAGE: &str = "extension/package-uninstall";

/// The facts an uninstall preserves, stated so they can be asserted rather than
/// assumed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreservedFacts {
    /// The conversation and result history the package produced.
    pub history: bool,
    /// Credentials the user granted, in the host's custody.
    pub credentials: bool,
    /// Protocol state such as keys and epochs.
    pub protocol_state: bool,
}

impl PreservedFacts {
    /// Everything survives an uninstall.
    pub const fn all_kept() -> Self {
        Self {
            history: true,
            credentials: true,
            protocol_state: true,
        }
    }
}

/// What the user chose for work that is still in flight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemainingWork {
    /// Wait for it to finish. Draining refuses while work is unsettled.
    Wait,
    /// Cancel it. Cancellation is a request: the outcome is recorded `Unknown`,
    /// which is a fact and not a failure to hide.
    Cancel,
}

/// What uninstall would do, shown before anything is removed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPlan {
    pub package_id: String,
    pub version: String,
    /// Packages in the catalogue that declare this one in `requires`.
    pub reverse_dependencies: Vec<String>,
    /// Instances of this package that are still running.
    pub running_instances: Vec<String>,
    pub in_flight: u32,
    /// Managed bytes that are this version's own.
    pub exclusive_bytes: u64,
    /// A shared runtime this package references, when it references one.
    pub shared_runtime_ref: Option<String>,
    /// Registrations this version recorded in surfaces other modules own. The
    /// preview names them because uninstall releases them, and a user is told
    /// what will be touched outside the package's own bytes.
    pub registrations: Vec<RecordedRegistration>,
    pub preserved: PreservedFacts,
}

impl UninstallPlan {
    /// Whether the user still has to decide about dependent packages.
    pub fn needs_user_choice(&self) -> bool {
        !self.reverse_dependencies.is_empty()
    }

    /// What the plan would explain to a user, naming who depends on what.
    pub fn explanation(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "{}@{}: {} running instance(s), {} in-flight, {} exclusive byte(s)",
            self.package_id,
            self.version,
            self.running_instances.len(),
            self.in_flight,
            self.exclusive_bytes
        )];
        for dependent in &self.reverse_dependencies {
            lines.push(format!("{dependent} requires {}", self.package_id));
        }
        if let Some(reference) = &self.shared_runtime_ref {
            lines.push(format!(
                "shared runtime {reference} stays until nothing uses it"
            ));
        }
        for registration in &self.registrations {
            lines.push(format!(
                "{} registration {} is released through {}",
                registration.owner.wire_name(),
                registration.key,
                registration.owner.owner_module()
            ));
        }
        lines
    }
}

/// Build the preview for one installed version.
pub fn preview(
    store: &PackageStore,
    catalogue: &LocalCatalogue,
    installed: &InstalledPackage,
    registry: &InstanceRegistry,
) -> Result<UninstallPlan, ApplicationFailure> {
    let package_id = installed.package_id.as_str();
    let reverse_dependencies: Vec<String> = catalogue
        .ids()
        .filter(|id| *id != package_id)
        .filter(|id| {
            catalogue.get(id).is_some_and(|entry| {
                entry
                    .requires
                    .iter()
                    .any(|dep| dep.package_id == package_id)
            })
        })
        .map(str::to_owned)
        .collect();
    let running: Vec<String> = registry
        .running_of(package_id)
        .map(|machine| machine.identity().instance_id.clone())
        .collect();
    Ok(UninstallPlan {
        package_id: package_id.to_owned(),
        version: installed.version.clone(),
        reverse_dependencies,
        running_instances: running,
        in_flight: registry
            .running_of(package_id)
            .map(|machine| machine.in_flight())
            .sum(),
        exclusive_bytes: store.installed_bytes(package_id, &installed.version)?,
        shared_runtime_ref: installed
            .runtime_ref
            .clone()
            .filter(|reference| !reference.starts_with(USER_RUNTIME_PREFIX)),
        registrations: installed.registrations.clone(),
        preserved: PreservedFacts::all_kept(),
    })
}

/// The user's decision about packages that depend on the one being removed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependentsDecision {
    /// Remove only what was asked for, and refuse while a dependent still
    /// requires it.
    SelectedOnly,
    /// The user accepted the named dependents being removed with it.
    RemoveTogether(Vec<String>),
}

/// One package uninstall, in order.
///
/// Holding this value means admission is already withdrawn: [`begin`] did that
/// before returning. It cannot reclaim anything — only [`UninstallTransaction::drain`]
/// produces a [`Drained`], and only a `Drained` can collect.
///
/// [`begin`]: UninstallTransaction::begin
#[derive(Debug)]
pub struct UninstallTransaction {
    plan: UninstallPlan,
    withdrawn_instances: Vec<String>,
    removed_together: Vec<String>,
}

/// Every instance has stopped; managed bytes may now be reclaimed.
#[derive(Debug)]
pub struct Drained {
    plan: UninstallPlan,
    drained_instances: Vec<String>,
    canceled_work: u32,
    unknown_work: u32,
    removed_together: Vec<String>,
}

impl UninstallTransaction {
    /// Withdraw new admission first, then hand back a transaction that can drain.
    pub fn begin(
        registry: &mut InstanceRegistry,
        plan: UninstallPlan,
        dependents: DependentsDecision,
    ) -> Result<Self, ApplicationFailure> {
        let removed_together = match &dependents {
            DependentsDecision::SelectedOnly => {
                if plan.needs_user_choice() {
                    let mut failure = refusal("package_uninstall_has_dependents", UNINSTALL_STAGE)
                        .with_field("packageId")
                        .with_presentation_arg("package", plan.package_id.as_str());
                    for dependent in &plan.reverse_dependencies {
                        failure = failure.with_presentation_arg("requiredBy", dependent.as_str());
                    }
                    return Err(failure);
                }
                Vec::new()
            }
            DependentsDecision::RemoveTogether(names) => {
                for name in names {
                    if !plan.reverse_dependencies.contains(name) {
                        return Err(refusal(
                            "package_uninstall_dependent_unknown",
                            UNINSTALL_STAGE,
                        )
                        .with_field("packageId")
                        .with_presentation_arg("package", name.as_str()));
                    }
                }
                names.clone()
            }
        };

        // Step one, before anything else happens: new admissions stop.
        let _ = registry.withdraw_admission_of(&plan.package_id);
        for name in &removed_together {
            let _ = registry.withdraw_admission_of(name);
        }
        let withdrawn_instances: Vec<String> = registry
            .instances()
            .filter(|machine| {
                machine.identity().package_id == plan.package_id
                    || removed_together.contains(&machine.identity().package_id)
            })
            .map(|machine| machine.identity().instance_id.clone())
            .collect();

        Ok(Self {
            plan,
            withdrawn_instances,
            removed_together,
        })
    }

    pub fn plan(&self) -> &UninstallPlan {
        &self.plan
    }

    /// Instances whose admission is already closed.
    pub fn withdrawn_instances(&self) -> &[String] {
        &self.withdrawn_instances
    }

    /// Drain every instance, then move to the `Drained` state.
    ///
    /// `Wait` refuses while admitted work is unsettled and changes nothing, so a
    /// caller can poll it while the user waits. `Cancel` settles the remaining
    /// work as `Unknown`.
    pub fn drain(
        self,
        registry: &mut InstanceRegistry,
        remaining: RemainingWork,
    ) -> Result<Drained, ApplicationFailure> {
        let in_flight: u32 = registry
            .instances()
            .filter(|machine| {
                machine.identity().package_id == self.plan.package_id
                    || self
                        .removed_together
                        .contains(&machine.identity().package_id)
            })
            .map(|machine| machine.in_flight())
            .sum();
        if remaining == RemainingWork::Wait && in_flight > 0 {
            return Err(refusal("package_uninstall_in_flight", UNINSTALL_STAGE)
                .with_field("packageId")
                .with_presentation_arg("package", self.plan.package_id.as_str())
                .with_presentation_arg("inFlight", &in_flight.to_string()));
        }

        let ids: Vec<String> = registry
            .instances()
            .filter(|machine| {
                machine.identity().package_id == self.plan.package_id
                    || self
                        .removed_together
                        .contains(&machine.identity().package_id)
            })
            .filter(|machine| {
                !matches!(
                    machine.state(),
                    InstanceLifecycle::Stopped | InstanceLifecycle::Quarantined
                )
            })
            .map(|machine| machine.identity().instance_id.clone())
            .collect();
        let mut drained = Vec::new();
        let mut canceled_work = 0_u32;
        let mut unknown_work = 0_u32;
        for id in ids {
            let Some(machine) = registry.get_mut(&id) else {
                continue;
            };
            if matches!(machine.state(), InstanceLifecycle::Failed) {
                // A failed instance is not running, and its pins stay recorded:
                // the work it admitted is unknown, not gone.
                continue;
            }
            if machine.state() == InstanceLifecycle::Active {
                machine.drain()?;
            }
            if remaining == RemainingWork::Cancel {
                while machine.in_flight() > 0 {
                    machine.settle(Settlement::Unknown)?;
                    canceled_work += 1;
                    unknown_work += 1;
                }
            }
            machine.stop()?;
            drained.push(id);
        }

        Ok(Drained {
            plan: self.plan,
            drained_instances: drained,
            canceled_work,
            unknown_work,
            removed_together: self.removed_together,
        })
    }
}

impl Drained {
    pub fn plan(&self) -> &UninstallPlan {
        &self.plan
    }

    /// The drained decision, as a value that can survive this process.
    ///
    /// A one-shot caller cannot hold the transaction in memory between two
    /// invocations, so the decision is written down and read back. What is *not*
    /// written down is permission: [`Drained::collect`] re-derives the running
    /// set and re-checks it, so a record that outlived the state it described
    /// fails closed instead of reclaiming bytes.
    pub fn record(&self) -> DrainedRecord {
        DrainedRecord {
            schema: DRAINED_RECORD_SCHEMA.to_owned(),
            plan: self.plan.clone(),
            drained_instances: self.drained_instances.clone(),
            canceled_work: self.canceled_work,
            unknown_work: self.unknown_work,
            removed_together: self.removed_together.clone(),
        }
    }

    /// Reclaim this version's managed bytes, after releasing what it registered.
    ///
    /// Three things happen in this order, and the order is the point:
    ///
    /// 1. **Refuse while an instance is still running.** Draining is what makes
    ///    deletion safe, and this check notices a caller who drained a *different*
    ///    set of instances.
    /// 2. **Release every registration the record names**, through the owner
    ///    module for that surface. The package's own record says what it created;
    ///    nothing here guesses. A refusal from an owner stops the uninstall with
    ///    the bytes still in place, so a half-removed package is not
    ///    representable: registration entries never outlive the bytes that used
    ///    them and bytes never disappear while an entry still points at them.
    /// 3. **Reclaim this version's managed bytes.**
    ///
    /// User data is not on this path at all: history, credentials and protocol
    /// state are preserved, and clearing them is the separate
    /// [`purge_user_data`] operation.
    pub fn collect(
        self,
        store: &PackageStore,
        registry: &InstanceRegistry,
        owners: &dyn RegistrationOwners,
    ) -> Result<UninstallOutcome, ApplicationFailure> {
        let still_running: Vec<String> = registry
            .instances()
            .filter(|machine| {
                machine.identity().package_id == self.plan.package_id
                    || self
                        .removed_together
                        .contains(&machine.identity().package_id)
            })
            .filter(|machine| machine.is_running())
            .map(|machine| machine.identity().instance_id.clone())
            .collect();
        if !still_running.is_empty() {
            return Err(refusal("package_instance_still_active", UNINSTALL_STAGE)
                .with_field("packageId")
                .with_presentation_arg("package", self.plan.package_id.as_str())
                .with_presentation_arg(
                    "instance",
                    still_running
                        .first()
                        .map(String::as_str)
                        .unwrap_or("unknown"),
                ));
        }

        // The registrations are released before a single managed byte moves.
        let released_registrations = owners.release_all(&self.plan.registrations)?;

        let installed = store.installed_version(&self.plan.package_id, &self.plan.version)?;
        let user_runtime_kept = installed
            .as_ref()
            .is_none_or(|record| keeps_user_runtime(record.runtime_ref.as_deref()));
        let removed = store.remove_installed(&self.plan.package_id, &self.plan.version)?;

        Ok(UninstallOutcome {
            package_id: self.plan.package_id.clone(),
            version: self.plan.version.clone(),
            reclaimed_bytes: removed.reclaimed_bytes(),
            drained_instances: self.drained_instances,
            canceled_work: self.canceled_work,
            unknown_work: self.unknown_work,
            preserved: self.plan.preserved,
            shared_runtime_retained: self.plan.shared_runtime_ref.clone(),
            user_runtime_kept,
            removed_together: self.removed_together,
            released_registrations,
        })
    }
}

/// The schema of the drained handoff record.
pub const DRAINED_RECORD_SCHEMA: &str = "licoup.package-uninstall-drained.v1";

/// The longest drained handoff record accepted.
pub const MAX_DRAINED_RECORD_BYTES: usize = 64 * 1024;

/// A drained uninstall decision, written down so it survives one process.
///
/// It carries the plan and what the drain observed. It does not carry a
/// permission: reading one back is evidence about a decision that was taken, not
/// authority to reclaim anything, and [`Drained::collect`] still checks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DrainedRecord {
    pub schema: String,
    pub plan: UninstallPlan,
    pub drained_instances: Vec<String>,
    pub canceled_work: u32,
    pub unknown_work: u32,
    pub removed_together: Vec<String>,
}

impl DrainedRecord {
    /// Validate the record before any of it is believed.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.schema != DRAINED_RECORD_SCHEMA {
            return Err(
                refusal("package_uninstall_record_invalid", UNINSTALL_STAGE).with_field("schema")
            );
        }
        if self.plan.package_id.is_empty() || self.plan.version.is_empty() {
            return Err(refusal("package_uninstall_record_invalid", UNINSTALL_STAGE)
                .with_field("packageId"));
        }
        self.plan
            .registrations
            .iter()
            .try_for_each(RecordedRegistration::validate)
    }

    /// Return the drained value this record describes.
    ///
    /// Re-reading a decision is not re-authorizing it: `collect` re-checks the
    /// running set and the store before anything is removed.
    pub fn resume(self) -> Result<Drained, ApplicationFailure> {
        self.validate()?;
        Ok(Drained {
            plan: self.plan,
            drained_instances: self.drained_instances,
            canceled_work: self.canceled_work,
            unknown_work: self.unknown_work,
            removed_together: self.removed_together,
        })
    }
}

/// Where the drained handoff for one version lives inside the store.
///
/// It sits outside `records/`, so the installed-version reader can never mistake
/// a drain decision for a package record.
pub fn drained_record_path(store: &PackageStore, package_id: &str, version: &str) -> PathBuf {
    store
        .root()
        .join("uninstall")
        .join(format!("{package_id}@{version}.json"))
}

/// Write down one drained decision.
pub fn write_drained_record(
    store: &PackageStore,
    drained: &Drained,
) -> Result<PathBuf, ApplicationFailure> {
    let record = drained.record();
    let text = serde_json::to_string_pretty(&record).map_err(|_| {
        refusal("package_uninstall_record_invalid", UNINSTALL_STAGE).with_field("record")
    })?;
    let path = drained_record_path(store, &record.plan.package_id, &record.plan.version);
    replace_file_atomically(&path, &text)?;
    Ok(path)
}

/// Read back the drained decision for one version.
///
/// An absent record is a refusal and not an empty drain: reclaiming bytes without
/// a recorded drain would be exactly the "uninstall while a generation is still
/// serving" path the transaction exists to make unrepresentable.
pub fn read_drained_record(
    store: &PackageStore,
    package_id: &str,
    version: &str,
) -> Result<DrainedRecord, ApplicationFailure> {
    let path = drained_record_path(store, package_id, version);
    let Some(text) = read_bounded_text(&path, MAX_DRAINED_RECORD_BYTES)? else {
        return Err(refusal("package_uninstall_not_drained", UNINSTALL_STAGE)
            .with_field("packageId")
            .with_presentation_arg("package", package_id));
    };
    serde_json::from_str(&text).map_err(|_| {
        refusal("package_uninstall_record_invalid", UNINSTALL_STAGE).with_field("record")
    })
}

/// Remove the drained handoff once its collect succeeded.
pub fn clear_drained_record(
    store: &PackageStore,
    package_id: &str,
    version: &str,
) -> Result<(), ApplicationFailure> {
    let path = drained_record_path(store, package_id, version);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(
            refusal("package_uninstall_record_unavailable", UNINSTALL_STAGE).with_field("record"),
        ),
    }
}

/// What one uninstall did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallOutcome {
    pub package_id: String,
    pub version: String,
    /// Managed bytes actually removed.
    pub reclaimed_bytes: u64,
    pub drained_instances: Vec<String>,
    /// Units of work cancelled by the user's choice.
    pub canceled_work: u32,
    /// Units of work whose outcome is now `Unknown`. They are recorded, never
    /// re-dispatched.
    pub unknown_work: u32,
    pub preserved: PreservedFacts,
    /// A shared runtime that was retained because other packages still use it.
    pub shared_runtime_retained: Option<String>,
    /// Whether an interpreter the user installed was left alone.
    pub user_runtime_kept: bool,
    /// Dependent packages the user chose to remove in the same decision.
    pub removed_together: Vec<String>,
    /// What the owning modules reported releasing, in the order they were asked.
    pub released_registrations: Vec<ReleasedRegistration>,
}

/// Removing a package never removes an interpreter the user installed.
pub fn keeps_user_runtime(reference: Option<&str>) -> bool {
    reference.is_some_and(|reference| reference.starts_with(USER_RUNTIME_PREFIX))
}

/// A page or panel closed. It is not an uninstall.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceClosure {
    /// The package is still installed; closing a view never removes anything.
    pub package_still_installed: bool,
    /// Instances affected. Always zero: a page holds no package state.
    pub instances_changed: usize,
}

/// Close a surface without touching the package or its instances.
pub fn close_surface(_package_id: &str) -> SurfaceClosure {
    SurfaceClosure {
        package_still_installed: true,
        instances_changed: 0,
    }
}

/// The user data one package may have produced, as a separate operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserDataPurgeRequest {
    pub package_id: String,
    pub history: bool,
    pub credentials: bool,
    pub protocol_state: bool,
}

/// Clear user data for a package.
///
/// This is deliberately not part of uninstall: an uninstall keeps the history and
/// the security facts, and a user who wants them gone asks for that separately.
/// The package must already be uninstalled, and only paths inside the managed
/// `user-data` root are removed.
pub fn purge_user_data(
    user_data_root: &Path,
    request: &UserDataPurgeRequest,
    still_installed: bool,
) -> Result<u64, ApplicationFailure> {
    // The id becomes a path segment below the user-data root, so it is held to
    // the same contract rule the package store applies.
    crate::platform::extension_packages::install::checked_package_id(&request.package_id)?;
    if still_installed {
        return Err(refusal("package_still_installed", UNINSTALL_STAGE)
            .with_field("packageId")
            .with_presentation_arg("package", request.package_id.as_str()));
    }
    if !request.history && !request.credentials && !request.protocol_state {
        return Err(refusal("package_user_data_purge_empty", UNINSTALL_STAGE).with_field("history"));
    }
    let mut reclaimed = 0;
    for (selected, segment) in [
        (request.history, "history"),
        (request.credentials, "credentials"),
        (request.protocol_state, "protocol-state"),
    ] {
        if !selected {
            continue;
        }
        reclaimed += remove_managed_tree(&user_data_root.join(&request.package_id).join(segment))?;
    }
    Ok(reclaimed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::package_registration_release::PackageRegistrationOwners;

    /// The production registration adapter with no caller-supplied release inputs.
    ///
    /// These scenarios record no external registration, so every owner is asked for
    /// nothing and the reclaim proceeds. One shared value keeps a `'static` reference
    /// available without each test naming or rebuilding the adapter.
    static OWNERS: std::sync::LazyLock<PackageRegistrationOwners> =
        std::sync::LazyLock::new(PackageRegistrationOwners::default);

    fn owners() -> &'static PackageRegistrationOwners {
        &OWNERS
    }

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
    use crate::platform::extension_packages::artifact::{ArtifactLimits, content_digest};
    use crate::platform::extension_packages::install::InstallRequest;
    use crate::platform::extension_packages::state::{
        InstanceIdentity, InstanceMachine, TrustRecord,
    };
    use crate::platform::extension_packages::{ensure_private_directory, unique_suffix};
    use licoup_extension_contracts::deployment::{PackageEntry, PackageSource};
    use licoup_extension_contracts::manifest::Dependency;
    use licoup_extension_contracts::manifest::PermissionRequest;
    use licoup_extension_contracts::wire;
    use std::io::Write;
    use std::path::PathBuf;

    fn package_bytes(id: &str, version: &str, runtime_ref: Option<&str>) -> Vec<u8> {
        let mut runtime = serde_json::json!({ "mode": "process", "entry": "agent.py" });
        if let Some(reference) = runtime_ref {
            runtime["runtimeRef"] = serde_json::Value::String(reference.to_owned());
        }
        let manifest = serde_json::json!({
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
            "permissions": [],
            "contributions": [],
        });
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).expect("entry");
        writer
            .write_all(manifest.to_string().as_bytes())
            .expect("manifest");
        writer.start_file("agent.py", options).expect("entry");
        writer
            .write_all(b"print('echo')\n".as_slice())
            .expect("agent");
        writer.finish().expect("finish").into_inner()
    }

    fn install(store: &PackageStore, id: &str, version: &str, runtime_ref: Option<&str>) {
        let bytes = package_bytes(id, version, runtime_ref);
        let trust = TrustRecord::local_approved(
            content_digest(&bytes),
            [PermissionRequest::new("example.specialist/net", "self")],
        )
        .expect("trust");
        let request = InstallRequest::new(id, version, PackageSource::LocalImport, trust)
            .with_limits(ArtifactLimits::default());
        store.install(&request, &bytes).expect("install");
    }

    fn active_instance(id: &str, generation: u64, registry: &mut InstanceRegistry) -> String {
        let identity = InstanceIdentity::new(
            format!("instance-{generation}"),
            id,
            "1.0.0",
            generation,
            3,
            Vec::<String>::new(),
        )
        .expect("identity");
        let mut machine = InstanceMachine::discovered(identity).expect("instance");
        machine
            .advance(InstanceLifecycle::Preparing)
            .expect("prepare");
        machine.advance(InstanceLifecycle::Active).expect("active");
        let instance_id = machine.identity().instance_id.clone();
        registry.insert(machine);
        instance_id
    }

    fn store(tag: &str) -> (PathBuf, PackageStore) {
        let root =
            std::env::temp_dir().join(format!("licoup-pkg-uninstall-{tag}-{}", unique_suffix()));
        let store = PackageStore::open(&root).expect("store");
        (root, store)
    }

    #[test]
    fn an_uninstall_withdraws_admission_before_it_drains_anything() {
        let (root, store) = store("order");
        install(&store, "example.specialist.echo", "1.0.0", None);
        let installed = store.installed().expect("installed").remove(0);
        let mut registry = InstanceRegistry::new();
        let instance_id = active_instance("example.specialist.echo", 1, &mut registry);
        let catalogue = LocalCatalogue::new();

        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");
        let transaction =
            UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
                .expect("begin");
        assert_eq!(
            transaction.withdrawn_instances(),
            std::slice::from_ref(&instance_id)
        );
        assert_eq!(
            registry.get(&instance_id).expect("instance").admission(),
            crate::platform::extension_packages::Admission::Withdrawn,
            "admission closes at begin"
        );
        assert_eq!(
            registry.get(&instance_id).expect("instance").state(),
            InstanceLifecycle::Active,
            "and the instance is still running its admitted work"
        );

        let drained = transaction
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain");
        let outcome = drained
            .collect(&store, &registry, owners())
            .expect("collect");
        assert!(outcome.reclaimed_bytes > 0);
        assert_eq!(outcome.preserved, PreservedFacts::all_kept());
        assert!(store.installed().expect("installed").is_empty());
        assert!(
            !store
                .installed_path("example.specialist.echo", "1.0.0")
                .exists()
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn work_in_flight_is_waited_for_or_cancelled_but_never_hidden() {
        let (root, store) = store("inflight");
        install(&store, "example.specialist.echo", "1.0.0", None);
        let installed = store.installed().expect("installed").remove(0);
        let mut registry = InstanceRegistry::new();
        let instance_id = active_instance("example.specialist.echo", 1, &mut registry);
        registry
            .get_mut(&instance_id)
            .expect("instance")
            .begin_in_flight()
            .expect("work");

        let catalogue = LocalCatalogue::new();
        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");
        assert_eq!(plan.in_flight, 1);

        let transaction = UninstallTransaction::begin(
            &mut registry,
            plan.clone(),
            DependentsDecision::SelectedOnly,
        )
        .expect("begin");
        let failure = transaction
            .drain(&mut registry, RemainingWork::Wait)
            .expect_err("wait refuses");
        assert_eq!(failure.code, "package_uninstall_in_flight");
        assert!(
            store
                .installed_path("example.specialist.echo", "1.0.0")
                .join("agent.py")
                .exists(),
            "a refused drain removes nothing"
        );

        let transaction =
            UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
                .expect("begin again");
        let drained = transaction
            .drain(&mut registry, RemainingWork::Cancel)
            .expect("cancel");
        let outcome = drained
            .collect(&store, &registry, owners())
            .expect("collect");
        assert_eq!(outcome.canceled_work, 1);
        assert_eq!(
            outcome.unknown_work, 1,
            "cancelled work is Unknown, not success"
        );
        assert_eq!(registry.get(&instance_id).expect("instance").unknown(), 1);
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_dependent_package_stops_the_uninstall_until_the_user_decides() {
        let (root, store) = store("dependents");
        install(&store, "example.specialist.echo", "1.0.0", None);
        let installed = store.installed().expect("installed").remove(0);
        let mut registry = InstanceRegistry::new();
        let mut catalogue = LocalCatalogue::new();
        catalogue.insert(
            PackageEntry::new("example.host.panel", "1.0.0", PackageSource::LocalImport)
                .requiring([Dependency::new("example.specialist.echo", "^1")]),
        );

        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");
        assert!(plan.needs_user_choice());
        assert_eq!(
            plan.reverse_dependencies,
            vec!["example.host.panel".to_owned()]
        );
        assert!(
            plan.explanation()
                .iter()
                .any(|line| line == "example.host.panel requires example.specialist.echo")
        );

        let failure = UninstallTransaction::begin(
            &mut registry,
            plan.clone(),
            DependentsDecision::SelectedOnly,
        )
        .expect_err("no silent cascade");
        assert_eq!(failure.code, "package_uninstall_has_dependents");
        assert!(
            store
                .installed_path("example.specialist.echo", "1.0.0")
                .exists()
        );

        let unknown = UninstallTransaction::begin(
            &mut registry,
            plan.clone(),
            DependentsDecision::RemoveTogether(vec!["example.nobody".to_owned()]),
        )
        .expect_err("an unnamed dependent");
        assert_eq!(unknown.code, "package_uninstall_dependent_unknown");

        let transaction = UninstallTransaction::begin(
            &mut registry,
            plan,
            DependentsDecision::RemoveTogether(vec!["example.host.panel".to_owned()]),
        )
        .expect("explicit decision");
        let drained = transaction
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain");
        let outcome = drained
            .collect(&store, &registry, owners())
            .expect("collect");
        assert_eq!(
            outcome.removed_together,
            vec!["example.host.panel".to_owned()]
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_still_running_instance_blocks_the_reclaim() {
        let (root, store) = store("still-active");
        install(&store, "example.specialist.echo", "1.0.0", None);
        let installed = store.installed().expect("installed").remove(0);
        let mut registry = InstanceRegistry::new();
        active_instance("example.specialist.echo", 1, &mut registry);
        let catalogue = LocalCatalogue::new();
        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");

        let transaction =
            UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
                .expect("begin");
        let drained = transaction
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain");
        // A new instance appearing between the drain and the collection must not
        // be able to have its package deleted underneath it.
        active_instance("example.specialist.echo", 2, &mut registry);
        let failure = drained
            .collect(&store, &registry, owners())
            .expect_err("instance still running");
        assert_eq!(failure.code, "package_instance_still_active");
        assert!(
            store
                .installed_path("example.specialist.echo", "1.0.0")
                .exists()
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn closing_a_page_is_not_uninstalling_a_package() {
        let (root, store) = store("surface");
        install(&store, "example.specialist.echo", "1.0.0", None);
        let mut registry = InstanceRegistry::new();
        let instance_id = active_instance("example.specialist.echo", 1, &mut registry);

        let closure = close_surface("example.specialist.echo");
        assert!(closure.package_still_installed);
        assert_eq!(closure.instances_changed, 0);
        assert_eq!(store.installed().expect("installed").len(), 1);
        assert_eq!(
            registry.get(&instance_id).expect("instance").state(),
            InstanceLifecycle::Active
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_user_installed_runtime_is_never_the_package_managers_to_remove() {
        assert!(keeps_user_runtime(Some("user:python3")));
        assert!(!keeps_user_runtime(Some("runtime.node-22")));
        assert!(!keeps_user_runtime(None));

        let (root, store) = store("user-runtime");
        install(
            &store,
            "example.specialist.echo",
            "1.0.0",
            Some("user:python3"),
        );
        let installed = store.installed().expect("installed").remove(0);
        let mut registry = InstanceRegistry::new();
        let catalogue = LocalCatalogue::new();
        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");
        assert_eq!(
            plan.shared_runtime_ref, None,
            "a user runtime is not a shared runtime the host may release"
        );
        let drained =
            UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
                .expect("begin")
                .drain(&mut registry, RemainingWork::Wait)
                .expect("drain");
        let outcome = drained
            .collect(&store, &registry, owners())
            .expect("collect");
        assert!(outcome.user_runtime_kept);
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_shared_runtime_is_retained_and_named_in_the_preview() {
        let (root, store) = store("shared-runtime");
        install(
            &store,
            "example.specialist.echo",
            "1.0.0",
            Some("runtime.node-22"),
        );
        let installed = store.installed().expect("installed").remove(0);
        let registry = InstanceRegistry::new();
        let catalogue = LocalCatalogue::new();
        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");
        assert_eq!(plan.shared_runtime_ref, Some("runtime.node-22".to_owned()));
        assert!(
            plan.explanation()
                .iter()
                .any(|line| line.contains("shared runtime runtime.node-22"))
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn clearing_user_data_is_a_separate_explicit_operation() {
        let root = std::env::temp_dir().join(format!("licoup-pkg-userdata-{}", unique_suffix()));
        let user_data = root.join("user-data");
        ensure_private_directory(&user_data.join("example.specialist.echo").join("history"))
            .expect("history");
        ensure_private_directory(
            &user_data
                .join("example.specialist.echo")
                .join("credentials"),
        )
        .expect("credentials");
        std::fs::write(
            user_data
                .join("example.specialist.echo")
                .join("history")
                .join("turns.jsonl"),
            vec![1u8; 128],
        )
        .expect("write");

        let request = UserDataPurgeRequest {
            package_id: "example.specialist.echo".to_owned(),
            history: true,
            credentials: false,
            protocol_state: false,
        };
        let failure = purge_user_data(&user_data, &request, true).expect_err("still installed");
        assert_eq!(failure.code, "package_still_installed");

        let reclaimed = purge_user_data(&user_data, &request, false).expect("purge");
        assert_eq!(reclaimed, 128);
        assert!(
            user_data
                .join("example.specialist.echo")
                .join("credentials")
                .exists(),
            "an unselected scope survives"
        );
        assert_eq!(
            purge_user_data(
                &user_data,
                &UserDataPurgeRequest {
                    package_id: "example.specialist.echo".to_owned(),
                    history: false,
                    credentials: false,
                    protocol_state: false,
                },
                false,
            )
            .expect_err("nothing selected")
            .code,
            "package_user_data_purge_empty"
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn history_survives_an_uninstall_and_a_reinstall_still_writes_there() {
        let (root, store) = store("history");
        let user_data = root.join("user-data");
        ensure_private_directory(&user_data.join("example.specialist.echo").join("history"))
            .expect("history");
        std::fs::write(
            user_data
                .join("example.specialist.echo")
                .join("history")
                .join("turns.jsonl"),
            vec![2u8; 64],
        )
        .expect("write");

        install(&store, "example.specialist.echo", "1.0.0", None);
        let installed = store.installed().expect("installed").remove(0);
        let mut registry = InstanceRegistry::new();
        let catalogue = LocalCatalogue::new();
        let plan = preview(&store, &catalogue, &installed, &registry).expect("preview");
        UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
            .expect("begin")
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain")
            .collect(&store, &registry, owners())
            .expect("collect");

        assert!(
            user_data
                .join("example.specialist.echo")
                .join("history")
                .join("turns.jsonl")
                .exists(),
            "history is the user's, not the package's"
        );
        install(&store, "example.specialist.echo", "1.0.0", None);
        assert_eq!(store.installed().expect("installed").len(), 1);
        remove_managed_tree(&root).expect("cleanup");
    }
}
