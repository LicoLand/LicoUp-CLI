use super::super::event_hash::advance_event_chain;
use serde_json::json;

#[test]
fn canonical_chain_ignores_transport_timestamp_and_response_id() {
    let first = json!({
        "timestamp": "2026-07-01T00:00:00Z",
        "type": "response_item",
        "payload": {"type": "message", "id": "first", "role": "user", "text": "same"}
    });
    let second = json!({
        "payload": {"text": "same", "role": "user", "id": "second", "type": "message"},
        "type": "response_item",
        "timestamp": "2026-07-02T00:00:00Z"
    });
    let mut first_chain = String::new();
    let mut second_chain = String::new();
    assert_eq!(
        advance_event_chain(&mut first_chain, b"domain\0", &first),
        advance_event_chain(&mut second_chain, b"domain\0", &second)
    );
}

#[test]
fn canonical_chain_binds_prior_identity_and_content() {
    let value = json!({"type": "event_msg", "payload": {"value": 1}});
    let changed = json!({"type": "event_msg", "payload": {"value": 2}});
    let mut chain = String::new();
    let first = advance_event_chain(&mut chain, b"domain\0", &value);
    let second = advance_event_chain(&mut chain, b"domain\0", &value);
    let mut changed_chain = String::new();
    let changed = advance_event_chain(&mut changed_chain, b"domain\0", &changed);
    assert_ne!(first, second);
    assert_ne!(first, changed);
}
