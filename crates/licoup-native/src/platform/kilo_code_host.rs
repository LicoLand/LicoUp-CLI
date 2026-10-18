//! The client's answer for the Kilo Code adapter package's ports.
//!
//! The package owns what one Kilo turn *is*; this module owns where its effects
//! go, because the client owns the consumer and the shared local-service engine.
//! Both halves meet exactly here:
//!
//! - [`serve_port`] is the package's [`ServePort`] answered from
//!   `licoup-agent-drivers`' serve engine — the same engine that starts and
//!   supervises the endpoint, reads its documents over HTTP, frames its SSE
//!   stream, records raw bytes and registers an active turn for force stop.
//! - [`turn_event_port`] is the package's turn-event emission answered from this
//!   host's own emitters, so a Kilo event and a Cursor event reach the same
//!   reader through the same path.
//! - [`host_ports`] states both at once, which is what composition installs.
//!
//! Nothing here names a vendor field: the engine is protocol-agnostic and the
//! policy it runs on comes from the package ([`kilo_serve_spec`]).
//!
//! The package performs the turn and holds the registration that keeps it
//! reachable; what it cannot own is the engine underneath, so the attachment,
//! the reads, the framing, the byte record and the active-turn registry are all
//! answered here, and force stop reaches a running Kilo turn through [`cancel`].

use std::sync::atomic::AtomicBool;

use licoup_agent_kilo::host::HostPorts;
use licoup_agent_kilo::policy;
use licoup_agent_kilo::port::serve::{
    ServeAttachment, ServeByteDirection, ServeEndpoint, ServeFramingFailure, ServePort,
    ServeTurnGuard,
};
use licoup_agent_kilo::port::turn_event::TurnEventPort;
use serde_json::Value;

use super::local_service;
use super::raw_execution::{RawExecutionDirection, RawExecutionObserver};

/// The durable serve owner descriptor force stop reads.
///
/// Control reads the state and pid records this specification's owner writes, so
/// force stop finds a Kilo endpoint through the same record the engine created
/// rather than through a second registry. It is the same specification
/// [`kilo_serve_spec`] answers with: the engine's contract and force stop's
/// descriptor are one contract, so neither can drift from the other or from the
/// package's own policy.
pub(crate) const CONTROL_SPEC: local_service::ServeSpec = local_service::ServeSpec {
    identity: policy::SPEC.identity,
    default_port: policy::SPEC.default_port,
    port_range_span: policy::SPEC.port_range_span,
    default_host: policy::SPEC.default_host,
    health_path: policy::SPEC.health_path,
    session_probe_path: policy::SPEC.session_probe_path,
    config_path: policy::SPEC.config_path,
    provider_path: policy::SPEC.provider_path,
    state_dir: policy::SPEC.state_dir,
    state_schema_version: policy::SPEC.state_schema_version,
    default_health_timeout_ms: policy::SPEC.default_health_timeout_ms,
    reserved_ports: policy::SPEC.reserved_ports,
    executable_environment: policy::SPEC.executable_environment,
    default_executable: policy::SPEC.default_executable,
    configure_command,
    parse_readiness,
    // The engine's failure vocabulary is its own shape, so the package's closed
    // set crosses as a field copy rather than by assignment: an assignment would
    // silently carry the package's type where the engine expects its own.
    errors: local_service::ServeErrorCodes {
        executable_missing: policy::SPEC.errors.executable_missing,
        port_exhausted: policy::SPEC.errors.port_exhausted,
        start_failed: policy::SPEC.errors.start_failed,
        health_failed: policy::SPEC.errors.health_failed,
        attach_probe_failed: policy::SPEC.errors.attach_probe_failed,
        not_found: policy::SPEC.errors.not_found,
        request_failed: policy::SPEC.errors.request_failed,
        invalid_json: policy::SPEC.errors.invalid_json,
        invalid_state: policy::SPEC.errors.invalid_state,
        stop_failed: policy::SPEC.errors.stop_failed,
    },
};

/// The serve engine specification this Agent's policy describes.
///
/// The engine supplies the two things the package does not own: the environment
/// it launches the endpoint in and the readiness reader that turns this Agent's
/// documents into the shared readiness record. Everything else — the identity,
/// the port, the paths, the reserved ports, the executable names and the failure
/// codes — is read from the package's own policy, so the client carries no
/// second copy of Kilo Code's endpoint contract. This is the one specification
/// the engine runs and force stop reads.
pub(crate) fn kilo_serve_spec() -> local_service::ServeSpec {
    CONTROL_SPEC
}

/// How the engine launches this Agent's endpoint.
fn configure_command(command: &mut std::process::Command, host: &str, port: u16) {
    command.args(["serve", "--hostname", host, "--port", &port.to_string()]);
}

/// How this Agent's documents decide readiness.
///
/// The reading is the package's; only the record type changes, because the
/// engine's readiness record and the SDK's readiness vocabulary are both shared
/// shapes.
fn parse_readiness(
    health: &Value,
    sessions: &Value,
    config: &Value,
    providers: &Value,
) -> Option<local_service::ServeReadiness> {
    licoup_agent_kilo::parser::readiness(health, sessions, config, providers).map(|ready| {
        local_service::ServeReadiness {
            version: ready.version,
            catalog: local_service::ServeModelCatalog {
                current: serve_model(ready.catalog.current),
                models: ready.catalog.models.into_iter().map(serve_model).collect(),
            },
            health: ready.health,
        }
    })
}

/// This Agent's readiness vocabulary is the SDK's, which the engine's record and
/// this client both already carry; the crossing is a field copy.
fn serve_model(model: licoup_agent_adapter_sdk::serve::ServeModel) -> local_service::ServeModel {
    local_service::ServeModel {
        provider_id: model.provider_id,
        model_id: model.model_id,
    }
}

