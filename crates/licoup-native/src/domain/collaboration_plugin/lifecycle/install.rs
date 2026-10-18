use super::super::package::{InspectedPackage, inspect_package, write_inspected_package};
use super::super::source::{GitHubSource, stage_github_package};
use super::cleanup::{
    prune_cancelled_install_plans, push_pending_cleanup, simulate_cleanup_failure,
};
use super::model::{CancelledInstallPlan, InstallPlanRecord, InstalledPlugin, PendingCleanup};
use super::state::{
    collaboration_root, epoch_seconds, installed_projection, plugins_root, read_state, write_state,
};
use super::support::{
    require_direct_confirmation, require_direct_request, required_digest, required_plan_id,
};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::platform::client_state::ClientStateStore;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir,
};

const PLAN_SCHEMA: &str = "licoup.optional-collaboration-install-plan.v3";
const PLAN_TTL_SECONDS: u64 = 30 * 60;
const MAX_PLAN_RECORD_BYTES: usize = 16 * 1024;
const MAX_ACTIVE_PLANS: usize = 8;

pub(super) fn install_plan_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_plugin_install_plan_confirmation_required",
    )?;
    let state = read_state(store)?;
    ensure!(
        state.capability_enabled,
        "collaboration_plugin_capability_disabled"
    );
    ensure!(
        state.installed.is_none(),
        "collaboration_plugin_already_installed"
    );
    let trust = state
        .runner_trust
        .as_ref()
        .ok_or_else(|| anyhow!("collaboration_plugin_runner_trust_missing"))?;
    let projected_authority = super::super::authority::projected(&state)?;
    let verified_authority = super::super::authority::read(
        store,
        projected_authority.secure_record.version(),
        projected_authority.secure_record.record_digest_sha256(),
        "Verify runner trust before planning an exact local-server package",
    )?;
    super::super::authority::ensure_projection_matches(&verified_authority.authority, &state)?;
    let source = GitHubSource::from_params(params)?;
    ensure_trust_source(trust, &source.normalized_url)?;
    cleanup_install_plans(store)?;
    let plan_id = Uuid::new_v4().to_string();
    let plan_root = plan_root(store, &plan_id)?;
    let result = (|| -> Result<(InspectedPackage, InstallPlanRecord)> {
        let package = stage_github_package(&source, &plan_root)?;
        verify_package_trust(
            &package,
            &source.normalized_url,
            source.ref_name.as_deref().unwrap_or_default(),
            trust,
        )?;
        let record = plan_record(&plan_id, &source, &package, trust)?;
        write_plan_record(&plan_root, &record)?;
        Ok((package, record))
    })();
    match result {
        Ok((package, record)) => Ok(plan_projection(&record, &package)),
        Err(error) => {
            let _ = fs::remove_dir_all(&plan_root);
            Err(error)
        }
    }
}

#[cfg(test)]
pub(super) fn install_plan_from_directory_in(
    store: &ClientStateStore,
    source_dir: &Path,
) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    let state = read_state(store)?;
    ensure!(
        state.capability_enabled,
        "collaboration_plugin_capability_disabled"
    );
    cleanup_install_plans(store)?;
    let plan_id = Uuid::new_v4().to_string();
    let plan_root = plan_root(store, &plan_id)?;
    ensure_private_dir(&plan_root)?;
    let package = inspect_package(source_dir)?;
    write_inspected_package(&package, &plan_root.join("package"))?;
    let source = GitHubSource {
        normalized_url: "https://github.com/example/collaboration-plugin.git".to_owned(),
        owner: "example".to_owned(),
        repository: "collaboration-plugin".to_owned(),
        ref_name: Some(super::super::test_support::TEST_COMMIT_OID.to_owned()),
        plugin_path: None,
    };
    let trust = state
        .runner_trust
        .as_ref()
        .ok_or_else(|| anyhow!("collaboration_plugin_runner_trust_missing"))?;
    let projected_authority = super::super::authority::projected(&state)?;
    let verified_authority = super::super::authority::read(
        store,
        projected_authority.secure_record.version(),
        projected_authority.secure_record.record_digest_sha256(),
        "Verify runner trust before planning an exact local-server package",
    )?;
    super::super::authority::ensure_projection_matches(&verified_authority.authority, &state)?;
    ensure_trust_source(trust, &source.normalized_url)?;
    verify_package_trust(
        &package,
        &source.normalized_url,
        source.ref_name.as_deref().unwrap_or_default(),
        trust,
    )?;
    let record = plan_record(&plan_id, &source, &package, trust)?;
    write_plan_record(&plan_root, &record)?;
    Ok(plan_projection(&record, &package))
}

