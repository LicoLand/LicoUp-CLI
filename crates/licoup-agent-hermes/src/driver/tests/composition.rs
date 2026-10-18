use super::*;

#[test]
fn facade_keeps_the_stable_hermes_runtime_contract() {
    assert_eq!(RUNTIME_PROTOCOL, "hermes-acp-stdio-jsonrpc");
    assert_eq!(HERMES_SESSION_DRIVER.driver_id, "hermes-acp");
    assert_eq!(HERMES_SESSION_DRIVER.launch_args, &["acp"]);
    assert_ne!(
        ControlDisposition::Accepted,
        ControlDisposition::TransportUnavailable
    );
}
