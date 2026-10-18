use super::super::{DEFAULT_PORT, is_reserved_conflict_port, policy, select_available_port_with};

#[test]
fn target_policy_owns_ports_identity_and_static_failures() {
    assert_eq!(DEFAULT_PORT, 24173);
    assert!(!is_reserved_conflict_port(DEFAULT_PORT));
    assert!(is_reserved_conflict_port(4096));
    assert!(is_reserved_conflict_port(18789));
    assert_eq!(policy::SPEC.identity, "opencode_serve");
    assert_eq!(
        policy::SPEC.errors.executable_missing,
        "opencode_executable_missing"
    );
    assert_eq!(
        select_available_port_with(4096, |port| port == 4097).unwrap(),
        4097
    );
}
