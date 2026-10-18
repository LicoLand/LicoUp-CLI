//! This package's own protocol declaration: the wire it speaks and the runtime
//! it declares to its host.
//!
//! The two names live here rather than in the release documents, because the
//! release declaration is read *by* the packaging tool and both the tooling and
//! this package's own artifact test must agree with what the program speaks. A
//! name retyped into a JSON document is a second copy that can drift; a name
//! this crate exports is the one the binary reports.

/// The vendor protocol this package's entry ingests, as the release declaration
/// reports it.
///
/// It is the framing the parser really speaks: a turn's facts arrive as the
/// official Agent Hooks receipt, the PTY lane's bytes and the supervised
/// process's own outcome, never as a vendor line protocol.
pub const PROTOCOL_FORMAT: &str = "antigravity.pty-hook.v1";

/// The runtime protocol revision this package declares to its host.
pub const RUNTIME_PROTOCOL: &str = "antigravity-cli-argv-hook-v1";
