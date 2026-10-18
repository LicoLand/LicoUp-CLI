use anyhow::{Result, anyhow, ensure};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use licoup_workflow::{CommandKind, RunCommand, RuntimeKind};

// The effect permit is the runtime's own contract: the runtime issues it, the
// host consumes it, and there is exactly one type with that name.
pub(crate) use licoup_workflow_runtime::ports::StrategyEffectPermit;

mod effect_dispatch;

pub(crate) use effect_dispatch::{LaneEffectDispatch, RuntimeRegistryAgentProfiles};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeDescriptor {
    pub id: String,
    pub kind: RuntimeKind,
    pub version: String,
    pub available: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct VerifiedRuntime {
    descriptor: RuntimeDescriptor,
    executable: PathBuf,
    runtime_root: PathBuf,
    fingerprint: String,
}

impl VerifiedRuntime {
    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeCatalog {
    values: BTreeMap<String, VerifiedRuntime>,
}

impl RuntimeCatalog {
    pub(crate) fn discover() -> Self {
        let mut values = BTreeMap::new();
        let mut canonical = BTreeSet::new();
        for (kind, names) in [
            (RuntimeKind::Python, &["python3", "python"][..]),
            (RuntimeKind::Node, &["node"][..]),
        ] {
            for name in names {
                let Some(path) = resolve_on_path(name) else {
                    continue;
                };
                let Ok(path) = fs::canonicalize(path) else {
                    continue;
                };
                if !canonical.insert(path.clone()) {
                    continue;
                }
                let Ok(value) = verify_runtime(kind, &path) else {
                    continue;
                };
                values.insert(value.descriptor.id.clone(), value);
                break;
            }
        }
        Self { values }
    }

    pub(crate) fn descriptors(&self) -> Vec<RuntimeDescriptor> {
        self.values
            .values()
            .map(|runtime| runtime.descriptor.clone())
            .collect()
    }

    pub(crate) fn resolve(
        &self,
        id: &str,
        kind: RuntimeKind,
        requirement: &str,
    ) -> Result<VerifiedRuntime> {
        let runtime = self
            .values
            .get(id)
            .filter(|runtime| runtime.descriptor.kind == kind)
            .ok_or_else(|| anyhow!("strategy_runtime_unavailable"))?;
        if !requirement.trim().is_empty() {
            let requirement = VersionReq::parse(requirement)
                .map_err(|_| anyhow!("strategy_runtime_requirement_invalid"))?;
            let version = Version::parse(&runtime.descriptor.version)
                .map_err(|_| anyhow!("strategy_runtime_version_invalid"))?;
            ensure!(
                requirement.matches(&version),
                "strategy_runtime_unavailable"
            );
        }
        verify_runtime(kind, &runtime.executable).and_then(|current| {
            ensure!(
                current.fingerprint == runtime.fingerprint,
                "strategy_runtime_drifted"
            );
            Ok(current)
        })
    }

    pub(crate) fn compatible_id(&self, kind: RuntimeKind, requirement: &str) -> Option<String> {
        self.values.keys().find_map(|id| {
            self.resolve(id, kind, requirement)
                .ok()
                .map(|_| id.to_owned())
        })
    }
}

pub(crate) fn execute_script(
    command: &RunCommand,
    authorization_digest: &str,
    runtime: &VerifiedRuntime,
    revision_content: &Path,
    runtime_state_root: &Path,
    permit: &mut StrategyEffectPermit,
) -> Result<Value> {
    ensure!(
        command.kind == CommandKind::Script,
        "strategy_command_kind_invalid"
    );
    permit.consume(command, authorization_digest, runtime.fingerprint())?;
    let entry = command
        .entry
        .as_deref()
        .ok_or_else(|| anyhow!("strategy_script_entry_missing"))?;
    ensure!(
        entry.starts_with("scripts/")
            && !entry.contains("..")
            && !entry.contains('\\')
            && !entry.contains('\0'),
        "strategy_script_entry_invalid"
    );
    let revision_content = fs::canonicalize(revision_content)?;
    let script = fs::canonicalize(revision_content.join(entry))?;
    ensure!(
        script.starts_with(&revision_content),
        "strategy_script_entry_invalid"
    );
    let metadata = fs::symlink_metadata(&script)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "strategy_script_entry_invalid"
    );
    licoup_foundation::platform::file_security::ensure_private_dir(runtime_state_root)?;
    let scratch = runtime_state_root.join(&command.id);
    if scratch.exists() {
        let _ = fs::remove_dir_all(&scratch);
    }
    licoup_foundation::platform::file_security::ensure_private_dir(&scratch)?;
    let scratch = fs::canonicalize(&scratch)?;
    let mut process = crate::platform::process_sandbox::strategy_script_command(
        &runtime.executable,
        &runtime.runtime_root,
        &script,
        &revision_content,
        &scratch,
    )?;
    let input = serde_json::to_vec(&command.input)?;
    ensure!(input.len() <= 1024 * 1024, "strategy_input_too_large");
    let output = crate::platform::run_bounded_command_input(
        &mut process,
        &input,
        Duration::from_secs(120),
        1024 * 1024,
    )
    .map_err(|_| anyhow!("strategy_script_execution_failed"))?;
    let _ = fs::remove_dir_all(&scratch);
    ensure!(!output.timed_out, "strategy_script_timed_out");
    ensure!(!output.truncated, "strategy_script_output_too_large");
    ensure!(
        output.status.is_some_and(|status| status.success()),
        "strategy_script_execution_failed"
    );
    serde_json::from_slice(&output.stdout).map_err(|_| anyhow!("strategy_script_output_invalid"))
}

pub(crate) fn actor_fingerprint(
    actor_id: &str,
    model: &str,
    reasoning_effort: &str,
) -> Result<String> {
    ensure!(
        actor_id == actor_id.trim()
            && !actor_id.is_empty()
            && actor_id.len() <= 96
            && actor_id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_')
            }),
        "strategy_actor_binding_invalid"
    );
    Ok(sha256_hex(
        format!(
            "licoup-strategy-actor-v1\0{actor_id}\0{}\0{}",
            model.trim(),
            reasoning_effort.trim()
        )
        .as_bytes(),
    ))
}