pub(super) fn install_apply_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(params, "collaboration_plugin_install_confirmation_required")?;
    let mut state = read_state(store)?;
    let expected_authority = super::super::authority::projected(&state)?;
    ensure!(
        state.capability_enabled,
        "collaboration_plugin_capability_disabled"
    );
    ensure!(
        state.installed.is_none(),
        "collaboration_plugin_already_installed"
    );
    let plan_id = required_plan_id(params)?;
    let expected_digest = required_digest(params, "expectedDigestSha256")?;
    let plan_root = plan_root(store, &plan_id)?;
    let record = read_plan_record(&plan_root)?;
    ensure!(
        record.plan_id == plan_id,
        "collaboration_plugin_plan_id_mismatch"
    );
    ensure!(
        record.expires_at_epoch_seconds > epoch_seconds(),
        "collaboration_plugin_install_plan_expired"
    );
    ensure!(
        record.digest_sha256 == expected_digest,
        "collaboration_plugin_install_digest_mismatch"
    );
    let staged = inspect_package(&plan_root.join("package"))?;
    validate_staged_plan(&record, &staged)?;
    let trust = state
        .runner_trust
        .as_ref()
        .ok_or_else(|| anyhow!("collaboration_plugin_runner_trust_missing"))?;
    ensure!(
        trust.key_id == record.runner_trust_key_id
            && trust.public_key_base64url == record.runner_trust_public_key_base64url
            && trust.fingerprint_sha256 == record.runner_trust_fingerprint_sha256
            && trust.source_repository_url == record.source_url
            && trust.runner_identity
                == super::super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
        "collaboration_plugin_runner_trust_changed"
    );
    verify_package_trust(
        &staged,
        &record.source_url,
        record.source_ref.as_deref().unwrap_or_default(),
        trust,
    )?;
    let pending = PendingCleanup {
        kind: "install-plan".to_owned(),
        entry_name: plan_id.clone(),
    };
    push_pending_cleanup(&mut state, pending.clone())?;

    let plugins_root = plugins_root(store)?;
    ensure_private_dir(&plugins_root)?;
    let destination = plugins_root.join(&record.plugin_id);
    ensure!(
        !destination.exists(),
        "collaboration_plugin_destination_exists"
    );
    let temporary = plugins_root.join(format!(".install-{}", Uuid::new_v4()));
    write_inspected_package(&staged, &temporary)?;
    super::super::workflow::commit_directory_no_replace(&temporary, &destination)
        .map_err(|_| anyhow!("collaboration_plugin_install_commit_failed"))?;

    state.installed = Some(InstalledPlugin {
        plugin_id: record.plugin_id.clone(),
        display_name: record.display_name.clone(),
        version: record.version.clone(),
        digest_sha256: record.digest_sha256.clone(),
        capabilities: record.capabilities.clone(),
        source_url: record.source_url.clone(),
        source_commit_oid: record.source_ref.clone().unwrap_or_default(),
        signed_package_inventory_digest_sha256: record
            .signed_package_inventory_digest_sha256
            .clone(),
        runner_trust_key_id: record.runner_trust_key_id.clone(),
        runner_trust_public_key_base64url: record.runner_trust_public_key_base64url.clone(),
        runner_trust_fingerprint_sha256: record.runner_trust_fingerprint_sha256.clone(),
        runner_platform: record.runner_platform.clone(),
        runner_architecture: record.runner_architecture.clone(),
        runner_relative_path: record.runner_relative_path.clone(),
        runner_digest_sha256: record.runner_digest_sha256.clone(),
        runner_contract_version: record.runner_contract_version.clone(),
        health_contract_version: record.health_contract_version.clone(),
        capabilities_contract_version: record.capabilities_contract_version.clone(),
    });
    let mut replacement_authority = expected_authority.authority.clone();
    replacement_authority.installed = state
        .installed
        .as_ref()
        .map(super::super::authority::AuthorityInstalledArtifact::from);
    let bound_authority = match super::super::authority::replace(
        store,
        &expected_authority,
        replacement_authority,
        "Install the exact signed local-server package and runner target",
    ) {
        Ok(bound) => bound,
        Err(error) => {
            let _ = fs::remove_dir_all(&destination);
            return Err(error);
        }
    };
    super::super::authority::apply_projection(&mut state, &bound_authority)?;
    if let Err(error) = write_state(store, &state) {
        // The protected authority already advanced. Preserve its exact bound
        // artifact so explicit transaction recovery can finish the projection.
        return Err(error);
    }
    let cleanup_pending = if simulate_cleanup_failure(params) {
        true
    } else if fs::remove_dir_all(&plan_root).is_ok() {
        state.cleanup_pending.retain(|item| item != &pending);
        write_state(store, &state).is_err()
    } else {
        true
    };
    Ok(json!({
        "ok": true,
        "status": "installed",
        "capabilityEnabled": true,
        "pluginLoaded": false,
        "loadPolicy": "explicit-command-only",
        "cleanupPending": cleanup_pending,
        "plugin": state.installed.as_ref().map(installed_projection)
    }))
}

