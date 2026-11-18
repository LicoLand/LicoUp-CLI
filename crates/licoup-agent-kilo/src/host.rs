//! The composition seam: what a host installs to run this package.
//!
//! A composer — the extension host that starts this package's binary, or the
//! client while the binary route is completed — installs the two ports this
//! package needs before it asks the package to do anything. One call states both
//! because a package that has a serve engine but no event sink, or an event sink
//! but no engine, is half-wired: it would either perform an effect it cannot
//! report or report an effect it cannot perform.
//!
//! The ports are installed once per process and are first-wins, so a second
//! composition cannot silently replace the consumer a running turn is writing
//! to. A package that is never installed is fail-closed rather than broken: the
//! parser, the replay corpus and the endpoint policy are all fully exercisable
//! with no host at all.

use crate::port::execution::{self, ExecutionPort};
use crate::port::serve::{self, ServePort};
use crate::port::turn_event::{self, TurnEventPort};

/// The ports one host installs for this package.
#[derive(Clone, Copy)]
pub struct HostPorts {
    /// Where this Agent's progressive turn events go.
    pub turn_event: TurnEventPort,
    /// The shared local-service engine this Agent's turn runs on.
    pub serve: ServePort,
}

/// Install this package's ports for one process.
///
/// Both installations are attempted and the first refusal is reported, so a host
/// learns that its composition was refused rather than that half of it was
/// accepted. Installation is idempotent only in the sense that an identical
/// second installation is still a refusal: the consumer and the engine belong to
/// one process.
pub fn install(ports: HostPorts) -> Result<(), &'static str> {
    turn_event::install(ports.turn_event)?;
    serve::install(ports.serve)
}

/// Install the agent-execution answers, when the host has them.
///
/// It is a separate call because a host that drives this package in-process has
/// no second side to answer: the execution port exists for the extension host
/// that starts this package's binary, and a host that installs nothing stays
/// fail-closed.
pub fn install_execution(port: ExecutionPort) -> Result<(), &'static str> {
    execution::install(port)
}

/// Whether this package has the ports it needs to run a turn.
pub fn installed() -> bool {
    turn_event::installed() && serve::installed()
}

/// The ports this process installed, or `None` before composition.
///
/// It reads the one installation rather than keeping a second copy of it: a
/// port answered here is the port this package's own accessors reach, so the two
/// names cannot disagree about which engine a turn runs on.
pub fn ports() -> Option<HostPorts> {
    Some(HostPorts {
        turn_event: turn_event::port()?,
        serve: serve::port()?,
    })
}
