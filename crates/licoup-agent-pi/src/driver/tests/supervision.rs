use super::*;

#[test]
fn launch_arguments_are_fixed_rpc_without_prompt_session_or_shell() {
    let launch = LaunchSpec::new("pi", absolute_test_cwd().as_path());
    assert_eq!(launch.args, LAUNCH_ARGS);
    assert_eq!(launch.args, ["--mode", "rpc", "--offline"]);
    assert!(
        !launch
            .args
            .iter()
            .any(|argument| argument.contains("prompt") || argument.contains("session"))
    );
}
