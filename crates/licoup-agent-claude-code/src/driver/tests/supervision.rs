use super::*;

#[test]
fn unknown_session_never_spawns_or_accepts_control() {
    let session = "claude-code-test-session-that-is-not-registered";
    assert!(!has_live_session(session));
    assert_eq!(cancel(session), ControlDisposition::SessionUnavailable);
    assert_eq!(
        cleanup_session(session),
        ControlDisposition::SessionUnavailable
    );
}

#[cfg(unix)]
#[test]
fn cleanup_detaches_before_wait_and_acknowledges_after_tree_and_io_exit() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-cleanup-tree");
    let executable = executable.to_string_lossy().to_string();
    let first = execute(
        &executable,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1 fake-claude-retained-pipe",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok, "fixture turn failed: {:?}", first.error);
    let descendant_pid = wait_for_descendant_pid(&directory);
    assert!(process_exists(descendant_pid));

    let session_id = first.session_id.clone();
    let managed = lookup_session_transport(&session_id).unwrap();
    assert!(managed.lifecycle.is_live());
    let transport_gate = managed.transport.lock().unwrap();
    let cleanup_session_id = session_id.clone();
    let cleanup = thread::spawn(move || cleanup_session(&cleanup_session_id));
    assert!(managed.lifecycle.wait_until_closing(Duration::from_secs(1)));
    assert!(managed.lifecycle.is_closing());
    assert!(!has_live_session(&session_id));
    assert!(lookup_session_transport(&session_id).is_none());
    assert!(
        !cleanup.is_finished(),
        "cleanup crossed the deterministic transport gate before it was released"
    );

    // A closing transport is never entered; resume hands off to a fresh
    // --resume process that stays out of the deterministic transport gate.
    let resume = execute(
        &executable,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        &session_id,
        Some(&directory),
        5_000,
        Some(1024),
        1024,
    );
    assert!(
        resume.ok,
        "resume around closing transport failed: {:?}",
        resume.error
    );
    assert_eq!(resume.session_id, session_id);
    // The fresh resume transport must be released before the shared fixture
    // session can be observed as fully cleaned up.
    assert_eq!(cleanup_session(&session_id), ControlDisposition::Accepted);
    drop(transport_gate);
    assert_eq!(cleanup.join().unwrap(), ControlDisposition::Accepted);
    assert!(managed.lifecycle.wait_until_closed(Duration::from_secs(1)));
    assert!(managed.lifecycle.is_closed());
    assert!(!process_exists(descendant_pid));
    assert!(!has_live_session(&session_id));
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn cleanup_propagates_a_poisoned_transport_shutdown_failure() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-cleanup-failure");
    let executable = executable.to_string_lossy().to_string();
    let first = execute(
        &executable,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok);
    let managed = lookup_session_transport(&first.session_id).unwrap();
    let poisoned = Arc::clone(&managed);
    assert!(
        thread::spawn(move || {
            let _guard = poisoned.transport.lock().unwrap();
            panic!("synthetic transport-lock failure");
        })
        .join()
        .is_err()
    );

    assert_eq!(
        cleanup_session(&first.session_id),
        ControlDisposition::TransportUnavailable
    );
    assert!(!has_live_session(&first.session_id));
    drop(managed);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn test_cleanup_clears_the_exact_registry_and_is_idempotent() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-shutdown-all");
    let first = execute(
        &executable.to_string_lossy(),
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok);
    assert!(has_live_session(&first.session_id));

    assert_eq!(clear_all_for_test(), ControlDisposition::Accepted);
    assert!(!has_live_session(&first.session_id));
    assert_eq!(clear_all_for_test(), ControlDisposition::Accepted);

    let reclaimed_directory = directory.join("capacity-reclaimed");
    fs::create_dir_all(&reclaimed_directory).unwrap();
    let reclaimed = execute(
        &executable.to_string_lossy(),
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        "",
        Some(&reclaimed_directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(
        reclaimed.ok,
        "test cleanup did not reclaim transport capacity"
    );
    assert!(has_live_session(&reclaimed.session_id));
    assert_eq!(clear_all_for_test(), ControlDisposition::Accepted);
    assert!(!has_live_session(&reclaimed.session_id));
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn bounded_pool_rejects_overflow_then_exact_cleanup_reclaims_one_slot() {
    let _serial = process_local_test_guard();
    assert_eq!(clear_all_for_test(), ControlDisposition::Accepted);
    let (directory, executable) = compile_fake_claude("lico-claude-capacity");
    let executable = executable.to_string_lossy().to_string();
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let mut sessions = Vec::new();
    for index in 0..8 {
        let working_directory = directory.join(format!("capacity-{index}"));
        fs::create_dir_all(&working_directory).unwrap();
        let result = execute(
            &executable,
            &params,
            "fake-claude-private-prompt-1",
            "",
            Some(&working_directory),
            10_000,
            Some(1024 * 1024),
            1024,
        );
        assert!(
            result.ok,
            "bounded transport {index} failed: {:?}",
            result.error
        );
        assert!(sessions.iter().all(|session| session != &result.session_id));
        sessions.push(result.session_id);
    }

    let overflow_directory = directory.join("capacity-overflow");
    fs::create_dir_all(&overflow_directory).unwrap();
    let overflow = execute(
        &executable,
        &params,
        "fake-claude-private-prompt-1",
        "",
        Some(&overflow_directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert_eq!(
        overflow.error.unwrap().code,
        "claude_code_transport_capacity"
    );

    let reclaimed_session = sessions.remove(3);
    assert_eq!(
        cleanup_session(&reclaimed_session),
        ControlDisposition::Accepted
    );
    assert!(!has_live_session(&reclaimed_session));
    assert!(sessions.iter().all(|session| has_live_session(session)));

    let replacement = execute(
        &executable,
        &params,
        "fake-claude-private-prompt-1",
        "",
        Some(&overflow_directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(
        replacement.ok,
        "exact cleanup did not reclaim one bounded slot"
    );
    assert_ne!(replacement.session_id, reclaimed_session);
    assert!(sessions.iter().all(|session| has_live_session(session)));
    assert_eq!(clear_all_for_test(), ControlDisposition::Accepted);
    assert!(sessions.iter().all(|session| !has_live_session(session)));
    assert!(!has_live_session(&replacement.session_id));
    let _ = fs::remove_dir_all(directory);
}
