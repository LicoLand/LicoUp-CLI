mod command;
mod composition;
mod control;
mod events;
mod execution;
mod failure;
mod io;
mod launch;
mod model;
mod probe;
mod protocol;
mod supervision;
mod support;
mod transport;

#[path = "../../../tests/fixtures/claude_process_local_test_lock.rs"]
mod claude_process_local_test_lock;

use super::control::ControlDisposition;
use super::execution::execute;
use super::failure::ProtocolFailure;
use super::io::{
    MAX_PROTOCOL_LINE_BYTES, TransportEvent, drain_stderr, read_bounded, read_protocol_messages,
};
use super::launch::executable_augmented_path;
use super::model::{CompleteTranscript, RUNTIME_PROTOCOL, RunResult, TransportLifecycle};
use super::probe::probe;
use super::reset::requires_transport_reset;
use super::supervision::{
    cancel, cleanup_session, clear_all_for_test, has_live_session, history,
    lookup_session_transport, steer,
};
use super::transport::PersistentTransport;
use crate::protocol::parser::events::partial_text_delta;
use crate::protocol::parser::{ClaudeCodeParser, ClaudeEffect, interrupt_request};
use crate::protocol::{CapabilityProbe, EffectiveSettings};
use crate::protocol::{DriverConfig, FIXED_STREAM_ARGS, LaunchIdentity};
use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use support::*;
