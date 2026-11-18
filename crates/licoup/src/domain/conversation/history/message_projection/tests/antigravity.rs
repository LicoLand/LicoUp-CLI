use super::super::antigravity::{
    extract_user_request, looks_like_artifact_dump, strip_artifact_noise, strip_line_gutter,
    strip_system_messages,
};

#[test]
fn antigravity_keeps_user_request_and_removes_system_protocol_content() {
    let text = "<SYSTEM_MESSAGE>not actually sent by the user</SYSTEM_MESSAGE>\n\
                <USER_REQUEST>Keep this request</USER_REQUEST>";
    assert_eq!(extract_user_request(text), "Keep this request");
    assert!(!strip_system_messages(text).contains("not actually sent"));
}

#[test]
fn antigravity_gutter_policy_preserves_ordered_lists_and_drops_artifact_dumps() {
    assert_eq!(strip_line_gutter("  42 │ visible code"), "  visible code");
    assert_eq!(strip_line_gutter("1. ordered item"), "1. ordered item");
    let dump = [
        "1 │ one",
        "2 │ two",
        "3 │ three",
        "4 │ four",
        "5 │ five",
        "6 │ six",
    ];
    assert!(looks_like_artifact_dump(&dump));
    assert!(strip_artifact_noise(&dump.join("\n")).is_empty());
}
