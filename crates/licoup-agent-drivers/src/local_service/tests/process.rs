use super::super::process;

#[test]
fn empty_and_zero_pid_identity_never_reports_alive() {
    assert!(!process::alive(None));
    assert!(!process::alive(Some(0)));
}
