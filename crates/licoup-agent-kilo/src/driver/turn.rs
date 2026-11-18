//! One Kilo turn, performed against the engine's port.
//!
//! The shape of a turn is this Agent's contract and is therefore here: bind a
//! native session (load the requested one exactly, or create one), post the
//! message, watch the event stream for assistant chunks, and settle on the
//! terminal message document. The sockets, the framing, the active-turn registry
//! and the raw byte record belong to the engine and arrive through
//! [`crate::port::serve`].
//!
//! Two rules the turn keeps are the whole reason it is not in the client:
//!
//! - **Exact identity.** A resume never replaces the conversation it asked for.
//!   A service that answers with a different session identity is a mismatch and
//!   the turn stops before any message is posted.
//! - **One terminal.** The message document reports the protocol finish, so a
//!   plain stream end after a successful response is observer loss and cannot
//!   relabel a finished turn as failed.
//!
//! [`execute`] is the whole of one turn as the host composes it: it attaches to
//! the endpoint through the installed engine, resolves the model the endpoint
//! reports, registers the turn so force stop can reach it, performs
//! [`execute_via_serve`] against the installed ports, and states the outcome in
//! the host's own result vocabulary. Nothing above this module decides what a
//! Kilo turn is.

use super::config::{ServeTurnConfig, timestamp};
use super::probe;
use super::projection::{ProtocolOutcome, project_turn};
use super::{DRIVER, DRIVER_ID, ProtocolFailure, RUNTIME_PROTOCOL};
use crate::host;
use crate::parser;
use crate::policy;
use crate::port::serve::{self, ServeFramingFailure};
use crate::port::turn_event;
use licoup_agent_drivers::acp_driver_runtime::{
    CapabilityProbe as DriverCapabilityProbe, EffectiveSettings as DriverEffectiveSettings,
    RunResult,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How often the turn drains streamed chunks while the message response is in
/// flight.
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How many streamed chunks may be queued before the stream is back-pressured.
const SESSION_EVENT_QUEUE_CAPACITY: usize = 64;

/// Perform one turn, through the engine the host installed.
///
/// The engine owns the endpoint, the HTTP and SSE reads and the active-turn
/// registry; this function asks it for all three through the installed serve
/// port and states the turn's outcome in the host's shared driver vocabulary.
/// The result carries the raw protocol failure this Agent reported, so the
/// host's own normalization stays with the host.
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
    let mut config = match ServeTurnConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return failed(failure, started_at),
    };
    if executable.trim().is_empty() {
        return failed(probe::unavailable_failure(), started_at);
    }
    // No installed engine means no endpoint to attach to and no registry force
    // stop could reach this turn through, so the turn is refused rather than
    // performed without either.
    let Some(ports) = host::ports() else {
        return failed(probe::unavailable_failure(), started_at);
    };

    let attachment = match (ports.serve.ensure_attachment)(executable) {
        Ok(attachment) => attachment,
        Err(error) => {
            return failed(probe::endpoint_failure(&error.to_string()), started_at);
        }
    };
    let Some(model) = attachment.catalog.resolve(config.model.as_deref()) else {
        return failed(
            ProtocolFailure::new(
                "kilo_code_serve_model_unavailable",
                "The selected Kilo model is not available from the current provider catalog.",
                "serve/model",
            ),
            started_at,
        );
    };
    config.model = Some(model.selector());

    let endpoint = serve::ServeEndpoint {
        host: attachment.endpoint.host.clone(),
        port: attachment.endpoint.port,
        attach_url: attachment.endpoint.attach_url.clone(),
    };
    // Admission belongs to the engine and its guard is held for the whole turn:
    // force stop reaches an active turn through this registration, and a turn
    // that gave the guard up early would be unreachable while it still runs. The
    // session identity is bound once the turn has opened it, so the registration
    // names the endpoint rather than a session the turn has not chosen yet.
    let Some(_active_turn) =
        (ports.serve.register_turn)(&endpoint.attach_url, turn_registration_key(&config))
    else {
        return failed(
            ProtocolFailure::new(
                "acp_control_capacity",
                "The Kilo active-turn control registry is at capacity.",
                "turn/control",
            )
            .with_session(Some(&config.requested_session_id)),
            started_at,
        );
    };

    // timeoutMs 0 opts out of any turn deadline (see runtime_adapters/dispatch),
    // so only a non-zero window gets a concrete deadline.
    let deadline = (timeout_ms != 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    match execute_via_serve(&endpoint, &config, deadline) {
        Ok(outcome) => RunResult {
            transitions: outcome.transitions,
            ok: true,
            output: outcome.output,
            error: None,
            session_id: outcome.session_id,
            thread_id: outcome.thread_id,
            turn_id: outcome.turn_id,
            turn_status: outcome.turn_status,
            effective: effective(outcome.effective),
            capabilities: capabilities(outcome.capabilities),
            status_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
            started_at,
            runtime_protocol: RUNTIME_PROTOCOL,
            driver_id: DRIVER.agent_id,
        },
        Err(failure) => failed(failure, started_at),
    }
}

