//! macOS update authenticity verification before any installed-app mutation.

use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, anyhow, ensure};

use super::plan::ApplyPlan;
use crate::domain::client_update::tree::checked_app_directory;

const MAX_COMMAND_OUTPUT: usize = 64 * 1024;

#[derive(Debug)]
pub(in crate::domain::client_update) struct CommandEvidence {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Eq, PartialEq)]
struct CodeIdentity {
    identifier: String,
    team_identifier: String,
    developer_id_application: bool,
    hardened_runtime: bool,
    secure_timestamp: bool,
}

pub(super) fn verify_platform_update_authenticity(plan: &ApplyPlan) -> Result<bool> {
    if std::env::consts::OS != "macos" {
        return Ok(false);
    }
    let app_dir = plan
        .app_dir
        .as_deref()
        .context("macOS update application directory is missing")?;
    let expected_bundle_id = plan
        .bundle_id
        .as_deref()
        .context("macOS update bundle identifier is missing")?;
    ensure_regular_app_directory(&plan.target_path, "installed")?;
    let staged_app = checked_app_directory(&plan.expanded_dir, app_dir)?;
    verify_macos_update_authenticity_with_runner(
        &plan.target_path,
        &staged_app,
        expected_bundle_id,
        run_system_command,
    )?;
    Ok(true)
}

fn ensure_regular_app_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("client update {label} application is unavailable"))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "client update {label} application must be a regular directory"
    );
    Ok(())
}

fn run_system_command(program: &str, args: &[String]) -> Result<CommandEvidence> {
    let output = Command::new(program)
        .args(args)
        .output()
        .context("failed to execute macOS update authenticity check")?;
    ensure!(
        output.stdout.len() <= MAX_COMMAND_OUTPUT && output.stderr.len() <= MAX_COMMAND_OUTPUT,
        "macOS update authenticity check output exceeded its bound"
    );
    Ok(CommandEvidence {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub(in crate::domain::client_update) fn verify_macos_update_authenticity_with_runner<F>(
    current_app: &Path,
    staged_app: &Path,
    expected_bundle_id: &str,
    mut run: F,
) -> Result<()>
where
    F: FnMut(&str, &[String]) -> Result<CommandEvidence>,
{
    let current = path_arg(current_app)?;
    let staged = path_arg(staged_app)?;

    require_command_success(
        run(
            "/usr/bin/codesign",
            &["--verify", "--deep", "--strict", "--verbose=2", &current].map(str::to_string),
        )?,
        "installed application signature",
    )?;
    require_command_success(
        run(
            "/usr/bin/codesign",
            &["--verify", "--deep", "--strict", "--verbose=2", &staged].map(str::to_string),
        )?,
        "candidate application signature",
    )?;

    let requirement_evidence = run(
        "/usr/bin/codesign",
        &["--display", "--requirements", "-", &current].map(str::to_string),
    )?;
    require_command_success_ref(&requirement_evidence, "installed application requirement")?;
    let requirement = designated_requirement(&requirement_evidence)?;
    ensure!(
        requirement.contains("anchor apple generic"),
        "installed application is not anchored to Apple Developer ID"
    );
    require_command_success(
        run(
            "/usr/bin/codesign",
            &[
                "--verify".to_string(),
                "--deep".to_string(),
                "--strict".to_string(),
                format!("-R={requirement}"),
                staged.clone(),
            ],
        )?,
        "candidate designated requirement",
    )?;

    let current_identity = inspect_identity(&mut run, &current)?;
    let staged_identity = inspect_identity(&mut run, &staged)?;
    for (label, identity) in [
        ("installed", &current_identity),
        ("candidate", &staged_identity),
    ] {
        ensure!(
            identity.identifier == expected_bundle_id,
            "client update {label} application bundle identifier does not match signed metadata"
        );
        ensure!(
            identity.developer_id_application,
            "client update {label} application is not signed with Developer ID Application"
        );
        ensure!(
            identity.hardened_runtime,
            "client update {label} application does not enable hardened runtime"
        );
        ensure!(
            identity.secure_timestamp,
            "client update {label} application does not have a secure timestamp"
        );
    }
    ensure!(
        current_identity.team_identifier == staged_identity.team_identifier,
        "client update candidate signing team does not match the installed application"
    );

    require_command_success(
        run(
            "/usr/bin/xcrun",
            &["stapler", "validate", &staged].map(str::to_string),
        )?,
        "candidate notarization ticket",
    )?;
    require_command_success(
        run(
            "/usr/sbin/spctl",
            &["--assess", "--type", "execute", "--verbose=2", &staged].map(str::to_string),
        )?,
        "candidate Gatekeeper assessment",
    )?;
    Ok(())
}

fn inspect_identity<F>(run: &mut F, app: &str) -> Result<CodeIdentity>
where
    F: FnMut(&str, &[String]) -> Result<CommandEvidence>,
{
    let evidence = run(
        "/usr/bin/codesign",
        &["--display", "--verbose=4", app].map(str::to_string),
    )?;
    require_command_success_ref(&evidence, "application signing identity")?;
    parse_code_identity(&combined_output(&evidence))
}

fn parse_code_identity(output: &str) -> Result<CodeIdentity> {
    let mut identifier = None;
    let mut team_identifier = None;
    let mut developer_id_application = false;
    let mut hardened_runtime = false;
    let mut secure_timestamp = false;
    for line in output.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Identifier=") {
            identifier = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            team_identifier = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Authority=") {
            developer_id_application |= value.starts_with("Developer ID Application:");
        } else if line.starts_with("flags=") {
            hardened_runtime |= line.contains("runtime");
        } else if let Some(value) = line.strip_prefix("Timestamp=") {
            secure_timestamp |= !value.trim().is_empty() && value.trim() != "none";
        }
    }
    let identifier = identifier.context("macOS code signature identifier is missing")?;
    let team_identifier = team_identifier.context("macOS code signature team is missing")?;
    ensure!(
        !identifier.is_empty()
            && team_identifier.len() == 10
            && team_identifier
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
        "macOS code signature identity is invalid"
    );
    Ok(CodeIdentity {
        identifier,
        team_identifier,
        developer_id_application,
        hardened_runtime,
        secure_timestamp,
    })
}

fn designated_requirement(evidence: &CommandEvidence) -> Result<String> {
    let output = combined_output(evidence);
    let requirement = output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("designated => "))
        .ok_or_else(|| anyhow!("installed application designated requirement is missing"))?;
    ensure!(
        !requirement.is_empty()
            && requirement.len() <= 4096
            && requirement
                .bytes()
                .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte)),
        "installed application designated requirement is invalid"
    );
    Ok(requirement.to_string())
}

fn combined_output(evidence: &CommandEvidence) -> String {
    format!("{}\n{}", evidence.stdout, evidence.stderr)
}

fn require_command_success(evidence: CommandEvidence, label: &str) -> Result<()> {
    require_command_success_ref(&evidence, label)
}

fn require_command_success_ref(evidence: &CommandEvidence, label: &str) -> Result<()> {
    ensure!(evidence.success, "macOS update {label} verification failed");
    Ok(())
}

fn path_arg(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .context("macOS update path is not valid UTF-8")?;
    ensure!(
        path.is_absolute() && !text.is_empty(),
        "macOS update path must be absolute"
    );
    Ok(text.to_string())
}
