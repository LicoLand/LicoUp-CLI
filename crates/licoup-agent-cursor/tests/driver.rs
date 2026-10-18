//! The claims this package makes about one running Cursor turn.
//!
//! They drive the package's own entry points against a fake Cursor the test
//! compiles from source, on the pty transport the package launches its child on,
//! with the turn-event port answered by the client's own emitters, so every
//! event claim is read through the same seam the client reads it through.

use licoup_agent_cursor::driver as cursor_driver;
use licoup_agent_cursor::driver::{ControlDisposition, DRIVER_ID, RUNTIME_PROTOCOL};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The fake agent reads process-global env vars, so the tests that steer it
/// must not run concurrently: one test's env mutation would leak into another
/// test's spawned process and change its behavior.
static ENV_LOCK: Mutex<()> = Mutex::new(());

static FAKE_BUILD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    // A panicking test must not poison the lock for every later test.
    ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn wait_for_captured_event(
    events: &Arc<Mutex<Vec<serde_json::Value>>>,
    deadline: Instant,
    label: &str,
    predicate: impl Fn(&serde_json::Value) -> bool,
) {
    loop {
        if events.lock().unwrap().iter().any(&predicate) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Answer this package's turn-event port with the host's own emitters.
///
/// The port is fail-closed before a host answers it, so a turn run here emits
/// nothing until this installation. The client answers with these same
/// emitters, which write to the thread-local stream sink each capturing test
/// installs; the first test to run owns the answer for the whole binary, and
/// every later installation is refused as a duplicate rather than replacing it.
fn install_host_turn_event_port() {
    use licoup_agent_cursor::port::turn_event::{TurnEventPort, install};
    use licoup_foundation::platform::turn_event_emit as host_emitters;

    let _ = install(TurnEventPort {
        emit_turn_event: host_emitters::emit_turn_event,
        emit_agent_message_chunk: host_emitters::emit_agent_message_chunk,
        emit_agent_message_completed: host_emitters::emit_agent_message_completed,
        emit_agent_processing: host_emitters::emit_agent_processing,
        emit_agent_tool_error: host_emitters::emit_agent_tool_error,
    });
}

/// Builds a fake cursor-agent executable in a fresh temp dir.
///
/// The fake vendor CLI is one source the package ships, read at compile time so
/// a fixture that moves fails this suite's build rather than its run.
fn compile_fake_cursor(stamp: u128) -> (PathBuf, PathBuf) {
    let sequence = FAKE_BUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("lico-cursor-cli-fake-{stamp}-{sequence}"));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_cursor_agent.rs");
    let executable = dir.join(format!("fake-cursor-agent{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, include_str!("fixtures/fake_cursor_agent.rs")).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    (dir, executable)
}

#[test]
fn cli_exact_resume_places_session_and_prompt_in_argv() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    // The fake reads process-global env vars; serialize with the other fake
    // tests so no concurrent test's vars leak into this spawned process.
    let _guard = env_lock();

    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;

    let first = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private first prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok, "first Cursor CLI failure: {:?}", first.error);
    assert!(!first.session_id.is_empty());
    assert_eq!(first.output, "first response");
    assert!(matches!(
        first.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Lifecycle(
            licoup_agent_adapter_sdk::LifecycleStage::Completed
        ))
    ));

    let second = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private follow-up prompt",
        &first.session_id,
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(second.ok, "resume Cursor CLI failure: {:?}", second.error);
    assert_eq!(second.session_id, first.session_id);
    assert_eq!(second.output, "second response");
    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.accepted"
            && event["sessionId"].as_str() == Some(first.session_id.as_str())
    }));
    let accepted_positions = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            (event["event"] == "agent.turn.accepted"
                && event["sessionId"].as_str() == Some(first.session_id.as_str()))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let processing_positions = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            (event["event"] == "agent.turn.processing"
                && event["sessionId"].as_str() == Some(first.session_id.as_str()))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(accepted_positions.len(), 2);
    assert_eq!(processing_positions.len(), 2);
    assert!(
        accepted_positions
            .iter()
            .zip(&processing_positions)
            .all(|(accepted, processing)| accepted < processing),
        "native accepted must precede native processing: {events:?}"
    );
    assert!(processing_positions.iter().all(|position| {
        events[*position]["payload"]["lifecyclePrefix"]
            == json!(["submitted", "accepted", "processing"])
    }));
    // The NDJSON stream arrives through the pty transport (unix): the raw-mode
    // slave keeps `\n`-only line endings, so the progressive assistant
    // fragments must surface intact, once each, and concatenate exactly to the
    // cumulative reply for both the new and the resumed turn. The resumed turn
    // keeps the same session id, so ordering, not session id, splits the
    // per-turn fragments.
    let chunks: Vec<String> = events
        .iter()
        .filter_map(|event| {
            (event["event"] == "agent.message.chunk").then(|| {
                event["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
        })
        .collect();
    assert_eq!(
        chunks,
        vec![
            "first".to_owned(),
            " response".to_owned(),
            "second".to_owned(),
            " response".to_owned()
        ]
    );
    assert_eq!(chunks[..2].concat(), first.output);
    assert_eq!(chunks[2..].concat(), second.output);
    assert_eq!(RUNTIME_PROTOCOL, "cursor-agent-cli-v1");
    assert_eq!(DRIVER_ID, "cursor-cli");
    assert_eq!(
        cursor_driver::cleanup_session(&second.session_id),
        ControlDisposition::NotPersisted
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn create_chat_receives_the_same_scoped_caller_context_as_the_turn() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_REQUIRE_CALLER_CONTEXT", "1");
    }
    // The launch environment is the user shell snapshot; pin the fixture
    // steering channel into it explicitly for this thread.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({
            "agentId": "cursor",
            "conversationId": "conversation:fixture",
            "membershipId": "membership:cursor",
            "parentDispatchId": "subagent:parent"
        }),
        "caller context prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_REQUIRE_CALLER_CONTEXT");
    }
    assert!(result.ok, "Cursor CLI failure: {:?}", result.error);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn mismatched_stream_identity_fails_and_never_completes_a_turn() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_DRIFT_SESSION_ID", "1");
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);

    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;

    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private first prompt __lico_drift__",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_DRIFT_SESSION_ID");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(
        !result.ok,
        "a drifted stream identity must not report success: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("cursor_cli_session_identity_mismatch")
    );
    // The wrong conversation was never exposed as accepted, chunked, or
    // completed output.
    let events = captured.lock().unwrap().clone();
    assert!(!events.iter().any(|event| {
        event["event"] == "agent.message.chunk" || event["event"] == "agent.message.completed"
    }));
}

