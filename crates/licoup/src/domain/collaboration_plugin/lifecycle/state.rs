use super::cleanup::validate_cleanup_entry_name;
use super::model::{CapabilityState, InstalledPlugin, InstalledWorkflowPlugin};
use super::support::{require_direct_confirmation, require_direct_request};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::platform::client_state::ClientStateStore;
use licoup_foundation::platform::file_security::ensure_private_dir;

const STATE_COLLECTION: &str = "collaboration-plugins";
const STATE_SCHEMA: &str = "licoup.optional-collaboration-state.v5";

pub(super) fn status_in(store: &ClientStateStore) -> Result<Value> {
    let state = read_state(store)?;
    let installed = state.installed.as_ref();
    Ok(json!({
        "ok": true,
        "schemaVersion": STATE_SCHEMA,
        "capabilityEnabled": state.capability_enabled,
        "pluginInstalled": installed.is_some(),
        "pluginLoaded": false,
        "loadPolicy": "explicit-command-only",
        "cleanupPending": !state.cleanup_pending.is_empty(),
        "cleanupPendingCount": state.cleanup_pending.len(),
        "runnerTrust": state.runner_trust.as_ref().map(|trust| json!({
            "keyId": trust.key_id,
            "fingerprintSha256": trust.fingerprint_sha256,
            "sourceRepositoryUrl": trust.source_repository_url,
            "runnerIdentity": trust.runner_identity
        })),
        "authorityProtected": state.authority_record.is_some(),
        "authorityVersion": state.authority_record.as_ref().map(|record| record.version()),
        "authorityRecordDigestSha256": state.authority_record.as_ref().map(|record| record.record_digest_sha256()),
        "plugin": installed.map(installed_projection)
    }))
}

pub(super) fn enable_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(params, "collaboration_plugin_enable_confirmation_required")?;
    let mut state = read_state(store)?;
    state.capability_enabled = true;
    if state.authority_record.is_some() {
        let expected = super::super::authority::projected(&read_state(store)?)?;
        let mut replacement = expected.authority.clone();
        replacement.capability_enabled = true;
        let bound = super::super::authority::replace(
            store,
            &expected,
            replacement,
            "Enable the exact optional local-server capability",
        )?;
        super::super::authority::apply_projection(&mut state, &bound)?;
    }
    write_state(store, &state)?;
    Ok(json!({
        "ok": true,
        "status": "enabled",
        "capabilityEnabled": true,
        "pluginInstalled": state.installed.is_some(),
        "pluginLoaded": false
    }))
}

pub(super) fn installed_workflow_plugin(
    store: &ClientStateStore,
) -> Result<InstalledWorkflowPlugin> {
    let state = read_state(store)?;
    ensure!(
        state.capability_enabled,
        "collaboration_plugin_capability_disabled"
    );
    let installed = state
        .installed
        .as_ref()
        .ok_or_else(|| anyhow!("collaboration_plugin_not_installed"))?;
    let projected = super::super::authority::projected(&state)?;
    let verified = super::super::authority::read(
        store,
        projected.secure_record.version(),
        projected.secure_record.record_digest_sha256(),
        "Verify the exact installed local-server package authority",
    )?;
    super::super::authority::ensure_projection_matches(&verified.authority, &state)?;
    Ok(InstalledWorkflowPlugin {
        package_root: plugins_root(store)?.join(&installed.plugin_id),
        plugin_id: installed.plugin_id.clone(),
        digest_sha256: installed.digest_sha256.clone(),
        version: installed.version.clone(),
        source_url: installed.source_url.clone(),
        source_commit_oid: installed.source_commit_oid.clone(),
        signed_package_inventory_digest_sha256: installed
            .signed_package_inventory_digest_sha256
            .clone(),
        runner_trust_key_id: installed.runner_trust_key_id.clone(),
        runner_trust_public_key_base64url: installed.runner_trust_public_key_base64url.clone(),
        runner_trust_fingerprint_sha256: installed.runner_trust_fingerprint_sha256.clone(),
    })
}

pub(super) fn disable_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(params, "collaboration_plugin_disable_confirmation_required")?;
    super::super::assembly::stop_all(store)?;
    let mut state = read_state(store)?;
    state.capability_enabled = false;
    if state.authority_record.is_some() {
        let expected_state = read_state(store)?;
        let expected = super::super::authority::projected(&expected_state)?;
        let mut replacement = expected.authority.clone();
        replacement.capability_enabled = false;
        let bound = super::super::authority::replace(
            store,
            &expected,
            replacement,
            "Disable the exact optional local-server capability",
        )?;
        super::super::authority::apply_projection(&mut state, &bound)?;
    }
    write_state(store, &state)?;
    Ok(json!({
        "ok": true,
        "status": "disabled",
        "capabilityEnabled": false,
        "pluginInstalled": state.installed.is_some(),
        "pluginLoaded": false
    }))
}

