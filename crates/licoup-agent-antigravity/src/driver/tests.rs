use super::*;
use serde_json::{Value, json};
use std::fs;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(unix)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
fn environment_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Install the two ports this package's turn asks its host for, the way the host
/// installs them.
///
/// One Antigravity turn emits through [`crate::port::turn_event`] and reads its
/// admission answer from [`crate::port::execution`]; both are fail-closed before
/// a host answers, which is the honest state for a package running with no
/// composition above it. This suite drives the turn, so it answers both ports
/// with the host's own answers: the foundation turn-event emitters the host
/// composes (`platform::antigravity_turn_event_port`) and an open admission,
/// because a unit-test process has no close-admission barrier to hold. What a
/// *closed* answer does to a turn is asserted in `tests/closed_admission.rs`,
/// whose own process installs that answer. One installation per process, exactly
/// as the host installs once.
#[cfg(unix)]
fn install_driver_ports() {
    static PORTS: OnceLock<()> = OnceLock::new();
    PORTS.get_or_init(|| {
        crate::port::turn_event::install(crate::port::turn_event::TurnEventPort {
            emit_turn_event: licoup_foundation::platform::turn_event_emit::emit_turn_event,
            emit_agent_message_chunk:
                licoup_foundation::platform::turn_event_emit::emit_agent_message_chunk,
            emit_agent_message_completed:
                licoup_foundation::platform::turn_event_emit::emit_agent_message_completed,
            emit_agent_processing:
                licoup_foundation::platform::turn_event_emit::emit_agent_processing,
        })
        .expect("the driver suite installs its own turn-event port once");
        crate::port::execution::install(crate::port::execution::ExecutionPort {
            subagent_caller_context: || None,
            admits_execution: || true,
        })
        .expect("the driver suite installs its own execution port once");
    });
}

#[test]
fn missing_executable_is_unavailable_and_never_supported() {
    let capability = probe("lico-antigravity-driver-definitely-not-installed", 50, 1024);
    assert!(!capability.available);
    assert!(!capability.supported);
    assert_eq!(
        capability.error_code,
        Some("antigravity_executable_unavailable")
    );
}

#[test]
fn private_instructions_fail_before_process_launch() {
    install_driver_ports();
    let result = execute(
        "definitely-not-a-real-antigravity",
        &json!({"privateInstructions":"private sentinel"}),
        "exact user prompt",
        "session-secret-sentinel",
        Some(std::env::temp_dir().as_path()),
        1_000,
        None,
        1_024,
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("antigravity_private_instructions_unsupported")
    );
}

#[cfg(unix)]
#[test]
fn uninstall_removes_only_lico_hook_namespace() {
    let _environment_guard = environment_lock();
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-uninstall-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&gemini).unwrap();
    let previous = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-uninstall-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let fixture = FakeExecutable::new("uninstall", true);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let _ = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({}),
        "hello",
        "",
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );
    let hooks_path = gemini.join("hooks.json");
    let before = fs::read_to_string(&hooks_path).unwrap();
    assert!(before.contains("lico-up-antigravity-session"));
    // Preserve an unrelated user hook entry.
    let mut root: serde_json::Value = serde_json::from_str(&before).unwrap();
    root.as_object_mut().unwrap().insert(
        "user-hook".to_string(),
        json!({"enabled": true, "Stop": []}),
    );
    fs::write(&hooks_path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
    uninstall_hook_bridge().unwrap();
    let after = fs::read_to_string(&hooks_path).unwrap();
    assert!(!after.contains("lico-up-antigravity-session"));
    assert!(after.contains("user-hook"));
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
}

#[test]
fn empty_prompt_fails_closed_without_echoing_secrets() {
    let prompt = "";
    let cwd = "/workspace/path-secret-sentinel";
    let session_id = "session-secret-sentinel";
    install_driver_ports();
    let result = execute(
        "agy",
        &json!({}),
        prompt,
        session_id,
        Some(Path::new(cwd)),
        1_000,
        Some(1_024),
        1_024,
    );
    assert!(!result.ok);
    let message = result.error.unwrap().message;
    assert!(!message.contains(cwd));
    assert!(!message.contains(session_id));
}

#[cfg(unix)]
#[test]
fn probe_detects_official_print_and_conversation_surface() {
    let fixture = FakeExecutable::new("probe", true);
    let capability = probe(fixture.executable.to_string_lossy().as_ref(), 5_000, 8_192);
    assert!(capability.available);
    assert!(capability.supported);
    assert!(capability.new_session);
    assert!(capability.resume_session);
    assert_eq!(capability.error_code, None);
}

