//! The Lico-namespaced Antigravity Agent Hooks bridge.
//!
//! Antigravity's official Agent Hooks contract runs one command at a lifecycle
//! event. LicoUp registers exactly one global Stop hook, under its own namespace
//! in `~/.gemini/config/hooks.json`, whose only job is to record which native
//! conversation the turn just ran.
//!
//! The hook command is the Antigravity adapter package's own program,
//! `lico-agent-antigravity receipt`. The bridge used to generate a `/bin/sh`
//! script beside the config and pipe the vendor payload into `python3`; that put
//! two interpreters between the vendor client and LicoUp, and a machine without
//! `python3` silently lost the conversation identity. The package program is a
//! native executable that reads the payload itself, so the hook path needs no
//! interpreter and no generated script at all.
//!
//! What the bridge still owns is the *installation*: resolving the packaged
//! program, writing the command into the user's hooks configuration without
//! disturbing any other hook, and removing both the namespace and any script a
//! previous client installed when the adapter is detached.

use super::errors::ProtocolFailure;
use super::model::HOOK_NAMESPACE;
use crate::parser::parse_hook_receipt;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The generated script earlier clients installed beside the hooks config.
///
/// It is not written any more: the hook command is the package program itself.
/// An upgrade still removes it, so a machine that ran the script version does
/// not keep a stale interpreter hook on disk after the entry stops naming it.
const RETIRED_SCRIPT_NAME: &str = "session-receipt-hook.sh";

/// The package subcommand the Stop hook runs.
const RECEIPT_SUBCOMMAND: &str = "receipt";

/// The package program's file name, without the platform's executable suffix.
const PACKAGE_PROGRAM: &str = "lico-agent-antigravity";

pub(crate) fn ensure_hook_bridge() -> Result<(), ProtocolFailure> {
    let command = hook_command()?;
    install_global_hook(&command)
}

pub fn hook_bridge_status() -> Value {
    let config_dir = gemini_config_dir().ok();
    let hooks_path = config_dir.as_ref().map(|path| path.join("hooks.json"));
    let hook_registered = hooks_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|root| root.get(HOOK_NAMESPACE).cloned())
        .is_some();
    let program = package_program_path();
    let program_installed = program.as_ref().is_some_and(|path| path.is_file());
    let installed = hook_registered && program_installed;
    json!({
        "ok": true,
        "adapterId": "antigravity",
        "driverId": super::model::DRIVER_ID,
        "runtimeProtocol": super::model::RUNTIME_PROTOCOL,
        "installed": installed,
        "hookRegistered": hook_registered,
        // The hook is the package program, so "the hook is installed" and
        // "the program it names is present" are the same fact. The name is kept
        // because a reader that watched the script version asks it.
        "scriptInstalled": program_installed,
        "hookCommand": program
            .as_ref()
            .map(|path| hook_command_for(path)),
        "hookNamespace": HOOK_NAMESPACE,
    })
}

pub fn install_hook_bridge() -> Result<Value, &'static str> {
    ensure_hook_bridge().map_err(|failure| failure.code)?;
    let mut status = hook_bridge_status();
    if let Some(object) = status.as_object_mut() {
        object.insert("action".to_string(), json!("install"));
    }
    Ok(status)
}

pub fn uninstall_hook_bridge_report() -> Result<Value, &'static str> {
    uninstall_hook_bridge().map_err(|failure| failure.code)?;
    let mut status = hook_bridge_status();
    if let Some(object) = status.as_object_mut() {
        object.insert("action".to_string(), json!("uninstall"));
    }
    Ok(status)
}

