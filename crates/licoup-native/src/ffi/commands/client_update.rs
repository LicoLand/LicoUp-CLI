// update commands: update status|check|download|verify|apply

use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

pub(super) fn handle_update(command: AdmittedCommand) -> Result<CliExecution> {
    let action = match command.path() {
        ["update", action] => *action,
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    let params = admitted_params(
        &[
            (
                "targetReleaseTrack",
                command.option_text("target-release-track"),
            ),
            ("manifestPath", command.option_text("manifest-path")),
            ("publicKeysPath", command.option_text("public-keys-path")),
            ("revocationPath", command.option_text("revocation-path")),
            ("sourcePath", command.option_text("source-path")),
            ("source", command.option_text("source")),
            ("repo", command.option_text("repo")),
            ("stagingRoot", command.option_text("staging-root")),
            ("stateRoot", command.option_text("state-root")),
            (
                "dataRoot",
                (action == "apply")
                    .then(|| command.option_text("data-root"))
                    .flatten(),
            ),
            ("execute", command.option_text("execute")),
            ("installRoot", command.option_text("install-root")),
            ("guiPid", command.option_text("gui-pid")),
            ("waitForScript", command.option_text("wait-for-script")),
        ],
        &[],
        &[],
    );
    let route = ["update".to_string(), action.to_string()];
    Ok(CliExecution::Json(crate::domain::client_update::dispatch(
        &route, &params,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::commands::{CliCommandError, execute_cli};

    fn admission_code(result: &Result<CliExecution>) -> Option<&'static str> {
        match result {
            Err(error) => error
                .downcast_ref::<CliCommandError>()
                .map(|error| error.code()),
            Ok(_) => None,
        }
    }

    #[test]
    fn update_status_admits_non_apply_option_surface() {
        let output = match execute_cli(vec![
            "update".into(),
            "status".into(),
            "--target-release-track".into(),
            "stable".into(),
            "--source".into(),
            "github".into(),
            "--repo".into(),
            "LicoLand/LicoUp".into(),
            "--staging-root".into(),
            "/fixture-root/licoup-staging".into(),
            "--state-root".into(),
            "/fixture-root/licoup-state".into(),
            "--execute".into(),
            "true".into(),
            "--install-root".into(),
            "/Applications".into(),
            "--gui-pid".into(),
            "4242".into(),
            "--wait-for-script".into(),
            "true".into(),
        ]) {
            Ok(CliExecution::Json(value)) => value,
            _ => {
                assert!(false, "update status must be JSON");
                serde_json::Value::Null
            }
        };
        assert_eq!(output["ok"], true);
        assert_eq!(output["runningReleaseTrack"], "nightly");
    }

    #[test]
    fn update_data_root_is_admitted_only_for_apply() {
        for action in ["status", "check", "download", "verify"] {
            let result = execute_cli(vec![
                "update".into(),
                action.into(),
                "--data-root".into(),
                "/fixture-root/licoup-data".into(),
            ]);
            assert_eq!(admission_code(&result), Some("cli_option_unknown"));
        }
        let result = execute_cli(vec![
            "update".into(),
            "apply".into(),
            "--data-root".into(),
            "/fixture-root/licoup-data".into(),
        ]);
        assert_ne!(admission_code(&result), Some("cli_option_unknown"));
    }

    #[test]
    fn update_track_override_is_admitted_only_for_status_and_check() {
        for action in ["download", "verify", "apply"] {
            let result = execute_cli(vec![
                "update".into(),
                action.into(),
                "--target-release-track".into(),
                "stable".into(),
            ]);
            assert_eq!(admission_code(&result), Some("cli_option_unknown"));
        }
        for action in ["status", "check"] {
            let result = execute_cli(vec![
                "update".into(),
                action.into(),
                "--target-release-track".into(),
                "stable".into(),
            ]);
            assert_ne!(admission_code(&result), Some("cli_option_unknown"));
        }
    }

    #[test]
    fn update_check_admits_github_source_options() {
        let result = execute_cli(vec![
            "update".into(),
            "check".into(),
            "--source".into(),
            "github".into(),
            "--repo".into(),
            "LicoLand/LicoUp".into(),
            "--target-release-track".into(),
            "stable".into(),
        ]);
        // Reaches the domain layer: GitHub fetch is not expected to succeed
        // here, so any outcome other than an admission rejection is proof of
        // admission.
        assert_ne!(admission_code(&result), Some("cli_option_unknown"));
    }

    #[test]
    fn update_rejects_unknown_options() {
        let result = execute_cli(vec![
            "update".into(),
            "status".into(),
            "--bogus-option".into(),
            "value".into(),
        ]);
        assert_eq!(admission_code(&result), Some("cli_option_unknown"));
    }

    #[test]
    fn update_rejects_source_path_combined_with_github_source() {
        let result = execute_cli(vec![
            "update".into(),
            "check".into(),
            "--source-path".into(),
            "/fixture-root/artifact.zip".into(),
            "--source".into(),
            "github".into(),
        ]);
        assert_eq!(
            admission_code(&result),
            Some("cli_option_constraint_violation")
        );
    }

    #[test]
    fn update_rejects_source_path_combined_with_repo() {
        let result = execute_cli(vec![
            "update".into(),
            "check".into(),
            "--source-path".into(),
            "/fixture-root/artifact.zip".into(),
            "--repo".into(),
            "LicoLand/LicoUp".into(),
        ]);
        assert_eq!(
            admission_code(&result),
            Some("cli_option_constraint_violation")
        );
    }
}
