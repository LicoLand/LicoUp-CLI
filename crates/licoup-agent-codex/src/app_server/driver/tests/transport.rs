use crate::app_server::driver::{
    active_control::{ControlDisposition, interrupt, steer},
    execute,
};
use serde_json::json;
use std::fs as test_fs;
use std::path::Path;
use std::process::Command as TestCommand;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn fake_child_proves_spawn_stdin_concurrent_drain_and_completion() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionDirection, RawExecutionObserver, RawExecutionScope,
    };
    use std::sync::{Arc, Mutex};

    // One fixture, shared with the host's own end-to-end Codex suites.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("licoup-native")
        .join("tests")
        .join("fixtures")
        .join("fake_codex_app_server.rs");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_dir = std::env::temp_dir().join(format!("lico-codex-fake-{suffix}"));
    test_fs::create_dir_all(&temp_dir).unwrap();
    let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = TestCommand::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("fake Codex fixture should compile with the active Rust toolchain");
    assert!(compile.success());

    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let _scope = RawExecutionScope::enter(Some(RawExecutionObserver::new(
        move |source, direction, text| {
            sink.lock()
                .unwrap()
                .push((source.to_owned(), direction, text.to_owned()));
            Ok(())
        },
    )));

    let result = execute(
        &executable.to_string_lossy(),
        &json!({"model": "gpt-5.6-luna", "reasoningEffort": "high"}),
        "fake-child-private-prompt",
        "",
        Some(&temp_dir),
        10_000,
        Some(1024 * 1024),
        1024,
        None,
    );

    assert!(result.ok, "fake child protocol failed: {:?}", result.error);
    assert_eq!(result.output, "fake child final answer");
    assert_eq!(result.session_id, "fake-thread");
    assert_eq!(result.thread_id, "fake-thread");
    assert_eq!(result.turn_id, "fake-turn");
    assert_eq!(result.turn_status, "completed");
    assert!(matches!(
        result.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Lifecycle(
            licoup_agent_adapter_sdk::LifecycleStage::Completed
        ))
    ));
    assert_eq!(result.effective.model.as_deref(), Some("gpt-5.6-luna"));
    assert_eq!(result.effective.reasoning_effort.as_deref(), Some("high"));
    assert!(result.stderr_truncated);
    let records = records.lock().unwrap();
    assert!(
        records
            .iter()
            .any(|(source, direction, text)| source == "codex-app-server"
                && *direction == RawExecutionDirection::Sent
                && text.contains("fake-child-private-prompt")
                && text.ends_with('\n'))
    );
    let received: String = records
        .iter()
        .filter(|(_, direction, _)| *direction == RawExecutionDirection::Received)
        .map(|(_, _, text)| text.as_str())
        .collect();
    assert!(received.contains("fake-thread") && received.ends_with('\n'));
    assert!(
        records
            .iter()
            .filter(|(_, direction, _)| *direction == RawExecutionDirection::Stderr)
            .map(|(_, _, text)| text.len())
            .sum::<usize>()
            > 1024
    );

    let _ = test_fs::remove_dir_all(temp_dir);
}

#[test]
fn fake_child_acknowledges_native_guidance_during_the_active_turn() {
    // One fixture, shared with the host's own end-to-end Codex suites.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("licoup-native")
        .join("tests")
        .join("fixtures")
        .join("fake_codex_app_server.rs");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("lico-codex-steer-{suffix}"));
    test_fs::create_dir_all(&temp_dir).unwrap();
    let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = TestCommand::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("fake Codex fixture should compile with the active Rust toolchain");
    assert!(compile.success());
    let mut steer_marker = executable.clone();
    steer_marker.set_extension("steer-mode");
    test_fs::write(&steer_marker, b"").unwrap();

    let executable_text = executable.to_string_lossy().to_string();
    let cwd = temp_dir.clone();
    let run = std::thread::spawn(move || {
        execute(
            &executable_text,
            &json!({"model": "fake-steer"}),
            "fake-codex-steer-prompt",
            "",
            Some(&cwd),
            10_000,
            Some(1024 * 1024),
            1024,
            None,
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let disposition = loop {
        let disposition = steer(
            "fake-steer-thread",
            "fake-steer-turn",
            "fake-codex-steer-guidance",
        );
        if disposition == ControlDisposition::Accepted || std::time::Instant::now() >= deadline {
            break disposition;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(disposition, ControlDisposition::Accepted);
    let result = run.join().unwrap();
    assert!(result.ok, "fake child protocol failed: {:?}", result.error);
    assert_eq!(result.output, "fake child guided answer");
    assert_eq!(result.session_id, "fake-steer-thread");
    assert_eq!(result.turn_id, "fake-steer-turn");

    let _ = test_fs::remove_dir_all(temp_dir);
}

#[test]
fn fake_child_keeps_the_turn_active_after_steer_so_interrupt_is_observable() {
    // One fixture, shared with the host's own end-to-end Codex suites.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("licoup-native")
        .join("tests")
        .join("fixtures")
        .join("fake_codex_app_server.rs");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("lico-codex-cancel-{suffix}"));
    test_fs::create_dir_all(&temp_dir).unwrap();
    let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = TestCommand::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("fake Codex fixture should compile with the active Rust toolchain");
    assert!(compile.success());
    let mut steer_marker = executable.clone();
    steer_marker.set_extension("steer-mode");
    test_fs::write(&steer_marker, b"").unwrap();
    let mut cancel_marker = executable.clone();
    cancel_marker.set_extension("cancel-mode");
    test_fs::write(&cancel_marker, b"").unwrap();
    let mut interrupt_path = executable.clone();
    interrupt_path.set_extension("interrupt.json");
    let _ = test_fs::remove_file(&interrupt_path);

    let executable_text = executable.to_string_lossy().to_string();
    let cwd = temp_dir.clone();
    let run = std::thread::spawn(move || {
        execute(
            &executable_text,
            &json!({"model": "fake-cancel"}),
            "fake-codex-steer-prompt",
            "",
            Some(&cwd),
            10_000,
            Some(1024 * 1024),
            1024,
            None,
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let steered = loop {
        let disposition = steer(
            "fake-cancel-thread",
            "fake-cancel-turn",
            "fake-codex-steer-guidance",
        );
        if disposition == ControlDisposition::Accepted || std::time::Instant::now() >= deadline {
            break disposition;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(steered, ControlDisposition::Accepted);
    assert_eq!(
        interrupt("fake-cancel-thread"),
        ControlDisposition::Accepted,
        "live Codex interrupt must be Accepted after steer keeps the turn active"
    );
    let result = run.join().unwrap();
    assert_eq!(result.session_id, "fake-cancel-thread");
    assert_eq!(result.turn_id, "fake-cancel-turn");
    assert_eq!(result.turn_status, "cancelled");
    let interrupt = serde_json::from_str::<serde_json::Value>(
        &test_fs::read_to_string(&interrupt_path).expect("native interrupt receipt"),
    )
    .unwrap();
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["threadId"], "fake-cancel-thread");
    assert_eq!(interrupt["turnId"], "fake-cancel-turn");

    let _ = test_fs::remove_dir_all(temp_dir);
}
