mod composition;
mod errors;
mod execution;
mod interaction;
mod io;
mod model;
mod params;
mod parser_events;
mod parser_protocol;
mod probe;
mod sessions;
mod settings;
mod supervision;
mod support;

use super::errors::ProtocolFailure;
use super::execution::{execute, extend_deadline_for_pause};
use super::io::{TransportEvent, drain_stderr, read_protocol_messages};
use super::model::{CapabilityProbe, EffectiveSettings, RUNTIME_PROTOCOL, RunResult};
use super::params::ProtocolConfig;
use super::probe::probe;
use super::sessions::{
    resolve_session_path_in_roots, session_header_matches, session_roots_from_sources,
};
use super::supervision::{LAUNCH_ARGS, LaunchSpec};
use crate::parser::{PiProtocol, ProtocolEffect, decode_jsonl_line, sanitized_event};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use support::*;
