//! Debug-surface gating for diagnostic evidence.
//!
//! Verification evidence — readiness states, evidence ages, pass counts and
//! summary codes — is diagnostic state, not runtime admission fact. Normal
//! operation never projects it: available routes are attempted on structural
//! prerequisites and fail at the operation that actually fails. Setting
//! `LICOUP_DEBUG` to any non-empty value other than `0`, `false`, `off` or
//! `no` exposes the diagnostic surface for inspection.

use std::cell::Cell;

const DEBUG_ENV: &str = "LICOUP_DEBUG";

thread_local! {
    static DEBUG_DIAGNOSTICS_OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether diagnostic evidence may be projected into client-facing output.
pub fn debug_enabled() -> bool {
    DEBUG_DIAGNOSTICS_OVERRIDE
        .with(Cell::get)
        .unwrap_or_else(environment_debug_enabled)
}

fn environment_debug_enabled() -> bool {
    std::env::var(DEBUG_ENV)
        .map(|value| {
            let value = value.trim();
            !value.is_empty()
                && !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
        })
        .unwrap_or(false)
}

/// Pin the debug-diagnostics answer for the calling thread. Tests use this
/// the way they use `set_portable_data_dir_override`: the override is
/// thread-local, so parallel tests never share the flag.
#[doc(hidden)]
pub fn set_debug_diagnostics_override(enabled: Option<bool>) -> Option<bool> {
    DEBUG_DIAGNOSTICS_OVERRIDE.with(|value| value.replace(enabled))
}
