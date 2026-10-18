//! OpenClaw Gateway lifecycle facade owned by the native client.

mod command;
mod config;
mod health;
mod lifecycle;
mod model;
mod policy;

use anyhow::Result;
use serde_json::Value;

pub use model::GatewayEndpoint;
pub const VENDOR_DEFAULT_PORT: u16 = policy::VENDOR_DEFAULT_PORT;
pub const DEFAULT_PORT: u16 = policy::DEFAULT_PORT;

pub fn ensure(params: &Value) -> Result<Value> {
    lifecycle::ensure(params)
}

pub fn start(params: &Value) -> Result<Value> {
    lifecycle::start(params)
}

pub fn restart(params: &Value) -> Result<Value> {
    lifecycle::restart(params)
}

pub fn stop(params: &Value) -> Result<Value> {
    lifecycle::stop(params)
}

pub fn status(params: &Value) -> Result<Value> {
    lifecycle::status(params)
}

pub fn ensure_attach_endpoint(executable: &str) -> Result<GatewayEndpoint> {
    lifecycle::ensure_attach_endpoint(executable)
}

pub fn select_available_port(preferred: u16) -> Result<u16> {
    lifecycle::select_available_port(preferred)
}

pub fn select_available_port_with<F>(preferred: u16, is_bindable: F) -> Result<u16>
where
    F: Fn(u16) -> bool,
{
    lifecycle::select_available_port_with(preferred, is_bindable)
}

pub fn is_reserved_conflict_port(port: u16) -> bool {
    lifecycle::is_reserved_conflict_port(port)
}

#[cfg(test)]
mod tests;
