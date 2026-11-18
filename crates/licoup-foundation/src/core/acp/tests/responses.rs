use super::*;
use serde_json::json;

#[test]
fn initialize_response_validates_version_and_capability_types() {
    let response = validate_initialize_response(
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "sessionCapabilities": {"resume": {}, "close": {}, "list": {}},
                    "promptCapabilities": {"image": true},
                    "mcpCapabilities": {"http": true, "sse": false}
                }
            }
        }),
        1,
    )
    .unwrap();
    assert!(response.capabilities.load_session);
    assert!(response.capabilities.resume_session);
    assert!(response.capabilities.close_session);
    assert!(response.capabilities.list_sessions);
    assert!(response.capabilities.image_prompts);
    assert!(response.capabilities.mcp_http);

    let unsupported = validate_initialize_response(
        &json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 2}}),
        1,
    );
    assert_eq!(
        unsupported.unwrap_err(),
        AcpError::UnsupportedProtocolVersion { received: 2 }
    );
    let malformed = validate_initialize_response(
        &json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": "yes"}}
        }),
        1,
    );
    assert_eq!(malformed.unwrap_err(), AcpError::CapabilityInvalid);
}

#[test]
fn session_list_response_validates_metadata_and_cursor() {
    let response = validate_session_list_response(
        &json!({
            "jsonrpc": "2.0",
            "id": 9,
            "result": {
                "sessions": [{
                    "sessionId": "session-1",
                    "cwd": "/workspace/project",
                    "additionalDirectories": [
                        "/workspace/shared",
                        "/workspace/generated"
                    ],
                    "title": "First task",
                    "updatedAt": "2026-07-26T00:00:00Z",
                    "_meta": {"messageCount": 2}
                }],
                "nextCursor": "opaque-next"
            }
        }),
        9,
    )
    .unwrap();
    assert_eq!(response.sessions[0].session_id, "session-1");
    assert_eq!(response.sessions[0].cwd, "/workspace/project");
    assert_eq!(
        response.sessions[0].additional_directories,
        ["/workspace/shared", "/workspace/generated"]
    );
    assert_eq!(response.next_cursor.as_deref(), Some("opaque-next"));
}

#[test]
fn session_list_response_rejects_invalid_additional_directories() {
    for additional_directories in [
        json!(null),
        json!(["relative/path"]),
        json!(["/workspace/shared", 7]),
    ] {
        let response = validate_session_list_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 9,
                "result": {
                    "sessions": [{
                        "sessionId": "session-1",
                        "cwd": "/workspace/project",
                        "additionalDirectories": additional_directories
                    }]
                }
            }),
            9,
        );
        assert_eq!(response.unwrap_err(), AcpError::SessionListResponseInvalid);
    }
}

#[test]
fn response_envelope_rejects_wrong_jsonrpc_id_and_mixed_outcomes() {
    let wrong_version = validate_initialize_response(
        &json!({"jsonrpc": "1.0", "id": 1, "result": {"protocolVersion": 1}}),
        1,
    );
    assert_eq!(wrong_version.unwrap_err(), AcpError::JsonRpcVersionInvalid);
    let wrong_id = validate_initialize_response(
        &json!({"jsonrpc": "2.0", "id": 2, "result": {"protocolVersion": 1}}),
        1,
    );
    assert_eq!(wrong_id.unwrap_err(), AcpError::ResponseIdMismatch);
    let mixed = validate_initialize_response(
        &json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": 1},
            "error": {"code": -32600, "message": "invalid"}
        }),
        1,
    );
    assert_eq!(mixed.unwrap_err(), AcpError::ResponseOutcomeInvalid);
}

#[test]
fn session_response_requires_a_valid_new_session_id() {
    let created = validate_session_response(
        &json!({
            "jsonrpc": "2.0", "id": 2,
            "result": {"sessionId": "session-1", "configOptions": []}
        }),
        2,
        AcpSessionMethod::New,
    )
    .unwrap();
    assert_eq!(created.session_id.as_deref(), Some("session-1"));

    let missing = validate_session_response(
        &json!({"jsonrpc": "2.0", "id": 2, "result": {}}),
        2,
        AcpSessionMethod::New,
    );
    assert_eq!(missing.unwrap_err(), AcpError::SessionResponseInvalid);

    let wrong_type = validate_session_response(
        &json!({"jsonrpc": "2.0", "id": 2, "result": {"sessionId": 7}}),
        2,
        AcpSessionMethod::New,
    );
    assert_eq!(wrong_type.unwrap_err(), AcpError::SessionResponseInvalid);

    let invalid = validate_session_response(
        &json!({"jsonrpc": "2.0", "id": 2, "result": {"sessionId": ""}}),
        2,
        AcpSessionMethod::New,
    );
    assert_eq!(invalid.unwrap_err(), AcpError::SessionIdInvalid);
}

