//! The Codex app-server protocol vocabulary this package owns.
//!
//! It is the *wire* half of one Codex turn: the request identifiers, the phases
//! the handshake advances through, the effects a parsed frame produces, the
//! failure shape the parser reports, the effective settings a thread response
//! establishes, the launch configuration, and the reserve-model projection.
//!
//! It is here rather than in the client because it is Codex's protocol, and
//! ADR-0008 keeps vendor protocol below the adapter port: a raw app-server line
//! is parsed once, here, and nothing above the port re-parses it. A client that
//! carries no Codex package therefore carries no app-server field name at all.
//!
//! `driver` is the process half: the app-server this package starts, the
//! bounded transport it writes and reads, the supervision of one turn and the
//! live control channel into it. Both halves are the package's, so a client
//! that carries no Codex package starts no app-server and names no app-server
//! field.

pub mod config;
pub mod contract;
pub mod driver;
pub mod failure;
pub mod limits;
pub mod model;
pub mod reserve;
