use super::test_support::*;

#[test]
fn secure_mesh_pairwise_relay_header_public_boundary_is_explicit_and_payload_free() {
    let (mut sender_session, mut receiver_session) = pairwise_sessions_between(
        "desktop_sidecar:relay-header-sender-private-canary",
        "mobile:relay-header-recipient-private-canary",
    );
    let context = payload_context_with_mailbox(
        &sender_session,
        "msg-relay-header-boundary",
        "mailbox-relay-header-boundary",
        &sender_session.local_endpoint_id,
        &sender_session.remote_endpoint_id,
    );
    let plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        serde_json::to_vec(&json!({
            "commandKind": "agent.message.send",
            "targetEndpointId": "mobile:relay-header-recipient-private-canary",
            "targetAgentId": "agent-relay-header-private-canary",
            "token": (["relay", "header", "private", "token", "canary"].join("-")),
            "fileName": "relay-header-private-file-canary.txt",
            "path": "test-data/relay-header-private-path-canary",
            "body": {"message": "relay-header-private-payload-canary"}
        }))
        .unwrap(),
    )
    .with_content_type("application/json");
    let envelope = sender_session
        .seal_payload_envelope(&context, &plaintext)
        .unwrap();
    let relay_header_bytes = envelope.decoded_encrypted_header().unwrap();
    let relay_header_wire = String::from_utf8_lossy(&relay_header_bytes);
    for forbidden in [
        "relay-header-sender-private-canary",
        "relay-header-recipient-private-canary",
        "agent-relay-header-private-canary",
        "relay-header-private-token-canary",
        "relay-header-private-file-canary.txt",
        "relay-header-private-path-canary",
        "relay-header-private-payload-canary",
        "agent.message.send",
    ] {
        assert!(
            !relay_header_wire.contains(forbidden),
            "pairwise relay header leaked {forbidden}"
        );
    }
    assert_relay_envelope_hides(
        &envelope,
        &[
            "relay-header-sender-private-canary",
            "relay-header-recipient-private-canary",
            "agent-relay-header-private-canary",
            "relay-header-private-token-canary",
            "relay-header-private-file-canary.txt",
            "relay-header-private-path-canary",
            "relay-header-private-payload-canary",
            "agent.message.send",
        ],
    );
    let opened = receiver_session
        .open_payload_envelope(&envelope, SecureMeshPayloadKind::Command)
        .unwrap();
    assert_eq!(opened.body, plaintext.body);
}

