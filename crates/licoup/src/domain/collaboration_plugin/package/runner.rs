use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};

use super::super::manifest::{
    ValidatedManifest, ValidatedServerRunner, current_server_runner_target,
};
use super::{InspectedPackage, PackageFile};

#[derive(Clone)]
pub(in crate::domain::collaboration_plugin) struct SelectedServerRunner {
    pub(in crate::domain::collaboration_plugin) contract: ValidatedServerRunner,
    pub(in crate::domain::collaboration_plugin) bytes: Vec<u8>,
}

pub(super) fn validate_server_runner_files(
    manifest: &ValidatedManifest,
    files: &[PackageFile],
) -> Result<()> {
    for runner in &manifest.server_runners {
        let bytes = runner_bytes(files, runner)?;
        ensure!(
            sha256(bytes) == runner.digest_sha256,
            "collaboration_plugin_server_runner_digest_mismatch"
        );
    }
    Ok(())
}

pub(in crate::domain::collaboration_plugin) fn select_current_server_runner(
    package: &InspectedPackage,
) -> Result<SelectedServerRunner> {
    let (platform, architecture) = current_server_runner_target()?;
    let contract = package
        .manifest
        .server_runners
        .iter()
        .find(|runner| runner.platform == platform && runner.architecture == architecture)
        .cloned()
        .ok_or_else(|| anyhow!("collaboration_plugin_server_runner_target_missing"))?;
    let bytes = runner_bytes(&package.files, &contract)?.to_vec();
    ensure!(
        sha256(&bytes) == contract.digest_sha256,
        "collaboration_plugin_server_runner_digest_mismatch"
    );
    Ok(SelectedServerRunner { contract, bytes })
}

fn runner_bytes<'a>(files: &'a [PackageFile], runner: &ValidatedServerRunner) -> Result<&'a [u8]> {
    files
        .binary_search_by(|file| file.relative_path.cmp(&runner.relative_path))
        .ok()
        .map(|index| files[index].bytes.as_slice())
        .ok_or_else(|| anyhow!("collaboration_plugin_server_runner_missing"))
}

pub(in crate::domain::collaboration_plugin) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