#[cfg(unix)]
#[test]
fn execute_reads_hook_receipt_and_returns_session_output() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let fixture = FakeExecutable::new("execute", true);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let result = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({"model": "gemini-test"}),
        "hello-from-lico",
        "",
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.session_id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(result.output, "PONG");
    assert!(!result.transitions.is_empty());
}

#[cfg(unix)]
#[test]
fn execute_streams_pty_chunks_before_completion() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_target = std::sync::Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;

    let fixture = FakeExecutable::new_streaming("streaming");
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let result = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({}),
        "hello-from-lico",
        "",
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }

    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.session_id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(result.output, "first\nsecond");

    let events = captured.lock().unwrap().clone();
    let accepted_at = events
        .iter()
        .position(|event| event["event"] == "agent.turn.accepted");
    let first_chunk_at = events
        .iter()
        .position(|event| event["event"] == "agent.message.chunk");
    assert!(
        accepted_at.is_some_and(|accepted| first_chunk_at.is_some_and(|chunk| accepted < chunk)),
        "accepted must precede the first chunk: {events:?}"
    );
    let chunks: Vec<_> = events
        .iter()
        .filter(|event| event["event"] == "agent.message.chunk")
        .collect();
    assert_eq!(
        chunks.len(),
        1,
        "new sessions publish after identity binding: {events:?}"
    );
    let joined: String = chunks
        .iter()
        .map(|event| event["payload"]["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(joined, "first\nsecond");
    assert!(
        chunks.iter().all(
            |event| event["sessionId"].as_str() == Some("11111111-2222-3333-4444-555555555555")
        ),
        "new-session chunks carry the Stop-hook native session id"
    );
    let completed_at = events.iter().position(|value| {
        value["event"] == "agent.message.completed"
            && value["sessionId"].as_str() == Some("11111111-2222-3333-4444-555555555555")
    });
    assert!(
        completed_at.is_some_and(|completed_at| {
            first_chunk_at.is_some_and(|chunk_at| chunk_at < completed_at)
        }),
        "completed must follow the chunks (the fake only exits after 'second'): {events:?}"
    );
}

/// A cancelled resume must not publish `agent.message.completed` or post-cancel
/// stdout as assistant text, even when the vendor CLI writes a Stop-hook
/// receipt and exits 0 after SIGTERM. Pre-cancel chunks may remain.
#[cfg(unix)]
#[test]
fn cancelled_resume_does_not_emit_completed() {
    let session_id = unique_session_token("resume");
    let (result, events) = execute_and_cancel("cancel-resume", &session_id, json!({}), &session_id);
    assert_cancelled_without_completed(&result, &events);
}

/// A new conversation buffers until the Stop-hook binds. Cancel on the private
/// control identity must still suppress that buffered completion flush.
#[cfg(unix)]
#[test]
fn cancelled_new_conversation_does_not_emit_completed() {
    let control_id = unique_session_token("new");
    let dispatch_id = format!("subagent:{control_id}");
    let (result, events) = execute_and_cancel(
        "cancel-new",
        "",
        json!({"dispatchId": dispatch_id}),
        DEFAULT_RECEIPT_ID,
    );
    assert_cancelled_without_completed(&result, &events);
}

#[cfg(unix)]
fn unique_session_token(label: &str) -> String {
    format!(
        "agy-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

#[cfg(unix)]
fn execute_and_cancel(
    label: &str,
    session_id: &str,
    params: Value,
    receipt_id: &str,
) -> (RunResult, Vec<Value>) {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let fixture = FakeExecutable::new_hold_for_cancel(label, receipt_id);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let executable = fixture.executable.to_string_lossy().into_owned();
    let workspace_for_turn = workspace.clone();
    let session_for_turn = session_id.to_owned();
    let params_for_turn = params;
    let cancel_id = if session_id.is_empty() {
        params_for_turn
            .get("dispatchId")
            .and_then(Value::as_str)
            .and_then(|dispatch_id| dispatch_id.rsplit(':').next())
            .unwrap_or_default()
            .to_owned()
    } else {
        session_id.to_owned()
    };
    let handle = std::thread::spawn(move || {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink_target = Arc::clone(&captured);
        licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
            sink_target.lock().unwrap().push(event);
        }));
        let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;
        install_driver_ports();
        let result = execute(
            &executable,
            &params_for_turn,
            "synthetic-cancel-prompt",
            &session_for_turn,
            Some(workspace_for_turn.as_path()),
            30_000,
            Some(8_192),
            8_192,
        );
        let events = captured.lock().unwrap().clone();
        (result, events)
    });
    let mut accepted = false;
    for _ in 0..160 {
        if cancel(&cancel_id) == ControlDisposition::Accepted {
            accepted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (result, events) = handle.join().unwrap();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    // A pending cancel can interrupt on register and be consumed before this
    // poll observes Accepted. Settling as cancelled without a finish marker is
    // the product proof in that race.
    assert!(
        accepted || result.turn_status == "cancelled",
        "cancel was neither accepted nor settled cancelled: {:?}",
        result.error.as_ref().map(|error| error.code)
    );
    (result, events)
}

#[cfg(unix)]
fn assert_cancelled_without_completed(result: &RunResult, events: &[Value]) {
    assert!(
        !result.ok,
        "a cancelled turn must not report success: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("antigravity_cli_cancelled")
    );
    assert_eq!(result.turn_status, "cancelled");
    assert!(
        events
            .iter()
            .all(|event| event["event"] != "agent.message.completed"),
        "cancelled turns must not emit a finish marker: {events:?}"
    );
    assert!(
        events.iter().all(|event| {
            !event["payload"]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("post-cancel-output")
        }),
        "post-cancel stdout must not become assistant text: {events:?}"
    );
}

#[cfg(unix)]
#[test]
fn execute_with_zero_timeout_runs_to_completion() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let fixture = FakeExecutable::new("zero-timeout", true);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let result = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({}),
        "hello-from-lico",
        "",
        Some(&workspace),
        0,
        Some(8_192),
        8_192,
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.output, "PONG");
}

