//! OpenCode headless serve facade.
//!
//! The Agent's own half of a turn — the launch declaration, the session-open
//! protocol, the request shape, the stream classification, the capability probe
//! and the endpoint contract — belongs to the OpenCode adapter package
//! (`licoup-agent-opencode`). What stays here is the one thing a package cannot
//! hold: the client's own serve engine, reached at this facade's width and run
//! on the package's specification ([`policy`]).
//!
//! Nothing here classifies a frame or owns a vendor fact: [`policy::SPEC`] is
//! assembled from the package's `policy`, the readiness reader is the package's
//! parser, and force-stop control reads the same descriptor through
//! [`CONTROL_SPEC`].

mod policy;

use anyhow::Result;
use serde_json::Value;
use std::sync::atomic::AtomicBool;

use super::local_service::{self, http::HttpFailure};

pub use super::local_service::ServeEndpoint;

/// The durable serve owner descriptor used by force-stop control. Control
/// reads the same state and pid records this owner writes.
pub(in crate::platform) const CONTROL_SPEC: super::local_service::ServeSpec = policy::SPEC;
/// The port this Agent's endpoint prefers, read from the package that owns it.
pub const DEFAULT_PORT: u16 = licoup_agent_opencode::policy::SPEC.default_port;

pub fn ensure(params: &Value) -> Result<Value> {
    local_service::serve::ensure(policy::SPEC, params)
}

pub fn start(params: &Value) -> Result<Value> {
    local_service::serve::start(policy::SPEC, params)
}

pub fn restart(params: &Value) -> Result<Value> {
    local_service::serve::restart(policy::SPEC, params)
}

pub fn stop(_params: &Value) -> Result<Value> {
    local_service::serve::stop(policy::SPEC)
}

pub fn status(_params: &Value) -> Result<Value> {
    local_service::serve::status(policy::SPEC)
}

pub fn ensure_attach_endpoint(executable: &str) -> Result<ServeEndpoint> {
    local_service::serve::ensure_attach_endpoint(policy::SPEC, executable)
}

pub(super) fn ensure_attachment(executable: &str) -> Result<local_service::ServeAttachment> {
    local_service::serve::ensure_attachment(policy::SPEC, executable)
}

pub fn select_available_port(preferred: u16) -> Result<u16> {
    local_service::serve::select_available_port(policy::SPEC, preferred)
}

pub fn select_available_port_with<F>(preferred: u16, is_bindable: F) -> Result<u16>
where
    F: Fn(u16) -> bool,
{
    local_service::serve::select_available_port_with(policy::SPEC, preferred, is_bindable)
}

pub fn is_reserved_conflict_port(port: u16) -> bool {
    local_service::serve::is_reserved_port(policy::SPEC, port)
}

pub(super) fn get_json(url: &str) -> std::result::Result<Value, HttpFailure> {
    local_service::http::get_json(url, std::time::Duration::from_secs(5))
}

pub(super) fn get_session_json(url: &str) -> std::result::Result<Value, HttpFailure> {
    local_service::http::get_json_observed(url, std::time::Duration::from_secs(5), "opencode.http")
}

pub(super) fn post_json_with_optional_timeout(
    url: &str,
    body: &Value,
    timeout: Option<std::time::Duration>,
) -> std::result::Result<Value, HttpFailure> {
    local_service::http::post_json_observed(url, body, timeout, "opencode.http")
}

/// The engine's framed event ingress, at this facade's width.
///
/// The engine performs the framing and hands each frame to the caller's
/// callback; what a frame means is the adapter package's, so no classification
/// happens here.
pub(super) fn watch_frames(
    url: &str,
    stop: &AtomicBool,
    on_frame: &mut dyn FnMut(&str, &[u8]) -> bool,
) -> std::result::Result<(), local_service::sse::SseFailure> {
    local_service::sse::watch_frames(url, stop, &mut *on_frame)
}

#[cfg(test)]
mod tests;
