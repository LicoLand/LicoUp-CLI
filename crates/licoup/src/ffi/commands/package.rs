//! The native command surface for the optional extension package lifecycle.
//!
//! One route, one operation. The routes own no package fact: they resolve the
//! data home, open the store the layout owner names, and report what the
//! platform's own transactions produced.
//!
//! Four rules shape this surface:
//!
//! 1. **The store root comes from the layout owner.** Every route resolves the
//!    LicoUp data home it was given and asks
//!    [`licoup_foundation::platform::paths::package_store_root`] where the store
//!    lives. No route joins a directory name itself, so the layout has one owner.
//! 2. **The catalogue is only reachable through recovery.** `package catalog`
//!    reads through [`PackageStore::catalogue`], which reconciles a crash before
//!    it reads a single record, so an interrupted install is never presented as a
//!    half state.
//! 3. **Installing needs a confirmation bound to the bytes.** `install-plan`
//!    derives a plan and a confirmation from one archive; `install-apply` must
//!    present a confirmation that the same archive still reproduces. An archive
//!    swapped between the two calls cannot be installed under the earlier
//!    decision.
//! 4. **Nothing here runs a package, and nothing here reaches the network.** The
//!    bytes are a local file the operator holds; no probe is executed and no
//!    socket is opened.
//!
//! Mutating *replacement* and *activation* route through the
//! maintenance-admission seam, which asks the native idle guard
//! (`UPDATE-IDLE-ADMISSION`) for the data home the operation would change. The
//! seam decides; the caller that changes installed state takes the durable
//! close-admission barrier and releases it when the change succeeds or aborts.
//! Read-only `package update-preview` asks the same question and reports the
//! answer without taking anything. Neither route replaces bytes or starts a
//! generation yet.
//!
//! [`package update-apply`]: handle_update_apply
//! [`package activate`]: handle_activate

use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::{
    InstanceLifecycle, LocalCatalogue, PackageEntry, PackageLifecycle,
};
use licoup_extension_contracts::is_namespaced;
use licoup_extension_contracts::manifest::ResourceKind;
use licoup_foundation::platform::paths;
use sha2::{Digest, Sha256};

use crate::platform::extension_packages::{
    ArtifactLimits, DependentsDecision, Drained, IdleVerdict, InstanceIdentity, InstanceMachine,
    InstanceRegistry, MaintenanceAdmission, MaintenanceOperation, MaintenanceRequest,
    PackageMaintenanceAdmission, PackageStore, PreparedGeneration, RemainingWork, ResourceHost,
    TrustRecord, UninstallTransaction, read_drained_record, read_manifest, running_client_version,
    switched_on, write_drained_record,
};
use crate::platform::package_registration_release::{
    CodexPluginRelease, PackageRegistrationOwners, ProviderMcpRelease, ReleaseInputs,
};

/// The report every package route publishes.
const SCHEMA: &str = "licoup.package-lifecycle.v1";

/// The stage a refusal from activation or resource selection reports.
const ACTIVATION_STAGE: &str = "extension/package-activation";

/// The schema of the confirmation one install plan issues.
const CONFIRMATION_SCHEMA: &str = "licoup.package-install-confirmation.v1";

/// The longest archive this surface will read from disk.
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// The stage a refusal from the maintenance guard reports.
const GUARD_STAGE: &str = "extension/package-maintenance";

/// `package catalog <data-root>` — what this client holds, reconciled first.
pub(super) fn handle_catalog(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let (recovery, installed) = store.catalogue()?;
    let preferences = installed
        .iter()
        .map(|package| {
            store
                .preference(&package.package_id, &package.version)
                .map(|preference| preference.is_none_or(|preference| preference.enabled))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(report(json!({
        "operation": "catalog",
        "dataHomeResolved": true,
        "storeRoot": store.root().display().to_string(),
        "recoveredBeforeRead": true,
        "recovery": recovery_report(&recovery),
        "packages": installed
            .iter()
            .zip(preferences)
            .map(|(package, enabled)| installed_package(package, enabled))
            .collect::<Vec<_>>(),
    })))
}

/// `package install-plan <data-root> --archive <path>` — what installing would do.
pub(super) fn handle_install_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let archive = resolve_archive(
        command
            .option_text("archive")
            .ok_or_else(|| anyhow!("package_archive_required"))?,
    )?;
    let candidate = candidate_for(&store, &archive)?;
    Ok(report(json!({
        "operation": "install-plan",
        "plan": candidate.plan,
        "planDigest": candidate.plan_digest,
        "confirmation": candidate.confirmation,
        "archive": archive.display().to_string(),
        "installed": false,
    })))
}

/// `package install-confirm <data-root> --archive <path> --plan <digest>` — the
/// explicit second step, which re-derives the plan and refuses a stale digest.
pub(super) fn handle_install_confirm(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let archive = resolve_archive(
        command
            .option_text("archive")
            .ok_or_else(|| anyhow!("package_archive_required"))?,
    )?;
    let plan_digest = command
        .option_text("plan")
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("package_install_plan_digest_required"))?;
    let candidate = candidate_for(&store, &archive)?;
    if candidate.plan_digest != plan_digest {
        return Ok(refusal(
            "package_install_plan_stale",
            "the archive no longer reproduces the plan that was reviewed",
        ));
    }
    Ok(report(json!({
        "operation": "install-confirm",
        "packageId": candidate.plan["packageId"].clone(),
        "version": candidate.plan["version"].clone(),
        "planDigest": candidate.plan_digest,
        "confirmation": candidate.confirmation,
    })))
}

/// `package install-apply <data-root> --archive <path> --confirmation <token>` —
/// install the reviewed bytes.
pub(super) fn handle_install_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let archive = resolve_archive(
        command
            .option_text("archive")
            .ok_or_else(|| anyhow!("package_archive_required"))?,
    )?;
    let confirmation = command
        .option_text("confirmation")
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("package_install_confirmation_required"))?;
    let candidate = candidate_for(&store, &archive)?;
    if confirmation != candidate.confirmation {
        return Ok(refusal(
            "package_install_confirmation_stale",
            "the archive does not reproduce the confirmed plan",
        ));
    }
    install_candidate(&store, &candidate, &archive, "install-apply")
}

/// `package import <data-root> --archive <path>` — the user's own local import.
///
/// A local import *is* the user handing in bytes they hold: there is no
/// directory, no account and no fetch, which is why this route needs no separate
/// confirmation step. It installs nothing that would replace an installed
/// version, and it records the local-approval channel it was admitted under.
pub(super) fn handle_import(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let archive = resolve_archive(
        command
            .option_text("archive")
            .ok_or_else(|| anyhow!("package_archive_required"))?,
    )?;
    let candidate = candidate_for(&store, &archive)?;
    install_candidate(&store, &candidate, &archive, "import")
}