#[cfg(unix)]
#[test]
fn execute_resume_binds_exact_requested_conversation() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let requested = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let fixture =
        FakeExecutable::with_receipt_style("resume", true, ReceiptStyle::Direct, requested);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let result = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({}),
        "hello-from-lico",
        requested,
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.session_id, requested);
}

#[cfg(unix)]
#[test]
fn execute_resume_rejects_receipt_drift() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let requested = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let fixture =
        FakeExecutable::with_receipt_style("drift", true, ReceiptStyle::Direct, DEFAULT_RECEIPT_ID);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let result = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({}),
        "hello-from-lico",
        requested,
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "antigravity_cli_session_drift");
}

#[cfg(unix)]
#[test]
fn execute_reads_legacy_wrapped_receipt_for_compatibility() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    let fixture = FakeExecutable::with_receipt_style(
        "wrapped",
        true,
        ReceiptStyle::Wrapped,
        DEFAULT_RECEIPT_ID,
    );
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    install_driver_ports();
    let result = execute(
        fixture.executable.to_string_lossy().as_ref(),
        &json!({}),
        "hello-from-lico",
        "",
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.session_id, DEFAULT_RECEIPT_ID);
}

#[cfg(unix)]
#[test]
fn antigravity_effective_settings_match_executed_command() {
    let _environment_guard = environment_lock();
    let portable = std::env::temp_dir().join(format!(
        "lico-agy-effective-portable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-effective-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&portable).unwrap();
    fs::create_dir_all(&gemini).unwrap();
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }
    let previous_capture = std::env::var_os("ARGV_CAPTURE_PATH");

    let row = |label: &str| -> (ArgvCapturingExecutable, PathBuf) {
        let fixture = ArgvCapturingExecutable::new(label, DEFAULT_RECEIPT_ID);
        let workspace = fixture.root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        unsafe {
            std::env::set_var("ARGV_CAPTURE_PATH", &fixture.argv_path);
        }
        (fixture, workspace)
    };
    let launch = |fixture: &ArgvCapturingExecutable,
                  workspace: &Path,
                  params: &Value,
                  session_id: &str| {
        // Pin the row's argv-capture channel into the launch snapshot;
        // the production shell snapshot would drop it.
        let _pin =
                licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
        install_driver_ports();
        execute(
            fixture.executable_str(),
            params,
            "hello-from-lico",
            session_id,
            Some(workspace),
            5_000,
            Some(8_192),
            8_192,
        )
    };
    let assert_token = |argv: &[String], token: &str| {
        assert!(
            argv.iter().any(|arg| arg == token),
            "argv {argv:?} must contain {token:?}"
        );
    };
    let assert_absent = |argv: &[String], prefix: &str| {
        assert!(
            !argv.iter().any(|arg| arg.starts_with(prefix)),
            "argv {argv:?} must not contain a {prefix:?} token"
        );
    };

    // Omitted policy: the actual dangerous-skip launch remains the default and
    // the effective report names exactly that execution.
    let (fixture, workspace) = row("omitted");
    let result = launch(&fixture, &workspace, &json!({}), "");
    assert!(result.ok, "{:?}", result.error);
    let argv = fixture.captured_argv();
    assert_token(&argv, "--print=hello-from-lico");
    assert_token(&argv, "--dangerously-skip-permissions");
    assert_token(&argv, &format!("--add-dir={}", workspace.display()));
    assert_absent(&argv, "--model=");
    assert_absent(&argv, "--effort=");
    assert_absent(&argv, "--sandbox");
    assert_absent(&argv, "--conversation=");
    assert_eq!(
        result.effective.permission_mode.as_deref(),
        Some("dangerously-skip-permissions")
    );
    assert_eq!(result.effective.approval_policy, None);
    assert_eq!(result.effective.sandbox, Some(json!(false)));
    assert_eq!(result.effective.model, None);
    assert_eq!(result.effective.reasoning_effort, None);
    assert_eq!(
        result.effective.cwd.as_deref(),
        Some(workspace.to_str().unwrap())
    );

    // The dangerous-skip alias and every other supported option map one-to-one
    // onto executed argv and the effective projection.
    let (fixture, workspace) = row("explicit");
    let result = launch(
        &fixture,
        &workspace,
        &json!({
            "permissionMode": "dangerously-skip-permissions",
            "approvalPolicy": "dangerously-skip-permissions",
            "model": "gemini-2.5-pro",
            "reasoningEffort": "high",
            "sandbox": true
        }),
        "",
    );
    assert!(result.ok, "{:?}", result.error);
    let argv = fixture.captured_argv();
    assert_token(&argv, "--dangerously-skip-permissions");
    assert_token(&argv, "--model=gemini-2.5-pro");
    assert_token(&argv, "--effort=high");
    assert_token(&argv, "--sandbox");
    assert_eq!(
        result.effective.permission_mode.as_deref(),
        Some("dangerously-skip-permissions")
    );
    assert_eq!(
        result.effective.approval_policy,
        Some(json!("dangerously-skip-permissions"))
    );
    assert_eq!(result.effective.sandbox, Some(json!(true)));
    assert_eq!(result.effective.model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(result.effective.reasoning_effort.as_deref(), Some("high"));

    // Unsupported approval policies fail before any process starts.
    for (label, params) in [
        ("unsupported-manual", json!({"approvalPolicy": "manual"})),
        ("unsupported-safe", json!({"permissionMode": "safe"})),
        ("unsupported-auto", json!({"approvalPolicy": "auto"})),
    ] {
        let (fixture, workspace) = row(label);
        let result = launch(&fixture, &workspace, &params, "");
        assert!(!result.ok, "{label} must be rejected");
        assert_eq!(
            result.error.unwrap().code,
            "antigravity_permission_policy_unsupported",
            "{label}"
        );
        assert!(
            !fixture.launched(),
            "{label} must fail before the CLI is launched"
        );
    }

    // A non-boolean sandbox choice is also rejected rather than silently
    // dropped while a different execution is reported.
    let (fixture, workspace) = row("sandbox-string");
    let result = launch(&fixture, &workspace, &json!({"sandbox": "browser"}), "");
    assert!(!result.ok);
    assert_eq!(
        result.error.unwrap().code,
        "antigravity_sandbox_unsupported"
    );
    assert!(!fixture.launched());

    // Resume identity maps into exactly one --conversation in argv and stays
    // the returned durable identity.
    let resume_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let fixture = ArgvCapturingExecutable::new("resume", resume_id);
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    unsafe {
        std::env::set_var("ARGV_CAPTURE_PATH", &fixture.argv_path);
    }
    let result = launch(
        &fixture,
        &workspace,
        &json!({"model": "gemini-test"}),
        resume_id,
    );
    assert!(result.ok, "{:?}", result.error);
    let argv = fixture.captured_argv();
    assert_token(&argv, &format!("--conversation={resume_id}"));
    assert_token(&argv, "--model=gemini-test");
    assert_eq!(result.session_id, resume_id);

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    if let Some(value) = previous_capture {
        unsafe {
            std::env::set_var("ARGV_CAPTURE_PATH", value);
        }
    } else {
        unsafe {
            std::env::remove_var("ARGV_CAPTURE_PATH");
        }
    }
}

#[cfg(unix)]
#[test]
fn hook_bridge_installs_the_package_program_as_the_stop_hook() {
    let _environment_guard = environment_lock();
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-hook-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&gemini).unwrap();
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    ensure_hook_bridge().unwrap();
    let hooks_path = gemini.join("hooks.json");
    let hooks_json = fs::read_to_string(&hooks_path).unwrap();
    let root: serde_json::Value = serde_json::from_str(&hooks_json).unwrap();
    let namespace = root
        .get("lico-up-antigravity-session")
        .expect("the Lico namespace is registered");
    // Only Stop is installed: an earlier lifecycle hook can overwrite the
    // receipt with an empty identity.
    assert!(namespace.get("Stop").is_some());
    assert!(namespace.get("SessionStart").is_none());
    let command = namespace["Stop"][0]["command"]
        .as_str()
        .expect("the Stop hook names one command");
    // The acceptance this node states: the receipt hook is a native package
    // subcommand. There is no generated script and no interpreter in the path.
    assert!(
        command.contains("lico-agent-antigravity"),
        "the hook runs the adapter package program: {command}"
    );
    assert!(
        command.ends_with(" receipt"),
        "the hook runs the package's receipt subcommand: {command}"
    );
    assert!(!command.contains("python"), "no interpreter: {command}");
    assert!(!command.contains(".sh"), "no generated script: {command}");

    // A previous client's generated script is removed, not merely unreferenced.
    let retired = gemini
        .join("lico-up-antigravity")
        .join("session-receipt-hook.sh");
    assert!(!retired.exists(), "the retired hook script is not on disk");

    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
}

#[cfg(unix)]
#[test]
fn hook_bridge_replaces_a_retired_generated_script() {
    let _environment_guard = environment_lock();
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-hook-retire-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&gemini).unwrap();
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    // A machine that ran the script version still has it on disk, and an
    // unrelated file in the same directory is not this client's to remove.
    let directory = gemini.join("lico-up-antigravity");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("session-receipt-hook.sh"),
        "#!/bin/sh\necho retired\n",
    )
    .unwrap();
    fs::write(directory.join("user-notes.txt"), "keep me\n").unwrap();

    ensure_hook_bridge().unwrap();
    assert!(
        !directory.join("session-receipt-hook.sh").exists(),
        "the retired script is removed on install"
    );
    assert!(
        directory.join("user-notes.txt").exists(),
        "a file this client did not write is never removed with it"
    );

    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
}

