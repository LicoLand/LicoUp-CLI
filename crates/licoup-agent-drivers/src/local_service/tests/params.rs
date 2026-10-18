use super::super::params;
use serde_json::json;

#[test]
fn typed_params_trim_text_reject_zero_ports_and_accept_bounded_numbers() {
    let value = json!({"binary": "  agent  ", "port": "4097", "timeout": 250});
    assert_eq!(
        params::text(&value, &["executable", "binary"]).as_deref(),
        Some("agent")
    );
    assert_eq!(params::u16_value(&value, &["port"]), Some(4097));
    assert_eq!(params::u64_value(&value, &["timeout"]), Some(250));
    assert_eq!(params::u16_value(&json!({"port": 0}), &["port"]), None);
}
