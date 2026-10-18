//! LicoUp credential vault contracts.
//!
//! API-key bytes never appear in an inventory, configuration document, debug
//! projection, or command result. Platform custody owns persistence and
//! authorization; a runtime receives only a process-bound, revocable in-memory
//! lease through [`port`].

pub mod api_key;
pub mod port;
