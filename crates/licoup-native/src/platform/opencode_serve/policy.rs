//! The serve specification this host runs for the OpenCode endpoint.
//!
//! The endpoint's own facts — its identity, the port it prefers, the paths it
//! exposes, the state record it writes and the failure codes it reports — are
//! the adapter package's (`licoup_agent_opencode::policy`). What is left here is
//! the engine half the package cannot state: how this Agent's program is
//! launched, and which of the package's readers turns its documents into the
//! engine's readiness record. Both are read by `local_service::serve`, so this is
//! the one specification the engine runs and force-stop control reads.

use std::process::Command;

use super::super::local_service::{ServeErrorCodes, ServeSpec};
use licoup_agent_opencode::policy as vendor;

pub(super) const SPEC: ServeSpec = ServeSpec {
    identity: vendor::SPEC.identity,
    default_port: vendor::SPEC.default_port,
    port_range_span: vendor::SPEC.port_range_span,
    default_host: vendor::SPEC.default_host,
    health_path: vendor::SPEC.health_path,
    session_probe_path: vendor::SPEC.session_probe_path,
    config_path: vendor::SPEC.config_path,
    provider_path: vendor::SPEC.provider_path,
    state_dir: vendor::SPEC.state_dir,
    state_schema_version: vendor::SPEC.state_schema_version,
    default_health_timeout_ms: vendor::SPEC.default_health_timeout_ms,
    reserved_ports: vendor::SPEC.reserved_ports,
    executable_environment: vendor::SPEC.executable_environment,
    default_executable: vendor::SPEC.default_executable,
    configure_command,
    // The readiness reader is the package's own, reached through the name this
    // composition gives that parser: the engine's readiness record and the
    // package's are one shared shape, so the package classifies its documents
    // and the engine keeps the record.
    parse_readiness: super::super::native_agent_parser::adapters::opencode::readiness,
    // The failure vocabulary is the package's closed set; the engine's is its own
    // shape, so the crossing is a field copy rather than an assignment that would
    // silently carry the package's type where the engine expects its own.
    errors: ServeErrorCodes {
        executable_missing: vendor::SPEC.errors.executable_missing,
        port_exhausted: vendor::SPEC.errors.port_exhausted,
        start_failed: vendor::SPEC.errors.start_failed,
        health_failed: vendor::SPEC.errors.health_failed,
        attach_probe_failed: vendor::SPEC.errors.attach_probe_failed,
        not_found: vendor::SPEC.errors.not_found,
        request_failed: vendor::SPEC.errors.request_failed,
        invalid_json: vendor::SPEC.errors.invalid_json,
        invalid_state: vendor::SPEC.errors.invalid_state,
        stop_failed: vendor::SPEC.errors.stop_failed,
    },
};

/// How the engine launches this Agent's endpoint.
fn configure_command(command: &mut Command, host: &str, port: u16) {
    command.args(["serve", "--hostname", host, "--port", &port.to_string()]);
}
