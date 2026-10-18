use super::super::generated_context::{
    background_context_prompt_text, extract_user_authored_text, extract_user_image_attachments,
    generated_control_text, strip_generated_context_blocks,
};

#[test]
fn generated_context_blocks_are_removed_without_dropping_trailing_user_text() {
    let text = "<environment_context>\nprivate context\n</environment_context>Keep this\nUser line";
    assert_eq!(strip_generated_context_blocks(text), "Keep this\nUser line");
    assert_eq!(
        extract_user_authored_text("## My request for Codex:\nDo the bounded change"),
        "\nDo the bounded change"
    );
    assert_eq!(
        extract_user_authored_text("## My request:\nRender the screenshot"),
        "\nRender the screenshot"
    );
    assert_eq!(
        extract_user_authored_text(
            "<timestamp>Saturday</timestamp>\n<userquery>Keep the real question</userquery>"
        ),
        "Keep the real question"
    );
}

#[test]
fn cursor_userquery_wrappers_project_inner_text_only() {
    assert_eq!(
        extract_user_authored_text("<userquery>Keep the real question</userquery>"),
        "Keep the real question"
    );
    assert_eq!(
        extract_user_authored_text(
            "<user_info>synthetic host facts</user_info>\n<userquery>\nKeep the real question\n</userquery>"
        ),
        "Keep the real question"
    );
    let inner = extract_user_authored_text("<USERQUERY>Keep the real question</USERQUERY>");
    assert_eq!(inner, "Keep the real question");
    assert!(!inner.contains("userquery"));
}

#[test]
fn cursor_userquery_missing_close_fails_closed_without_raw_tags() {
    let visible = extract_user_authored_text("<userquery>Keep the visible question");
    assert_eq!(visible.trim(), "Keep the visible question");
    assert!(!visible.contains("<userquery>"));
    assert!(!visible.contains("</userquery>"));
}

#[test]
fn generated_image_wrapper_projects_a_typed_local_attachment() {
    let text = "# Files mentioned by the user:\n\n## screenshot.png: /fixture-root/screenshot.png\n\n## My request:\nRender this image\n<image name=[Image #1] path=\"/fixture-root/screenshot.png\">\nprivate image metadata\n</image>";
    let images = extract_user_image_attachments(text);

    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["mediaType"], "image/png");
    assert_eq!(images[0]["path"], "/fixture-root/screenshot.png");
    assert_eq!(images[0]["name"], "screenshot.png");
}

#[test]
fn generated_control_and_background_prompts_fail_closed() {
    assert!(generated_control_text(
        "<local-command-output>secret</local-command-output>"
    ));
    assert!(background_context_prompt_text(
        "# AGENTS.md instructions\nprivate rules"
    ));
    assert!(!generated_control_text("User-authored message"));
}
