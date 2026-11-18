//! The port this package asks its host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and it owns
//! nothing the client owns. Everything it needs from its host arrives here as a
//! value composition installs once per process, and the port is fail-closed
//! before it is installed — an uninstalled port answers nothing rather than
//! guessing, so a package started without its host cannot invent an effect.
//!
//! One seam is declared. The ACP transport engines this Agent's turns run
//! through stay shared in `licoup-agent-drivers`, so this package asks its host
//! for the decision it may not make for itself — whether a new execution is
//! admitted — and for nothing else. The shared engine's own
//! `raw_execution`/`turn_event` observers belong to the host's composition and
//! are not a second port here: a package that declared a seam it never reads
//! would be describing a conversation it does not have.

pub mod execution;
