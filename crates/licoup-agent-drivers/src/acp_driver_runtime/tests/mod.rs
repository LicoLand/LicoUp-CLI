mod composition;
mod continuity;
mod errors;
mod events;
mod interaction;
mod io;
mod model;
mod params;
mod probe;
mod protocol;
mod settings;
mod stdio_transport;
mod supervision;
mod support;

use super::errors::ProtocolFailure;
use super::errors::failure_from_response;
use super::events::{ACP_EVENT_CHANNEL_CAPACITY, read_protocol_messages};
use super::io::drain_stderr;
use super::model::{AcpDriverSpec, CapabilityProbe};
use super::params::{ProtocolConfig, timestamp};
use super::probe::probe_acp;
use super::protocol::{
    AcpProtocol, FIRST_CONFIG_REQUEST_ID, INITIALIZE_REQUEST_ID, PROMPT_REQUEST_ID, ProtocolEffect,
    ProtocolPhase, SESSION_REQUEST_ID,
};
use super::session_plan::{AcpSessionPlan, reconcile_acp_session_id};
use super::settings::{ConfigChange, ConfigValue, requested_config_changes, setting_applied};
use super::stdio_transport::execute_acp;
use super::supervision::LaunchSpec;
use licoup_foundation::core::acp;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::*;