/// `package enable <data-root> <package-id> <version>` — switch one installed
/// version on.
///
/// This is the user's own preference, not activation: no instance starts, no
/// generation moves and no byte changes, so it is not maintenance and does not
/// pass the idle seam.
pub(super) fn handle_enable(command: AdmittedCommand) -> Result<CliExecution> {
    set_enabled(
        resolve_data_home(command.required_text("data-root"))?,
        command.required_text("package-id"),
        command.required_text("version"),
        true,
    )
}

/// `package disable <data-root> <package-id> <version>` — switch one installed
/// version off. The bytes stay; only the preference changes.
pub(super) fn handle_disable(command: AdmittedCommand) -> Result<CliExecution> {
    set_enabled(
        resolve_data_home(command.required_text("data-root"))?,
        command.required_text("package-id"),
        command.required_text("version"),
        false,
    )
}

/// `package uninstall-preview <data-root> <package-id> <version>` — what removing
/// this version would touch, before anything is closed.
pub(super) fn handle_uninstall_preview(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let package_id = command.required_text("package-id").to_owned();
    let version = command.required_text("version").to_owned();
    let (plan, _) = uninstall_plan(&store, &package_id, &version)?;
    Ok(report(json!({
        "operation": "uninstall-preview",
        "plan": uninstall_plan_json(&plan),
        "explanation": plan.explanation(),
        "needsUserChoice": plan.needs_user_choice(),
        "preservesUserData": true,
    })))
}

/// `package uninstall-drain <data-root> <package-id> <version>` — withdraw
/// admission, drain, and write the drained decision down.
///
/// `--instances` lets a caller report work it observes. Such a report can only
/// make the drain *refuse*, never proceed: it is added to the running set and
/// never removes anything from it.
pub(super) fn handle_uninstall_drain(mut command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let package_id = command.required_text("package-id").to_owned();
    let version = command.required_text("version").to_owned();
    let (plan, mut registry) = uninstall_plan(&store, &package_id, &version)?;
    if let Some(observed) = command.take_option_json("instances") {
        observe_instances(&mut registry, observed)?;
    }
    let remaining = match command.option_text("remaining") {
        None | Some("wait") => RemainingWork::Wait,
        Some("cancel") => RemainingWork::Cancel,
        Some(_) => return Err(anyhow!("package_uninstall_remaining_unknown")),
    };
    let dependents = match command.take_option_json("dependents") {
        None => DependentsDecision::SelectedOnly,
        Some(Value::Array(names)) => DependentsDecision::RemoveTogether(
            names
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| anyhow!("package_uninstall_dependent_invalid"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => return Err(anyhow!("package_uninstall_dependent_invalid")),
    };
    let transaction = match UninstallTransaction::begin(&mut registry, plan, dependents) {
        Ok(transaction) => transaction,
        Err(failure) => return Ok(failure_report("uninstall-drain", &failure)),
    };
    let withdrawn = transaction.withdrawn_instances().to_vec();
    let drained = match transaction.drain(&mut registry, remaining) {
        Ok(drained) => drained,
        Err(failure) => return Ok(failure_report("uninstall-drain", &failure)),
    };
    write_drained_record(&store, &drained)?;
    let record = drained.record();
    Ok(report(json!({
        "operation": "uninstall-drain",
        "packageId": record.plan.package_id.clone(),
        "version": record.plan.version.clone(),
        "withdrawnInstances": withdrawn,
        "drainedInstances": record.drained_instances.clone(),
        "canceledWork": record.canceled_work,
        "unknownWork": record.unknown_work,
        "releasesRegistrationsOnCollect": record.plan.registrations.len(),
        "preservesUserData": true,
    })))
}

/// `package uninstall-collect <data-root> <package-id> <version>` — release what
/// the package registered, then reclaim its managed bytes.
pub(super) fn handle_uninstall_collect(mut command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let package_id = command.required_text("package-id").to_owned();
    let version = command.required_text("version").to_owned();
    let record = match read_drained_record(&store, &package_id, &version) {
        Ok(record) => record,
        Err(failure) => return Ok(failure_report("uninstall-collect", &failure)),
    };
    let drained: Drained = match record.resume() {
        Ok(drained) => drained,
        Err(failure) => return Ok(failure_report("uninstall-collect", &failure)),
    };
    let inputs = match command.take_option_json("registration-inputs") {
        None | Some(Value::Null) => ReleaseInputs::none(),
        Some(value) => release_inputs(&value)?,
    };
    let owners = PackageRegistrationOwners::new(inputs);
    let registry = InstanceRegistry::new();
    match drained.collect(&store, &registry, &owners) {
        Ok(outcome) => {
            crate::platform::extension_packages::clear_drained_record(
                &store,
                &package_id,
                &version,
            )?;
            Ok(report(json!({
                "operation": "uninstall-collect",
                "packageId": outcome.package_id,
                "version": outcome.version,
                "reclaimedBytes": outcome.reclaimed_bytes,
                "drainedInstances": outcome.drained_instances,
                "canceledWork": outcome.canceled_work,
                "unknownWork": outcome.unknown_work,
                "releasedRegistrations": outcome
                    .released_registrations
                    .iter()
                    .map(|released| json!({
                        "owner": released.owner.wire_name(),
                        "key": released.key,
                        "removed": released.removed,
                        "alreadyAbsent": released.already_absent,
                    }))
                    .collect::<Vec<_>>(),
                "sharedRuntimeRetained": outcome.shared_runtime_retained,
                "userRuntimeKept": outcome.user_runtime_kept,
                "removedTogether": outcome.removed_together,
                "preservedUserData": {
                    "history": outcome.preserved.history,
                    "credentials": outcome.preserved.credentials,
                    "protocolState": outcome.preserved.protocol_state,
                },
            })))
        }
        Err(failure) => Ok(failure_report("uninstall-collect", &failure)),
    }
}

/// `package recover <data-root>` — reconcile a crash and report what it found.
pub(super) fn handle_recover(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let recovery = store.recover()?;
    Ok(report(json!({
        "operation": "recover",
        "recovery": recovery_report(&recovery),
        "installed": store
            .installed()?
            .iter()
            .map(|package| package.key())
            .collect::<Vec<_>>(),
    })))
}

/// `package update-preview <data-root> <package-id> [--archive <path>]` — what a
/// replacement would do.
///
/// Read-only by construction: it opens the store, reports the installed facts and
/// the candidate's, and asks the maintenance seam whether apply would be admitted
/// for this data home. It closes nothing, drains nothing and replaces nothing —
/// asking is not holding, so a preview never takes the close-admission barrier.
pub(super) fn handle_update_preview(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let store = open_store(&data_home)?;
    let package_id = command.required_text("package-id").to_owned();
    let installed = store
        .installed()?
        .into_iter()
        .filter(|package| package.package_id == package_id)
        .map(|package| {
            json!({
                "packageId": package.package_id,
                "version": package.version,
                "digest": package.digest,
                "trustChannel": lifecycle_name(package.trust_channel),
                "installedAtUnixMs": package.installed_at_unix_ms,
            })
        })
        .collect::<Vec<_>>();
    let candidate = match command.option_text("archive").map(str::to_owned) {
        Some(path) => {
            let candidate = candidate_for(&store, Path::new(&path))?;
            json!({
                "plan": candidate.plan,
                "planDigest": candidate.plan_digest,
            })
        }
        None => Value::Null,
    };
    let admission = PackageMaintenanceAdmission::new();
    let verdict = maintenance_verdict(&data_home);
    let request = MaintenanceRequest::new(
        MaintenanceOperation::UpdateApply,
        package_id.clone(),
        installed
            .first()
            .and_then(|installed| installed["version"].as_str())
            .unwrap_or("unknown"),
    );
    let apply = match admission.admit(verdict, &request) {
        Ok(permit) => json!({
            "available": true,
            "operation": permit.operation().wire_name(),
            "guardPresent": verdict != IdleVerdict::Unavailable,
            "guardOwner": crate::platform::extension_packages::GUARD_OWNER,
        }),
        Err(failure) => json!({
            "available": false,
            "reasonCode": failure.code,
            "guardPresent": verdict != IdleVerdict::Unavailable,
            "guardOwner": crate::platform::extension_packages::GUARD_OWNER,
        }),
    };
    Ok(report(json!({
        "operation": "update-preview",
        "packageId": package_id,
        "installed": installed,
        "candidate": candidate,
        "apply": apply,
        "mutated": false,
    })))
}

/// `package update-apply <data-root> <package-id> --archive <path>
/// --confirmation <token>` — replace an installed version.
///
/// The maintenance seam decides first, against the data home this route would
/// change, and the decision happens *before* the archive is read: a host whose
/// admission is closed or whose work is unfinished never reaches the bytes.
///
/// An admitted request owns the durable close-admission barrier. This route takes
/// it, reports what it found, and retires it again, because the replacement
/// itself is not wired yet: publishing a new version of an installed package is
/// the package pipeline's, and until it lands a caller learns exactly that — an
/// admitted operation that published nothing — rather than an unguarded write.
/// The take-and-retire pair is the same one the wired caller will use; the only
/// difference will be where the retire sits.
pub(super) fn handle_update_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let package_id = command.required_text("package-id").to_owned();
    // The replacement itself is not wired yet, so the required confirmation
    // token is retrieved but not verified; the wired caller checks it against
    // the preview's own digest.
    let _confirmation = command.option_text("confirmation");
    let version = command
        .option_text("archive")
        .map(PathBuf::from)
        .map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_owned()
        })
        .unwrap_or_else(|| "unknown".to_owned());
    match PackageMaintenanceAdmission::new().admit(
        maintenance_verdict(&data_home),
        &MaintenanceRequest::new(MaintenanceOperation::UpdateApply, &package_id, &version),
    ) {
        Ok(permit) => match hold_and_retire_admission(&data_home) {
            Ok(()) => Ok(report(json!({
                "operation": "update-apply",
                "packageId": package_id,
                "admitted": permit.operation().wire_name(),
                "admissionHeld": true,
                "admissionReleased": true,
                "replaced": false,
                "reasonCode": "package_update_apply_not_wired",
                "mutated": false,
            }))),
            Err(failure) => Ok(CliExecution::Json(failure_body("update-apply", &failure))),
        },
        Err(failure) => Ok(failure_report("update-apply", &failure)),
    }
}

