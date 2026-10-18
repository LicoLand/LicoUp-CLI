use super::super::allow_list::connectable_relay_targets;
use serde_json::json;

#[test]
fn allow_list_keeps_only_detected_runtime_send_targets() {
    let targets = connectable_relay_targets(&json!([
        {"target": "ready", "status": "ready", "supportedActions": ["runtime.message.send"]},
        {"target": "missing", "status": "not-detected", "supportedActions": ["runtime.message.send"]},
        {"target": "read-only", "status": "ready", "supportedActions": ["runtime.sessions.list"]}
    ]));
    assert_eq!(targets.as_array().unwrap().len(), 1);
    assert_eq!(targets[0]["target"], "ready");
}