#[test]
fn load_response_accepts_legacy_null() {
    let legacy = validate_session_response(
        &json!({"jsonrpc": "2.0", "id": 2, "result": null}),
        2,
        AcpSessionMethod::Load("session-1"),
    )
    .unwrap();
    assert!(legacy.session_id.is_none());
    assert!(legacy.modes.is_none());
    assert!(legacy.config_options.is_empty());
}

#[test]
fn load_response_accepts_idless_optional_state_object() {
    let restored = validate_session_response(
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "configOptions": [{
                    "id": "pace",
                    "name": "Pace",
                    "type": "select",
                    "currentValue": "steady",
                    "options": [{"value": "steady", "name": "Steady"}]
                }]
            }
        }),
        2,
        AcpSessionMethod::Load("session-1"),
    )
    .unwrap();
    assert!(restored.session_id.is_none());
    assert!(restored.modes.is_none());
    assert_eq!(restored.config_options.len(), 1);
    assert_eq!(restored.config_options[0]["id"], "pace");
}

#[test]
fn load_response_accepts_matching_id_optional_state_object() {
    let modes = json!({
        "currentModeId": "review",
        "availableModes": [{
            "id": "review",
            "name": "Review",
            "description": "Review the synthetic fixture",
            "futureModeField": {"preserved": true}
        }],
        "futureModesField": true
    });
    let config_options = json!([
        {
            "id": "pace",
            "name": "Pace",
            "description": "Synthetic select option",
            "type": "select",
            "currentValue": "steady",
            "options": [{
                "value": "steady",
                "name": "Steady",
                "futureValueField": true
            }],
            "futureConfigField": {"preserved": true}
        },
        {
            "id": "guarded",
            "name": "Guarded",
            "type": "boolean",
            "currentValue": true,
            "futureConfigField": true
        },
        {
            "id": "profile",
            "name": "Profile",
            "type": "select",
            "currentValue": "safe",
            "options": [{
                "group": "recommended",
                "name": "Recommended",
                "options": [{"value": "safe", "name": "Safe"}],
                "futureGroupField": true
            }]
        }
    ]);
    let restored = validate_session_response(
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "sessionId": "session-1",
                "modes": modes.clone(),
                "configOptions": config_options.clone(),
                "futureResultField": true
            }
        }),
        2,
        AcpSessionMethod::Load("session-1"),
    )
    .unwrap();
    assert_eq!(restored.session_id.as_deref(), Some("session-1"));
    assert_eq!(restored.modes, Some(modes));
    assert_eq!(
        restored.config_options,
        config_options.as_array().unwrap().clone()
    );
}

#[test]
fn load_response_rejects_malformed_optional_state_fields() {
    let malformed_results = [
        json!({"sessionId": 7}),
        json!({"modes": []}),
        json!({"configOptions": {}}),
    ];

    for result in malformed_results {
        let response = validate_session_response(
            &json!({"jsonrpc": "2.0", "id": 2, "result": result}),
            2,
            AcpSessionMethod::Load("session-1"),
        );
        assert_eq!(response.unwrap_err(), AcpError::SessionResponseInvalid);
    }
}

#[test]
fn load_response_rejects_malformed_nested_mode_state() {
    let malformed_modes = [
        json!({"availableModes": []}),
        json!({"currentModeId": 7, "availableModes": []}),
        json!({"currentModeId": "", "availableModes": []}),
        json!({"currentModeId": " review ", "availableModes": []}),
        json!({"currentModeId": "x".repeat(1025), "availableModes": []}),
        json!({"currentModeId": "review"}),
        json!({"currentModeId": "review", "availableModes": {}}),
        json!({"currentModeId": "review", "availableModes": [{}]}),
        json!({"currentModeId": "review", "availableModes": [{"id": "review"}]}),
        json!({
            "currentModeId": "review",
            "availableModes": [{"id": 7, "name": "Review"}]
        }),
        json!({
            "currentModeId": "review",
            "availableModes": [{"id": "review", "name": []}]
        }),
        json!({
            "currentModeId": "review",
            "availableModes": [{"id": " review ", "name": "Review"}]
        }),
    ];

    for modes in malformed_modes {
        let response = validate_session_response(
            &json!({"jsonrpc": "2.0", "id": 2, "result": {"modes": modes}}),
            2,
            AcpSessionMethod::Load("session-1"),
        );
        assert_eq!(response.unwrap_err(), AcpError::SessionResponseInvalid);
    }
}

