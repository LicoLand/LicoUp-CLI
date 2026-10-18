use super::*;

#[test]
fn empty_prompt_fails_closed() {
    let failure =
        ProtocolConfig::from_params(&json!({}), "   ", "", Some(absolute_test_cwd().as_path()))
            .unwrap_err();
    assert_eq!(failure.code, "pi_empty_prompt");
}

#[test]
fn unsupported_turn_overrides_and_invalid_thinking_fail_before_launch() {
    for (params, expected) in [
        (
            json!({"sandbox": "workspace"}),
            "pi_sandbox_override_unsupported",
        ),
        (
            json!({"approvalPolicy": "never"}),
            "pi_approval_override_unsupported",
        ),
        (
            json!({"thinking": "unbounded"}),
            "pi_invalid_thinking_level",
        ),
        (
            json!({"privateInstructions": "synthetic-private-instruction"}),
            "pi_private_instructions_unsupported",
        ),
    ] {
        let failure =
            ProtocolConfig::from_params(&params, "hello", "", Some(absolute_test_cwd().as_path()))
                .unwrap_err();
        assert_eq!(failure.code, expected);
    }
}

#[test]
fn private_instructions_never_modify_the_exact_user_prompt() {
    let private = "synthetic-private-instruction";
    let failure = ProtocolConfig::from_params(
        &json!({"privateInstructions": private}),
        "exact-user-prompt",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap_err();
    assert_eq!(failure.code, "pi_private_instructions_unsupported");
    assert!(!failure.message.contains(private));
}

#[test]
fn relative_cwd_fails_closed() {
    let failure = ProtocolConfig::from_params(&json!({}), "hello", "", Some(Path::new("relative")))
        .unwrap_err();
    assert_eq!(failure.code, "pi_absolute_cwd_required");
}
