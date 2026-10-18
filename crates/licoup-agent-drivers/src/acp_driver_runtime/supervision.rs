use super::errors::ProtocolFailure;
use super::model::{AcpDriverSpec, EffectiveSettings};
use super::params::RequestedSettings;
use licoup_foundation::platform::process_supervisor::SupervisedChild;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct LaunchSpec {
    pub executable: String,
    pub driver: AcpDriverSpec,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub allow_all: Option<bool>,
}

impl LaunchSpec {
    pub fn new(executable: &str, driver: AcpDriverSpec, cwd: &Path) -> Self {
        Self {
            executable: executable.to_string(),
            driver,
            cwd: cwd.to_path_buf(),
            model: None,
            reasoning_effort: None,
            allow_all: None,
        }
    }

    pub fn for_execution(
        executable: &str,
        driver: AcpDriverSpec,
        cwd: &Path,
        requested: &mut RequestedSettings,
    ) -> Result<Self, ProtocolFailure> {
        let model = driver.launch_model_arg.and_then(|_| requested.model.take());
        let reasoning_effort = if driver.launch_reasoning_env.is_some() {
            requested.reasoning_effort.take()
        } else {
            None
        };
        let allow_all = if driver.launch_allow_all_arg.is_some() {
            requested.allow_all.take()
        } else {
            None
        };
        if reasoning_effort
            .as_deref()
            .is_some_and(|effort| !driver.launch_reasoning_values.contains(&effort))
        {
            return Err(ProtocolFailure::new(
                "acp_setting_unsupported",
                "The ACP agent cannot preserve one of the requested native session settings.",
                "process/configure",
            ));
        }
        Ok(Self {
            executable: executable.to_string(),
            driver,
            cwd: cwd.to_path_buf(),
            model,
            reasoning_effort,
            allow_all,
        })
    }

    pub fn apply_effective_settings(&self, effective: &mut EffectiveSettings) {
        if let Some(model) = self.model.as_ref() {
            effective.model = Some(model.clone());
        }
        if let Some(reasoning_effort) = self.reasoning_effort.as_ref() {
            effective.reasoning_effort = Some(reasoning_effort.clone());
        }
        if let Some(allow_all) = self.allow_all {
            effective.allow_all = Some(allow_all);
            effective.approval_policy = Some(serde_json::Value::Bool(allow_all));
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = Vec::with_capacity(self.driver.launch_args.len() + 3);
        let settings = |arguments: &mut Vec<String>| {
            if self.allow_all == Some(true)
                && let Some(argument) = self.driver.launch_allow_all_arg
            {
                arguments.push(argument.to_string());
            }
            if let (Some(argument), Some(model)) =
                (self.driver.launch_model_arg, self.model.as_ref())
            {
                arguments.push(argument.to_string());
                arguments.push(model.clone());
            }
        };
        if self.driver.launch_settings_after_args {
            arguments.extend(
                self.driver
                    .launch_args
                    .iter()
                    .map(|value| (*value).to_string()),
            );
            settings(&mut arguments);
        } else {
            settings(&mut arguments);
            arguments.extend(
                self.driver
                    .launch_args
                    .iter()
                    .map(|value| (*value).to_string()),
            );
        }
        arguments
    }

    pub fn spawn(&self) -> io::Result<SupervisedChild> {
        let mut command = Command::new(&self.executable);
        licoup_agent_targets::platform::user_shell_environment::apply_to_command(&mut command);
        command
            .args(self.arguments())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let (Some(environment), Some(reasoning_effort)) = (
            self.driver.launch_reasoning_env,
            self.reasoning_effort.as_ref(),
        ) {
            command.env(environment, reasoning_effort);
        }
        SupervisedChild::spawn(&mut command)
    }
}

pub fn acp_pipe_failure(child: &mut SupervisedChild) -> ProtocolFailure {
    if child.terminate_tree().is_ok() {
        ProtocolFailure::new(
            "acp_process_pipe_failed",
            "The ACP agent protocol pipes are unavailable.",
            "process/start",
        )
    } else {
        ProtocolFailure::new(
            "acp_process_cleanup_failed",
            "The ACP agent process cleanup could not be completed safely.",
            "process/cleanup",
        )
    }
}