#[test]
fn load_response_rejects_malformed_nested_config_options() {
    let malformed_options = [
        json!([{}]),
        json!([{"id": "pace", "name": "Pace"}]),
        json!([{"id": "pace", "name": "Pace", "type": 7, "currentValue": true}]),
        json!([{"id": 7, "name": "Pace", "type": "boolean", "currentValue": true}]),
        json!([{"id": "x".repeat(1025), "name": "Pace", "type": "boolean", "currentValue": true}]),
        json!([{"id": "pace", "name": [], "type": "boolean", "currentValue": true}]),
        json!([{"id": " pace ", "name": "Pace", "type": "boolean", "currentValue": true}]),
        json!([{"id": "pace", "name": "Pace", "type": "future", "currentValue": true}]),
        json!([{"id": "pace", "name": "Pace", "type": "boolean"}]),
        json!([{"id": "pace", "name": "Pace", "type": "boolean", "currentValue": "true"}]),
        json!([{"id": "pace", "name": "Pace", "type": "select", "options": []}]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": " steady ", "options": []
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": {}
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{}]
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{"value": "steady"}]
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{"name": "Steady"}]
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{"value": 7, "name": "Steady"}]
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{"value": " steady ", "name": "Steady"}]
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{
                "group": "normal", "name": "Normal", "options": [{}]
            }]
        }]),
        json!([{
            "id": "pace", "name": "Pace", "type": "select",
            "currentValue": "steady", "options": [{
                "group": 7, "name": "Normal", "options": []
            }]
        }]),
    ];

    for config_options in malformed_options {
        let response = validate_session_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {"configOptions": config_options}
            }),
            2,
            AcpSessionMethod::Load("session-1"),
        );
        assert_eq!(response.unwrap_err(), AcpError::SessionResponseInvalid);
    }
}

#[test]
fn prompt_response_is_strict_and_remote_errors_are_redacted() {
    let response = validate_prompt_response(
        &json!({"jsonrpc": "2.0", "id": 3, "result": {"stopReason": "end_turn"}}),
        3,
    )
    .unwrap();
    assert_eq!(response.stop_reason, AcpStopReason::EndTurn);

    let remote = validate_prompt_response(
        &json!({
            "jsonrpc": "2.0", "id": 3,
            "error": {"code": -32000, "message": "sensitive runtime detail"}
        }),
        3,
    )
    .unwrap_err();
    assert_eq!(remote, AcpError::RemoteError { code: -32000 });
    assert_eq!(remote.to_string(), "acp_remote_error");
}

#[test]
fn close_response_requires_an_empty_result_object() {
    validate_close_session_response(&json!({"jsonrpc": "2.0", "id": 4, "result": {}}), 4).unwrap();
    assert_eq!(
        validate_close_session_response(&json!({"jsonrpc": "2.0", "id": 4, "result": null}), 4,)
            .unwrap_err(),
        AcpError::CloseResponseInvalid
    );
    assert_eq!(
        validate_close_session_response(
            &json!({"jsonrpc": "2.0", "id": 4, "result": {"closed": true}}),
            4,
        )
        .unwrap_err(),
        AcpError::CloseResponseInvalid
    );
}

#[test]
fn session_update_validates_notification_shape_and_session_association() {
    let message = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "session-1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "hello"}
            }
        }
    });
    let update = validate_session_update(&message, Some("session-1")).unwrap();
    assert_eq!(update.kind, AcpSessionUpdateKind::AgentMessageChunk);
    assert_eq!(update.agent_message_text(), Some("hello"));
    assert_eq!(
        validate_session_update(&message, Some("other-session")).unwrap_err(),
        AcpError::SessionMismatch
    );

    let mut unstable = message.clone();
    unstable["params"]["update"]["sessionUpdate"] = json!("plan_update");
    assert_eq!(
        validate_session_update(&unstable, Some("session-1")).unwrap_err(),
        AcpError::SessionUpdateInvalid
    );
    let mut request_shaped = message;
    request_shaped["id"] = json!(1);
    assert_eq!(
        validate_session_update(&request_shaped, Some("session-1")).unwrap_err(),
        AcpError::NotificationEnvelopeInvalid
    );
}

