//! The host's own integration claims over the Claude Code adapter package.
//!
//! The package owns the CLI process, the frames it classifies and the outcome
//! and transcript it reports. Two of this host's own facilities sit over that
//! process, and neither is the package's: the conversation lane's process-local
//! history page folds the package's projection into the lane's envelope, and
//! the Secure Mesh approval authority decides whether a parked interaction may
//! be raised. Both are composed here, so the claims about them belong here and
//! name [`licoup_agent_claude_code::driver`] for the driver they fold.
//!
//! The projection's own contents are the package's claim and are asserted
//! beside the driver it belongs to; what is asserted here is what the host does
//! with the package's answer.

use licoup_agent_claude_code::driver::{ControlDisposition, cleanup_session, execute};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[path = "../../../../tests/fixtures/claude_process_local_test_lock.rs"]
mod claude_process_local_test_lock;

/// A directory no other Claude Code suite shares, so the fixture's process-local
/// registry, transcript and lease are this test's alone.
fn temporary_directory(prefix: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

/// The Claude Code streaming-input CLI the host's own lane suites drive.
fn compile_fake_claude(prefix: &str) -> (PathBuf, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_claude_code.rs");
    let directory = temporary_directory(prefix);
    let executable = directory.join(format!("fake-claude{}", std::env::consts::EXE_SUFFIX));
    let status = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string()))
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    (directory, executable)
}

fn history(params: Value) -> Value {
    crate::platform::conversation_lane::process_local_history(&params).unwrap()
}