/// Remove only the Lico-owned hook namespace and any retired helper script.
///
/// Leaves unrelated user hooks untouched. Safe to call when detaching or
/// updating this adapter module.
pub(crate) fn uninstall_hook_bridge() -> Result<(), ProtocolFailure> {
    let config_dir = gemini_config_dir()?;
    let hooks_path = config_dir.join("hooks.json");
    if hooks_path.exists() {
        let text = fs::read_to_string(&hooks_path).map_err(|_| {
            ProtocolFailure::new(
                "antigravity_hook_bridge_unavailable",
                "Antigravity hooks configuration could not be read.",
                "capability/hooks",
            )
        })?;
        let mut root = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
        if let Some(object) = root.as_object_mut() {
            object.remove(HOOK_NAMESPACE);
            if object.is_empty() {
                let _ = fs::remove_file(&hooks_path);
            } else {
                let encoded = serde_json::to_vec_pretty(&root).map_err(|_| {
                    ProtocolFailure::new(
                        "antigravity_hook_bridge_unavailable",
                        "Antigravity hooks configuration could not be encoded.",
                        "capability/hooks",
                    )
                })?;
                let temporary = hooks_path.with_extension("json.tmp");
                fs::write(&temporary, encoded).map_err(|_| {
                    ProtocolFailure::new(
                        "antigravity_hook_bridge_unavailable",
                        "Antigravity hooks configuration could not be written.",
                        "capability/hooks",
                    )
                })?;
                fs::rename(&temporary, &hooks_path).map_err(|_| {
                    ProtocolFailure::new(
                        "antigravity_hook_bridge_unavailable",
                        "Antigravity hooks configuration could not be updated.",
                        "capability/hooks",
                    )
                })?;
            }
        }
    }
    let directory = gemini_config_dir()?.join("lico-up-antigravity");
    let _ = fs::remove_file(directory.join(RETIRED_SCRIPT_NAME));
    // `remove_dir` refuses a non-empty directory, so a file this client did not
    // write is never removed with it.
    let _ = fs::remove_dir(&directory);
    Ok(())
}

pub(super) fn receipt_path_for_turn() -> Result<PathBuf, ProtocolFailure> {
    let root = receipt_root()?.join("receipts");
    fs::create_dir_all(&root).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity session receipt directory could not be created.",
            "capability/hooks",
        )
    })?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(root.join(format!("receipt-{}-{}.json", std::process::id(), nonce)))
}

pub(super) fn read_conversation_id(receipt: &Path) -> Option<String> {
    let text = fs::read_to_string(receipt).ok()?;
    parse_hook_receipt(&text)
}

/// The command the Stop hook runs: the package program and its receipt
/// subcommand, shell-quoted as one path.
pub(super) fn hook_command() -> Result<String, ProtocolFailure> {
    package_program_path()
        .map(|path| hook_command_for(&path))
        .ok_or_else(|| {
            ProtocolFailure::new(
                "antigravity_hook_bridge_unavailable",
                "The Antigravity adapter package program could not be found next to this client.",
                "capability/hooks",
            )
        })
}

fn hook_command_for(path: &Path) -> String {
    format!("{} {RECEIPT_SUBCOMMAND}", shell_quote(path))
}

/// The packaged program, resolved beside the running client.
///
/// The hook is started by the vendor client, not by LicoUp, so it must name an
/// absolute path that outlives this process. It is the same directory the
/// client's other packaged sidecars live in: a bundle's `Contents/MacOS`, or the
/// directory of the running executable for a development build.
fn package_program_path() -> Option<PathBuf> {
    let name = format!("{PACKAGE_PROGRAM}{}", std::env::consts::EXE_SUFFIX);
    let executable = std::env::current_exe().ok()?;
    let mut candidates = Vec::new();
    if let Some(directory) =
        licoup_foundation::platform::paths::packaged_binary_directory(&executable)
    {
        candidates.push(directory.join(&name));
    }
    // A cargo test binary runs from `target/<profile>/deps`, so the sibling
    // directory is where `cargo build` puts the package program. This is a
    // development layout only, which is why it exists in a debug build alone: a
    // shipped client resolves the bundle, never a checkout's build directory.
    #[cfg(debug_assertions)]
    if let Some(parent) = executable.parent().and_then(Path::parent) {
        candidates.push(parent.join(&name));
    }
    // The vendor client resolves the documented bundle location for an
    // installed application even when this process runs from a different path.
    candidates.push(PathBuf::from("/Applications/LicoUp.app/Contents/MacOS").join(name));
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Quote one path as a single POSIX shell word.
///
/// The vendor client passes the command to a shell, so a path containing a space
/// must survive as one word. Single quotes preserve everything except a single
/// quote itself, which is closed, escaped and reopened.
fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn receipt_root() -> Result<PathBuf, ProtocolFailure> {
    let root = licoup_foundation::platform::paths::portable_data_dir()
        .map_err(|_| {
            ProtocolFailure::new(
                "antigravity_hook_bridge_unavailable",
                "Antigravity hook bridge data root is unavailable.",
                "capability/hooks",
            )
        })?
        .join("antigravity");
    fs::create_dir_all(&root).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity hook bridge data root could not be created.",
            "capability/hooks",
        )
    })?;
    Ok(root)
}

