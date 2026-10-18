//! Shared protocol contracts for the native host.
//!
//! - [`frame`]: the L2/L4-common Frame Layer (bounded streaming frame decode
//!   and encode). It is payload-agnostic.
//! - [`conversation_protocol`]: generated Payload Layer for the
//!   `licoup.stdio.v1` method registry, typed Command decode (Rust) and Delta
//!   builders, produced by `tools/scripts/protocol-codegen/generate-conversation-protocol.mjs`
//!   from `schemas/conversation_protocol/`. Do not edit by hand.

pub mod conversation_protocol;
pub mod frame;
