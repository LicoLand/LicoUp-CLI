use super::execution::execute_with_test_handshake_bound;
use super::{RUNTIME_PROTOCOL, RunResult, execute};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The fake agent reads process-global env vars, so the tests that steer it
/// must not run concurrently or one test's env leaks into another test's
/// spawned process.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The fake answers synchronously, but a fully loaded test runner can delay
/// scheduling either process. Give immediate-response fixtures test-only
/// scheduling tolerance without relaxing the production handshake bound.
const FAKE_RESPONSE_BOUND: Duration = Duration::from_secs(30);

#[test]
fn private_instructions_fail_before_process_launch() {
    let result = execute(
        "definitely-not-a-real-lico-agent",
        &json!({"model":"test","privateInstructions":"private sentinel"}),
        "exact user prompt",
        "",
        Some(std::env::temp_dir().as_path()),
        1_000,
        None,
        1_024,
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("lico_agent_private_instructions_unsupported")
    );
}

#[test]
fn unsafe_stale_and_client_owned_workspaces_fail_before_process_launch() {
    let root = std::env::temp_dir().join(format!("lico-workspace-{}", uuid::Uuid::new_v4()));
    let portable = root.join("portable");
    let fallback = portable.join("agent-workspace");
    let home = root.join("home");
    let personal = home.join("Documents");
    let fallback_child = fallback.join("nested-project");
    fs::create_dir_all(&fallback).unwrap();
    fs::create_dir_all(&fallback_child).unwrap();
    fs::create_dir_all(&personal).unwrap();
    let _guard = ENV_LOCK.lock().unwrap();
    let prior_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("LICOUP_HOME", &portable);
        std::env::set_var("HOME", &home);
    }
    let cases = [
        PathBuf::from("relative/project"),
        root.join("absent-project"),
        fallback,
        fallback_child,
        home,
        personal,
        PathBuf::from(std::path::MAIN_SEPARATOR.to_string()),
    ];
    for workspace in cases {
        let result = execute_with_test_handshake_bound(
            "definitely-not-a-real-lico-agent",
            &json!({"model":"test"}),
            "prompt",
            "",
            Some(&workspace),
            0,
            None,
            1_024,
            FAKE_RESPONSE_BOUND,
        );
        assert_eq!(
            result.error.as_ref().map(|error| error.code),
            Some("lico_agent_workspace_rejected")
        );
    }
    unsafe {
        std::env::remove_var("LICOUP_HOME");
        if let Some(prior_home) = prior_home {
            std::env::set_var("HOME", prior_home);
        } else {
            std::env::remove_var("HOME");
        }
    }
    drop(_guard);
    let _ = fs::remove_dir_all(root);
}

/// M12: a lico-agent that starts but never answers `get_state` must fail the
/// send within the handshake bound instead of blocking forever.
#[test]
fn readiness_handshake_hang_fails_bounded() {
    let stamp = stamp_nanos();
    let (dir, executable) = compile_fake_lico_agent(stamp);
    let portable_dir = dir.join("portable");
    fs::create_dir_all(&portable_dir).unwrap();
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LICO_FAKE_LICO_AGENT_HANG", "1");
        std::env::set_var("LICOUP_HOME", &portable_dir);
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let started = Instant::now();
    let result = execute_with_production_bound(executable.to_string_lossy().as_ref(), &dir);
    unsafe {
        std::env::remove_var("LICO_FAKE_LICO_AGENT_HANG");
        std::env::remove_var("LICOUP_HOME");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !result.ok,
        "a silent agent must fail the send: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("lico_agent_rpc_handshake_timeout")
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(4),
        "handshake bound was not enforced: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "handshake exceeded its bound: {elapsed:?}"
    );
}