fn ensure_attachment(executable: &str) -> Result<ServeAttachment, String> {
    let attachment = local_service::serve::ensure_attachment(kilo_serve_spec(), executable)
        .map_err(|error| error.to_string())?;
    Ok(ServeAttachment {
        endpoint: ServeEndpoint {
            host: attachment.endpoint.host,
            port: attachment.endpoint.port,
            attach_url: attachment.endpoint.attach_url,
        },
        catalog: licoup_agent_adapter_sdk::serve::ServeModelCatalog {
            current: package_model(attachment.catalog.current),
            models: attachment
                .catalog
                .models
                .into_iter()
                .map(package_model)
                .collect(),
        },
    })
}

fn package_model(model: local_service::ServeModel) -> licoup_agent_adapter_sdk::serve::ServeModel {
    licoup_agent_adapter_sdk::serve::ServeModel {
        provider_id: model.provider_id,
        model_id: model.model_id,
    }
}

fn get_json(url: &str, observed: bool) -> Result<Value, String> {
    let source = observed.then_some("kilo-code.http");
    local_service::serve::get_json(kilo_serve_spec(), url, source)
        .map_err(|error| error.to_string())
}

fn post_json(url: &str, body: &Value) -> Result<Value, String> {
    local_service::serve::post_json(kilo_serve_spec(), url, body, "kilo-code.http")
        .map_err(|error| error.to_string())
}

fn watch_frames(
    url: &str,
    stop: &AtomicBool,
    on_frame: &mut dyn FnMut(&str, &str) -> bool,
) -> Result<(), ServeFramingFailure> {
    // The engine frames the stream; the package decides what a frame means. The
    // observer scope is entered here because the diagnostic record is the
    // client's, and a frame the package never looks at is still a frame the
    // record captured.
    let raw_observer = RawExecutionObserver::current();
    let _scope = licoup_foundation::platform::raw_execution::RawExecutionScope::enter(raw_observer);
    local_service::sse::watch_frames(url, stop, |data, frame| {
        let frame = String::from_utf8_lossy(frame);
        on_frame(data, &frame)
    })
    .map_err(framing_failure)
}

fn framing_failure(failure: local_service::sse::SseFailure) -> ServeFramingFailure {
    use local_service::sse::SseFailure;
    match failure {
        SseFailure::Busy => ServeFramingFailure::Busy,
        SseFailure::EventLimit => ServeFramingFailure::EventLimit,
        SseFailure::FrameTooLarge => ServeFramingFailure::FrameTooLarge,
        SseFailure::HeadersTooLarge => ServeFramingFailure::HeadersTooLarge,
        SseFailure::InvalidUtf8 => ServeFramingFailure::InvalidUtf8,
        SseFailure::InvalidUrl => ServeFramingFailure::InvalidUrl,
        SseFailure::LineTooLarge => ServeFramingFailure::LineTooLarge,
        SseFailure::Request => ServeFramingFailure::Request,
        SseFailure::Unavailable => ServeFramingFailure::Unavailable,
    }
}

fn observe_bytes(
    source: &str,
    direction: ServeByteDirection,
    session_id: Option<&str>,
    bytes: &str,
) {
    let Some(observer) = RawExecutionObserver::current() else {
        return;
    };
    if let Some(session_id) = session_id
        && !local_service::sse::frame_belongs_to_session(bytes, session_id)
    {
        return;
    }
    let direction = match direction {
        ServeByteDirection::Received => RawExecutionDirection::Received,
        ServeByteDirection::Sent => RawExecutionDirection::Sent,
    };
    observer.record_bytes(source, direction, bytes.as_bytes());
}

fn register_turn(attach_url: &str, session_id: &str) -> Option<ServeTurnGuard> {
    // The guard is handed to the caller, which holds it for the whole turn:
    // force stop reaches an active turn through this registration, and a turn
    // that gave the guard up early would be unreachable while it still runs.
    // The identity is the package's own, because the same identity is what a
    // resumed turn registers its requested session under.
    local_service::turn_control::register(
        licoup_agent_kilo::driver::DRIVER_ID,
        attach_url,
        session_id,
        None,
    )
    .ok()
    .map(ServeTurnGuard::hold)
}

/// Reach the endpoint's active turn for force stop.
///
/// The active-turn registry belongs to the serve engine, not to the package: it
/// is the same registry every serve-family Agent's stop uses, and the identity a
/// Kilo turn registers under is the one its own declaration names.
pub(crate) fn cancel(session_id: &str) -> local_service::turn_control::ControlDisposition {
    local_service::turn_control::cancel(licoup_agent_kilo::driver::DRIVER_ID, session_id)
}

/// This host's answer for the package's serve port.
pub(crate) fn serve_port() -> ServePort {
    ServePort {
        ensure_attachment,
        get_json,
        post_json,
        watch_frames,
        observe_bytes,
        register_turn,
    }
}

/// This host's answer for the package's turn-event port.
pub(crate) fn turn_event_port() -> TurnEventPort {
    TurnEventPort {
        emit_turn_event: super::turn_event_emit::emit_turn_event,
        emit_agent_message_chunk: super::turn_event_emit::emit_agent_message_chunk,
        emit_agent_message_completed: super::turn_event_emit::emit_agent_message_completed,
    }
}

/// Both ports, as composition installs them.
pub(crate) fn host_ports() -> HostPorts {
    HostPorts {
        turn_event: turn_event_port(),
        serve: serve_port(),
    }
}

#[cfg(test)]
mod tests;
