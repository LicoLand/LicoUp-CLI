use super::{AdmittedCommand, CliExecution};
use anyhow::Result;

pub(super) fn handle_read(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::model_registry::read()))
}

pub(super) fn handle_refresh(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::model_registry::refresh()))
}

#[cfg(test)]
mod tests {
    use super::super::{CliExecution, admit_cli_command, execute_cli};

    /// The command layer stays the only entry to the registry: the verbs are
    /// admitted by the shared command table, take no arguments, and `read`
    /// reports the current snapshot without refreshing it.
    #[test]
    fn local_registry_commands_are_typed_and_read_does_not_refresh() {
        for verb in ["read", "refresh"] {
            assert!(admit_cli_command(vec!["model-registry".into(), verb.into()]).is_ok());
            assert!(
                admit_cli_command(vec![
                    "model-registry".into(),
                    verb.into(),
                    "--source".into(),
                    "arbitrary".into()
                ])
                .is_err()
            );
        }
        let CliExecution::Json(result) =
            execute_cli(vec!["model-registry".into(), "read".into()]).unwrap()
        else {
            panic!("expected JSON");
        };
        assert_eq!(result["ok"], true);
        assert_eq!(result["status"], "empty");
    }
}
