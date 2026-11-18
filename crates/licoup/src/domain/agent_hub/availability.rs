//! Cached Agent availability facts, and the conditions that invalidate them.
//!
//! Reading one Agent's availability is expensive on purpose: the Hub resolves the
//! executable the scan found, spawns the vendor's own version probe (bounded, but
//! a real process) and reads the local package manager's metadata. A desktop that
//! paints a card per Agent would pay that cost once per card, so the answer is
//! remembered — and a remembered answer that outlives its truth is worse than no
//! cache at all, which is why every entry carries the conditions that retire it.
//!
//! # What is cached
//!
//! One entry per Agent **and channel**: the same Agent installed through two
//! channels is two facts, and a fact observed for a channel nobody selected is
//! not an answer about the channel that is selected now.
//!
//! # What invalidates an entry
//!
//! Three conditions, each one a fact the caller already holds:
//!
//! * **Freshness** — an entry observed more than [`FRESHNESS_WINDOW_MS`] ago is
//!   not used ([`EXPIRED`]). The window is short enough that a user who installed
//!   an Agent outside LicoUp is not told otherwise for long.
//! * **Scan generation** — an entry recorded under another platform scan
//!   generation is not used ([`SUPERSEDED`]), so a rescan observes again.
//! * **Explicit invalidation** — [`invalidate`] retires one Agent or every Agent,
//!   and the Hub calls it after an install, update or uninstall it performed, so
//!   the operation that changed the machine is the one that retires the fact.
//!
//! A record that cannot be read is reported as [`UNREADABLE`] and no entry is
//! used: an unreadable cache is not evidence about this Host.

use super::contract::PlatformInstallCapabilities;
use super::ownership::hub_dir;
use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
    remove_private_state_marker,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const AVAILABILITY_FILE: &str = "availability-cache.json";
const AVAILABILITY_SCHEMA: &str = "v0.0.1:client-agent-availability-cache-1";
const MAX_AVAILABILITY_BYTES: usize = 256 * 1024;

/// How long one observation stays usable.
pub const FRESHNESS_WINDOW_MS: i64 = 5 * 60 * 1000;

/// The cached fact was observed within the freshness window.
pub const FRESH: &str = "fresh";
/// No entry exists for this Agent and channel.
pub const ABSENT: &str = "absent";
/// The entry exists but was observed longer ago than the window.
pub const EXPIRED: &str = "expired";
/// The entry exists but belongs to another platform scan generation.
pub const SUPERSEDED: &str = "superseded";
/// The record exists but could not be read, so nothing was used.
pub const UNREADABLE: &str = "unreadable";
/// The read observed nothing: no live lookup was requested, so the cache was
/// neither consulted nor written.
pub const NOT_REQUESTED: &str = "not-requested";

/// One Agent's observed availability, as this Host read it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityFact {
    pub agent_id: String,
    /// The channel the fact is about; empty when no channel was selected.
    pub channel_id: String,
    pub present: bool,
    pub installed_version: String,
    pub latest_version: String,
    pub probed_version: String,
    /// The version-domain verdict this observation reached, in
    /// [`super::version_support`]'s own vocabulary. A record carries the fact it
    /// classified, so the read path answers from the observation instead of
    /// classifying a version the record no longer holds.
    pub version_status: String,
    /// The stable reason code behind that verdict, empty when it was supported.
    pub version_reason: String,
    /// The range that decided the verdict, when one did.
    pub version_range: String,
    pub observed_at_unix_ms: i64,
    pub scan_generation: u64,
}

/// One cached fact, or the reason none was used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedAvailability {
    pub fact: Option<AvailabilityFact>,
    pub state: &'static str,
}