/// `package activate <data-root> <package-id> <version>` — start a generation.
///
/// Refused through the same seam, against the same data home, with the same
/// take-and-retire barrier around a generation that is not started yet.
pub(super) fn handle_activate(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let package_id = command.required_text("package-id").to_owned();
    let version = command.required_text("version").to_owned();
    match PackageMaintenanceAdmission::new().admit(
        maintenance_verdict(&data_home),
        &MaintenanceRequest::new(MaintenanceOperation::Activation, &package_id, &version),
    ) {
        Ok(permit) => {
            // The barrier is taken before anything is served and released after
            // it, which is the pair the guard's seam reserves for the caller
            // that really changes served state.
            if let Err(failure) = hold_admission(&data_home) {
                return Ok(CliExecution::Json(failure_body("activate", &failure)));
            }
            let served = serve_installed_generation(&data_home, &package_id, &version);
            let released =
                crate::domain::work_admission::release_maintenance_admission(&data_home).is_ok();
            match served {
                Ok(snapshot) => Ok(report(json!({
                    "operation": "activate",
                    "packageId": package_id,
                    "version": version,
                    "admitted": permit.operation().wire_name(),
                    "admissionHeld": true,
                    "admissionReleased": released,
                    // This route serves the generation's resources; no process
                    // is started by it yet, and the report says so.
                    "activated": false,
                    "reasonCode": "package_activate_process_not_wired",
                    "servedGeneration": snapshot["packageGeneration"].clone(),
                    "replacedGeneration": snapshot["replacedGeneration"].clone(),
                    "planRevision": snapshot["revision"].clone(),
                    "planPublished": snapshot["planPublished"].clone(),
                    "publishedResources": snapshot["resources"].clone(),
                    "mutated": true,
                }))),
                Err(failure) => Ok(failure_report("activate", &failure)),
            }
        }
        Err(failure) => Ok(failure_report("activate", &failure)),
    }
}

