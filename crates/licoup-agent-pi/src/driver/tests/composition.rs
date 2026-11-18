use super::*;

#[test]
fn facade_keeps_the_official_rpc_result_and_capability_contracts() {
    assert_eq!(RUNTIME_PROTOCOL, "pi-rpc-stdio-jsonl");
    let _: Option<RunResult> = None;
    let _: Option<ProtocolFailure> = None;
    let _: Option<CapabilityProbe> = None;
}
