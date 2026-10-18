use licoup_conversation::{ConversationStore, DispatchState, EventPartKind};
use serde_json::json;

#[test]
fn local_turns_stream_continue_cancel_and_reopen_without_losing_history() {
    let root = std::env::temp_dir().join(format!("local-conversation-{}", uuid::Uuid::new_v4()));
    let store = ConversationStore::open(&root).unwrap();
    let first = store
        .prepare_runtime_dispatch("synthetic", "", "First question", None, None, None, None)
        .unwrap();
    store
        .bind_runtime_session(&first, "synthetic", "session-one", None, None)
        .unwrap();
    store
        .append_runtime_frame(
            &first,
            1,
            &json!({
                "event":"agent.message.chunk", "sessionId":"session-one", "turnId":"turn-one",
                "payload":{"text":"Visible partial", "messageUnit":"answer"}
            }),
        )
        .unwrap();
    assert!(visible_text(&store, &first.conversation_id).contains(&"Visible partial".to_owned()));
    store
        .finish_runtime_dispatch(&first, &json!({"ok":true}), DispatchState::Completed, None)
        .unwrap();

    let cancelled = store
        .prepare_runtime_dispatch(
            "synthetic",
            "session-one",
            "Second question",
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(cancelled.conversation_id, first.conversation_id);
    assert_eq!(cancelled.membership_id, first.membership_id);
    store
        .finish_runtime_dispatch(
            &cancelled,
            &json!({"ok":false,"status":"cancelled"}),
            DispatchState::Cancelled,
            None,
        )
        .unwrap();
    drop(store);

    let reopened = ConversationStore::open(&root).unwrap();
    let third = reopened
        .prepare_runtime_dispatch(
            "synthetic",
            "session-one",
            "After stop and restart",
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(third.conversation_id, first.conversation_id);
    assert_eq!(third.membership_id, first.membership_id);
    reopened
        .append_runtime_frame(
            &third,
            1,
            &json!({
                "event":"agent.message.completed", "sessionId":"session-one", "turnId":"turn-three",
                "payload":{"text":"Continued reply", "messageUnit":"answer"}
            }),
        )
        .unwrap();
    reopened
        .finish_runtime_dispatch(&third, &json!({"ok":true}), DispatchState::Completed, None)
        .unwrap();
    drop(reopened);

    let reopened = ConversationStore::open(&root).unwrap();
    assert_eq!(reopened.list(false).unwrap().len(), 1);
    assert_eq!(
        visible_text(&reopened, &first.conversation_id),
        [
            "First question",
            "Visible partial",
            "Second question",
            "After stop and restart",
            "Continued reply"
        ]
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

fn visible_text(store: &ConversationStore, conversation_id: &str) -> Vec<String> {
    store
        .page_events(conversation_id, None, 100)
        .unwrap()
        .events
        .into_iter()
        .flat_map(|event| event.parts)
        .filter(|part| part.kind == EventPartKind::Text)
        .map(|part| part.content)
        .collect()
}