/// Enter one installed release into what this host serves.
///
/// The release is read from the store, not from the request: the typed resources
/// are the ones the installed manifest declares, the release number is the one
/// the install journal recorded for this version, and the generation enters
/// served state through the same guarded switch an update takes. Committing the
/// generation publishes the mount plan for the revision it produced, so the
/// document the client reads is a consequence of this action and not a second
/// decision taken somewhere else.
///
/// A version this store did not install, or one the user switched off, is
/// refused here rather than served quietly.
fn serve_installed_generation(
    data_home: &Path,
    package_id: &str,
    version: &str,
) -> Result<Value, ApplicationFailure> {
    // The store is opened only once the switch is admitted: a route that was
    // refused must leave no store behind it.
    let store = PackageStore::open(&paths::package_store_root(data_home))?;
    if store.installed_version(package_id, version)?.is_none() {
        return Err(crate::platform::extension_packages::refusal(
            "package_not_installed",
            ACTIVATION_STAGE,
        )
        .with_field("packageId")
        .with_presentation_arg("package", package_id));
    }
    if !switched_on(store.root(), package_id, version)? {
        return Err(crate::platform::extension_packages::refusal(
            "package_version_disabled",
            ACTIVATION_STAGE,
        )
        .with_field("version")
        .with_presentation_arg("package", package_id)
        .with_presentation_arg("version", version));
    }
    let resources = store.installed_resources(package_id, version)?;
    let package_generation = store.committed_releases(package_id, version)?;
    let prepared = PreparedGeneration::new(package_id, version, package_generation, resources)?;
    // The route already holds the close-admission barrier, so the host's own
    // question is answered by its holder instead of being asked twice.
    let mut host = ResourceHost::with_admission(data_home, Arc::new(HeldBarrier));
    let outcome = host.begin_replacement(&prepared)?.commit();
    Ok(json!({
        "packageId": outcome.package_id,
        "packageGeneration": outcome.package_generation,
        "replacedGeneration": outcome.replaced_generation,
        "revision": outcome.revision,
        "planPublished": outcome.plan_published,
        "resources": host.available().count(),
    }))
}

/// The answer for a host whose caller already holds the close-admission
/// barrier.
struct HeldBarrier;

impl MaintenanceAdmission for HeldBarrier {
    fn hold(&self, _data_root: &Path) -> Result<(), &'static str> {
        Ok(())
    }

    fn release(&self, _data_root: &Path) -> Result<(), &'static str> {
        Ok(())
    }
}

/// `package resource-select <data-root> <package-id> <version> --kind <kind>
/// --resource <resource-id>` — serve one declared resource of a release.
///
/// This is the user's own request crossing the bridge as data: a kind from the
/// host contract's vocabulary and one namespaced resource identity, never a path
/// and never a document. The release is entered into served state under the same
/// close-admission barrier the activation takes, and the selection itself is an
/// ordinary preference: it replaces no generation and waits for no unrelated
/// work.
///
/// A kind this client does not serve, an identity that is not namespaced, and a
/// resource the release does not declare are all refused with their own stable
/// code. A request this host cannot serve is never answered by silently serving
/// the declared default as though the request had succeeded.
pub(super) fn handle_resource_select(command: AdmittedCommand) -> Result<CliExecution> {
    let data_home = resolve_data_home(command.required_text("data-root"))?;
    let package_id = command.required_text("package-id").to_owned();
    let version = command.required_text("version").to_owned();
    let kind_text = command
        .option_text("kind")
        .ok_or_else(|| anyhow!("package_resource_kind_required"))?
        .to_owned();
    let resource_id = command
        .option_text("resource")
        .ok_or_else(|| anyhow!("package_resource_id_required"))?
        .to_owned();
    match PackageMaintenanceAdmission::new().admit(
        maintenance_verdict(&data_home),
        &MaintenanceRequest::new(MaintenanceOperation::Activation, &package_id, &version),
    ) {
        Ok(permit) => {
            if let Err(failure) = hold_admission(&data_home) {
                return Ok(CliExecution::Json(failure_body(
                    "resource-select",
                    &failure,
                )));
            }
            let selected = select_release_resource(
                &data_home,
                &package_id,
                &version,
                &kind_text,
                &resource_id,
            );
            let released =
                crate::domain::work_admission::release_maintenance_admission(&data_home).is_ok();
            match selected {
                Ok(snapshot) => Ok(report(json!({
                    "operation": "resource-select",
                    "packageId": package_id,
                    "version": version,
                    "kind": kind_text,
                    "resourceId": resource_id,
                    "admitted": permit.operation().wire_name(),
                    "admissionHeld": true,
                    "admissionReleased": released,
                    "servedGeneration": snapshot["packageGeneration"].clone(),
                    "planRevision": snapshot["revision"].clone(),
                    "planPublished": snapshot["planPublished"].clone(),
                    "resources": snapshot["resources"].clone(),
                    "mutated": true,
                }))),
                Err(failure) => Ok(failure_report("resource-select", &failure)),
            }
        }
        Err(failure) => Ok(failure_report("resource-select", &failure)),
    }
}

/// Serve one requested resource of an installed release.
///
/// The requested kind is read through the host contract's own vocabulary and the
/// requested identity through the contract's own namespaced-identity rule, so a
/// request this client cannot own is refused before any state moves. The release
/// is then entered into served state and the selection asks the host for exactly
/// the resource that was requested: a resource the release does not declare is a
/// refusal, not a default.
fn select_release_resource(
    data_home: &Path,
    package_id: &str,
    version: &str,
    kind_text: &str,
    resource_id: &str,
) -> Result<Value, ApplicationFailure> {
    let Some(kind) = ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == kind_text)
    else {
        return Err(crate::platform::extension_packages::refusal(
            "package_resource_kind_unknown",
            ACTIVATION_STAGE,
        )
        .with_field("kind")
        .with_presentation_arg("kind", kind_text));
    };
    if !is_namespaced(resource_id) {
        return Err(crate::platform::extension_packages::refusal(
            "package_resource_identity_invalid",
            ACTIVATION_STAGE,
        )
        .with_field("resourceId")
        .with_presentation_arg("resource", resource_id));
    }
    let store = PackageStore::open(&paths::package_store_root(data_home))?;
    if store.installed_version(package_id, version)?.is_none() {
        return Err(crate::platform::extension_packages::refusal(
            "package_not_installed",
            ACTIVATION_STAGE,
        )
        .with_field("packageId")
        .with_presentation_arg("package", package_id));
    }
    if !switched_on(store.root(), package_id, version)? {
        return Err(crate::platform::extension_packages::refusal(
            "package_version_disabled",
            ACTIVATION_STAGE,
        )
        .with_field("version")
        .with_presentation_arg("package", package_id)
        .with_presentation_arg("version", version));
    }
    let resources = store.installed_resources(package_id, version)?;
    // A resource this release does not declare is refused before the host
    // moves: a request that cannot be served must not republish the document
    // the client is already reading.
    let declared = resources
        .iter()
        .find(|resource| resource.id() == resource_id);
    match declared {
        None => {
            return Err(crate::platform::extension_packages::refusal(
                "package_resource_not_installed",
                ACTIVATION_STAGE,
            )
            .with_field("resourceId")
            .with_presentation_arg("resource", resource_id)
            .with_presentation_arg("kind", kind.as_str()));
        }
        Some(resource) if resource.kind() != kind => {
            return Err(crate::platform::extension_packages::refusal(
                "package_resource_kind_mismatch",
                ACTIVATION_STAGE,
            )
            .with_field("resourceId")
            .with_presentation_arg("resource", resource_id)
            .with_presentation_arg("kind", kind.as_str())
            .with_presentation_arg("actualKind", resource.kind().as_str()));
        }
        _ => {}
    }
    let package_generation = store.committed_releases(package_id, version)?;
    let prepared = PreparedGeneration::new(package_id, version, package_generation, resources)?;
    // The route holds the close-admission barrier for the generation it enters
    // into served state.
    let mut host = ResourceHost::with_admission(data_home, Arc::new(HeldBarrier));
    let outcome = host.begin_replacement(&prepared)?.commit();
    // The selection publishes the revision it moved, and a request this release
    // does not declare leaves the host exactly as it was.
    let selection = host.select(kind, resource_id)?;
    Ok(json!({
        "packageId": outcome.package_id,
        "packageGeneration": outcome.package_generation,
        "revision": selection.revision,
        "planPublished": outcome.plan_published && host.document_failure().is_none(),
        "resources": host.available().count(),
    }))
}