pub(super) fn install_cancel_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_plugin_install_cancel_confirmation_required",
    )?;
    let plan_id = required_plan_id(params)?;
    let expected_digest = required_digest(params, "expectedDigestSha256")?;
    let mut state = read_state(store)?;
    prune_cancelled_install_plans(&mut state);
    let plan_root = plan_root(store, &plan_id)?;
    if !plan_root.exists() {
        let receipt = state
            .cancelled_install_plans
            .iter()
            .find(|receipt| receipt.plan_id == plan_id)
            .ok_or_else(|| anyhow!("collaboration_plugin_install_plan_missing"))?;
        ensure!(
            receipt.digest_sha256 == expected_digest,
            "collaboration_plugin_install_cancel_digest_mismatch"
        );
        return Ok(json!({
            "ok": true,
            "status": "cancelled",
            "planId": plan_id,
            "planConsumed": true,
            "idempotentReplay": true,
            "cleanupPending": state.cleanup_pending.iter().any(|item| item.kind == "install-cancel")
        }));
    }
    let record = read_plan_record(&plan_root)?;
    ensure!(
        record.plan_id == plan_id && record.digest_sha256 == expected_digest,
        "collaboration_plugin_install_cancel_digest_mismatch"
    );
    let entry_name = format!(".cancelled-{plan_id}-{}", Uuid::new_v4());
    let quarantine = plans_root(store)?.join(&entry_name);
    let pending = PendingCleanup {
        kind: "install-cancel".to_owned(),
        entry_name,
    };
    push_pending_cleanup(&mut state, pending.clone())?;
    super::super::workflow::commit_directory_no_replace(&plan_root, &quarantine)
        .map_err(|_| anyhow!("collaboration_plugin_install_cancel_prepare_failed"))?;
    if state.cancelled_install_plans.len() == 32 {
        state.cancelled_install_plans.remove(0);
    }
    state.cancelled_install_plans.push(CancelledInstallPlan {
        plan_id: plan_id.clone(),
        digest_sha256: expected_digest.clone(),
        expires_at_epoch_seconds: epoch_seconds().saturating_add(PLAN_TTL_SECONDS),
    });
    if let Err(error) = write_state(store, &state) {
        let _ = super::super::workflow::commit_directory_no_replace(&quarantine, &plan_root);
        return Err(error);
    }
    let cleanup_pending = if simulate_cleanup_failure(params) {
        true
    } else if fs::remove_dir_all(&quarantine).is_ok() {
        state.cleanup_pending.retain(|item| item != &pending);
        write_state(store, &state).is_err()
    } else {
        true
    };
    Ok(json!({
        "ok": true,
        "status": "cancelled",
        "planId": plan_id,
        "planConsumed": true,
        "idempotentReplay": false,
        "cleanupPending": cleanup_pending
    }))
}

