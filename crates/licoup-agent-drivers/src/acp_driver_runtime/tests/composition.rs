use super::*;

#[test]
fn driver_specs_keep_vendor_identity_out_of_shared_runtime_state() {
    let first =
        AcpDriverSpec::new("first-acp-v1", &["acp"]).with_identity("first-acp", "first_acp");
    let second =
        AcpDriverSpec::new("second-acp-v1", &["--acp"]).with_identity("second-acp", "second_acp");

    assert_ne!(first.agent_id, second.agent_id);
    assert_ne!(first.runtime_protocol, second.runtime_protocol);
    assert_eq!(first.launch_args, &["acp"]);
    assert_eq!(second.launch_args, &["--acp"]);
    assert_eq!(
        ProtocolFailure::new("acp_process_start_failed", "redacted", "process/start")
            .namespaced(first)
            .code,
        "first_acp_process_start_failed"
    );
}