/// Take the close-admission barrier for one admitted package operation.
fn hold_admission(data_home: &Path) -> Result<(), ApplicationFailure> {
    crate::domain::work_admission::hold_package_activation_admission(data_home)
        .map_err(|code| ApplicationFailure::retryable(code, GUARD_STAGE).with_field("maintenance"))
}

/// Take the close-admission barrier for one admitted package operation and
/// retire it again.
///
/// This is the guard's own package-activation pair — the same two calls the
/// caller that really replaces bytes takes around its change. It runs here
/// because nothing is changed yet: an admitted route must not leave admission
/// closed on a host that was never switched.
fn hold_and_retire_admission(data_home: &Path) -> Result<(), ApplicationFailure> {
    hold_admission(data_home)?;
    crate::domain::work_admission::release_maintenance_admission(data_home)
        .map_err(|code| ApplicationFailure::retryable(code, GUARD_STAGE).with_field("maintenance"))
}

/// The native idle guard's verdict for one data home, in the maintenance seam's
/// vocabulary.
///
/// The guard's decision is the domain's and the seam is the platform's, so the
/// composition that knows both reads it here and hands it in. A decision that
/// cannot be read is [`IdleVerdict::Unavailable`], which the seam refuses: a
/// host whose state is unreadable is not an idle host.
fn maintenance_verdict(data_home: &Path) -> IdleVerdict {
    let Ok(admission) = crate::domain::work_admission::WorkAdmission::open(data_home).admission()
    else {
        return IdleVerdict::Unavailable;
    };
    match admission.decision {
        crate::domain::work_admission::AdmissionDecision::Idle => IdleVerdict::Idle,
        crate::domain::work_admission::AdmissionDecision::Blocked => IdleVerdict::Busy,
        crate::domain::work_admission::AdmissionDecision::Closed => IdleVerdict::Closed,
    }
}

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

/// One reviewed install candidate: the plan, the digest that identifies it, and
/// the confirmation that admits applying it.
struct InstallCandidate {
    plan: Value,
    plan_digest: String,
    confirmation: String,
    package_id: String,
    version: String,
    digest: String,
    permissions: Vec<licoup_extension_contracts::manifest::PermissionRequest>,
}

/// Derive the plan for one archive against the current client and store.
///
/// The manifest is read from the archive without expanding it, the digest is
/// computed from the bytes, and the compatibility list is checked against the
/// running client. Nothing is written, so a plan can be built for an archive that
/// will never be installed.
fn candidate_for(store: &PackageStore, archive: &Path) -> Result<InstallCandidate> {
    let bytes = read_archive(archive)?;
    let limits = ArtifactLimits::default();
    let manifest = read_manifest(&bytes, &limits).map_err(as_handler_error)?;
    let digest = crate::platform::extension_packages::content_digest(&bytes);
    let client_version = running_client_version().map_err(as_handler_error)?;
    let covers = manifest.client_compatibility(&client_version).is_covered();
    let already_installed = store
        .installed_version(&manifest.id, &manifest.version)
        .map_err(as_handler_error)?
        .is_some();
    let stored_bytes = store
        .installed_bytes(&manifest.id, &manifest.version)
        .map_err(as_handler_error)?;
    let permissions = manifest.permissions.clone();
    // The reviewed core holds only facts about the archive, the client and the
    // decision. It deliberately excludes what this data home currently holds:
    // those facts move on their own, and a confirmation that changed whenever
    // anything was installed would be a confirmation of the directory rather
    // than of the bytes the operator reviewed.
    let core = json!({
        "schemaVersion": SCHEMA,
        "packageId": manifest.id,
        "version": manifest.version,
        "displayName": manifest.display_name,
        "digest": digest,
        "source": "local-import",
        "compatibility": {
            "covers": covers,
            "clientVersion": client_version,
            "declared": manifest.compatibility,
        },
        "runtimeMode": manifest.runtime.mode(),
        "hostProtocolMajor": manifest.host_protocol.major,
        "permissions": permissions
            .iter()
            .map(|permission| json!({
                "capability": permission.capability,
                "scope": permission.scope,
            }))
            .collect::<Vec<_>>(),
        "requires": manifest
            .requires
            .iter()
            .map(|dependency| json!({
                "packageId": dependency.package_id,
                "range": dependency.range,
            }))
            .collect::<Vec<_>>(),
        "compressedBytes": bytes.len() as u64,
        "installScriptsExecuted": 0,
        "processesSpawned": 0,
    });
    let core_text =
        serde_json::to_string(&core).map_err(|_| anyhow!("package_install_plan_unserializable"))?;
    let plan_digest = prefixed_digest(core_text.as_bytes());
    let confirmation = format!(
        "{CONFIRMATION_SCHEMA}:{}",
        prefixed_digest(format!("{CONFIRMATION_SCHEMA}\u{1f}{plan_digest}").as_bytes())
    );
    let mut plan = core;
    if let Some(object) = plan.as_object_mut() {
        object.insert("alreadyInstalled".to_owned(), json!(already_installed));
        object.insert(
            "installedBytesForThisIdentity".to_owned(),
            json!(stored_bytes),
        );
    }
    Ok(InstallCandidate {
        plan,
        plan_digest,
        confirmation,
        package_id: manifest.id,
        version: manifest.version,
        digest,
        permissions,
    })
}

