use std::time::Duration;

pub const INITIALIZE_REQUEST_ID: i64 = 1;
pub const THREAD_REQUEST_ID: i64 = 2;
pub const TURN_REQUEST_ID: i64 = 3;
pub const ACCOUNT_RATE_LIMITS_REQUEST_ID: i64 = 4;
pub const THREAD_UNARCHIVE_REQUEST_ID: i64 = 5;
pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