fn plan_record(
    plan_id: &str,
    source: &GitHubSource,
    package: &InspectedPackage,
    trust: &super::model::RunnerTrustRecord,
) -> Result<InstallPlanRecord> {
    let created_at_epoch_seconds = epoch_seconds();
    let expires_at_epoch_seconds = created_at_epoch_seconds
        .checked_add(PLAN_TTL_SECONDS)
        .ok_or_else(|| anyhow!("collaboration_plugin_plan_expiry_invalid"))?;
    let runner = super::super::package::select_current_server_runner(package)?;
    Ok(InstallPlanRecord {
        schema_version: PLAN_SCHEMA.to_owned(),
        plan_id: plan_id.to_owned(),
        source_url: source.normalized_url.clone(),
        source_ref: source.ref_name.clone(),
        plugin_path: source
            .plugin_path
            .as_ref()
            .map(|path| path.to_string_lossy().replace('\\', "/")),
        plugin_id: package.manifest.plugin_id.clone(),
        display_name: package.manifest.display_name.clone(),
        version: package.manifest.version.clone(),
        digest_sha256: package.digest_sha256.clone(),
        capabilities: package.manifest.capabilities.clone(),
        file_count: package.file_count,
        total_bytes: package.total_bytes,
        created_at_epoch_seconds,
        expires_at_epoch_seconds,
        signed_package_inventory_digest_sha256: package
            .manifest
            .signed_package_inventory_digest_sha256
            .clone(),
        runner_trust_key_id: trust.key_id.clone(),
        runner_trust_public_key_base64url: trust.public_key_base64url.clone(),
        runner_trust_fingerprint_sha256: trust.fingerprint_sha256.clone(),
        runner_platform: runner.contract.platform,
        runner_architecture: runner.contract.architecture,
        runner_relative_path: runner
            .contract
            .relative_path
            .to_string_lossy()
            .replace('\\', "/"),
        runner_digest_sha256: runner.contract.digest_sha256,
        runner_contract_version: runner.contract.runner_contract_version,
        health_contract_version: runner.contract.health_contract_version,
        capabilities_contract_version: runner.contract.capabilities_contract_version,
    })
}

fn validate_staged_plan(record: &InstallPlanRecord, package: &InspectedPackage) -> Result<()> {
    let runner = super::super::package::select_current_server_runner(package)?;
    ensure!(
        record.schema_version == PLAN_SCHEMA
            && record.plugin_id == package.manifest.plugin_id
            && record.display_name == package.manifest.display_name
            && record.version == package.manifest.version
            && record.digest_sha256 == package.digest_sha256
            && record.capabilities == package.manifest.capabilities
            && record.file_count == package.file_count
            && record.total_bytes == package.total_bytes
            && record.signed_package_inventory_digest_sha256
                == package.manifest.signed_package_inventory_digest_sha256
            && record.runner_platform == runner.contract.platform
            && record.runner_architecture == runner.contract.architecture
            && record.runner_relative_path
                == runner
                    .contract
                    .relative_path
                    .to_string_lossy()
                    .replace('\\', "/")
            && record.runner_digest_sha256 == runner.contract.digest_sha256
            && record.runner_contract_version == runner.contract.runner_contract_version
            && record.health_contract_version == runner.contract.health_contract_version
            && record.capabilities_contract_version
                == runner.contract.capabilities_contract_version,
        "collaboration_plugin_install_plan_package_mismatch"
    );
    Ok(())
}

fn write_plan_record(plan_root: &Path, record: &InstallPlanRecord) -> Result<()> {
    ensure!(
        record.schema_version == PLAN_SCHEMA,
        "collaboration_plugin_plan_schema_invalid"
    );
    let text = serde_json::to_string(record)?;
    atomic_write_private_text_bounded(&plan_root.join("plan.json"), &text, MAX_PLAN_RECORD_BYTES)
}

fn read_plan_record(plan_root: &Path) -> Result<InstallPlanRecord> {
    let bytes = fs::read(plan_root.join("plan.json"))
        .map_err(|_| anyhow!("collaboration_plugin_install_plan_missing"))?;
    ensure!(
        bytes.len() <= MAX_PLAN_RECORD_BYTES,
        "collaboration_plugin_install_plan_too_large"
    );
    let record: InstallPlanRecord = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("collaboration_plugin_install_plan_invalid"))?;
    ensure!(
        record.schema_version == PLAN_SCHEMA,
        "collaboration_plugin_plan_schema_invalid"
    );
    Ok(record)
}