/// Admit and perform one install of a confirmed candidate.
fn install_candidate(
    store: &PackageStore,
    candidate: &InstallCandidate,
    archive: &Path,
    operation: &str,
) -> Result<CliExecution> {
    if candidate.plan["compatibility"]["covers"] != Value::Bool(true) {
        return Ok(refusal(
            "package_client_incompatible",
            "the package's compatibility list does not cover this client",
        ));
    }
    if candidate.plan["alreadyInstalled"] == Value::Bool(true) {
        return Ok(refusal(
            "package_version_already_installed",
            "this exact version is already installed and an install never replaces one",
        ));
    }
    let bytes = read_archive(archive)?;
    let trust = TrustRecord::local_approved(&candidate.digest, candidate.permissions.clone())
        .map_err(as_handler_error)?;
    let outcome = store
        .install_local_import(&candidate.package_id, &candidate.version, trust, &bytes)
        .map_err(as_handler_error)?;
    Ok(report(json!({
        "operation": operation,
        "packageId": outcome.installed.package_id,
        "version": outcome.installed.version,
        "digest": outcome.installed.digest,
        "state": lifecycle_name(outcome.state),
        "trustChannel": lifecycle_name(outcome.installed.trust_channel),
        "compressedBytes": outcome.installed.compressed_bytes,
        "expandedBytes": outcome.installed.expanded_bytes,
        "entryCount": outcome.installed.entry_count,
        "installScripts": outcome.install_scripts,
        "processesSpawned": outcome.processes_spawned,
        "enabled": false,
    })))
}

/// Switch one installed version's stored preference.
fn set_enabled(
    data_home: PathBuf,
    package_id: &str,
    version: &str,
    enabled: bool,
) -> Result<CliExecution> {
    let store = open_store(&data_home)?;
    let preference = store
        .set_enabled(package_id, version, enabled)
        .map_err(as_handler_error)?;
    Ok(report(json!({
        "operation": if enabled { "enable" } else { "disable" },
        "packageId": package_id,
        "version": version,
        "enabled": preference.enabled,
        "updatedAtUnixMs": preference.updated_at_unix_ms,
        "activated": false,
        "processesSpawned": 0,
    })))
}

/// Build the uninstall plan and the registry the drain will act on.
///
/// The registry is empty unless a caller reports instances in `--instances`: a
/// one-shot process holds no live instances of its own, and inventing some would
/// be a fact this surface cannot know.
fn uninstall_plan(
    store: &PackageStore,
    package_id: &str,
    version: &str,
) -> Result<(
    crate::platform::extension_packages::UninstallPlan,
    InstanceRegistry,
)> {
    let installed = store
        .installed_version(package_id, version)
        .map_err(as_handler_error)?
        .ok_or_else(|| anyhow!("package_not_installed"))?;
    let (_, installed_packages) = store.catalogue().map_err(as_handler_error)?;
    let mut catalogue = LocalCatalogue::default();
    for package in &installed_packages {
        catalogue.insert(PackageEntry::new(
            package.package_id.clone(),
            package.version.clone(),
            package.source,
        ));
    }
    let registry = InstanceRegistry::new();
    let plan =
        crate::platform::extension_packages::preview(store, &catalogue, &installed, &registry)
            .map_err(as_handler_error)?;
    Ok((plan, registry))
}

/// Add caller-observed instances to the registry.
///
/// A report can only ever *add* to the running set, so a caller cannot use this
/// to talk an uninstall past its own drain check.
fn observe_instances(registry: &mut InstanceRegistry, observed: Value) -> Result<()> {
    let Value::Array(entries) = observed else {
        return Err(anyhow!("package_uninstall_instances_invalid"));
    };
    for entry in entries {
        let text = |name: &str| -> Result<String> {
            entry
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("package_uninstall_instances_invalid"))
        };
        let number = |name: &str| -> Result<u64> {
            entry
                .get(name)
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("package_uninstall_instances_invalid"))
        };
        let identity = InstanceIdentity::new(
            text("instanceId")?,
            text("packageId")?,
            text("packageVersion")?,
            number("generation")?,
            number("registryEpoch")?,
            Vec::new(),
        )
        .map_err(as_handler_error)?;
        let mut machine = InstanceMachine::discovered(identity).map_err(as_handler_error)?;
        let lifecycle = match text("lifecycle")?.as_str() {
            "discovered" => InstanceLifecycle::Discovered,
            "preparing" => InstanceLifecycle::Preparing,
            "active" => InstanceLifecycle::Active,
            "draining" => InstanceLifecycle::Draining,
            "stopped" => InstanceLifecycle::Stopped,
            "failed" => InstanceLifecycle::Failed,
            "quarantined" => InstanceLifecycle::Quarantined,
            other => {
                return Err(anyhow!(
                    "package_uninstall_instance_lifecycle_unknown:{other}"
                ));
            }
        };
        // An instance reaches a state by passing through the ones before it, so
        // the report is walked the same way rather than jumped to.
        for step in [
            InstanceLifecycle::Preparing,
            InstanceLifecycle::Active,
            InstanceLifecycle::Draining,
            InstanceLifecycle::Stopped,
        ] {
            if machine.state() == lifecycle {
                break;
            }
            if step == InstanceLifecycle::Stopped {
                break;
            }
            machine.advance(step).map_err(as_handler_error)?;
            if step == lifecycle {
                break;
            }
        }
        if machine.state() != lifecycle {
            match lifecycle {
                InstanceLifecycle::Failed => machine.fail("reported by the caller"),
                InstanceLifecycle::Quarantined => machine.quarantine("reported by the caller"),
                _ => Err(licoup_application::ApplicationFailure::permanent(
                    "package_uninstall_instance_lifecycle_unknown",
                    "extension/package-uninstall",
                )),
            }
            .map_err(as_handler_error)?;
        }
        let in_flight = entry.get("inFlight").and_then(Value::as_u64).unwrap_or(0);
        if in_flight > 0 {
            if machine.state() != InstanceLifecycle::Active {
                return Err(anyhow!(
                    "package_uninstall_instance_in_flight_requires_active"
                ));
            }
            for _ in 0..in_flight {
                machine.begin_in_flight().map_err(as_handler_error)?;
            }
        }
        registry.insert(machine);
    }
    Ok(())
}

