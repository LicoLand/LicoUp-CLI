use super::super::support::temp_cli_dir;
use super::*;
use licoup_native::domain::client_conversation::ConversationService;

fn rpc_input(requests: &[Value]) -> Cursor<Vec<u8>> {
    let mut bytes = Vec::new();
    for request in requests {
        serde_json::to_writer(&mut bytes, request).unwrap();
        bytes.push(b'\n');
    }
    Cursor::new(bytes)
}

fn rpc_output(bytes: Vec<u8>) -> Vec<Value> {
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn execute_request(id: &str, workflow_id: &str, args: &[&str]) -> Value {
    json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "method": "execute",
        "args": args,
    })
}

#[test]
fn stdio_rpc_frames_results_errors_and_shutdown() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let input = rpc_input(&[
        execute_request("request-1", "workflow-1", &["ok"]),
        execute_request("request-2", "workflow-1", &["fail"]),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-3",
            "workflowId": "workflow-1",
            "method": "shutdown",
        }),
    ]);
    let output = serve_stdio_rpc(input, Vec::new(), |args, _| match args[0].as_str() {
        "ok" => Ok(licoup_native::ffi::commands::CliExecution::Json(
            json!({"status": "ok"}),
        )),
        _ => Err(anyhow::anyhow!("sensitive internal failure")),
    })
    .unwrap();

    let frames = rpc_output(output);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["result"]["status"], "ok");
    assert_eq!(frames[1]["error"]["code"], "command_failed");
    assert!(!frames[1].to_string().contains("sensitive internal failure"));
    assert_eq!(frames[2]["result"]["status"], "shutdown");
}

#[test]
fn stdio_rpc_rejects_cross_workflow_requests_and_continues() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let input = rpc_input(&[
        execute_request("request-1", "workflow-1", &["ok"]),
        execute_request("request-2", "workflow-2", &["ok"]),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-3",
            "workflowId": "workflow-1",
            "method": "shutdown",
        }),
    ]);
    let output = serve_stdio_rpc(input, Vec::new(), |_, _| {
        Ok(licoup_native::ffi::commands::CliExecution::Json(
            json!({"status": "ok"}),
        ))
    })
    .unwrap();

    let frames = rpc_output(output);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[1]["workflowId"], "workflow-2");
    assert_eq!(frames[1]["error"]["code"], "workflow_mismatch");
    assert_eq!(frames[2]["ok"], true);
}

#[test]
fn stdio_rpc_rejects_streaming_execute_without_invoking_the_command() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let input = rpc_input(&[execute_request(
        "request-1",
        "workflow-1",
        &["conversations", "stream"],
    )]);
    let output = serve_stdio_rpc(input, Vec::new(), |_, _| -> anyhow::Result<_> {
        panic!("streaming execute must not reach the command closure")
    })
    .unwrap();

    let frames = rpc_output(output);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["error"]["code"], "streaming_command_unsupported");
}

#[test]
fn stdio_rpc_rejects_nested_native_calls_without_writing_outside_its_frames() {
    let input = rpc_input(&[execute_request(
        "request-1",
        "workflow-1",
        &["rpc", "call", "catalog.status", "--stdin-json", "{}"],
    )]);
    let output = serve_stdio_rpc(input, Vec::new(), |_, _| -> anyhow::Result<_> {
        panic!("native call must not reach the command closure")
    })
    .unwrap();
    let frames = rpc_output(output);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["error"]["code"], "streaming_command_unsupported");
}

#[test]
fn ordinary_stdio_rpc_rejects_every_conversation_dispatch_entry_point() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let input = rpc_input(&[
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-1",
            "workflowId": "workflow-1",
            "method": "agent.conversation.send",
            "params": {"agent": "synthetic", "text": "private"},
        }),
        execute_request(
            "request-2",
            "workflow-1",
            &["agent", "conversation", "send", "--stdin-json", "{}"],
        ),
        execute_request(
            "request-3",
            "workflow-1",
            &["conversation", "execute", "--stdin-json", "{}"],
        ),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-4",
            "workflowId": "workflow-1",
            "method": "client.conversation.execute",
            "params": {
                "action": "conversation.dispatch.after-post",
                "conversationId": "conversation:group",
                "eventId": "event:1",
                "content": "hi",
                "mentionedMembershipIds": ["membership:agent"]
            },
        }),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-5",
            "workflowId": "workflow-1",
            "method": "strategy.execute",
            "params": {"action": "strategy.run.start"},
        }),
    ]);
    let output = serve_stdio_rpc(input, Vec::new(), |_, _| -> anyhow::Result<_> {
        panic!("conversation dispatch must not reach ordinary stdio execution")
    })
    .unwrap();

    let frames = rpc_output(output);
    assert_eq!(frames.len(), 5);
    for frame in frames {
        assert_eq!(frame["ok"], true);
        assert_eq!(
            frame["result"]["error"]["code"],
            "persistent_conversation_transport_required"
        );
        assert_eq!(frame["result"]["ok"], false);
    }
}