#[test]
fn secure_mesh_pairwise_envelope_failure_rolls_back_complete_triple_ratchet_state() {
    let (mut sender, _) = pairwise_sessions();
    let mut context = payload_context_with_mailbox(
        &sender,
        "msg-envelope-transaction",
        "mailbox-envelope-transaction",
        &sender.local_endpoint_id,
        &sender.remote_endpoint_id,
    );
    context.created_at = "x".repeat(4096);
    let before = Zeroizing::new(
        serde_json::to_vec(
            &sender
                .to_secret_snapshot(1, "transaction-test".to_string())
                .unwrap(),
        )
        .unwrap(),
    );

    let error = sender
        .seal_payload_envelope(
            &context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"must not advance state"),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("private relay header payload is too large")
    );

    let after = Zeroizing::new(
        serde_json::to_vec(
            &sender
                .to_secret_snapshot(1, "transaction-test".to_string())
                .unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(before.as_slice(), after.as_slice());
}

#[test]
fn licoarc_carrier_limit_fails_before_pairwise_ratchet_commit() {
    let (mut sender, _) = pairwise_sessions();
    let context = payload_context_with_mailbox(
        &sender,
        "msg-carrier-limit",
        "mailbox-carrier-limit",
        &sender.local_endpoint_id,
        &sender.remote_endpoint_id,
    );
    let before = Zeroizing::new(
        serde_json::to_vec(
            &sender
                .to_secret_snapshot(1, "carrier-limit-test".to_string())
                .unwrap(),
        )
        .unwrap(),
    );

    let error = sender
        .seal_payload_envelope(
            &context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, vec![0x41; 730 * 1024]),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("carrier exceeds the ciphertext character limit")
    );

    let after = Zeroizing::new(
        serde_json::to_vec(
            &sender
                .to_secret_snapshot(1, "carrier-limit-test".to_string())
                .unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(before.as_slice(), after.as_slice());
}

#[test]
fn secure_mesh_pairwise_pc_pc_command_result_relay_round_trip() {
    let canary = "pc-pc-command-canary-secret";
    let (mut pc_a_session, mut pc_b_session) =
        pairwise_sessions_between("desktop_sidecar:pc-a", "desktop_sidecar:pc-b");
    let mut relay = OpaquePairwiseRelay::default();

    let command = pc_pc_command_fixture("cmd-pcpc-1", "idem-pcpc-1", canary);
    let command_context = payload_context_with_mailbox(
        &pc_a_session,
        "msg-pcpc-command",
        "mailbox-pc-b",
        "desktop_sidecar:pc-a",
        "desktop_sidecar:pc-b",
    );
    let command_plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        serde_json::to_vec(&command).unwrap(),
    )
    .with_content_type("application/json");
    let command_envelope = pc_a_session
        .seal_payload_envelope(&command_context, &command_plaintext)
        .unwrap();
    relay.send(command_envelope, canary);
    assert_eq!(relay.queue_len(), 1);

    let synced_for_pc_b = relay.sync("mailbox-pc-b");
    assert_eq!(synced_for_pc_b.len(), 1);
    let opened_command = pc_b_session
        .open_payload_envelope(&synced_for_pc_b[0], SecureMeshPayloadKind::Command)
        .unwrap();
    assert_eq!(opened_command.kind, SecureMeshPayloadKind::Command);
    assert_eq!(
        opened_command.content_type.as_deref(),
        Some("application/json")
    );
    let command_value: Value = serde_json::from_slice(&opened_command.body).unwrap();
    assert_eq!(command_value["body"]["message"], canary);

    let command_payload = SecureCommandPayload::from_value(&command_value).unwrap();
    let command_gate =
        SecureCommandEvaluationContext::from_value(&pc_pc_command_context_fixture()).unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&command_payload, &command_gate, &mut ledger).unwrap();
    assert!(evaluation.accepted);
    assert!(evaluation.should_execute);
    let mut executor = PcRelayExecutor::default();
    let execution = execute_evaluated_secure_command(
        &command_payload,
        &evaluation,
        &mut executor,
        "2026-06-26T00:02:00Z",
    )
    .unwrap();
    assert_eq!(executor.calls, 1);
    let result = execution.result().unwrap();
    assert!(!String::from_utf8_lossy(&result.output).contains("requiresUserConfirmation"));

    assert!(!relay.ack("msg-pcpc-command"));
    assert!(relay.ack("msg-pcpc-command"));
    assert_eq!(relay.queue_len(), 0);

    let result_context = payload_context_with_mailbox(
        &pc_b_session,
        "msg-pcpc-result",
        "mailbox-pc-a",
        "desktop_sidecar:pc-b",
        "desktop_sidecar:pc-a",
    );
    let result_plaintext =
        SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, result.output.clone())
            .with_content_type("application/json");
    let result_envelope = pc_b_session
        .seal_payload_envelope(&result_context, &result_plaintext)
        .unwrap();
    relay.send(result_envelope, canary);
    let synced_for_pc_a = relay.sync("mailbox-pc-a");
    assert_eq!(synced_for_pc_a.len(), 1);
    let opened_result = pc_a_session
        .open_payload_envelope(&synced_for_pc_a[0], SecureMeshPayloadKind::ResultPayload)
        .unwrap();
    assert_eq!(opened_result.kind, SecureMeshPayloadKind::ResultPayload);
    assert_eq!(
        opened_result.content_type.as_deref(),
        Some("application/json")
    );
    let result_value: Value = serde_json::from_slice(&opened_result.body).unwrap();
    assert_eq!(result_value["ok"], true);
    assert_eq!(result_value["commandKind"], "agent.message.send");
    assert_eq!(result_value["output"]["message"], canary);

    assert!(!relay.ack("msg-pcpc-result"));
    assert_eq!(relay.queue_len(), 0);
}

#[test]
fn secure_mesh_pairwise_mobile_pc_command_result_relay_round_trip() {
    assert_pairwise_command_result_relay_round_trip(CommandRelayScenario {
        label: "mobile-pc",
        sender_endpoint_id: "mobile:phone-a",
        sender_endpoint_kind: "mobile",
        recipient_endpoint_id: "desktop_sidecar:pc-b",
        target_agent_id: "agent-pc-b",
        workspace_id: "workspace-a",
        sender_mailbox_id: "mbx-mobile-pc-sender",
        recipient_mailbox_id: "mbx-mobile-pc-recipient",
        canary: "mobile-pc-command-canary-secret",
    });
}

