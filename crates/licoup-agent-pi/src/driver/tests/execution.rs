use super::*;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};
use std::time::{Duration, Instant};

#[test]
fn fake_child_completes_rpc_with_bounded_stderr_and_native_session() {
    let (directory, executable) = compile_fake_pi("lico-pi-rpc-fake");
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-pi-prompt",
        "",
        Some(directory.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "pi rpc failure: {:?}", result.error);
    assert_eq!(result.output, "pi-ok");
    assert_eq!(result.session_id, "pi-native-fake-1");
    assert!(matches!(
        result.transitions.last(),
        Some(Transition::Lifecycle(LifecycleStage::Completed))
    ));
    assert!(result.stderr_truncated);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn provider_credential_error_is_not_reported_as_missing_final_message() {
    let (directory, executable) = compile_fake_pi("lico-pi-rpc-credential");
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "credential-case",
        "",
        Some(directory.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(!result.ok);
    let error = result.error.expect("typed failure");
    assert_eq!(error.code, "pi_gateway_credentials_unavailable");
    assert!(error.message.contains("authorized API keys"));
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn stalled_rpc_turn_times_out_and_cleans_up_the_child_tree() {
    let (directory, executable) = compile_fake_pi("lico-pi-rpc-timeout");
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "timeout-case",
        "",
        Some(directory.as_path()),
        50,
        Some(1024 * 1024),
        1024,
    );
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "pi_rpc_timeout");
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn interaction_wait_does_not_consume_the_turn_deadline() {
    let started_at = Instant::now();
    let original_deadline = started_at + Duration::from_millis(500);
    let parked_at = started_at + Duration::from_millis(100);
    let resumed_at = started_at + Duration::from_secs(2);

    let resumed_deadline =
        extend_deadline_for_pause(Some(original_deadline), parked_at, resumed_at).unwrap();
    assert_eq!(
        resumed_deadline,
        original_deadline + resumed_at.duration_since(parked_at)
    );
    assert!(resumed_at < resumed_deadline);
    assert_eq!(extend_deadline_for_pause(None, parked_at, resumed_at), None);
}

#[test]
fn interaction_route_resumes_the_same_live_turn() {
    let (directory, executable) = compile_fake_pi("lico-pi-rpc-interaction");
    let executable = executable.to_string_lossy().to_string();
    let working_directory = directory.clone();
    let run = std::thread::spawn(move || {
        execute(
            &executable,
            &json!({}),
            "interaction-case",
            "",
            Some(working_directory.as_path()),
            10_000,
            Some(1024 * 1024),
            1024,
        )
    });
    let route = licoup_foundation::platform::native_agent_interaction::wait_for_pending_route(
        "pi",
        "pi-native-interaction-1",
        "Synthetic confirmation",
        Duration::from_secs(5),
    )
    .expect("Pi should park the exact interaction route");
    assert_eq!(route.adapter_id, "pi");
    assert_eq!(route.session_id, "pi-native-interaction-1");
    assert_eq!(route.summary, "Synthetic confirmation");
    licoup_foundation::platform::native_agent_interaction::resolve_scoped(
        &route.token,
        Some(&route.session_id),
        Some(&route.turn_id),
        json!({"confirmed": true}),
    )
    .unwrap();

    let result = run.join().unwrap();
    assert!(result.ok, "Pi interaction turn failed: {:?}", result.error);
    assert_eq!(result.output, "pi-interaction-ok");
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn interaction_park_reports_transport_loss_and_releases_its_route() {
    let (directory, executable) = compile_fake_pi("lico-pi-rpc-interaction-exit");
    let executable = executable.to_string_lossy().to_string();
    let working_directory = directory.clone();
    let run = std::thread::spawn(move || {
        execute(
            &executable,
            &json!({}),
            "interaction-exit-case",
            "",
            Some(working_directory.as_path()),
            0,
            Some(1024 * 1024),
            1024,
        )
    });
    let route = licoup_foundation::platform::native_agent_interaction::wait_for_pending_route(
        "pi",
        "pi-native-interaction-exit-1",
        "Synthetic confirmation",
        Duration::from_secs(5),
    )
    .expect("Pi should park the exact interaction route before transport loss");
    fs::write(directory.join("release-interaction-exit"), b"").unwrap();
    let result = run.join().unwrap();
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "pi_rpc_exited");
    assert_eq!(
        licoup_foundation::platform::native_agent_interaction::resolve_scoped(
            &route.token,
            Some(&route.session_id),
            Some(&route.turn_id),
            json!({"confirmed": true}),
        ),
        Err("native_interaction_route_missing")
    );
    let _ = fs::remove_dir_all(directory);
}
