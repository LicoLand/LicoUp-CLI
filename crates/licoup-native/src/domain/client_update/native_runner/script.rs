//! Fixed native script templates executed by the OS-bundled interpreter.
//!
//! Values are passed strictly as positional argv (`$1..$n` in /bin/sh,
//! `$args[n]` in PowerShell) and are never interpolated into script text.
//! Every value is validated against a conservative printable-ASCII charset
//! that excludes shell metacharacters, and paths must be absolute with a
//! guarded replacement target.

use anyhow::{Result, ensure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::client_update) enum ScriptAction {
    Apply,
}

pub(super) fn script_extension() -> &'static str {
    if cfg!(windows) { "ps1" } else { "sh" }
}

pub(super) fn apply_script(_action: ScriptAction) -> &'static str {
    match std::env::consts::OS {
        "macos" => MACOS_APPLY_SH,
        "linux" => LINUX_APPLY_SH,
        "windows" => WINDOWS_APPLY_PS1,
        _ => unreachable!("platform gating happens in the apply plan"),
    }
}

/// Validates every positional argv value that will reach a script.
pub(in crate::domain::client_update) fn validate_script_args(args: &[&str]) -> Result<()> {
    for (index, value) in args.iter().enumerate() {
        validate_arg(value, index)?;
    }
    Ok(())
}

/// Validates path argv values: printable safe charset plus absolute paths
/// (`/...`, `\\...` or a drive-letter prefix).
pub(in crate::domain::client_update) fn validate_script_paths(paths: &[&str]) -> Result<()> {
    for (index, value) in paths.iter().enumerate() {
        validate_arg(value, index)?;
        let absolute = value.starts_with('/')
            || value.starts_with("\\\\")
            || value.as_bytes().get(1) == Some(&b':');
        ensure!(
            absolute,
            "client update script path argument {index} must be absolute"
        );
    }
    Ok(())
}

fn validate_arg(value: &str, index: usize) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 512,
        "client update script argument {index} is invalid"
    );
    ensure!(
        value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
        "client update script argument {index} contains unsupported characters"
    );
    for forbidden in [
        '"', '\'', '$', '`', ';', '&', '|', '>', '<', '(', ')', '*', '?', '[', ']', '{', '}', '#',
        '~', '!',
    ] {
        ensure!(
            !value.contains(forbidden),
            "client update script argument {index} contains forbidden characters"
        );
    }
    Ok(())
}

pub(super) fn validate_bundle_id_arg(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 255
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')),
        "client update bundleId is invalid"
    );
    Ok(())
}

