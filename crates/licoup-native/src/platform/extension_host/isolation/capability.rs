//! What this build can actually enforce, dimension by dimension.
//!
//! A claim of confinement is only useful if it names the mechanism and refuses
//! to run when that mechanism is absent. [`PlatformConfinement::detect`] probes
//! the real host: Seatbelt is reported only when `/usr/bin/sandbox-exec` passes
//! the same integrity check the existing `process_sandbox` profiles use, and the
//! other dimensions are reported from the platform's own facilities rather than
//! from a wish list. `Unavailable` is a complete, visible answer; it is never
//! translated into "assume it is fine".

use serde::{Deserialize, Serialize};

/// How a program is run.
///
/// The two modes are deliberately not interchangeable: a trusted local program
/// is the user's own software, run as the user with no filesystem or network
/// confinement; a restricted program is third-party code the host confines to a
/// declared envelope. Choosing the weaker mode is an explicit, recorded
/// decision, never a fallback.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationMode {
    /// The user supplied this program themselves. It runs unconfined; the
    /// record says so and no confinement is claimed.
    TrustedLocal,
    /// Third-party code confined to its declared envelope. Refused when the
    /// platform cannot enforce what the mode claims.
    Restricted,
}

impl IsolationMode {
    pub const fn id(self) -> &'static str {
        match self {
            Self::TrustedLocal => "trusted-local",
            Self::Restricted => "restricted",
        }
    }

    /// Whether this mode makes a confinement claim that must be backed by the
    /// operating system.
    pub const fn claims_confinement(self) -> bool {
        matches!(self, Self::Restricted)
    }
}

/// Whether one mechanism exists on this host, and what would provide it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Support {
    /// The mechanism was found on this host and is used when requested.
    Enforced { mechanism: String },
    /// The mechanism is absent or cannot be trusted on this host. The reason is
    /// carried so a surface can show why, and no run pretends otherwise.
    Unavailable { reason: String },
}

impl Support {
    pub fn enforced(mechanism: &str) -> Self {
        Self::Enforced {
            mechanism: mechanism.to_owned(),
        }
    }

    pub fn unavailable(reason: &str) -> Self {
        Self::Unavailable {
            reason: reason.to_owned(),
        }
    }

    pub const fn is_enforced(&self) -> bool {
        matches!(self, Self::Enforced { .. })
    }

    pub fn mechanism(&self) -> Option<&str> {
        match self {
            Self::Enforced { mechanism } => Some(mechanism.as_str()),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unavailable { reason } => Some(reason.as_str()),
            Self::Enforced { .. } => None,
        }
    }
}

/// The operating-system controls this host can really apply.
///
/// This is a fact about *this build on this machine*, read before anything is
/// started; it is not a product support matrix and it is not serialized into
/// any public contract. `filesystem_scopes` and `network_deny` come from
/// Seatbelt; `single_process` from the Seatbelt `process-fork` denial;
/// `process_group` from process-group teardown; the limit dimensions from POSIX
/// rlimits (per process — an instance limit only under `single_process`).
/// Anything this host cannot enforce stays `Unavailable`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformConfinement {
    /// Path-scoped reads and writes for the confined process.
    pub filesystem_scopes: Support,
    /// Denial of outbound and inbound network access (or a loopback port only).
    pub network_deny: Support,
    /// Whether a restricted instance is forced to stay a single process: the
    /// sandbox denies `process-fork`, which on this host also denies fork,
    /// vfork, `posix_spawn` and everything built on them (measured for
    /// `os.fork`, `os.posix_spawn` and `subprocess`; threads stay allowed
    /// because they live inside the same process). When this is enforced, the
    /// supervised wait covers the whole instance, and the per-process limits
    /// below are instance limits.
    pub single_process: Support,
    /// Termination of the supervised process group, including descendants that
    /// stay in it and still hold the instance's pipes. This is the scope a
    /// trusted local instance gets; it is *not* a whole-tree guarantee.
    pub process_group: Support,
    /// Reclamation of a descendant that leaves the process group with
    /// `setsid`/`setpgid`. No portable mechanism is implemented, and the macOS
    /// sandbox does not refuse the call (measured: a confined interpreter can
    /// call `setsid` and leave the group), so this is not claimed. Such a
    /// descendant stays inside the sandbox; it is simply not killed. A
    /// restricted instance cannot have descendants at all, so this concerns
    /// trusted local programs.
    pub group_escape: Support,
    /// Hard CPU-seconds limit (`RLIMIT_CPU`). Per process: it bounds a whole
    /// instance only where `single_process` is enforced.
    pub cpu_seconds: Support,
    /// Maximum size of one file the process may write (`RLIMIT_FSIZE`), also one
    /// file per process. Per process for the same reason as CPU seconds.
    pub file_bytes: Support,
    /// Maximum number of open file descriptors (`RLIMIT_NOFILE`). Per process
    /// for the same reason as CPU seconds.
    pub open_files: Support,
    /// Hard address-space limit (`RLIMIT_AS`). Not claimed where the platform
    /// ignores it.
    pub address_space: Support,
    /// Whether core dumps can be disabled for the process (`RLIMIT_CORE`).
    pub core_dumps: Support,
}

