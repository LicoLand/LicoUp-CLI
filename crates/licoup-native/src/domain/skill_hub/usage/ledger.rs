//! Daily skill-usage ledger shared by two record sources.
//!
//! - Runtime live events ([`RecordSource::Runtime`]) require an approved agent
//!   pairing and accept only sanitized identifiers reported by that local
//!   agent.
//! - History backfill ([`RecordSource::Backfill`]) accepts any locally
//!   discovered agent and any well-formed sanitized skill id: the backfill
//!   scanner projects only aggregate counts from local transcripts, the same
//!   privacy posture as the agent-usage token scanner.

use crate::domain::skill_hub::{
    ClientStateStore, Result, Value, collection_items_mut, is_agent_approved, json,
};
use anyhow::{anyhow, ensure};
use fs2::FileExt;
use licoup_foundation::platform::file_security::open_private_lock_file;
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) const COLLECTION: &str = "skill-usage";
const LOCK_FILE: &str = ".skill-usage.lock";
const MAX_DAY_COUNT: u64 = 1_000_000_000;
const MAX_SKILL_ID_BYTES: usize = 128;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum RecordSource {
    Runtime,
    Backfill,
}

impl RecordSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Runtime => "runtime-skill-invocation-events",
            Self::Backfill => "history-backfill-scan",
        }
    }
}

pub(super) fn record_counts(
    store: &ClientStateStore,
    agent_id: &str,
    counts: BTreeMap<String, u64>,
    occurred_at: OffsetDateTime,
    source: RecordSource,
) -> Result<Value> {
    if counts.is_empty() {
        return Ok(receipt(agent_id, 0, source));
    }
    let selected = match source {
        RecordSource::Runtime => {
            ensure!(
                is_agent_approved(store, agent_id)?,
                "runtime skill usage requires an approved agent pairing"
            );
            counts
                .into_iter()
                .filter(|(skill_id, _)| is_sanitized_skill_id(skill_id))
                .collect::<BTreeMap<_, _>>()
        }
        RecordSource::Backfill => counts
            .into_iter()
            .filter(|(skill_id, _)| is_sanitized_skill_id(skill_id))
            .collect::<BTreeMap<_, _>>(),
    };
    if selected.is_empty() {
        return Ok(receipt(agent_id, 0, source));
    }

    let lock = usage_lock(store)?;
    lock.lock_exclusive()?;
    let recorded = record_counts_locked(store, agent_id, &selected, occurred_at)?;
    Ok(receipt(agent_id, recorded, source))
}

/// Shared exclusive lock for the usage collection. The backfill scanner holds
/// it across a ledger update and its watermark update so both land together.
pub(super) fn usage_lock(store: &ClientStateStore) -> Result<std::fs::File> {
    Ok(open_private_lock_file(&store.root().join(LOCK_FILE))?)
}

/// Record already-gated counts while the caller holds the usage lock.
pub(super) fn record_counts_locked(
    store: &ClientStateStore,
    agent_id: &str,
    counts: &BTreeMap<String, u64>,
    occurred_at: OffsetDateTime,
) -> Result<u64> {
    let day = format_date(occurred_at)?;
    let mut document = store.read_collection(COLLECTION)?;
    let items = collection_items_mut(&mut document)?;
    let recorded = upsert_day_buckets(items, agent_id, &day, counts, occurred_at)?;
    store.write_collection(COLLECTION, document)?;
    Ok(recorded)
}

/// Merge counts into the per-day buckets of a decoded collection document.
/// Items that are not day buckets (for example backfill scan watermarks) are
/// preserved untouched.
pub(super) fn upsert_day_buckets(
    items: &mut Vec<Value>,
    agent_id: &str,
    day: &str,
    counts: &BTreeMap<String, u64>,
    occurred_at: OffsetDateTime,
) -> Result<u64> {
    let mut positions = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((item.get("bucketKey")?.as_str()?.to_string(), index)))
        .collect::<BTreeMap<_, _>>();
    let mut recorded = 0_u64;
    for (skill_id, count) in counts {
        let bucket_key = bucket_key(agent_id, skill_id, day);
        let previous = positions
            .get(&bucket_key)
            .and_then(|index| items.get(*index))
            .and_then(|item| item.get("count"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let next = previous
            .checked_add(*count)
            .filter(|next| *next <= MAX_DAY_COUNT)
            .ok_or_else(|| anyhow!("skill usage counter overflow"))?;
        recorded = recorded
            .checked_add(*count)
            .ok_or_else(|| anyhow!("skill usage counter overflow"))?;
        let record = json!({
            "kind": "skill-usage-day",
            "bucketKey": bucket_key,
            "agentId": agent_id,
            "skillId": skill_id,
            "date": day,
            "count": next,
            "lastUsedAt": occurred_at.format(&Rfc3339)?
        });
        match positions.get(&bucket_key) {
            Some(index) => items[*index] = record,
            None => {
                positions.insert(bucket_key.clone(), items.len());
                items.push(record);
            }
        }
    }
    Ok(recorded)
}

pub(super) fn is_sanitized_skill_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SKILL_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn receipt(agent_id: &str, recorded: u64, source: RecordSource) -> Value {
    json!({
        "ok": true,
        "status": if recorded == 0 { "no_local_invocations" } else { "recorded" },
        "agentId": agent_id,
        "recordedCount": recorded,
        "source": source.label(),
        "privacy": "aggregate-only"
    })
}

fn bucket_key(agent_id: &str, skill_id: &str, day: &str) -> String {
    format!("{day}\u{1f}{agent_id}\u{1f}{skill_id}")
}

fn format_date(value: OffsetDateTime) -> Result<String> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
    Ok(value.date().format(&format)?)
}