#[test]
fn secure_mesh_pairwise_pc_mobile_command_result_relay_round_trip() {
    assert_pairwise_command_result_relay_round_trip(CommandRelayScenario {
        label: "pc-mobile",
        sender_endpoint_id: "desktop_sidecar:pc-a",
        sender_endpoint_kind: "desktop_sidecar",
        recipient_endpoint_id: "mobile:phone-b",
        target_agent_id: "agent-mobile-b",
        workspace_id: "workspace-a",
        sender_mailbox_id: "mbx-pc-mobile-sender",
        recipient_mailbox_id: "mbx-pc-mobile-recipient",
        canary: "pc-mobile-command-canary-secret",
    });
}

#[test]
fn secure_mesh_pairwise_mobile_mobile_command_result_relay_round_trip() {
    assert_pairwise_command_result_relay_round_trip(CommandRelayScenario {
        label: "mobile-mobile",
        sender_endpoint_id: "mobile:phone-a",
        sender_endpoint_kind: "mobile",
        recipient_endpoint_id: "mobile:phone-b",
        target_agent_id: "agent-mobile-b",
        workspace_id: "workspace-a",
        sender_mailbox_id: "mbx-mobile-mobile-sender",
        recipient_mailbox_id: "mbx-mobile-mobile-recipient",
        canary: "mobile-mobile-command-canary-secret",
    });
}

#[test]
fn secure_mesh_pairwise_cli_desktop_command_result_relay_round_trip() {
    assert_pairwise_command_result_relay_round_trip(CommandRelayScenario {
        label: "cli-desktop",
        sender_endpoint_id: "cli:cli-a",
        sender_endpoint_kind: "cli",
        recipient_endpoint_id: "desktop_gui:desktop-b",
        target_agent_id: "agent-desktop-b",
        workspace_id: "workspace-a",
        sender_mailbox_id: "mbx-cli-desktop-sender",
        recipient_mailbox_id: "mbx-cli-desktop-recipient",
        canary: "cli-desktop-command-canary-secret",
    });
}

#[test]
fn secure_mesh_pairwise_agent_host_command_result_relay_round_trip() {
    assert_pairwise_command_result_relay_round_trip(CommandRelayScenario {
        label: "agent-host",
        sender_endpoint_id: "desktop_sidecar:pc-a",
        sender_endpoint_kind: "desktop_sidecar",
        recipient_endpoint_id: "agent_host:runtime-b",
        target_agent_id: "agent-runtime-b",
        workspace_id: "workspace-a",
        sender_mailbox_id: "mbx-agent-host-sender",
        recipient_mailbox_id: "mbx-agent-host-recipient",
        canary: "agent-host-command-canary-secret",
    });
}

#[test]
fn secure_mesh_pairwise_payload_codec_uses_ratchet_message_key() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let context = payload_context(
        &alice_session,
        "payload-1",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        serde_json::to_vec(&json!({
            "commandKind": "agent.message.send",
            "secret": (["session", "derived"].join("-"))
        }))
        .unwrap(),
    )
    .with_content_type("application/json");
    let sealed = alice_session.seal_payload(&context, &plaintext).unwrap();
    assert_eq!(sealed.cipher_suite, SECURE_MESH_PAIRWISE_CIPHER_SUITE);
    assert_eq!(sealed.session_id, alice_session.session_id);
    assert_eq!(sealed.message_id, context.message_id);
    assert!(!sealed.ciphertext.contains("session-derived"));
    assert_eq!(alice_session.sent_count(), 1);

    let opened = bob_session
        .open_payload(&context, &sealed, SecureMeshPayloadKind::Command)
        .unwrap();
    assert_eq!(opened.kind, SecureMeshPayloadKind::Command);
    assert_eq!(opened.body, plaintext.body);
    assert_eq!(opened.content_type.as_deref(), Some("application/json"));
    assert_eq!(bob_session.received_count(), 1);

    let replay = bob_session
        .open_payload(&context, &sealed, SecureMeshPayloadKind::Command)
        .unwrap_err();
    assert!(replay.to_string().contains("replay detected"));
}

