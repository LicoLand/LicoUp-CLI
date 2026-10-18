mod copy;
mod path;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::Value;
use url::Url;

use super::{
    canonical::sha256_file_exact,
    model::{VerifiedArtifact, VerifiedUpdateSelection},
    params::json_text,
};
use copy::copy_remaining_bytes;

pub(super) fn canonical_regular_file(path: &Path, label: &str) -> Result<PathBuf> {
    path::canonical_regular_file(path, label)
}

pub(super) fn safe_staged_path(root: &Path, file_name: &str) -> Result<PathBuf> {
    path::safe_staged_path(root, file_name)
}

pub(super) fn validate_staged_regular_file(root: &Path, path: &Path) -> Result<()> {
    path::validate_staged_regular_file(root, path)
}

pub(super) fn prepare_staging_root(params: &Value) -> Result<std::path::PathBuf> {
    path::prepare_staging_root(params)
}

pub(super) fn reject_artifact_overrides(params: &Value) -> Result<()> {
    path::reject_artifact_overrides(params)
}

pub(super) struct StagedArtifact {
    pub resumed: bool,
}

pub(super) fn staged_artifact_paths(
    root: &Path,
    artifact: &VerifiedArtifact,
) -> Result<(PathBuf, PathBuf)> {
    let final_path = safe_staged_path(root, &artifact.file_name)?;
    let partial_path = safe_staged_path(root, &format!("{}.partial", artifact.file_name))?;
    Ok((final_path, partial_path))
}

/// Verifies the partial file digest against the signed metadata and renames it
/// into place. Shared by the local copy path and the remote GitHub download
/// path so every staged artifact passes the same exact-size + sha256 gate.
pub(super) fn finalize_partial_artifact(
    root: &Path,
    partial_path: &Path,
    final_path: &Path,
    artifact: &VerifiedArtifact,
) -> Result<()> {
    validate_staged_regular_file(root, partial_path)?;
    let actual = sha256_file_exact(partial_path, artifact.size)?;
    if actual != artifact.sha256 {
        let _ = fs::remove_file(partial_path);
        ensure!(
            actual == artifact.sha256,
            "client update staged artifact digest does not match signed metadata"
        );
    }
    fs::rename(partial_path, final_path)
        .context("failed to finalize staged client update artifact")?;
    validate_staged_regular_file(root, final_path)
}

pub(super) fn stage_selected_artifact(
    params: &Value,
    selection: &VerifiedUpdateSelection,
) -> Result<StagedArtifact> {
    reject_artifact_overrides(params)?;
    let source_text = json_text(params, &["sourcePath", "source-path", "artifactPath"])
        .ok_or_else(|| anyhow!("client update download requires a local sourcePath"))?;
    let source = canonical_regular_file(Path::new(&source_text), "client update source artifact")?;
    ensure!(
        fs::metadata(&source)?.len() == selection.artifact.size,
        "client update source artifact size does not match signed metadata"
    );
    verify_signed_file_url(&selection.artifact.url, &source)?;

    let root = prepare_staging_root(params)?;
    let (final_path, partial_path) = staged_artifact_paths(&root, &selection.artifact)?;
    if final_path.exists() {
        validate_staged_regular_file(&root, &final_path)?;
        if sha256_file_exact(&final_path, selection.artifact.size)? == selection.artifact.sha256 {
            return Ok(StagedArtifact { resumed: true });
        }
        fs::remove_file(&final_path)
            .context("failed to remove invalid staged client update artifact")?;
    }

    let mut partial_size = 0_u64;
    if partial_path.exists() {
        validate_staged_regular_file(&root, &partial_path)?;
        partial_size = fs::metadata(&partial_path)?.len();
        if partial_size > selection.artifact.size {
            fs::remove_file(&partial_path)
                .context("failed to remove oversized partial client update artifact")?;
            partial_size = 0;
        }
    }
    let resumed = partial_size > 0;
    copy_remaining_bytes(
        &source,
        &partial_path,
        partial_size,
        selection.artifact.size,
    )?;
    finalize_partial_artifact(&root, &partial_path, &final_path, &selection.artifact)?;
    Ok(StagedArtifact { resumed })
}

pub(super) fn verified_staged_artifact(
    params: &Value,
    artifact: &VerifiedArtifact,
) -> Result<std::path::PathBuf> {
    reject_artifact_overrides(params)?;
    let root = prepare_staging_root(params)?;
    let path = safe_staged_path(&root, &artifact.file_name)?;
    validate_staged_regular_file(&root, &path)?;
    ensure!(
        sha256_file_exact(&path, artifact.size)? == artifact.sha256,
        "client update staged artifact digest does not match signed metadata"
    );
    Ok(path)
}

fn verify_signed_file_url(url_text: &str, source: &Path) -> Result<()> {
    let url = Url::parse(url_text).map_err(|_| anyhow!("client update signed url is invalid"))?;
    if url.scheme() != "file" {
        return Ok(());
    }
    let signed_source = url
        .to_file_path()
        .map_err(|_| anyhow!("client update signed file url is invalid"))?;
    let signed_source =
        canonical_regular_file(&signed_source, "client update signed source artifact")?;
    ensure!(
        signed_source == source,
        "client update sourcePath does not match the signed artifact url"
    );
    Ok(())
}
