use super::super::semantic::{
    HistoryMessageKind, history_message_kind_from_semantic, normalize_history_message_semantic,
};

#[test]
fn semantic_normalization_and_classification_are_stable() {
    assert_eq!(normalize_history_message_semantic(" Tool_Use "), "tool-use");
    assert_eq!(
        history_message_kind_from_semantic("function_call_output"),
        HistoryMessageKind::ToolResult
    );
    assert_eq!(
        history_message_kind_from_semantic("reasoning-summary"),
        HistoryMessageKind::Reasoning
    );
    assert_eq!(
        history_message_kind_from_semantic("assistant-message"),
        HistoryMessageKind::Text
    );
}