#[test]
fn secure_mesh_pairwise_payload_open_failure_does_not_advance_ratchet() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let context = payload_context(
        &alice_session,
        "payload-atomic",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let plaintext =
        SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, br#"{"ok":true}"#);
    let sealed = alice_session.seal_payload(&context, &plaintext).unwrap();
    let mut wrong_context = context.clone();
    wrong_context.message_id = "payload-atomic-tampered".to_string();
    let received_before = bob_session.received_count();
    let error = bob_session
        .open_payload(
            &wrong_context,
            &sealed,
            SecureMeshPayloadKind::ResultPayload,
        )
        .unwrap_err();
    assert!(error.to_string().contains("context message mismatch"));
    assert_eq!(bob_session.received_count(), received_before);

    let opened = bob_session
        .open_payload(&context, &sealed, SecureMeshPayloadKind::ResultPayload)
        .unwrap();
    assert_eq!(opened.body, plaintext.body);
    assert_eq!(bob_session.received_count(), received_before + 1);
}

#[test]
fn secure_mesh_pairwise_payload_authenticates_complete_ratchet_header_without_state_advance() {
    let (mut alice_session, bob_session) = pairwise_sessions();
    let context = payload_context(
        &alice_session,
        "payload-ratchet-header-aad",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, br#"{"ok":true}"#);
    let sealed = alice_session.seal_payload(&context, &plaintext).unwrap();

    let mut changed_previous_chain_length = sealed.clone();
    changed_previous_chain_length.previous_chain_length = changed_previous_chain_length
        .previous_chain_length
        .checked_add(1)
        .unwrap();
    let mut changed_chain_index = sealed.clone();
    changed_chain_index.chain_index = changed_chain_index.chain_index.checked_add(1).unwrap();
    let mut changed_epoch = sealed.clone();
    changed_epoch.dh_epoch = changed_epoch.dh_epoch.checked_add(1).unwrap();
    let mut changed_ratchet_key = sealed.clone();
    changed_ratchet_key.sender_ratchet_public_key = SecureMeshPairwisePrivateKey::generate()
        .public_key()
        .to_vec();
    let mut changed_sparse_pq_number = sealed.clone();
    changed_sparse_pq_number.sparse_pq_header.message_number = changed_sparse_pq_number
        .sparse_pq_header
        .message_number
        .checked_add(1)
        .unwrap();

    for (label, tampered) in [
        ("previous-chain-length", changed_previous_chain_length),
        ("chain-index", changed_chain_index),
        ("dh-epoch", changed_epoch),
        ("ratchet-public-key", changed_ratchet_key),
        ("sparse-pq-message-number", changed_sparse_pq_number),
    ] {
        let mut receiver = bob_session.clone();
        let before = (
            receiver.receiving_chain_index,
            receiver.receiving_ratchet_epoch,
            receiver.dh_epoch,
            receiver.skipped_key_count(),
            receiver.pending_sending_ratchet,
            receiver.remote_ratchet_public_key,
        );
        let error = receiver
            .open_payload(&context, &tampered, SecureMeshPayloadKind::Command)
            .unwrap_err();
        assert!(!error.to_string().is_empty(), "{label} tamper was accepted");
        assert_eq!(
            (
                receiver.receiving_chain_index,
                receiver.receiving_ratchet_epoch,
                receiver.dh_epoch,
                receiver.skipped_key_count(),
                receiver.pending_sending_ratchet,
                receiver.remote_ratchet_public_key,
            ),
            before,
            "{label} tamper advanced authenticated state"
        );
        let opened = receiver
            .open_payload(&context, &sealed, SecureMeshPayloadKind::Command)
            .unwrap();
        assert_eq!(opened.body, plaintext.body, "{label} damaged valid state");
    }
}

#[test]
fn secure_mesh_pairwise_encrypted_relay_header_hides_ratchet_structure_and_rejects_tamper() {
    let (mut sender, mut receiver) = pairwise_sessions();
    let context = payload_context_with_mailbox(
        &sender,
        "msg-encrypted-header-tamper",
        "mailbox-encrypted-header-tamper",
        &sender.local_endpoint_id,
        &sender.remote_endpoint_id,
    );
    let envelope = sender
        .seal_payload_envelope(
            &context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"opaque command"),
        )
        .unwrap();
    let wire = envelope.decoded_encrypted_header().unwrap();
    assert_eq!(
        wire.len(),
        licoup_protocol_bindings::licoarc_relay::LICOARC_ENCRYPTED_HEADER_BYTES
    );
    assert!(!wire.windows(8).any(|window| window == 1u64.to_be_bytes()));
    assert!(
        !wire
            .windows(sender.local_ratchet_public_key.len())
            .any(|window| window == sender.local_ratchet_public_key)
    );

    let mut tampered_value: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();
    let mut tampered_wire = general_purpose::URL_SAFE_NO_PAD
        .decode(envelope.ciphertext())
        .unwrap();
    tampered_wire[32] ^= 1;
    tampered_value["ciphertext"] =
        Value::String(general_purpose::URL_SAFE_NO_PAD.encode(tampered_wire));
    let tampered =
        LicoArcRelayEnvelope::from_json(&serde_json::to_string(&tampered_value).unwrap()).unwrap();
    assert!(
        receiver
            .open_payload_envelope(&tampered, SecureMeshPayloadKind::Command)
            .unwrap_err()
            .to_string()
            .contains("header authentication failed")
    );
    assert_eq!(receiver.received_count(), 0);

    let mut rebound_value: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();
    rebound_value["envelopeId"] =
        Value::String(general_purpose::URL_SAFE_NO_PAD.encode([0x7fu8; 24]));
    let rebound =
        LicoArcRelayEnvelope::from_json(&serde_json::to_string(&rebound_value).unwrap()).unwrap();
    assert!(
        receiver
            .open_payload_envelope(&rebound, SecureMeshPayloadKind::Command)
            .unwrap_err()
            .to_string()
            .contains("header authentication failed")
    );
    assert_eq!(receiver.received_count(), 0);
    assert_eq!(
        receiver
            .open_payload_envelope(&envelope, SecureMeshPayloadKind::Command)
            .unwrap()
            .body,
        b"opaque command"
    );
}

