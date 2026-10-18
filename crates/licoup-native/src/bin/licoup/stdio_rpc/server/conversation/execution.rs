use super::*;
use licoup_conversation::store::RuntimeExecutionSnapshot;

pub(super) struct ExecutionObservation {
    scope: ConversationRuntimeScope,
    turn: Option<Weak<PersistentTurn>>,
    detached: AtomicBool,
}

struct ObservationRegistration {
    runtime: Weak<PersistentConversationRuntimeInner>,
    key: (String, String),
}

impl Drop for ObservationRegistration {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime
                .execution_observers
                .lock()
                .expect("execution observers lock")
                .remove(&self.key);
        }
    }
}

impl PersistentConversationRuntime {
    pub(in super::super) fn detach_execution(
        &self,
        params: &Value,
    ) -> std::result::Result<Value, ClientError> {
        let required = |key| {
            params
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| valid_turn_handle(value))
                .ok_or_else(|| stdio_rpc_client_error("invalid_execution_scope"))
        };
        let key = (
            required("workflowId")?.to_owned(),
            required("requestId")?.to_owned(),
        );
        let handle = required("turnHandle")?;
        let conversation_id = required("conversationId")?;
        let membership_id = required("membershipId")?;
        let observation = self
            .inner
            .execution_observers
            .lock()
            .expect("execution observers lock")
            .get(&key)
            .cloned();
        let Some(observation) = observation else {
            return Ok(json!({"detached":false}));
        };
        if observation.scope.dispatch_id != handle
            || observation.scope.conversation_id != conversation_id
            || observation.scope.membership_id != membership_id
        {
            return Err(stdio_rpc_client_error("turn_scope_mismatch"));
        }
        if let Some(turn) = observation.turn.as_ref().and_then(Weak::upgrade) {
            // Match the waiter's lock so detach cannot land between its flag
            // check and condvar wait. This never touches Agent cancellation.
            let _state = turn.state.lock().expect("turn state lock");
            observation.detached.store(true, Ordering::Release);
            turn.changed.notify_all();
        } else {
            observation.detached.store(true, Ordering::Release);
        }
        Ok(json!({"detached":true}))
    }
}

/// Explicit endpoint-local read door. It deliberately bypasses the public
/// attach projection's redaction and is never registered as an MCP operation.
pub(in super::super) fn spawn_execution<W>(
    writer: Arc<StdioRpcWriter<W>>,
    request_id: String,
    workflow_id: String,
    params: Value,
    runtime: PersistentConversationRuntime,
) -> std::result::Result<std::thread::JoinHandle<()>, ClientError>
where
    W: Write + Send + 'static,
{
    let required = |key| {
        params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| valid_turn_handle(value))
            .ok_or_else(|| stdio_rpc_client_error("invalid_execution_scope"))
    };
    let handle = required("turnHandle")?;
    let conversation_id = required("conversationId")?;
    let membership_id = required("membershipId")?;
    let cursor = match params.get("afterCursor") {
        None => 0,
        Some(value) => value
            .as_u64()
            .filter(|cursor| *cursor <= i64::MAX as u64 + 2)
            .ok_or_else(|| stdio_rpc_client_error("invalid_execution_cursor"))?,
    };
    let scope = runtime
        .inner
        .store
        .runtime_execution_scope(handle, conversation_id, membership_id)
        .map_err(|_| stdio_rpc_client_error("turn_scope_mismatch"))?;
    let turn = runtime.turn(handle);
    if turn.as_ref().is_some_and(|turn| turn.scope != scope) {
        return Err(stdio_rpc_client_error("turn_scope_mismatch"));
    }
    let store = runtime.inner.store.clone();
    let (snapshot, _, _) = capture_snapshot(&store, &scope, turn.as_ref())
        .map_err(|_| stdio_rpc_client_error("conversation_persistence_failed"))?;
    if cursor > snapshot.records_high_water() {
        return Err(stdio_rpc_client_error("cursor_ahead"));
    }
    let observation = Arc::new(ExecutionObservation {
        scope: scope.clone(),
        turn: turn.as_ref().map(Arc::downgrade),
        detached: AtomicBool::new(false),
    });
    let key = (workflow_id.clone(), request_id.clone());
    {
        let mut observers = runtime
            .inner
            .execution_observers
            .lock()
            .expect("execution observers lock");
        if observers.contains_key(&key) {
            return Err(stdio_rpc_client_error("execution_observer_duplicate"));
        }
        observers.insert(key.clone(), observation.clone());
    }
    let registration = ObservationRegistration {
        runtime: Arc::downgrade(&runtime.inner),
        key,
    };
    std::thread::Builder::new()
        .name("conversation-execution".to_owned())
        .spawn(move || {
            let _registration = registration;
            let mut sequence = 0;
            if replay_execution(
                &writer,
                &request_id,
                &workflow_id,
                &store,
                &scope,
                turn.as_ref(),
                cursor,
                &mut sequence,
                &observation,
            )
            .is_err()
            {
                let _ = write_stdio_rpc_terminal_error(
                    writer.stream(),
                    &request_id,
                    &workflow_id,
                    sequence + 1,
                    &stdio_rpc_client_error("conversation_execution_read_failed"),
                );
            }
        })
        .map_err(|_| stdio_rpc_client_error("agent_conversation_dispatch_failed"))
}

