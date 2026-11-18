use super::*;

#[test]
fn capability_and_failure_results_preserve_the_stable_projection() {
    let installed = CapabilityProbe::installed(true, false);
    assert!(installed.available && installed.supported && installed.version_command_ok);
    assert!(!installed.help_command_ok);
    let unavailable = CapabilityProbe::unavailable();
    assert_eq!(unavailable.error_code, Some("pi_executable_unavailable"));

    let failure =
        ProtocolFailure::new("pi_test", "Static failure.", "test").with_session(Some("native"));
    let result = RunResult::failed(failure, "0".to_string(), None, false, false);
    assert!(!result.ok);
    assert_eq!(result.session_id, "native");
    assert_eq!(result.effective.cwd, EffectiveSettings::default().cwd);
    assert!(matches!(
        result.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Failed { code, .. })
            if code == "pi_test"
    ));
}
