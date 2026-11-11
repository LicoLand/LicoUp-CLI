use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::domain::client_update::params::staging_root;

pub(super) fn prepare_staging_root(params: &Value) -> Result<PathBuf> {
    let requested = staging_root(params)?;
    if requested.exists() {
        ensure!(
            !fs::symlink_metadata(&requested)?.file_type().is_symlink(),
            "client update staging root must not be a symbolic link"
        );
    }
    fs::create_dir_all(&requested).context("failed to create client update staging root")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&requested)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&requested, permissions)?;
    }
    let canonical = fs::canonicalize(&requested)
        .context("failed to canonicalize client update staging root")?;
    ensure!(
        fs::metadata(&canonical)?.is_dir(),
        "client update staging root is not a directory"
    );
    Ok(canonical)
}

pub(super) fn safe_staged_path(root: &Path, file_name: &str) -> Result<PathBuf> {
    let file_name = crate::domain::client_update::params::validate_relative_file_name(
        file_name,
        "client update staged fileName",
    )?;
    let candidate = root.join(file_name);
    ensure!(
        candidate.parent() == Some(root),
        "client update staged path escapes its root"
    );
    Ok(candidate)
}

pub(super) fn validate_staged_regular_file(root: &Path, path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).context("client update staged artifact is missing")?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.is_file(),
        "client update staged artifact must be a regular file"
    );
    let canonical =
        fs::canonicalize(path).context("failed to canonicalize client update staged artifact")?;
    ensure!(
        canonical.parent() == Some(root),
        "client update staged artifact escapes its root"
    );
    Ok(())
}

pub(super) fn canonical_regular_file(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("{label} is missing"))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{label} must be a regular file"
    );
    fs::canonicalize(path).with_context(|| format!("failed to canonicalize {label}"))
}

pub(super) fn reject_artifact_overrides(params: &Value) -> Result<()> {
    for field in [
        "sha256",
        "expectedSha256",
        "size",
        "expectedSize",
        "stagedFileName",
        "staged-file-name",
        "fileName",
        "stagedAppPath",
        "staged-app-path",
        "installAppPath",
        "install-app-path",
        "targetAppPath",
        "installDir",
        "install-dir",
        "appName",
        "app-name",
        "installerStrategy",
        "installer-strategy",
    ] {
        ensure!(
            params.get(field).is_none(),
            "client update caller artifact and application path overrides are forbidden"
        );
    }
    Ok(())
}