/// The lane folds the package's projection for a live Claude Code conversation
/// into its own page, with the lane's envelope around it and the lane's paging
/// parameters on it.
#[test]
fn the_lane_folds_a_live_claude_code_transcript_into_its_own_page() {
    super::compose();
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let (directory, executable) = compile_fake_claude("lico-claude-lane-history");
    let executable_text = executable.to_string_lossy().to_string();
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let first = execute(
        &executable_text,
        &params,
        "fake-claude-private-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok, "first turn failed: {:?}", first.error);
    let second = execute(
        &executable_text,
        &json!({}),
        "fake-claude-private-prompt-2",
        &first.session_id,
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(second.ok, "second turn failed: {:?}", second.error);

    let page = history(json!({
        "agent": "claude-code",
        "sessionId": second.session_id
    }));
    assert_eq!(page["ok"], true);
    assert_eq!(page["continuityScope"], "process-local");
    assert_eq!(page["nativeSessionId"], second.session_id);
    assert_eq!(page["turnCount"], 2);
    assert_eq!(page["turns"].as_array().unwrap().len(), 2);
    assert_eq!(
        page.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        [
            "byteCount",
            "continuityScope",
            "hasMore",
            "nativeSessionId",
            "nextBefore",
            "ok",
            "turnCount",
            "turns"
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
    assert_eq!(page["turns"][0]["turnId"], first.turn_id);
    assert_eq!(page["turns"][1]["turnId"], second.turn_id);
    assert_eq!(page["turns"][0]["output"], "fake Claude final answer 1");
    assert_eq!(page["turns"][1]["output"], "fake Claude final answer 2");
    assert_eq!(page["turns"][0]["prompt"], "fake-claude-private-prompt-1");
    assert_eq!(page["turns"][1]["prompt"], "fake-claude-private-prompt-2");
    assert_eq!(page["hasMore"], false);
    assert!(page["nextBefore"].is_null());

    // The lane's own paging parameters reach the package's backward page: one
    // turn back with the earlier page flagged, then the turn before the cursor.
    let latest = history(json!({
        "agent": "claude-code",
        "sessionId": second.session_id,
        "limit": 1
    }));
    assert_eq!(latest["turns"].as_array().unwrap().len(), 1);
    assert_eq!(latest["turns"][0]["turnId"], second.turn_id);
    assert_eq!(latest["hasMore"], true);
    assert_eq!(latest["nextBefore"], 1);
    let earlier = history(json!({
        "agent": "claude-code",
        "sessionId": second.session_id,
        "before": 1,
        "limit": 1
    }));
    assert_eq!(earlier["turns"].as_array().unwrap().len(), 1);
    assert_eq!(earlier["turns"][0]["turnId"], first.turn_id);
    assert_eq!(earlier["hasMore"], false);
    assert!(earlier["nextBefore"].is_null());

    // A released transport has no process-local transcript, so the lane reports
    // the exact conversation unavailable rather than an empty page.
    assert_eq!(
        cleanup_session(&second.session_id),
        ControlDisposition::Accepted
    );
    let released = history(json!({
        "agent": "claude-code",
        "sessionId": second.session_id
    }));
    assert_eq!(released["ok"], false);
    assert_eq!(released["error"]["code"], "claude_code_session_unavailable");
    assert_eq!(released["error"]["stage"], "session/history");
    let _ = fs::remove_dir_all(directory);
}

/// The lane's history entry answers only for the Agent it composes a
/// process-local transcript for, and only for an exact native session.
#[test]
fn the_history_entry_refuses_what_it_does_not_compose() {
    super::compose();
    let other = history(json!({"agent": "codex", "sessionId": "native-1"}));
    assert_eq!(other["ok"], false);
    assert_eq!(other["error"]["code"], "process_local_history_unsupported");
    assert_eq!(other["error"]["stage"], "session/history");

    assert!(
        crate::platform::conversation_lane::process_local_history(&json!({
            "agent": "claude-code"
        }))
        .is_err(),
        "a history read without an exact native session identifier is refused"
    );

    let unbound = history(json!({
        "agent": "claude-code",
        "sessionId": "claude-code-session-that-is-not-registered"
    }));
    assert_eq!(unbound["ok"], false);
    assert_eq!(unbound["error"]["code"], "claude_code_session_unavailable");
    assert_eq!(unbound["error"]["stage"], "session/history");
}

/// A Claude Code permission request parks in the host's own approval registry,
/// and the user's decision resumes the same native turn.
#[test]
fn a_claude_code_permission_request_parks_until_the_host_resolves_it() {
    super::compose();
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let (directory, executable) = compile_fake_claude("lico-claude-approval");
    // A freshly compiled unsigned binary pays a one-time cold-launch policy
    // scan that can exceed the first turn's deliberate 500ms deadline; warm
    // the binary once so that deadline measures the turn, never the OS scan.
    let warm_up = Command::new(&executable).arg("--version").output().unwrap();
    assert!(warm_up.status.success());
    let executable_text = executable.to_string_lossy().to_string();
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let working_dir = directory.clone();
    let executable_for_run = executable_text.clone();
    let run_params = params.clone();
    let run = thread::spawn(move || {
        execute(
            &executable_for_run,
            &run_params,
            "fake-claude-permission-prompt-1",
            "",
            Some(&working_dir),
            500,
            Some(1024 * 1024),
            1024,
        )
    });
    // The turn suspends on the permission request instead of failing; resolve
    // the parked approval to allow and let the turn continue.
    let deadline = Instant::now() + Duration::from_secs(5);
    let token = loop {
        let token = licoup_foundation::platform::native_agent_interaction::pending_token(
            "claude-code",
            "Bash",
        );
        if let Some(token) = token {
            break token;
        }
        if Instant::now() >= deadline {
            panic!("permission request never parked");
        }
        thread::sleep(Duration::from_millis(10));
    };
    // Deliberately exceed the ordinary turn deadline while the native
    // permission route is parked. User decision time is not execution time.
    thread::sleep(Duration::from_millis(550));
    let resolved =
        licoup_agent_drivers::acp_session_transport::resolve_interaction_approval(&token, true)
            .unwrap();
    assert_eq!(resolved["adapterId"], "claude-code");
    let allowed = run.join().unwrap();
    assert!(allowed.ok, "allowed turn failed: {:?}", allowed.error);
    assert_eq!(allowed.output, "fake Claude allowed answer");
    // Release the transport so the second fixture turn binds the shared
    // fixture session to its own fresh process.
    assert_eq!(
        cleanup_session(&allowed.session_id),
        ControlDisposition::Accepted
    );

    // Denying a later permission request resumes the same native turn. The
    // CLI's valid reply and denial metadata remain authoritative.
    let working_dir = directory.clone();
    let executable_for_deny = executable_text.clone();
    let deny_params = params.clone();
    let run = thread::spawn(move || {
        execute(
            &executable_for_deny,
            &deny_params,
            "fake-claude-permission-prompt-1",
            "",
            Some(&working_dir),
            10_000,
            Some(1024 * 1024),
            1024,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let token = loop {
        let token = licoup_foundation::platform::native_agent_interaction::pending_token(
            "claude-code",
            "Bash",
        );
        if let Some(token) = token {
            break token;
        }
        if Instant::now() >= deadline {
            panic!("second permission request never parked");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let denied =
        licoup_agent_drivers::acp_session_transport::resolve_interaction_approval(&token, false)
            .unwrap();
    assert_eq!(denied["adapterId"], "claude-code");
    let turn = run.join().unwrap();
    assert!(turn.ok, "denied turn failed: {:?}", turn.error);
    assert_eq!(turn.output, "fake Claude denied answer");
    assert_eq!(
        cleanup_session(&turn.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

/// A parked Claude Code approval is released when its transport is lost, so a
/// dead process never leaves a route the user could still answer.
#[test]
fn a_lost_claude_code_transport_releases_its_parked_approval() {
    super::compose();
    let _serial = claude_process_local_test_lock::lock_claude_process_local_tests();
    let (directory, executable) = compile_fake_claude("lico-claude-approval-exit");
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-permission-exit-prompt",
        "",
        Some(&directory),
        0,
        Some(1024 * 1024),
        1024,
    );
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "claude_code_exited");
    assert!(
        licoup_foundation::platform::native_agent_interaction::pending_token("claude-code", "Bash")
            .is_none()
    );
    let _ = fs::remove_dir_all(directory);
}
