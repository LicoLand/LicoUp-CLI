//! Ports the Gateway Runtime consumes and the composing host installs.
//!
//! Each port is fail-closed: a process that never installs it keeps every call
//! unavailable instead of silently reaching a host implementation.

pub mod readiness;
