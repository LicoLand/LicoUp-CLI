use super::*;

#[test]
fn facade_exports_the_stable_runtime_and_control_contract() {
    assert_eq!(RUNTIME_PROTOCOL, "claude-code-cli-stream-json");
    let _execute: fn(
        &str,
        &Value,
        &str,
        &str,
        Option<&Path>,
        u64,
        Option<usize>,
        usize,
    ) -> RunResult = execute;
    let _probe: fn(&str, u64, usize) -> CapabilityProbe = probe;
    let _cancel: fn(&str) -> ControlDisposition = cancel;
}
