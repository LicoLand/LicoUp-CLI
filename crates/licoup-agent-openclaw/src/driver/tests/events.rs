use super::*;

#[test]
fn event_projection_keeps_agent_text_but_removes_runtime_metadata() {
    let session_key = ["private", "key"].join("-");
    let private_path = ["/", "private", "path"].join("/");
    let update = validated_update(json!({
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": "visible answer"},
        "_meta": {"sessionKey": session_key.clone(), "path": private_path.clone()}
    }));
    let event = projected_event(&update).unwrap();
    assert_eq!(event["content"]["text"], "visible answer");
    let encoded = event.to_string();
    assert!(!encoded.contains(&session_key));
    assert!(!encoded.contains(&private_path));
}

#[test]
fn tool_and_thought_events_do_not_project_arguments_or_content() {
    let private_path = ["/", "private", "path"].join("/");
    let tool = projected_event(&validated_update(json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "opaque-id",
        "title": format!("read {private_path}"),
        "rawInput": {"secret": "value"}
    })))
    .unwrap();
    assert_eq!(tool, json!({"sessionUpdate": "tool_call"}));

    let thought = projected_event(&validated_update(json!({
        "sessionUpdate": "agent_thought_chunk",
        "content": {"type": "text", "text": "private reasoning"}
    })))
    .unwrap();
    assert_eq!(thought, json!({"sessionUpdate": "agent_thought_chunk"}));
}

#[test]
fn session_info_is_never_projected() {
    let session_key = ["private", "key"].join("-");
    let update = validated_update(json!({
        "sessionUpdate": "session_info_update",
        "_meta": {"sessionKey": session_key}
    }));
    assert!(projected_event(&update).is_none());
}
