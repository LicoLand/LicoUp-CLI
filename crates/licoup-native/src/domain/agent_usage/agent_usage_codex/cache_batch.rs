use super::file_collection::FileMetadata;
use super::models::{CachedFile, ParserState};
use super::utils::{from_i64, to_i64, totals_columns, totals_from_columns};
use anyhow::Result;
use licoup_statistics::contract::UsageVariant;
use rusqlite::{OptionalExtension, Statement, Transaction, params};

pub(super) struct CacheBatch<'connection> {
    load_file: Statement<'connection>,
    save_file: Statement<'connection>,
    delete_rows: Statement<'connection>,
    delete_file: Statement<'connection>,
}

impl<'connection> CacheBatch<'connection> {
    pub(super) fn new(transaction: &'connection Transaction<'_>) -> Result<Self> {
        Ok(Self {
            load_file: transaction.prepare(
                "SELECT modified_ns, size, file_id, parsed_bytes, append_guard, session_id,
                        forked_from_id, last_model, current_turn_id, raw_input, raw_cached,
                        raw_output, counted_input, counted_cached, counted_output, divergent,
                        next_event_index, token_chain_hash,current_effort,current_fast,pending_context
                 FROM usage_files WHERE root_key=?1 AND source_key=?2",
            )?,
            save_file: transaction.prepare(
                "INSERT INTO usage_files(
                   root_key, source_key, modified_ns, size, file_id, parsed_bytes, append_guard,
                   session_id, forked_from_id, lineage_scope, last_model, current_turn_id,
                   raw_input, raw_cached, raw_output, counted_input, counted_cached,
                   counted_output, divergent, next_event_index, token_chain_hash,current_effort,current_fast,pending_context
                 ) VALUES(
                   ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                   ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
                 ) ON CONFLICT(root_key, source_key) DO UPDATE SET
                   modified_ns=excluded.modified_ns,
                   size=excluded.size,
                   file_id=excluded.file_id,
                   parsed_bytes=excluded.parsed_bytes,
                   append_guard=excluded.append_guard,
                   session_id=excluded.session_id,
                   forked_from_id=excluded.forked_from_id,
                   lineage_scope=excluded.lineage_scope,
                   last_model=excluded.last_model,
                   current_turn_id=excluded.current_turn_id,
                   raw_input=excluded.raw_input,
                   raw_cached=excluded.raw_cached,
                   raw_output=excluded.raw_output,
                   counted_input=excluded.counted_input,
                   counted_cached=excluded.counted_cached,
                   counted_output=excluded.counted_output,
                   divergent=excluded.divergent,
                   next_event_index=excluded.next_event_index,
                   token_chain_hash=excluded.token_chain_hash,
                   current_effort=excluded.current_effort,current_fast=excluded.current_fast,
                   pending_context=excluded.pending_context",
            )?,
            delete_rows: transaction
                .prepare("DELETE FROM usage_rows WHERE root_key=?1 AND source_key=?2")?,
            delete_file: transaction
                .prepare("DELETE FROM usage_files WHERE root_key=?1 AND source_key=?2")?,
        })
    }

    pub(super) fn load(&mut self, root_key: &str, source_key: &str) -> Result<Option<CachedFile>> {
        self.load_file
            .query_row(params![root_key, source_key], |row| {
                let raw_values = (
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                );
                let counted_values = (
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                );
                Ok(CachedFile {
                    modified_ns: from_i64(row.get(0)?),
                    size: from_i64(row.get(1)?),
                    file_id: row.get(2)?,
                    parsed_bytes: from_i64(row.get(3)?),
                    append_guard: row.get(4)?,
                    state: ParserState {
                        session_id: row.get(5)?,
                        forked_from_id: row.get(6)?,
                        current_model: row.get(7)?,
                        current_turn_id: row.get(8)?,
                        current_variant: UsageVariant {
                            effort: row.get(18)?,
                            fast: row.get(19)?,
                        },
                        pending_context: row.get(20)?,
                        raw_totals: totals_from_columns(raw_values),
                        counted_totals: totals_from_columns(counted_values),
                        has_divergent_totals: row.get::<_, i64>(15)? != 0,
                        next_event_index: from_i64(row.get(16)?),
                        token_chain_hash: row.get(17)?,
                    },
                })
            })
            .optional()
            .map_err(Into::into)
    }

    pub(super) fn save(
        &mut self,
        root_key: &str,
        source_key: &str,
        metadata: &FileMetadata,
        parsed_bytes: u64,
        append_guard: &str,
        state: &ParserState,
    ) -> Result<()> {
        let (raw_input, raw_cached, raw_output) = totals_columns(state.raw_totals);
        let (counted_input, counted_cached, counted_output) = totals_columns(state.counted_totals);
        let initial_lineage_scope = state
            .forked_from_id
            .as_deref()
            .or(state.session_id.as_deref())
            .map(|session_id| format!("session:{session_id}"))
            .unwrap_or_else(|| format!("source:{source_key}"));
        self.save_file.execute(params![
            root_key,
            source_key,
            to_i64(metadata.modified_ns),
            to_i64(metadata.size),
            metadata.file_id,
            to_i64(parsed_bytes),
            append_guard,
            state.session_id,
            state.forked_from_id,
            initial_lineage_scope,
            state.current_model,
            state.current_turn_id,
            raw_input,
            raw_cached,
            raw_output,
            counted_input,
            counted_cached,
            counted_output,
            i64::from(state.has_divergent_totals),
            to_i64(state.next_event_index),
            state.token_chain_hash,
            state.current_variant.effort,
            state.current_variant.fast,
            state.pending_context,
        ])?;
        Ok(())
    }

    pub(super) fn reset_parsed_source(&mut self, root_key: &str, source_key: &str) -> Result<()> {
        self.delete_rows.execute(params![root_key, source_key])?;
        Ok(())
    }

    pub(super) fn delete_source(&mut self, root_key: &str, source_key: &str) -> Result<()> {
        self.reset_parsed_source(root_key, source_key)?;
        self.delete_file.execute(params![root_key, source_key])?;
        Ok(())
    }
}
