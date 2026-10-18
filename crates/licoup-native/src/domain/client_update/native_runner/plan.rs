//! Apply plan resolution: install root, replacement target, staging layout
//! and the running GUI process identity.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use anyhow::anyhow;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::super::{
    model::VerifiedUpdateSelection,
    params::{bool_param, json_text},
};
use super::script::ScriptAction;

pub(super) struct ApplyPlan {
    pub action: ScriptAction,
    pub install_root: PathBuf,
    pub app_dir: Option<String>,
    pub bundle_id: Option<String>,
    pub target_path: PathBuf,
    pub expanded_dir: PathBuf,
    pub script_path: PathBuf,
    pub log_path: PathBuf,
    pub handoff_path: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub gui_pid: String,
    pub wait: bool,
}

pub(super) fn build_apply_plan(
    selection: &VerifiedUpdateSelection,
    staged_path: &Path,
    params: &Value,
    action: ScriptAction,
) -> Result<ApplyPlan> {
    let staging_root = staged_path
        .parent()
        .context("client update staged artifact root is invalid")?;
    let binding = binding_suffix(selection)?;
    let strategy = selection.artifact.installer_strategy.as_str();
    let (install_root, app_dir, bundle_id) = match (std::env::consts::OS, strategy) {
        ("macos", "app-bundle-replacement") => {
            let application_name = selection
                .artifact
                .application_name
                .as_deref()
                .context("client update signed applicationName is required")?;
            let bundle_id = selection
                .artifact
                .bundle_id
                .as_deref()
                .context("client update signed bundleId is required")?;
            (
                install_root_override(params)?.unwrap_or_else(macos_install_root),
                Some(application_name.to_string()),
                Some(bundle_id.to_string()),
            )
        }
        ("windows" | "linux", "portable-replacement") => {
            let root = match install_root_override(params)? {
                Some(root) => root,
                None => portable_install_root()?,
            };
            (root, None, None)
        }
        _ => bail!("client update live apply is not enabled for installer strategy '{strategy}'"),
    };
    let target_path = match &app_dir {
        Some(app_dir) => install_root.join(app_dir),
        None => install_root.clone(),
    };
    ensure_guarded_target(&target_path)?;
    Ok(ApplyPlan {
        action,
        install_root,
        app_dir,
        bundle_id,
        target_path,
        expanded_dir: staging_root.join(format!(".expanded-{binding}")),
        script_path: staging_root.join(".scripts").join(format!(
            "apply-{binding}.{}",
            super::script::script_extension()
        )),
        log_path: staging_root.join(format!("apply-{binding}.log")),
        handoff_path: None,
        backup_path: None,
        gui_pid: resolved_gui_pid(params)?,
        wait: bool_param(params, "waitForScript")?,
    })
}

pub(super) fn binding_suffix(selection: &VerifiedUpdateSelection) -> Result<String> {
    let receipt = selection.receipt();
    let receipt_id = receipt["receiptId"]
        .as_str()
        .context("client update artifact receiptId is missing")?;
    receipt_id
        .strip_prefix("sha256:")
        .map(ToOwned::to_owned)
        .context("client update artifact receiptId is invalid")
}

fn install_root_override(params: &Value) -> Result<Option<PathBuf>> {
    match json_text(params, &["installRoot", "install-root"]) {
        Some(path) => {
            let absolute = PathBuf::from(&path);
            ensure!(
                absolute.is_absolute(),
                "client update install root must be absolute"
            );
            Ok(Some(absolute))
        }
        None => Ok(None),
    }
}

fn macos_install_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|executable| {
            licoup_foundation::platform::paths::desktop_bundle_for_cli(&executable)
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(|| PathBuf::from("/Applications"))
}

fn portable_install_root() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("failed to resolve client executable")?;
    let directory = executable
        .parent()
        .context("client executable directory is invalid")?
        .to_path_buf();
    ensure!(
        directory.is_absolute(),
        "client executable directory must be absolute"
    );
    Ok(directory)
}

/// Guards the replacement target against `rm -rf /` style accidents: the
/// target must be absolute and contain at least two normal path components
/// (rejects `/`, drive roots and shallow single-level targets).
fn ensure_guarded_target(target: &Path) -> Result<()> {
    use std::path::Component;
    ensure!(
        target.is_absolute(),
        "client update replacement target must be absolute"
    );
    let normal_components = target
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    ensure!(
        normal_components >= 2,
        "client update replacement target must not be a filesystem root"
    );
    ensure!(
        target.file_name().is_some(),
        "client update replacement target is invalid"
    );
    Ok(())
}

fn resolved_gui_pid(params: &Value) -> Result<String> {
    if let Some(value) = json_text(params, &["guiPid", "gui-pid"]) {
        validate_pid(&value)?;
        return Ok(value);
    }
    let pid = parent_gui_pid()?;
    Ok(pid.to_string())
}

fn validate_pid(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 10 && value.bytes().all(|byte| byte.is_ascii_digit()),
        "client update gui-pid is invalid"
    );
    let pid: u64 = value.parse().context("client update gui-pid is invalid")?;
    ensure!(
        pid > 1 && pid < i32::MAX as u64,
        "client update gui-pid is invalid"
    );
    Ok(())
}

/// The GUI is always the direct parent of the native CLI it spawned.
#[cfg(unix)]
fn parent_gui_pid() -> Result<u32> {
    let pid = unsafe { libc::getppid() };
    ensure!(pid > 1, "client update gui process is not resolvable");
    Ok(pid as u32)
}

#[cfg(windows)]
fn parent_gui_pid() -> Result<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        bail!("client update gui process is not resolvable");
    }
    let self_pid = unsafe { GetCurrentProcessId() };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    if unsafe { Process32FirstW(snapshot, &mut entry) } != 0 {
        loop {
            if entry.th32ParentProcessID == self_pid {
                found = Some(entry.th32ProcessID);
                break;
            }
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                break;
            }
        }
    }
    unsafe {
        CloseHandle(snapshot);
    }
    found.ok_or_else(|| anyhow!("client update gui process is not resolvable"))
}

#[cfg(test)]
pub(in crate::domain::client_update) fn ensure_guarded_target_for_test(
    target: &Path,
) -> Result<()> {
    ensure_guarded_target(target)
}
