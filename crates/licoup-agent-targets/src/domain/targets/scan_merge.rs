use super::binaries::{cursor_binary_supports_acp, find_target_binary_with_source};
use super::catalog::{
    TargetCandidate, TargetDef, adapter_capabilities_for, candidate_runtime_is_available,
};
use super::manual::ManualTarget;
use super::model_catalog::{empty_model_catalog, model_catalog_for_target};
use super::parameters::{param_bool, param_string};
use super::platform_paths::{default_config_path_with_params, default_detection_path_with_params};
use super::processes::{ScanContext, running_process_for, target_uses_running_process_detection};
use super::support::display_path;
use super::virtual_machine_discovery::AutomaticVmTarget;
use crate::platform::virtual_machine::SshRuntimeConnection;
use crate::port::AgentTargetPort;
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn scan_target_with_manual(
    port: &AgentTargetPort,
    def: &TargetDef,
    manual: Option<&ManualTarget>,
    automatic_vm: Option<&AutomaticVmTarget>,
    scan_context: &mut ScanContext,
    params: &Value,
) -> Result<TargetCandidate> {
    scan_target_with_manual_projection(
        port,
        def,
        manual,
        automatic_vm,
        scan_context,
        params,
        TargetScanProjection::Full,
    )
}

/// Read the target facts needed for readiness without opening model stores,
/// executing capability probes, or producing a cacheable discovery refresh.
pub(super) fn scan_target_read_only_with_manual(
    port: &AgentTargetPort,
    def: &TargetDef,
    manual: Option<&ManualTarget>,
    automatic_vm: Option<&AutomaticVmTarget>,
    scan_context: &mut ScanContext,
    params: &Value,
) -> Result<TargetCandidate> {
    scan_target_with_manual_projection(
        port,
        def,
        manual,
        automatic_vm,
        scan_context,
        params,
        TargetScanProjection::ReadOnly,
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TargetScanProjection {
    Full,
    ReadOnly,
}

fn scan_target_with_manual_projection(
    port: &AgentTargetPort,
    def: &TargetDef,
    manual: Option<&ManualTarget>,
    automatic_vm: Option<&AutomaticVmTarget>,
    scan_context: &mut ScanContext,
    params: &Value,
    projection: TargetScanProjection,
) -> Result<TargetCandidate> {
    let isolate_automatic_sources = synthetic_discovery_isolation(params);
    if let Some(manual) = manual.filter(|item| item.location == "virtual-machine") {
        return Ok(scan_virtual_machine_target(port, def, manual));
    }
    let config_path = manual
        .and_then(|item| item.config_path.clone())
        .or_else(|| {
            if isolate_automatic_sources {
                None
            } else {
                default_config_path_with_params(def.id, params)
            }
        });
    let manual_binary = manual.and_then(|item| item.binary_path.clone());
    let located_binary = manual_binary
        .filter(|path| {
            if def.id != "cursor" {
                return true;
            }
            // Accept a manual cursor-agent binding even when the short probe
            // fails; reject only the IDE `cursor` shim without Agent CLI help.
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            file_name.starts_with("cursor-agent") || cursor_binary_supports_acp(port, path, params)
        })
        .map(|path| (path, "manual"))
        .or_else(|| {
            if isolate_automatic_sources {
                None
            } else {
                find_target_binary_with_source(def, params)
            }
        });
    let binary_path = located_binary.as_ref().map(|(path, _)| path.clone());
    let binary_source = located_binary.as_ref().map(|(_, source)| *source);
    let detection_path = (!isolate_automatic_sources)
        .then(|| default_detection_path_with_params(def.id, params))
        .flatten();
    let history_roots = manual
        .map(|item| item.history_roots.clone())
        .unwrap_or_default();
    let config_exists = config_path.as_ref().is_some_and(|path| {
        if manual.and_then(|item| item.config_path.as_ref()) == Some(path) {
            crate::domain::targets::scan_paths::explicit_file_exists(path)
        } else {
            crate::domain::targets::scan_paths::probe_exists(path)
        }
    });
    let detection_exists = detection_path
        .as_ref()
        .is_some_and(|path| crate::domain::targets::scan_paths::probe_exists(path));
    let detected_without_process = config_exists || binary_path.is_some() || detection_exists;
    let should_check_process = scan_context.running_processes_injected
        || (!detected_without_process && target_uses_running_process_detection(def.id));
    let running_process = if should_check_process {
        running_process_for(def, scan_context)
    } else {
        None
    };
    let detected =
        config_exists || binary_path.is_some() || detection_exists || running_process.is_some();
    let manual_entry = manual.is_some();
    if manual.is_none()
        && binary_path.is_none()
        && let Some(automatic_vm) = automatic_vm
    {
        return Ok(scan_automatic_virtual_machine_target(
            port,
            def,
            automatic_vm,
        ));
    }
    let configured = config_exists;
    let status = if configured {
        "configured"
    } else if detected {
        "detected"
    } else if manual_entry {
        "manual"
    } else {
        "not-detected"
    };
    let confidence = if configured {
        1.0
    } else if detected {
        0.72
    } else {
        0.15
    };
    let mut detail_parts = Vec::<String>::new();
    detail_parts.push(match (&config_path, &binary_path) {
        (Some(config), Some(binary)) => {
            format!(
                "{}: {}; binary: {}",
                def.config_hint,
                config.display(),
                binary.display()
            )
        }
        (Some(config), None) => format!("{}: {}", def.config_hint, config.display()),
        (None, Some(binary)) => format!("binary: {}", binary.display()),
        (None, None) => def.config_hint.to_string(),
    });
    if let Some(path) = detection_path
        .as_ref()
        .filter(|path| config_path.as_ref() != Some(path))
    {
        detail_parts.push(format!("evidence: {}", path.display()));
    }
    if let Some(process) = running_process.as_deref() {
        detail_parts.push(format!("process: {} running", process));
    }
    let base_detail = detail_parts.join("; ");
    let detail = if manual_entry {
        format!("Manual entry: {}", base_detail)
    } else {
        base_detail
    };
    let mut capabilities = adapter_capabilities_for(port, def.id);
    if projection == TargetScanProjection::Full
        && let Some(binary) = binary_path.as_deref()
        && param_bool(params, "probeConversationRuntime") == Some(true)
    {
        let probe_cwd = param_string(params, "workingDirectory")
            .or_else(|| param_string(params, "cwd"))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| (port.default_local_agent_workspace)(def.id));
        if let Some(probe_cwd) = probe_cwd.as_deref() {
            capabilities.conversation_probe =
                (port.probe_runtime_driver)(def.id, binary, probe_cwd);
        }
    } else if binary_path.is_some() {
        capabilities.conversation_probe = json!({
            "available": true,
            "supported": false,
            "errorCode": "probe_not_run"
        });
    } else {
        capabilities.conversation_probe = json!({
            "available": false,
            "supported": false,
            "errorCode": "runtime_not_detected"
        });
    }

    let runtime_available =
        candidate_runtime_is_available(port, &mut capabilities, def.id, binary_path.as_deref());
    let adapter_status = "implemented";
    let model_catalog = if projection == TargetScanProjection::ReadOnly {
        None
    } else if detected || manual_entry {
        let catalog_params = model_catalog_params(def.id, binary_path.as_deref(), params);
        Some(model_catalog_for_target(
            port,
            def.id,
            config_path.as_deref(),
            &catalog_params,
        ))
    } else {
        Some(empty_model_catalog("unavailable", "not-detected"))
    };

    let mut supported_actions = Vec::new();
    if runtime_available {
        supported_actions.push("runtime.message.send".to_string());
    }

    Ok(TargetCandidate {
        id: Some(def.id.to_string()),
        target: def.id.to_string(),
        label: manual
            .map(|item| item.label.clone())
            .unwrap_or_else(|| def.label.to_string()),
        kind: manual
            .map(|item| item.kind.clone())
            .unwrap_or_else(|| def.kind.to_string()),
        status: status.to_string(),
        configured,
        confidence,
        detail,
        config_path: config_path.map(display_path),
        binary_path: binary_path.map(display_path),
        history_roots: history_roots.into_iter().map(display_path).collect(),
        location: "local".to_string(),
        runtime_connection: None,
        manual: manual_entry,
        adapter_status: adapter_status.to_string(),
        adapter_capabilities: capabilities,
        supported_actions,
        scan_source: Some(
            if manual_entry {
                "manual"
            } else if let Some(source) = binary_source {
                source
            } else if config_exists || detection_exists {
                "local-configuration"
            } else if running_process.is_some() {
                "running-process"
            } else {
                "host-local-discovery"
            }
            .to_string(),
        ),
        model_catalog,
    })
}

// A caller may ask a scan to ignore every automatic host source and answer from
// the process snapshot alone. It is a test seam, and `cfg(test)` is false for a
// dependency: while this tree lived inside `licoup-native` the attribute was
// true, and once it moved out the seam silently stopped working and the fixture
// began discovering the developer's real machine. It travels as an explicit
// feature, which is `licoup-foundation`'s pattern, and native's test build
// enables it.
#[cfg(any(test, feature = "test-support"))]
fn synthetic_discovery_isolation(params: &Value) -> bool {
    param_bool(params, "syntheticDiscoveryIsolation") == Some(true)
}

#[cfg(not(any(test, feature = "test-support")))]
fn synthetic_discovery_isolation(_params: &Value) -> bool {
    false
}

fn scan_virtual_machine_target(
    port: &AgentTargetPort,
    def: &TargetDef,
    manual: &ManualTarget,
) -> TargetCandidate {
    project_virtual_machine_target(
        port,
        def,
        &manual.label,
        &manual.kind,
        manual.runtime_connection.as_ref(),
        true,
        "Manual virtual machine connection; runtime validation is deferred until use.",
        "Manual virtual machine connection is incomplete.",
        "virtual-machine-ssh",
    )
}

pub(super) fn scan_automatic_virtual_machine_target(
    port: &AgentTargetPort,
    def: &TargetDef,
    automatic: &AutomaticVmTarget,
) -> TargetCandidate {
    project_virtual_machine_target(
        port,
        def,
        &automatic.label,
        def.kind,
        Some(&automatic.runtime_connection),
        false,
        "Detected in an accessible local virtual machine; runtime validation is deferred until use.",
        "Automatic virtual machine connection is unavailable.",
        "virtual-machine-orbstack",
    )
}

#[allow(clippy::too_many_arguments)]
fn project_virtual_machine_target(
    port: &AgentTargetPort,
    def: &TargetDef,
    label: &str,
    kind: &str,
    runtime_connection: Option<&SshRuntimeConnection>,
    manual: bool,
    ready_detail: &str,
    unavailable_detail: &str,
    scan_source: &str,
) -> TargetCandidate {
    let mut capabilities = adapter_capabilities_for(port, def.id);
    let runtime_ready =
        runtime_connection.is_some() && (port.runtime_driver_profile)(def.id).is_some();
    let uses_hermes_gateway =
        runtime_connection.is_some_and(SshRuntimeConnection::is_hermes_tui_gateway);
    if uses_hermes_gateway {
        capabilities.conversation_protocol =
            licoup_foundation::core::acp::HERMES_TUI_GATEWAY_PROTOCOL.to_string();
        if let Some(matrix) = capabilities.conversation_capability_matrix.as_object_mut() {
            matrix.insert("laneFamily".to_string(), json!("rpc"));
            matrix.insert("cancel".to_string(), json!(false));
            matrix.insert("interruptSteer".to_string(), json!(false));
        }
    }
    capabilities.conversation_probe = if runtime_ready {
        json!({
            "available": true,
            "supported": false,
            "errorCode": "probe_not_run",
            "transport": "ssh-stdio",
            "protocol": if uses_hermes_gateway {
                licoup_foundation::core::acp::HERMES_TUI_GATEWAY_PROTOCOL
            } else {
                "acp"
            }
        })
    } else {
        json!({
            "available": false,
            "supported": false,
            "errorCode": "virtual_machine_connection_invalid"
        })
    };
    if runtime_ready {
        capabilities.conversation_blocker = None;
    }
    let supported_actions = runtime_ready
        .then(|| vec!["runtime.message.send".to_string()])
        .unwrap_or_default();
    TargetCandidate {
        id: Some(def.id.to_string()),
        target: def.id.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        status: if runtime_ready {
            if manual { "configured" } else { "detected" }
        } else {
            if manual { "manual" } else { "not-detected" }
        }
        .to_string(),
        configured: runtime_ready,
        confidence: if runtime_ready {
            if manual { 1.0 } else { 0.92 }
        } else {
            0.15
        },
        detail: if runtime_ready {
            ready_detail
        } else {
            unavailable_detail
        }
        .to_string(),
        config_path: None,
        binary_path: runtime_connection
            .map(|connection| connection.remote_executable().to_string()),
        history_roots: Vec::new(),
        location: "virtual-machine".to_string(),
        runtime_connection: runtime_connection.map(|connection| connection.to_value()),
        manual,
        adapter_status: "implemented".to_string(),
        adapter_capabilities: capabilities,
        supported_actions,
        scan_source: Some(scan_source.to_string()),
        model_catalog: Some(empty_model_catalog(
            "unavailable",
            "virtual-machine-runtime-deferred",
        )),
    }
}

/// Reuse the executable already bound for this candidate so selected-agent
/// model lookup does not depend on a second PATH search. This is required for
/// package-bundled CLIs such as Kilo Code's extension binary.
pub(super) fn model_catalog_params(
    target: &str,
    binary_path: Option<&Path>,
    params: &Value,
) -> Value {
    let mut next = params.clone();
    let Some(binary_path) = binary_path else {
        return next;
    };
    let parameter = match target {
        "antigravity" => "antigravityCliPath",
        "claude-code" => "claudeCliPath",
        "codex" => "codexCliPath",
        "cursor" => "cursorCliPath",
        "deepseek-harness" => "deepseekHarnessCliPath",
        "kilo-code" => "kiloCliPath",
        "opencode" => "opencodeCliPath",
        "pi" => "piCliPath",
        _ => return next,
    };
    if let Some(object) = next.as_object_mut() {
        object
            .entry(parameter)
            .or_insert_with(|| json!(display_path(binary_path.to_path_buf())));
    }
    next
}
