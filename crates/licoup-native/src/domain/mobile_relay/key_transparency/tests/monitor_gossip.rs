use crate::domain::mobile_relay::key_transparency::gossip::key_transparency_gossip;
use crate::domain::mobile_relay::key_transparency::self_monitor::key_transparency_self_monitor;
use serde_json::json;

#[test]
fn monitor_and_gossip_reject_unknown_input_before_secret_access() {
    for error in [
        key_transparency_self_monitor(&json!({"unexpected": true})).unwrap_err(),
        key_transparency_gossip(&json!({"unexpected": true})).unwrap_err(),
    ] {
        assert!(error.to_string().contains("unsupported field"));
    }
}
