use super::*;

#[test]
fn foundation_exposes_http_sse_and_lifecycle_without_jsonl_ownership() {
    let endpoint = ServeEndpoint::new("127.0.0.1", 24173);
    assert_eq!(endpoint.attach_url, "http://127.0.0.1:24173");
    assert_eq!(
        licoup_foundation::core::acp::MAX_JSON_LINE_BYTES,
        1024 * 1024 + 2
    );
}

#[test]
fn endpoint_generation_stays_pinned_until_its_attachment_guard_drops() {
    let attach_url = "http://127.0.0.1:49152";
    assert!(!turn_control::endpoint_has_active_turn(attach_url));
    let guard = turn_control::pin_endpoint(attach_url).unwrap();
    assert!(turn_control::endpoint_has_active_turn(attach_url));
    drop(guard);
    assert!(!turn_control::endpoint_has_active_turn(attach_url));
}