fn capture_snapshot(
    store: &ConversationStore,
    scope: &ConversationRuntimeScope,
    turn: Option<&Arc<PersistentTurn>>,
) -> Result<(RuntimeExecutionSnapshot, bool, u64)> {
    // Same lock ordering as frame/terminal writers prevents a finish between
    // the durable snapshot and the observation-availability decision.
    let state = turn.map(|turn| turn.state.lock().expect("turn state lock"));
    let snapshot = store.runtime_execution_snapshot(scope)?;
    let available =
        state.as_ref().is_some_and(|state| state.terminal.is_none()) && !snapshot.is_terminal();
    let generation = state.as_ref().map_or(0, |state| state.execution_generation);
    Ok((snapshot, available, generation))
}

fn execution_state(
    scope: &ConversationRuntimeScope,
    snapshot: &RuntimeExecutionSnapshot,
    observation_available: bool,
) -> Value {
    json!({
        "turnHandle":scope.dispatch_id,"conversationId":scope.conversation_id,"membershipId":scope.membership_id,
        "cursor":snapshot.records_high_water(),"status":snapshot.status,
        "terminalPayloadAvailable":snapshot.terminal_payload_available,
        "observationAvailable":observation_available,
    })
}

fn replay_execution<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    request_id: &str,
    workflow_id: &str,
    store: &ConversationStore,
    scope: &ConversationRuntimeScope,
    turn: Option<&Arc<PersistentTurn>>,
    mut cursor: u64,
    sequence: &mut u64,
    observation: &ExecutionObservation,
) -> Result<()> {
    let mut history_ready = false;
    loop {
        let (snapshot, available, generation) = capture_snapshot(store, scope, turn)?;
        while cursor < snapshot.records_high_water()
            && !observation.detached.load(Ordering::Acquire)
        {
            let records = store.runtime_execution_records_after(scope, &snapshot, cursor, 1)?;
            if records.is_empty() {
                return Err(anyhow!("canonical_execution_replay_gap"));
            }
            for record in records {
                if record.cursor <= cursor {
                    return Err(anyhow!("canonical_execution_cursor_invalid"));
                }
                let next_cursor = record.cursor;
                write_execution_record(writer, request_id, workflow_id, scope, record, sequence)?;
                cursor = next_cursor;
            }
        }
        let detached = observation.detached.load(Ordering::Acquire);
        let mut status = execution_state(scope, &snapshot, available && !detached);
        if detached {
            status["detached"] = json!(true);
            status["cursor"] = json!(cursor);
        }
        if !history_ready && !detached {
            let mut ready = status.clone();
            ready["event"] = json!("agent.execution.ready");
            *sequence += 1;
            write_stdio_rpc_event(writer.stream(), request_id, workflow_id, *sequence, ready)?;
            history_ready = true;
        }
        if !available || detached {
            *sequence += 1;
            return write_stdio_rpc_terminal_success(
                writer.stream(),
                request_id,
                workflow_id,
                *sequence,
                status,
            )
            .map_err(Into::into);
        }
        let turn = turn.expect("available observation has a turn");
        let mut state = turn.state.lock().expect("turn state lock");
        while state.execution_generation == generation
            && state.terminal.is_none()
            && !observation.detached.load(Ordering::Acquire)
        {
            state = turn.changed.wait(state).expect("turn state lock");
        }
    }
}