impl CachedAvailability {
    fn refused(state: &'static str) -> Self {
        Self { fact: None, state }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailabilityDocument {
    schema_version: String,
    #[serde(default)]
    facts: Vec<AvailabilityFact>,
}

/// The fact this Host may reuse for one Agent and channel, or why it may not.
pub fn lookup(
    store: &ClientStateStore,
    agent_id: &str,
    channel_id: &str,
    scan_generation: u64,
    now_unix_ms: i64,
) -> CachedAvailability {
    let document = match load(store) {
        Ok(Some(document)) => document,
        // No record is not a broken record: this Host simply has not observed
        // this Agent and channel yet.
        Ok(None) => return CachedAvailability::refused(ABSENT),
        Err(_) => return CachedAvailability::refused(UNREADABLE),
    };
    let Some(fact) = document
        .facts
        .into_iter()
        .find(|fact| fact.agent_id == agent_id && fact.channel_id == channel_id)
    else {
        return CachedAvailability::refused(ABSENT);
    };
    if fact.scan_generation != scan_generation {
        return CachedAvailability::refused(SUPERSEDED);
    }
    if now_unix_ms.saturating_sub(fact.observed_at_unix_ms) > FRESHNESS_WINDOW_MS {
        return CachedAvailability::refused(EXPIRED);
    }
    CachedAvailability {
        fact: Some(fact),
        state: FRESH,
    }
}

/// The scan generation a fact observed under, from the capability snapshot.
pub fn generation_of(capabilities: &PlatformInstallCapabilities) -> u64 {
    capabilities.scan_generation
}

/// Remember one observation.
///
/// One Agent and channel holds one fact; recording replaces it, so a cache never
/// accumulates superseded observations of the same Agent.
pub fn record(store: &ClientStateStore, fact: AvailabilityFact) -> Result<()> {
    ensure!(
        !fact.agent_id.trim().is_empty(),
        "agent availability fact names no agent"
    );
    // A record this writer cannot read is replaced rather than preserved: the
    // cache is rebuildable, so a corrupt record must not make every later
    // observation unrecordable.
    let mut document = match load(store) {
        Ok(Some(document)) => document,
        _ => empty_document(),
    };
    document
        .facts
        .retain(|entry| !(entry.agent_id == fact.agent_id && entry.channel_id == fact.channel_id));
    document.facts.push(fact);
    persist(store, &document)
}

/// Retire one Agent's cached facts, or every Agent's when none is named.
pub fn invalidate(store: &ClientStateStore, agent_id: Option<&str>) -> Result<()> {
    let Some(mut document) = load(store)? else {
        return Ok(());
    };
    match agent_id {
        Some(agent_id) => document.facts.retain(|fact| fact.agent_id != agent_id),
        None => document.facts.clear(),
    }
    if document.facts.is_empty() {
        let _ = remove_private_state_marker(&availability_path(store))?;
        return Ok(());
    }
    persist(store, &document)
}

/// The invalidation conditions this cache publishes, for the Hub's contract.
pub fn invalidation_conditions() -> Value {
    json!({
        "freshnessWindowMs": FRESHNESS_WINDOW_MS,
        "conditions": ["freshness-window", "platform-scan-generation", "explicit-invalidation"],
        "states": [FRESH, ABSENT, EXPIRED, SUPERSEDED, UNREADABLE, NOT_REQUESTED]
    })
}

/// The observation clock, in Unix milliseconds.
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

fn availability_path(store: &ClientStateStore) -> std::path::PathBuf {
    hub_dir(store).join(AVAILABILITY_FILE)
}

fn empty_document() -> AvailabilityDocument {
    AvailabilityDocument {
        schema_version: AVAILABILITY_SCHEMA.to_owned(),
        facts: Vec::new(),
    }
}

fn load(store: &ClientStateStore) -> Result<Option<AvailabilityDocument>> {
    let Some(raw) = read_private_text_bounded(&availability_path(store), MAX_AVAILABILITY_BYTES)?
    else {
        return Ok(None);
    };
    ensure!(
        raw.len() <= MAX_AVAILABILITY_BYTES,
        "agent availability cache exceeds its bounded size"
    );
    let document: AvailabilityDocument = serde_json::from_str(&raw)?;
    ensure!(
        document.schema_version == AVAILABILITY_SCHEMA,
        "agent availability cache schema is unsupported"
    );
    Ok(Some(document))
}

fn persist(store: &ClientStateStore, document: &AvailabilityDocument) -> Result<()> {
    ensure_private_dir(&hub_dir(store))?;
    let mut raw = serde_json::to_string(document)?;
    ensure!(
        raw.len() <= MAX_AVAILABILITY_BYTES,
        "agent availability cache exceeds its bounded size"
    );
    raw.push('\n');
    atomic_write_private_text(&availability_path(store), &raw)
}
