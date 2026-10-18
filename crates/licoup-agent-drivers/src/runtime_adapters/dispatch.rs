use super::adapter::{CapabilityFactName, CapabilityFactState, NativeAbilityKind};
use super::artifact::runtime_executable;
use super::normalization::{execution_response, execution_response_named};
use super::params::{
    AttachmentShapeFailure, LocalImageInput, MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE,
    MAX_IMAGE_ATTACHMENT_BYTES_TOTAL, binary_param, bounded_output_param, codex_binary_param,
    message_param, optional_output_param, parse_attachments, text_param,
};
use super::port::dispatch_timeout;
use super::{DEFAULT_MAX_STDERR_BYTES, RuntimeAdapter, RuntimeAdapterError};
use super::{RuntimeLane, runtime_lane_for_agent};
use licoup_agent_targets::platform::virtual_machine::{
    SshRuntimeConnection, is_valid_guest_working_directory,
};
use licoup_foundation::platform::agent_workspace::resolve_local_agent_workspace;
use serde_json::Value;
use std::{
    borrow::Cow,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn attachment_shape_error(failure: AttachmentShapeFailure) -> RuntimeAdapterError {
    match failure {
        AttachmentShapeFailure::ListExceeded => RuntimeAdapterError::AttachmentListExceeded,
        AttachmentShapeFailure::NotArray
        | AttachmentShapeFailure::NotObject
        | AttachmentShapeFailure::UnknownField
        | AttachmentShapeFailure::FieldMissing => RuntimeAdapterError::AttachmentInvalid,
        AttachmentShapeFailure::MediaUnsupported => RuntimeAdapterError::AttachmentMediaUnsupported,
        AttachmentShapeFailure::RemoteUrl => RuntimeAdapterError::AttachmentRemoteUnsupported,
    }
}

/// Only an Agent whose own lane declares image input may carry image
/// attachments, and only over a transport that can move the local files. Both
/// admission failures happen before any process launch and never expose a path.
fn admit_attachments(
    attachments: &[LocalImageInput],
    adapter: &RuntimeAdapter,
    remote_transport: bool,
) -> Result<(), RuntimeAdapterError> {
    let image_input = super::registry::projected_capability_fact(
        adapter.id(),
        CapabilityFactName::Ability(NativeAbilityKind::ImageInput),
    );
    if image_input.state != CapabilityFactState::Declared {
        return Err(RuntimeAdapterError::AttachmentUnsupportedForAdapter {
            agent_label: adapter.id().to_string(),
        });
    }
    if remote_transport {
        return Err(RuntimeAdapterError::AttachmentUnsupportedForTransport {
            agent_label: adapter.id().to_string(),
        });
    }
    let mut total_bytes: u64 = 0;
    for attachment in attachments {
        let metadata = fs::symlink_metadata(&attachment.path)
            .map_err(|_| RuntimeAdapterError::AttachmentFileUnavailable)?;
        if metadata.file_type().is_symlink() {
            return Err(RuntimeAdapterError::AttachmentSymlinkRejected);
        }
        if !metadata.file_type().is_file() {
            return Err(RuntimeAdapterError::AttachmentFileUnavailable);
        }
        total_bytes = total_bytes.checked_add(metadata.len()).unwrap_or(u64::MAX);
        if metadata.len() > MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE
            || total_bytes > MAX_IMAGE_ATTACHMENT_BYTES_TOTAL
        {
            return Err(RuntimeAdapterError::AttachmentSizeLimit);
        }
        verify_attachment_signature(Path::new(&attachment.path), &attachment.media_type)?;
    }
    Ok(())
}

fn verify_attachment_signature(path: &Path, media_type: &str) -> Result<(), RuntimeAdapterError> {
    let mut file =
        fs::File::open(path).map_err(|_| RuntimeAdapterError::AttachmentFileUnavailable)?;
    let mut prefix = [0u8; 12];
    let mut read = 0usize;
    loop {
        let chunk = file
            .read(&mut prefix[read..])
            .map_err(|_| RuntimeAdapterError::AttachmentFileUnavailable)?;
        if chunk == 0 {
            break;
        }
        read += chunk;
        if read == prefix.len() {
            break;
        }
    }
    let matches = match media_type {
        "image/png" => read >= 8 && prefix[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "image/jpeg" => read >= 3 && prefix[..3] == [0xFF, 0xD8, 0xFF],
        "image/gif" => read >= 4 && &prefix[..4] == b"GIF8",
        "image/webp" => read >= 12 && &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WEBP",
        _ => false,
    };
    if !matches {
        return Err(RuntimeAdapterError::AttachmentSignatureMismatch);
    }
    Ok(())
}

/// Every capability a Profile requires must be satisfied by a declared fact
/// before any driver runs. A name outside the closed fact vocabulary cannot be
/// satisfied either, so the typed refusal names it as well. A requirement that
/// was silently ignored until now becomes an explicit refusal instead.
fn admit_capability_requirements(
    params: &Value,
    agent_id: &str,
) -> Result<(), RuntimeAdapterError> {
    let Some(required) = params.get("requiredCapabilities") else {
        return Ok(());
    };
    let invalid = || RuntimeAdapterError::InvalidRuntimeSetting {
        field: "requiredCapabilities",
    };
    let required = required.as_array().ok_or_else(invalid)?;
    for value in required {
        let capability = value.as_str().ok_or_else(invalid)?;
        let declared = CapabilityFactName::parse(capability).is_some_and(|name| {
            super::registry::projected_capability_fact(agent_id, name).state
                == CapabilityFactState::Declared
        });
        if !declared {
            return Err(RuntimeAdapterError::CapabilityRequirementUnsatisfied {
                capability: capability.to_owned(),
            });
        }
    }
    Ok(())
}

pub fn send_message(
    port: &licoup_agent_targets::port::AgentTargetPort,
    params: &Value,
) -> Result<Value, RuntimeAdapterError> {
    if params.get("command").is_some() || params.get("args").is_some() {
        return Err(RuntimeAdapterError::LegacyLaunchConfiguration);
    }
    let agent_id = text_param(params, &["agent", "agentId", "target"])
        .filter(|value| !value.is_empty())
        .ok_or(RuntimeAdapterError::AgentIdentifierMissing)?;
    let attachments = parse_attachments(params).map_err(attachment_shape_error)?;
    let text = message_param(params, &["text", "message", "prompt"]).unwrap_or_default();
    if text.trim().is_empty() && attachments.is_empty() {
        return Err(RuntimeAdapterError::MessageMissing);
    }
    admit_capability_requirements(params, &agent_id)?;
    let lane = runtime_lane_for_agent(&agent_id).ok_or_else(|| {
        RuntimeAdapterError::UnsupportedAdapter {
            agent_label: agent_id.clone(),
        }
    })?;
    let adapter = match &lane {
        RuntimeLane::Dedicated(adapter) => *adapter,
        RuntimeLane::GenericCli(registration) => {
            return send_generic_cli(port, params, &agent_id, registration, &text, &attachments);
        }
    };
    // The parser set and the driver registrations are one composition, so an
    // adapter this host composed a driver for is also an adapter it composed a
    // parser for. `require_registered` keeps the original fail-loud contract
    // for a packaged adapter whose parser is missing.
    let Some(registration) = super::port::registration_for_adapter(adapter) else {
        return Err(RuntimeAdapterError::UnsupportedAdapter {
            agent_label: adapter.id().to_string(),
        });
    };
    licoup_agent_adapter_sdk::registry::require_registered(
        &super::port::parser_set(),
        adapter.id(),
    );
    let runtime_connection = SshRuntimeConnection::from_params(params, adapter.id())
        .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)?;
    if !attachments.is_empty() {
        admit_attachments(&attachments, &adapter, runtime_connection.is_some())?;
    }
    let session_id = text_param(params, &["sessionId", "nativeSessionId"]).unwrap_or_default();
    let requested_cwd = text_param(params, &["cwd", "workingDirectory"])
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if runtime_connection.is_some()
        && requested_cwd
            .as_deref()
            .is_some_and(|path| !is_valid_guest_working_directory(path))
    {
        return Err(RuntimeAdapterError::ConversationDispatchFailed);
    }
    // A local agent indexes the directory it runs in, so the client resolves
    // one bounded workspace here and every driver reads it from the same place.
    // A guest working directory belongs to the remote host and stays untouched.
    let cwd = match runtime_connection.as_ref() {
        Some(connection) => {
            requested_cwd.or_else(|| Some(PathBuf::from(connection.working_directory())))
        }
        None => resolve_local_agent_workspace(adapter.id(), requested_cwd.as_deref()),
    };
    let params = match (runtime_connection.is_none(), cwd.as_deref()) {
        (true, Some(workspace)) => Cow::Owned(params_with_workspace(params, workspace)),
        _ => Cow::Borrowed(params),
    };
    let params = params.as_ref();
    // Omission and zero mean "use the writable policy", whose default is
    // unbounded (0). A finite deadline comes only from an explicit caller
    // value or a configured policy entry inside the 1s–30min clamp.
    let timeout_ms = dispatch_timeout(params)
        .map_err(|_| RuntimeAdapterError::InvalidRuntimeSetting { field: "timeoutMs" })?;
    let max_stdout = optional_output_param(params, "maxStdoutBytes").map_err(|_| {
        RuntimeAdapterError::InvalidRuntimeSetting {
            field: "maxStdoutBytes",
        }
    })?;
    let max_stderr = bounded_output_param(params, "maxStderrBytes", DEFAULT_MAX_STDERR_BYTES)
        .map_err(|_| RuntimeAdapterError::InvalidRuntimeSetting {
            field: "maxStderrBytes",
        })?;
    let requested_executable = if adapter == RuntimeAdapter::Codex {
        codex_binary_param(params)
    } else {
        binary_param(params, adapter.default_binary())
    };
    let executable = if runtime_connection.is_some() {
        "ssh".to_string()
    } else {
        runtime_executable(port, adapter, &requested_executable)?
    };

    // Which Agent's driver runs a turn is that Agent's own fact, so it arrives
    // through the registration the composition supplies. The host resolves
    // every argument and reports the result in its own vocabulary; it never
    // names the engine that produced it.
    let execution = (registration.run)(&super::port::AgentRun {
        executable: &executable,
        params,
        prompt: &text,
        session_id: &session_id,
        cwd: cwd.as_deref(),
        timeout_ms,
        max_stdout,
        max_stderr,
        runtime_connection: runtime_connection.as_ref(),
    });

    Ok(execution_response(adapter, execution))
}

fn send_generic_cli(
    port: &licoup_agent_targets::port::AgentTargetPort,
    params: &Value,
    agent_id: &str,
    registration: &licoup_agent_targets::domain::cli_registration::CliRegistration,
    text: &str,
    attachments: &[LocalImageInput],
) -> Result<Value, RuntimeAdapterError> {
    if !attachments.is_empty() {
        return Err(RuntimeAdapterError::AttachmentUnsupportedForAdapter {
            agent_label: agent_id.to_owned(),
        });
    }
    if SshRuntimeConnection::from_params(params, agent_id)
        .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)?
        .is_some()
    {
        return Err(RuntimeAdapterError::ConversationDispatchFailed);
    }
    let requested_cwd = text_param(params, &["cwd", "workingDirectory"])
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let cwd = resolve_local_agent_workspace(agent_id, requested_cwd.as_deref());
    let params = match cwd.as_deref() {
        Some(workspace) => Cow::Owned(params_with_workspace(params, workspace)),
        None => Cow::Borrowed(params),
    };
    let params = params.as_ref();
    let timeout_ms = dispatch_timeout(params)
        .map_err(|_| RuntimeAdapterError::InvalidRuntimeSetting { field: "timeoutMs" })?;
    let max_stdout = optional_output_param(params, "maxStdoutBytes").map_err(|_| {
        RuntimeAdapterError::InvalidRuntimeSetting {
            field: "maxStdoutBytes",
        }
    })?;
    let Some(lane) = super::port::composition().map(|composition| composition.generic_cli) else {
        return Err(RuntimeAdapterError::UnsupportedAdapter {
            agent_label: agent_id.to_owned(),
        });
    };
    let executable = (lane.resolve_executable)(port, registration, params)?;
    let execution = (lane.execute)(
        registration,
        &executable,
        params,
        text,
        cwd.as_deref(),
        timeout_ms,
        max_stdout,
    )?;
    Ok(execution_response_named(
        agent_id,
        &registration.label,
        lane.driver_id,
        normalize_generic_cli(agent_id, params, execution),
    ))
}