fn write_execution_record<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    request_id: &str,
    workflow_id: &str,
    scope: &ConversationRuntimeScope,
    record: licoup_conversation::store::ExecutionRecord,
    sequence: &mut u64,
) -> Result<()> {
    // A record can exceed the transport frame bound. Split only transport
    // text; the receiver commits this cursor after every part is reassembled.
    const PART_BYTES: usize = 512 * 1024;
    let mut parts = Vec::new();
    let mut start = 0;
    while start < record.raw_text.len() {
        let mut end = (start + PART_BYTES).min(record.raw_text.len());
        while !record.raw_text.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(&record.raw_text[start..end]);
        start = end;
    }
    if parts.is_empty() {
        parts.push("");
    }
    for (part_index, raw_text) in parts.iter().enumerate() {
        *sequence += 1;
        write_stdio_rpc_event(
            writer.stream(),
            request_id,
            workflow_id,
            *sequence,
            json!({
                "event":"agent.execution.record","turnHandle":scope.dispatch_id,
                "conversationId":scope.conversation_id,"membershipId":scope.membership_id,
                "cursor":record.cursor,"partIndex":part_index,"partCount":parts.len(),
                "record":{"id":record.id,"kind":record.kind,"timestamp":record.timestamp,"cursor":record.cursor,"rawText":raw_text},
            }),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn runtime() -> PersistentConversationRuntime {
        PersistentConversationRuntime::with_cache_budget(
            ConversationStore::open_in_memory().unwrap(),
            1,
        )
    }
    fn params(turn: &PersistentTurn, after_cursor: u64) -> Value {
        json!({"turnHandle":turn.scope.dispatch_id,"conversationId":turn.scope.conversation_id,"membershipId":turn.scope.membership_id,"afterCursor":after_cursor})
    }
    fn frames(writer: &Arc<StdioRpcWriter<Vec<u8>>>) -> Vec<Value> {
        String::from_utf8(writer.lock_for_test().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn finish(turn: &Arc<PersistentTurn>) {
        PersistentConversationRuntime::finish(
            turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok":true,"output":"synthetic final","nativeUnknown":[1,2,3]}),
            },
        )
        .unwrap();
    }

    #[test]
    fn execution_raw_transport_frames_are_ordered_private_and_drained_before_terminal() {
        use licoup_native::platform::raw_execution::RawExecutionDirection as Direction;
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"synthetic prompt"}))
            .unwrap();
        let observer = PersistentConversationRuntime::raw_execution_observer(&turn);
        let original = format!(
            "  [{{\"unknown\":\"{}\",\"toolArguments\":{{\"secretExample\":\"synthetic-only\"}}}}] \r\n",
            "字".repeat(190_000)
        );
        observer.record("synthetic", Direction::Sent, &original);
        PersistentConversationRuntime::record_event(
            &turn,
            json!({"event":"agent.synthetic","payload":{"text":"public"}}),
        )
        .unwrap();
        observer.record(
            "synthetic",
            Direction::Received,
            "invalid JSON\0 retained verbatim",
        );
        PersistentConversationRuntime::close_raw_execution_observer(&turn, &observer);
        finish(&turn);
        observer.record(
            "synthetic",
            Direction::Received,
            "late frame from closed invocation",
        );
        let snapshot = turn.store.runtime_execution_snapshot(&turn.scope).unwrap();
        let records = turn
            .store
            .runtime_execution_records_after(&turn.scope, &snapshot, 0, 20)
            .unwrap();
        let protocol = records
            .iter()
            .filter(|record| record.kind.starts_with("protocol."))
            .collect::<Vec<_>>();
        assert_eq!(protocol.len(), 2);
        assert_eq!(protocol[0].raw_text, original);
        assert_eq!(protocol[0].kind, "protocol.synthetic.sent");
        assert_eq!(protocol[1].raw_text, "invalid JSON\0 retained verbatim");
        assert_eq!(records.last().unwrap().kind, "terminal");
        assert!(
            records
                .windows(2)
                .all(|window| window[0].cursor < window[1].cursor)
        );
        let local = Arc::new(StdioRpcWriter::new(Vec::new()));
        spawn_execution(
            local.clone(),
            "local".into(),
            "workflow".into(),
            params(&turn, 0),
            runtime,
        )
        .unwrap()
        .join()
        .unwrap();
        let raw_from_replay = frames(&local)
            .iter()
            .filter_map(|frame| frame.pointer("/event/record"))
            .filter(|record| record["kind"] == "protocol.synthetic.sent")
            .map(|record| record["rawText"].as_str().unwrap())
            .collect::<String>();
        assert_eq!(raw_from_replay, original);
        let public = Arc::new(StdioRpcWriter::new(Vec::new()));
        replay_turn(&public, "public", "workflow", &turn, 0).unwrap();
        let public = String::from_utf8(public.lock_for_test().clone()).unwrap();
        assert!(!public.contains("toolArguments"));
        assert!(!public.contains("retained verbatim"));
    }

    #[test]
    fn execution_raw_capture_failure_is_visible_in_persisted_terminal_without_changing_success() {
        use licoup_native::platform::raw_execution::RawExecutionDirection;
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"synthetic prompt"}))
            .unwrap();
        let connection = rusqlite::Connection::open(turn.store.db_path()).unwrap();
        connection.execute_batch("CREATE TRIGGER synthetic_capture_failure BEFORE INSERT ON event_parts WHEN NEW.execution_kind IS NOT NULL BEGIN SELECT RAISE(FAIL,'synthetic capture fault'); END;").unwrap();
        let observer = PersistentConversationRuntime::raw_execution_observer(&turn);
        observer.record(
            "synthetic",
            RawExecutionDirection::Received,
            "received while capture store fails",
        );
        connection
            .execute_batch("DROP TRIGGER synthetic_capture_failure;")
            .unwrap();
        PersistentConversationRuntime::close_raw_execution_observer(&turn, &observer);
        finish(&turn);
        let snapshot = turn.store.runtime_execution_snapshot(&turn.scope).unwrap();
        assert_eq!(snapshot.status, "completed");
        let records = turn
            .store
            .runtime_execution_records_after(&turn.scope, &snapshot, 0, 20)
            .unwrap();
        let terminal: Value = serde_json::from_str(&records.last().unwrap().raw_text).unwrap();
        assert_eq!(terminal["ok"], true);
        assert_eq!(terminal["output"], "synthetic final");
        assert_eq!(
            terminal["licoUpExecutionCapture"],
            json!({"complete":false,"errorCode":"conversation_persistence_failed"})
        );
        assert!(!turn.cancel_requested.load(Ordering::Acquire));
    }

    #[test]
    fn execution_provenance_records_provider_keys_without_using_host_turn_ids() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"synthetic prompt"}))
            .unwrap();
        PersistentConversationRuntime::record_event(&turn,json!({"event":"agent.turn.accepted","sessionId":"provider-session","turnId":"host-local-id"})).unwrap();
        assert!(turn.state.lock().unwrap().native_provenance_keys.is_empty());
        let provider_ack = json!({"event":"agent.turn.accepted","sessionId":"provider-session","turnId":"provider-turn","payload":{"nativeTurnId":"provider-turn"}});
        PersistentConversationRuntime::record_event(&turn, provider_ack.clone()).unwrap();
        PersistentConversationRuntime::record_event(&turn, provider_ack).unwrap();
        assert_eq!(turn.state.lock().unwrap().native_provenance_keys.len(), 1);
        PersistentConversationRuntime::record_native_provenance(&turn,&json!({"driverId":"other-adapter","sessionId":"provider-session","turnId":"host-terminal-id"}),true).unwrap();
        assert_eq!(turn.state.lock().unwrap().native_provenance_keys.len(), 1);
        PersistentConversationRuntime::record_native_provenance(&turn,&json!({"driverId":"codex-app-server","nativeSessionId":"provider-session","turnId":"native-terminal-id"}),true).unwrap();
        assert_eq!(turn.state.lock().unwrap().native_provenance_keys.len(), 2);
        finish(&turn);
    }

    #[test]
    fn execution_replay_preserves_raw_evidence_after_registry_eviction_and_reconnect() {
        let runtime = runtime();
        let turn = runtime.begin(&json!({"agent":"synthetic","text":"synthetic prompt","arbitraryRequest":[1,{"a":2}]})).unwrap();
        PersistentConversationRuntime::record_event(
            &turn,
            json!({"event":"agent.turn.context","payload":{"unknown":[1,{"opaque":true}]}}),
        )
        .unwrap();
        finish(&turn);
        runtime
            .inner
            .turns
            .lock()
            .unwrap()
            .remove(&turn.scope.dispatch_id);
        let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
        spawn_execution(
            writer.clone(),
            "request".into(),
            "workflow".into(),
            params(&turn, 0),
            runtime.clone(),
        )
        .unwrap()
        .join()
        .unwrap();
        let all = frames(&writer);
        let records = all
            .iter()
            .filter_map(|frame| frame.pointer("/event/record"))
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["kind"], "request");
        assert!(
            records[0]["rawText"]
                .as_str()
                .unwrap()
                .contains("arbitraryRequest")
        );
        assert_eq!(records[1]["kind"], "runtime");
        assert_eq!(records[2]["kind"], "terminal");
        let result = &all.last().unwrap()["result"];
        assert_eq!(result["cursor"], 3);
        assert_eq!(result["status"], "completed");
        assert_eq!(result["observationAvailable"], false);
        assert_eq!(result["terminalPayloadAvailable"], true);
        let resumed = Arc::new(StdioRpcWriter::new(Vec::new()));
        spawn_execution(
            resumed.clone(),
            "reconnect".into(),
            "workflow".into(),
            params(&turn, 3),
            runtime,
        )
        .unwrap()
        .join()
        .unwrap();
        let resumed = frames(&resumed);
        assert_eq!(resumed.len(), 2);
        assert_eq!(resumed[0]["event"]["event"], "agent.execution.ready");
        assert_eq!(resumed[1]["result"]["cursor"], 3);
    }

    #[test]
    fn execution_local_and_public_views_keep_the_agents_own_prose() {
        let runtime = runtime();
        let turn = runtime.begin_with(&json!({"agent":"synthetic","text":"prompt","continuityKind":CONTINUITY_KIND_USER_POSTED}),PersistentTurnAdmission::Host).unwrap();
        let emitted = PersistentConversationRuntime::record_event(&turn,json!({"event":"agent.message.chunk","payload":{"text":"synthetic unpublished full raw text"}})).unwrap();
        assert_eq!(
            emitted["payload"]["text"],
            "synthetic unpublished full raw text"
        );
        finish(&turn);
        let local = Arc::new(StdioRpcWriter::new(Vec::new()));
        spawn_execution(
            local.clone(),
            "local".into(),
            "workflow".into(),
            params(&turn, 0),
            runtime,
        )
        .unwrap()
        .join()
        .unwrap();
        assert!(
            String::from_utf8(local.lock_for_test().clone())
                .unwrap()
                .contains("synthetic unpublished full raw text")
        );
        let public = Arc::new(StdioRpcWriter::new(Vec::new()));
        replay_turn(&public, "public", "workflow", &turn, 0).unwrap();
        assert!(
            String::from_utf8(public.lock_for_test().clone())
                .unwrap()
                .contains("synthetic unpublished full raw text")
        );
    }

    #[test]
    fn execution_unavailable_observer_returns_stored_state_without_false_completion() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"prompt"}))
            .unwrap();
        runtime
            .inner
            .turns
            .lock()
            .unwrap()
            .remove(&turn.scope.dispatch_id);
        let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
        spawn_execution(
            writer.clone(),
            "read".into(),
            "workflow".into(),
            params(&turn, 0),
            runtime,
        )
        .unwrap()
        .join()
        .unwrap();
        let frames = frames(&writer);
        assert_eq!(frames.last().unwrap()["result"]["status"], "accepted");
        assert_eq!(
            frames.last().unwrap()["result"]["observationAvailable"],
            false
        );
        assert_eq!(
            frames.last().unwrap()["result"]["terminalPayloadAvailable"],
            false
        );
        assert!(!turn.cancel_requested.load(Ordering::Acquire));
    }

    #[test]
    fn execution_historical_empty_dispatch_finishes_read_without_inventing_terminal_payload() {
        let runtime = runtime();
        let scope = runtime
            .inner
            .store
            .prepare_runtime_dispatch("synthetic", "", "old prompt", None, None, None, None)
            .unwrap();
        runtime
            .inner
            .store
            .update_dispatch(&scope.dispatch_id, DispatchState::Completed, None, None)
            .unwrap();
        let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
        spawn_execution(writer.clone(),"historical".into(),"workflow".into(),json!({
            "turnHandle":scope.dispatch_id,"conversationId":scope.conversation_id,"membershipId":scope.membership_id,
        }),runtime).unwrap().join().unwrap();
        let frames = frames(&writer);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["event"]["event"], "agent.execution.ready");
        assert_eq!(frames[0]["event"]["cursor"], 0);
        assert_eq!(frames[1]["result"]["status"], "completed");
        assert_eq!(frames[1]["result"]["terminalPayloadAvailable"], false);
    }

    #[test]
    fn execution_large_record_transport_parts_reassemble_exactly() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"prompt"}))
            .unwrap();
        let raw = format!(" {{\n\"text\": \"{}\" }} ", "字\\n".repeat(400_000));
        let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
        let mut sequence = 0;
        write_execution_record(
            &writer,
            "read",
            "workflow",
            &turn.scope,
            licoup_conversation::store::ExecutionRecord {
                id: "synthetic:2".into(),
                kind: "runtime".into(),
                raw_text: raw.clone(),
                timestamp: 7,
                cursor: 2,
            },
            &mut sequence,
        )
        .unwrap();
        let frames = frames(&writer);
        assert!(frames.len() > 1);
        let joined = frames
            .iter()
            .map(|frame| frame["event"]["record"]["rawText"].as_str().unwrap())
            .collect::<String>();
        assert_eq!(joined, raw);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame["event"]["partIndex"], index);
            assert_eq!(frame["event"]["partCount"], frames.len());
            assert_eq!(frame["event"]["record"]["cursor"], 2);
        }
    }

    struct ObservedWriter {
        pending: Vec<u8>,
        tx: mpsc::Sender<Value>,
        disconnected: Arc<AtomicBool>,
    }
    impl Write for ObservedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.disconnected.load(Ordering::Acquire) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.pending.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            let frame = serde_json::from_slice(&self.pending).map_err(io::Error::other)?;
            self.pending.clear();
            self.tx.send(frame).map_err(io::Error::other)
        }
    }

    #[test]
    fn execution_live_observer_wakes_on_commit_and_disconnect_does_not_cancel_turn() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"prompt"}))
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let disconnected = Arc::new(AtomicBool::new(false));
        let writer = Arc::new(StdioRpcWriter::new(ObservedWriter {
            pending: Vec::new(),
            tx,
            disconnected: disconnected.clone(),
        }));
        let worker = spawn_execution(
            writer,
            "live".into(),
            "workflow".into(),
            params(&turn, 0),
            runtime,
        )
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap()["event"]["record"]["kind"],
            "request"
        );
        let ready = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ready["event"]["observationAvailable"], true);
        assert!(!worker.is_finished());
        assert!(rx.try_recv().is_err());
        let raw_observer = PersistentConversationRuntime::raw_execution_observer(&turn);
        raw_observer.record(
            "synthetic",
            licoup_native::platform::raw_execution::RawExecutionDirection::Received,
            "unprojected original tool arguments",
        );
        let raw_frame = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            raw_frame["event"]["record"]["kind"],
            "protocol.synthetic.received"
        );
        assert_eq!(
            raw_frame["event"]["record"]["rawText"],
            "unprojected original tool arguments"
        );
        assert_eq!(turn.state.lock().unwrap().high_water, 0);
        PersistentConversationRuntime::close_raw_execution_observer(&turn, &raw_observer);
        PersistentConversationRuntime::record_event(&turn,json!({"event":licoup_conversation::projection::USER_MESSAGE_EVENT_KIND,"payload":{"text":"synthetic follow up","extra":["original metadata"]}})).unwrap();
        let speech = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            speech["event"]["record"]["rawText"]
                .as_str()
                .unwrap()
                .contains("original metadata")
        );
        assert_eq!(turn.state.lock().unwrap().high_water, 0);
        PersistentConversationRuntime::record_event(
            &turn,
            json!({"event":"agent.turn.processing","payload":{"actual":"metadata"}}),
        )
        .unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap()["event"]["record"]["cursor"]
                .as_u64()
                .unwrap()
                > speech["event"]["record"]["cursor"].as_u64().unwrap()
        );
        disconnected.store(true, Ordering::Release);
        PersistentConversationRuntime::record_event(
            &turn,
            json!({"event":"agent.turn.processing","payload":{"next":"metadata"}}),
        )
        .unwrap();
        worker.join().unwrap();
        assert!(!turn.cancel_requested.load(Ordering::Acquire));
        assert!(turn.state.lock().unwrap().terminal.is_none());
        finish(&turn);
    }

    #[test]
    fn execution_request_scope_and_cursor_are_admitted_before_spawn() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"prompt"}))
            .unwrap();
        for (key, value) in [
            ("conversationId", json!("wrong-conversation")),
            ("membershipId", json!("wrong-member")),
            ("turnHandle", json!("wrong-dispatch")),
            ("afterCursor", json!(200)),
            ("afterCursor", json!(-1)),
        ] {
            let mut request = params(&turn, 0);
            request[key] = value;
            assert!(
                spawn_execution(
                    Arc::new(StdioRpcWriter::new(Vec::new())),
                    "invalid".into(),
                    "workflow".into(),
                    request,
                    runtime.clone()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn execution_detach_releases_repeated_quiet_observers_without_cancelling_turn() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"prompt"}))
            .unwrap();
        for index in 0..80 {
            let request_id = format!("observer-{index}");
            let (tx, rx) = mpsc::channel();
            let writer = Arc::new(StdioRpcWriter::new(ObservedWriter {
                pending: Vec::new(),
                tx,
                disconnected: Arc::new(AtomicBool::new(false)),
            }));
            let worker = spawn_execution(
                writer,
                request_id.clone(),
                "workflow".into(),
                params(&turn, 0),
                runtime.clone(),
            )
            .unwrap();
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap()["event"]["record"]["kind"],
                "request"
            );
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap()["event"]["event"],
                "agent.execution.ready"
            );
            let mut detach = params(&turn, 0);
            detach["requestId"] = json!(request_id);
            detach["workflowId"] = json!("workflow");
            assert_eq!(runtime.detach_execution(&detach).unwrap()["detached"], true);
            let terminal = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(terminal["kind"], "terminal");
            assert_eq!(terminal["result"]["detached"], true);
            assert_eq!(terminal["result"]["status"], "accepted");
            assert_eq!(terminal["result"]["observationAvailable"], false);
            worker.join().unwrap();
            assert!(runtime.inner.execution_observers.lock().unwrap().is_empty());
            assert_eq!(
                runtime.detach_execution(&detach).unwrap()["detached"],
                false
            );
        }
        assert!(!turn.cancel_requested.load(Ordering::Acquire));
        assert_eq!(turn.state.lock().unwrap().high_water, 0);
        assert!(turn.state.lock().unwrap().terminal.is_none());
        finish(&turn);
    }

    #[test]
    fn execution_detach_checks_scope_and_leaves_other_observers_running() {
        let runtime = runtime();
        let turn = runtime
            .begin(&json!({"agent":"synthetic","text":"prompt"}))
            .unwrap();
        let mut observers = Vec::new();
        for request_id in ["first", "second"] {
            let (tx, rx) = mpsc::channel();
            let writer = Arc::new(StdioRpcWriter::new(ObservedWriter {
                pending: Vec::new(),
                tx,
                disconnected: Arc::new(AtomicBool::new(false)),
            }));
            let worker = spawn_execution(
                writer,
                request_id.into(),
                "workflow".into(),
                params(&turn, 0),
                runtime.clone(),
            )
            .unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            observers.push((worker, rx));
        }
        let mut detach = params(&turn, 0);
        detach["requestId"] = json!("first");
        detach["workflowId"] = json!("workflow");
        let mut wrong = detach.clone();
        wrong["membershipId"] = json!("another-member");
        assert!(runtime.detach_execution(&wrong).is_err());
        assert!(!observers[0].0.is_finished());
        runtime.detach_execution(&detach).unwrap();
        let (first, rx) = observers.remove(0);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap()["result"]["detached"],
            true
        );
        first.join().unwrap();
        assert!(!observers[0].0.is_finished());
        PersistentConversationRuntime::record_event(
            &turn,
            json!({"event":"agent.synthetic","payload":{"text":"still observing"}}),
        )
        .unwrap();
        assert_eq!(
            observers[0].1.recv_timeout(Duration::from_secs(5)).unwrap()["event"]["record"]["kind"],
            "runtime"
        );
        finish(&turn);
        let (second, rx) = observers.remove(0);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap()["event"]["record"]["kind"],
            "terminal"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap()["result"]["status"],
            "completed"
        );
        second.join().unwrap();
        assert!(runtime.inner.execution_observers.lock().unwrap().is_empty());
        assert!(!turn.cancel_requested.load(Ordering::Acquire));
    }
}
