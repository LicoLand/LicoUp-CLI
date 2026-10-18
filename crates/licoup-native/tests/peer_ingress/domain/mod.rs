//! Mounts the two production module trees at their real crate paths so the
//! suite compiles the exact source files the native host wires.
//!
//! `endpoint_transport` lives at `domain::mobile_relay::endpoint_transport`
//! and `peer_ingress` at `domain::client_conversation::peer_ingress` in the
//! crate; the same paths are reproduced here.

pub mod client_conversation;
pub mod mobile_relay;
