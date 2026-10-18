use licoup_conversation::{
    ConversationStore, DispatchState, EventKind, EventPartKind, MembershipAccess, NewEventPart,
    Principal, PrincipalKind, SubagentDispatchClaimState,
};
use rusqlite::params;
use serde_json::json;

#[test]
fn cold_recovery_kill_mid_turn_rebuilds_store_without_orphan_inflight_state() {
    let root = std::env::temp_dir().join(format!(
        "lico-conversation-cold-recovery-{}",
        uuid::Uuid::new_v4()
    ));
    let scope = {
        let host = ConversationStore::open(&root).unwrap();
        let scope = host
            .prepare_runtime_dispatch(
                "fixture-agent",
                "",
                "persist this turn",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        host.append_runtime_frame(
            &scope,
            1,
            &json!({"type": "agent.message.chunk", "delta": "partial"}),
        )
        .unwrap();
        assert_eq!(
            host.dispatch_record(&scope.dispatch_id)
                .unwrap()
                .unwrap()
                .state,
            DispatchState::Running
        );
        // Dropping the only host models an abrupt process loss: no in-memory
        // value is handed to the next host.
        scope
    };

    let reopened = ConversationStore::open(&root).unwrap();
    let dispatch = reopened
        .dispatch_record(&scope.dispatch_id)
        .unwrap()
        .unwrap();
    assert_eq!(dispatch.state, DispatchState::Failed);
    assert_eq!(
        dispatch.error_code.as_deref(),
        Some("host_lifecycle_interrupted")
    );
    let events = reopened
        .page_events(&scope.conversation_id, None, 100)
        .unwrap()
        .events;
    let runtime_event = events
        .iter()
        .find(|event| event.id == scope.event_id)
        .unwrap();
    assert!(runtime_event.finalized);
    assert!(
        runtime_event
            .parts
            .iter()
            .any(|part| { part.content.contains("host_lifecycle_interrupted") })
    );
    assert_eq!(reopened.cold_recover().unwrap().recovered_dispatches, 0);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cold_recovery_finalizes_orphan_unfinalized_message_without_inflight_dispatch() {
    let root = std::env::temp_dir().join(format!(
        "lico-conversation-cold-orphan-{}",
        uuid::Uuid::new_v4()
    ));
    let (conversation_id, event_id) = {
        let host = ConversationStore::open(&root).unwrap();
        let conversation = host
            .create_conversation(
                "Project",
                Principal {
                    id: "human:local".into(),
                    kind: PrincipalKind::Human,
                    display_name: "You".into(),
                    agent_id: None,
                    created_at_unix_ms: 1,
                },
            )
            .unwrap();
        let agent = host
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let event = host
            .append_event(
                &conversation.id,
                Some(&agent.id),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "吻合".into(),
                }],
                None,
                None,
                false,
            )
            .unwrap();
        assert!(!event.finalized);
        (conversation.id, event.id)
    };

    let reopened = ConversationStore::open(&root).unwrap();
    let event = reopened
        .page_events(&conversation_id, None, 20)
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.id == event_id)
        .unwrap();
    assert!(event.finalized);
    assert!(
        event
            .parts
            .iter()
            .any(|part| part.kind == EventPartKind::Diagnostic
                && part.content.contains("host_lifecycle_interrupted"))
    );
    assert_eq!(reopened.cold_recover().unwrap().finalized_events, 0);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cold_recovery_finalizes_event_when_dispatch_already_terminal() {
    let root = std::env::temp_dir().join(format!(
        "lico-conversation-cold-terminal-{}",
        uuid::Uuid::new_v4()
    ));
    let scope = {
        let host = ConversationStore::open(&root).unwrap();
        let scope = host
            .prepare_runtime_dispatch(
                "fixture-agent",
                "",
                "persist this turn",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        host.append_runtime_frame(
            &scope,
            1,
            &json!({"event": "agent.message.chunk", "payload": {"text": "吻合"}}),
        )
        .unwrap();
        let dispatch_id = scope.dispatch_id.clone();
        drop(host);
        let database = root.join("client-state/conversations/conversations.sqlite3");
        let connection = rusqlite::Connection::open(database).unwrap();
        connection
            .execute(
                "UPDATE conversation_dispatches SET state='completed' WHERE id=?1",
                params![dispatch_id],
            )
            .unwrap();
        connection.close().unwrap();
        scope
    };

    let reopened = ConversationStore::open(&root).unwrap();
    let dispatch = reopened
        .dispatch_record(&scope.dispatch_id)
        .unwrap()
        .unwrap();
    assert_eq!(dispatch.state, DispatchState::Completed);
    let runtime_event = reopened
        .page_events(&scope.conversation_id, None, 100)
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.id == scope.event_id)
        .unwrap();
    assert!(runtime_event.finalized);
    assert!(
        runtime_event
            .parts
            .iter()
            .any(|part| { part.content.contains("host_lifecycle_interrupted") })
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cold_recovery_settles_a_running_claim_when_dispatch_is_interrupted() {
    let root = std::env::temp_dir().join(format!(
        "lico-conversation-cold-claim-{}",
        uuid::Uuid::new_v4()
    ));
    let claim_id = {
        let host = ConversationStore::open(&root).unwrap();
        let owner = Principal {
            id: "human:owner".into(),
            kind: PrincipalKind::Human,
            display_name: "Owner".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        };
        let members = ["caller-agent", "target-agent"].map(|agent_id| {
            (
                Principal {
                    id: format!("agent:{agent_id}"),
                    kind: PrincipalKind::Agent,
                    display_name: agent_id.into(),
                    agent_id: Some(agent_id.into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
        });
        let conversation = host
            .create_conversation_with_members("Subagent Recover", owner, &members)
            .unwrap();
        let membership = |agent_id: &str| {
            conversation
                .memberships
                .iter()
                .find(|membership| membership.principal.agent_id.as_deref() == Some(agent_id))
                .unwrap()
                .id
                .clone()
        };
        let claim = host
            .claim_subagent_dispatch(
                &conversation.id,
                &membership("caller-agent"),
                &membership("target-agent"),
                None,
            )
            .unwrap();
        host.update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Running)
            .unwrap();
        host.prepare_runtime_dispatch(
            "target-agent",
            "native-target",
            "delegated prompt",
            Some(&conversation.id),
            Some(&membership("target-agent")),
            Some("subagent-mcp"),
            Some(&claim.id),
        )
        .unwrap();
        claim.id
    };

    let reopened = ConversationStore::open(&root).unwrap();
    let claim = reopened.subagent_claim(&claim_id).unwrap().unwrap();
    assert_eq!(claim.state, SubagentDispatchClaimState::Failed);
    let dispatch = reopened.dispatch_record(&claim_id).unwrap().unwrap();
    assert_eq!(dispatch.state, DispatchState::Failed);

    let _ = std::fs::remove_dir_all(root);
}
