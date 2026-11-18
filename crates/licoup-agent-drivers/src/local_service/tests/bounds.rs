use super::super::bounds::*;

#[test]
fn every_external_buffer_and_concurrency_lane_has_a_finite_ceiling() {
    assert!(MAX_HTTP_REQUEST_BODY_BYTES > 0);
    assert!(MAX_HTTP_RESPONSE_BODY_BYTES >= MAX_HTTP_REQUEST_BODY_BYTES);
    assert!(MAX_HTTP_HEADER_COUNT > 0);
    assert!(MAX_HTTP_HEADER_BYTES > 0);
    assert!(MAX_HTTP_IN_FLIGHT > MAX_SSE_STREAMS);
    assert!(MAX_SSE_LINE_BYTES < MAX_SSE_FRAME_BYTES);
    assert!(MAX_SSE_DATA_LINES > 0);
    assert!(MAX_SSE_EVENTS_PER_STREAM > 0);
    assert!(MAX_PRIVATE_STATE_BYTES > MAX_PID_BYTES);
}