#[cfg(unix)]
#[test]
fn auto_update_lock_and_staging_are_surfaced_as_runtime_events() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    // The fake reads process-global env vars; serialize with the other fake
    // tests so no concurrent test's vars leak into this spawned process.
    let _guard = env_lock();

    // Isolate the watcher's install root and hold the turn open.
    let install_root = std::env::temp_dir().join(format!("lico-cursor-install-fake-{stamp}"));
    fs::create_dir_all(install_root.join("versions")).unwrap();
    let release_path = install_root.join("update-test-release");
    // Safe in the single-threaded test body; restored before the assertions.
    unsafe {
        std::env::set_var("LICO_CURSOR_AGENT_INSTALL_DIR", &install_root);
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_UPDATE_RELEASE_PATH", &release_path);
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);

    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;

    // The stream sink is thread-local: `execute` must run on this thread.
    // Drive the lock/staging timeline from a helper thread instead.
    let timeline_root = install_root.clone();
    let timeline_events = Arc::clone(&captured);
    let timeline = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(12);
        wait_for_captured_event(
            &timeline_events,
            deadline,
            "native session binding",
            |event| event["event"] == "dispatch.turn.bound",
        );
        fs::write(timeline_root.join(".install.lock"), b"").unwrap();
        wait_for_captured_event(
            &timeline_events,
            deadline,
            "runtime update start",
            |event| event["event"] == "agent.runtime.updating",
        );
        fs::create_dir_all(timeline_root.join("versions").join(".2026.08.04-aaa8809")).unwrap();
        wait_for_captured_event(
            &timeline_events,
            deadline,
            "runtime installing phase",
            |event| {
                event["event"] == "agent.runtime.updating"
                    && event["payload"]["phase"] == "installing"
            },
        );
        fs::remove_file(timeline_root.join(".install.lock")).unwrap();
        fs::remove_dir_all(timeline_root.join("versions").join(".2026.08.04-aaa8809")).unwrap();
        wait_for_captured_event(
            &timeline_events,
            deadline,
            "runtime update completion",
            |event| event["event"] == "agent.runtime.update.completed",
        );
        fs::write(timeline_root.join("update-test-release"), b"").unwrap();
    });

    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private first prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    timeline.join().unwrap();
    unsafe {
        std::env::remove_var("LICO_CURSOR_AGENT_INSTALL_DIR");
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_UPDATE_RELEASE_PATH");
    }
    let _ = fs::remove_dir_all(install_root);
    let _ = fs::remove_dir_all(dir);

    assert!(result.ok, "turn failed: {:?}", result.error);
    assert_eq!(result.output, "first response");
    let events = captured.lock().unwrap().clone();
    let updating = events.iter().filter(|event| {
        event["event"] == "agent.runtime.updating" && event["payload"]["artifact"] == "cursor-agent"
    });
    let updating: Vec<_> = updating.collect();
    assert!(
        !updating.is_empty(),
        "expected runtime updating events: {events:?}"
    );
    assert!(
        updating.iter().any(|event| {
            event["payload"]["phase"] == "downloading" || event["payload"]["phase"] == "installing"
        }),
        "expected a download/install phase: {events:?}"
    );
    let completed = events.iter().any(|event| {
        event["event"] == "agent.runtime.update.completed"
            && event["payload"]["version"] == "2026.08.04-aaa8809"
    });
    assert!(completed, "expected update completion event: {events:?}");
}