#[test]
fn persistent_conversation_rpc_projects_admission_without_execute_fallback() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let portable = temp_cli_dir("persistent-conversation-admission-rpc");
    let service = ConversationService::open(&portable).unwrap();
    let runtime = PersistentConversationRuntime::new(service.store().clone());
    let input = rpc_input(&[
        execute_request(
            "request-1",
            "workflow-1",
            &[
                "agent",
                "conversation",
                "send",
                "--stdin-json",
                r#"{"invalid""#,
            ],
        ),
        execute_request(
            "request-2",
            "workflow-1",
            &[
                "agent",
                "conversation",
                "stream",
                "--stdin-json",
                r#"{"invalid""#,
            ],
        ),
        execute_request(
            "request-3",
            "workflow-1",
            &[
                "agent",
                "conversation",
                "steer",
                "--stdin-json",
                r#"{"invalid""#,
            ],
        ),
        execute_request(
            "request-4",
            "workflow-1",
            &[
                "agent",
                "conversation",
                "cancel",
                "--stdin-json",
                r#"{"invalid""#,
            ],
        ),
        execute_request(
            "request-5",
            "workflow-1",
            &["agent", "conversation", "send", "--stdin-json", "{}"],
        ),
        execute_request(
            "request-6",
            "workflow-1",
            &["conversation", "execute", "--stdin-json", r#"{"invalid""#],
        ),
        execute_request(
            "request-7",
            "workflow-1",
            &["conversation", "execute", "--stdin-json", "{}"],
        ),
    ]);
    let output = serve_stdio_rpc_with_runtime(
        input,
        Vec::new(),
        |_, _| -> anyhow::Result<_> {
            panic!("legacy execute must never dispatch persistent conversation work")
        },
        runtime,
    )
    .unwrap();

    let frames = rpc_output(output);
    assert_eq!(frames.len(), 7);
    for frame in &frames[..4] {
        assert_eq!(frame["error"]["code"], "cli_json_invalid");
        assert_eq!(frame["error"]["stage"], "cli/admission");
        assert_eq!(frame["error"]["component"], "native_cli");
    }
    assert_eq!(
        frames[4]["result"]["error"]["code"],
        "persistent_conversation_transport_required"
    );
    assert_eq!(frames[5]["error"]["code"], "cli_json_invalid");
    assert_eq!(frames[5]["error"]["stage"], "cli/admission");
    assert_eq!(frames[5]["error"]["component"], "native_cli");
    assert_eq!(
        frames[6]["result"]["error"]["code"],
        "persistent_conversation_transport_required"
    );
    drop(service);
    let _ = fs::remove_dir_all(portable);
}

#[test]
fn stdio_rpc_executes_client_conversation_actions_on_the_bound_portable_root() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let portable = temp_cli_dir("client-conversation-rpc");
    let input = rpc_input(&[
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-1",
            "workflowId": "workflow-1",
            "method": "client.conversation.execute",
            "params": {"action": "conversation.list", "includeArchived": false},
            "portableDataDir": portable.clone(),
        }),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-2",
            "workflowId": "workflow-1",
            "method": "shutdown",
        }),
    ]);
    let output = serve_stdio_rpc(input, Vec::new(), |_, _| {
        panic!("structured client conversation must not reach execute")
    })
    .unwrap();

    let frames = rpc_output(output);
    assert_eq!(frames[0]["ok"], true);
    assert_eq!(frames[0]["result"]["ok"], true);
    let conversations = frames[0]["result"]["result"].as_array().unwrap();
    assert_eq!(
        conversations.len(),
        1,
        "startup pins exactly one canonical Local group"
    );
    assert_eq!(conversations[0]["id"], "lico-group-default");
    assert_eq!(conversations[0]["title"], "Local");
    assert_eq!(conversations[0]["isGroup"], true);
    assert_eq!(conversations[0]["pinned"], true);
    assert_eq!(conversations[0]["archived"], false);
    assert_eq!(conversations[0]["eventCount"], 0);
    assert_eq!(conversations[0]["membershipCount"], 1);
    assert_eq!(conversations[0]["revision"], 1);
    assert!(
        conversations[0].get("updatedAtUnixMs").is_some(),
        "canonical Local projection keeps its recency fact"
    );
    let _ = fs::remove_dir_all(portable);
}

