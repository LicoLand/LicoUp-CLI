//! Argv-only runner. Network responses never enter a shell.

use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::process::{Command, Stdio};

const SHELL_PROGRAMS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "cmd",
    "cmd.exe",
    "powershell",
    "powershell.exe",
    "pwsh",
    "pwsh.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgvKind {
    Homebrew,
    Npm,
    Winget,
    OfficialArtifact,
    Lifecycle,
}

impl ArgvKind {
    pub fn for_channel(kind: &str) -> Self {
        match kind {
            "homebrew" => Self::Homebrew,
            "npm" => Self::Npm,
            "winget" => Self::Winget,
            _ => Self::OfficialArtifact,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArgvOutcome {
    pub program: String,
    pub args: Vec<String>,
    pub status: i32,
    pub stdout: String,
}

pub trait ArgvRunner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> Result<ArgvOutcome>;
}

#[derive(Clone, Debug, Default)]
pub struct ProcessArgvRunner;

impl ArgvRunner for ProcessArgvRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<ArgvOutcome> {
        validate_program_args(program, args, ArgvKind::Lifecycle)?;
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| anyhow!("argv_runner_failed: {error}"))?;
        Ok(ArgvOutcome {
            program: program.to_string(),
            args: args.to_vec(),
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        })
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct RecordingArgvRunner {
    pub commands: std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>>,
    pub status: i32,
}

#[cfg(test)]
impl RecordingArgvRunner {
    pub fn new() -> Self {
        Self {
            commands: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            status: 0,
        }
    }

    pub fn recorded(&self) -> Vec<(String, Vec<String>)> {
        self.commands
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl ArgvRunner for RecordingArgvRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<ArgvOutcome> {
        validate_program_args(program, args, ArgvKind::Lifecycle)?;
        if let Ok(mut guard) = self.commands.lock() {
            guard.push((program.to_string(), args.to_vec()));
        }
        Ok(ArgvOutcome {
            program: program.to_string(),
            args: args.to_vec(),
            status: self.status,
            stdout: "ok".to_string(),
        })
    }
}

pub fn validate(argv: &[String], kind: ArgvKind) -> Result<()> {
    if argv.is_empty() {
        return Ok(());
    }
    validate_program_args(&argv[0], &argv[1..], kind)
}

pub fn validate_program_args(program: &str, args: &[String], kind: ArgvKind) -> Result<()> {
    let program = program.trim();
    ensure!(!program.is_empty(), "argv_forbidden");
    ensure!(!program.contains('|'), "argv_forbidden");
    ensure!(!program.contains(';'), "argv_forbidden");
    for arg in args {
        ensure!(!arg.contains('|'), "argv_forbidden");
        ensure!(!arg.contains("curl "), "argv_forbidden");
        ensure!(!arg.contains("irm "), "argv_forbidden");
        ensure!(!arg.contains("iwr "), "argv_forbidden");
    }
    let file_name = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let is_shell = SHELL_PROGRAMS.contains(&file_name.as_str());
    if is_shell {
        ensure!(
            kind == ArgvKind::OfficialArtifact || kind == ArgvKind::Lifecycle,
            "argv_forbidden"
        );
        ensure!(
            !args
                .iter()
                .any(|arg| arg == "-c" || arg == "/c" || arg == "-Command" || arg == "-c"),
            "argv_forbidden"
        );
        ensure!(
            args.iter()
                .any(|arg| arg == "{script}" || arg == "-File" || arg == "-NoProfile")
                || args.len() == 1,
            "argv_forbidden"
        );
    }
    Ok(())
}

pub fn outcome_json(outcome: &ArgvOutcome) -> Value {
    json!({
        "program": outcome.program,
        "argc": outcome.args.len(),
        "status": outcome.status
    })
}