/// M10: the turn deadline must span the whole turn, including the
/// session-creation phase. A create-chat that consumes most of the window
/// leaves the turn with the remainder — not with a fresh full window.
///
/// The margins are deliberately generous: this sandbox inflates child-process
/// lifecycle timing by hundreds of milliseconds, so sub-second windows are
/// not reliable here. With a 6s window, a create-chat that takes ~2s leaves
/// the turn ~3.5s of budget while the turn needs ~5s to answer: the turn must
/// time out at the caller's deadline. (Before the fix the turn would get a
/// fresh 6s window and complete.)
#[test]
fn create_chat_time_is_charged_against_the_turn_deadline() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_CREATE_CHAT_DELAY_MS", "2000");
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS", "5000");
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let started = std::time::Instant::now();
    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private first prompt __lico_test__",
        "",
        Some(dir.as_path()),
        6000,
        Some(1024 * 1024),
        1024,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_CREATE_CHAT_DELAY_MS");
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(
        !result.ok,
        "a turn slower than the remaining window must time out: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("cursor_cli_timeout")
    );
    // The wall clock stays near the caller's deadline instead of deadline
    // plus a fresh create-chat window.
    assert!(
        started.elapsed() < Duration::from_millis(9000),
        "wall clock exceeded the deadline by too much"
    );
}

/// timeoutMs 0 keeps its contract: no synthetic deadline in either session
/// creation or turn execution.
#[test]
fn timeout_zero_keeps_the_turn_deadline_free() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_CREATE_CHAT_DELAY_MS", "300");
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS", "300");
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private first prompt __lico_test__",
        "",
        Some(dir.as_path()),
        0,
        Some(1024 * 1024),
        1024,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_CREATE_CHAT_DELAY_MS");
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(
        result.ok,
        "timeoutMs 0 must not time out: {:?}",
        result.error
    );
    assert_eq!(result.output, "first response");
}

/// M11: a CLI crash after streaming partial output must not be reported as a
/// completed turn.
#[test]
fn crashed_cli_after_partial_output_is_reported_as_failed() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_CRASH_AFTER_CHUNK", "1");
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private first prompt __lico_crash__",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_CRASH_AFTER_CHUNK");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(
        !result.ok,
        "a crashed CLI must not report success: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("cursor_cli_turn_failed")
    );
    assert_eq!(result.status_code, Some(3));
    assert_ne!(result.turn_status, "completed");
}