#[test]
fn strategy_definition_actions_do_not_require_persistent_runtime() {
    assert!(!strategy_requires_persistent_runtime(&json!({
        "action": "strategy.definition.list"
    })));
    assert!(!strategy_requires_persistent_runtime(&json!({
        "action": "strategy.definition.inspect"
    })));
    assert!(strategy_requires_persistent_runtime(&json!({
        "action": "strategy.run.start"
    })));
    assert!(!strategy_requires_persistent_runtime(&json!({
        "action": "strategy.run.active"
    })));
    assert!(!strategy_requires_persistent_runtime(&json!({
        "action": "strategy.run.inspect"
    })));
    assert!(!strategy_requires_persistent_runtime(&json!({
        "action": "strategy.run.cancel"
    })));
    assert!(strategy_requires_persistent_runtime(&json!({
        "action": "strategy.run.resume"
    })));
    assert!(strategy_requires_persistent_runtime(&json!({
        "action": "strategy.run.retry"
    })));
}

#[test]
fn persistent_rpc_dispatches_after_post_by_identity_and_returns_the_entry_handle() {
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let portable = temp_cli_dir("persistent-conversation-dispatch-rpc");
    let service = ConversationService::open(&portable).unwrap();
    let created = service
        .execute(json!({
            "action": "conversation.create",
            "title": "Group",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            "members": [
                {"principal": {"id": "agent:synthetic", "kind": "agent", "displayName": "Synthetic", "agentId": "synthetic"}, "access": "member"}
            ]
        }))
        .unwrap();
    let conversation_id = created["id"].as_str().unwrap().to_owned();
    let owner_id = created["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|membership| membership["principal"]["kind"] == "human")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let membership_id = created["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|membership| membership["principal"]["kind"] == "agent")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let runtime = PersistentConversationRuntime::new(service.store().clone());
    let service = bind_conversation_runtime(service, &runtime, None);
    let input = rpc_input(&[
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-1",
            "workflowId": "workflow-1",
            "method": "client.conversation.execute",
            "params": {
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@Synthetic please answer"
            },
        }),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-2",
            "workflowId": "workflow-1",
            "method": "shutdown",
        }),
    ]);
    // Post first so the dispatch can address the stored Event by identity.
    let output = serve_stdio_rpc_with_persistent_conversation(
        input,
        Vec::new(),
        |_, _| -> anyhow::Result<_> {
            panic!("structured client conversation must not reach execute")
        },
        runtime.clone(),
        service.clone(),
    )
    .unwrap();
    let frames = rpc_output(output);
    assert_eq!(frames[0]["ok"], true);
    let event_id = frames[0]["result"]["result"]["event"]["id"]
        .as_str()
        .expect("posted event id")
        .to_owned();

    let dispatch_input = rpc_input(&[
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-3",
            "workflowId": "workflow-1",
            "method": "client.conversation.execute",
            "params": {
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": event_id,
            },
        }),
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": "request-4",
            "workflowId": "workflow-1",
            "method": "shutdown",
        }),
    ]);
    let output = serve_stdio_rpc_with_persistent_conversation(
        dispatch_input,
        Vec::new(),
        |_, _| -> anyhow::Result<_> {
            panic!("structured client conversation must not reach execute")
        },
        runtime.clone(),
        service.clone(),
    )
    .unwrap();
    let frames = rpc_output(output);
    // The dispatch runs on a worker, so the shutdown acknowledgement can be
    // written first; select the dispatch frame by request id.
    let dispatch_frame = frames
        .iter()
        .find(|frame| frame["id"] == "request-3")
        .expect("dispatch response frame");
    assert_eq!(dispatch_frame["ok"], true);
    let result = &dispatch_frame["result"]["result"];
    assert_eq!(result["dispatchPending"], true);
    let turns = result["turns"].as_array().expect("live turns");
    assert_eq!(turns.len(), 1);
    let handle = turns[0]["turnHandle"].as_str().unwrap();
    assert!(!handle.is_empty(), "the returned handle is attachable");
    assert_eq!(turns[0]["membershipId"], membership_id);
    assert_eq!(
        result["directTurns"][0]["id"].as_str().unwrap(),
        handle,
        "the mention turn identity is the dispatch handle"
    );
    assert!(
        dispatch_frame.to_string().contains("synthetic"),
        "native addressing resolved the Membership from the stored Event text"
    );
    // The detached turn settles its unsupported-adapter failure through the
    // completion authority; wait for the registry to go idle before cleanup.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !runtime.idle() {
        assert!(
            std::time::Instant::now() < deadline,
            "the dispatched turn did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(service);
    let _ = fs::remove_dir_all(portable);
}