/// macOS apply: quit via osascript (best effort), wait for the GUI to exit,
/// replace it with a pre-claim sibling backup, re-register LaunchServices,
/// and relaunch through LaunchServices. The candidate deletes the backup when
/// it atomically claims the verified handoff before state admission. The
/// launcher remains attached until claim, rejection, or pre-claim process
/// exit; there is no time-based interruption. The signed archive nests the
/// .app inside the extraction root, so the staged app is `EXPANDED/APP_DIR`.
const MACOS_APPLY_SH: &str = r#"#!/bin/sh
set -eu
APP_DIR="$1"
INSTALL_ROOT="$2"
EXPANDED="$3"
GUI_PID="$4"
BUNDLE_ID="$5"
BACKUP="$6"
HANDOFF="$7"
REJECTED="$HANDOFF.rejected"
TARGET="$INSTALL_ROOT/$APP_DIR"
STAGED_APP="$EXPANDED/$APP_DIR"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$TARGET" >/dev/null 2>&1
CURRENT_REQUIREMENT=$(/usr/bin/codesign --display --requirements - "$TARGET" 2>&1)
CURRENT_REQUIREMENT=${CURRENT_REQUIREMENT##*designated => }
/usr/bin/codesign --verify --deep --strict -R="$CURRENT_REQUIREMENT" "$STAGED_APP" >/dev/null 2>&1
/usr/bin/xcrun stapler validate "$STAGED_APP" >/dev/null 2>&1
/usr/sbin/spctl --assess --type execute "$STAGED_APP" >/dev/null 2>&1
/usr/bin/osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
while /bin/kill -0 "$GUI_PID" 2>/dev/null; do
  /bin/sleep 1
done
/bin/rm -rf -- "$BACKUP"
/bin/mv -- "$TARGET" "$BACKUP"
restore_pre_claim() {
  /bin/rm -rf -- "$TARGET"
  if [ -e "$BACKUP" ]; then /bin/mv -- "$BACKUP" "$TARGET"; fi
  /bin/rm -f -- "$HANDOFF"
  /bin/rm -f -- "$REJECTED"
}
if ! /usr/bin/ditto "$STAGED_APP" "$TARGET"; then
  restore_pre_claim
  exit 1
fi
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$TARGET" >/dev/null 2>&1 || true
/usr/bin/mdimport "$TARGET" >/dev/null 2>&1 || true
/usr/bin/open -W "$TARGET" >/dev/null 2>&1 &
LAUNCHER_PID=$!
while :; do
  if [ -e "$REJECTED" ]; then
    /usr/bin/osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
    wait "$LAUNCHER_PID" 2>/dev/null || true
    restore_pre_claim
    exit 1
  fi
  if [ ! -e "$HANDOFF" ] || /usr/bin/grep -q '"state":"claimed"' "$HANDOFF" 2>/dev/null; then
    exit 0
  fi
  if ! /bin/kill -0 "$LAUNCHER_PID" 2>/dev/null; then
    wait "$LAUNCHER_PID" 2>/dev/null || true
    restore_pre_claim
    exit 1
  fi
  /bin/sleep 1
done
"#;

/// Linux apply: wait for the GUI to exit, atomically replace the
/// bundle directory and relaunch detached. Only coreutils and /bin/sh
/// builtins are used; no tar/unzip/pkill/pgrep.
const LINUX_APPLY_SH: &str = r#"#!/bin/sh
set -eu
INSTALL_ROOT="$1"
EXPANDED="$2"
GUI_PID="$3"
BACKUP="$4"
HANDOFF="$5"
REJECTED="$HANDOFF.rejected"
APP_NAME="licoup"
while /bin/kill -0 "$GUI_PID" 2>/dev/null; do
  /bin/sleep 1
done
/bin/rm -rf -- "$BACKUP"
/bin/mv -- "$INSTALL_ROOT" "$BACKUP"
restore_pre_claim() {
  /bin/rm -rf -- "$INSTALL_ROOT"
  if [ -e "$BACKUP" ]; then /bin/mv -- "$BACKUP" "$INSTALL_ROOT"; fi
  /bin/rm -f -- "$HANDOFF"
  /bin/rm -f -- "$REJECTED"
}
if ! /bin/cp -R "$EXPANDED" "$INSTALL_ROOT"; then
  restore_pre_claim
  exit 1
fi
/usr/bin/nohup "$INSTALL_ROOT/$APP_NAME" >/dev/null 2>&1 &
NEW_PID=$!
while :; do
  if [ -e "$REJECTED" ]; then
    /bin/kill "$NEW_PID" 2>/dev/null || true
    wait "$NEW_PID" 2>/dev/null || true
    restore_pre_claim
    exit 1
  fi
  if [ ! -e "$HANDOFF" ] || /bin/grep -q '"state":"claimed"' "$HANDOFF" 2>/dev/null; then
    exit 0
  fi
  if ! /bin/kill -0 "$NEW_PID" 2>/dev/null; then
    wait "$NEW_PID" 2>/dev/null || true
    restore_pre_claim
    exit 1
  fi
  /bin/sleep 1
done
"#;

/// Windows apply: PowerShell cmdlets only; the Remove-Item retry loop covers
/// the window where the CLI process is still unlinking from inside the
/// install directory.
const WINDOWS_APPLY_PS1: &str = r#"$ErrorActionPreference = 'Stop'
$installRoot = $args[0]
$expanded = $args[1]
$guiPid = [int]$args[2]
$backup = $args[3]
$handoff = $args[4]
$rejected = "$handoff.rejected"
$appName = 'licoup.exe'
while ($null -ne (Get-Process -Id $guiPid -ErrorAction SilentlyContinue)) {
  Start-Sleep -Seconds 1
}
if (Test-Path -LiteralPath $backup) {
  Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction Stop
}
Move-Item -LiteralPath $installRoot -Destination $backup -Force
try {
  Copy-Item -LiteralPath $expanded -Destination $installRoot -Recurse -Force
  $candidate = Start-Process -FilePath (Join-Path $installRoot $appName) -WorkingDirectory $installRoot -PassThru
  while ($true) {
    if (Test-Path -LiteralPath $rejected) {
      Stop-Process -Id $candidate.Id -Force -ErrorAction SilentlyContinue
      throw 'candidate rejected the update handoff'
    }
    if (-not (Test-Path -LiteralPath $handoff)) {
      break
    }
    try {
      $handoffState = (Get-Content -LiteralPath $handoff -Raw | ConvertFrom-Json).state
    } catch {
      $handoffState = ''
    }
    if ($handoffState -eq 'claimed') {
      break
    }
    if ($candidate.HasExited) {
      throw 'candidate exited before claiming the update handoff'
    }
    Start-Sleep -Seconds 1
  }
} catch {
  if (Test-Path -LiteralPath $installRoot) {
    Remove-Item -LiteralPath $installRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
  if (Test-Path -LiteralPath $backup) {
    Move-Item -LiteralPath $backup -Destination $installRoot -Force
  }
  Remove-Item -LiteralPath $handoff -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $rejected -Force -ErrorAction SilentlyContinue
  throw
}
"#;

/// Platform-independent template access for tests so the windows scripts can
/// be asserted from any build machine.
#[cfg(test)]
pub(in crate::domain::client_update) fn platform_script_for_test(
    platform: &str,
    _action: ScriptAction,
) -> &'static str {
    match platform {
        "macos" => MACOS_APPLY_SH,
        "linux" => LINUX_APPLY_SH,
        "windows" => WINDOWS_APPLY_PS1,
        _ => unreachable!("unknown platform in test"),
    }
}
