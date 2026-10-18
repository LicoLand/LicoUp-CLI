//! Single confirmation token bound to one planned Agent Hub action.
//!
//! The token binds the requested version as well as the action and channel: for
//! a vendor-binary channel the version selects which artifact is fetched, so a
//! confirmed plan must not apply a different one.

use super::contract::InstallChannel;
use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};

pub fn token(operation: &str, agent_id: &str, channel: &InstallChannel, version: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(operation.as_bytes());
    digest.update([0]);
    digest.update(agent_id.as_bytes());
    digest.update([0]);
    digest.update(channel.id.as_bytes());
    digest.update([0]);
    digest.update(channel.kind.as_bytes());
    digest.update([0]);
    digest.update(channel.package_coordinate.as_bytes());
    digest.update([0]);
    digest.update(version.as_bytes());
    digest.update([0]);
    for arg in install_argv_for("macos", channel) {
        digest.update(arg.as_bytes());
        digest.update([0]);
    }
    // A channel that places its own result states where that result lands, so
    // the declared destination and the placement step bind the token too: a
    // confirmed plan must not write to a destination the caller never saw.
    if let Some(install) = channel.install.as_ref() {
        for (os, dir) in &install.dir {
            digest.update(os.as_bytes());
            digest.update([0]);
            digest.update(dir.as_bytes());
            digest.update([0]);
        }
        for (os, binary) in &install.binary {
            digest.update(os.as_bytes());
            digest.update([0]);
            digest.update(binary.as_bytes());
            digest.update([0]);
        }
        for arg in &install.argv {
            digest.update(arg.as_bytes());
            digest.update([0]);
        }
    }
    let encoded = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("agent-hub:{operation}:{agent_id}:{}:{encoded}", channel.id)
}

pub fn require(params: &serde_json::Value, expected: &str) -> Result<()> {
    let provided = params
        .get("confirmation")
        .or_else(|| params.get("confirm"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("confirmation_required"))?;
    ensure!(provided == expected, "confirmation_mismatch");
    Ok(())
}

pub fn install_argv_for(os: &str, channel: &InstallChannel) -> Vec<String> {
    if os == "windows" && !channel.windows_install_argv.is_empty() {
        channel.windows_install_argv.clone()
    } else {
        channel.install_argv.clone()
    }
}
