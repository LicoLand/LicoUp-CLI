//! Where this package's folded usage samples go.
//!
//! One session artifact yields an ordered list of [`UsageSample`] values: the
//! attempts the Harness actually billed, with the route and effort each was made
//! under and the vendor's own token object. *What* those samples are is this
//! package's answer, because the row format is the vendor's; *what they mean for
//! the client's accounting* is the host's, because the host owns the calendar
//! window, the request records and the cache.
//!
//! The port is one installed function rather than a second accounting path: a
//! process installs [`install`] once, and every artifact this package folds
//! reaches the same consumer the host's own reader reached. Before installation
//! the port is fail-closed, which is the honest answer for a package running
//! outside the client.

use std::path::Path;
use std::sync::OnceLock;

use crate::session_store::{SessionReadError, UsageSample};

/// The host's consumer for one folded session artifact.
///
/// `size` is the artifact's length in bytes as the caller measured it, so the
/// host's own "how much of this file has been read" bookkeeping keeps working
/// without the package having to publish a second copy of it.
pub type UsageSink =
    fn(path: &Path, size: u64, samples: Vec<UsageSample>) -> Result<(), SessionReadError>;

static PORT: OnceLock<UsageSink> = OnceLock::new();

/// Install the host's consumer once per process.
///
/// A second installation is refused rather than silently replacing the first:
/// the consumer belongs to one process, and a second answer would mean two
/// accounting paths for one artifact.
pub fn install(sink: UsageSink) -> Result<(), &'static str> {
    PORT.set(sink)
        .map_err(|_| "the usage port is already installed")
}

/// Whether the host has installed its consumer.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// Hand one folded artifact to the host.
///
/// Fail-closed: with no consumer installed the samples are dropped rather than
/// written somewhere this package chose.
pub fn publish(path: &Path, size: u64, samples: Vec<UsageSample>) -> Result<(), SessionReadError> {
    match PORT.get() {
        Some(sink) => sink(path, size, samples),
        None => Ok(()),
    }
}

/// Read one artifact and hand its samples to the host in one step.
pub fn read_and_publish(path: &Path, size: u64) -> Result<(), SessionReadError> {
    let samples = crate::session_store::read_usage_samples(path)?;
    publish(path, size, samples)
}
