//! One OpenCode turn, performed against the engine's port.
//!
//! The shape of a turn is this Agent's contract and is therefore here: bind a
//! native session, admit the turn so force stop can reach it, post the message,
//! watch the event stream for assistant chunks, and settle on the terminal
//! message document. The sockets, the framing, the process and the raw byte
//! record belong to the engine and arrive through [`crate::port::serve`]; the
//! consumer the progressive events reach arrives through
//! [`crate::port::turn_event`].
//!
//! One rule the turn keeps is the whole reason it is not in the client: a turn
//! that hits its deadline stops *before* the next request rather than after a
//! socket timeout, because the remaining budget is this Agent's contract and not
//! the engine's.

use super::continuity::open_serve_session;
use super::{OPENCODE_DRIVER, ProtocolFailure, serve_capabilities};
use crate::parser as serve_parser;
use crate::port::serve::{self, ServeFramingFailure, ServeRequestFailure, ServeTurnAdmission};
use crate::port::turn_event;
use licoup_agent_drivers::acp_driver_runtime::{
    CapabilityProbe, EffectiveSettings, ProtocolConfig, RunResult, timestamp,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;
use uuid::Uuid;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct ServeOutcome {
    output: String,
    transitions: Vec<licoup_agent_adapter_sdk::Transition>,
    session_id: String,
    thread_id: String,
    turn_id: String,
    turn_status: String,
    effective: EffectiveSettings,
    capabilities: CapabilityProbe,
}

pub fn execute(
    executable: &str,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    let _ = (max_stdout, max_stderr);
    let started_at = timestamp();
    let mut config = match ProtocolConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => {
            return failed(failure, started_at);
        }
    };
    // Admission gate: the adapter package asks this host whether it currently
    // admits a new execution, and the answer is the host's own close-admission
    // barrier. A turn started while a maintenance switch holds that barrier would
    // run an Agent the switch may be replacing, so the refusal is reported before
    // the endpoint is attached and before any process starts. A host that
    // installed no port on the package answers fail-closed and this turn never
    // claims it was admitted.
    if !crate::port::execution::admits_execution() {
        return failed(
            ProtocolFailure::new(
                "opencode_execution_admission_closed",
                "OpenCode execution is not admitted while this client is changing installed state.",
                "turn/execute",
            )
            .with_session(Some(session_id)),
            started_at,
        );
    }

    let private_instructions = params
        .get("privateInstructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let attachment = match serve::ensure_attachment(executable) {
        Ok(attachment) => attachment,
        Err(error) => {
            return failed(endpoint_failure(&error), started_at);
        }
    };
    let Some(model) = attachment.catalog.resolve(config.settings.model.as_deref()) else {
        return failed(
            ProtocolFailure::new(
                "opencode_serve_model_unavailable",
                "The selected OpenCode model is not available from the current provider catalog.",
                "serve/model",
            ),
            started_at,
        );
    };
    config.settings.model = Some(model.selector());

    // timeoutMs 0 opts out of any turn deadline (see runtime_adapters/dispatch),
    // so only a non-zero window gets a concrete deadline.
    let deadline = (timeout_ms != 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    match execute_via_serve(
        &attachment.endpoint,
        &config,
        private_instructions.as_deref(),
        deadline,
    ) {
        Ok(outcome) => RunResult {
            ok: true,
            output: outcome.output,
            transitions: outcome.transitions,
            error: None,
            session_id: outcome.session_id,
            thread_id: outcome.thread_id,
            turn_id: outcome.turn_id,
            turn_status: outcome.turn_status,
            effective: outcome.effective,
            capabilities: outcome.capabilities,
            status_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
            started_at,
            runtime_protocol: OPENCODE_DRIVER.runtime_protocol,
            driver_id: OPENCODE_DRIVER.agent_id,
        },
        Err(failure) => failed(failure, started_at),
    }
}

fn failed(failure: ProtocolFailure, started_at: String) -> RunResult {
    let failure = failure.namespaced(OPENCODE_DRIVER);
    let transitions =
        serve_parser::failure_transitions(&failure.code, failure.stage, failure.message);
    RunResult {
        ok: false,
        output: String::new(),
        transitions,
        session_id: failure.session_id.clone().unwrap_or_default(),
        thread_id: failure.thread_id.clone().unwrap_or_default(),
        turn_id: failure.turn_id.clone().unwrap_or_default(),
        turn_status: failure.turn_status.clone().unwrap_or_default(),
        effective: EffectiveSettings::default(),
        capabilities: CapabilityProbe::default(),
        status_code: None,
        stdout_truncated: false,
        stderr_truncated: false,
        started_at,
        runtime_protocol: OPENCODE_DRIVER.runtime_protocol,
        driver_id: OPENCODE_DRIVER.agent_id,
        error: Some(failure),
    }
}

/// The failure one engine attach code means, in this Agent's own vocabulary.
pub(super) fn endpoint_failure(error_code: &str) -> ProtocolFailure {
    match error_code.trim() {
        "opencode_executable_missing" => ProtocolFailure::new(
            "opencode_serve_executable_missing",
            "The OpenCode executable is not available.",
            "serve/start",
        ),
        "opencode_serve_attach_probe_failed" => ProtocolFailure::new(
            "opencode_serve_attach_probe_failed",
            "The OpenCode serve endpoint rejected the attach/session probe.",
            "serve/attach",
        ),
        "opencode_serve_health_failed" => ProtocolFailure::new(
            "opencode_serve_health_failed",
            "The OpenCode serve endpoint did not become healthy.",
            "serve/health",
        ),
        "opencode_serve_port_exhausted" => ProtocolFailure::new(
            "opencode_serve_port_exhausted",
            "No permitted OpenCode serve port is available.",
            "serve/start",
        ),
        _ => ProtocolFailure::new(
            "opencode_serve_start_failed",
            "The OpenCode serve endpoint could not be started.",
            "serve/start",
        ),
    }
}

fn execute_via_serve(
    endpoint: &serve::ServeEndpoint,
    config: &ProtocolConfig,
    private_instructions: Option<&str>,
    deadline: Option<Instant>,
) -> Result<ServeOutcome, ProtocolFailure> {
    let session_id = open_serve_session(endpoint, config, deadline)?;
    let message_body = build_serve_message_body(config, private_instructions);

    let turn_id = Uuid::new_v4().to_string();
    let workspace_attach_url = workspace_request_url(&endpoint.attach_url, &[], &config.cwd)?;
    let first_failure = Arc::new(FirstFailure::default());
    let turn_completed = Arc::new(AtomicBool::new(false));
    let control_failure = Arc::clone(&first_failure);
    let control_completed = Arc::clone(&turn_completed);
    let control_session = session_id.clone();
    // The guard is held for the whole turn: it is what makes force stop able to
    // reach this session's turn, and dropping it is what releases it.
    let _active_turn = match serve::admit_turn(
        &workspace_attach_url,
        &session_id,
        Some(Arc::new(move |failure: ServeRequestFailure| {
            record_preterminal_failure(
                &control_failure,
                &control_completed,
                request_failure(failure, "turn/control", Some(&control_session)),
            );
        })),
    ) {
        ServeTurnAdmission::Admitted(guard) => guard,
        ServeTurnAdmission::AtCapacity => {
            return Err(ProtocolFailure::new(
                "opencode_serve_control_capacity",
                "The OpenCode active-turn control registry is at capacity.",
                "turn/control",
            )
            .with_session(Some(&session_id)));
        }
    };
    turn_event::emit_turn_event("dispatch.turn.bound", &session_id, &turn_id, json!({}));
    let watch_stop = Arc::new(AtomicBool::new(false));
    let watch_flag = Arc::clone(&watch_stop);
    let watch_url = workspace_request_url(&endpoint.attach_url, &["event"], &config.cwd)?;
    let watch_session = session_id.clone();
    let (chunk_sender, chunk_receiver) = mpsc::sync_channel::<String>(64);
    let watch_failure = Arc::clone(&first_failure);
    let watch_completed = Arc::clone(&turn_completed);
    let watch_handle = thread::spawn(move || {
        if let Err(failure) =
            watch_session_events(&watch_url, &watch_session, &watch_flag, &chunk_sender)
        {
            record_preterminal_failure(
                &watch_failure,
                &watch_completed,
                sse_failure(failure, &watch_session),
            );
        }
    });
    let post_url = workspace_request_url(
        &endpoint.attach_url,
        &["session", &session_id, "message"],
        &config.cwd,
    )?;
    let post_failure = Arc::clone(&first_failure);
    let post_completed = Arc::clone(&turn_completed);
    let post_handle = thread::spawn(move || {
        let response = wait_post_json(&post_url, &message_body, deadline);
        if let Err(failure) = &response {
            post_failure.record(failure.clone());
        } else {
            post_completed.store(true, Ordering::Release);
        }
        response
    });
    while !post_handle.is_finished() {
        match chunk_receiver.recv_timeout(PROCESS_POLL_INTERVAL) {
            Ok(text) => {
                turn_event::emit_agent_message_chunk(&session_id, &turn_id, &text);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let response = match post_handle.join() {
        Ok(response) => Some(response),
        Err(_) => {
            first_failure.record(
                ProtocolFailure::new(
                    "opencode_serve_cleanup_failed",
                    "The OpenCode message response worker could not be joined.",
                    "serve/cleanup",
                )
                .with_session(Some(&session_id)),
            );
            None
        }
    };
    watch_stop.store(true, Ordering::Relaxed);
    if watch_handle.join().is_err() {
        first_failure.record(
            ProtocolFailure::new(
                "opencode_serve_cleanup_failed",
                "The OpenCode event-stream worker could not be joined.",
                "serve/cleanup",
            )
            .with_session(Some(&session_id)),
        );
    }
    for text in chunk_receiver.try_iter() {
        turn_event::emit_agent_message_chunk(&session_id, &turn_id, &text);
    }
    if let Some(failure) = first_failure.get() {
        return Err(failure.with_session(Some(&session_id)));
    }
    let response = response.ok_or_else(|| {
        ProtocolFailure::new(
            "opencode_serve_cleanup_failed",
            "The OpenCode message response worker did not return an outcome.",
            "serve/cleanup",
        )
        .with_session(Some(&session_id))
    })??;
    let parsed = serve_parser::message(&response).ok_or_else(|| {
        ProtocolFailure::new(
            "acp_final_message_missing",
            "The ACP agent completed the turn without a final agent message.",
            "session/prompt",
        )
        .with_session(Some(&session_id))
    })?;
    let output = parsed.output;
    turn_event::emit_agent_message_completed(&session_id, &turn_id, &output);
    Ok(ServeOutcome {
        output: output.clone(),
        transitions: parsed.transitions,
        session_id: session_id.clone(),
        thread_id: session_id,
        turn_id,
        turn_status: "end_turn".to_string(),
        effective: EffectiveSettings {
            cwd: Some(config.cwd.clone()),
            model: config.settings.model.clone(),
            reasoning_effort: config.settings.reasoning_effort.clone(),
            mode: config.settings.mode.clone(),
            runtime_agent: config.settings.runtime_agent.clone(),
            allow_all: config.settings.allow_all,
            sandbox: None,
            approval_policy: None,
        },
        capabilities: serve_capabilities(),
    })
}

#[derive(Default)]
pub(super) struct FirstFailure(Mutex<Option<ProtocolFailure>>);

impl FirstFailure {
    pub(super) fn record(&self, failure: ProtocolFailure) {
        if let Ok(mut slot) = self.0.lock()
            && slot.is_none()
        {
            *slot = Some(failure);
        }
    }

    pub(super) fn get(&self) -> Option<ProtocolFailure> {
        self.0.lock().ok().and_then(|slot| slot.clone())
    }
}

pub(super) fn record_preterminal_failure(
    first_failure: &FirstFailure,
    turn_completed: &AtomicBool,
    failure: ProtocolFailure,
) {
    if !turn_completed.load(Ordering::Acquire) {
        first_failure.record(failure);
    }
}

/// Why an event stream ended without the turn being stopped.
///
/// The variants are this package's own closed vocabulary over the engine's
/// framing answer: a stream that closed, a frame this Agent's parser rejected,
/// and the engine's own framing limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeStreamFailure {
    /// The stream ended before the turn completed.
    Closed,
    /// A stream document did not decode.
    Decode(serve_parser::ServeEventFailure),
    /// The engine's framing refused a frame.
    Framing(ServeFramingFailure),
}

/// Watch the endpoint's event stream, forwarding assistant chunks.
///
/// The stream is the engine's; what a frame means is this parser's. A decode
/// failure is recorded once and ends the watch, because the stream can no longer
/// be trusted to report the turn's progress.
pub fn watch_session_events(
    url: &str,
    session_id: &str,
    stop: &AtomicBool,
    chunks: &SyncSender<String>,
) -> Result<(), ServeStreamFailure> {
    let mut parser = serve_parser::ServeEventParser::new(session_id);
    let mut decode_failure = None;
    let result = serve::watch_frames(url, stop, &mut |data, frame| {
        // The whole framed text is what the diagnostic record keeps; the frame's
        // own payload is what decides it belongs to this session.
        serve::observe_bytes(
            "opencode.sse",
            serve::ServeByteDirection::Received,
            Some(session_id),
            data,
            frame,
        );
        match parser.observe(data) {
            Ok(Some(text)) => {
                let _ = chunks.try_send(text);
                true
            }
            Ok(None) => true,
            Err(failure) => {
                decode_failure = Some(failure);
                false
            }
        }
    });
    if let Some(failure) = decode_failure {
        return Err(ServeStreamFailure::Decode(failure));
    }
    match result {
        Ok(()) if !stop.load(Ordering::Relaxed) => Err(ServeStreamFailure::Closed),
        Ok(()) => Ok(()),
        Err(failure) => Err(ServeStreamFailure::Framing(failure)),
    }
}

pub(super) fn sse_failure(failure: ServeStreamFailure, session_id: &str) -> ProtocolFailure {
    use ServeStreamFailure as StreamFailure;
    let code = match failure {
        StreamFailure::Closed => "opencode_serve_sse_closed",
        StreamFailure::Decode(_) => "opencode_serve_sse_invalid_json",
        StreamFailure::Framing(ServeFramingFailure::Busy) => "opencode_serve_sse_busy",
        StreamFailure::Framing(ServeFramingFailure::EventLimit) => "opencode_serve_sse_event_limit",
        StreamFailure::Framing(ServeFramingFailure::FrameTooLarge) => {
            "opencode_serve_sse_frame_too_large"
        }
        StreamFailure::Framing(ServeFramingFailure::HeadersTooLarge) => {
            "opencode_serve_sse_headers_too_large"
        }
        StreamFailure::Framing(ServeFramingFailure::InvalidUtf8) => {
            "opencode_serve_sse_invalid_utf8"
        }
        StreamFailure::Framing(ServeFramingFailure::InvalidUrl) => "opencode_serve_sse_url_invalid",
        StreamFailure::Framing(ServeFramingFailure::LineTooLarge) => {
            "opencode_serve_sse_line_too_large"
        }
        StreamFailure::Framing(ServeFramingFailure::Request) => "opencode_serve_sse_request_failed",
        StreamFailure::Framing(ServeFramingFailure::Unavailable) => {
            "opencode_serve_sse_unavailable"
        }
    };
    ProtocolFailure::new(
        code,
        "The OpenCode event stream failed before the turn completed.",
        "serve/sse",
    )
    .with_session(Some(session_id))
}

pub(super) fn build_serve_message_body(
    config: &ProtocolConfig,
    private_instructions: Option<&str>,
) -> Value {
    let mut message_body = json!({
        "parts": [{"type": "text", "text": config.prompt}]
    });
    if let Some(model) = config.settings.model.as_deref() {
        // OpenCode accepts provider/model; when only a bare id is present keep text.
        if let Some((provider, model_id)) = model.split_once('/') {
            message_body["model"] = json!({
                "providerID": provider,
                "modelID": model_id
            });
        }
    }
    if let Some(agent) = config.settings.runtime_agent.as_deref() {
        message_body["agent"] = json!(agent);
    }
    if let Some(instructions) = private_instructions {
        // OpenCode's native message contract carries private system guidance
        // separately from user-authored parts.
        message_body["system"] = json!(instructions);
    }
    message_body
}

pub(super) fn wait_post_json(
    url: &str,
    body: &Value,
    deadline: Option<Instant>,
) -> Result<Value, ProtocolFailure> {
    let timeout = remaining_turn_timeout(deadline)?;
    serve::post_json(url, body, timeout).map_err(|failure| {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            turn_timeout_failure()
        } else {
            request_failure(failure, "session/prompt", None)
        }
    })
}

pub(super) fn workspace_request_url(
    attach_url: &str,
    segments: &[&str],
    directory: &str,
) -> Result<String, ProtocolFailure> {
    let mut url = Url::parse(attach_url).map_err(|_| {
        ProtocolFailure::new(
            "opencode_serve_url_invalid",
            "The OpenCode serve endpoint URL is invalid.",
            "serve/url",
        )
    })?;
    {
        let mut path = url.path_segments_mut().map_err(|_| {
            ProtocolFailure::new(
                "opencode_serve_url_invalid",
                "The OpenCode serve endpoint URL is invalid.",
                "serve/url",
            )
        })?;
        path.pop_if_empty();
        path.extend(segments.iter().copied());
    }
    url.query_pairs_mut().append_pair("directory", directory);
    Ok(url.into())
}

pub(super) fn request_failure(
    failure: ServeRequestFailure,
    stage: &'static str,
    session_id: Option<&str>,
) -> ProtocolFailure {
    use ServeRequestFailure as RequestFailure;

    let (code, message) = match failure {
        RequestFailure::BodyTooLarge => (
            "opencode_serve_request_too_large",
            "The OpenCode serve request exceeds the supported size.",
        ),
        RequestFailure::Busy => (
            "opencode_serve_client_busy",
            "The OpenCode serve client is busy with other requests.",
        ),
        RequestFailure::HeadersTooLarge => (
            "opencode_serve_response_headers_too_large",
            "The OpenCode serve response headers exceed the supported size.",
        ),
        RequestFailure::InvalidJson => (
            "opencode_serve_invalid_json",
            "The OpenCode serve endpoint returned invalid JSON.",
        ),
        RequestFailure::InvalidUrl => (
            "opencode_serve_url_invalid",
            "The OpenCode serve endpoint URL is invalid.",
        ),
        RequestFailure::NotFound => (
            "opencode_serve_not_found",
            "The requested OpenCode serve resource does not exist.",
        ),
        RequestFailure::Serialize => (
            "opencode_serve_request_invalid",
            "The OpenCode serve request could not be encoded.",
        ),
        RequestFailure::Status(401 | 403) => (
            "opencode_serve_authentication_required",
            "The OpenCode serve endpoint requires authentication.",
        ),
        RequestFailure::Status(400 | 422) => (
            "opencode_serve_request_rejected",
            "The OpenCode serve endpoint rejected the request.",
        ),
        RequestFailure::Status(409) => (
            "opencode_serve_session_busy",
            "The OpenCode session is already processing another request.",
        ),
        RequestFailure::Status(429) => (
            "opencode_serve_rate_limited",
            "The OpenCode provider rate-limited the request.",
        ),
        RequestFailure::Status(500..=599) | RequestFailure::Unavailable => phase_failure(stage),
        RequestFailure::Request | RequestFailure::Status(_) => (
            "opencode_serve_request_failed",
            "The OpenCode serve request could not be completed.",
        ),
    };
    let mut failure = ProtocolFailure::new(code, message, stage).with_session(session_id);
    if code == "opencode_serve_authentication_required" {
        failure.user_interaction_required = true;
        failure.request_method = Some("authenticate".to_string());
    }
    failure
}

fn phase_failure(stage: &str) -> (&'static str, &'static str) {
    match stage {
        "serve/health" => (
            "opencode_serve_health_failed",
            "The OpenCode health endpoint failed before becoming ready.",
        ),
        "session/prompt" => (
            "opencode_serve_message_failed",
            "The OpenCode message endpoint failed while running the turn.",
        ),
        "turn/control" => (
            "opencode_serve_control_failed",
            "The OpenCode control endpoint failed while controlling the turn.",
        ),
        "session/new" | "session/load" | "serve/session" => (
            "opencode_serve_session_failed",
            "The OpenCode session endpoint failed while opening the conversation.",
        ),
        _ => (
            "opencode_serve_request_failed",
            "The OpenCode serve request could not be completed.",
        ),
    }
}

pub(super) fn remaining_turn_timeout(
    deadline: Option<Instant>,
) -> Result<Option<Duration>, ProtocolFailure> {
    match deadline {
        None => Ok(None),
        Some(deadline) => deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .map(Some)
            .ok_or_else(turn_timeout_failure),
    }
}

pub(super) fn turn_timeout_failure() -> ProtocolFailure {
    ProtocolFailure::new(
        "opencode_serve_deadline_exceeded",
        "The OpenCode turn deadline elapsed before completion.",
        "turn/deadline",
    )
}
