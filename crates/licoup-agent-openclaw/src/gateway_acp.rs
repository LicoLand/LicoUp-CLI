//! The Gateway ACP driver half this package owns.
//!
//! OpenClaw attaches an ACP bridge to its own local Gateway; the wire between
//! the two is this package's vendor protocol, and the vocabulary a driver needs
//! to speak it lives here:
//!
//! - [`errors`] is the typed protocol failure and its payload — the code, the
//!   stage, whether explicit user interaction is required, and the identifiers a
//!   caller may project.
//! - [`model`] is the run vocabulary: the runtime-protocol identity, the
//!   effective settings a completed turn reported, the bounded capability probe
//!   and the run result itself.
//! - [`params`] is one OpenClaw request, validated and normalized before any
//!   process exists: which parameters this Agent accepts, which it refuses with
//!   a typed failure, and how a private prompt stays out of launch arguments.
//! - [`continuity`] is the session binding: how one ACP protocol session is tied
//!   to the resumable Gateway conversation key, and how a mismatch is refused
//!   rather than resumed into the wrong conversation.
//!
//! What is *not* here is the process half — spawning the bridge, reading its
//! framed lines, supervising the turn and bounding its output. That is
//! [`crate::driver`], which reads this vocabulary at its own paths rather than
//! through a second name; the client holds no copy of either half.

pub mod continuity;
pub mod contract;
pub mod errors;
pub mod model;
pub mod params;

pub use model::{
    CapabilityProbe, EffectiveSettings, PROCESS_POLL_INTERVAL, RUNTIME_PROTOCOL, RunResult,
};