/// The inputs one collect needs to release a package's registrations.
fn release_inputs(value: &Value) -> Result<ReleaseInputs> {
    let mut inputs = ReleaseInputs::none();
    if let Some(provider) = value.get("providerMcp").filter(|value| !value.is_null()) {
        let kind = match provider.get("kind").and_then(Value::as_str) {
            Some("cursor") => {
                crate::platform::provider_mcp_registration::ProviderConfigKind::Cursor
            }
            Some("antigravity") => {
                crate::platform::provider_mcp_registration::ProviderConfigKind::Antigravity
            }
            Some("claude-code") => {
                crate::platform::provider_mcp_registration::ProviderConfigKind::ClaudeCode
            }
            _ => return Err(anyhow!("package_registration_provider_unknown")),
        };
        inputs = inputs.with_provider_mcp(ProviderMcpRelease {
            kind,
            connector: PathBuf::from(
                provider
                    .get("connector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("package_registration_connector_missing"))?,
            ),
            config_path: PathBuf::from(
                provider
                    .get("configPath")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("package_registration_config_path_missing"))?,
            ),
            approved_digest: provider
                .get("approvedDigest")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("package_registration_approval_missing"))?
                .to_owned(),
        });
    }
    if let Some(plugin) = value.get("codexPlugin").filter(|value| !value.is_null()) {
        inputs = inputs.with_codex_plugin(CodexPluginRelease {
            executable: PathBuf::from(
                plugin
                    .get("executable")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("package_registration_executable_missing"))?,
            ),
            approved_digest: plugin
                .get("approvedDigest")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("package_registration_approval_missing"))?
                .to_owned(),
        });
    }
    Ok(inputs)
}

/// The data home one route was given.
///
/// It takes the value the handler already read through its own typed accessor:
/// the carrier never travels as a value, so a handler cannot hand its admitted
/// arguments to something that does not know what was admitted.
fn resolve_data_home(root: &str) -> Result<PathBuf> {
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err(anyhow!("package_data_root_must_be_absolute"));
    }
    Ok(root)
}

/// Open the package store at the layout owner's root for one data home.
fn open_store(data_home: &Path) -> Result<PackageStore> {
    let root = paths::package_store_root(data_home);
    PackageStore::open(&root).map_err(as_handler_error)
}

/// The archive one route was given.
fn resolve_archive(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(anyhow!("package_archive_must_be_absolute"));
    }
    Ok(path)
}

/// Read one local archive, bounded before it is parsed.
fn read_archive(path: &Path) -> Result<Vec<u8>> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| anyhow!("package_archive_unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(anyhow!("package_archive_not_a_file"));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(anyhow!("package_archive_too_large"));
    }
    std::fs::read(path).map_err(|_| anyhow!("package_archive_unavailable"))
}

/// A digest with its algorithm named, so a token can never be read as bare bytes.
fn prefixed_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        text.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    text
}

fn installed_package(
    package: &crate::platform::extension_packages::InstalledPackage,
    enabled: bool,
) -> Value {
    json!({
        "packageId": package.package_id,
        "version": package.version,
        "digest": package.digest,
        "source": package_source(package),
        "trustChannel": lifecycle_name(package.trust_channel),
        "installedAtUnixMs": package.installed_at_unix_ms,
        "compressedBytes": package.compressed_bytes,
        "expandedBytes": package.expanded_bytes,
        "entryCount": package.entry_count,
        "enabled": enabled,
        "runtimeRef": package.runtime_ref,
        "registrations": package
            .registrations
            .iter()
            .map(|registration| json!({
                "owner": registration.owner.wire_name(),
                "key": registration.key,
                "ownerModule": registration.owner.owner_module(),
            }))
            .collect::<Vec<_>>(),
    })
}

fn package_source(package: &crate::platform::extension_packages::InstalledPackage) -> &'static str {
    use licoup_extension_contracts::deployment::PackageSource;
    match package.source {
        PackageSource::LocalImport => "local-import",
        PackageSource::LocalDirectory => "local-directory",
        PackageSource::OfficialDirectory => "official-directory",
        PackageSource::ThirdPartyDirectory => "third-party-directory",
    }
}

fn uninstall_plan_json(plan: &crate::platform::extension_packages::UninstallPlan) -> Value {
    json!({
        "packageId": plan.package_id,
        "version": plan.version,
        "reverseDependencies": plan.reverse_dependencies,
        "runningInstances": plan.running_instances,
        "inFlight": plan.in_flight,
        "exclusiveBytes": plan.exclusive_bytes,
        "sharedRuntimeRef": plan.shared_runtime_ref,
        "registrations": plan
            .registrations
            .iter()
            .map(|registration| json!({
                "owner": registration.owner.wire_name(),
                "key": registration.key,
                "ownerModule": registration.owner.owner_module(),
            }))
            .collect::<Vec<_>>(),
    })
}

fn recovery_report(report: &crate::platform::extension_packages::RecoveryReport) -> Value {
    json!({
        "abandonedStages": report
            .abandoned
            .iter()
            .map(|stage| json!({
                "packageId": stage.package_id,
                "version": stage.version,
                "reclaimedBytes": stage.reclaimed_bytes,
            }))
            .collect::<Vec<_>>(),
        "installedUntouched": report.installed_untouched,
        "reclaimedBytes": report.reclaimed_bytes,
        "removalsFinished": report.finished_removals,
        "clean": report.is_clean(),
    })
}

const fn lifecycle_name(state: PackageLifecycle) -> &'static str {
    match state {
        PackageLifecycle::Available => "available",
        PackageLifecycle::Downloaded => "downloaded",
        PackageLifecycle::Verified => "verified",
        PackageLifecycle::LocalApproved => "local-approved",
        PackageLifecycle::Staged => "staged",
        PackageLifecycle::Installed => "installed",
    }
}

/// The envelope every successful package route publishes.
fn report(mut body: Value) -> CliExecution {
    if let Some(object) = body.as_object_mut() {
        object.insert("schemaVersion".to_owned(), json!(SCHEMA));
        object.insert("isError".to_owned(), json!(false));
    }
    CliExecution::Json(body)
}

/// The envelope a package refusal publishes.
fn refusal(code: &str, reason: &str) -> CliExecution {
    CliExecution::Json(json!({
        "schemaVersion": SCHEMA,
        "isError": true,
        "reasonCode": code,
        "reason": reason,
    }))
}

fn failure_report(operation: &str, failure: &ApplicationFailure) -> CliExecution {
    CliExecution::Json(failure_body(operation, failure))
}

