use super::*;

#[test]
fn fixed_stream_command_keeps_prompt_off_argv_and_resumes_by_session_flag() {
    let prompt = "private prompt must stay off argv";
    let session = "native-session-1";
    let driver_config = config(
        json!({
            "model": "claude-test-model",
            "reasoningEffort": "xhigh",
            "permissionMode": "plan"
        }),
        prompt,
        session,
    );
    let identity = LaunchIdentity::new("claude-test", &driver_config, Some(&absolute_test_cwd()));
    let args = identity.args();
    assert!(FIXED_STREAM_ARGS.contains(&"--include-partial-messages"));
    assert!(!FIXED_STREAM_ARGS.contains(&"--no-session-persistence"));
    // The prompt never leaves the stdin transport; resuming a native
    // conversation requires the CLI's --resume session flag (like Cursor).
    assert!(args.iter().all(|argument| !argument.contains(prompt)));
    let resume_position = args.iter().position(|argument| argument == "--resume");
    assert_eq!(
        resume_position.map(|index| args[index + 1].as_str()),
        Some(session)
    );
    let effort_position = args.iter().position(|argument| argument == "--effort");
    assert_eq!(
        effort_position.map(|index| args[index + 1].as_str()),
        Some("xhigh")
    );
    let fresh = config(json!({}), prompt, "");
    let fresh_identity = LaunchIdentity::new("claude-test", &fresh, Some(&absolute_test_cwd()));
    assert!(
        !fresh_identity
            .args()
            .iter()
            .any(|argument| argument == "--resume")
    );
}

#[test]
fn default_permission_mode_is_yolo() {
    let driver_config = config(json!({}), "exact user prompt", "");
    let identity = LaunchIdentity::new("claude-test", &driver_config, None);
    let args = identity.args();
    let position = args
        .iter()
        .position(|argument| argument == "--permission-mode");
    assert_eq!(
        position.map(|index| args[index + 1].as_str()),
        Some("bypassPermissions")
    );
    // The mapping uses the existing --permission-mode argument only: no MCP,
    // SDK, second switch, or vendor legacy flag appears in the argv.
    assert!(args.iter().all(|argument| {
        !matches!(argument.as_str(), "mcp" | "sdk")
            && !argument.starts_with("--mcp")
            && !argument.starts_with("--sdk")
            && !argument.starts_with("--dangerously")
    }));
    let effective = identity.effective();
    assert_eq!(
        effective.permission_mode.as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(
        effective.approval_policy.as_ref().and_then(Value::as_str),
        Some("bypassPermissions")
    );
    // The default resolves into the identity (one value), so an unspecified
    // turn stays compatible with the live process instead of contradicting it.
    let continued = config(json!({}), "next exact user prompt", "");
    assert!(identity.compatible_with("claude-test", &continued, None));
}

#[test]
fn explicit_permission_mode_overrides_win_against_the_default() {
    let cases = [
        ("plan", "plan"),
        ("default", "default"),
        ("manual", "manual"),
        ("acceptEdits", "acceptEdits"),
        ("auto", "auto"),
        ("dontAsk", "dontAsk"),
        ("bypassPermissions", "bypassPermissions"),
        ("on-request", "default"),
        ("never", "dontAsk"),
    ];
    for (input, expected) in cases {
        let driver_config = config(json!({"permissionMode": input}), "exact user prompt", "");
        let identity = LaunchIdentity::new("claude-test", &driver_config, None);
        let args = identity.args();
        let position = args
            .iter()
            .position(|argument| argument == "--permission-mode");
        assert_eq!(
            position.map(|index| args[index + 1].as_str()),
            Some(expected),
            "explicit {input} must win over the YOLO default"
        );
        let effective = identity.effective();
        assert_eq!(effective.permission_mode.as_deref(), Some(expected));
        assert_eq!(
            effective.approval_policy.as_ref().and_then(Value::as_str),
            Some(expected)
        );
    }
}

#[test]
fn private_instructions_use_the_native_system_channel_and_define_process_identity() {
    let driver_config = config(
        json!({"privateInstructions": "synthetic private guidance"}),
        "exact user prompt",
        "",
    );
    let identity = LaunchIdentity::new("claude-test", &driver_config, None);
    let args = identity.args();
    let position = args
        .iter()
        .position(|argument| argument == "--append-system-prompt");
    assert_eq!(
        position.map(|index| args[index + 1].as_str()),
        Some("synthetic private guidance")
    );
    assert!(args.iter().all(|argument| argument != "exact user prompt"));

    let without_guidance = config(json!({}), "next exact user prompt", "");
    assert!(!identity.compatible_with("claude-test", &without_guidance, None));
}

#[test]
fn executable_directory_precedes_inherited_runtime_path() {
    let inherited = std::ffi::OsStr::new("/usr/bin:/bin");
    let path = executable_augmented_path("/runtime/bin/claude", Some(inherited)).unwrap();
    let entries = std::env::split_paths(&path).collect::<Vec<_>>();

    assert_eq!(entries[0], std::path::PathBuf::from("/runtime/bin"));
    assert_eq!(entries[1], std::path::PathBuf::from("/usr/bin"));
    assert_eq!(entries[2], std::path::PathBuf::from("/bin"));
}