#[cfg(unix)]
#[test]
fn hook_bridge_status_reports_the_program_the_hook_names() {
    let _environment_guard = environment_lock();
    let gemini = std::env::temp_dir().join(format!(
        "lico-agy-hook-status-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&gemini).unwrap();
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }

    // Before installation the bridge reports itself absent rather than claiming
    // a hook the vendor client cannot see.
    let before = super::hook_bridge_status();
    assert_eq!(before["installed"], json!(false));
    assert_eq!(before["hookRegistered"], json!(false));

    ensure_hook_bridge().unwrap();
    let after = super::hook_bridge_status();
    assert_eq!(after["hookRegistered"], json!(true));
    assert!(after["hookCommand"].as_str().is_some());
    assert_eq!(
        after["installed"],
        json!(after["scriptInstalled"].as_bool().unwrap()),
        "the hook is installed exactly when the program it names is present"
    );

    uninstall_hook_bridge().unwrap();
    let removed = super::hook_bridge_status();
    assert_eq!(removed["hookRegistered"], json!(false));

    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
}

#[cfg(unix)]
const DEFAULT_RECEIPT_ID: &str = "11111111-2222-3333-4444-555555555555";

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ReceiptStyle {
    /// One direct JSON object `{"conversationId": "<id>"}` — the format the
    /// vendor CLI and the installed LicoUp Stop-hook write today.
    Direct,
    /// Legacy LicoUp hook wrapper (`hookPayload` + `environmentConversationId`),
    /// retained as a compatible input only.
    Wrapped,
}

#[cfg(unix)]
impl ReceiptStyle {
    /// The shell body a fake vendor CLI runs to write one receipt.
    ///
    /// It is a shell fragment rather than a `python3` program on purpose: the
    /// receipt writers under test stand in for the vendor client, and a fixture
    /// that needed an interpreter would test a machine this product no longer
    /// requires.
    ///
    /// The wrapped receipt carries its payload as a JSON *string*, so the
    /// document itself contains backslash escapes. A quoted here-document
    /// writes those bytes verbatim; a `printf` format string would have the
    /// shell consume the escapes and emit a document that is not JSON at all,
    /// which is not the input this compatibility case is about.
    fn writer_body(self, receipt_id: &str) -> String {
        match self {
            ReceiptStyle::Direct => {
                format!(r#"printf '{{"conversationId":"%s"}}\n' '{receipt_id}' > "$1""#)
            }
            ReceiptStyle::Wrapped => format!(
                r#"cat > "$1" <<'RECEIPT'
{{"hookPayload":"{{\"conversationId\":\"{receipt_id}\"}}","environmentConversationId":""}}
RECEIPT
"#
            ),
        }
    }
}

#[cfg(unix)]
struct FakeExecutable {
    root: PathBuf,
    executable: PathBuf,
}

#[cfg(unix)]
impl FakeExecutable {
    fn new(label: &str, emit_receipt: bool) -> Self {
        Self::with_receipt_style(
            label,
            emit_receipt,
            ReceiptStyle::Direct,
            DEFAULT_RECEIPT_ID,
        )
    }

    fn with_receipt_style(
        label: &str,
        emit_receipt: bool,
        style: ReceiptStyle,
        receipt_id: &str,
    ) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lico-antigravity-driver-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-agy");
        let script = if emit_receipt {
            let writer = style.writer_body(receipt_id);
            format!(
                r#"#!/bin/sh
set -eu
for arg in "$@"; do
  case "$arg" in
    --help)
      printf '%s\n' '--print --conversation --model --effort --dangerously-skip-permissions'
      exit 0
      ;;
    --version)
      printf '%s\n' '1.1.5'
      exit 0
      ;;
  esac
done
receipt="${{LICO_ANTIGRAVITY_SESSION_RECEIPT:?}}"
sh -s "$receipt" <<'SH'
{writer}
SH
printf '%s\n' 'PONG'
exit 0
"#
            )
        } else {
            r#"#!/bin/sh
printf '%s\n' "$@"
exit 0
"#
            .to_string()
        };
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        Self { root, executable }
    }

    /// Writes the receipt, then prints `first`, sleeps, and prints `second`
    /// before exiting — proving the pty lane delivers progressive chunks
    /// before the process completes.
    fn new_streaming(label: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lico-antigravity-driver-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-agy");
        let writer = ReceiptStyle::Direct.writer_body(DEFAULT_RECEIPT_ID);
        let script = format!(
            r#"#!/bin/sh
set -eu
for arg in "$@"; do
  case "$arg" in
    --help)
      printf '%s\n' '--print --conversation --model --effort --dangerously-skip-permissions'
      exit 0
      ;;
    --version)
      printf '%s\n' '1.1.5'
      exit 0
      ;;
  esac
