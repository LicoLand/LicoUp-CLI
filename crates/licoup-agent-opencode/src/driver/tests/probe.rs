use super::*;

#[test]
fn serve_probe_rejects_relative_working_directory_before_launch() {
    let failure = capability_probe("unused", Path::new("relative"), 10, Some(16), 16).unwrap_err();
    assert_eq!(failure.code, "opencode_serve_working_directory_invalid");
}
