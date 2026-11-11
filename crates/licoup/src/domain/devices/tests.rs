use super::command::*;
use super::processor::DeviceProcessor;
use super::session::{DeviceIdentity, device_line};
use super::state::{DevicesState, Pairing, Peer};
use super::*;
use anyhow::Result;
use licoarc_client::ports::{PortFailure, Transport, TransportOutcome};
use licoarc_client::{HandleToken, Verdict};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

struct TempHome(PathBuf);

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_home(tag: &str) -> TempHome {
    let path = std::env::temp_dir().join(format!(
        "licoup-devices-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).unwrap();
    TempHome(path)
}

/// Counts executions and answers with a fixed reply.
#[derive(Clone, Default)]
struct CountingExecutor {
    calls: Rc<Cell<usize>>,
    fail_with: Option<&'static str>,
}

impl DeviceCommandExecutor for CountingExecutor {
    fn execute(&mut self, _command: &DeviceCommand, params: Value) -> Result<Value> {
        self.calls.set(self.calls.get() + 1);
        if let Some(code) = self.fail_with {
            return successful_agent_dispatch(Ok(json!({"ok": false, "error": {"code": code}})));
        }
        Ok(json!({ "reply": "done", "agent": params["agent"] }))
    }
}

fn message(command_id: &str) -> DeviceCommand {
    DeviceCommand::agent_message(
        command_id.to_owned(),
        "codex",
        Some("session-1"),
        "hello",
        "2026-11-09T00:00:00Z".to_owned(),
    )
}

fn ledger(home: &TempHome) -> DeviceCommandLedger {
    DevicesState::open(&devices_dir(&home.0))
        .unwrap()
        .ledger()
        .unwrap()
}

#[test]
fn the_record_payload_round_trips_and_refuses_unknown_shapes() {
    let command = message("cmd-1");
    let record = DeviceRecord::Command(command.clone());
    assert_eq!(
        DeviceRecord::decode(&record.encode().unwrap()).unwrap(),
        record
    );
    let mut value = command.to_value();
    value["senderIdentity"] = json!({"endpointId": "legacy"});
    assert_eq!(
        DeviceCommand::from_value(&value).unwrap_err().to_string(),
        "device_command_invalid"
    );
    let mut value = command.to_value();
    value["schema"] = json!("licomesh.secure-mesh.command.v1");
    assert!(DeviceRecord::decode(&serde_json::to_vec(&value).unwrap()).is_err());

    let result = DeviceCommandResult::from_value(&json!({
        "schema": DEVICE_RESULT_SCHEMA,
        "commandId": "cmd-1",
        "status": "completed",
        "output": {"reply": "ok"},
        "completedAt": "2026-11-09T00:00:01Z",
    }))
    .unwrap();
    assert_eq!(result.status(), "completed");
}

#[test]
fn a_duplicate_delivery_executes_once_and_returns_the_recorded_result() {
    let home = temp_home("duplicate");
    let ledger = ledger(&home);
    let mut executor = CountingExecutor::default();
    let command = message("cmd-dup");
    let first = execute_device_command(
        &command,
        "peer-a",
        true,
        &ledger,
        &mut executor,
        "2026-11-09T00:00:02Z",
    )
    .unwrap();
    let second = execute_device_command(
        &command,
        "peer-a",
        true,
        &ledger,
        &mut executor,
        "2026-11-09T00:00:09Z",
    )
    .unwrap();
    assert_eq!(executor.calls.get(), 1);
    assert!(first.completed);
    assert_eq!(first, second);

    // The same command id from another peer is a different delivery.
    execute_device_command(
        &command,
        "peer-b",
        true,
        &ledger,
        &mut executor,
        "2026-11-09T00:00:10Z",
    )
    .unwrap();
    assert_eq!(executor.calls.get(), 2);

    // A redelivery whose content changed is refused, not executed.
    let mut changed = message("cmd-dup");
    changed.params["text"] = json!("other");
    let conflict = execute_device_command(
        &changed,
        "peer-a",
        true,
        &ledger,
        &mut executor,
        "2026-11-09T00:00:11Z",
    )
    .unwrap();
    assert_eq!(executor.calls.get(), 2);
    assert_eq!(conflict.output["errorCode"], "command_replay_conflict");
}

#[test]
fn an_unpaired_peer_and_unscoped_commands_are_refused_without_execution() {
    let home = temp_home("refusal");
    let ledger = ledger(&home);
    let mut executor = CountingExecutor::default();
    let refused = execute_device_command(
        &message("cmd-unpaired"),
        "peer-gone",
        false,
        &ledger,
        &mut executor,
        "2026-11-09T00:00:00Z",
    )
    .unwrap();
    assert_eq!(refused.output["errorCode"], "device_peer_not_paired");

    let mut shell = message("cmd-shell");
    shell.command_kind = "shell.exec".to_owned();
    let mut unscoped = message("cmd-env");
    unscoped.params["env"] = json!({"PATH": "/tmp"});
    let mut unbound = message("cmd-unbound");
    unbound.params.as_object_mut().unwrap().remove("agent");
    for (command, code) in [
        (shell, "command_not_allowlisted"),
        (unscoped, "unscoped_execution_field"),
        (unbound, "agent_binding_required"),
    ] {
        let result = execute_device_command(
            &command,
            "peer-a",
            true,
            &ledger,
            &mut executor,
            "2026-11-09T00:00:00Z",
        )
        .unwrap();
        assert!(!result.completed);
        assert_eq!(result.output["errorCode"], code);
    }
    assert_eq!(executor.calls.get(), 0);
}

#[test]
fn agent_dispatch_failures_keep_their_classified_codes() {
    let home = temp_home("dispatch");
    let ledger = ledger(&home);
    let mut executor = CountingExecutor {
        fail_with: Some("adapter_login_required"),
        ..CountingExecutor::default()
    };
    let result = execute_device_command(
        &message("cmd-auth"),
        "peer-a",
        true,
        &ledger,
        &mut executor,
        "2026-11-09T00:00:00Z",
    )
    .unwrap();
    assert_eq!(
        result.output,
        json!({"errorCode": "native_agent_authentication_required", "retryable": true})
    );
    let params = agent_message_params(&message("cmd-params")).unwrap();
    assert_eq!(params["idempotencyKey"], "cmd-params");
    assert!(params.get("timeoutMs").is_none());
}

#[test]
fn results_are_recorded_once_and_only_from_the_peer_that_was_asked() {
    let home = temp_home("results");
    let ledger = ledger(&home);
    ledger
        .record_outbound("intent-1", "cmd-r", "peer-a", "2026-11-09T00:00:00Z")
        .unwrap();
    let result = DeviceCommandResult {
        command_id: "cmd-r".to_owned(),
        completed: true,
        output: json!({"reply": "ok"}),
        completed_at: "2026-11-09T00:00:05Z".to_owned(),
    };
    assert!(ledger.record_result("peer-a", &result).unwrap());
    assert!(!ledger.record_result("peer-a", &result).unwrap());
    assert!(ledger.record_result("peer-b", &result).is_err());
    let listed = message_results_in(&home.0, Some("peer-a"), None).unwrap();
    assert_eq!(listed["results"].as_array().unwrap().len(), 1);
    assert_eq!(listed["results"][0]["status"], "completed");
    let later = message_results_in(&home.0, None, Some("2026-11-09T00:00:06Z")).unwrap();
    assert!(later["results"].as_array().unwrap().is_empty());
}

#[test]
fn status_reports_enrollment_peers_and_the_retired_relay_store() {
    let home = temp_home("status");
    let status = status_in(&home.0).unwrap();
    assert_eq!(status["enrolled"], false);
    assert_eq!(status["legacyPairingsDetected"], false);
    assert_eq!(status["mailboxId"], Value::Null);

    let legacy = home.0.join("client-state/mobile-relay/config.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, b"{\"schemaVersion\":2}").unwrap();
    let state = DevicesState::open(&devices_dir(&home.0)).unwrap();
    state
        .insert_peer(Peer {
            peer_id: "peer-a".to_owned(),
            label: "Studio".to_owned(),
            paired_at: "2026-11-09T00:00:00Z".to_owned(),
            role: "initiator".to_owned(),
            local_handle: String::new(),
            remote_handle: String::new(),
            session_dir: String::new(),
            peer_identity: Value::Null,
            send_bootstrapped: false,
        })
        .unwrap();
    state
        .insert_pairing(Pairing {
            pairing_id: "pairing-1".to_owned(),
            role: "offerer".to_owned(),
            state: "awaiting-claim".to_owned(),
            created_at: "2026-11-09T00:00:00Z".to_owned(),
            handle: String::new(),
            session_dir: String::new(),
            peer_identity: None,
            label: None,
            peer_id: None,
            reply_b64: None,
            reply_handle: None,
            reply_pending: false,
        })
        .unwrap();
    let status = status_in(&home.0).unwrap();
    assert_eq!(status["legacyPairingsDetected"], true);
    assert_eq!(status["peers"][0]["peerId"], "peer-a");
    assert_eq!(status["pairings"][0]["state"], "awaiting-claim");
    // The retired store is preserved exactly.
    assert_eq!(std::fs::read(&legacy).unwrap(), b"{\"schemaVersion\":2}");

    assert_eq!(
        pair_cancel_in(&home.0, "pairing-1").unwrap(),
        json!({"ok": true})
    );
    assert_eq!(
        pair_cancel_in(&home.0, "pairing-1")
            .unwrap_err()
            .to_string(),
        "devices_pairing_not_found"
    );
    assert_eq!(unpair_in(&home.0, "peer-a").unwrap(), json!({"ok": true}));
    assert_eq!(
        unpair_in(&home.0, "peer-a").unwrap_err().to_string(),
        "devices_peer_not_found"
    );
}

#[test]
fn enrollment_input_is_validated_before_any_custody_or_network_use() {
    let home = temp_home("enroll");
    for (input, code) in [
        (
            json!({"station": "http://station.test", "invitation": "btm1.x"}),
            "devices_station_invalid",
        ),
        (
            json!({"station": "https://station.test", "invitation": "bto1.x"}),
            "devices_invitation_invalid",
        ),
        (
            json!({"station": "https://station.test", "invitation": "btm1.x", "ca": "not a pem"}),
            "devices_ca_invalid",
        ),
        (
            json!({"station": "https://station.test", "invitation": "btm1.x", "extra": 1}),
            "devices_enroll_input_invalid",
        ),
    ] {
        assert_eq!(enroll_in(&home.0, &input).unwrap_err().to_string(), code);
    }
    assert!(!devices_dir(&home.0).join("enrollment.json").exists());
}

#[test]
fn an_unreachable_station_leaves_no_enrollment_and_no_device_key() {
    let home = temp_home("unreachable");
    let store: std::sync::Arc<dyn crate::platform::secret_store::SecretStore> =
        std::sync::Arc::new(crate::platform::secret_store::EphemeralSecretStore::new());
    let refused = custody::with_secret_store_override(store.clone(), || {
        enroll_in(
            &home.0,
            &json!({"station": "https://127.0.0.1:9", "invitation": "btm1.AAAA"}),
        )
    });
    assert_eq!(
        refused.unwrap_err().to_string(),
        "devices_station_unavailable"
    );
    assert!(!devices_dir(&home.0).join("enrollment.json").exists());
    let missing = custody::with_secret_store_override(store, || {
        custody::DeviceKeyCustody::for_devices_dir(&devices_dir(&home.0))
            .unwrap()
            .load()
    });
    assert_eq!(
        missing.unwrap_err().to_string(),
        "devices_device_key_missing"
    );
}

/// A closure-backed `Transport`: each submitted packet is captured verbatim
/// and handed to the delivery closure.
struct FnTransport<F: FnMut(&[u8]) -> Result<TransportOutcome, PortFailure>>(F);

impl<F: FnMut(&[u8]) -> Result<TransportOutcome, PortFailure>> Transport for FnTransport<F> {
    fn submit(&mut self, packet: &[u8]) -> Result<TransportOutcome, PortFailure> {
        (self.0)(packet)
    }
}

/// Enrolls one home against a synthetic station identity; the integration
/// test never contacts a station — packets move through the in-process bus.
fn enroll_home(home: &TempHome, mailbox_id: &str) -> DevicesState {
    let state = DevicesState::open(&devices_dir(&home.0)).unwrap();
    state
        .write_enrollment(&Enrollment {
            station: "https://station.test".to_owned(),
            invitation: "btm1.test".to_owned(),
            mailbox_id: mailbox_id.to_owned(),
            enrolled_at: "2026-11-09T00:00:00Z".to_owned(),
        })
        .unwrap();
    state
}

/// Two data homes pair through real pairing envelopes, exchange one command
/// and its result through real endpoint records, refuse a duplicate delivery
/// and refuse post-unpair traffic — all in-process, no station. Gated on
/// `LICOARC_AUTHORITY_BUNDLE` like the SDK's own devnet tests.
#[test]
fn two_homes_pair_send_results_and_unpair() {
    if std::env::var_os(session::AUTHORITY_BUNDLE_ENV).is_none() {
        eprintln!("skipping: LICOARC_AUTHORITY_BUNDLE is not set");
        return;
    }
    let store: Arc<dyn crate::platform::secret_store::SecretStore> =
        Arc::new(crate::platform::secret_store::EphemeralSecretStore::new());
    custody::with_secret_store_override(store, || {
        let line = device_line().unwrap();
        let home_a = temp_home("bus-a");
        let home_b = temp_home("bus-b");
        let state_a = enroll_home(&home_a, &"aa".repeat(16));
        let state_b = enroll_home(&home_b, &"bb".repeat(16));

        // A publishes an offer; B builds and records its claim. The claim
        // envelope then travels to A's bootstrap handle on the bus.
        let offer = pair_offer_in(&home_a.0, Some("Studio")).unwrap();
        let pairing_a = offer["pairingId"].as_str().unwrap().to_owned();
        let claim =
            pair_claim_prepare(&home_b.0, &json!({"code": offer["code"], "label": "Field"}))
                .unwrap();
        let pairing_b = claim.pairing.pairing_id.clone();
        state_b.upsert_pairing(&claim.pairing).unwrap();

        let exec_a = CountingExecutor::default();
        let exec_b = CountingExecutor::default();
        let calls_a = exec_a.calls.clone();
        let calls_b = exec_b.calls.clone();
        let proc_a = Rc::new(RefCell::new(
            DeviceProcessor::open(
                &state_a,
                line.clone(),
                DeviceIdentity::open(&state_a.identity_dir()).unwrap(),
                exec_a,
            )
            .unwrap(),
        ));
        let proc_b = Rc::new(RefCell::new(
            DeviceProcessor::open(
                &state_b,
                line.clone(),
                DeviceIdentity::open(&state_b.identity_dir()).unwrap(),
                exec_b,
            )
            .unwrap(),
        ));
        let seq_a = Rc::new(Cell::new(0_u64));
        let seq_b = Rc::new(Cell::new(0_u64));

        // Claim → A's bootstrap handle → reply leaves A's outbox → B's reply
        // handle completes the pairing on both sides.
        seq_a.set(seq_a.get() + 1);
        assert_eq!(
            proc_a
                .borrow_mut()
                .accept(seq_a.get(), claim.offer_handle, &claim.envelope),
            Verdict::Done
        );
        let outbox = proc_a.borrow_mut().take_outbox();
        assert_eq!(outbox.len(), 1);
        seq_b.set(seq_b.get() + 1);
        assert_eq!(
            proc_b
                .borrow_mut()
                .accept(seq_b.get(), outbox[0].handle, &outbox[0].packet),
            Verdict::Done
        );

        let peers_a = state_a.peer_rows().unwrap();
        let peers_b = state_b.peer_rows().unwrap();
        assert_eq!(peers_a.len(), 1);
        assert_eq!(peers_b.len(), 1);
        assert_eq!(peers_a[0].role, "responder");
        assert_eq!(peers_b[0].role, "initiator");
        assert_eq!(peers_a[0].local_handle, peers_b[0].remote_handle);
        assert_eq!(peers_b[0].local_handle, peers_a[0].remote_handle);

        // The bus: packets submitted for one peer's remote handle are
        // delivered to the other home's processor on its local handle and
        // captured for replay tests.
        let captured_b = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
        let to_b = {
            let proc_b = proc_b.clone();
            let seq_b = seq_b.clone();
            let captured = captured_b.clone();
            let handle = HandleToken::parse(&peers_b[0].local_handle).unwrap();
            FnTransport(move |packet: &[u8]| {
                captured.borrow_mut().push(packet.to_vec());
                seq_b.set(seq_b.get() + 1);
                match proc_b.borrow_mut().accept(seq_b.get(), handle, packet) {
                    Verdict::Done => Ok(TransportOutcome::Accepted),
                    Verdict::Defer => Err(PortFailure::Unavailable),
                }
            })
        };
        let captured_a = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
        let to_a = {
            let proc_a = proc_a.clone();
            let seq_a = seq_a.clone();
            let captured = captured_a.clone();
            let handle = HandleToken::parse(&peers_a[0].local_handle).unwrap();
            FnTransport(move |packet: &[u8]| {
                captured.borrow_mut().push(packet.to_vec());
                seq_a.set(seq_a.get() + 1);
                match proc_a.borrow_mut().accept(seq_a.get(), handle, packet) {
                    Verdict::Done => Ok(TransportOutcome::Accepted),
                    Verdict::Defer => Err(PortFailure::Unavailable),
                }
            })
        };

        // A sends one command; B executes once and its result record returns
        // on the reverse bus.
        let send = message_send_prepare_in(
            &home_a.0,
            &peers_a[0].peer_id,
            "codex",
            None,
            &json!({"body": "summarize", "clientIntentId": "intent-1"}),
        )
        .unwrap();
        assert!(!send.duplicate);
        proc_a
            .borrow_mut()
            .send_record(&peers_a[0].peer_id, &send.record)
            .unwrap();
        proc_a
            .borrow_mut()
            .drain_outbound(&peers_a[0].session_dir, to_b)
            .unwrap();
        assert_eq!(calls_b.get(), 1);
        proc_b
            .borrow_mut()
            .drain_outbound(&peers_b[0].session_dir, to_a)
            .unwrap();

        let results = message_results_in(&home_a.0, Some(&peers_a[0].peer_id), None).unwrap();
        assert_eq!(results["results"].as_array().unwrap().len(), 1);
        assert_eq!(results["results"][0]["commandId"], send.command_id);
        assert_eq!(results["results"][0]["status"], "completed");

        // The same client intent is an idempotent retry: one command id, no
        // second record.
        let retry = message_send_prepare_in(
            &home_a.0,
            &peers_a[0].peer_id,
            "codex",
            None,
            &json!({"body": "summarize", "clientIntentId": "intent-1"}),
        )
        .unwrap();
        assert!(retry.duplicate);
        assert_eq!(retry.command_id, send.command_id);

        // Replaying the captured command packet to B is refused by the
        // session layer without executing again.
        let replay = captured_b.borrow()[0].clone();
        seq_b.set(seq_b.get() + 1);
        let handle_b = HandleToken::parse(&peers_b[0].local_handle).unwrap();
        assert_eq!(
            proc_b.borrow_mut().accept(seq_b.get(), handle_b, &replay),
            Verdict::Done
        );
        assert_eq!(calls_b.get(), 1);

        // After A unpairs, B's lane is refused: the packet resolves (no
        // redelivery storm) but nothing executes on A.
        assert_eq!(
            unpair_in(&home_a.0, &peers_a[0].peer_id).unwrap(),
            json!({"ok": true})
        );
        let later = message_send_prepare_in(
            &home_b.0,
            &peers_b[0].peer_id,
            "codex",
            None,
            &json!({"body": "late", "clientIntentId": "intent-2"}),
        )
        .unwrap();
        proc_b
            .borrow_mut()
            .send_record(&peers_b[0].peer_id, &later.record)
            .unwrap();
        let handle_a = HandleToken::parse(&peers_a[0].local_handle).unwrap();
        let to_a_after_unpair = {
            let proc_a = proc_a.clone();
            let seq_a = seq_a.clone();
            FnTransport(move |packet: &[u8]| {
                seq_a.set(seq_a.get() + 1);
                match proc_a.borrow_mut().accept(seq_a.get(), handle_a, packet) {
                    Verdict::Done => Ok(TransportOutcome::Accepted),
                    Verdict::Defer => Err(PortFailure::Unavailable),
                }
            })
        };
        proc_b
            .borrow_mut()
            .drain_outbound(&peers_b[0].session_dir, to_a_after_unpair)
            .unwrap();
        assert_eq!(calls_a.get(), 0);
        assert!(proc_a.borrow().received >= 1);

        // Completed pairing rows refuse `pair cancel` — ending them is
        // `devices unpair`'s job and their session directory is live.
        assert_eq!(
            pair_cancel_in(&home_a.0, &pairing_a)
                .unwrap_err()
                .to_string(),
            "devices_pairing_completed"
        );
        assert_eq!(
            pair_cancel_in(&home_b.0, &pairing_b)
                .unwrap_err()
                .to_string(),
            "devices_pairing_completed"
        );
    });
}
