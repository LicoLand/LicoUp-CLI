//! OpenCode's `serve` protocol, interpreted once below the adapter port.
//!
//! `opencode serve` is a loopback HTTP service. One turn reads three real
//! document kinds from it and nothing else: the session identity document the
//! session endpoint returns, the SSE `data:` payloads of the event stream, and
//! the whole-message document the message endpoint returns when the turn
//! completes. What each of those means for a turn is stated here, exactly once,
//! and no reader above re-parses a vendor frame (ADR-0008).
//!
//! The module is the interpreter, not the engine. It starts no process, opens
//! no socket, keeps no session state, settles no turn and writes nothing; the
//! shared local-service engine in `licoup-agent-drivers` owns the process, the
//! HTTP and SSE transports and the active-turn registry, and reads this module
//! through the client's own `opencode_serve` facade.
//!
//! Two directions are stated here rather than in a caller:
//!
//! * [`ServeEventParser`] is the streaming half. It binds to one native session
//!   and reports assistant text only after the message it belongs to has been
//!   announced as an assistant message, so a user part or another session's part
//!   is never projected as the Agent's reply.
//! * [`message`] is the terminal half. It folds the completed message document
//!   into the turn's reply and its typed transitions, including one
//!   [`Transition::Control`] per native tool interaction the Agent reported.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use licoup_agent_drivers::local_service::{ServeModel, ServeModelCatalog, ServeReadiness};
use serde_json::Value;
use std::collections::HashSet;

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: AdapterContract = AdapterContract::new("opencode", "http-sse");

/// Why one SSE event frame could not be interpreted.
///
/// The variants are a closed vocabulary rather than a forwarded vendor error: a
/// frame that does not decode is the only thing the parser rejects, and the
/// caller decides what a stream that carried it means for the turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeEventFailure {
    /// The frame was not a JSON document.
    InvalidJson,
}

/// One completed OpenCode message, and the transitions it reports.
#[derive(Debug)]
pub struct ServeMessage {
    /// The assistant text the message document carried, in part order.
    pub output: String,
    /// The typed transitions one completed OpenCode turn reports.
    pub transitions: Vec<Transition>,
}

/// The streaming half: one native session's assistant text, exactly once.
///
/// The drive builds this once the session identity is known, feeds it every SSE
/// `data:` payload in arrival order, and gets back the text of each assistant
/// part that belongs to this session. A part is reported only when the message
/// it belongs to has already been announced as an assistant message on this
/// session, so a user's own part and a neighbouring session's part are both
/// excluded rather than guessed at.
pub struct ServeEventParser {
    session_id: String,
    assistant_messages: HashSet<String>,
}

impl ServeEventParser {
    /// Bind the parser to one native session identity.
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            assistant_messages: HashSet::new(),
        }
    }

    /// Interpret one SSE `data:` payload, in the order the stream delivered it.
    ///
    /// `Ok(None)` is the parser's answer for a frame that carries no assistant
    /// text: a message announcement, a part of another session, a non-text part,
    /// or a frame the protocol does not classify. `Err` is reserved for a frame
    /// that is not JSON at all, which is the stream's own framing failure rather
    /// than a turn outcome.
    pub fn observe(&mut self, frame: &str) -> Result<Option<String>, ServeEventFailure> {
        let event =
            serde_json::from_str::<Value>(frame).map_err(|_| ServeEventFailure::InvalidJson)?;
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(properties) = event.get("properties") else {
            return Ok(None);
        };
        if event_type == "message.updated" {
            if let Some(info) = properties.get("info")
                && session_id(info) == Some(self.session_id.as_str())
                && info.get("role").and_then(Value::as_str) == Some("assistant")
                && let Some(message_id) = info.get("id").and_then(Value::as_str)
            {
                self.assistant_messages.insert(message_id.to_owned());
            }
            return Ok(None);
        }
        if event_type != "message.part.updated"
            || session_id(properties) != Some(self.session_id.as_str())
        {
            return Ok(None);
        }
        let Some(part) = properties.get("part") else {
            return Ok(None);
        };
        if part.get("type").and_then(Value::as_str) != Some("text") {
            return Ok(None);
        }
        let Some(message_id) = message_id(part) else {
            return Ok(None);
        };
        if !self.assistant_messages.contains(message_id) {
            return Ok(None);
        }
        Ok(part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned))
    }
}