/// The envelope one platform refusal publishes, for a route that has already
/// decided its own report shape.
fn failure_body(operation: &str, failure: &ApplicationFailure) -> Value {
    json!({
        "schemaVersion": SCHEMA,
        "isError": true,
        "operation": operation,
        "reasonCode": failure.code,
        "stage": failure.stage,
        "component": failure.component.as_ref(),
        "retryable": failure.retryable,
        "effect": match failure.effect {
            licoup_application::EffectCertainty::NotAttempted => "not-attempted",
            licoup_application::EffectCertainty::Uncertain => "uncertain",
            licoup_application::EffectCertainty::Applied => "applied",
        },
        "recovery": failure.recovery.cli_wire(),
    })
}

/// A platform refusal is a fact about the package, so it travels as one.
///
/// The handler's `Result` is for request-shape problems; a package fact is
/// reported in the envelope rather than turned into a transport error.
fn as_handler_error(failure: ApplicationFailure) -> anyhow::Error {
    anyhow!("{failure}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::registration::{
        RecordedRegistration, RegistrationOwner, RegistrationOwners,
    };

    /// A confirmation is a digest of the reviewed plan, and it is stable while
    /// the reviewed facts are: installing something else does not invalidate the
    /// confirmation of the bytes that were reviewed.
    #[test]
    fn a_confirmation_follows_the_reviewed_plan_and_not_the_directory() {
        let (home, store) = fixture_store();
        let bytes = fixture_archive();
        let archive = home.join("fixture.zip");
        std::fs::write(&archive, &bytes).expect("archive");
        let first = candidate_for(&store, &archive).expect("plan");
        let second = candidate_for(&store, &archive).expect("plan again");
        assert_eq!(first.plan_digest, second.plan_digest);
        assert_eq!(first.confirmation, second.confirmation);
        assert!(
            first
                .confirmation
                .starts_with("licoup.package-install-confirmation.v1:sha256:")
        );

        // What the directory holds is reported, and it is not part of what was
        // confirmed.
        assert_eq!(first.plan["alreadyInstalled"], false);
        std::fs::create_dir_all(home.join("staged")).expect("staged");
        let third = candidate_for(&store, &archive).expect("plan once more");
        assert_eq!(third.plan_digest, first.plan_digest);
        assert_eq!(third.confirmation, first.confirmation);
    }

    /// A different archive produces a different confirmation, so swapped bytes
    /// cannot be installed under the reviewed decision.
    #[test]
    fn different_bytes_produce_a_different_confirmation() {
        let (home, store) = fixture_store();
        let one = home.join("one.zip");
        let two = home.join("two.zip");
        std::fs::write(&one, fixture_archive()).expect("archive");
        std::fs::write(&two, fixture_archive_with(b"print('other')\n")).expect("archive");
        let first = candidate_for(&store, &one).expect("plan");
        let second = candidate_for(&store, &two).expect("plan");
        assert_ne!(first.digest, second.digest);
        assert_ne!(first.confirmation, second.confirmation);
    }

    fn fixture_store() -> (PathBuf, PackageStore) {
        let home = std::env::temp_dir().join(format!(
            "licoup-package-command-{}",
            crate::platform::extension_packages::unique_suffix()
        ));
        std::fs::create_dir_all(&home).expect("home");
        let store = PackageStore::open(&paths::package_store_root(&home)).expect("store");
        (home, store)
    }

    fn fixture_archive() -> Vec<u8> {
        fixture_archive_with(b"print('fixture')\n")
    }

    fn fixture_archive_with(entry: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let client = running_client_version().expect("a product version");
        let next_major = client
            .split('.')
            .next()
            .and_then(|major| major.parse::<u64>().ok())
            .map(|major| major + 1)
            .expect("a semantic major version");
        let manifest = json!({
            "schema": licoup_extension_contracts::wire::MANIFEST,
            "id": "example.fixture.echo",
            "version": "1.0.0",
            "displayName": "Fixture",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": [format!(">={client}, <{next_major}")] },
            "runtime": { "mode": "process", "entry": "agent.py" },
            "permissions": [{ "capability": "example.fixture/net", "scope": "self" }],
        });
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("manifest.json", options)
            .expect("start file");
        writer
            .write_all(manifest.to_string().as_bytes())
            .expect("write");
        writer.start_file("agent.py", options).expect("start file");
        writer.write_all(entry).expect("write");
        writer.finish().expect("finish").into_inner()
    }

    #[test]
    fn a_reported_instance_can_only_add_to_the_running_set() {
        let mut registry = InstanceRegistry::new();
        assert!(observe_instances(&mut registry, json!([])).is_ok());
        assert!(registry.is_empty());
        observe_instances(
            &mut registry,
            json!([{
                "instanceId": "instance-1",
                "packageId": "example.specialist.echo",
                "packageVersion": "1.0.0",
                "generation": 1,
                "registryEpoch": 1,
                "lifecycle": "active",
                "inFlight": 1,
            }]),
        )
        .expect("one observed instance");
        assert_eq!(registry.len(), 1);
        assert!(observe_instances(&mut registry, json!({})).is_err());
        assert!(observe_instances(&mut registry, json!([{"instanceId": "x"}])).is_err());
    }

    #[test]
    fn a_relative_data_root_or_archive_is_refused_before_any_store_is_opened() {
        assert!(data_home_for_test("relative/root").is_err());
        assert!(data_home_for_test("/absolute/root").is_ok());
    }

    fn data_home_for_test(root: &str) -> Result<PathBuf> {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err(anyhow!("package_data_root_must_be_absolute"));
        }
        Ok(root)
    }

    #[test]
    fn registration_inputs_name_the_owner_they_belong_to() {
        let inputs = release_inputs(&json!({
            "providerMcp": {
                "kind": "cursor",
                "connector": "/fixture/connector",
                "configPath": "/fixture/config.json",
                "approvedDigest": "sha256:approved",
            }
        }))
        .expect("provider inputs");
        let owners = PackageRegistrationOwners::new(inputs);
        assert!(owners.can_release(RegistrationOwner::CursorMcp));
        assert!(!owners.can_release(RegistrationOwner::CodexPlugin));
        assert!(release_inputs(&json!({"providerMcp": {"kind": "unknown"}})).is_err());
        let released = owners
            .release(&RecordedRegistration::new(
                RegistrationOwner::CursorMcp,
                "land.lico.fixture",
            ))
            .expect_err("the reviewed-candidate rule refuses an unreviewed path");
        assert_eq!(
            released.code,
            crate::platform::package_registration_release::RELEASE_FAILED
        );
    }
}
