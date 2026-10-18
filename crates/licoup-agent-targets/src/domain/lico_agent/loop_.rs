use super::events::AgentEvent;
use super::tools::ToolRegistry;
use super::transport::LlmTransport;
use crate::port::AgentTargetPort;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn run_turn(
    port: &AgentTargetPort,
    transport: &dyn LlmTransport,
    model: &str,
    system_prompt: &str,
    history: &mut Vec<Value>,
    tools: &ToolRegistry,
    abort: &AtomicBool,
    mut on_event: impl FnMut(AgentEvent),
) -> Result<(), String> {
    on_event(AgentEvent::TurnStart);
    let tool_defs = tools.definitions_for_llm();
    let mut messages = vec![json!({"role": "system", "content": system_prompt})];
    messages.extend(history.iter().cloned());

    for _ in 0..8 {
        if abort.load(Ordering::SeqCst) {
            on_event(AgentEvent::Error {
                code: "aborted".into(),
                message: "turn aborted".into(),
            });
            break;
        }
        let response = transport
            .complete(model, &messages, &tool_defs)
            .map_err(|e| e.to_string())?;
        if let Some(event) = response_usage(port, &response, model) {
            on_event(event);
        }
        let choice = response
            .pointer("/choices/0/message")
            .cloned()
            .ok_or_else(|| "gateway_response_missing_message".to_string())?;
        let role = choice
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant")
            .to_string();
        on_event(AgentEvent::MessageStart { role: role.clone() });
        if let Some(content) = choice.get("content").and_then(Value::as_str) {
            if !content.is_empty() {
                on_event(AgentEvent::MessageUpdate {
                    role: role.clone(),
                    delta: content.to_string(),
                });
                on_event(AgentEvent::MessageEnd {
                    role: role.clone(),
                    content: content.to_string(),
                });
            }
        }
        messages.push(choice.clone());
        history.push(choice.clone());

        let tool_calls = choice
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tool_calls.is_empty() {
            break;
        }
        for call in tool_calls {
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args_raw = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let args: Value = serde_json::from_str(args_raw).unwrap_or(json!({}));
            on_event(AgentEvent::ToolExecutionStart {
                name: name.clone(),
                call_id: call_id.clone(),
            });
            let (ok, output) = match tools.get(&name) {
                Some(tool) => match tool.execute(&args) {
                    Ok(out) => (true, out),
                    Err(err) => (false, err.to_string()),
                },
                None => (false, format!("unknown_tool:{name}")),
            };
            on_event(AgentEvent::ToolExecutionEnd {
                name: name.clone(),
                call_id: call_id.clone(),
                ok,
                output: output.clone(),
            });
            let tool_msg = json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output,
            });
            messages.push(tool_msg.clone());
            history.push(tool_msg);
        }
    }
    on_event(AgentEvent::TurnEnd);
    Ok(())
}

fn response_usage(
    port: &AgentTargetPort,
    response: &Value,
    requested_model: &str,
) -> Option<AgentEvent> {
    let usage = (port.extract_token_usage)(response)?;
    let concrete_model = |model: &str| {
        let model = model.trim();
        (!model.is_empty()
            && !["auto", "default", "unknown", "unspecified"]
                .into_iter()
                .any(|selector| model.eq_ignore_ascii_case(selector)))
        .then(|| model.to_owned())
    };
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .and_then(concrete_model)
        .or_else(|| concrete_model(requested_model));
    Some(AgentEvent::Usage { model, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::AgentTargetPort;

    /// The usage record a recorded response carries is conversation state, so
    /// the loop reads it through the port. These fixtures answer it with the
    /// projection the loop is asked to carry into an event; the counter and
    /// field-selection claims themselves belong to that owner and are asserted
    /// there — `domain/conversation/usage.rs`'s own tests cover every shape
    /// these two used to build, including the private-content canary in
    /// `usage_normalization_preserves_actual_options_without_copying_content`.
    fn port_returning(extract: crate::port::ExtractTokenUsage) -> AgentTargetPort {
        AgentTargetPort {
            extract_token_usage: extract,
            ..AgentTargetPort::unavailable()
        }
    }

    #[test]
    fn usage_is_carried_into_the_event_with_the_resolved_model() {
        static USAGE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        fn extract(_value: &Value) -> Option<Value> {
            USAGE.get().cloned()
        }
        USAGE
            .set(json!({
                "promptTokens": 12,
                "cachedInputTokens": 4,
                "completionTokens": 3,
                "totalTokens": 15,
                "reasoningEffort": "high",
                "fast": false
            }))
            .ok();
        let event = response_usage(
            &port_returning(extract),
            &json!({"model": "actual-model"}),
            "requested-model",
        )
        .unwrap();
        let AgentEvent::Usage { model, usage } = event else {
            panic!("expected usage event");
        };
        assert_eq!(model.as_deref(), Some("actual-model"));
        assert_eq!(usage["promptTokens"], 12);
        assert_eq!(usage["cachedInputTokens"], 4);
        assert_eq!(usage["completionTokens"], 3);
        assert_eq!(usage["totalTokens"], 15);
        assert_eq!(usage["reasoningEffort"], "high");
        assert_eq!(usage["fast"], false);
    }

    #[test]
    fn usage_without_response_options_does_not_invent_effort_or_auto_model() {
        let response = json!({"usage": {"prompt_tokens": 2, "completion_tokens": 1}});
        for selector in ["auto", " Default ", "UNKNOWN", "unspecified", ""] {
            let AgentEvent::Usage { model, usage } = response_usage(
                &port_returning(|_| Some(json!({"promptTokens": 2, "completionTokens": 1}))),
                &response,
                selector,
            )
            .unwrap() else {
                panic!("expected usage event");
            };
            assert!(model.is_none());
            assert!(usage.get("reasoningEffort").is_none());
            assert!(usage.get("fast").is_none());
        }
        let AgentEvent::Usage { model, .. } = response_usage(
            &port_returning(|_| Some(json!({"promptTokens": 2}))),
            &response,
            "explicit-request-model",
        )
        .unwrap() else {
            panic!("expected usage event");
        };
        assert_eq!(model.as_deref(), Some("explicit-request-model"));
        // A response whose owner reports no usage record produces no event.
        assert!(
            response_usage(
                &port_returning(|_| None),
                &json!({"model": "actual-model"}),
                "auto"
            )
            .is_none()
        );
    }

    #[test]
    fn a_response_options_object_without_requested_model_keeps_the_option_only() {
        let AgentEvent::Usage { model, usage } = response_usage(
            &port_returning(|_| Some(json!({"reasoningEffort": "high", "fast": true}))),
            &json!({"model": "configured-model"}),
            "configured-model",
        )
        .unwrap() else {
            panic!("expected usage event");
        };
        assert_eq!(model.as_deref(), Some("configured-model"));
        assert_eq!(usage["reasoningEffort"], "high");
        assert_eq!(usage["fast"], true);
    }
}