/// Whether one health document reports a ready endpoint.
pub fn health_ready(frame: &Value) -> bool {
    frame.get("healthy").and_then(Value::as_bool) == Some(true)
}

/// The native session identity one OpenCode document carries.
///
/// The three spellings are the protocol's own: a session document names `id`, a
/// stream payload names `sessionID`, and a session-scoped document names
/// `sessionId`. An empty value is not an identity and is reported as absent.
pub fn session_id(frame: &Value) -> Option<&str> {
    frame
        .get("sessionID")
        .or_else(|| frame.get("sessionId"))
        .or_else(|| frame.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// Whether one session endpoint document is the session collection.
pub fn session_collection(frame: &Value) -> bool {
    frame.as_array().is_some()
}

/// The readiness one attached endpoint reports, or `None` when it is not ready.
///
/// The four documents are the health probe, the session collection, the
/// configuration document and the provider catalogue, in the order the serve
/// policy reads them. Readiness is a conjunction of protocol facts — a healthy
/// endpoint, a session collection, a named version — and a selected model: the
/// configured model when the configuration names one, otherwise the provider's
/// own default in provider order.
///
/// The session documents are read for their shape only. A session's own recorded
/// model is deliberately not consulted: a stale session is not evidence about
/// what the endpoint can run now, and readiness decides what a new turn may ask
/// for.
pub fn readiness(
    health: &Value,
    sessions: &Value,
    config: &Value,
    providers: &Value,
) -> Option<ServeReadiness> {
    if !health_ready(health) || !session_collection(sessions) {
        return None;
    }
    let version = health.get("version")?.as_str()?.trim();
    if version.is_empty() {
        return None;
    }
    let provider_rows = providers
        .get("all")
        .or_else(|| providers.get("providers"))?
        .as_array()?;
    let mut models = Vec::<ServeModel>::new();
    let mut provider_order = Vec::<String>::new();
    for provider in provider_rows {
        let Some(provider_id) = provider
            .get("id")
            .or_else(|| provider.get("providerID"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        provider_order.push(provider_id.to_string());
        if let Some(rows) = provider.get("models").and_then(Value::as_object) {
            for model_id in rows.keys().map(String::as_str) {
                push_model(&mut models, provider_id, model_id);
            }
        } else if let Some(rows) = provider.get("models").and_then(Value::as_array) {
            for row in rows {
                if let Some(model_id) = row
                    .get("id")
                    .or_else(|| row.get("modelID"))
                    .and_then(Value::as_str)
                {
                    push_model(&mut models, provider_id, model_id);
                }
            }
        }
    }
    let defaults = providers.get("default").and_then(Value::as_object);
    let configured = configured_model(config, &models, &provider_order);
    let current = configured.or_else(|| {
        provider_order.iter().find_map(|provider_id| {
            let model_id = defaults?.get(provider_id)?.as_str()?.trim();
            (!model_id.is_empty()).then(|| ServeModel {
                provider_id: provider_id.clone(),
                model_id: model_id.to_string(),
            })
        })
    })?;
    push_model(&mut models, &current.provider_id, &current.model_id);
    Some(ServeReadiness {
        version: version.to_string(),
        catalog: ServeModelCatalog { current, models },
        health: serde_json::json!({"healthy": true, "version": version}),
    })
}

fn configured_model(
    config: &Value,
    models: &[ServeModel],
    provider_order: &[String],
) -> Option<ServeModel> {
    let value = config.get("model")?;
    if let Some(object) = value.as_object() {
        let provider_id = object
            .get("providerID")
            .or_else(|| object.get("providerId"))?
            .as_str()?
            .trim();
        let model_id = object
            .get("modelID")
            .or_else(|| object.get("modelId"))?
            .as_str()?
            .trim();
        return (!provider_id.is_empty() && !model_id.is_empty()).then(|| ServeModel {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        });
    }
    let selector = value.as_str()?.trim();
    if let Some(exact) = models.iter().find(|model| model.selector() == selector) {
        return Some(exact.clone());
    }
    let model_id_matches = models
        .iter()
        .filter(|model| model.model_id == selector)
        .collect::<Vec<_>>();
    if model_id_matches.len() == 1 {
        return Some(model_id_matches[0].clone());
    }
    if let Some(current_provider_match) = provider_order.iter().find_map(|provider| {
        model_id_matches
            .iter()
            .find(|model| model.provider_id == *provider)
            .copied()
    }) {
        return Some(current_provider_match.clone());
    }
    if let Some((provider_id, model_id)) = selector.split_once('/') {
        return (!provider_id.is_empty() && !model_id.is_empty()).then(|| ServeModel {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        });
    }
    None
}

fn push_model(models: &mut Vec<ServeModel>, provider_id: &str, model_id: &str) {
    let provider_id = provider_id.trim();
    let model_id = model_id.trim();
    if provider_id.is_empty() || model_id.is_empty() {
        return;
    }
    if !models
        .iter()
        .any(|model| model.provider_id == provider_id && model.model_id == model_id)
    {
        models.push(ServeModel {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        });
    }
}

/// The terminal half: one completed message document, and its transitions.
///
/// `None` is the parser's literal answer for a document that carries no
/// assistant text. A `serve` turn has no in-band error terminal — it fails out
/// of band, through the HTTP status and the abort control lane — so a
/// whole-message document without assistant text leaves this boundary with
/// nothing to report rather than inventing a failure.
pub fn message(frame: &Value) -> Option<ServeMessage> {
    let output = assistant_text(frame);
    if output.is_empty() {
        return None;
    }
    let transitions = completed_transitions_with_controls(&output, tool_controls(frame));
    Some(ServeMessage {
        output,
        transitions,
    })
}

fn message_id(value: &Value) -> Option<&str> {
    value
        .get("messageID")
        .or_else(|| value.get("messageId"))
        .and_then(Value::as_str)
}

fn assistant_text(response: &Value) -> String {
    let mut chunks = Vec::new();
    if let Some(parts) = response.get("parts").and_then(Value::as_array) {
        append_text_parts(parts, &mut chunks);
    }
    if chunks.is_empty()
        && let Some(items) = response.as_array()
    {
        for item in items {
            if let Some(parts) = item.get("parts").and_then(Value::as_array) {
                append_text_parts(parts, &mut chunks);
            }
        }
    }
    chunks.concat()
}

fn append_text_parts(parts: &[Value], chunks: &mut Vec<String>) {
    for part in parts {
        if part.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = part.get("text").and_then(Value::as_str)
        {
            chunks.push(text.to_owned());
        }
    }
}

/// The transitions one completed OpenCode turn with no tool interaction reports.
///
/// It is the same projection [`message`] builds for a message document that
/// names no tool part, stated as its own entry point so a reader that already
/// holds the reply text — the host's own response normalization, for one — reads
/// the parser's transitions rather than retyping them.
pub fn completed_transitions(output: &str) -> Vec<Transition> {
    completed_transitions_with_controls(output, Vec::new())
}

fn completed_transitions_with_controls(output: &str, controls: Vec<Transition>) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    transitions.extend(controls);
    if !output.is_empty() {
        transitions.extend(reducer.advance(LifecycleStage::Responding));
        transitions.push(Transition::Text {
            unit_id: "opencode:reply".to_owned(),
            text: output.to_owned(),
        });
    }
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// The transitions one failed OpenCode turn reports, in arrival order.
///
/// The turn is submitted and then fails with the caller's own code, stage and
/// redacted message: the reducer is the authority on what a failure may be
/// reported after, and the first failure stays write-once.
pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Submitted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

fn tool_controls(response: &Value) -> Vec<Transition> {
    let mut controls = Vec::new();
    if let Some(parts) = response.get("parts").and_then(Value::as_array) {
        append_tool_controls(parts, &mut controls);
    }
    if let Some(items) = response.as_array() {
        for item in items {
            if let Some(parts) = item.get("parts").and_then(Value::as_array) {
                append_tool_controls(parts, &mut controls);
            }
        }
    }
    controls
}

fn append_tool_controls(parts: &[Value], controls: &mut Vec<Transition>) {
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let method = part
            .get("tool")
            .or_else(|| part.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("native_tool")
            .chars()
            .take(64)
            .collect();
        controls.push(Transition::Control {
            method,
            summary: "OpenCode reported a native tool interaction.".to_owned(),
        });
    }
}
