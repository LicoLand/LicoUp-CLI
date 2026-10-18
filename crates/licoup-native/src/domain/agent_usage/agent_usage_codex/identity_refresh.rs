//! Correct model identities only when every persisted event and cursor agrees.

use super::append_guard::append_guard_matches;
use super::cache::open_cache_database;
use super::file_collection::file_metadata;
use super::models::{CachedFile, ParserState};
use super::parser::ParserBatch;
use anyhow::Result;
use licoup_statistics::variant::{is_placeholder_id, model_selection};
use licoup_statistics::window::UsageWindow;
use rusqlite::{Connection, Transaction, params};
use std::path::Path;

pub(super) fn needed(connection: &Connection, root: &str, source: &str, day: &str) -> Result<bool> {
    let mut query = connection.prepare(
        "SELECT DISTINCT model FROM usage_rows WHERE root_key=?1 AND source_key=?2 AND day=?3",
    )?;
    for row in query.query_map(params![root, source, day], |row| {
        row.get::<_, Option<String>>(0)
    })? {
        if row?
            .as_deref()
            .is_none_or(|raw| is_placeholder_id(&model_selection(raw).id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn apply(
    transaction: &Transaction<'_>,
    root: &str,
    source: &str,
    path: &Path,
    cached: &CachedFile,
    window: &UsageWindow,
) -> Result<bool> {
    if cached.file_id.is_none() || cached.parsed_bytes > cached.size {
        return Ok(false);
    }
    // Parsing into a separate in-memory database cannot replace the real
    // ledger or accidentally recreate already sealed historical rows.
    let mut scratch = open_cache_database(Path::new(":memory:"))?;
    let staged = scratch.transaction()?;
    let mut state = ParserState::default();
    let Ok(parsed_bytes) =
        ParserBatch::new(&staged)?.parse_file(root, source, path, 0, window, &mut state)
    else {
        return Ok(false);
    };
    let Some(after) = file_metadata(path) else {
        return Ok(false);
    };
    let old = &cached.state;
    if parsed_bytes != cached.parsed_bytes
        || after.size != cached.size
        || after.modified_ns != cached.modified_ns
        || after.file_id != cached.file_id
        || !append_guard_matches(path, cached)
        || state.session_id != old.session_id
        || state.forked_from_id != old.forked_from_id
        || state.current_turn_id != old.current_turn_id
        || state.current_variant != old.current_variant
        || state.pending_context != old.pending_context
        || state.raw_totals != old.raw_totals
        || state.counted_totals != old.counted_totals
        || state.has_divergent_totals != old.has_divergent_totals
        || state.next_event_index != old.next_event_index
        || state.token_chain_hash != old.token_chain_hash
    {
        return Ok(false);
    }
    // Old days may already be sealed while replay still traverses their raw
    // events. Compare only today's retained details; the full cursor above
    // still proves that the complete source was replayed without drift.
    let previous = rows(transaction, root, source, &window.end)?;
    let projected = rows(&staged, root, source, &window.end)?;
    if previous.is_empty()
        || previous.len() != projected.len()
        || previous
            .iter()
            .zip(&projected)
            .any(|(old, new)| old.identity != new.identity)
    {
        return Ok(false);
    }
    if previous
        .iter()
        .zip(&projected)
        .all(|(old, new)| old.model == new.model)
    {
        return Ok(false);
    }
    let mut update = transaction.prepare(
        "UPDATE usage_rows SET model=?4 WHERE root_key=?1 AND source_key=?2 AND event_index=?3 AND day=?5",
    )?;
    for row in projected {
        update.execute(params![
            root,
            source,
            row.identity.event_index,
            row.model,
            &window.end
        ])?;
    }
    transaction.execute(
        "UPDATE usage_files SET last_model=?3 WHERE root_key=?1 AND source_key=?2",
        params![root, source, state.current_model],
    )?;
    Ok(true)
}

#[derive(Eq, PartialEq)]
struct EventIdentity {
    event_index: i64,
    session: Option<String>,
    turn: Option<String>,
    day: String,
    input: i64,
    cached: i64,
    output: i64,
    event_hash: String,
    effort: Option<String>,
    fast: Option<bool>,
}

struct ModelRow {
    identity: EventIdentity,
    model: Option<String>,
}

fn rows(connection: &Connection, root: &str, source: &str, day: &str) -> Result<Vec<ModelRow>> {
    let mut query = connection.prepare(
        "SELECT event_index,session_id,turn_id,day,input_tokens,cached_input_tokens,output_tokens,
                event_identity,effort,fast,model FROM usage_rows WHERE root_key=?1 AND source_key=?2 AND day=?3
         ORDER BY event_index",
    )?;
    Ok(query
        .query_map(params![root, source, day], |row| {
            Ok(ModelRow {
                identity: EventIdentity {
                    event_index: row.get(0)?,
                    session: row.get(1)?,
                    turn: row.get(2)?,
                    day: row.get(3)?,
                    input: row.get(4)?,
                    cached: row.get(5)?,
                    output: row.get(6)?,
                    event_hash: row.get(7)?,
                    effort: row.get(8)?,
                    fast: row.get(9)?,
                },
                model: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}