done
receipt="${{LICO_ANTIGRAVITY_SESSION_RECEIPT:?}}"
sh -s "$receipt" <<'SH'
{writer}
SH
printf '%s\n' 'first'
sleep 0.4
printf '%s\n' 'second'
exit 0
"#
        );
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        Self { root, executable }
    }

    /// Prints pre-cancel text, holds, then on SIGTERM prints post-cancel text
    /// and writes a Stop-hook receipt with exit 0. Post-cancel stdout must not
    /// become assistant text after the cancel claim.
    fn new_hold_for_cancel(label: &str, receipt_id: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lico-antigravity-driver-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-agy");
        let writer = ReceiptStyle::Direct.writer_body(receipt_id);
        let script = format!(
            r#"#!/bin/sh
set -eu
for arg in "$@"; do
  case "$arg" in
    --help)
      printf '%s\n' '--print --conversation --model --effort --dangerously-skip-permissions'
      exit 0
      ;;
    --version)
      printf '%s\n' '1.1.5'
      exit 0
      ;;
    models)
      exit 0
      ;;
  esac
done
receipt="${{LICO_ANTIGRAVITY_SESSION_RECEIPT:?}}"
write_receipt() {{
  sh -s "$receipt" <<'SH'
{writer}
SH
}}
printf '%s\n' 'pre-cancel-output'
trap 'printf "%s\n" "post-cancel-output"; write_receipt; exit 0' TERM
i=0
while [ "$i" -lt 400 ]; do
  i=$((i + 1))
  sleep 0.05
