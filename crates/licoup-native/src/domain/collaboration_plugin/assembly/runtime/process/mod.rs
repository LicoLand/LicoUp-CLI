#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::collaboration_plugin::assembly) enum ProcessLiveness {
    Alive,
    Dead,
    Unavailable,
}

#[cfg(unix)]
pub(super) use unix::{capture_identity, liveness};
#[cfg(windows)]
pub(super) use windows::{capture_identity, liveness};

#[cfg(not(any(unix, windows)))]
pub(super) fn capture_identity(_pid: u32) -> anyhow::Result<String> {
    Err(anyhow::anyhow!(
        "collaboration_local_server_platform_unsupported"
    ))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn liveness(_pid: u32) -> ProcessLiveness {
    ProcessLiveness::Unavailable
}
