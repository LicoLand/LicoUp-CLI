mod catalog;
mod codex;
mod cursor_openagent;
mod generic;
mod kimi;
mod message_projection;
mod pi_copilot;
mod query;
mod session_merge;
mod session_metadata;
mod test_support;

#[test]
fn split_history_module_composition_keeps_the_public_schema() {
    assert_eq!(super::CONVERSATION_SCHEMA_VERSION, 2);
    let facade = include_str!("../../conversations.rs");
    assert!(facade.lines().count() < 100);
    assert!(facade.contains("conversation::history::conversation_list"));
}
