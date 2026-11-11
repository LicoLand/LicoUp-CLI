use anyhow::Result;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir, read_private_text_bounded,
};
use std::path::Path;

const MAX_CONFIG_BYTES: usize = 16 * 1024;

pub(super) fn ensure_minimal(config_path: &Path, port: u16) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        ensure_private_dir(parent)?;
    }
    if read_private_text_bounded(config_path, MAX_CONFIG_BYTES)?.is_some() {
        return Ok(());
    }
    let body = format!(
        "{{\n  \"gateway\": {{\n    \"mode\": \"local\",\n    \"port\": {port},\n    \"bind\": \"loopback\",\n    \"auth\": {{ \"mode\": \"none\" }}\n  }}\n}}\n"
    );
    atomic_write_private_text_bounded(config_path, &body, MAX_CONFIG_BYTES)
}