/// The key one turn is registered under.
///
/// A fresh turn has no native session yet, so it registers under the driver
/// identity; a resume registers under the session it is resuming, which is what
/// force stop looks a resumed conversation up by.
fn turn_registration_key(config: &ServeTurnConfig) -> &str {
    if config.is_resume() {
        &config.requested_session_id
    } else {
        DRIVER.agent_id
    }
}

/// This Agent's failure, in the host's shared driver vocabulary.
///
/// The code, the message and the stage are this Agent's own; only the type
/// changes, because the host's driver table reads its own shape.
fn failed(failure: ProtocolFailure, started_at: String) -> RunResult {
    let failure = probe::failure_to_driver(failure).namespaced(DRIVER);
    let transitions = parser::failure_transitions(&failure.code, failure.stage, failure.message);
    RunResult {
        ok: false,
        output: String::new(),
        transitions,
        session_id: failure.session_id.clone().unwrap_or_default(),
        thread_id: failure.thread_id.clone().unwrap_or_default(),
        turn_id: failure.turn_id.clone().unwrap_or_default(),
        turn_status: failure.turn_status.clone().unwrap_or_default(),
        effective: Default::default(),
        capabilities: DriverCapabilityProbe::default(),
        status_code: None,
        stdout_truncated: false,
        stderr_truncated: false,
        started_at,
        runtime_protocol: RUNTIME_PROTOCOL,
        driver_id: DRIVER.agent_id,
        error: Some(failure),
    }
}

fn effective(settings: super::projection::EffectiveSettings) -> DriverEffectiveSettings {
    DriverEffectiveSettings {
        cwd: settings.cwd,
        model: settings.model,
        reasoning_effort: settings.reasoning_effort,
        mode: settings.mode,
        runtime_agent: settings.runtime_agent,
        allow_all: settings.allow_all,
        sandbox: settings.sandbox,
        approval_policy: settings.approval_policy,
    }
}

fn capabilities(probe: super::projection::CapabilityProbe) -> DriverCapabilityProbe {
    DriverCapabilityProbe {
        protocol_version: probe.protocol_version,
        load_session: probe.load_session,
        resume_session: probe.resume_session,
        close_session: probe.close_session,
        list_sessions: probe.list_sessions,
        delete_session: probe.delete_session,
        additional_directories: probe.additional_directories,
        image_prompts: probe.image_prompts,
        audio_prompts: probe.audio_prompts,
        embedded_context: probe.embedded_context,
    }
}