done
write_receipt
printf '%s\n' 'uncancelled-output'
exit 0
"#,
            writer = writer,
        );
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        Self { root, executable }
    }
}

#[cfg(unix)]
impl Drop for FakeExecutable {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
struct ArgvCapturingExecutable {
    root: PathBuf,
    executable: PathBuf,
    argv_path: PathBuf,
}

#[cfg(unix)]
impl ArgvCapturingExecutable {
    /// An authorized fake that records every launcher argument and writes a
    /// direct hook receipt with `receipt_id` before printing PONG.
    fn new(label: &str, receipt_id: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lico-antigravity-argv-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-agy");
        let argv_path = root.join("argv.log");
        let writer = ReceiptStyle::Direct.writer_body(receipt_id);
        let script = format!(
            r#"#!/bin/sh
set -eu
for arg in "$@"; do
  case "$arg" in
    --help)
      printf '%s\n' '--print --conversation --model --effort --dangerously-skip-permissions'
      exit 0
      ;;
    --version)
      printf '%s\n' '1.1.5'
      exit 0
      ;;
    models)
      exit 0
      ;;
  esac
done
printf '%s\n' "$@" > "$ARGV_CAPTURE_PATH"
receipt="${{LICO_ANTIGRAVITY_SESSION_RECEIPT:?}}"
sh -s "$receipt" <<'SH'
{writer}
SH
printf '%s\n' 'PONG'
exit 0
"#,
            writer = writer,
        );
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        Self {
            root,
            executable,
            argv_path,
        }
    }

    fn executable_str(&self) -> &str {
        self.executable.to_str().unwrap()
    }

    fn captured_argv(&self) -> Vec<String> {
        fs::read_to_string(&self.argv_path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn launched(&self) -> bool {
        self.argv_path.exists()
    }
}

#[cfg(unix)]
impl Drop for ArgvCapturingExecutable {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
struct AuthFakeExecutable {
    root: PathBuf,
    executable: PathBuf,
    print_marker: PathBuf,
}

#[cfg(unix)]
impl AuthFakeExecutable {
    /// `mode`: `logged_out` (models exit 1), `authorized` (models exit 0),
    /// `authorize_flow` (models fails until a print turn creates the login
    /// flag), or `authorize_incomplete` (print never completes the login).
    fn new(label: &str, mode: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lico-antigravity-auth-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-agy");
        let print_marker = root.join("print-invocations.log");
        let login_flag = root.join("login-complete.flag");
        let writer = ReceiptStyle::Direct.writer_body(DEFAULT_RECEIPT_ID);
        let script = format!(
            r#"#!/bin/sh
set -eu
mode="{mode}"
marker="{marker}"
login_flag="{login_flag}"
for arg in "$@"; do
  case "$arg" in
    models)
      if [ "$mode" = "authorized" ]; then
        printf '%s\n' 'gemini-test-model'
        exit 0
      fi
      if [ "$mode" = "authorize_flow" ] && [ -f "$login_flag" ]; then
        printf '%s\n' 'gemini-test-model'
        exit 0
      fi
      printf '%s\n' 'Error: Please sign in to view available models. Launch the CLI without arguments to sign in.'
      exit 1
      ;;
    --print=*)
      printf '%s\n' invoked >> "$marker"
      if [ "$mode" = "authorize_flow" ]; then
        : > "$login_flag"
      fi
      if [ -n "${{LICO_ANTIGRAVITY_SESSION_RECEIPT:-}}" ]; then
        sh -s "$LICO_ANTIGRAVITY_SESSION_RECEIPT" <<'SH'
{writer}
SH
      fi
      printf '%s\n' 'PONG'
      exit 0
      ;;
    --help)
      printf '%s\n' '--print --conversation --model --effort --dangerously-skip-permissions'
      exit 0
      ;;
    --version)
      printf '%s\n' '1.1.8'
      exit 0
      ;;
  esac
done
receipt="${{LICO_ANTIGRAVITY_SESSION_RECEIPT:?}}"
sh -s "$receipt" <<'SH'
{writer}
SH
printf '%s\n' 'PONG'
exit 0
"#,
            mode = mode,
            marker = print_marker.display(),
            login_flag = login_flag.display(),
        );
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        Self {
            root,
            executable,
            print_marker,
        }
    }

    fn executable_str(&self) -> &str {
        self.executable.to_str().unwrap()
    }

    fn print_invoked(&self) -> bool {
        self.print_marker.exists()
    }
}