#[test]
fn session_update_kind_exposes_only_lifecycle_evidence_classification() {
    assert_eq!(
        AcpSessionUpdateKind::AgentThoughtChunk.processing_evidence_kind(),
        Some("reasoning")
    );
    assert_eq!(
        AcpSessionUpdateKind::ToolCall.processing_evidence_kind(),
        Some("tool")
    );
    assert_eq!(
        AcpSessionUpdateKind::Plan.processing_evidence_kind(),
        Some("plan")
    );
    assert_eq!(
        AcpSessionUpdateKind::AgentMessageChunk.processing_evidence_kind(),
        None
    );
}

fn session_update_notification(update: serde_json::Value) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "session-1",
            "update": update
        }
    })
}

fn message_chunk_notification(kind: &str, content: serde_json::Value) -> serde_json::Value {
    session_update_notification(json!({
        "sessionUpdate": kind,
        "content": content,
        "futureUpdateField": {"preserved": true}
    }))
}

#[test]
fn message_chunk_content_accepts_supported_acp_blocks_and_extension_fields() {
    let supported_blocks = [
        json!({"type": "text", "text": "hello", "futureContentField": true}),
        json!({
            "type": "image",
            "data": "aW1hZ2U=",
            "mimeType": "image/png",
            "futureContentField": true
        }),
        json!({
            "type": "audio",
            "data": "YXVkaW8=",
            "mimeType": "audio/wav",
            "futureContentField": true
        }),
        json!({
            "type": "resource_link",
            "uri": "file:///fixture.txt",
            "name": "fixture.txt",
            "futureContentField": true
        }),
        json!({
            "type": "resource",
            "resource": {
                "uri": "file:///fixture.txt",
                "text": "fixture",
                "futureResourceField": true
            },
            "futureContentField": true
        }),
        json!({
            "type": "resource",
            "resource": {
                "uri": "file:///fixture.bin",
                "blob": "Zml4dHVyZQ==",
                "futureResourceField": true
            },
            "futureContentField": true
        }),
    ];

    for kind in [
        "user_message_chunk",
        "agent_message_chunk",
        "agent_thought_chunk",
    ] {
        for content in &supported_blocks {
            let message = message_chunk_notification(kind, content.clone());
            let update = validate_session_update(&message, Some("session-1")).unwrap();
            assert_eq!(update.payload()["content"], *content);
        }
    }
}

#[test]
fn message_chunk_content_rejects_missing_unknown_and_malformed_required_fields() {
    const MALFORMED_CONTENT_CANARY: &str = "MALFORMED-CONTENT-CANARY";
    let malformed_blocks = [
        json!({}),
        json!({"text": MALFORMED_CONTENT_CANARY}),
        json!({"type": "future_content", "text": MALFORMED_CONTENT_CANARY}),
        json!({"type": "text"}),
        json!({"type": "text", "text": {"canary": MALFORMED_CONTENT_CANARY}}),
        json!({"type": "image", "data": "aW1hZ2U="}),
        json!({"type": "image", "data": 7, "mimeType": "image/png"}),
        json!({"type": "image", "data": "aW1hZ2U=", "mimeType": 7}),
        json!({"type": "audio", "mimeType": "audio/wav"}),
        json!({"type": "audio", "data": 7, "mimeType": "audio/wav"}),
        json!({"type": "resource_link", "uri": "file:///fixture.txt"}),
        json!({"type": "resource_link", "uri": 7, "name": "fixture.txt"}),
        json!({"type": "resource", "resource": {}}),
        json!({"type": "resource", "resource": {"uri": 7, "text": "fixture"}}),
        json!({"type": "resource", "resource": {"uri": "file:///fixture", "text": 7}}),
    ];

    for content in malformed_blocks {
        let message = message_chunk_notification("agent_message_chunk", content);
        assert_eq!(
            validate_session_update(&message, Some("session-1")).unwrap_err(),
            AcpError::SessionUpdateInvalid
        );
    }

    let oversized = message_chunk_notification(
        "agent_message_chunk",
        json!({"type": "text", "text": "x".repeat(DEFAULT_MAX_MESSAGE_BYTES)}),
    );
    assert_eq!(
        validate_session_update(&oversized, Some("session-1")).unwrap_err(),
        AcpError::MessageTooLarge
    );
}

