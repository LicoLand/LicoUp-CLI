use licoup_agent_drivers::acp_session_transport::{
    APPROVAL_POLL_INTERVAL, CONTROL_QUEUE_CAPACITY, MAX_POOLED_TRANSPORTS, MAX_TRACKED_SESSIONS,
    PROCESS_POLL_INTERVAL,
};

#[test]
fn runtime_resources_are_bounded_without_bounding_user_response_time() {
    assert!(PROCESS_POLL_INTERVAL > std::time::Duration::ZERO);
    assert!(APPROVAL_POLL_INTERVAL > std::time::Duration::ZERO);
    assert!(CONTROL_QUEUE_CAPACITY > 0);
    assert!(MAX_POOLED_TRANSPORTS > 0);
    assert!(MAX_TRACKED_SESSIONS >= MAX_POOLED_TRANSPORTS);
}