impl PlatformConfinement {
    /// Probe this host.
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        let seatbelt = match super::confinement::sandbox_exec_is_trusted() {
            true => Support::enforced("macos-seatbelt"),
            false => Support::unavailable(
                "macos seatbelt is present but /usr/bin/sandbox-exec did not pass the integrity check",
            ),
        };
        #[cfg(not(target_os = "macos"))]
        let seatbelt = Support::unavailable(
            "filesystem and network confinement are implemented for macOS seatbelt in this build",
        );

        #[cfg(target_os = "macos")]
        let single_process = Support::enforced("macos-seatbelt-process-fork-denied");
        #[cfg(not(target_os = "macos"))]
        let single_process = Support::unavailable(
            "no mechanism forces a restricted instance to stay a single process on this platform",
        );

        #[cfg(unix)]
        let process_group = Support::enforced("posix-process-group");
        #[cfg(not(unix))]
        let process_group =
            Support::unavailable("process-group teardown is not verified on this platform");

        #[cfg(unix)]
        let group_escape = Support::unavailable(
            "trusted local programs may spawn descendants that leave the group with setsid/setpgid; they stay inside the sandbox, but this build does not reclaim them or prove their absence",
        );
        #[cfg(not(unix))]
        let group_escape =
            Support::unavailable("no process supervision is implemented on this platform");

        #[cfg(unix)]
        let rlimits = Support::enforced("posix-rlimit");
        #[cfg(not(unix))]
        let rlimits = Support::unavailable("POSIX resource limits are absent on this platform");

        #[cfg(target_os = "linux")]
        let address_space = Support::enforced("linux-rlimit-as");
        #[cfg(target_os = "macos")]
        let address_space =
            Support::unavailable("darwin does not enforce RLIMIT_AS; no memory ceiling is claimed");
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        let address_space = Support::unavailable("no address-space limit is implemented here");

        Self {
            filesystem_scopes: seatbelt.clone(),
            network_deny: seatbelt,
            single_process,
            process_group,
            group_escape,
            cpu_seconds: rlimits.clone(),
            file_bytes: rlimits.clone(),
            open_files: rlimits.clone(),
            address_space,
            core_dumps: rlimits,
        }
    }

    /// Whether a restricted run can be honoured at all on this host.
    ///
    /// A restricted run promises a complete instance boundary, so it needs the
    /// filesystem and network confinement *and* the single-process enforcement:
    /// without the latter, a released instance could leave descendants that
    /// nobody can verify.
    pub fn supports_restricted(&self) -> bool {
        self.filesystem_scopes.is_enforced()
            && self.network_deny.is_enforced()
            && self.single_process.is_enforced()
    }
}