fn gemini_config_dir() -> Result<PathBuf, ProtocolFailure> {
    if let Ok(override_dir) = std::env::var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity hook bridge could not resolve the user home directory.",
            "capability/hooks",
        )
    })?;
    Ok(PathBuf::from(home).join(".gemini").join("config"))
}

fn install_global_hook(command: &str) -> Result<(), ProtocolFailure> {
    let config_dir = gemini_config_dir()?;
    fs::create_dir_all(&config_dir).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity hooks configuration directory could not be created.",
            "capability/hooks",
        )
    })?;
    let hooks_path = config_dir.join("hooks.json");
    let mut root = if hooks_path.exists() {
        let text = fs::read_to_string(&hooks_path).map_err(|_| {
            ProtocolFailure::new(
                "antigravity_hook_bridge_unavailable",
                "Antigravity hooks configuration could not be read.",
                "capability/hooks",
            )
        })?;
        serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }
    // Only Stop is required for print-mode session receipt. Avoid earlier
    // lifecycle hooks that can overwrite the receipt with an empty id.
    let entry = json!({
        "enabled": true,
        "Stop": [
            {
                "type": "command",
                "command": command,
                "timeout": 10
            }
        ]
    });
    root.as_object_mut()
        .expect("hooks root object")
        .insert(HOOK_NAMESPACE.to_string(), entry);
    let encoded = serde_json::to_vec_pretty(&root).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity hooks configuration could not be encoded.",
            "capability/hooks",
        )
    })?;
    let temporary = hooks_path.with_extension("json.tmp");
    fs::write(&temporary, encoded).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity hooks configuration could not be written.",
            "capability/hooks",
        )
    })?;
    fs::rename(&temporary, &hooks_path).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_hook_bridge_unavailable",
            "Antigravity hooks configuration could not be installed.",
            "capability/hooks",
        )
    })?;
    // The generated script is retired: remove whatever a previous client left,
    // so exactly one hook command for this namespace exists on disk.
    let directory = config_dir.join("lico-up-antigravity");
    let _ = fs::remove_file(directory.join(RETIRED_SCRIPT_NAME));
    let _ = fs::remove_dir(&directory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting_keeps_one_path_one_word() {
        assert_eq!(
            shell_quote(Path::new(
                "/Applications/LicoUp.app/Contents/MacOS/lico-agent-antigravity"
            )),
            "'/Applications/LicoUp.app/Contents/MacOS/lico-agent-antigravity'"
        );
        assert_eq!(
            shell_quote(Path::new("/tmp/synthetic-bin/a b/lico-agent-antigravity")),
            "'/tmp/synthetic-bin/a b/lico-agent-antigravity'"
        );
        assert_eq!(
            shell_quote(Path::new("/tmp/synthetic-bin/o'brien/bin/x")),
            r"'/tmp/synthetic-bin/o'\''brien/bin/x'"
        );
    }

    #[test]
    fn the_hook_command_is_the_native_program_and_its_receipt_subcommand() {
        let command = hook_command_for(Path::new("/fixture/bin/lico-agent-antigravity"));
        assert_eq!(command, "'/fixture/bin/lico-agent-antigravity' receipt");
        // The retirement the acceptance states: no generated script and no
        // interpreter anywhere in the hook path.
        assert!(!command.contains("python"), "the hook needs no interpreter");
        assert!(!command.contains("sh -"), "the hook runs no shell script");
        assert!(
            !command.ends_with(".sh"),
            "the hook command is not a script: {command}"
        );
    }

    #[test]
    fn the_receipt_environment_name_is_the_packages_own() {
        assert_eq!(crate::hook::RECEIPT_ENV, "LICO_ANTIGRAVITY_SESSION_RECEIPT");
    }
}