pub(super) fn read_state(store: &ClientStateStore) -> Result<CapabilityState> {
    let collection = store.read_collection(STATE_COLLECTION)?;
    let Some(value) = collection
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    else {
        return Ok(CapabilityState {
            schema_version: STATE_SCHEMA.to_owned(),
            capability_enabled: false,
            installed: None,
            cleanup_pending: Vec::new(),
            cancelled_install_plans: Vec::new(),
            runner_trust: None,
            authority_record: None,
        });
    };
    let state: CapabilityState = serde_json::from_value(value.clone())
        .map_err(|_| anyhow!("collaboration_plugin_state_invalid"))?;
    ensure!(
        state.schema_version == STATE_SCHEMA,
        "collaboration_plugin_state_schema_invalid"
    );
    validate_runner_trust(state.runner_trust.as_ref())?;
    validate_authority_projection(&state)?;
    Ok(state)
}

pub(super) fn write_state(store: &ClientStateStore, state: &CapabilityState) -> Result<()> {
    ensure!(
        state.schema_version == STATE_SCHEMA,
        "collaboration_plugin_state_schema_invalid"
    );
    ensure!(
        state.cleanup_pending.len() <= 16 && state.cancelled_install_plans.len() <= 32,
        "collaboration_plugin_state_bounds_invalid"
    );
    validate_runner_trust(state.runner_trust.as_ref())?;
    validate_authority_projection(state)?;
    for pending in &state.cleanup_pending {
        validate_cleanup_entry_name(&pending.entry_name)?;
        ensure!(
            matches!(
                pending.kind.as_str(),
                "install-plan"
                    | "install-cancel"
                    | "uninstall-quarantine"
                    | "uninstall-registrations-quarantine"
            ),
            "collaboration_plugin_cleanup_kind_invalid"
        );
    }
    store.write_collection(
        STATE_COLLECTION,
        json!({
            "items": [serde_json::to_value(state)?]
        }),
    )?;
    Ok(())
}

pub(super) fn installed_projection(installed: &InstalledPlugin) -> Value {
    json!({
        "pluginId": installed.plugin_id,
        "displayName": installed.display_name,
        "version": installed.version,
        "packageDigestSha256": installed.digest_sha256,
        "signedPackageInventoryDigestSha256": installed.signed_package_inventory_digest_sha256,
        "capabilities": installed.capabilities,
        "sourceCommitOid": installed.source_commit_oid,
        "runnerTrustKeyId": installed.runner_trust_key_id,
        "runnerTrustFingerprintSha256": installed.runner_trust_fingerprint_sha256,
        "runner": {
            "platform": installed.runner_platform,
            "architecture": installed.runner_architecture,
            "relativePath": installed.runner_relative_path,
            "digestSha256": installed.runner_digest_sha256,
            "runnerContractVersion": installed.runner_contract_version,
            "healthContractVersion": installed.health_contract_version,
            "capabilitiesContractVersion": installed.capabilities_contract_version
        },
        "source": {"kind": "github", "url": installed.source_url}
    })
}

fn validate_authority_projection(state: &CapabilityState) -> Result<()> {
    if let Some(record) = &state.authority_record {
        record.validate()?;
        let bound = super::super::authority::decode_projected(record.clone())?;
        super::super::authority::ensure_projection_matches(&bound.authority, state)?;
    } else {
        ensure!(
            state.runner_trust.is_none() && state.installed.is_none(),
            "collaboration_authority_projection_missing"
        );
    }
    Ok(())
}

fn validate_runner_trust(trust: Option<&super::model::RunnerTrustRecord>) -> Result<()> {
    let Some(trust) = trust else {
        return Ok(());
    };
    ensure!(
        super::super::runner_signature::public_key_fingerprint(&trust.public_key_base64url)?
            == trust.fingerprint_sha256
            && super::super::source::normalized_github_repository_url(
                &trust.source_repository_url,
            )? == trust.source_repository_url
            && trust.runner_identity
                == super::super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
        "collaboration_plugin_runner_trust_state_invalid"
    );
    Ok(())
}

pub(super) fn plugins_root(store: &ClientStateStore) -> Result<PathBuf> {
    let root = collaboration_root(store).join("installed");
    ensure_private_dir(&root)?;
    Ok(root)
}

pub(super) fn collaboration_root(store: &ClientStateStore) -> PathBuf {
    store.root().join("optional-collaboration")
}

pub(super) fn client_state_store(params: &Value) -> Result<ClientStateStore> {
    if let Some(path) = params.get("stateRoot").and_then(Value::as_str) {
        ensure!(
            cfg!(test),
            "collaboration_plugin_state_root_override_forbidden"
        );
        return ClientStateStore::new(PathBuf::from(path));
    }
    ClientStateStore::portable()
}

pub(super) fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
