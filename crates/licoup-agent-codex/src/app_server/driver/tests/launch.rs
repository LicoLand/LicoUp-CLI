use crate::app_server::driver::launch::{
    CodexLaunchSpec, apply_launch_environment, apply_launch_environment_with_root,
};
use serde_json::json;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

#[test]
fn launch_spec_has_no_prompt_channel_and_uses_official_stdio() {
    let prompt = "must-not-appear-in-process-metadata";
    let launch = CodexLaunchSpec::new("codex-test", Some(Path::new("/workspace/project")));
    assert_eq!(launch.executable, "codex-test");
    assert_eq!(launch.args, ["app-server", "--stdio"]);
    assert!(!launch.executable.contains(prompt));
    assert!(
        launch
            .args
            .iter()
            .all(|argument| !argument.contains(prompt))
    );
}

fn command_environment(command: &Command, key: &str) -> Option<String> {
    command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
}

#[test]
fn launch_forwards_caller_context_and_binds_only_the_named_environment() {
    let previous_root = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
        std::path::PathBuf::from("/synthetic/licoup-home"),
    ));
    let mut command = Command::new("codex-test");
    apply_launch_environment(
        &mut command,
        Some(&json!({
            "agentId": "codex",
            "conversationId": "conversation:fixture",
            "membershipId": "membership:codex",
            "dispatchId": "turn:direct"
        })),
        Some(&[("PATH".to_owned(), "/user/shell/bin".to_owned())]),
    )
    .expect("the selected data root should be available to the child");

    assert_eq!(
        command_environment(&command, "LICOUP_MCP_CALLER_PROVIDER").as_deref(),
        Some("codex")
    );
    assert_eq!(
        command_environment(&command, "LICOUP_MCP_CONVERSATION_ID").as_deref(),
        Some("conversation:fixture")
    );
    assert_eq!(
        command_environment(&command, "LICOUP_MCP_MEMBERSHIP_ID").as_deref(),
        Some("membership:codex")
    );
    assert_eq!(
        command_environment(&command, "LICOUP_HOME").as_deref(),
        Some("/synthetic/licoup-home")
    );
    // The caller's own environment is the child's whole environment: the
    // package process's variables are not inherited on top of it.
    assert_eq!(
        command_environment(&command, "PATH").as_deref(),
        Some("/user/shell/bin")
    );
    assert_eq!(
        command_environment(&command, "LICOUP_MCP_PARENT_DISPATCH_ID"),
        None
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_root);
}

#[test]
fn launch_binds_only_the_live_process_portable_root() {
    // Without a live process root the launch environment never binds either
    // root name, even when the caller's environment carries one.
    let mut command = Command::new("codex-test");
    apply_launch_environment_with_root(
        &mut command,
        Some(&json!({
            "agentId": "codex",
            "conversationId": "conversation:fixture",
            "membershipId": "membership:codex"
        })),
        Some(&[
            ("LICOUP_HOME".to_owned(), "/caller/home".to_owned()),
            (
                "LICOUP_PORTABLE_DIR".to_owned(),
                "/caller/legacy".to_owned(),
            ),
        ]),
        None,
    );
    assert_eq!(command_environment(&command, "LICOUP_HOME"), None);
    assert_eq!(command_environment(&command, "LICOUP_PORTABLE_DIR"), None);

    // Both names carry the same selected root: the released Subagent MCP
    // server forwards the legacy name through its allowlist.
    let mut command = Command::new("codex-test");
    apply_launch_environment_with_root(
        &mut command,
        None,
        None,
        Some(OsString::from("/portable/lico-up")),
    );
    assert_eq!(
        command_environment(&command, "LICOUP_HOME").as_deref(),
        Some("/portable/lico-up")
    );
    assert_eq!(
        command_environment(&command, "LICOUP_PORTABLE_DIR").as_deref(),
        Some("/portable/lico-up")
    );
}
