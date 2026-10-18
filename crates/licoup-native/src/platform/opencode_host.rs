//! The client's answer for the OpenCode adapter package's ports.
//!
//! The package owns what one OpenCode turn *is*; this module owns where its
//! effects go, because the client owns the consumer and the shared local-service
//! engine. Both halves meet exactly here:
//!
//! - [`serve_port`] is the package's [`ServePort`](licoup_agent_opencode::port::serve::ServePort)
//!   answered from this client's serve engine — the same engine that starts and
//!   supervises the endpoint, reads its documents over HTTP, frames its SSE
//!   stream, records raw bytes and admits an active turn for force stop. Every
//!   engine call is the one `opencode_serve` already made, so the facade stays
//!   the single engine entry and this module only states the package's port in
//!   the engine's vocabulary.
//! - [`turn_event_port`] is the package's turn-event emission answered from this
//!   host's own emitters, so an OpenCode event and a Kilo Code event reach the
//!   same reader through the same path.
//! - [`host_ports`] states both at once, which is what composition installs.
//!
//! Nothing here names a vendor field: the engine is protocol-agnostic and the
//! policy it runs on comes from the package ([`super::opencode_serve::policy`]
//! reading `licoup_agent_opencode::policy`).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use licoup_agent_opencode::driver::OPENCODE_DRIVER;
use licoup_agent_opencode::host::HostPorts;
use licoup_agent_opencode::port::serve::{
    ServeAttachment, ServeAttachmentLease, ServeByteDirection, ServeControlFailureObserver,
    ServeEndpoint, ServeFramingFailure, ServePort, ServeRequestFailure, ServeTurnAdmission,
    ServeTurnGuard,
};
use licoup_agent_opencode::port::turn_event::TurnEventPort;
use serde_json::Value;

use super::local_service::sse::SseFailure;
use super::local_service::turn_control;
use super::local_service::{self, http::HttpFailure};
use super::raw_execution::{RawExecutionDirection, RawExecutionObserver, RawExecutionScope};

fn ensure_attachment(executable: &str) -> Result<ServeAttachment, String> {
    let attachment =
        super::opencode_serve::ensure_attachment(executable).map_err(|error| error.to_string())?;
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
        // The engine's own lease travels with the attachment: it is what keeps
        // force stop able to find the endpoint for the whole turn, and the
        // package holds it for exactly as long as it holds the attachment.
        lease: ServeAttachmentLease::held(attachment._lease),
    })
}

fn package_model(model: local_service::ServeModel) -> licoup_agent_adapter_sdk::serve::ServeModel {
    licoup_agent_adapter_sdk::serve::ServeModel {
        provider_id: model.provider_id,
        model_id: model.model_id,
    }
}

/// The engine's HTTP failure, named in the package's own closed vocabulary.
///
/// The variants are the engine's; the codes the Agent reports for them are the
/// package's. The crossing is a field copy so neither side invents the other's
/// answer and no reader has to parse a message.
fn request_failure(failure: HttpFailure) -> ServeRequestFailure {
    match failure {
        HttpFailure::BodyTooLarge => ServeRequestFailure::BodyTooLarge,
        HttpFailure::Busy => ServeRequestFailure::Busy,
        HttpFailure::HeadersTooLarge => ServeRequestFailure::HeadersTooLarge,
        HttpFailure::InvalidJson => ServeRequestFailure::InvalidJson,
        HttpFailure::InvalidUrl => ServeRequestFailure::InvalidUrl,
        HttpFailure::NotFound => ServeRequestFailure::NotFound,
        HttpFailure::Request => ServeRequestFailure::Request,
        HttpFailure::Serialize => ServeRequestFailure::Serialize,
        HttpFailure::Status(status) => ServeRequestFailure::Status(status),
        HttpFailure::Unavailable => ServeRequestFailure::Unavailable,
    }
}

fn get_json(url: &str, observed: bool) -> Result<Value, ServeRequestFailure> {
    if observed {
        super::opencode_serve::get_session_json(url)
    } else {
        super::opencode_serve::get_json(url)
    }
    .map_err(request_failure)
}

fn post_json(
    url: &str,
    body: &Value,
    timeout: Option<Duration>,
) -> Result<Value, ServeRequestFailure> {
    super::opencode_serve::post_json_with_optional_timeout(url, body, timeout)
        .map_err(request_failure)
}

fn framing_failure(failure: SseFailure) -> ServeFramingFailure {
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

fn watch_frames(
    url: &str,
    stop: &AtomicBool,
    on_frame: &mut dyn FnMut(&str, &str) -> bool,
) -> Result<(), ServeFramingFailure> {
    // The engine frames the stream and the diagnostic record files what the
    // package never looks at, so the observer scope is entered here: the record
    // is the client's, and the caller's thread is the one the engine reads it
    // from.
    let raw_observer = RawExecutionObserver::current();
    let _scope = RawExecutionScope::enter(raw_observer);
    super::opencode_serve::watch_frames(url, stop, &mut |data, frame| {
        let frame = String::from_utf8_lossy(frame);
        on_frame(data, &frame)
    })
    .map_err(framing_failure)
}

fn observe_bytes(
    source: &str,
    direction: ServeByteDirection,
    session_id: Option<&str>,
    payload: &str,
    frame: &str,
) {
    let Some(observer) = RawExecutionObserver::current() else {
        return;
    };
    if let Some(session_id) = session_id
        && !local_service::sse::frame_belongs_to_session(payload, session_id)
    {
        return;
    }
    let direction = match direction {
        ServeByteDirection::Received => RawExecutionDirection::Received,
        ServeByteDirection::Sent => RawExecutionDirection::Sent,
    };
    observer.record_bytes(source, direction, frame.as_bytes());
}

fn admit_turn(
    attach_url: &str,
    session_id: &str,
    on_failure: Option<ServeControlFailureObserver>,
) -> ServeTurnAdmission {
    let observer: Option<turn_control::ControlFailureObserver> = on_failure.map(|on_failure| {
        let observer: std::sync::Arc<dyn Fn(HttpFailure) + Send + Sync> =
            std::sync::Arc::new(move |failure| on_failure(request_failure(failure)));
        observer
    });
    match turn_control::register(OPENCODE_DRIVER.agent_id, attach_url, session_id, observer) {
        // The guard travels with the admission: the package holds it for the
        // whole turn, and dropping it is what releases the registration.
        Ok(guard) => ServeTurnAdmission::Admitted(ServeTurnGuard::held(guard)),
        Err(()) => ServeTurnAdmission::AtCapacity,
    }
}

/// This client's answer for the package's serve port.
pub(crate) fn serve_port() -> ServePort {
    ServePort {
        ensure_attachment,
        get_json,
        post_json,
        watch_frames,
        observe_bytes,
        admit_turn,
    }
}

/// This client's answer for the package's turn-event port.
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
