use serde_json::{Value, json};

use super::super::delegated_merge::{
    merge_delegated_subagent_sessions, subagent_card_preview_text,
};

fn session(id: &str, parent: Option<&str>, delegated: bool, messages: Vec<Value>) -> Value {
    let message_count = messages.len();
    let mut session = json!({
        "id": id,
        "nativeSessionId": id,
        "sourcePath": format!("fixture/{id}"),
        "messages": messages,
        "messageCount": message_count,
        "delegatedSubagent": delegated,
        "subagentTitle": id
    });
    if let Some(parent) = parent {
        session["parentSessionId"] = json!(parent);
    }
    session
}

#[test]
fn nested_delegated_lineage_merges_leaf_to_root_without_flattening_children() {
    let main = session(
        "main",
        None,
        false,
        vec![json!({"role": "user", "text": "Start", "createdAt": 0})],
    );
    let child = session(
        "child",
        Some("main"),
        true,
        vec![json!({"role": "assistant", "text": "Child result", "createdAt": 2})],
    );
    let grandchild = session(
        "grandchild",
        Some("child"),
        true,
        vec![json!({"role": "assistant", "text": "Nested result", "createdAt": 1})],
    );

    let merged = merge_delegated_subagent_sessions(
        vec![main.clone(), child.clone(), grandchild.clone()],
        None,
    );
    assert_eq!(merged.len(), 1);
    let main_messages = merged[0]["messages"].as_array().unwrap();
    let child_card = main_messages
        .iter()
        .find(|message| message["cardTitle"] == "child")
        .expect("child card");
    assert_eq!(child_card["childSessionId"], "child");
    assert_eq!(child_card["childMessageCount"], 2);
    assert_eq!(child_card["subagentDepth"], 2);
    assert!(child_card["messages"].as_array().unwrap().is_empty());
    let child_read =
        merge_delegated_subagent_sessions(vec![main, child, grandchild], Some("child"));
    let child = child_read
        .iter()
        .find(|session| session["nativeSessionId"] == "child")
        .unwrap();
    let nested = child["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["childSessionId"] == "grandchild")
        .unwrap();
    assert_eq!(nested["childMessageCount"], 1);
    assert!(nested["messages"].as_array().unwrap().is_empty());
}

#[test]
fn running_delegated_task_marks_its_conversation_running() {
    let main = session(
        "main",
        None,
        false,
        vec![json!({"role": "user", "text": "Start", "createdAt": 0})],
    );
    let mut child = session(
        "child",
        Some("main"),
        true,
        vec![json!({"role": "assistant", "text": "Working", "createdAt": 1})],
    );
    child["running"] = json!(true);

    let merged = merge_delegated_subagent_sessions(vec![main, child], None);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["running"], true);
}

#[test]
fn child_source_revision_is_stable_across_sibling_order_and_repeated_folding() {
    let mut main = session("main", None, false, vec![]);
    main["sourceRevision"] = json!("10:100");
    let mut first = session("a", Some("main"), true, vec![]);
    first["sourceRevision"] = json!("20:200");
    let mut second = session("b", Some("main"), true, vec![]);
    second["sourceRevision"] = json!("30:300");
    let merged =
        merge_delegated_subagent_sessions(vec![main.clone(), second.clone(), first.clone()], None);
    let reordered = merge_delegated_subagent_sessions(vec![main, first.clone(), second], None);
    assert_eq!(merged[0]["sourceRevision"], "10:100|20:200|30:300");
    assert_eq!(merged[0]["sourceRevision"], reordered[0]["sourceRevision"]);
    let repeated = merge_delegated_subagent_sessions(vec![merged[0].clone(), first], None);
    assert_eq!(merged[0]["sourceRevision"], repeated[0]["sourceRevision"]);
}

#[test]
fn card_without_a_timestamp_sits_after_the_conversation_flow_not_past_events() {
    let main = session(
        "main",
        None,
        false,
        vec![
            json!({"role": "user", "text": "Start", "createdAt": 0}),
            json!({"role": "agent", "text": "Working", "createdAt": 1}),
            json!({"role": "event", "text": "Runtime trace", "createdAt": 2, "cardType": "event"}),
        ],
    );
    let child = session(
        "child",
        Some("main"),
        true,
        vec![
            json!({"role": "user", "text": "Delegate", "createdAt": ""}),
            json!({"role": "assistant", "text": "Child result", "createdAt": ""}),
        ],
    );

    let merged = merge_delegated_subagent_sessions(vec![main, child], None);
    let main_messages = merged[0]["messages"].as_array().unwrap();
    let card = main_messages
        .iter()
        .find(|message| message["role"] == "subagent")
        .expect("child card");
    assert_eq!(card["text"], "Child result");
    let card_index = main_messages
        .iter()
        .position(|message| message == card)
        .unwrap();
    assert_eq!(main_messages[card_index - 1]["role"], "agent");
    assert_eq!(main_messages[card_index + 1]["role"], "event");
}

#[test]
fn delegated_cycles_preserve_each_identity_without_guessing_a_parent() {
    let main = session(
        "main",
        None,
        false,
        vec![json!({"role": "user", "text": "Start", "createdAt": 0})],
    );
    let left = session(
        "left",
        Some("right"),
        true,
        vec![json!({"role": "assistant", "text": "Left", "createdAt": 1})],
    );
    let right = session(
        "right",
        Some("left"),
        true,
        vec![json!({"role": "assistant", "text": "Right", "createdAt": 2})],
    );
    let merged = merge_delegated_subagent_sessions(vec![main, left, right], None);
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0]["messageCount"], 1);
    assert_eq!(merged[1]["nativeSessionId"], "left");
    assert_eq!(merged[2]["nativeSessionId"], "right");

    let preview = subagent_card_preview_text(&"界".repeat(181));
    assert_eq!(preview.chars().count(), 183);
    assert!(preview.ends_with("..."));
}

#[test]
fn missing_parent_preserves_child_instead_of_attaching_to_a_nearby_session() {
    let main = session(
        "main",
        None,
        false,
        vec![json!({"role": "user", "text": "Start", "createdAt": 1})],
    );
    let child = session(
        "child",
        Some("missing"),
        true,
        vec![json!({"role": "tool", "text": "Result", "createdAt": 1})],
    );
    assert_eq!(
        merge_delegated_subagent_sessions(vec![child.clone()], None),
        vec![child.clone()]
    );
    assert_eq!(
        merge_delegated_subagent_sessions(vec![main.clone(), child.clone()], None),
        vec![main, child]
    );
}

#[test]
fn explicit_siblings_keep_timestamp_order_independent_of_discovery_order() {
    let main = session(
        "main",
        None,
        false,
        vec![json!({"role": "user", "text": "Start", "createdAt": "2026-08-01T00:00:00Z"})],
    );
    let child = |id, at| {
        session(
            id,
            Some("main"),
            true,
            vec![json!({"role": "user", "text": id, "createdAt": at})],
        )
    };
    let merged = merge_delegated_subagent_sessions(
        vec![
            main,
            child("late", "2026-08-01T00:00:02Z"),
            child("early", "2026-08-01T00:00:01Z"),
        ],
        None,
    );
    assert_eq!(merged[0]["messages"][1]["cardTitle"], "early");
    assert_eq!(merged[0]["messages"][2]["cardTitle"], "late");
}
