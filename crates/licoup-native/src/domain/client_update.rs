//! Fail-closed signed client update workflow with public metadata only.

mod apply;
// Linux can verify update metadata, but does not yet consume every
// platform-specific artifact descriptor field.
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod archive;
mod canonical;
mod check;
mod constants;
mod dispatch;
mod download;
mod github_source;
mod keys;
mod metadata;
mod native_runner;
// Linux can verify update metadata, but does not yet consume every
// platform-specific artifact descriptor field.
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod model;
mod params;
mod receipt;
mod release;
mod revocation;
mod selection;
mod signature;
mod staging;
mod status;
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod tree;
mod verify;

pub use apply::apply;
pub use check::check;
pub use constants::{CLIENT_UPDATE_MANIFEST_SCHEMA, CLIENT_UPDATE_MODE};
pub use dispatch::dispatch;
pub use download::download;
pub use status::status;
pub use verify::verify;

#[cfg(test)]
mod tests;