#[test]
fn structured_session_updates_accept_complete_acp_objects_and_extensions() {
    let valid_updates = [
        (
            AcpSessionUpdateKind::ToolCall,
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "tool-1",
                "title": "Inspect fixture",
                "kind": "read",
                "status": "in_progress",
                "content": [{
                    "type": "content",
                    "content": {"type": "text", "text": "synthetic output"},
                    "futureContentField": true
                }, {
                    "type": "terminal",
                    "terminalId": "terminal-1",
                    "futureTerminalField": true
                }, {
                    "type": "diff",
                    "path": "/fixture/project/lib.rs",
                    "oldText": "before",
                    "newText": "after",
                    "futureDiffField": true
                }],
                "locations": [{"path": "/fixture/project/lib.rs", "line": 7}],
                "futureToolField": {"preserved": true}
            }),
        ),
        (
            AcpSessionUpdateKind::ToolCallUpdate,
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tool-1",
                "title": "Inspect fixture complete",
                "status": "completed",
                "content": [{
                    "type": "content",
                    "content": {"type": "text", "text": "complete"}
                }],
                "futureToolUpdateField": true
            }),
        ),
        (
            AcpSessionUpdateKind::Plan,
            json!({
                "sessionUpdate": "plan",
                "entries": [{
                    "content": "Validate the fixture",
                    "priority": "high",
                    "status": "in_progress",
                    "futurePlanEntryField": true
                }],
                "futurePlanField": true
            }),
        ),
        (
            AcpSessionUpdateKind::AvailableCommandsUpdate,
            json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": [{
                    "name": "review",
                    "description": "Review a synthetic fixture",
                    "input": {"hint": "fixture path", "futureInputField": true},
                    "futureCommandField": true
                }],
                "futureCommandsField": true
            }),
        ),
        (
            AcpSessionUpdateKind::CurrentModeUpdate,
            json!({
                "sessionUpdate": "current_mode_update",
                "currentModeId": "review",
                "futureModeField": true
            }),
        ),
        (
            AcpSessionUpdateKind::ConfigOptionUpdate,
            json!({
                "sessionUpdate": "config_option_update",
                "configOptions": [{
                    "id": "guarded",
                    "name": "Guarded",
                    "type": "boolean",
                    "currentValue": true,
                    "futureConfigField": true
                }],
                "futureConfigUpdateField": true
            }),
        ),
        (
            AcpSessionUpdateKind::SessionInfoUpdate,
            json!({
                "sessionUpdate": "session_info_update",
                "title": "Synthetic session",
                "updatedAt": "2026-01-01T00:00:00Z",
                "futureSessionInfoField": true
            }),
        ),
        (
            AcpSessionUpdateKind::UsageUpdate,
            json!({
                "sessionUpdate": "usage_update",
                "used": 4,
                "size": 128,
                "cost": {"amount": 0.25, "currency": "USD", "futureCostField": true},
                "futureUsageField": true
            }),
        ),
    ];

    for (kind, payload) in valid_updates {
        let message = session_update_notification(payload.clone());
        let update = validate_session_update(&message, Some("session-1")).unwrap();
        assert_eq!(update.kind, kind);
        assert_eq!(update.payload(), &payload);
    }
}

