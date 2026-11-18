//! The ports this package asks the host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and it owns
//! nothing the client owns. Everything it needs from its host arrives here as a
//! value composition installs once per process, and every port is fail-closed
//! before it is installed — an uninstalled port answers nothing rather than
//! guessing, so a package started without its host cannot invent an effect.
//!
//! Two seams are declared here:
//!
//! - [`usage`] is where one folded session artifact goes. The client owns the
//!   caller — the usage pipeline that decides which artifacts are in scope, how
//!   a sample becomes a request record and which calendar day it lands on — and
//!   this package owns the fold that reads the vendor's own row format, so the
//!   package hands the samples over rather than reimplementing the accounting.
//! - [`launch_environment`] is the environment one launch observes. The host
//!   owns the user's login-shell snapshot and reads it once per process; this
//!   package owns the launch, so it applies the host's answer to its own command
//!   rather than reading a second copy of the login-shell rules.
//!
//! The agent-execution port is deliberately *not* declared. Running one Harness
//! turn is this package's driver, [`crate::driver`], and it asks its host for
//! nothing beyond the launch environment above: the shared process supervisor,
//! the raw-execution record and the turn-event emitters are
//! `licoup-foundation`'s, and the adapter registry the transports are pooled in
//! is `licoup-agent-adapter-sdk`'s. A seam nothing answers would describe a route
//! that does not exist.

pub mod launch_environment;
pub mod usage;
