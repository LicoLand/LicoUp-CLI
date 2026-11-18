//! This Agent's vendor protocol: the `serve` documents Kilo Code answers, read
//! once below the adapter port.
//!
//! Kilo Code's headless program runs an OpenCode-compatible HTTP service. Every
//! fact LicoUp reads out of that service is read here and nowhere else: the
//! health document, the session identity document, the session collection, the
//! provider catalogue that decides readiness, and the whole-message document
//! that closes a turn. The event stream's own documents are classified by
//! [`serve`], which is the same protocol's stream half.
//!
//! The module is the sole ingress, per ADR-0008. Nothing above it re-reads a
//! vendor document: a caller that needs a session identity calls [`session_id`],
//! not `Value::get`.
//!
//! [`CONTRACT`] is this Agent's adapter declaration. `http-sse` is the framing
//! its protocol really speaks — an HTTP request/response pair for the turn, an
//! SSE stream for its progress — and it is the same string the fixtures record,
//! so a corpus cannot pass against another channel.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::serve::{ServeModel, ServeModelCatalog, ServeReadiness};
use serde_json::Value;

pub mod serve;

pub use serve::{ServeEventFailure, ServeEventParser};

/// The shared lifecycle vocabulary, re-exported so a caller of this module's
/// projections names one path rather than two.
pub use licoup_agent_adapter_sdk::{LifecycleStage, Transition};

use licoup_agent_adapter_sdk::TransitionReducer;

/// This Agent's adapter declaration.
pub const CONTRACT: AdapterContract = AdapterContract::new("kilo-code", FRAMING);

/// The adapter id this package carries.
pub const ID: &str = "kilo-code";

/// The framing this Agent's protocol speaks, and the channel its fixtures
/// record.
pub const FRAMING: &str = "http-sse";

/// The one assistant message a completed serve turn carries.
#[derive(Debug)]
pub struct ServeMessage {
    /// The turn's assistant text.
    pub output: String,
    /// The native tool interactions the same document reported.
    pub transitions: Vec<Transition>,
}

/// Whether the service's health document reports a healthy endpoint.
pub fn health_ready(frame: &Value) -> bool {
    frame.get("healthy").and_then(Value::as_bool) == Some(true)
}

/// The native session identity a serve document carries.
///
/// Kilo Code answers the identity under `sessionID`, the OpenCode-compatible
/// spelling, and older documents use `sessionId` or a bare `id`. An empty
/// identity is not an identity: it is reported as absent so a caller never binds
/// a turn to nothing.
pub fn session_id(frame: &Value) -> Option<&str> {
    frame
        .get("sessionID")
        .or_else(|| frame.get("sessionId"))
        .or_else(|| frame.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// Whether the service's session document is the collection it must be.
pub fn session_collection(frame: &Value) -> bool {
    frame.as_array().is_some()
}

/// What one capability probe learned from this Agent's endpoint.
///
/// Readiness needs all four documents: a healthy endpoint, a session
/// collection, a non-empty version and a provider catalogue that yields a
/// current model. Any one of them missing reports `None` — the endpoint is not
/// ready, and a partial answer would let a turn start against a service that
/// cannot serve it.
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

/// The model the service's own configuration names, resolved against the
/// catalogue it reported.
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

/// The completed message a serve message document carries, or `None` when it
/// carries no assistant text.
///
/// `None` is an answer: this protocol fails a turn out of band (an HTTP status,
/// the abort control lane), so a document without assistant text is a message
/// with nothing to report rather than a malformed one.
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

/// The transitions one completed serve turn produces.
///
/// It is the shared lifecycle machine's own answer, advanced in arrival order,
/// with the assistant reply emitted as this Agent's text unit. It settles no
/// turn: the caller decides what a completed protocol turn means.
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
            unit_id: "kilo-code:reply".to_owned(),
            text: output.to_owned(),
        });
    }
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// The transitions one failed serve turn produces.
pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Submitted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

/// The control transitions a serve document's native tool parts report.
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
        // The method is a control label, not content: it is bounded so one
        // document cannot grow an unbounded field, and the summary is this
        // Agent's own fixed wording rather than the vendor's text.
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
            summary: "Kilo reported a native tool interaction.".to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serve_frames_decode_only_inside_the_kilo_component() {
        assert!(health_ready(&json!({"healthy": true})));
        assert_eq!(session_id(&json!({"id": "kilo-1"})), Some("kilo-1"));
        assert!(session_collection(&json!([])));
        let parsed = message(&json!({"parts": [
            {"type": "reasoning", "text": "hidden"},
            {"type": "text", "text": "answer"}
        ]}))
        .unwrap();
        assert_eq!(parsed.output, "answer");
    }

    #[test]
    fn serve_execution_produces_closed_typed_transitions() {
        let transitions = completed_transitions("answer");
        assert!(matches!(
            transitions.last(),
            Some(Transition::Lifecycle(LifecycleStage::Completed))
        ));
        let failed = failure_transitions("code", "serve/sse", "safe");
        assert!(matches!(
            failed.last(),
            Some(Transition::Failed { code, .. }) if code == "code"
        ));
    }

    #[test]
    fn readiness_requires_version_sessions_and_current_provider_model() {
        let ready = readiness(
            &json!({"healthy": true, "version": "1.2.3"}),
            &json!([{"id": "old", "model": "stale/session-model"}]),
            &json!({"model": "kilo-auto/free"}),
            &json!({
                "all": [{"id": "kilo", "models": {"kilo-auto/free": {}}}],
                "default": {"kilo": "kilo-auto/free"}
            }),
        )
        .unwrap();
        assert_eq!(ready.version, "1.2.3");
        assert_eq!(ready.catalog.current.provider_id, "kilo");
        assert_eq!(ready.catalog.current.model_id, "kilo-auto/free");
        assert!(
            readiness(
                &json!({"healthy": true}),
                &json!([]),
                &json!({}),
                &json!({"all": []}),
            )
            .is_none()
        );
    }

    #[test]
    fn tool_controls_are_bounded_and_text_parts_come_from_both_shapes() {
        let long = "t".repeat(200);
        let parsed = message(&json!({"parts": [
            {"type": "tool", "tool": long},
            {"type": "text", "text": "answer"}
        ]}))
        .unwrap();
        let control = parsed
            .transitions
            .iter()
            .find_map(|transition| match transition {
                Transition::Control { method, summary } => Some((method, summary)),
                _ => None,
            })
            .expect("a native tool part must report one control transition");
        assert_eq!(control.0.len(), 64, "the control label is bounded");
        assert_eq!(control.1, "Kilo reported a native tool interaction.");
        // A bare array of items is the other accepted whole-message shape.
        let list = message(&json!([
            {"parts": [{"type": "text", "text": "first "}]},
            {"parts": [{"type": "text", "text": "second"}]}
        ]))
        .unwrap();
        assert_eq!(list.output, "first second");
    }
}
