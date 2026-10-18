use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn timestamp_after_seconds(seconds: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs().saturating_add(seconds);
    format!("{}.{:09}Z", secs, now.subsec_nanos())
}

pub(super) fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", now.as_secs(), now.subsec_nanos())
}