/// Perform one turn against one attached endpoint.
pub fn execute_via_serve(
    endpoint: &serve::ServeEndpoint,
    config: &ServeTurnConfig,
    deadline: Option<Instant>,
) -> Result<ProtocolOutcome, ProtocolFailure> {
    let session_id = open_session(endpoint, config, deadline)?;
    let message_body = build_message_body(config);
    let turn_id = Uuid::new_v4().to_string();
    // Admission is the host's answer, not this package's: the host registers the
    // active turn before calling here, because the active-turn registry and force
    // stop belong to the engine that owns the endpoint. Reaching this point means
    // the host admitted the turn.
    turn_event::emit_turn_event("dispatch.turn.bound", &session_id, &turn_id, json!({}));

    let watch_stop = Arc::new(AtomicBool::new(false));
    let watch_flag = Arc::clone(&watch_stop);
    let watch_url = policy::endpoint_url(&endpoint.attach_url, "/event");
    let watch_session = session_id.clone();
    let (chunk_sender, chunk_receiver) = mpsc::sync_channel::<String>(SESSION_EVENT_QUEUE_CAPACITY);
    let first_failure = Arc::new(Mutex::new(None::<ProtocolFailure>));
    let turn_completed = Arc::new(AtomicBool::new(false));
    let (watch_result_sender, watch_result_receiver) =
        mpsc::channel::<Result<(), ServeStreamOutcome>>();
    let watch_handle = thread::spawn(move || {
        let result = watch_session_events(&watch_url, &watch_session, &watch_flag, &chunk_sender);
        // The sender is moved in so it drops with the thread: the receiver then
        // observes disconnection exactly when the watcher is finished, which is
        // what lets the turn stop draining without a timeout guess.
        let _ = watch_result_sender.send(result);
    });
    let post_url = policy::endpoint_url(
        &endpoint.attach_url,
        &format!("/session/{session_id}/message"),
    );
    let post_failure = Arc::clone(&first_failure);
    let post_completed = Arc::clone(&turn_completed);
    let post_handle = thread::spawn(move || {
        let response = wait_post_json(&post_url, &message_body, deadline);
        match &response {
            Ok(_) => post_completed.store(true, Ordering::Release),
            Err(failure) => record_first_failure(&post_failure, failure.clone()),
        }
        response
    });
    // Drain streamed chunks until the terminal message response has returned
    // and the stream has nothing queued. The observer stops the watch by ending
    // its own frame callback; the turn never waits on a duration it guessed.
    loop {
        match chunk_receiver.recv_timeout(PROCESS_POLL_INTERVAL) {
            Ok(text) => turn_event::emit_agent_message_chunk(&session_id, &turn_id, &text),
            Err(RecvTimeoutError::Timeout) => {
                if post_handle.is_finished() && chunk_receiver.try_iter().next().is_none() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    for text in chunk_receiver.try_iter() {
        turn_event::emit_agent_message_chunk(&session_id, &turn_id, &text);
    }
    let response = join_post(post_handle, &first_failure, &session_id);
    watch_stop.store(true, Ordering::Relaxed);
    // The watcher's own answer, not a signal: the terminal document and the
    // recorded failure already decide the turn's outcome, and a stream that
    // ended after a successful response is observer loss rather than a failure.
    let provisional_observer_failure = watch_result_receiver
        .recv()
        .ok()
        .and_then(|result| result.err())
        .map(|failure| sse_failure(failure, &session_id));
    let _ = watch_handle.join();
    if let Some(failure) = select_canonical_failure(
        &first_failure,
        response.as_ref(),
        provisional_observer_failure,
    ) {
        return Err(failure);
    }
    let response = response.ok_or_else(|| {
        ProtocolFailure::new(
            "kilo_code_serve_cleanup_failed",
            "The Kilo message response worker did not return an outcome.",
            "serve/cleanup",
        )
    })??;
    let response = parser::message(&response).ok_or_else(|| {
        ProtocolFailure::new(
            "kilo_code_serve_final_message_missing",
            "The Kilo turn completed without a final assistant message.",
            "session/prompt",
        )
        .with_session(Some(&session_id))
    })?;
    let outcome = project_turn(response, session_id, turn_id, config)?;
    turn_event::emit_agent_message_completed(
        &outcome.session_id,
        &outcome.turn_id,
        &outcome.output,
    );
    Ok(outcome)
}

fn join_post(
    handle: thread::JoinHandle<Result<Value, ProtocolFailure>>,
    first_failure: &Arc<Mutex<Option<ProtocolFailure>>>,
    session_id: &str,
) -> Option<Result<Value, ProtocolFailure>> {
    match handle.join() {
        Ok(response) => Some(response),
        Err(_) => {
            record_first_failure(
                first_failure,
                ProtocolFailure::new(
                    "kilo_code_serve_cleanup_failed",
                    "The Kilo message response worker could not be joined.",
                    "serve/cleanup",
                )
                .with_session(Some(session_id)),
            );
            None
        }
    }
}

/// Watch the endpoint's event stream, forwarding assistant chunks.
///
/// The stream is the engine's; what a frame means is this parser's. A decode
/// failure is recorded once and ends the watch, because the stream can no longer
/// be trusted to report the turn's progress. A stream that ends before the turn
/// was stopped *is* reported here — the terminal document decides whether that
/// matters, and a turn that already completed ignores it.
fn watch_session_events(
    url: &str,
    session_id: &str,
    stop: &AtomicBool,
    chunks: &mpsc::SyncSender<String>,
) -> Result<(), ServeStreamOutcome> {
    let mut parser = parser::ServeEventParser::new(session_id);
    let mut decode_failure = false;
    let result = serve::watch_frames(url, stop, &mut |data, frame| {
        if data.len() <= 4096 {
            serve::observe_bytes(
                "kilo-code.sse",
                serve::ServeByteDirection::Received,
                Some(session_id),
                frame,
            );
        }
        match parser.observe(data) {
            Ok(Some(text)) => {
                let _ = chunks.try_send(text);
                true
            }
            Ok(None) => true,
            Err(_) => {
                decode_failure = true;
                false
            }
        }
    });
    if decode_failure {
        return Err(ServeStreamOutcome::Decode);
    }
    match result {
        Ok(()) if !stop.load(Ordering::Relaxed) => Err(ServeStreamOutcome::Closed),
        Ok(()) => Ok(()),
        Err(failure) => Err(ServeStreamOutcome::Framing(failure)),
    }
}

/// Why a stream watch ended without the turn being stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServeStreamOutcome {
    Closed,
    Decode,
    Framing(ServeFramingFailure),
}

fn sse_failure(failure: ServeStreamOutcome, session_id: &str) -> ProtocolFailure {
    use ServeStreamOutcome as Outcome;
    let code = match failure {
        Outcome::Closed => "kilo_code_serve_sse_closed",
        Outcome::Decode => "kilo_code_serve_sse_invalid_json",
        Outcome::Framing(ServeFramingFailure::Busy) => "kilo_code_serve_sse_busy",
        Outcome::Framing(ServeFramingFailure::EventLimit) => "kilo_code_serve_sse_event_limit",
        Outcome::Framing(ServeFramingFailure::FrameTooLarge) => {
            "kilo_code_serve_sse_frame_too_large"
        }
        Outcome::Framing(ServeFramingFailure::HeadersTooLarge) => {
            "kilo_code_serve_sse_headers_too_large"
        }
        Outcome::Framing(ServeFramingFailure::InvalidUtf8) => "kilo_code_serve_sse_invalid_utf8",
        Outcome::Framing(ServeFramingFailure::InvalidUrl) => "kilo_code_serve_sse_url_invalid",
        Outcome::Framing(ServeFramingFailure::LineTooLarge) => "kilo_code_serve_sse_line_too_large",
        Outcome::Framing(ServeFramingFailure::Request) => "kilo_code_serve_sse_request_failed",
        Outcome::Framing(ServeFramingFailure::Unavailable) => "kilo_code_serve_sse_unavailable",
    };
    ProtocolFailure::new(
        code,
        "The Kilo event stream failed before the turn completed.",
        "serve/sse",
    )
    .with_session(Some(session_id))
}

/// Bind the native session one turn runs on.
///
/// A resume reads the requested session exactly: an identity that differs from
/// the one asked for is a mismatch, and a missing one is not found. Neither is
/// replaced by another native session, and no message is posted after either.
fn open_session(
    endpoint: &serve::ServeEndpoint,
    config: &ServeTurnConfig,
    deadline: Option<Instant>,
) -> Result<String, ProtocolFailure> {
    if config.is_resume() {
        let url = policy::endpoint_url(
            &endpoint.attach_url,
            &format!("/session/{}", config.requested_session_id),
        );
        return match serve::get_json(&url, true) {
            Ok(payload) => match parser::session_id(&payload) {
                Some(id) if id == config.requested_session_id => Ok(id.to_string()),
                Some(_) => Err(load_identity_mismatch(&config.requested_session_id)),
                None => Err(load_session_not_found(&config.requested_session_id)),
            },
            Err(_) => Err(load_session_not_found(&config.requested_session_id)),
        };
    }

    let body = if config.cwd.is_empty() {
        json!({})
    } else {
        json!({"directory": config.cwd})
    };
    let created = wait_post_json(
        &policy::endpoint_url(&endpoint.attach_url, "/session"),
        &body,
        deadline,
    )?;
    parser::session_id(&created)
        .map(str::to_string)
        .ok_or_else(|| {
            ProtocolFailure::new(
                "acp_session_id_missing",
                "The ACP agent did not return a native conversation identifier.",
                "session/new",
            )
        })
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

fn record_first_failure(slot: &Mutex<Option<ProtocolFailure>>, failure: ProtocolFailure) {
    if let Ok(mut slot) = slot.lock()
        && slot.is_none()
    {
        *slot = Some(failure);
    }
}

fn select_canonical_failure(
    first_failure: &Mutex<Option<ProtocolFailure>>,
    response: Option<&Result<Value, ProtocolFailure>>,
    provisional_observer_failure: Option<ProtocolFailure>,
) -> Option<ProtocolFailure> {
    let exact_failure = first_failure.lock().ok().and_then(|slot| slot.clone());
    if exact_failure.is_some() {
        return exact_failure;
    }
    if matches!(response, Some(Ok(_))) {
        return None;
    }
    provisional_observer_failure
}

/// The message document one turn posts, in this Agent's accepted shape.
///
/// Kilo's `serve` API keeps its gateway routes under the canonical `kilo`
/// provider, including the nested route as the model id, while local history
/// presents the same routes without that prefix. A selector that is not the
/// gateway route is split into its provider and model id; a selector with no
/// provider is not forwarded as a model at all.
pub fn build_message_body(config: &ServeTurnConfig) -> Value {
    let mut body = json!({
        "parts": [{"type": "text", "text": config.prompt}]
    });
    if let Some(model) = config.model.as_deref() {
        if model == "kilo-auto/free" || model.ends_with(":free") {
            body["model"] = json!({
                "providerID": "kilo",
                "modelID": model
            });
        } else if let Some((provider, model_id)) = model.split_once('/') {
            body["model"] = json!({
                "providerID": provider,
                "modelID": model_id
            });
        }
    }
    if let Some(agent) = config.runtime_agent.as_deref() {
        body["agent"] = json!(agent);
    }
    if let Some(reasoning_effort) = config.reasoning_effort.as_deref() {
        body["variant"] = json!(reasoning_effort);
    }
    if let Some(instructions) = config.private_instructions.as_deref() {
        // Kilo's OpenCode-compatible message contract accepts native system
        // guidance independently from the user's text part.
        body["system"] = json!(instructions);
    }
    body
}

/// Post one document, refusing before the write when the deadline already
/// passed.
pub fn wait_post_json(
    url: &str,
    body: &Value,
    deadline: Option<Instant>,
) -> Result<Value, ProtocolFailure> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(ProtocolFailure::new(
            "acp_protocol_timeout",
            "The ACP agent timed out before the turn completed.",
            "session/prompt",
        ));
    }
    serve::post_json(url, body).map_err(|_| {
        ProtocolFailure::new(
            "acp_protocol_write_failed",
            "The ACP agent stopped accepting protocol messages.",
            "serve/http",
        )
    })
}

/// The runtime protocol stamp and driver identity one result carries.
pub fn identity() -> (&'static str, &'static str) {
    (RUNTIME_PROTOCOL, DRIVER_ID)
}

/// The instant one turn started, for the host's result record.
pub fn started_at() -> String {
    timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(model: Option<&str>) -> ServeTurnConfig {
        ServeTurnConfig {
            prompt: "hello".to_owned(),
            private_instructions: None,
            requested_session_id: String::new(),
            cwd: "/workspace/project".to_owned(),
            model: model.map(str::to_owned),
            runtime_agent: None,
            reasoning_effort: None,
            mode: None,
            allow_all: None,
        }
    }

    #[test]
    fn the_gateway_route_keeps_its_canonical_provider_and_nested_model_id() {
        assert_eq!(
            build_message_body(&config(Some("kilo-auto/free")))["model"],
            json!({"providerID": "kilo", "modelID": "kilo-auto/free"})
        );
        assert_eq!(
            build_message_body(&config(Some("some/route:free")))["model"],
            json!({"providerID": "kilo", "modelID": "some/route:free"})
        );
    }

    #[test]
    fn an_ordinary_selector_splits_into_provider_and_model_and_a_bare_one_is_not_forwarded() {
        assert_eq!(
            build_message_body(&config(Some("anthropic/claude")))["model"],
            json!({"providerID": "anthropic", "modelID": "claude"})
        );
        assert!(
            build_message_body(&config(Some("claude")))
                .get("model")
                .is_none()
        );
        assert!(build_message_body(&config(None)).get("model").is_none());
    }

    #[test]
    fn optional_settings_are_forwarded_under_this_agents_own_field_names() {
        let mut config = config(None);
        config.runtime_agent = Some("build".to_owned());
        config.reasoning_effort = Some("high".to_owned());
        config.private_instructions = Some("be brief".to_owned());
        let body = build_message_body(&config);
        assert_eq!(body["agent"], json!("build"));
        assert_eq!(body["variant"], json!("high"));
        assert_eq!(body["system"], json!("be brief"));
        assert_eq!(body["parts"], json!([{"type": "text", "text": "hello"}]));
    }

    #[test]
    fn a_passed_deadline_refuses_before_the_write() {
        let failure = wait_post_json(
            "http://127.0.0.1:4097/session",
            &json!({}),
            Some(Instant::now() - Duration::from_millis(1)),
        )
        .unwrap_err();
        assert_eq!(failure.code, "acp_protocol_timeout");
        assert_eq!(failure.stage, "session/prompt");
    }

    #[test]
    fn every_stream_failure_names_its_own_code() {
        for (outcome, expected) in [
            (ServeStreamOutcome::Closed, "kilo_code_serve_sse_closed"),
            (
                ServeStreamOutcome::Decode,
                "kilo_code_serve_sse_invalid_json",
            ),
            (
                ServeStreamOutcome::Framing(ServeFramingFailure::Busy),
                "kilo_code_serve_sse_busy",
            ),
            (
                ServeStreamOutcome::Framing(ServeFramingFailure::Unavailable),
                "kilo_code_serve_sse_unavailable",
            ),
        ] {
            let failure = sse_failure(outcome, "kilo-1");
            assert_eq!(failure.code, expected);
            assert_eq!(failure.session_id.as_deref(), Some("kilo-1"));
        }
    }

    #[test]
    fn a_finished_turn_is_never_relabelled_by_a_later_stream_end() {
        let first_failure = Mutex::new(None);
        let finished = Ok(json!({"parts": [{"type": "text", "text": "answer"}]}));
        assert!(
            select_canonical_failure(
                &first_failure,
                Some(&finished),
                Some(sse_failure(ServeStreamOutcome::Closed, "kilo-1")),
            )
            .is_none()
        );
        // An exact failure recorded before the terminal response always wins.
        record_first_failure(
            &first_failure,
            ProtocolFailure::new("exact", "observed", "serve/http"),
        );
        assert_eq!(
            select_canonical_failure(&first_failure, Some(&finished), None)
                .unwrap()
                .code,
            "exact"
        );
    }

    #[test]
    fn identity_is_this_agents_own_stamp() {
        assert_eq!(identity(), ("kilo-code-serve-http-v1", "kilo-code-serve"));
        assert!(!started_at().is_empty());
    }

    #[test]
    fn an_empty_executable_fails_closed_without_a_session_fallback() {
        let cwd = std::env::current_dir().unwrap();
        let result = execute(
            "",
            &json!({}),
            "private-kilo-prompt",
            "existing-kilo-native",
            Some(&cwd),
            1_000,
            Some(1024),
            1024,
        );
        assert!(!result.ok);
        assert_eq!(result.driver_id, "kilo-code-serve");
        assert_eq!(result.runtime_protocol, RUNTIME_PROTOCOL);
        assert_eq!(
            result.error.as_ref().map(|failure| failure.code.as_str()),
            Some("kilo_code_serve_process_start_failed")
        );
        assert!(matches!(
            result.transitions.last(),
            Some(super::super::Transition::Failed { code, .. })
                if code == "kilo_code_serve_process_start_failed"
        ));
    }

    #[test]
    fn a_relative_workspace_is_refused_before_the_endpoint_is_attached() {
        let result = execute(
            "kilo",
            &json!({}),
            "prompt",
            "native",
            Some(Path::new("relative")),
            1_000,
            None,
            1024,
        );
        assert!(!result.ok);
        assert_eq!(
            result.error.as_ref().map(|failure| failure.code.as_str()),
            Some("kilo_code_serve_working_directory_invalid")
        );
    }

    #[test]
    fn a_fresh_turn_registers_under_the_driver_and_a_resume_under_its_session() {
        let fresh = ServeTurnConfig::from_params(&json!({"cwd": "/workspace"}), "prompt", "", None)
            .unwrap();
        assert_eq!(turn_registration_key(&fresh), "kilo-code-serve");
        let resumed =
            ServeTurnConfig::from_params(&json!({"cwd": "/workspace"}), "prompt", "kilo-1", None)
                .unwrap();
        assert_eq!(turn_registration_key(&resumed), "kilo-1");
    }
}
