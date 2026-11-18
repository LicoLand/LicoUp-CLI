mod codec;
mod composition;
mod continuity;
mod errors;
mod events;
mod execution;
mod interaction;
mod io;
mod model;
mod params;
mod probe;
mod protocol;
mod supervision;
mod support;

use crate::driver::execution::execute;
use crate::driver::io::{TransportEvent, drain_stderr, read_protocol_messages};
use crate::driver::probe::{first_nonempty_line, probe};
use crate::driver::supervision::{ATTACH_ARGS_PREFIX, LaunchSpec, resolve_gateway_endpoint};
use crate::gateway_acp::continuity::{SessionBinding, session_request};
use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::model::{EffectiveSettings, RUNTIME_PROTOCOL, RunResult};
use crate::gateway_acp::params::{ProtocolConfig, normalize_agent_id};
use crate::parser::codec::{
    INITIALIZE_REQUEST_ID, SESSION_REQUEST_ID, decode_message, encode_message, request_id_matches,
};
use crate::parser::events::projected_event;
use crate::parser::protocol::{OpenClawProtocol, ProtocolEffect, ProtocolPhase};
use crate::policy::attach_mode;
use licoup_foundation::core::acp;
use serde_json::{Value, json};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use support::*;
