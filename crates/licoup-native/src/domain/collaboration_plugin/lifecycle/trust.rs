use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

use super::model::RunnerTrustRecord;
use super::state::{read_state, write_state};
use super::support::{require_direct_confirmation, require_direct_request};
use crate::platform::client_state::ClientStateStore;

pub(super) fn trust_import_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_plugin_runner_trust_import_confirmation_required",
    )?;
    let key_id = required_text(params, "runnerTrustKeyId")?;
    let public_key = required_text(params, "runnerTrustPublicKeyBase64url")?;
    let expected_fingerprint = required_text(params, "expectedRunnerTrustFingerprintSha256")?;
    let source_repository_url = super::super::source::normalized_github_repository_url(
        required_text(params, "runnerSourceRepositoryUrl")?,
    )?;
    let runner_identity = required_text(params, "runnerIdentity")?;
    ensure!(
        runner_identity == super::super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
        "collaboration_plugin_runner_identity_invalid"
    );
    super::super::runner_signature::validate_key_id(key_id)?;
    let fingerprint = super::super::runner_signature::public_key_fingerprint(public_key)?;
    ensure!(
        expected_fingerprint == fingerprint,
        "collaboration_plugin_runner_trust_fingerprint_mismatch"
    );
    let mut state = read_state(store)?;
    ensure!(
        state.installed.is_none(),
        "collaboration_plugin_runner_trust_change_requires_uninstall"
    );
    if let Some(existing) = state.runner_trust.as_ref() {
        ensure!(
            existing.key_id == key_id
                && existing.public_key_base64url == public_key
                && existing.fingerprint_sha256 == fingerprint
                && existing.source_repository_url == source_repository_url
                && existing.runner_identity == runner_identity,
            "collaboration_plugin_runner_trust_change_requires_remove"
        );
        let projected = super::super::authority::projected(&state)?;
        let verified = super::super::authority::read(
            store,
            projected.secure_record.version(),
            projected.secure_record.record_digest_sha256(),
            "Verify the exact existing local-server runner trust binding",
        )?;
        super::super::authority::ensure_projection_matches(&verified.authority, &state)?;
        return Ok(json!({
            "ok": true,
            "status": "runner-trust-imported",
            "keyId": existing.key_id,
            "fingerprintSha256": existing.fingerprint_sha256,
            "sourceRepositoryUrl": existing.source_repository_url,
            "runnerIdentity": existing.runner_identity,
            "idempotent": true
        }));
    }
    let next_trust = RunnerTrustRecord {
        key_id: key_id.to_owned(),
        public_key_base64url: public_key.to_owned(),
        fingerprint_sha256: fingerprint.clone(),
        source_repository_url: source_repository_url.clone(),
        runner_identity: runner_identity.to_owned(),
    };
    let bound = if state.authority_record.is_some() {
        let expected = super::super::authority::projected(&state)?;
        let mut replacement = expected.authority.clone();
        replacement.trust = Some(super::super::authority::AuthorityTrust::from(&next_trust));
        state.runner_trust = Some(next_trust);
        let bound = super::super::authority::replace(
            store,
            &expected,
            replacement,
            "Authorize the exact local-server runner trust binding",
        )?;
        super::super::authority::apply_projection(&mut state, &bound)?;
        bound
    } else {
        state.runner_trust = Some(next_trust.clone());
        let bound = super::super::authority::create(
            store,
            super::super::authority::CollaborationAuthority::new(
                super::super::authority::AuthorityTrust::from(&next_trust),
                state.capability_enabled,
            ),
            "Authorize the exact local-server runner trust binding",
        )?;
        super::super::authority::apply_projection(&mut state, &bound)?;
        bound
    };
    ensure!(
        bound.secure_record.record_digest_sha256()
            == state
                .authority_record
                .as_ref()
                .map(|record| record.record_digest_sha256())
                .unwrap_or_default(),
        "collaboration_authority_projection_commit_failed"
    );
    write_state(store, &state)?;
    Ok(json!({
        "ok": true,
        "status": "runner-trust-imported",
        "keyId": key_id,
        "fingerprintSha256": fingerprint,
        "sourceRepositoryUrl": source_repository_url,
        "runnerIdentity": runner_identity,
        "idempotent": false
    }))
}

pub(super) fn trust_remove_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_plugin_runner_trust_remove_confirmation_required",
    )?;
    let expected = required_text(params, "expectedRunnerTrustFingerprintSha256")?;
    let expected_source = super::super::source::normalized_github_repository_url(required_text(
        params,
        "expectedRunnerSourceRepositoryUrl",
    )?)?;
    let expected_identity = required_text(params, "expectedRunnerIdentity")?;
    let mut state = read_state(store)?;
    ensure!(
        state.installed.is_none(),
        "collaboration_plugin_runner_trust_remove_requires_uninstall"
    );
    let trust = state
        .runner_trust
        .as_ref()
        .ok_or_else(|| anyhow!("collaboration_plugin_runner_trust_missing"))?;
    ensure!(
        trust.fingerprint_sha256 == expected
            && trust.source_repository_url == expected_source
            && trust.runner_identity == expected_identity
            && expected_identity == super::super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
        "collaboration_plugin_runner_trust_fingerprint_mismatch"
    );
    let expected_authority = super::super::authority::projected(&state)?;
    let mut replacement = expected_authority.authority.clone();
    replacement.capability_enabled = false;
    replacement.trust = None;
    state.capability_enabled = false;
    state.runner_trust = None;
    let bound = super::super::authority::replace(
        store,
        &expected_authority,
        replacement,
        "Remove the exact local-server runner trust binding",
    )?;
    super::super::authority::apply_projection(&mut state, &bound)?;
    write_state(store, &state)?;
    Ok(json!({
        "ok": true,
        "status": "runner-trust-removed",
        "fingerprintSha256": expected,
        "sourceRepositoryUrl": expected_source,
        "runnerIdentity": expected_identity
    }))
}

fn required_text<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("collaboration_plugin_runner_trust_input_required"))?;
    ensure!(
        value == value.trim() && !value.is_empty() && value.len() <= 4096,
        "collaboration_plugin_runner_trust_input_invalid"
    );
    Ok(value)
}