#[cfg(unix)]
impl Drop for AuthFakeExecutable {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
fn scoped_gemini_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lico-agy-auth-gemini-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn logged_out_send_returns_auth_required_without_spawning_a_turn() {
    let _environment_guard = environment_lock();
    let gemini = scoped_gemini_dir("logged-out");
    let portable = scoped_gemini_dir("logged-out-portable");
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let fixture = AuthFakeExecutable::new("logged-out", "logged_out");
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();

    install_driver_ports();
    let result = execute(
        fixture.executable_str(),
        &json!({}),
        "hello",
        "",
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(!result.ok);
    let failure = result.error.unwrap();
    assert_eq!(failure.code, "antigravity_auth_required");
    assert!(failure.user_interaction_required);
    assert!(
        !fixture.print_invoked(),
        "a logged-out send must never spawn the OAuth-opening turn"
    );
    assert!(
        !gemini.join("hooks.json").exists(),
        "the hook bridge must not be installed before authorization"
    );
}

#[cfg(unix)]
#[test]
fn authorized_send_proceeds_past_the_probe() {
    let _environment_guard = environment_lock();
    let gemini = scoped_gemini_dir("authorized");
    let portable = scoped_gemini_dir("authorized-portable");
    let previous_gemini = std::env::var_os("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
    unsafe {
        std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", &gemini);
    }
    let previous_portable =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable));
    let fixture = AuthFakeExecutable::new("authorized", "authorized");
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();

    install_driver_ports();
    let result = execute(
        fixture.executable_str(),
        &json!({}),
        "hello-from-lico",
        "",
        Some(&workspace),
        5_000,
        Some(8_192),
        8_192,
    );

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous_portable);
    if let Some(value) = previous_gemini {
        unsafe {
            std::env::set_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR", value);
        }
    } else {
        unsafe {
            std::env::remove_var("LICO_ANTIGRAVITY_GEMINI_CONFIG_DIR");
        }
    }
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.output, "PONG");
    assert!(fixture.print_invoked());
}

