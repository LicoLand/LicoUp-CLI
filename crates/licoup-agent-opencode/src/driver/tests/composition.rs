use super::*;

#[test]
fn facade_keeps_serve_and_shared_acp_contracts() {
    assert_eq!(RUNTIME_PROTOCOL, "opencode-serve-http-v1");
    assert_eq!(OPENCODE_DRIVER.launch_args, &["serve"]);
    let capabilities = serve_capabilities();
    assert!(capabilities.load_session);
    assert!(capabilities.resume_session);
    assert!(capabilities.list_sessions);
}
