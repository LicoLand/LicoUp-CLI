use super::*;

#[test]
fn streaming_chunks_emit_progressive_turn_events() {
    use std::sync::{Arc, Mutex};

    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_target = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;

    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", ""),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-hermes-session"}
    }));
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "native-hermes-session",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "watch-"}
            }
        }
    }));
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "native-hermes-session",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "able"}
            }
        }
    }));
    let completed = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    }));
    assert!(matches!(completed[0], ProtocolEffect::Complete(_)));
    assert_eq!(protocol.output, "watch-able");

    let events = captured.lock().unwrap().clone();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["event"], "agent.message.chunk");
    assert_eq!(events[0]["payload"]["text"], "watch-");
    assert_eq!(events[0]["sessionId"], "native-hermes-session");
    assert_eq!(events[1]["payload"]["text"], "able");
    assert_eq!(events[2]["event"], "agent.message.completed");
    assert_eq!(events[2]["payload"]["text"], "watch-able");
    assert!(!events[0]["turnId"].as_str().unwrap_or("").is_empty());
}

#[test]
fn session_update_for_another_session_fails_closed() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", ""),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-hermes-session"}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "other-session",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "wrong"}
            }
        }
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("cross-session output must fail the protocol")
    };
    assert_eq!(failure.code, "acp_session_mismatch");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
    assert!(protocol.output.is_empty());
}
