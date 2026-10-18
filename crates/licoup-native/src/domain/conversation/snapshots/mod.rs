use crate::domain::conversation::snapshot_codec::{RawExport, export_raw_content};
use crate::domain::conversation::snapshot_collection::{
    COLLECTION_SCHEMA_VERSION, ProfileMatch, archive_index_record, archive_key_for_session,
    archive_match_record, archive_status_for, build_collection, collection_summary,
    empty_collection, existing_conversations, upsert_conversation_record,
};
use crate::domain::conversation::snapshot_content::{
    candidate_has_real_conversation, looks_like_archive_database_record,
    looks_like_archive_text_conversation,
};
use crate::domain::conversation::snapshot_identity::{candidate_id, native_identity, text_value};
use crate::domain::conversation_semantic::hash_text;
use crate::domain::conversations;
use crate::domain::targets;
use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, anyhow, ensure};
use licoup_foundation::platform::file_security::atomic_write_private_text;
use licoup_foundation::platform::paths::portable_data_dir;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MARKER_FILE: &str = ".lico-native-conversation-snapshots.json";
const COLLECTION_JSON: &str = "collection.json";
const SNAPSHOT_JSON: &str = "snapshot.json";
const DEFAULT_SNAPSHOT_ROOT_DIR: &str = "native-conversation-snapshots";
const SETTINGS_COLLECTION: &str = "settings";
const TARGETS_COLLECTION: &str = "targets";
const PROFILES_COLLECTION: &str = "conversation-archive-profiles";
const CONVERSATION_INDEX_JSONL: &str = "conversation-index.jsonl";
const CONVERSATION_INDEX_MD: &str = "conversation-index.md";
const SUMMARY_MD: &str = "summary.md";
const SOURCES_JSON: &str = "sources.json";
const MATCHES_JSONL: &str = "matches.jsonl";
const VALIDATION_JSON: &str = "validation.json";
const SEMANTIC_JSON: &str = "semantic.json";
const SEMANTIC_MD: &str = "semantic.md";

const SUPPORTED_AGENTS: &[&str] = &[
    "antigravity",
    "claude-code",
    "code",
    "codex",
    "copilot",
    "cursor",
    "hermes",
    "kilo-code",
    "openclaw",
    "opencode",
];

#[derive(Clone, Debug)]
struct SnapshotRoot {
    path: PathBuf,
    mode: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveCollectionLayout {
    CollectionsSubdir,
    DirectKeywordFolders,
}

#[derive(Clone, Debug)]
struct SelectedCandidate {
    session: Value,
    selection_mode: String,
    reason: String,
    labels: Vec<String>,
    group: String,
    summary: String,
}

#[derive(Clone, Debug)]
struct KeywordArchiveRun {
    position: usize,
    keyword: String,
    run: Value,
}

#[derive(Clone, Debug)]
struct DiscoveryResult {
    agents: Vec<String>,
    candidates: Vec<Value>,
    source_summaries: Vec<Value>,
    diagnostics: Vec<Value>,
}

#[derive(Clone, Debug)]
struct ArchiveProfile {
    profile_id: String,
    display_name: String,
    collection_path_segments: Vec<String>,
    archive_root: Option<PathBuf>,
    canonical_names: Vec<String>,
    alias_names: Vec<String>,
    project_paths: Vec<String>,
    expected_agents: Vec<String>,
    expected_sources: Vec<String>,
    exclusion_rules: Vec<String>,
    baseline_index_path: Option<PathBuf>,
    raw: Value,
}

#[derive(Clone, Debug)]
struct ArchiveMaterializeWorkItem {
    position: usize,
    selected: SelectedCandidate,
    profile_match: ProfileMatch,
}

#[derive(Debug)]
struct ArchiveMaterializeResult {
    position: usize,
    archive_key: String,
    session: Value,
    profile_match: ProfileMatch,
    record: Value,
}

#[derive(Clone, Debug)]
struct DerivedArchiveIdentity {
    profile_id: String,
    display_name: String,
    canonical_names: Vec<String>,
    alias_names: Vec<String>,
    collection_path_segments: Vec<String>,
}

mod discovery;
mod materialization;
mod orchestration;
mod privacy_projection;
mod relocation;
mod reporting;
mod selection;
mod selection_plan;
mod settings;
mod support;
mod validation;

use discovery::*;
use materialization::*;
use orchestration::run_archive_with_profile_discovery;
use privacy_projection::*;
use reporting::*;
use selection::*;
use selection_plan::{ALL_SELECTION, EXACT_KEYWORD_SELECTION};
use settings::*;
use support::*;
use validation::*;

pub(crate) use orchestration::{archive_collect, archive_run, collect};
pub(crate) use relocation::relocate_copied_data_home_references;
pub(crate) use reporting::archive_report;
pub(crate) use selection_plan::{archive_selection_collect, archive_selection_preview};
pub(crate) use settings::{
    collections_list, profile_get, profile_import, profiles_list, root_get, root_set,
};
pub(crate) use validation::archive_verify;

#[cfg(test)]
mod tests;
