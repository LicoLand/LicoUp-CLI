mod composition;
mod probe;
mod serve_transport;
mod support;

use super::continuity::build_session_create_body;
use super::serve_transport::{
    FirstFailure, build_serve_message_body, endpoint_failure, execute, record_preterminal_failure,
    remaining_turn_timeout, request_failure, sse_failure, turn_timeout_failure, wait_post_json,
    workspace_request_url,
};
use super::{
    OPENCODE_DRIVER, RUNTIME_PROTOCOL, ServeStreamFailure, capability_probe, serve_capabilities,
};
use crate::port::serve::{ServeFramingFailure, ServeRequestFailure};
use licoup_agent_drivers::acp_driver_runtime::{ProtocolConfig, ProtocolFailure};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use support::*;
