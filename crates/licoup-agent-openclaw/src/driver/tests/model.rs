use super::*;

#[test]
fn failed_result_projects_only_static_error_and_bound_ids() {
    let failure = ProtocolFailure::new("static_code", "Static message.", "test/stage")
        .with_ids(Some("native-session".to_string()), "turn-1");
    let result = RunResult::failed(failure, "started".to_string(), None, true, false);
    assert!(!result.ok);
    assert_eq!(result.session_id, "native-session");
    assert_eq!(result.turn_id, "turn-1");
    assert!(result.stdout_truncated);
    assert!(matches!(
        result.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Failed { .. })
    ));
    assert_eq!(result.effective.cwd, EffectiveSettings::default().cwd);
}
