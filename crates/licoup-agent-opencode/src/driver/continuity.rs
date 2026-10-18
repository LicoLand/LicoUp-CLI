//! Binding the native conversation one turn runs against.
//!
//! A resume loads exactly the requested native session; a fresh turn creates
//! one. Both are this Agent's continuity contract, and both go through the
//! engine's document reader rather than a socket of this package's own.

use super::serve_transport::{remaining_turn_timeout, request_failure, workspace_request_url};
use super::{ProtocolConfig, ProtocolFailure};
use crate::parser as serve_parser;
use crate::port::serve::{self, ServeEndpoint};
use serde_json::{Value, json};
use std::time::Instant;

pub(super) fn open_serve_session(
    endpoint: &ServeEndpoint,
    config: &ProtocolConfig,
    deadline: Option<Instant>,
) -> Result<String, ProtocolFailure> {
    if config.is_resume() {
        let url = workspace_request_url(
            &endpoint.attach_url,
            &["session", &config.requested_session_id],
            &config.cwd,
        )?;
        return match serve::get_json(&url, true) {
            Ok(payload) => match serve_parser::session_id(&payload) {
                Some(id) if id == config.requested_session_id => Ok(id.to_string()),
                // A returned different identity is an exact-lookup mismatch:
                // the HTTP session for the requested id must not be replaced
                // by another native conversation.
                Some(_) => Err(load_identity_mismatch(&config.requested_session_id)),
                None => Err(load_session_not_found(&config.requested_session_id)),
            },
            Err(failure) => Err(request_failure(
                failure,
                "session/load",
                Some(&config.requested_session_id),
            )),
        };
    }

    let timeout = remaining_turn_timeout(deadline)?;
    let url = workspace_request_url(&endpoint.attach_url, &["session"], &config.cwd)?;
    let body = build_session_create_body();
    let created = serve::post_json(&url, &body, timeout).map_err(|failure| {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            super::serve_transport::turn_timeout_failure()
        } else {
            request_failure(failure, "session/new", None)
        }
    })?;
    serve_parser::session_id(&created)
        .map(str::to_string)
        .ok_or_else(|| {
            ProtocolFailure::new(
                "acp_session_id_missing",
                "The ACP agent did not return a native conversation identifier.",
                "session/new",
            )
        })
}

pub(super) fn build_session_create_body() -> Value {
    json!({})
}

fn load_session_not_found(requested_session_id: &str) -> ProtocolFailure {
    ProtocolFailure::new(
        "acp_native_session_not_found",
        "The requested native conversation does not exist in the ACP agent.",
        "session/load",
    )
    .with_session(Some(requested_session_id))
}

fn load_identity_mismatch(requested_session_id: &str) -> ProtocolFailure {
    ProtocolFailure::new(
        "acp_session_id_mismatch",
        "The ACP agent returned a different conversation than the one requested.",
        "session/load",
    )
    .with_session(Some(requested_session_id))
}
