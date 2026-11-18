//! Verified conversation readiness callback.
//!
//! Readiness is pushed to the running gateway as a document; applying it stays
//! the host's answer, installed here as a callback. Nothing pulls readiness
//! from the host.

use std::path::Path;
use std::sync::RwLock;

const MAX_READINESS_BYTES: usize = 4 * 1024 * 1024;

pub type ReadinessApplier = fn(&str) -> Result<(), &'static str>;

static READINESS: RwLock<Option<ReadinessApplier>> = RwLock::new(None);

/// Install the host answer that applies a verified readiness document.
/// Reinstalling the same answer is accepted so repeated startup is harmless.
pub fn install(applier: ReadinessApplier) -> Result<(), &'static str> {
    let mut guard = READINESS
        .write()
        .map_err(|_| "gateway_readiness_port_lock_failed")?;
    match guard.as_ref() {
        Some(installed) if std::ptr::fn_addr_eq(*installed, applier) => return Ok(()),
        Some(_) => return Err("gateway_readiness_port_already_installed"),
        None => {}
    }
    *guard = Some(applier);
    Ok(())
}

pub fn installed() -> bool {
    READINESS
        .read()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
pub fn clear() {
    if let Ok(mut guard) = READINESS.write() {
        *guard = None;
    }
}

/// Apply one readiness document through the installed callback.
pub fn reload_document(readiness_json: &str) -> Result<(), String> {
    let applier = READINESS
        .read()
        .ok()
        .and_then(|guard| *guard)
        .ok_or_else(|| "gateway_readiness_port_unavailable".to_owned())?;
    applier(readiness_json).map_err(str::to_owned)
}

/// Read a bounded readiness overlay file and apply it.
pub fn reload_from_path(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|_| "readiness_overlay_read_failed".to_owned())?;
    if bytes.len() > MAX_READINESS_BYTES {
        return Err("readiness_overlay_too_large".to_owned());
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|_| "readiness_overlay_invalid_utf8".to_owned())?;
    reload_document(text)
}
