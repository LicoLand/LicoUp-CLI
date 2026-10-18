use super::*;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn bounded_read_keeps_only_the_configured_prefix() {
    let (kept, truncated) = read_bounded(Cursor::new(b"private-runtime-output"), 7);
    assert_eq!(kept, b"private");
    assert!(truncated);
}

#[test]
fn stderr_drain_records_only_truncation_not_content() {
    let truncated = AtomicBool::new(false);
    drain_stderr(Cursor::new(b"private-stderr-canary"), 4, &truncated);
    assert!(truncated.load(Ordering::Relaxed));
}
