use anyhow::Result;
use licoup_conversation::store::{
    ConversationStore, NativeExecutionReference, NativeExecutionReferenceIndex,
};
use serde_json::Value;

pub(super) fn annotate_selected_session(agent_id: &str, session: &mut Value) -> Result<()> {
    // Existing history tests must never open the developer's Canonical store.
    #[cfg(test)]
    if licoup_foundation::platform::paths::portable_data_dir_override_path().is_none() {
        return Ok(());
    }
    let Some(native_session_id) = session
        .get("nativeSessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(());
    };
    let root = licoup_foundation::platform::paths::portable_data_dir_read_only()?;
    let references =
        ConversationStore::native_execution_references(&root, agent_id, native_session_id)?;
    annotate_messages(session, &references);
    Ok(())
}

fn annotate_messages(session: &mut Value, references: &NativeExecutionReferenceIndex) {
    let Some(messages) = session.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if !matches!(
            message.get("role").and_then(Value::as_str),
            Some("agent" | "assistant")
        ) {
            continue;
        }
        let mut matched: Option<&NativeExecutionReference> = None;
        let mut ambiguous = false;
        for (kind, field) in [("turn", "sourceTurnId"), ("message", "sourceMessageId")] {
            let Some(key) = message.get(field).and_then(Value::as_str) else {
                continue;
            };
            match references.get(&(kind.to_owned(), key.to_owned())) {
                Some(Some(reference)) if matched.is_none_or(|known| known == reference) => {
                    matched = Some(reference)
                }
                Some(_) => {
                    ambiguous = true;
                    break;
                }
                None => {}
            }
        }
        if !ambiguous && let Some(reference) = matched {
            message["executionReference"] =
                serde_json::to_value(reference).expect("execution reference serializes");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn execution_provenance_hydrates_only_exact_codex_provider_turn_keys() {
        let root =
            std::env::temp_dir().join(format!("lico-history-provenance-{}", uuid::Uuid::new_v4()));
        let history = root.join(".codex/sessions/2026/09/13");
        std::fs::create_dir_all(&history).unwrap();
        struct Override(Option<std::path::PathBuf>);
        impl Drop for Override {
            fn drop(&mut self) {
                licoup_foundation::platform::paths::set_portable_data_dir_override(self.0.take());
            }
        }
        let _guard = Override(
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone())),
        );
        let store = ConversationStore::open(&root).unwrap();
        let mut expected = Vec::new();
        for turn_id in ["native-turn-one", "native-turn-two"] {
            let scope = store
                .prepare_runtime_dispatch(
                    "codex",
                    "native-session",
                    "same request",
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            store
                .record_native_execution_provenance(
                    &scope,
                    "codex",
                    "native-session",
                    Some(turn_id),
                    None,
                )
                .unwrap();
            expected.push(json!({"turnHandle":scope.dispatch_id,"conversationId":scope.conversation_id,"membershipId":scope.membership_id}));
        }
        let records = [
            json!({"type":"session_meta","payload":{"id":"native-session"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"same request"}]}}),
            json!({"type":"turn_context","payload":{"turn_id":"native-turn-one"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","id":"reply-one","content":[{"type":"output_text","text":"same answer"}]}}),
            json!({"type":"event_msg","payload":{"type":"task_complete"}}),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"native-turn-two"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","id":"reply-two","content":[{"type":"output_text","text":"same answer"}]}}),
            json!({"type":"event_msg","payload":{"type":"task_complete"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","id":"unbound","content":[{"type":"output_text","text":"same answer"}]}}),
        ];
        std::fs::write(
            history.join("rollout-2026-09-13T00-00-00-native-session.jsonl"),
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let listed = super::super::query::conversation_list(
            &json!({"agent":"codex","homeDir":root,"sessionId":"native-session"}),
        )
        .unwrap();
        let messages = listed["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "agent")
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["executionReference"], expected[0]);
        assert_eq!(messages[1]["executionReference"], expected[1]);
        assert_eq!(messages[0]["sourceMessageId"], "reply-one");
        assert!(messages[2].get("executionReference").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_provenance_conflicting_provider_keys_remain_unbound() {
        let reference = |id: &str| NativeExecutionReference {
            conversation_id: "conversation".to_owned(),
            membership_id: "member".to_owned(),
            turn_handle: id.to_owned(),
        };
        let references = NativeExecutionReferenceIndex::from([
            (
                ("turn".to_owned(), "turn".to_owned()),
                Some(reference("first")),
            ),
            (
                ("message".to_owned(), "message".to_owned()),
                Some(reference("second")),
            ),
            (("turn".to_owned(), "ambiguous".to_owned()), None),
        ]);
        let mut session = json!({"messages":[
            {"role":"agent","sourceTurnId":"turn","sourceMessageId":"message"},
            {"role":"agent","sourceTurnId":"ambiguous"},
            {"role":"user","sourceTurnId":"turn"},
            {"role":"agent","id":"turn","text":"turn"}
        ]});
        annotate_messages(&mut session, &references);
        assert!(
            session["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message.get("executionReference").is_none())
        );
    }
}