pub(crate) fn execute_actor(
    command: &RunCommand,
    authorization_digest: &str,
    binding: &crate::domain::workflow_store::BindingValue,
    permit: &mut StrategyEffectPermit,
    cwd: Option<&str>,
) -> Result<Value> {
    ensure!(
        matches!(command.kind, CommandKind::Actor | CommandKind::WorksetItem),
        "strategy_command_kind_invalid"
    );
    if let Some(cwd) = cwd {
        admit_strategy_cwd(cwd)?;
    }
    let fingerprint =
        actor_fingerprint(&binding.value_id, &binding.model, &binding.reasoning_effort)?;
    permit.consume(command, authorization_digest, &fingerprint)?;
    let prompt = command
        .input
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(serde_json::to_string(&command.input)?);
    ensure!(
        !prompt.trim().is_empty() && prompt.len() <= 1024 * 1024,
        "strategy_actor_input_invalid"
    );
    let mut request = serde_json::json!({
        "agent": binding.value_id,
        "message": prompt,
        "sessionId": command.resume_session_id,
        "timeoutMs": 0,
        "maxStderrBytes": 512 * 1024,
    });
    if let Value::Object(ref mut object) = request {
        if !binding.model.is_empty() {
            object.insert("model".into(), Value::String(binding.model.clone()));
        }
        if !binding.reasoning_effort.is_empty() {
            object.insert(
                "reasoningEffort".into(),
                Value::String(binding.reasoning_effort.clone()),
            );
        }
        if let Some(cwd) = cwd {
            object.insert("cwd".into(), Value::String(cwd.to_owned()));
            object.insert("workingDirectory".into(), Value::String(cwd.to_owned()));
        }
    }
    let response = match crate::agent_port::send(&request) {
        Ok(value) => value,
        Err(error) => return Err(anyhow!("strategy_actor_dispatch_failed:{error}")),
    };
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!("{}", actor_failure_code(&response)));
    }
    let parsed = response
        .get("output")
        .and_then(Value::as_str)
        .and_then(|value| serde_json::from_str::<Value>(value).ok());
    if let Some(Value::Object(mut output)) = parsed {
        for key in [
            "nativeSessionId",
            "sessionId",
            "turnId",
            "sourcePath",
            "sourceKind",
            "table",
            "keyPrefixes",
        ] {
            if let Some(value) = response.get(key).filter(|value| !value.is_null()) {
                output.insert(key.to_owned(), value.clone());
            }
        }
        Ok(Value::Object(output))
    } else {
        Ok(response)
    }
}

pub(crate) fn admit_strategy_cwd(cwd: &str) -> Result<()> {
    let path = Path::new(cwd);
    ensure!(
        path.is_absolute()
            && cwd == cwd.trim()
            && !cwd.is_empty()
            && cwd.len() <= 4096
            && !cwd.chars().any(char::is_control),
        "strategy_cwd_invalid"
    );
    let home = licoup_foundation::platform::paths::user_home_from_env();
    ensure!(
        !crate::platform::agent_workspace::is_unbounded_agent_workspace(path, home.as_deref()),
        "strategy_cwd_invalid"
    );
    Ok(())
}