#[cfg(unix)]
#[test]
fn authorize_runs_the_explicit_vendor_flow_and_reprobes() {
    let fixture = AuthFakeExecutable::new("authorize", "authorize_flow");
    let report = authorize(Some(fixture.executable_str())).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["action"], "authorize");
    assert_eq!(report["authorized"], true);
    assert_eq!(report["status"], "authorized");
    assert!(
        fixture.print_invoked(),
        "the explicit authorize action must run the vendor OAuth trigger"
    );
    let serialized = report.to_string();
    assert!(!serialized.contains(&fixture.root.display().to_string()));
}

#[cfg(unix)]
#[test]
fn authorize_reports_incomplete_when_login_does_not_finish() {
    let fixture = AuthFakeExecutable::new("incomplete", "authorize_incomplete");
    let report = authorize(Some(fixture.executable_str())).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["authorized"], false);
    assert_eq!(report["status"], "authorization_incomplete");
    assert!(fixture.print_invoked());
}

#[cfg(unix)]
#[test]
fn authorize_missing_executable_is_a_typed_failure() {
    let result = authorize(Some("/definitely/missing/lico-antigravity-agy"));
    assert_eq!(result, Err("antigravity_authorize_unavailable"));
}

/// Both auth spawns (the authorization probe and the explicit vendor OAuth
/// trigger) are CLI invocations of this adapter: they must observe the user
/// shell environment, per the environment-equivalence invariant.
#[cfg(unix)]
#[test]
fn auth_spawns_observe_the_user_shell_environment() {
    use std::os::unix::fs::PermissionsExt;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "lico-antigravity-auth-env-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("fake-agy");
    let env_capture = root.join("env.log");
    let login_flag = root.join("login-complete.flag");
    fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
set -eu
env > "{env_capture}"
for arg in "$@"; do
  case "$arg" in
    models)
      if [ -f "{login_flag}" ]; then
        printf '%s\n' 'gemini-test-model'
        exit 0
      fi
      printf '%s\n' 'Error: Please sign in to view available models.'
      exit 1
      ;;
    --print=*)
      : > "{login_flag}"
      printf '%s\n' 'PONG'
      exit 0
      ;;
  esac
done
exit 0
"#,
            env_capture = env_capture.display(),
            login_flag = login_flag.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();

    // The auth spawns observe the pinned user shell snapshot, not the raw
    // process environment.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[(
        "LICO_TEST_SHELL_SNAPSHOT_MARKER",
        "shell-snapshot-env",
    )]);

    // Phase 1: logged-in probe through ensure_authorized.
    fs::write(&login_flag, b"").unwrap();
    super::auth::ensure_authorized(executable.to_str().unwrap()).unwrap();
    let observed = fs::read_to_string(&env_capture).unwrap();
    assert!(
        observed.contains("LICO_TEST_SHELL_SNAPSHOT_MARKER=shell-snapshot-env"),
        "authorization probe must observe the shell snapshot: {observed}"
    );
    assert!(observed.contains("\nPATH=") || observed.starts_with("PATH="));

    // Phase 2: logged-out authorize flow (probe fails, print turn runs,
    // re-probe succeeds); every spawn sees the same snapshot.
    let _ = fs::remove_file(&login_flag);
    let _ = fs::remove_file(&env_capture);
    let report = authorize(Some(executable.to_str().unwrap())).unwrap();
    assert_eq!(report["authorized"], true);
    let observed = fs::read_to_string(&env_capture).unwrap();
    assert!(
        observed.contains("LICO_TEST_SHELL_SNAPSHOT_MARKER=shell-snapshot-env"),
        "authorize spawns must observe the shell snapshot: {observed}"
    );

    let _ = fs::remove_dir_all(&root);
}
