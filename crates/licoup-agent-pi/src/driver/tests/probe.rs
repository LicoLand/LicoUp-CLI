use super::*;

#[test]
fn missing_probe_executable_reports_unavailable_without_output_projection() {
    let capability = probe("lico-pi-definitely-missing-executable", 10, 16);
    assert!(!capability.available);
    assert!(!capability.supported);
    assert_eq!(capability.error_code, Some("pi_executable_unavailable"));
}