fn normalize_generic_cli(
    agent_id: &str,
    params: &Value,
    execution: super::port::GenericCliRun,
) -> super::model::NormalizedExecution {
    use super::model::{NormalizedEffectiveSettings, NormalizedExecution, NormalizedFailure};
    let error = if execution.ok {
        None
    } else {
        Some(NormalizedFailure {
            code: if execution.timed_out {
                "generic_cli_timeout".to_owned()
            } else {
                "generic_cli_failed".to_owned()
            },
            message: if execution.timed_out {
                "generic CLI turn timed out".to_owned()
            } else {
                "generic CLI turn failed".to_owned()
            },
            stage: "runtime/generic-cli".to_owned(),
            component: Some(agent_id.to_owned()),
            retryable: Some(execution.timed_out),
            recovery: None,
            user_interaction_required: false,
            request_method: None,
            session_id: None,
            thread_id: None,
            turn_id: None,
            turn_status: None,
        })
    };
    NormalizedExecution {
        ok: execution.ok,
        output: execution.output,
        transitions: Vec::new(),
        capabilities: serde_json::json!({
            "newSession": false,
            "resumeSession": false,
            "structuredEvents": false,
            "interactiveApprovalBridge": false
        }),
        error,
        session_id: String::new(),
        thread_id: String::new(),
        turn_id: String::new(),
        turn_status: if execution.ok {
            "completed".to_owned()
        } else {
            "failed".to_owned()
        },
        effective: NormalizedEffectiveSettings {
            cwd: text_param(params, &["cwd", "workingDirectory"]),
            model: text_param(params, &["model"]),
            reasoning_effort: text_param(params, &["reasoningEffort", "effort"]),
            ..NormalizedEffectiveSettings::default()
        },
        status_code: execution.status_code,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: false,
        started_at: execution.started_at,
        runtime_protocol: execution.runtime_protocol,
        driver_id: super::port::composition()
            .map(|composition| composition.generic_cli.driver_id)
            .unwrap_or(""),
    }
}

/// Republish the resolved workspace under both request keys so a driver that
/// reads its working directory from the request cannot reach past the bound.
pub fn params_with_workspace(params: &Value, workspace: &Path) -> Value {
    let mut resolved = params.clone();
    if let Some(object) = resolved.as_object_mut() {
        let workspace = Value::String(workspace.to_string_lossy().into_owned());
        object.insert("cwd".to_string(), workspace.clone());
        object.insert("workingDirectory".to_string(), workspace);
    }
    resolved
}
