//! Bounded resources for one extension instance: wall time per call, wire and
//! output budgets, and the POSIX limits the operating system really enforces.
//!
//! Every dimension is either applied or refused; none is silently ignored. A
//! limit that is `Some` but cannot be enforced on this host fails the start with
//! `extension_isolation_limit_unsupported`, because a policy that says "at most
//! 256 MiB" and does not enforce it is worse than no policy at all.
//!
//! What each dimension means here:
//!
//! - **Wall time** is the carrier's own bound on one request/response exchange.
//!   It is measured with the monotonic clock in the host process, not promised
//!   by the extension; a call that exceeds it faults as unresponsive and the
//!   supervised process group is torn down. Complete instance release requires
//!   the restricted mode's enforced single-process boundary.
//! - **Wire and output budgets** bound the bytes the carrier will buffer, so a
//!   chatty or hostile extension cannot grow host memory. They are enforced by
//!   the reader, never by trusting the extension to stop.
//! - **CPU seconds, file bytes, open files** are POSIX rlimits applied between
//!   fork and exec, so the extension cannot raise them back. They are *per
//!   process*: they bound a whole instance only where the instance is forced to
//!   be one process ([`crate::platform::extension_host::isolation::LimitScope`]).
//! - **Address space** is applied only where the platform enforces it. Darwin
//!   ignores `RLIMIT_AS`; asking for it there is refused instead of recorded as
//!   a ceiling that does not exist.

use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::{Deserialize, Serialize};

use licoup_application::ApplicationFailure;

use super::super::refusal;
use super::capability::{PlatformConfinement, Support};

/// The resource envelope one instance is granted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimits {
    /// How long one request/response exchange may take before it is a fault.
    pub call_wall_ms: u64,
    /// Largest single wire frame the carrier will accept from the extension.
    pub max_frame_bytes: u64,
    /// Total bytes the carrier will read from the extension's stdout.
    pub max_stdout_bytes: u64,
    /// Total bytes the carrier will keep from the extension's stderr.
    pub max_stderr_bytes: u64,
    pub cpu_seconds: Option<u64>,
    pub file_bytes: Option<u64>,
    pub open_files: Option<u64>,
    pub address_space_bytes: Option<u64>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            call_wall_ms: 5_000,
            max_frame_bytes: 64 * 1024,
            max_stdout_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 64 * 1024,
            cpu_seconds: None,
            file_bytes: None,
            open_files: None,
            address_space_bytes: None,
        }
    }
}

impl ResourceLimits {
    /// A starting envelope for a confined third-party program: bounded call
    /// time, bounded output, hard CPU and file ceilings, a small descriptor
    /// budget, and no address-space promise (the platform may not enforce it).
    pub fn confined() -> Self {
        Self {
            call_wall_ms: 5_000,
            cpu_seconds: Some(10),
            file_bytes: Some(8 * 1024 * 1024),
            open_files: Some(64),
            ..Self::default()
        }
    }

    pub fn call_wall(&self) -> Duration {
        Duration::from_millis(self.call_wall_ms)
    }
}

/// Exactly what was applied for one instance, recorded with its grant.
///
/// `None` means the dimension was not requested. A requested dimension that the
/// platform cannot enforce is refused before this value is built, so a `Some`
/// here is always backed by the operating system.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnforcedLimits {
    pub call_wall_ms: u64,
    pub max_frame_bytes: u64,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    pub cpu_seconds: Option<u64>,
    pub file_bytes: Option<u64>,
    pub open_files: Option<u64>,
    pub address_space_bytes: Option<u64>,
    pub core_dumps_disabled: bool,
}

impl ResourceLimits {
    pub(crate) fn enforced(self, core_dumps_disabled: bool) -> EnforcedLimits {
        EnforcedLimits {
            call_wall_ms: self.call_wall_ms,
            max_frame_bytes: self.max_frame_bytes,
            max_stdout_bytes: self.max_stdout_bytes,
            max_stderr_bytes: self.max_stderr_bytes,
            cpu_seconds: self.cpu_seconds,
            file_bytes: self.file_bytes,
            open_files: self.open_files,
            address_space_bytes: self.address_space_bytes,
            core_dumps_disabled,
        }
    }
}

/// The refusal for a limit this host cannot enforce.
pub(crate) fn unsupported_limit(field: &str, reason: &str) -> ApplicationFailure {
    refusal(
        "extension_isolation_limit_unsupported",
        "extension/isolation",
    )
    .with_field(field)
    .with_presentation_arg("limit", field)
    .with_presentation_arg("reason", reason)
}

/// Check a requested envelope against what this host can enforce.
pub(crate) fn check_supported(
    limits: &ResourceLimits,
    confinement: &PlatformConfinement,
) -> Result<(), ApplicationFailure> {
    let checks: [(bool, &Support, &str); 4] = [
        (
            limits.cpu_seconds.is_some(),
            &confinement.cpu_seconds,
            "cpuSeconds",
        ),
        (
            limits.file_bytes.is_some(),
            &confinement.file_bytes,
            "fileBytes",
        ),
        (
            limits.open_files.is_some(),
            &confinement.open_files,
            "openFiles",
        ),
        (
            limits.address_space_bytes.is_some(),
            &confinement.address_space,
            "addressSpaceBytes",
        ),
    ];
    for (requested, support, field) in checks {
        if requested && !support.is_enforced() {
            return Err(unsupported_limit(
                field,
                support.reason().unwrap_or("unavailable on this host"),
            ));
        }
    }
    Ok(())
}

/// Apply the POSIX rlimits of one envelope to a command.
///
/// The limits are set between `fork` and `exec`, so the extension starts already
/// bounded and cannot raise them back; a failure to set one aborts the spawn
/// instead of starting an unbounded process. On platforms without rlimits the
/// dimensions are already refused by [`check_supported`], so this is a no-op
/// there.
#[cfg(unix)]
pub(crate) fn apply(
    command: &mut std::process::Command,
    limits: &ResourceLimits,
    confirmation: &PlatformConfinement,
) -> Result<EnforcedLimits, ApplicationFailure> {
    check_supported(limits, confirmation)?;
    let mut rules: Vec<(libc::c_int, u64)> = Vec::new();
    if let Some(seconds) = limits.cpu_seconds {
        rules.push((libc::RLIMIT_CPU, seconds));
    }
    if let Some(bytes) = limits.file_bytes {
        rules.push((libc::RLIMIT_FSIZE, bytes));
    }
    if let Some(files) = limits.open_files {
        rules.push((libc::RLIMIT_NOFILE, files));
    }
    if let Some(bytes) = limits.address_space_bytes {
        rules.push((libc::RLIMIT_AS, bytes));
    }
    let core_dumps_disabled = confirmation.core_dumps.is_enforced();
    if core_dumps_disabled {
        rules.push((libc::RLIMIT_CORE, 0));
    }
    if !rules.is_empty() {
        // SAFETY: `pre_exec` runs in the forked child before exec. The closure
        // captures only plain integers and calls `setrlimit`, which is
        // async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                for (resource, value) in &rules {
                    let limit = libc::rlimit {
                        rlim_cur: *value,
                        rlim_max: *value,
                    };
                    if libc::setrlimit(*resource as _, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    Ok(limits.enforced(core_dumps_disabled))
}

#[cfg(not(unix))]
pub(crate) fn apply(
    _command: &mut std::process::Command,
    limits: &ResourceLimits,
    confirmation: &PlatformConfinement,
) -> Result<EnforcedLimits, ApplicationFailure> {
    check_supported(limits, confirmation)?;
    Ok(limits.enforced(false))
}
