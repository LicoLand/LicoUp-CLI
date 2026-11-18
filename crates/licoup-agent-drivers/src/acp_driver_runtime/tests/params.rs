use super::*;

#[test]
fn parameters_require_absolute_cwd_and_nonempty_prompt() {
    let relative =
        ProtocolConfig::from_params(&json!({}), "prompt", "", Some(Path::new("relative")))
            .unwrap_err();
    assert_eq!(relative.code, "acp_working_directory_invalid");
    let empty =
        ProtocolConfig::from_params(&json!({}), "", "", Some(absolute_test_cwd().as_path()))
            .unwrap_err();
    assert_eq!(empty.code, "acp_prompt_required");
}

#[test]
fn private_instructions_never_merge_into_the_acp_prompt() {
    let failure = ProtocolConfig::from_params(
        &json!({"privateInstructions": "private-system-canary"}),
        "exact-user-prompt",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap_err();
    assert_eq!(failure.code, "acp_private_instructions_unsupported");
    assert!(!failure.message.contains("canary"));
    assert!(!failure.message.contains("exact-user-prompt"));
}
