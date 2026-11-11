use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

use super::{
    constants::CLIENT_UPDATE_MANIFEST_SCHEMA,
    keys::load_public_keys,
    metadata::{load_manifest, load_revocation_list},
    model::VerifiedManifest,
    params::{
        product_version, selected_target_id, target_release_track, validate_public_identifier,
    },
    release::select_highest_release,
    revocation::enforce_revocation_policy,
    signature::verify_manifest_role_signatures,
};

pub(super) fn verify_update_selection(params: &Value) -> Result<VerifiedManifest> {
    let running_track = crate::domain::client_state_migration::ReleaseTrack::running()?;
    let target_track = target_release_track(params)?;
    let current_version = product_version(params)?;
    let target_id = selected_target_id(params)?;
    let manifest = load_manifest(params)?;
    ensure!(
        manifest.is_object(),
        "client update manifest must be an object"
    );
    ensure_exact_object_keys(
        &manifest,
        &[
            "schemaVersion",
            "releaseTrack",
            "releaseTrackPolicy",
            "releases",
            "signatures",
        ],
        "client update manifest contract is not closed",
    )?;
    ensure!(
        manifest.get("schemaVersion").and_then(Value::as_str)
            == Some(CLIENT_UPDATE_MANIFEST_SCHEMA),
        "client update manifest schema is unsupported"
    );
    ensure!(
        manifest.get("releaseTrack").and_then(Value::as_str) == Some(target_track.as_str()),
        "client update manifest release track does not match the selected track"
    );
    let offline_root_key_id = manifest
        .pointer("/releaseTrackPolicy/offlineRootKeyId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("client update offlineRootKeyId is required"))?;
    let online_signing_key_id = manifest
        .pointer("/releaseTrackPolicy/onlineSigningKeyId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("client update onlineSigningKeyId is required"))?;
    validate_public_identifier(offline_root_key_id, "client update offlineRootKeyId")?;
    validate_public_identifier(online_signing_key_id, "client update onlineSigningKeyId")?;
    ensure!(
        offline_root_key_id != online_signing_key_id,
        "client update offline root and online signing keys must be distinct"
    );
    let public_keys = load_public_keys(params)?;
    let verified_key_ids = verify_manifest_role_signatures(
        &manifest,
        &public_keys,
        offline_root_key_id,
        online_signing_key_id,
    )?;
    let selected_release = select_highest_release(
        &manifest,
        &current_version,
        &target_id,
        target_track == crate::domain::client_state_migration::ReleaseTrack::Stable,
    )?;
    for release in manifest
        .get("releases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        validate_migration_frontier_structure(release.get("migrationFrontier"))?;
    }
    if let Some(selected) = selected_release.as_ref() {
        validate_candidate_frontier(selected.release.get("migrationFrontier"))?;
    }
    let verified = VerifiedManifest::from_selection(
        running_track.as_str().to_owned(),
        target_track.as_str().to_owned(),
        current_version,
        verified_key_ids,
        &manifest,
        selected_release
            .as_ref()
            .map(|selected| (selected.artifact.clone(), selected.release)),
    );
    let revocation = load_revocation_list(params)?;
    enforce_revocation_policy(
        &manifest,
        revocation.as_ref(),
        &public_keys,
        offline_root_key_id,
        online_signing_key_id,
        target_track.as_str(),
        verified.selected.as_ref(),
    )?;
    Ok(verified)
}

fn validate_migration_frontier_structure(value: Option<&Value>) -> Result<()> {
    let value = value.ok_or_else(|| anyhow!("client update migrationFrontier is required"))?;
    ensure_exact_object_keys(
        value,
        &["frontierId", "domains"],
        "client update migration frontier contract is not closed",
    )?;
    let frontier_id = value
        .get("frontierId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("client update migration frontierId is required"))?;
    super::params::validate_public_identifier(frontier_id, "client update migration frontierId")?;
    let domains = value
        .get("domains")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("client update migration frontier domains are required"))?;
    ensure!(
        !domains.is_empty(),
        "client update migration frontier domains are required"
    );
    let mut domain_ids = std::collections::BTreeSet::new();
    for domain in domains {
        ensure_exact_object_keys(
            domain,
            &["domainId", "targetSchemaVersion", "requiredStepIds"],
            "client update migration frontier domain contract is not closed",
        )?;
        let domain_id = domain
            .get("domainId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("client update migration frontier domainId is required"))?;
        super::params::validate_public_identifier(
            domain_id,
            "client update migration frontier domainId",
        )?;
        ensure!(
            domain_ids.insert(domain_id),
            "client update migration frontier contains a duplicate domainId"
        );
        ensure!(
            domain
                .get("targetSchemaVersion")
                .and_then(Value::as_u64)
                .is_some_and(|version| version > 0 && version <= u32::MAX as u64),
            "client update migration frontier targetSchemaVersion is invalid"
        );
        let step_ids = domain
            .get("requiredStepIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow!("client update migration frontier requiredStepIds are required")
            })?;
        let mut seen_steps = std::collections::BTreeSet::new();
        for step_id in step_ids {
            let step_id = step_id.as_str().ok_or_else(|| {
                anyhow!("client update migration frontier requiredStepId is invalid")
            })?;
            super::params::validate_public_identifier(
                step_id,
                "client update migration frontier requiredStepId",
            )?;
            ensure!(
                seen_steps.insert(step_id),
                "client update migration frontier contains a duplicate requiredStepId"
            );
        }
    }
    Ok(())
}

pub(super) fn ensure_exact_object_keys(
    value: &Value,
    expected: &[&str],
    message: &'static str,
) -> Result<()> {
    let object = value.as_object().ok_or_else(|| anyhow!(message))?;
    ensure!(
        object.len() == expected.len() && expected.iter().all(|field| object.contains_key(*field)),
        message
    );
    Ok(())
}

fn validate_candidate_frontier(value: Option<&Value>) -> Result<()> {
    let value = value.ok_or_else(|| anyhow!("client update migrationFrontier is required"))?;
    let embedded = crate::domain::client_state_migration::frontier_projection()?;
    let candidate_domains = value
        .get("domains")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("client update migration frontier domains are required"))?;
    let candidate_by_id = candidate_domains
        .iter()
        .filter_map(|domain| {
            domain
                .get("domainId")
                .and_then(Value::as_str)
                .map(|domain_id| (domain_id, domain))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for current in embedded
        .get("domains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let domain_id = current["domainId"]
            .as_str()
            .ok_or_else(|| anyhow!("embedded migration frontier is invalid"))?;
        let candidate = candidate_by_id
            .get(domain_id)
            .ok_or_else(|| anyhow!("client update migration frontier removes a current domain"))?;
        let current_version = current["targetSchemaVersion"]
            .as_u64()
            .ok_or_else(|| anyhow!("embedded migration frontier is invalid"))?;
        let candidate_version = candidate["targetSchemaVersion"].as_u64().ok_or_else(|| {
            anyhow!("client update migration frontier targetSchemaVersion is invalid")
        })?;
        ensure!(
            candidate_version >= current_version,
            "client update migration frontier regresses a current domain"
        );
        let current_steps = current["requiredStepIds"]
            .as_array()
            .ok_or_else(|| anyhow!("embedded migration frontier is invalid"))?;
        let candidate_steps = candidate["requiredStepIds"].as_array().ok_or_else(|| {
            anyhow!("client update migration frontier requiredStepIds are required")
        })?;
        ensure!(
            candidate_steps.starts_with(current_steps),
            "client update migration frontier rewrites current migration history"
        );
    }
    Ok(())
}

pub(super) fn require_available_selection(
    params: &Value,
) -> Result<super::model::VerifiedUpdateSelection> {
    let (effective, receipt_id) = super::receipt::params_with_bound_track(params)?;
    let selection = verify_update_selection(&effective)?
        .selected
        .ok_or_else(|| anyhow!("client update has no eligible signed release for this client"))?;
    super::receipt::ensure_selection_matches(&selection, &receipt_id)?;
    Ok(selection)
}
