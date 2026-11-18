use std::time::Duration;

pub const MAX_HTTP_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_HTTP_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_HTTP_HEADER_COUNT: usize = 128;
pub const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_HTTP_IN_FLIGHT: usize = 16;
pub const MAX_SSE_STREAMS: usize = 8;
pub const MAX_SSE_LINE_BYTES: usize = 256 * 1024;
pub const MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_SSE_DATA_LINES: usize = 256;
pub const MAX_SSE_EVENTS_PER_STREAM: usize = 16 * 1024;
pub const MAX_PRIVATE_STATE_BYTES: usize = 64 * 1024;
pub const MAX_PID_BYTES: usize = 16;
pub const CONCURRENCY_WAIT: Duration = Duration::from_millis(500);
pub const OPERATION_LOCK_WAIT: Duration = Duration::from_secs(2);
pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const PROCESS_GRACEFUL_POLLS: usize = 40;
pub const PROCESS_FORCE_POLLS: usize = 20;