/// M12: a `get_state` error response must fail the send before the prompt is
/// written.
#[test]
fn rejected_readiness_handshake_fails_before_prompt() {
    let stamp = stamp_nanos();
    let (dir, executable) = compile_fake_lico_agent(stamp);
    let portable_dir = dir.join("portable");
    fs::create_dir_all(&portable_dir).unwrap();
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LICO_FAKE_LICO_AGENT_REJECT", "1");
        std::env::set_var("LICOUP_HOME", &portable_dir);
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let result = execute_with(executable.to_string_lossy().as_ref(), &dir);
    unsafe {
        std::env::remove_var("LICO_FAKE_LICO_AGENT_REJECT");
        std::env::remove_var("LICOUP_HOME");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(dir);
    assert!(
        !result.ok,
        "a rejected handshake must fail the send: {:?}",
        result.error
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("lico_agent_rpc_handshake_failed")
    );
}

/// The happy path still works: a successful handshake is followed by the
/// prompt and a completed turn.
#[test]
fn successful_readiness_handshake_keeps_the_turn_flow() {
    let stamp = stamp_nanos();
    let (dir, executable) = compile_fake_lico_agent(stamp);
    // Keep the parent transcript inside the temp dir.
    let portable_dir = dir.join("portable");
    fs::create_dir_all(&portable_dir).unwrap();
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LICOUP_HOME", &portable_dir);
    }
    let result = execute_with(executable.to_string_lossy().as_ref(), &dir);
    unsafe {
        std::env::remove_var("LICOUP_HOME");
    }
    drop(_guard);
    let _ = fs::remove_dir_all(&dir);
    assert!(result.ok, "happy path failed: {:?}", result.error);
    assert_eq!(result.turn_status, "completed");
    assert!(uuid::Uuid::parse_str(&result.session_id).is_ok());
    assert_eq!(result.session_id, result.thread_id);
    assert_eq!(
        result.effective.cwd.as_deref(),
        Some(dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        result.effective.model.as_deref(),
        Some("test-gateway-model")
    );
    assert_eq!(result.effective.permission_mode.as_deref(), Some("base"));
    assert_eq!(RUNTIME_PROTOCOL, "lico-agent-rpc-stdio-jsonl");
}

#[test]
fn resume_requires_persisted_header_and_observed_native_identity() {
    let stamp = stamp_nanos();
    let (dir, executable) = compile_fake_lico_agent(stamp);
    let portable_dir = dir.join("portable");
    let sessions = portable_dir.join("client-state/lico-agent/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    fs::write(
        sessions.join(format!("{session_id}.jsonl")),
        format!(
            "{{\"type\":\"session\",\"id\":\"{session_id}\",\"cwd\":\"/synthetic/project\"}}\n{{\"type\":\"message\",\"role\":\"user\",\"text\":\"first\"}}\n{{\"type\":\"message\",\"role\":\"assistant\",\"text\":\"answer\"}}\n"
        ),
    )
    .unwrap();
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LICOUP_HOME", &portable_dir);
    }
    let resumed = execute_with_test_handshake_bound(
        executable.to_string_lossy().as_ref(),
        &json!({"model": "test-gateway-model"}),
        "second",
        &session_id,
        Some(&dir),
        10_000,
        None,
        1_024,
        FAKE_RESPONSE_BOUND,
    );
    unsafe {
        std::env::set_var("LICO_FAKE_SESSION_ID", uuid::Uuid::new_v4().to_string());
    }
    // Re-pin the launch snapshot so the fixture observes the drifted
    // session-id steering through its environment.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let mismatch = execute_with_test_handshake_bound(
        executable.to_string_lossy().as_ref(),
        &json!({"model": "test-gateway-model"}),
        "must not be sent",
        &session_id,
        Some(&dir),
        10_000,
        None,
        1_024,
        FAKE_RESPONSE_BOUND,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_SESSION_ID");
        std::env::remove_var("LICOUP_HOME");
    }
    drop(_guard);
    assert!(resumed.ok, "resume failed: {:?}", resumed.error);
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(
        mismatch.error.as_ref().map(|error| error.code),
        Some("lico_agent_session_identity_mismatch")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn explicit_output_bound_and_persistence_failure_are_visible() {
    let stamp = stamp_nanos();
    let (dir, executable) = compile_fake_lico_agent(stamp);
    let portable_dir = dir.join("portable");
    fs::create_dir_all(&portable_dir).unwrap();
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LICOUP_HOME", &portable_dir);
        std::env::set_var("LICO_FAKE_OUTPUT", "complete synthetic output");
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let bounded = execute_with_test_handshake_bound(
        executable.to_string_lossy().as_ref(),
        &json!({"model": "test-gateway-model"}),
        "prompt",
        "",
        Some(&dir),
        0,
        Some(4),
        1_024,
        FAKE_RESPONSE_BOUND,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_OUTPUT");
        std::env::set_var("LICO_FAKE_PERSIST_FAIL", "1");
    }
    // Re-pin: the persistence-failure steering replaced the output steering.
    drop(_pin);
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let persistence = execute_with_test_handshake_bound(
        executable.to_string_lossy().as_ref(),
        &json!({"model": "test-gateway-model"}),
        "prompt",
        "",
        Some(&dir),
        0,
        None,
        1_024,
        FAKE_RESPONSE_BOUND,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_PERSIST_FAIL");
        std::env::remove_var("LICOUP_HOME");
    }
    drop(_guard);
    assert_eq!(
        bounded.error.as_ref().map(|error| error.code),
        Some("lico_agent_output_limit_exceeded")
    );
    assert_eq!(
        persistence.error.as_ref().map(|error| error.code),
        Some("lico_agent_transcript_persist_failed")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn omitted_output_bound_is_complete_and_sustained_stderr_cannot_deadlock() {
    let stamp = stamp_nanos();
    let (dir, executable) = compile_fake_lico_agent(stamp);
    let portable_dir = dir.join("portable");
    fs::create_dir_all(&portable_dir).unwrap();
    let expected = "synthetic-output".repeat(512);
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LICOUP_HOME", &portable_dir);
        std::env::set_var("LICO_FAKE_OUTPUT", &expected);
        std::env::set_var("LICO_FAKE_STDERR_BYTES", "262144");
    }
    // Pin the fixture steering channel into the launch snapshot explicitly.
    let _pin = licoup_agent_targets::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let result = execute_with_test_handshake_bound(
        executable.to_string_lossy().as_ref(),
        &json!({"model": "test-gateway-model"}),
        "prompt",
        "",
        Some(&dir),
        0,
        None,
        1_024,
        FAKE_RESPONSE_BOUND,
    );
    unsafe {
        std::env::remove_var("LICO_FAKE_STDERR_BYTES");
        std::env::remove_var("LICO_FAKE_OUTPUT");
        std::env::remove_var("LICOUP_HOME");
    }
    drop(_guard);
    assert!(result.ok, "unbounded output failed: {:?}", result.error);
    assert_eq!(result.output, expected);
    assert!(result.stderr_truncated);
    let _ = fs::remove_dir_all(dir);
}

fn execute_with(executable: &str, dir: &PathBuf) -> RunResult {
    execute_with_test_handshake_bound(
        executable,
        &json!({"model": "test-gateway-model"}),
        "private first prompt",
        "",
        Some(dir.as_path()),
        10_000,
        None,
        1024,
        FAKE_RESPONSE_BOUND,
    )
}

fn execute_with_production_bound(executable: &str, dir: &PathBuf) -> RunResult {
    execute(
        executable,
        &json!({"model": "test-gateway-model"}),
        "private first prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    )
}

/// Builds a fake lico-agent executable in a fresh temp dir (shared helper).
fn compile_fake_lico_agent(stamp: u128) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lico-lico-agent-fake-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_lico_agent.rs");
    let executable = dir.join(format!("fake-lico-agent{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_LICO_AGENT_SOURCE).unwrap();
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

const FAKE_LICO_AGENT_SOURCE: &str = r###"
use std::env;
use std::io::{self, BufRead, Write};

fn write_json(value: &str) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn id_of(line: &str) -> &str {
    line.split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("")
}

fn main() {
    let args = env::args().collect::<Vec<_>>();
    let argv_session_id = args
        .windows(2)
        .find(|pair| pair[0] == "--session-id")
        .map(|pair| pair[1].clone())
        .unwrap_or_default();
    let session_id = env::var("LICO_FAKE_SESSION_ID").unwrap_or(argv_session_id);
    let hang = env::var("LICO_FAKE_LICO_AGENT_HANG").is_ok();
    let reject = env::var("LICO_FAKE_LICO_AGENT_REJECT").is_ok();
    let output = env::var("LICO_FAKE_OUTPUT").unwrap_or_default();
    let persist_fail = env::var("LICO_FAKE_PERSIST_FAIL").is_ok();
    let stderr_bytes = env::var("LICO_FAKE_STDERR_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if stderr_bytes > 0 {
        io::stderr().write_all(&vec![b'e'; stderr_bytes]).unwrap();
        io::stderr().flush().unwrap();
    }
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return; };
        if line.trim().is_empty() { continue; }
        if hang { continue; }
        let id = id_of(&line);
        if line.contains("\"get_state\"") {
            if reject {
                write_json(&format!(r#"{{"id":"{id}","type":"response","success":false,"error":"unsupported_request"}}"#));
            } else {
                write_json(&format!(r#"{{"id":"{id}","type":"response","success":true,"data":{{"isRunning":false,"profile":"base","sessionId":"{session_id}"}}}}"#));
            }
        } else if line.contains("\"prompt\"") {
            write_json(&format!(r#"{{"id":"{id}","type":"response","success":true}}"#));
            if !output.is_empty() {
                write_json(&format!(r#"{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","delta":"{output}"}}}}"#));
            }
            if persist_fail {
                write_json(r#"{"type":"error","code":"lico_agent_transcript_persist_failed"}"#);
            } else {
                write_json(r#"{"type":"agent_end"}"#);
            }
        }
    }
}
"###;

fn stamp_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Plan mode without a host that can sandbox is not a Lico Agent turn: the
/// package refuses rather than spawning the program unsandboxed.
#[test]
fn plan_mode_without_a_sandbox_host_fails_closed() {
    use super::sandbox::{PlanSandboxFailure, plan_command};
    let root = std::env::temp_dir().join(format!("lico-plan-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let plan = root.join("active-plan.md");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    // No sandbox port is installed in this package's own test binary, so the
    // question cannot be asked and the answer is the refusal.
    assert_eq!(
        plan_command(&workspace, &plan, &workspace, 15_722, &[]).unwrap_err(),
        PlanSandboxFailure::Unavailable
    );
    let _ = fs::remove_dir_all(root);
}
