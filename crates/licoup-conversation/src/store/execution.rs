use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeExecutionProvenance {
    agent_id: String,
    native_session_id: String,
    turn_ids: BTreeSet<String>,
    message_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeExecutionReference {
    pub conversation_id: String,
    pub membership_id: String,
    pub turn_handle: String,
}

pub type NativeExecutionReferenceIndex =
    HashMap<(String, String), Option<NativeExecutionReference>>;

/// Endpoint-local execution evidence. Its cursor is independent of both the
/// runtime frame's cursor and the request-local stdio sequence: request is 1,
/// runtime frames use their final stored part ordinal + 2, and the terminal follows the last
/// existing record. Missing historical payloads never become synthetic records.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRecord {
    pub id: String,
    pub kind: String,
    pub raw_text: String,
    pub timestamp: i64,
    pub cursor: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeFrameRecord {
    pub cursor: u64,
    pub raw_text: String,
    pub timestamp: i64,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeExecutionSnapshot {
    pub status: String,
    pub runtime_high_water: u64,
    pub request_payload_available: bool,
    pub terminal_payload_available: bool,
}

impl RuntimeExecutionSnapshot {
    pub fn records_high_water(&self) -> u64 {
        let before_terminal = if self.runtime_high_water > 0 {
            self.runtime_high_water
        } else {
            u64::from(self.request_payload_available)
        };
        before_terminal + u64::from(self.terminal_payload_available)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}

impl ConversationStore {
    /// Register only identifiers explicitly supplied by an adapter's provider.
    /// A dispatch's native session binding is never silently reassigned.
    pub fn record_native_execution_provenance(
        &self,
        scope: &ConversationRuntimeScope,
        agent_id: &str,
        native_session_id: &str,
        native_turn_id: Option<&str>,
        source_message_id: Option<&str>,
    ) -> StoreResult<bool> {
        if agent_id.is_empty() || native_session_id.is_empty() {
            return Ok(false);
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let encoded: Option<String> = transaction.query_row("SELECT native_provenance FROM conversation_dispatches
                WHERE id=?1 AND conversation_id=?2 AND membership_id=?3 AND EXISTS(SELECT 1 FROM events
                  WHERE id=?4 AND conversation_id=?2 AND correlation_id=?1 AND author_membership_id=?3)",
                params![scope.dispatch_id,scope.conversation_id,scope.membership_id,scope.event_id],|row|row.get(0))?;
            let mut provenance = encoded.as_deref().map(serde_json::from_str::<NativeExecutionProvenance>).transpose()?.unwrap_or_else(||NativeExecutionProvenance {
                agent_id:agent_id.to_owned(),native_session_id:native_session_id.to_owned(),..Default::default()
            });
            if provenance.agent_id != agent_id || provenance.native_session_id != native_session_id { return Ok(false); }
            native_sessions::record_native_session(&transaction, &scope.conversation_id, &scope.membership_id, native_session_id)?;
            let mut changed = false;
            if let Some(id) = native_turn_id.filter(|id|!id.is_empty()) { changed |= provenance.turn_ids.insert(id.to_owned()); }
            if let Some(id) = source_message_id.filter(|id|!id.is_empty()) { changed |= provenance.message_ids.insert(id.to_owned()); }
            if changed {
                transaction.execute("UPDATE conversation_dispatches SET native_provenance=?2 WHERE id=?1",params![scope.dispatch_id,serde_json::to_string(&provenance)?])?;
            }
            transaction.commit()?;
            Ok(changed)
        })
    }

    /// Read just the selected native session's provenance through a read-only
    /// connection. History discovery cannot initialize or recover Canonical state.
    pub fn native_execution_references(
        portable_root: &Path,
        agent_id: &str,
        native_session_id: &str,
    ) -> StoreResult<NativeExecutionReferenceIndex> {
        let path = portable_root
            .join("client-state")
            .join("conversations")
            .join(DATABASE_FILE);
        if !path.is_file() {
            return Ok(HashMap::new());
        }
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: String = connection.query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )?;
        if version != CURRENT_SCHEMA_VERSION {
            return Ok(HashMap::new());
        }
        let mut statement = connection.prepare(
            "SELECT id,conversation_id,membership_id,native_provenance FROM conversation_dispatches
            WHERE native_provenance IS NOT NULL AND json_extract(native_provenance,'$.agentId')=?1
              AND json_extract(native_provenance,'$.nativeSessionId')=?2",
        )?;
        let rows = statement.query_map(params![agent_id, native_session_id], |row| {
            Ok((
                NativeExecutionReference {
                    turn_handle: row.get(0)?,
                    conversation_id: row.get(1)?,
                    membership_id: row.get(2)?,
                },
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut references = HashMap::new();
        for row in rows {
            let (reference, encoded) = row?;
            let provenance: NativeExecutionProvenance = serde_json::from_str(&encoded)?;
            for (kind, keys) in [
                ("turn", provenance.turn_ids),
                ("message", provenance.message_ids),
            ] {
                for key in keys {
                    references
                        .entry((kind.to_owned(), key))
                        .and_modify(|known: &mut Option<NativeExecutionReference>| {
                            if known.as_ref() != Some(&reference) {
                                *known = None;
                            }
                        })
                        .or_insert_with(|| Some(reference.clone()));
                }
            }
        }
        Ok(references)
    }

    /// Frames that must not become Agent-authored Canonical content use the
    /// existing private runtime parts with negative replay keys. Public attach
    /// reads positive keys only; Event projection excludes every runtime key.
    pub fn append_runtime_local_frame(
        &self,
        scope: &ConversationRuntimeScope,
        frame: &Value,
    ) -> StoreResult<()> {
        let encoded = serde_json::to_string(frame)?;
        self.append_runtime_local_raw_frame(scope, "runtime", &encoded)
    }

    /// Local-only text at its actual transport boundary. Source and direction
    /// live in the visible execution kind; the original text is never wrapped.
    pub fn append_runtime_local_raw_frame(
        &self,
        scope: &ConversationRuntimeScope,
        kind: &str,
        raw_text: &str,
    ) -> StoreResult<()> {
        let parts = runtime_frame_parts(raw_text);
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let active: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_dispatches d JOIN events e
                 ON e.correlation_id=d.id AND e.conversation_id=d.conversation_id AND e.author_membership_id=d.membership_id
                 WHERE d.id=?1 AND d.conversation_id=?2 AND d.membership_id=?3 AND e.id=?4 AND e.finalized=0
                   AND d.state IN ('accepted','running','cancel-requested'))",
                params![scope.dispatch_id,scope.conversation_id,scope.membership_id,scope.event_id],|row|row.get(0))?;
            if !active { return Err(anyhow!("runtime_dispatch_not_active")); }
            // The first part's ordinal is already unique and indexed. Derive
            // a negative group key without scanning all earlier runtime parts.
            let mut ordinal:i64 = transaction.query_row(
                "SELECT COALESCE((SELECT ordinal FROM event_parts WHERE event_id=?1 ORDER BY ordinal DESC LIMIT 1),-1)+1",
                params![scope.event_id],|row|row.get(0))?;
            let cursor = -ordinal-1;
            let now = now_ms();
            for part in parts {
                transaction.execute("INSERT INTO event_parts(id,event_id,ordinal,kind,content,runtime_cursor,execution_kind,created_at) VALUES(?1,?2,?3,'metadata',?4,?5,?6,?7)",params![new_id("part"),scope.event_id,ordinal,part.content,cursor,kind,now])?;
                ordinal += 1;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    fn runtime_execution_frames_after(
        &self,
        scope: &ConversationRuntimeScope,
        after_cursor: u64,
        through_cursor: u64,
        limit: usize,
    ) -> StoreResult<Vec<RuntimeFrameRecord>> {
        let after_ordinal = i64::try_from(i128::from(after_cursor) - 2)
            .map_err(|_| anyhow!("runtime_cursor_invalid"))?;
        let through_ordinal = i64::try_from(i128::from(through_cursor) - 2)
            .map_err(|_| anyhow!("runtime_cursor_invalid"))?;
        self.with_connection(|connection| {
            // Seek the next record through the event/ordinal index. Rewind to
            // its first part even when a caller supplies a cursor inside it.
            // Reading consecutive parts avoids repeatedly grouping the entire
            // remaining execution for every page.
            let mut statement = connection.prepare(
                "SELECT p.runtime_cursor,p.ordinal,p.content,p.created_at,COALESCE(p.execution_kind,'runtime')
                 FROM event_parts p JOIN events e ON e.id=p.event_id
                 JOIN conversation_dispatches d ON d.id=e.correlation_id
                   AND d.conversation_id=e.conversation_id AND d.membership_id=e.author_membership_id
                 WHERE e.id=?1 AND d.id=?2 AND d.conversation_id=?3 AND d.membership_id=?4
                   AND p.runtime_cursor IS NOT NULL AND p.ordinal<=?6
                   AND p.ordinal >= (
                     SELECT MIN(first.ordinal) FROM event_parts first WHERE first.event_id=?1
                       AND first.runtime_cursor=(SELECT next.runtime_cursor FROM event_parts next
                         WHERE next.event_id=?1 AND next.ordinal>?5 AND next.runtime_cursor IS NOT NULL
                         ORDER BY next.ordinal LIMIT 1)
                   ) ORDER BY p.ordinal",
            )?;
            let rows = statement.query_map(params![scope.event_id,scope.dispatch_id,scope.conversation_id,scope.membership_id,after_ordinal,through_ordinal],|row|Ok((
                row.get::<_,i64>(0)?, RuntimeFrameRecord {
                    cursor:row.get::<_,i64>(1)? as u64 + 2,raw_text:row.get(2)?,timestamp:row.get(3)?,
                    kind:row.get(4)?,
                }
            )))?;
            let mut frames: Vec<RuntimeFrameRecord> = Vec::new();
            let mut current_runtime_cursor = None;
            for row in rows {
                let (runtime_cursor,part) = row?;
                if current_runtime_cursor == Some(runtime_cursor) {
                    let frame = frames.last_mut().expect("current record has a first part");
                    frame.raw_text.push_str(&part.raw_text);
                    frame.cursor = part.cursor;
                } else {
                    if frames.len() == limit.clamp(1,512) { break; }
                    current_runtime_cursor = Some(runtime_cursor);
                    frames.push(part);
                }
            }
            Ok(frames)
        })
    }

    pub fn runtime_execution_scope(
        &self,
        dispatch_id: &str,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<ConversationRuntimeScope> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT e.id FROM conversation_dispatches d
                 JOIN events e ON e.conversation_id=d.conversation_id
                   AND e.correlation_id=d.id AND e.author_membership_id=d.membership_id
                 WHERE d.id=?1 AND d.conversation_id=?2 AND d.membership_id=?3
                   AND e.kind='message' LIMIT 2",
            )?;
            let events = statement
                .query_map(
                    params![dispatch_id, conversation_id, membership_id],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if events.len() != 1 {
                return Err(anyhow!("turn_scope_mismatch"));
            }
            Ok(ConversationRuntimeScope {
                dispatch_id: dispatch_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
                membership_id: membership_id.to_owned(),
                event_id: events.into_iter().next().expect("one scoped event"),
            })
        })
    }

    /// Preserve the complete structured request actually admitted by this host.
    /// This private column is excluded from Canonical Event projections/export.
    pub fn record_runtime_request(
        &self,
        scope: &ConversationRuntimeScope,
        request: &Value,
    ) -> StoreResult<()> {
        let encoded = serde_json::to_string(request)?;
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE conversation_dispatches SET request_payload=?5
                 WHERE id=?1 AND conversation_id=?2 AND membership_id=?3
                   AND request_payload IS NULL AND state IN ('accepted','running')
                   AND EXISTS(SELECT 1 FROM events WHERE id=?4 AND conversation_id=?2
                     AND correlation_id=?1 AND author_membership_id=?3)",
                params![
                    scope.dispatch_id,
                    scope.conversation_id,
                    scope.membership_id,
                    scope.event_id,
                    encoded
                ],
            )?;
            if changed != 1 {
                return Err(anyhow!("runtime_request_not_active"));
            }
            Ok(())
        })
    }

    pub fn runtime_execution_snapshot(
        &self,
        scope: &ConversationRuntimeScope,
    ) -> StoreResult<RuntimeExecutionSnapshot> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT d.state, d.request_payload IS NOT NULL, d.terminal_payload IS NOT NULL,
                   (SELECT COALESCE(MAX(ordinal)+2,0) FROM event_parts WHERE event_id=e.id AND runtime_cursor IS NOT NULL)
                 FROM conversation_dispatches d JOIN events e
                   ON e.conversation_id=d.conversation_id AND e.correlation_id=d.id
                     AND e.author_membership_id=d.membership_id
                 WHERE d.id=?1 AND d.conversation_id=?2 AND d.membership_id=?3 AND e.id=?4",
                params![scope.dispatch_id, scope.conversation_id, scope.membership_id, scope.event_id],
                |row| Ok(RuntimeExecutionSnapshot {
                    status: row.get(0)?, request_payload_available: row.get(1)?,
                    terminal_payload_available: row.get(2)?, runtime_high_water: row.get::<_, i64>(3)? as u64,
                }),
            ).map_err(Into::into)
        })
    }

    /// Reassemble original stored UTF-8 parts without parsing or reserializing
    /// JSON. Unknown fields, arrays, whitespace and large payloads survive.
    pub fn runtime_raw_frames_after(
        &self,
        scope: &ConversationRuntimeScope,
        after_cursor: u64,
        through_cursor: u64,
        limit: usize,
    ) -> StoreResult<Vec<RuntimeFrameRecord>> {
        if after_cursor > through_cursor || through_cursor > i64::MAX as u64 {
            return Err(anyhow!("runtime_cursor_invalid"));
        }
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT selected.runtime_cursor, p.content, p.created_at FROM (
                   SELECT parts.runtime_cursor FROM event_parts parts
                   JOIN events e ON e.id=parts.event_id
                   JOIN conversation_dispatches d ON d.id=e.correlation_id
                     AND d.conversation_id=e.conversation_id AND d.membership_id=e.author_membership_id
                   WHERE e.id=?1 AND d.id=?2 AND d.conversation_id=?3 AND d.membership_id=?4
                     AND parts.runtime_cursor>?5 AND parts.runtime_cursor<=?6
                   GROUP BY parts.runtime_cursor ORDER BY parts.runtime_cursor LIMIT ?7
                 ) selected JOIN event_parts p ON p.event_id=?1 AND p.runtime_cursor=selected.runtime_cursor
                 ORDER BY selected.runtime_cursor,p.ordinal",
            )?;
            let rows = statement.query_map(params![scope.event_id, scope.dispatch_id, scope.conversation_id, scope.membership_id, after_cursor as i64, through_cursor as i64, limit.clamp(1,512) as i64], |row| Ok(RuntimeFrameRecord {
                kind:"runtime".to_owned(),
                cursor: row.get::<_,i64>(0)? as u64, raw_text: row.get(1)?, timestamp: row.get(2)?,
            }))?;
            let mut frames: Vec<RuntimeFrameRecord> = Vec::new();
            for row in rows {
                let part = row?;
                if let Some(frame) = frames.last_mut().filter(|frame| frame.cursor == part.cursor) {
                    frame.raw_text.push_str(&part.raw_text);
                } else { frames.push(part); }
            }
            Ok(frames)
        })
    }

    pub fn runtime_execution_records_after(
        &self,
        scope: &ConversationRuntimeScope,
        snapshot: &RuntimeExecutionSnapshot,
        after_cursor: u64,
        limit: usize,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let limit = limit.clamp(1, 512);
        let mut records = Vec::new();
        let record = |kind: &str, cursor: u64, raw_text, timestamp| ExecutionRecord {
            id: format!("{}:{cursor}", scope.dispatch_id),
            kind: kind.to_owned(),
            raw_text,
            timestamp,
            cursor,
        };
        let payload = |column: &str, timestamp: &str| {
            self.with_connection(|connection| {
            connection.query_row(&format!("SELECT {column},{timestamp} FROM conversation_dispatches WHERE id=?1 AND conversation_id=?2 AND membership_id=?3
                AND EXISTS(SELECT 1 FROM events WHERE id=?4 AND conversation_id=?2 AND correlation_id=?1 AND author_membership_id=?3)"),
                params![scope.dispatch_id,scope.conversation_id,scope.membership_id,scope.event_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).map_err(Into::into)
        })
        };
        if after_cursor == 0 && snapshot.request_payload_available {
            let (raw, timestamp) = payload("request_payload", "created_at")?;
            records.push(record("request", 1, raw, timestamp));
        }
        if records.len() < limit && after_cursor < snapshot.runtime_high_water {
            for frame in self.runtime_execution_frames_after(
                scope,
                after_cursor,
                snapshot.runtime_high_water,
                limit - records.len(),
            )? {
                records.push(record(
                    &frame.kind,
                    frame.cursor,
                    frame.raw_text,
                    frame.timestamp,
                ));
            }
        }
        let terminal_cursor = snapshot.records_high_water();
        let last_cursor = records.last().map_or(after_cursor, |record| record.cursor);
        if records.len() < limit
            && snapshot.terminal_payload_available
            && last_cursor < terminal_cursor
        {
            let (raw, timestamp) = payload("terminal_payload", "updated_at")?;
            records.push(record("terminal", terminal_cursor, raw, timestamp));
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope(store: &ConversationStore) -> ConversationRuntimeScope {
        store
            .prepare_runtime_dispatch(
                "synthetic",
                "session-execution",
                "synthetic prompt",
                None,
                None,
                None,
                None,
            )
            .unwrap()
    }

    #[test]
    fn execution_raw_transport_reopens_losslessly_without_becoming_trusted_canonical_metadata() {
        let root = std::env::temp_dir().join(format!("lico-raw-transport-{}", Uuid::new_v4()));
        let store = ConversationStore::open(&root).unwrap();
        let scope = scope(&store);
        let raw = format!(
            " [ {{ \"unknown\" : \"{}\", \"array\" : [null,true] }} ] \r\n",
            "字".repeat(220_000)
        );
        store
            .append_runtime_local_raw_frame(&scope, "protocol.synthetic.received", &raw)
            .unwrap();
        store
            .append_runtime_frame(&scope, 1, &json!({"event":"agent.synthetic"}))
            .unwrap();
        store
            .append_runtime_local_raw_frame(
                &scope,
                "protocol.synthetic.received",
                r#"{"trustedResponseMode":"assistant-turn-response"}"#,
            )
            .unwrap();
        let state = store
            .finish_runtime_dispatch(
                &scope,
                &json!({"ok":true,"output":"ordinary reply"}),
                DispatchState::Completed,
                None,
            )
            .unwrap();
        assert_eq!(state, DispatchState::Completed);
        store.checkpoint().unwrap();
        drop(store);
        let store = ConversationStore::open(&root).unwrap();
        let snapshot = store.runtime_execution_snapshot(&scope).unwrap();
        let records = store
            .runtime_execution_records_after(&scope, &snapshot, 0, 20)
            .unwrap();
        assert_eq!(records[0].raw_text, raw);
        assert_eq!(records[0].kind, "protocol.synthetic.received");
        assert_eq!(records[1].kind, "runtime");
        assert_eq!(records[2].kind, "protocol.synthetic.received");
        assert_eq!(records[3].kind, "terminal");
        assert_eq!(
            store.runtime_frames_after(&scope, 0, 1, 20).unwrap().len(),
            1
        );
        assert!(
            !serde_json::to_string(&store.page_events(&scope.conversation_id, None, 20).unwrap())
                .unwrap()
                .contains("trustedResponseMode")
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_raw_parts_preserve_large_json_whitespace_arrays_and_unknown_fields() {
        let store = ConversationStore::open_in_memory().unwrap();
        let scope = scope(&store);
        let original = format!(
            " [ {{ \"unknown\" : \"{}\", \"array\" : [1, null, true] }} ] ",
            "字\n".repeat(190_000).replace('\n', "\\n")
        );
        let parts = runtime_frame_parts(&original);
        assert!(parts.len() > 1);
        store
            .with_connection(|connection| {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                for (index, part) in parts.iter().enumerate() {
                    insert_runtime_event_part(
                        &transaction,
                        &scope.conversation_id,
                        &scope.event_id,
                        index as i64,
                        part,
                        Some(1),
                        1234,
                    )?;
                }
                transaction.commit()?;
                Ok(())
            })
            .unwrap();
        let frames = store.runtime_raw_frames_after(&scope, 0, 1, 1).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].raw_text, original);
        assert_eq!(frames[0].timestamp, 1234);
        let parsed: Value = serde_json::from_str(&original).unwrap();
        assert_eq!(
            store.runtime_frames_after(&scope, 0, 1, 1).unwrap(),
            vec![parsed]
        );
    }

    #[test]
    fn execution_records_page_request_frames_terminal_and_reconnect_without_duplicates() {
        let store = ConversationStore::open_in_memory().unwrap();
        let scope = scope(&store);
        let request = json!({"text":"synthetic","unknown":{"array":[1,2]}});
        let terminal = json!({"ok":true,"output":"done","unknown":{"native":[1,2,3]}});
        store.record_runtime_request(&scope, &request).unwrap();
        store
            .append_runtime_frame(&scope, 1, &json!(["unknown",{"value":42}]))
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                2,
                &json!({"event":"agent.synthetic","arbitrary":"preserved"}),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(&scope, &terminal, DispatchState::Completed, None)
            .unwrap();
        let snapshot = store.runtime_execution_snapshot(&scope).unwrap();
        assert!(snapshot.is_terminal());
        assert_eq!(snapshot.records_high_water(), 4);
        let mut records = Vec::new();
        let mut cursor = 0;
        loop {
            let page = store
                .runtime_execution_records_after(&scope, &snapshot, cursor, 1)
                .unwrap();
            let Some(record) = page.into_iter().next() else {
                break;
            };
            assert!(record.cursor > cursor);
            cursor = record.cursor;
            records.push(record);
        }
        assert_eq!(
            records.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(),
            vec!["request", "runtime", "runtime", "terminal"]
        );
        assert_eq!(records[0].raw_text, request.to_string());
        assert_eq!(records[3].raw_text, terminal.to_string());
        assert_eq!(records[3].id, format!("{}:4", scope.dispatch_id));
        assert!(
            store
                .runtime_execution_records_after(&scope, &snapshot, 4, 512)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn execution_scope_rejects_other_conversation_membership_and_event() {
        let store = ConversationStore::open_in_memory().unwrap();
        let scope = scope(&store);
        store
            .record_runtime_request(&scope, &json!({"text":"synthetic request"}))
            .unwrap();
        store
            .append_runtime_frame(&scope, 1, &json!({"private":"synthetic"}))
            .unwrap();
        assert_eq!(
            store
                .runtime_execution_scope(
                    &scope.dispatch_id,
                    &scope.conversation_id,
                    &scope.membership_id
                )
                .unwrap(),
            scope
        );
        assert!(
            store
                .runtime_execution_scope(
                    &scope.dispatch_id,
                    "another-conversation",
                    &scope.membership_id
                )
                .is_err()
        );
        assert!(
            store
                .runtime_execution_scope(
                    &scope.dispatch_id,
                    &scope.conversation_id,
                    "another-member"
                )
                .is_err()
        );
        for invalid in [
            ConversationRuntimeScope {
                membership_id: "another-member".into(),
                ..scope.clone()
            },
            ConversationRuntimeScope {
                conversation_id: "another-conversation".into(),
                ..scope.clone()
            },
            ConversationRuntimeScope {
                event_id: "another-event".into(),
                ..scope.clone()
            },
        ] {
            assert!(store.runtime_execution_snapshot(&invalid).is_err());
            assert!(
                store
                    .runtime_execution_records_after(
                        &invalid,
                        &store.runtime_execution_snapshot(&scope).unwrap(),
                        0,
                        1
                    )
                    .is_err()
            );
            assert!(
                store
                    .runtime_raw_frames_after(&invalid, 0, 1, 1)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn execution_user_speech_is_complete_and_ordered_without_entering_public_attach_or_events() {
        let root =
            std::env::temp_dir().join(format!("lico-execution-interleaved-{}", Uuid::new_v4()));
        let store = ConversationStore::open(&root).unwrap();
        let scope = scope(&store);
        store
            .record_runtime_request(&scope, &json!({"text":"initial"}))
            .unwrap();
        store
            .append_runtime_frame(&scope, 1, &json!({"event":"agent.synthetic.before"}))
            .unwrap();
        let speech = json!({"event":crate::projection::USER_MESSAGE_EVENT_KIND,"payload":{"text":"字".repeat(400_000),"unknownMetadata":[{"secret":"synthetic only"}]}});
        store.append_runtime_local_frame(&scope, &speech).unwrap();
        store
            .append_runtime_frame(&scope, 2, &json!({"event":"agent.synthetic.after"}))
            .unwrap();
        store.append_runtime_local_frame(&scope,&json!({"event":crate::projection::USER_MESSAGE_EVENT_KIND,"payload":{"text":"second user frame"}})).unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &json!({"ok":true,"output":"complete"}),
                DispatchState::Completed,
                None,
            )
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let store = ConversationStore::open(&root).unwrap();
        let snapshot = store.runtime_execution_snapshot(&scope).unwrap();
        let records = store
            .runtime_execution_records_after(&scope, &snapshot, 0, 20)
            .unwrap();
        assert_eq!(records.len(), 6);
        assert_eq!(records[2].raw_text, speech.to_string());
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].cursor < pair[1].cursor)
        );
        let resumed = store
            .runtime_execution_records_after(&scope, &snapshot, records[2].cursor, 1)
            .unwrap();
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0], records[3]);
        let public = store.runtime_frames_after(&scope, 0, 2, 20).unwrap();
        assert_eq!(public.len(), 2);
        assert!(
            !serde_json::to_string(&public)
                .unwrap()
                .contains("unknownMetadata")
        );
        let canonical = store.page_events(&scope.conversation_id, None, 20).unwrap();
        assert!(
            !serde_json::to_string(&canonical)
                .unwrap()
                .contains("unknownMetadata")
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_terminal_survives_reopen_and_older_payloads_remain_absent() {
        let root = std::env::temp_dir().join(format!("lico-execution-{}", Uuid::new_v4()));
        let store = ConversationStore::open(&root).unwrap();
        let scope = scope(&store);
        let terminal =
            json!({"ok":false,"code":"synthetic_failure","opaque":[{"evidence":"complete"}]});
        store
            .finish_runtime_dispatch(
                &scope,
                &terminal,
                DispatchState::Failed,
                Some("synthetic_failure"),
            )
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let reopened = ConversationStore::open(&root).unwrap();
        let snapshot = reopened.runtime_execution_snapshot(&scope).unwrap();
        assert_eq!(snapshot.status, "failed");
        assert!(!snapshot.request_payload_available);
        let records = reopened
            .runtime_execution_records_after(&scope, &snapshot, 0, 20)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].raw_text, terminal.to_string());
        reopened
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE conversation_dispatches SET terminal_payload=NULL WHERE id=?1",
                    params![scope.dispatch_id],
                )?;
                Ok(())
            })
            .unwrap();
        let historical = reopened.runtime_execution_snapshot(&scope).unwrap();
        assert_eq!(historical.status, "failed");
        assert!(!historical.terminal_payload_available);
        assert_eq!(historical.records_high_water(), 0);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_provenance_reopens_exact_session_keys_and_leaves_collisions_unbound() {
        let root = std::env::temp_dir().join(format!("lico-provenance-{}", Uuid::new_v4()));
        let store = ConversationStore::open(&root).unwrap();
        let first = scope(&store);
        assert!(
            store
                .record_native_execution_provenance(
                    &first,
                    "synthetic",
                    "native-session",
                    Some("provider-turn"),
                    Some("provider-message")
                )
                .unwrap()
        );
        assert!(
            !store
                .record_native_execution_provenance(
                    &first,
                    "synthetic",
                    "native-session",
                    Some("provider-turn"),
                    Some("provider-message")
                )
                .unwrap()
        );
        let mut invalid = first.clone();
        invalid.membership_id = "other-member".to_owned();
        assert!(
            store
                .record_native_execution_provenance(
                    &invalid,
                    "synthetic",
                    "native-session",
                    Some("foreign"),
                    None
                )
                .is_err()
        );
        store.checkpoint().unwrap();
        let lookup = |agent, session| {
            ConversationStore::native_execution_references(&root, agent, session).unwrap()
        };
        assert!(lookup("other-agent", "native-session").is_empty());
        assert!(lookup("synthetic", "other-session").is_empty());
        let reference = NativeExecutionReference {
            conversation_id: first.conversation_id.clone(),
            membership_id: first.membership_id.clone(),
            turn_handle: first.dispatch_id.clone(),
        };
        assert_eq!(
            lookup("synthetic", "native-session")
                .get(&("turn".to_owned(), "provider-turn".to_owned())),
            Some(&Some(reference.clone()))
        );
        let second = scope(&store);
        store
            .record_native_execution_provenance(
                &second,
                "synthetic",
                "native-session",
                Some("provider-turn"),
                Some("other-message"),
            )
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let references = lookup("synthetic", "native-session");
        assert_eq!(
            references.get(&("turn".to_owned(), "provider-turn".to_owned())),
            Some(&None)
        );
        assert_eq!(
            references.get(&("message".to_owned(), "provider-message".to_owned())),
            Some(&Some(reference))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_schema_upgrade_preserves_existing_runtime_records() {
        let root = std::env::temp_dir().join(format!("lico-execution-upgrade-{}", Uuid::new_v4()));
        let store = ConversationStore::open(&root).unwrap();
        let scope = scope(&store);
        let original = json!({"event":"agent.synthetic","payload":{"unknown":[1,2,3]}});
        store.append_runtime_frame(&scope, 1, &original).unwrap();
        store
            .finish_runtime_dispatch(&scope, &json!({"ok":true}), DispatchState::Completed, None)
            .unwrap();
        store
            .with_connection(|connection| {
                connection.execute_batch(
                    "DROP INDEX conversation_dispatches_native_provenance_idx;
                ALTER TABLE conversation_dispatches DROP COLUMN native_provenance;
                ALTER TABLE event_parts DROP COLUMN execution_kind;
                ALTER TABLE conversation_dispatches DROP COLUMN request_payload;
                ALTER TABLE conversation_dispatches DROP COLUMN terminal_payload;
                UPDATE schema_meta SET value='12' WHERE key='version';",
                )?;
                Ok(())
            })
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        assert!(ConversationStore::open(&root).is_err());
        let upgraded = ConversationStore::open_for_migration(&root).unwrap();
        upgraded
            .record_native_execution_provenance(
                &scope,
                "synthetic",
                "upgraded-session",
                Some("upgraded-turn"),
                None,
            )
            .unwrap();
        let references =
            ConversationStore::native_execution_references(&root, "synthetic", "upgraded-session")
                .unwrap();
        assert!(
            references
                .get(&("turn".to_owned(), "upgraded-turn".to_owned()))
                .unwrap()
                .is_some()
        );
        upgraded.with_connection(|connection| {
            let plan:String = connection.query_row("EXPLAIN QUERY PLAN SELECT id FROM conversation_dispatches WHERE native_provenance IS NOT NULL AND json_extract(native_provenance,'$.agentId')=?1 AND json_extract(native_provenance,'$.nativeSessionId')=?2",params!["synthetic","upgraded-session"],|row|row.get(3))?;
            assert!(plan.contains("conversation_dispatches_native_provenance_idx"));
            Ok(())
        }).unwrap();
        let snapshot = upgraded.runtime_execution_snapshot(&scope).unwrap();
        assert_eq!(snapshot.status, "completed");
        assert!(!snapshot.request_payload_available);
        assert!(!snapshot.terminal_payload_available);
        let records = upgraded
            .runtime_execution_records_after(&scope, &snapshot, 0, 20)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].raw_text, original.to_string());
        drop(upgraded);
        assert!(ConversationStore::open(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
