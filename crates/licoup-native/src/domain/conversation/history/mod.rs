//! Native conversation history implementation behind the public facade.

use crate::domain::conversation::adapter_dispatch::parse_history_file;
use crate::domain::conversation::history_discovery::{
    HistoryDiscoveryOptions, discover_history_files,
};
use crate::domain::conversation::parameters::{
    agent_param, number_param, param_bool, string_list_param,
};
use crate::domain::conversation::source_catalog::{
    HistoryAdapter, adapter_for_agent, history_roots,
};
use crate::domain::conversation::usage::{extract_token_usage, token_count_value};
use anyhow::{Result, anyhow};
#[cfg(test)]
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const CONVERSATION_SCHEMA_VERSION: u32 = 2;
const MAX_HISTORY_PAGE_LIMIT: usize = 500;
const DEFAULT_HISTORY_MESSAGE_LIMIT: usize = 20;
const ARCHIVE_DISCOVERY_PREVIEW_MESSAGES: usize = 12;
const ARCHIVE_DISCOVERY_PREVIEW_TEXT_CHARS: usize = 8_000;

#[derive(Clone, Debug)]
pub(crate) struct HistoryScanConfig {
    pub(crate) archive_mode: bool,
    session_ids: Vec<String>,
    match_terms: Vec<String>,
    match_project_paths: Vec<String>,
    pub(crate) page: HistoryPageConfig,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryPageConfig {
    pub(crate) offset: usize,
    pub(crate) limit: Option<usize>,
}

mod catalog;
mod codex;
mod cursor_openagent;
mod delegated_transcripts;
mod execution_provenance;
mod generic;
mod kimi;
mod message_projection;
mod pi_copilot;
mod project_workspace;
mod projection_cache;
mod query;
mod query_filter;
mod runtime_observation;
mod session_merge;
mod session_metadata;

/// The composition installs the environment port conversation history asks.
pub(crate) use runtime_observation::install_open_codex_rollouts;

#[allow(unused_imports)]
use codex::*;
#[allow(unused_imports)]
use generic::*;
#[allow(unused_imports)]
use kimi::*;
#[allow(unused_imports)]
use message_projection::{
    HistoryMessageKind, background_context_prompt_text, clean_native_message_text,
    extract_antigravity_user_request, extract_native_model, extract_native_session_id,
    extract_role, extract_text, extract_timestamp, extract_user_image_attachments, find_string,
    generated_control_text, history_message_kind_from_semantic, native_history_message_id,
    native_message_timestamp, normalize_history_message_semantic, plain_history_message,
    strip_antigravity_artifact_noise, strip_generated_context_blocks, structured_history_message,
};
#[allow(unused_imports)]
use pi_copilot::*;
#[allow(unused_imports)]
use query::*;
#[allow(unused_imports)]
use query_filter::*;
use session_merge::collect_history_model_names;
#[allow(unused_imports)]
use session_metadata::*;

pub(crate) use codex::parse_codex_rollout_sessions;
pub(crate) use cursor_openagent::parse_sqlite_sessions;
pub(crate) use generic::{parse_json_sessions, parse_jsonl_sessions, parse_text_session};
pub(crate) use kimi::parse_kimi_code_wire_session;
pub(crate) use pi_copilot::{
    parse_copilot_transcript_session, parse_lico_agent_session, parse_pi_session,
};
pub(crate) use query::{
    conversation_append, conversation_delete, conversation_list, conversation_stream, model_catalog,
};
pub(crate) use session_merge::{
    apply_codex_session_index_titles, collapse_sessions_by_native_identity,
    dedupe_history_sessions, finalize_history_sessions, paged_history_sessions,
    sort_sessions_by_updated_at,
};

#[cfg(test)]
mod tests;
