use super::*;

#[test]
fn launch_arguments_are_fixed_and_do_not_contain_prompt_or_path() {
    let cwd = absolute_test_cwd();
    let driver = AcpDriverSpec::new("test-acp", &["acp"]);
    let launch = LaunchSpec::new("test-agent", driver, &cwd);
    assert_eq!(launch.driver.launch_args, &["acp"]);
    assert!(!launch.driver.launch_args.contains(&"private"));
    assert!(
        !launch
            .driver
            .launch_args
            .iter()
            .any(|arg| *arg == cwd.to_string_lossy())
    );
    assert!(
        !launch
            .driver
            .launch_args
            .iter()
            .any(|arg| *arg == "native-session" || *arg == "provider/model")
    );
}

#[test]
fn launch_bound_settings_are_removed_from_acp_and_reported_as_effective() {
    let cwd = absolute_test_cwd();
    let driver = AcpDriverSpec::new("test-acp", &["acp"])
        .with_launch_settings("--model", "TEST_REASONING_EFFORT", &["low", "high", "max"])
        .with_allow_all_argument("--auto");
    let mut requested = super::super::params::RequestedSettings {
        model: Some("provider/model".into()),
        reasoning_effort: Some("max".into()),
        mode: None,
        runtime_agent: None,
        allow_all: Some(true),
    };
    let launch = LaunchSpec::for_execution("test-agent", driver, &cwd, &mut requested).unwrap();

    assert_eq!(
        launch.arguments(),
        vec!["--auto", "--model", "provider/model", "acp"]
    );
    assert_eq!(launch.reasoning_effort.as_deref(), Some("max"));
    assert_eq!(requested.model, None);
    assert_eq!(requested.reasoning_effort, None);
    assert_eq!(requested.allow_all, None);
    let mut effective = super::super::model::EffectiveSettings::default();
    launch.apply_effective_settings(&mut effective);
    assert_eq!(effective.model.as_deref(), Some("provider/model"));
    assert_eq!(effective.reasoning_effort.as_deref(), Some("max"));
    assert_eq!(effective.allow_all, Some(true));
}

#[test]
fn launch_bound_reasoning_effort_fails_closed_outside_allowlist() {
    let cwd = absolute_test_cwd();
    let driver = AcpDriverSpec::new("test-acp", &["acp"]).with_launch_settings(
        "--model",
        "TEST_REASONING_EFFORT",
        &["low", "high", "max"],
    );
    let mut requested = super::super::params::RequestedSettings {
        model: None,
        reasoning_effort: Some("ultra".into()),
        mode: None,
        runtime_agent: None,
        allow_all: None,
    };
    let failure = LaunchSpec::for_execution("test-agent", driver, &cwd, &mut requested)
        .expect_err("unsupported effort must fail closed");
    assert_eq!(failure.code, "acp_setting_unsupported");
}
