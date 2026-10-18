//! Script-driven forward-only live apply for macOS, Windows and Linux.
//!
//! The Rust core owns extraction and verification; the generated native
//! script (executed by /bin/sh or powershell.exe) performs the exit-wait,
//! atomic replacement, registration and relaunch using only
//! OS-bundled tools. The script runs detached so it survives the CLI exit;
//! the GUI exits itself after receiving the applied response.

pub(in crate::domain::client_update) mod macos_integrity;
pub(super) mod plan;
pub(super) mod script;
mod spawn;

use std::{fs, path::Path};

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};

use super::{
    archive::extract_signed_archive,
    constants::CLIENT_UPDATE_MODE,
    model::VerifiedUpdateSelection,
    tree::{checked_app_directory, remove_generated_directory},
};
use plan::ApplyPlan;
use script::{
    ScriptAction, apply_script, validate_bundle_id_arg, validate_script_args, validate_script_paths,
};
use spawn::spawn_apply_script;

pub(super) fn apply_live(
    selection: &VerifiedUpdateSelection,
    staged_path: &Path,
    params: &Value,
) -> Result<Value> {
    let mut plan = build_plan(selection, staged_path, params, ScriptAction::Apply)?;
    remove_generated_directory(&plan.expanded_dir)?;
    fs::create_dir_all(&plan.expanded_dir)
        .context("failed to create client update extraction root")?;
    extract_signed_archive(selection, staged_path, &plan.expanded_dir)?;
    validate_expanded_layout(&plan)?;
    let platform_authenticity_verified =
        macos_integrity::verify_platform_update_authenticity(&plan)?;
    let data_root = super::params::live_data_root(params)?;
    let prepared = crate::domain::client_state_migration::prepare_update_handoff(
        &data_root,
        &selection.receipt(),
        &plan.target_path,
    )?;
    plan.handoff_path = Some(prepared.handoff_path.clone());
    plan.backup_path = Some(prepared.backup_path);
    let dispatch =
        validate_plan_args(&plan, ScriptAction::Apply).and_then(|()| dispatch_script(&plan));
    if let Err(error) = dispatch {
        // Dispatch failed before ownership could pass to the candidate. The
        // current binary remains authoritative and may clear this pre-handoff
        // claim; after successful dispatch only forward repair is allowed.
        licoup_foundation::platform::file_security::remove_private_state_marker(
            &prepared.handoff_path,
        )
        .context("failed to clear the pre-dispatch client update handoff")?;
        return Err(error);
    }
    Ok(json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": "applied",
        "runningVersion": selection.running_version,
        "runningReleaseTrack": selection.running_release_track,
        "targetReleaseTrack": selection.target_release_track,
        "availableVersion": selection.version,
        "targetId": selection.artifact.target_id,
        "installerStrategy": selection.artifact.installer_strategy,
        "installedArtifactId": selection.receipt()["receiptId"],
        "artifactSha256": selection.artifact.sha256,
        "artifactReceipt": selection.receipt(),
        "executed": true,
        "scriptDispatched": true,
        "platformAuthenticityVerified": platform_authenticity_verified,
        "restartRequired": true,
        "preUpdateStateRecord": {
            "runningVersion": selection.running_version,
            "recorded": true,
            "pathRedacted": true,
        },
        "productionReady": false,
        "publicMetadataOnly": true,
        "storeCredentialsRequired": false,
    }))
}

fn build_plan(
    selection: &VerifiedUpdateSelection,
    staged_path: &Path,
    params: &Value,
    action: ScriptAction,
) -> Result<ApplyPlan> {
    let plan = plan::build_apply_plan(selection, staged_path, params, action)?;
    let path_args = [
        plan.install_root.to_string_lossy().to_string(),
        plan.expanded_dir.to_string_lossy().to_string(),
    ];
    let path_refs = path_args.iter().map(String::as_str).collect::<Vec<_>>();
    validate_script_paths(&path_refs)?;
    if let Some(bundle_id) = plan.bundle_id.as_deref() {
        validate_bundle_id_arg(bundle_id)?;
    }
    Ok(plan)
}

fn validate_plan_args(plan: &ApplyPlan, action: ScriptAction) -> Result<()> {
    let argv = script_argv(plan, action)?;
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    // The macOS app_dir is a signed relative file name, the pid is validated
    // by the plan builder and the bundle id by its own rule; every remaining
    // argv value is a path that must be absolute.
    let mut path_args = vec![plan.install_root.to_string_lossy().to_string()];
    if action == ScriptAction::Apply {
        path_args.push(plan.expanded_dir.to_string_lossy().to_string());
    }
    if let Some(path) = plan.backup_path.as_ref() {
        path_args.push(path.to_string_lossy().to_string());
    }
    if let Some(path) = plan.handoff_path.as_ref() {
        path_args.push(path.to_string_lossy().to_string());
    }
    let path_refs: Vec<&str> = path_args.iter().map(String::as_str).collect();
    validate_script_paths(&path_refs)?;
    validate_script_args(&argv_refs)?;
    if let Some(bundle_id) = plan.bundle_id.as_deref() {
        validate_bundle_id_arg(bundle_id)?;
    }
    Ok(())
}

fn dispatch_script(plan: &ApplyPlan) -> Result<()> {
    let template = apply_script(plan.action);
    let script_parent = plan
        .script_path
        .parent()
        .context("client update script path is invalid")?;
    fs::create_dir_all(script_parent).context("failed to create client update script directory")?;
    fs::write(&plan.script_path, template).context("failed to write client update script")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&plan.script_path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&plan.script_path, permissions)?;
    }
    let argv = script_argv(plan, plan.action)?;
    spawn_apply_script(plan, &argv)
}

fn validate_expanded_layout(plan: &ApplyPlan) -> Result<()> {
    match &plan.app_dir {
        Some(app_dir) => {
            let _ = checked_app_directory(&plan.expanded_dir, app_dir)?;
        }
        None => {
            let executable_name = if cfg!(windows) {
                "licoup.exe"
            } else {
                "licoup"
            };
            let executable = plan.expanded_dir.join(executable_name);
            ensure!(
                executable.is_file(),
                "client update expanded bundle is missing its client executable"
            );
        }
    }
    Ok(())
}

fn script_argv(plan: &ApplyPlan, action: ScriptAction) -> Result<Vec<String>> {
    let mut args = Vec::new();
    if std::env::consts::OS == "macos" {
        args.push(
            plan.app_dir
                .clone()
                .ok_or_else(|| anyhow!("macOS apply plan is missing the app directory"))?,
        );
        args.push(plan.install_root.to_string_lossy().to_string());
        if action == ScriptAction::Apply {
            args.push(plan.expanded_dir.to_string_lossy().to_string());
        }
        args.push(plan.gui_pid.clone());
        args.push(
            plan.bundle_id
                .clone()
                .ok_or_else(|| anyhow!("macOS apply plan is missing the bundle id"))?,
        );
    } else {
        args.push(plan.install_root.to_string_lossy().to_string());
        if action == ScriptAction::Apply {
            args.push(plan.expanded_dir.to_string_lossy().to_string());
        }
        args.push(plan.gui_pid.clone());
    }
    if action == ScriptAction::Apply {
        args.push(
            plan.backup_path
                .as_ref()
                .ok_or_else(|| anyhow!("client update pre-claim backup path is missing"))?
                .to_string_lossy()
                .to_string(),
        );
        args.push(
            plan.handoff_path
                .as_ref()
                .ok_or_else(|| anyhow!("client update handoff path is missing"))?
                .to_string_lossy()
                .to_string(),
        );
    }
    Ok(args)
}