#[cfg(unix)]
#[test]
fn stdout_eof_waits_for_the_live_child_before_classifying_the_turn() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    let ready_path = dir.join("stdout-eof-ready");
    let release_path = dir.join("stdout-eof-release");
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_EOF_READY_PATH", &ready_path);
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_EOF_RELEASE_PATH", &release_path);
    }
    let executable = executable.to_string_lossy().into_owned();
    let turn_dir = dir.clone();
    let handle = std::thread::spawn(move || {
        let _pin =
            licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
        cursor_driver::execute(
            &executable,
            &json!({}),
            "synthetic request __lico_stdout_eof__",
            "",
            Some(turn_dir.as_path()),
            0,
            Some(1024 * 1024),
            1024,
        )
    });

    let ready_deadline = Instant::now() + Duration::from_secs(20);
    while !ready_path.is_file() && Instant::now() < ready_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let stream_closed = ready_path.is_file();
    std::thread::sleep(Duration::from_millis(350));
    let child_was_still_active = !handle.is_finished();
    fs::write(&release_path, b"release").unwrap();
    let result = handle.join().unwrap();
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_EOF_READY_PATH");
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_EOF_RELEASE_PATH");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);

    assert!(stream_closed, "the fixture did not close the turn stream");
    assert!(
        child_was_still_active,
        "stdout EOF must not terminate a child that has not ended the turn"
    );
    assert!(!result.ok);
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("cursor_cli_turn_failed")
    );
    assert_eq!(result.status_code, Some(0));
}

#[test]
fn non_error_terminal_subtype_keeps_the_completed_reply() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "answer normally __lico_non_error_subtype__",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(result.ok, "valid Cursor reply failed: {:?}", result.error);
    assert_eq!(result.output, "fake Cursor final answer");
}

#[test]
fn stderr_failure_is_classified_without_exposing_vendor_prose() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    let result = cursor_driver::execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "trigger bounded stderr __lico_stderr_usage__",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    let failure = result.error.expect("stderr failure must be classified");
    assert_eq!(failure.code, "cursor_cli_usage_limit_exceeded");
    assert_eq!(failure.component, Some("native_cli"));
    assert_eq!(failure.retryable, Some(false));
    assert_eq!(
        failure.recovery,
        Some("select_available_model_or_wait_for_quota_reset")
    );
    let encoded = format!("{failure:?}");
    assert!(!encoded.contains("private fixture account context"));
}

/// M11: a user-cancelled turn must be reported as cancelled, never as
/// completed with truncated output.
#[cfg(unix)]
#[test]
fn cancelled_turn_is_reported_as_cancelled_not_completed() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    install_host_turn_event_port();
    let (dir, executable) = compile_fake_cursor(stamp);
    let _guard = env_lock();
    // A per-test session tag keeps this turn's session id unique: the
    // active-turn registry is global and keyed by session id, and the fake's
    // default id is deterministic, so a concurrent test could otherwise be
    // cancelled by mistake.
    let session_tag = stamp.to_string();
    unsafe {
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS", "20000");
        std::env::set_var("LICO_FAKE_CURSOR_AGENT_SESSION_TAG", &session_tag);
    }
    let executable = executable.to_string_lossy().into_owned();
    let turn_dir = dir.clone();
    let handle = std::thread::spawn(move || {
        // The launch snapshot override is thread-local: pin the fixture
        // steering channel on the executing thread itself.
        let _pin =
            licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
        cursor_driver::execute(
            &executable,
            &json!({}),
            "private first prompt __lico_test__",
            "",
            Some(turn_dir.as_path()),
            30_000,
            Some(1024 * 1024),
            1024,
        )
    });
    // create-chat answers with the tagged fake session id; poll until this
    // turn is registered and cancelled.
    let session_id = format!("fake-cursor-session-{session_tag}-000000000001");
    // The sandbox can inflate child startup by seconds, so the poll window
    // is far wider than the fake's hold time.
    let mut accepted = false;
    for _ in 0..160 {
        if cursor_driver::cancel(&session_id) == ControlDisposition::Accepted {
            accepted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = handle.join().unwrap();
    assert!(
        accepted,
        "cancel was never accepted for {session_id}; turn result: {:?}",
        result.error
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS");
        std::env::remove_var("LICO_FAKE_CURSOR_AGENT_SESSION_TAG");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(
        !result.ok,
        "a cancelled turn must not report success: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("cursor_cli_cancelled")
    );
    assert_eq!(result.turn_status, "cancelled");
}