#[test]
fn structured_session_updates_reject_missing_or_malformed_required_fields() {
    let malformed_updates = [
        json!({"sessionUpdate": "tool_call", "title": "Inspect fixture"}),
        json!({"sessionUpdate": "tool_call", "toolCallId": 7, "title": "Inspect fixture"}),
        json!({"sessionUpdate": "tool_call", "toolCallId": "tool-1"}),
        json!({"sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": []}),
        json!({
            "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Inspect",
            "content": [{"type": "content"}]
        }),
        json!({
            "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Inspect",
            "content": [{"type": "terminal"}]
        }),
        json!({
            "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Inspect",
            "content": [{"type": "diff", "path": "/fixture/project/lib.rs"}]
        }),
        json!({
            "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Inspect",
            "locations": [{"path": 7}]
        }),
        json!({"sessionUpdate": "tool_call_update"}),
        json!({"sessionUpdate": "tool_call_update", "toolCallId": {}}),
        json!({"sessionUpdate": "tool_call_update", "toolCallId": " tool-1 "}),
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": 7}),
        json!({"sessionUpdate": "plan"}),
        json!({"sessionUpdate": "plan", "entries": {}}),
        json!({"sessionUpdate": "plan", "entries": [{}]}),
        json!({
            "sessionUpdate": "plan",
            "entries": [{"content": "Validate", "priority": "urgent", "status": "pending"}]
        }),
        json!({
            "sessionUpdate": "plan",
            "entries": [{"content": "Validate", "priority": "high", "status": "running"}]
        }),
        json!({"sessionUpdate": "available_commands_update"}),
        json!({"sessionUpdate": "available_commands_update", "availableCommands": {}}),
        json!({"sessionUpdate": "available_commands_update", "availableCommands": [{}]}),
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "review"}]
        }),
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": 7, "description": "Review"}]
        }),
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "review", "description": "Review", "input": {}}]
        }),
        json!({"sessionUpdate": "current_mode_update"}),
        json!({"sessionUpdate": "current_mode_update", "currentModeId": 7}),
        json!({"sessionUpdate": "current_mode_update", "currentModeId": " review "}),
        json!({"sessionUpdate": "config_option_update"}),
        json!({"sessionUpdate": "config_option_update", "configOptions": {}}),
        json!({"sessionUpdate": "config_option_update", "configOptions": [{}]}),
        json!({"sessionUpdate": "session_info_update", "title": 7}),
        json!({"sessionUpdate": "session_info_update", "updatedAt": []}),
        json!({"sessionUpdate": "session_info_update", "_meta": []}),
        json!({"sessionUpdate": "usage_update", "size": 128}),
        json!({"sessionUpdate": "usage_update", "used": 4}),
        json!({"sessionUpdate": "usage_update", "used": -1, "size": 128}),
        json!({"sessionUpdate": "usage_update", "used": 4, "size": 1.5}),
        json!({
            "sessionUpdate": "usage_update", "used": 4, "size": 128,
            "cost": {"amount": "0.25", "currency": "USD"}
        }),
        json!({
            "sessionUpdate": "usage_update", "used": 4, "size": 128,
            "cost": {"amount": 0.25}
        }),
    ];

    for payload in malformed_updates {
        let message = session_update_notification(payload);
        assert_eq!(
            validate_session_update(&message, Some("session-1")).unwrap_err(),
            AcpError::SessionUpdateInvalid
        );
    }
}

#[test]
fn available_commands_update_tolerates_untrimmed_vendor_display_text() {
    // Real Copilot advertises third-party skill descriptions that are not
    // whitespace-normalized; display text must not fail the protocol.
    let message = session_update_notification(json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": [{
            "name": "ppt-master",
            "description": " AI-driven presentation workflow. ",
            "input": {"hint": " deck file "}
        }]
    }));
    let update = validate_session_update(&message, Some("session-1")).unwrap();
    assert_eq!(update.kind, AcpSessionUpdateKind::AvailableCommandsUpdate);

    for payload in [
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": " ", "description": "Review"}]
        }),
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "review", "description": ""}]
        }),
    ] {
        let message = session_update_notification(payload);
        assert_eq!(
            validate_session_update(&message, Some("session-1")).unwrap_err(),
            AcpError::SessionUpdateInvalid
        );
    }
}

#[test]
fn available_commands_update_tolerates_multi_kilobyte_vendor_descriptions() {
    // Agent command catalogues can advertise skill catalogue entries whose descriptions
    // exceed the session-identifier bound; display text has its own bound.
    let description = "x".repeat(2048);
    let message = session_update_notification(json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": [{"name": "skill", "description": description}]
    }));
    let update = validate_session_update(&message, Some("session-1")).unwrap();
    assert_eq!(update.kind, AcpSessionUpdateKind::AvailableCommandsUpdate);

    let oversized = "x".repeat(9000);
    let message = session_update_notification(json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": [{"name": "skill", "description": oversized}]
    }));
    assert_eq!(
        validate_session_update(&message, Some("session-1")).unwrap_err(),
        AcpError::SessionUpdateInvalid
    );
}

#[test]
fn select_config_options_tolerate_empty_vendor_unset_values() {
    // Real Copilot advertises a select whose current value and default entry
    // value are empty strings for "unset/default".
    let option = |current: &str, value: &str| {
        json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [{
                "id": "agent", "name": "Agent", "type": "select",
                "currentValue": current,
                "options": [{"value": value, "name": "Default"}]
            }]
        })
    };
    let message = session_update_notification(option("", ""));
    let update = validate_session_update(&message, Some("session-1")).unwrap();
    assert_eq!(update.kind, AcpSessionUpdateKind::ConfigOptionUpdate);

    for bad in [option(" ", ""), option("", " ")] {
        let message = session_update_notification(bad);
        assert_eq!(
            validate_session_update(&message, Some("session-1")).unwrap_err(),
            AcpError::SessionUpdateInvalid
        );
    }
}