fn plan_projection(record: &InstallPlanRecord, package: &InspectedPackage) -> Value {
    json!({
        "ok": true,
        "status": "planned",
        "planId": record.plan_id,
        "source": {
            "kind": "github",
            "url": record.source_url,
            "ref": record.source_ref,
            "pluginPath": record.plugin_path
        },
        "plugin": {
            "pluginId": record.plugin_id,
            "displayName": record.display_name,
            "version": record.version,
            "capabilities": record.capabilities
        },
        "packageDigestSha256": record.digest_sha256,
        "signedPackageInventoryDigestSha256": record.signed_package_inventory_digest_sha256,
        "fileCount": package.file_count,
        "totalBytes": package.total_bytes,
        "expiresAtEpochSeconds": record.expires_at_epoch_seconds,
        "runnerTrust": {
            "keyId": record.runner_trust_key_id,
            "fingerprintSha256": record.runner_trust_fingerprint_sha256,
            "sourceRepositoryUrl": record.source_url,
            "runnerIdentity": super::super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY
        },
        "runner": {
            "platform": record.runner_platform,
            "architecture": record.runner_architecture,
            "relativePath": record.runner_relative_path,
            "digestSha256": record.runner_digest_sha256,
            "runnerContractVersion": record.runner_contract_version,
            "healthContractVersion": record.health_contract_version,
            "capabilitiesContractVersion": record.capabilities_contract_version
        },
        "requiresDirectConfirmation": true
    })
}

fn verify_package_trust(
    package: &InspectedPackage,
    source_url: &str,
    source_commit_oid: &str,
    trust: &super::model::RunnerTrustRecord,
) -> Result<()> {
    ensure_trust_source(trust, source_url)?;
    ensure!(
        source_commit_oid.len() == 40
            && package
                .manifest
                .signed_package_inventory_digest_sha256
                .len()
                == 64
            && package.manifest.server_runners.iter().all(|runner| {
                runner.source_url == source_url && runner.source_commit_oid == source_commit_oid
            }),
        "collaboration_plugin_server_runner_source_binding_mismatch"
    );
    for runner in &package.manifest.server_runners {
        super::super::runner_signature::verify_runner_signature(
            &package.manifest,
            runner,
            &trust.public_key_base64url,
        )?;
    }
    Ok(())
}

fn ensure_trust_source(trust: &super::model::RunnerTrustRecord, source_url: &str) -> Result<()> {
    ensure!(
        super::super::runner_signature::public_key_fingerprint(&trust.public_key_base64url,)?
            == trust.fingerprint_sha256
            && trust.source_repository_url == source_url
            && trust.runner_identity
                == super::super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
        "collaboration_plugin_runner_trust_source_binding_mismatch"
    );
    Ok(())
}

pub(super) fn plan_root(store: &ClientStateStore, plan_id: &str) -> Result<PathBuf> {
    let parsed =
        Uuid::parse_str(plan_id).map_err(|_| anyhow!("collaboration_plugin_plan_id_invalid"))?;
    ensure!(
        parsed.to_string() == plan_id,
        "collaboration_plugin_plan_id_invalid"
    );
    Ok(plans_root(store)?.join(plan_id))
}

pub(super) fn plans_root(store: &ClientStateStore) -> Result<PathBuf> {
    let root = collaboration_root(store).join("plans");
    ensure_private_dir(&root)?;
    Ok(root)
}

fn cleanup_install_plans(store: &ClientStateStore) -> Result<()> {
    let root = plans_root(store)?;
    let now = epoch_seconds();
    let mut retained = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(anyhow!("collaboration_plugin_plan_entry_invalid"));
        }
        let record = read_plan_record(&entry.path());
        match record {
            Ok(record) if record.expires_at_epoch_seconds > now => retained += 1,
            _ => fs::remove_dir_all(entry.path())
                .map_err(|_| anyhow!("collaboration_plugin_expired_plan_cleanup_failed"))?,
        }
    }
    ensure!(
        retained < MAX_ACTIVE_PLANS,
        "collaboration_plugin_active_plan_limit_reached"
    );
    Ok(())
}