pub(crate) fn predecessor_locator(facts: &Value) -> Value {
    let native_session_id = facts
        .get("nativeSessionId")
        .or_else(|| facts.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source_kind = facts
        .get("sourceKind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source_path = facts
        .get("sourcePath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path_ok = !source_path.is_empty() && admit_strategy_cwd(source_path).is_ok();
    let mut locator = serde_json::json!({
        "nativeSessionId": native_session_id,
        "sourceKind": source_kind,
        "locatorUnavailable": !path_ok,
    });
    if path_ok {
        locator["sourcePath"] = Value::String(source_path.to_owned());
    }
    if let Some(table) = facts
        .get("table")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        locator["table"] = Value::String(table.to_owned());
    }
    if let Some(prefixes) = facts.get("keyPrefixes").cloned() {
        locator["keyPrefixes"] = prefixes;
    }
    locator
}

fn actor_failure_code(response: &Value) -> String {
    let error = response.get("error").unwrap_or(response);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let blob = format!("{code}\n{message}");
    if [
        "quota",
        "usage_limit",
        "credit",
        "rate_limit",
        "rate-limit",
        "capacity",
        "exhaust",
    ]
    .iter()
    .any(|marker| blob.contains(marker))
    {
        "strategy_actor_quota_exhausted".into()
    } else if blob.contains("timed_out") || blob.contains("timeout") {
        "strategy_actor_dispatch_failed".into()
    } else {
        "strategy_actor_dispatch_failed".into()
    }
}

fn verify_runtime(kind: RuntimeKind, executable: &Path) -> Result<VerifiedRuntime> {
    let metadata = fs::symlink_metadata(executable)?;
    ensure!(
        executable.is_absolute() && metadata.is_file() && !metadata.file_type().is_symlink(),
        "strategy_runtime_unavailable"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o022 == 0,
            "strategy_runtime_unavailable"
        );
    }
    let value = executable
        .to_str()
        .filter(|value| !value.contains('\\') && !value.contains('"'))
        .ok_or_else(|| anyhow!("strategy_runtime_unavailable"))?;
    let mut command = Command::new(executable);
    command.arg("--version");
    let output = crate::platform::run_bounded_untrusted_agent_output(
        &mut command,
        Duration::from_secs(3),
        4 * 1024,
    )?;
    ensure!(
        !output.timed_out
            && !output.truncated
            && output.status.is_some_and(|status| status.success()),
        "strategy_runtime_unavailable"
    );
    let version = parse_version(kind, std::str::from_utf8(&output.stdout)?)?;
    let runtime_root = runtime_root(executable, kind)?;
    let fingerprint = sha256_hex(
        format!(
            "licoup-strategy-runtime-v1\0{}\0{}\0{}\0{}",
            kind_wire(kind),
            value,
            version,
            metadata.len()
        )
        .as_bytes(),
    );
    let id = format!("runtime-{}-{}", kind_wire(kind), &fingerprint[..24]);
    Ok(VerifiedRuntime {
        descriptor: RuntimeDescriptor {
            id,
            kind,
            version,
            available: true,
        },
        executable: executable.to_path_buf(),
        runtime_root,
        fingerprint,
    })
}

fn parse_version(kind: RuntimeKind, value: &str) -> Result<String> {
    let token = match kind {
        RuntimeKind::Python => value.trim().strip_prefix("Python "),
        RuntimeKind::Node => value.trim().strip_prefix('v'),
    }
    .ok_or_else(|| anyhow!("strategy_runtime_version_invalid"))?;
    let version = Version::parse(token).map_err(|_| anyhow!("strategy_runtime_version_invalid"))?;
    Ok(version.to_string())
}

fn runtime_root(executable: &Path, kind: RuntimeKind) -> Result<PathBuf> {
    let parent = executable
        .parent()
        .ok_or_else(|| anyhow!("strategy_runtime_unavailable"))?;
    Ok(match kind {
        RuntimeKind::Python => parent.parent().unwrap_or(parent),
        RuntimeKind::Node => parent.parent().unwrap_or(parent),
    }
    .to_path_buf())
}

fn resolve_on_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

const fn kind_wire(kind: RuntimeKind) -> &'static str {
    match kind {
        RuntimeKind::Python => "python",
        RuntimeKind::Node => "node",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