#[test]
fn secure_mesh_pairwise_payload_out_of_order_uses_bounded_skipped_keys() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first_context = payload_context(
        &alice_session,
        "payload-first",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let second_context = payload_context(
        &alice_session,
        "payload-second",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let first_plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"first-error");
    let second_plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"second-error");
    let first = alice_session
        .seal_payload(&first_context, &first_plaintext)
        .unwrap();
    let second = alice_session
        .seal_payload(&second_context, &second_plaintext)
        .unwrap();

    let opened_second = bob_session
        .open_payload(&second_context, &second, SecureMeshPayloadKind::Error)
        .unwrap();
    assert_eq!(opened_second.body, b"second-error");
    assert_eq!(bob_session.skipped_key_count(), 1);
    let opened_first = bob_session
        .open_payload(&first_context, &first, SecureMeshPayloadKind::Error)
        .unwrap();
    assert_eq!(opened_first.body, b"first-error");
    assert_eq!(bob_session.skipped_key_count(), 0);
}

#[test]
fn secure_mesh_pairwise_stale_and_replayed_relay_acks_do_not_advance_ratchet() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let before_epoch = alice_session.dh_epoch();
    let before_pending = alice_session.pending_sending_ratchet();
    let before_sent = alice_session.sent_count();
    let before_received = alice_session.received_count();

    let mut relay = OpaquePairwiseRelay::default();
    // Stale ACK for a message never queued, and a replayed ACK, are both outside crypto.
    assert!(relay.ack("msg-stale-ack"));
    assert!(relay.ack("msg-stale-ack"));
    assert!(relay.ack("msg-other-ack"));

    assert_eq!(alice_session.dh_epoch(), before_epoch);
    assert_eq!(alice_session.pending_sending_ratchet(), before_pending);
    assert_eq!(alice_session.sent_count(), before_sent);
    assert_eq!(alice_session.received_count(), before_received);
    assert_eq!(bob_session.dh_epoch(), 0);
    assert!(!bob_session.pending_sending_ratchet());

    // Authenticated remote ratchet still schedules rotation; ACKs never do.
    let first = alice_session
        .seal_message("msg-ack-crypto-1", b"authenticated body")
        .unwrap();
    bob_session.open_message(&first).unwrap();
    assert!(bob_session.pending_sending_ratchet());
    assert!(relay.ack("msg-ack-crypto-1"));
    assert!(relay.ack("msg-ack-crypto-1"));
    assert!(bob_session.pending_sending_ratchet());
    assert_eq!(bob_session.dh_epoch(), 1);
}
