use super::test_support::*;

#[test]
fn secure_mesh_pairwise_rejects_unbounded_headers_and_chain_gaps_without_state_advance() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session
        .seal_message("msg-bounds", b"bounded")
        .unwrap();

    let mut huge_gap = first.clone();
    huge_gap.chain_index = u64::MAX;
    assert!(
        bob_session
            .open_message(&huge_gap)
            .unwrap_err()
            .to_string()
            .contains("skipped-key limit exceeded")
    );
    assert_eq!(bob_session.dh_epoch(), 0);
    assert_eq!(bob_session.received_count(), 0);

    let mut huge_header = first.clone();
    huge_header.encrypted_header =
        "A".repeat(encoded_len_limit(MAX_CONTENT_ENCRYPTED_HEADER_BYTES).saturating_add(1));
    assert_eq!(
        bob_session
            .open_message(&huge_header)
            .unwrap_err()
            .to_string(),
        "secure mesh pairwise encrypted header is too large"
    );
    assert_eq!(bob_session.dh_epoch(), 0);
    assert_eq!(bob_session.received_count(), 0);

    let mut impossible_ciphertext = first.clone();
    impossible_ciphertext.ciphertext_size = MAX_CIPHERTEXT_BYTES.saturating_add(1);
    assert_eq!(
        bob_session
            .open_message(&impossible_ciphertext)
            .unwrap_err()
            .to_string(),
        "secure mesh pairwise ciphertext size is outside bounds"
    );
    assert_eq!(bob_session.dh_epoch(), 0);
    assert_eq!(bob_session.received_count(), 0);
    assert_eq!(bob_session.open_message(&first).unwrap().body, b"bounded");
}

#[test]
fn secure_mesh_pairwise_pqxdh_triple_ratchet_round_trips() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session
        .seal_message("msg-1", b"hello bob without server plaintext")
        .unwrap();
    assert_eq!(first.dh_epoch, 1);
    assert!(!first.ciphertext.contains("hello"));
    let opened = bob_session.open_message(&first).unwrap();
    assert_eq!(opened.body, b"hello bob without server plaintext");

    let reply = bob_session
        .seal_message("msg-2", b"hello alice encrypted")
        .unwrap();
    assert_eq!(reply.dh_epoch, 2);
    let opened_reply = alice_session.open_message(&reply).unwrap();
    assert_eq!(opened_reply.body, b"hello alice encrypted");

    alice_session.rotate_sending_ratchet().unwrap();
    let after_ratchet = alice_session
        .seal_message("msg-3", b"post compromise recovery direction")
        .unwrap();
    assert_eq!(after_ratchet.dh_epoch, 3);
    let opened_after_ratchet = bob_session.open_message(&after_ratchet).unwrap();
    assert_eq!(
        opened_after_ratchet.body,
        b"post compromise recovery direction"
    );
}

#[test]
fn secure_mesh_pairwise_dh_ratchet_reply_auto_rotates_after_remote_ratchet() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session.seal_message("msg-auto-1", b"first").unwrap();
    assert_eq!(first.dh_epoch, 1);
    bob_session.open_message(&first).unwrap();
    assert!(bob_session.pending_sending_ratchet());

    let reply = bob_session
        .seal_message("msg-auto-2", b"bob auto ratchet reply")
        .unwrap();
    assert_eq!(reply.dh_epoch, 2);
    assert_eq!(reply.chain_index, 0);
    assert_eq!(reply.previous_chain_length, 0);
    assert!(!bob_session.pending_sending_ratchet());
    let opened_reply = alice_session.open_message(&reply).unwrap();
    assert_eq!(opened_reply.body, b"bob auto ratchet reply");
    assert!(alice_session.pending_sending_ratchet());

    let next = alice_session
        .seal_message("msg-auto-3", b"alice auto ratchet reply")
        .unwrap();
    assert_eq!(next.dh_epoch, 3);
    assert_eq!(next.chain_index, 0);
    assert_eq!(
        bob_session.open_message(&next).unwrap().body,
        b"alice auto ratchet reply"
    );
}

#[test]
fn secure_mesh_pairwise_dh_ratchet_preserves_old_chain_in_flight_messages() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let old_first = alice_session
        .seal_message("msg-inflight-1", b"old chain first")
        .unwrap();
    let old_second = alice_session
        .seal_message("msg-inflight-2", b"old chain delayed")
        .unwrap();
    assert_eq!(
        bob_session.open_message(&old_first).unwrap().body,
        b"old chain first"
    );
    let bob_reply = bob_session
        .seal_message("msg-inflight-reply", b"ratchet trigger")
        .unwrap();
    alice_session.open_message(&bob_reply).unwrap();
    let new_epoch = alice_session
        .seal_message("msg-inflight-3", b"new epoch arrives first")
        .unwrap();
    assert_eq!(new_epoch.dh_epoch, 3);
    assert_eq!(new_epoch.previous_chain_length, 2);
    assert_eq!(
        bob_session.open_message(&new_epoch).unwrap().body,
        b"new epoch arrives first"
    );
    assert_eq!(bob_session.skipped_key_count(), 1);

    let opened_delayed = bob_session.open_message(&old_second).unwrap();
    assert_eq!(opened_delayed.body, b"old chain delayed");
    assert_eq!(bob_session.skipped_key_count(), 0);
}

#[test]
fn secure_mesh_pairwise_encrypted_headers_preserve_old_chain_envelope_out_of_order() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let old_first_context = payload_context(
        &alice_session,
        "msg-header-inflight-1",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let old_second_context = payload_context(
        &alice_session,
        "msg-header-inflight-2",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let old_first = alice_session
        .seal_payload_envelope(
            &old_first_context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"old first"),
        )
        .unwrap();
    let old_second = alice_session
        .seal_payload_envelope(
            &old_second_context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"old delayed"),
        )
        .unwrap();
    bob_session
        .open_payload_envelope(&old_first, SecureMeshPayloadKind::Error)
        .unwrap();

    let reply_context = payload_context(
        &bob_session,
        "msg-header-inflight-reply",
        &bob_session.local_endpoint_id,
        &bob_session.remote_endpoint_id,
    );
    let reply = bob_session
        .seal_payload_envelope(
            &reply_context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, b"ratchet"),
        )
        .unwrap();
    alice_session
        .open_payload_envelope(&reply, SecureMeshPayloadKind::ResultPayload)
        .unwrap();

    let new_context = payload_context(
        &alice_session,
        "msg-header-inflight-new",
        &alice_session.local_endpoint_id,
        &alice_session.remote_endpoint_id,
    );
    let new_epoch = alice_session
        .seal_payload_envelope(
            &new_context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"new first"),
        )
        .unwrap();
    bob_session
        .open_payload_envelope(&new_epoch, SecureMeshPayloadKind::Error)
        .unwrap();
    assert_eq!(bob_session.skipped_key_count(), 1);
    assert_eq!(bob_session.skipped_receiving_header_keys.len(), 2);
    assert_eq!(
        bob_session
            .open_payload_envelope(&old_second, SecureMeshPayloadKind::Error,)
            .unwrap()
            .body,
        b"old delayed"
    );
    assert_eq!(bob_session.skipped_key_count(), 0);
}

#[test]
fn secure_mesh_pairwise_dh_ratchet_skip_limit_fails_closed_without_state_advance() {
    // Deliver the first message from the chain to authenticate the initiator, while
    // intentionally leaving more than MAX_SKIPPED_KEYS later messages in flight.
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session
        .seal_message("msg-skip-limit-0", b"first delivered")
        .unwrap();
    bob_session.open_message(&first).unwrap();
    for index in 1..=(MAX_SKIPPED_KEYS + 1) {
        alice_session
            .seal_message(format!("msg-skip-limit-{index}"), b"queued old chain")
            .unwrap();
    }
    let reply = bob_session
        .seal_message("msg-skip-limit-reply", b"ratchet trigger")
        .unwrap();
    alice_session.open_message(&reply).unwrap();
    let new_epoch = alice_session
        .seal_message("msg-skip-limit-new", b"new epoch")
        .unwrap();

    let error = bob_session.open_message(&new_epoch).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("skipped-key limit exceeded before ratchet"),
        "unexpected skip-limit error: {error}"
    );
    assert_eq!(bob_session.dh_epoch(), 2);
    assert_eq!(bob_session.received_count(), 1);
    assert_eq!(bob_session.skipped_key_count(), 0);
    assert!(!bob_session.pending_sending_ratchet());
}

#[test]
fn secure_mesh_pairwise_rejects_replay_and_supports_out_of_order_skipped_key() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session.seal_message("msg-1", b"one").unwrap();
    let second = alice_session.seal_message("msg-2", b"two").unwrap();

    let opened_second = bob_session.open_message(&second).unwrap();
    assert_eq!(opened_second.body, b"two");
    assert_eq!(bob_session.skipped_key_count(), 1);
    let opened_first = bob_session.open_message(&first).unwrap();
    assert_eq!(opened_first.body, b"one");
    assert_eq!(bob_session.skipped_key_count(), 0);

    let replay_error = bob_session.open_message(&second).unwrap_err();
    assert!(replay_error.to_string().contains("replay detected"));
}

#[test]
fn secure_mesh_pairwise_replay_cache_uses_message_tuple_fingerprint() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let message = alice_session
        .seal_message("msg-replay-fingerprint", b"tuple-bound")
        .unwrap();
    let fingerprint = message_replay_fingerprint(&message).unwrap();

    bob_session.open_message(&message).unwrap();

    assert_eq!(bob_session.received_message_ids.len(), 1);
    assert_eq!(bob_session.received_message_ids[0], fingerprint);
    assert_ne!(bob_session.received_message_ids[0], message.message_id);
    assert!(bob_session.received_message_ids[0].starts_with("sha256:"));
}

#[test]
fn secure_mesh_pairwise_skipped_key_gap_limit_rejects_without_state_advance() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let mut messages = Vec::new();
    for index in 0..(MAX_SKIPPED_KEYS + 2) {
        messages.push(
            alice_session
                .seal_message(format!("msg-{index}"), format!("body-{index}"))
                .unwrap(),
        );
    }
    let last = messages.last().unwrap().clone();
    let gap_error = bob_session.open_message(&last).unwrap_err();
    assert_eq!(
        gap_error.to_string(),
        "secure mesh pairwise skipped-key limit exceeded"
    );
    assert_eq!(bob_session.received_count(), 0);
    assert_eq!(bob_session.skipped_key_count(), 0);
    assert_eq!(
        bob_session.open_message(&messages[0]).unwrap().body,
        b"body-0"
    );
}

#[test]
fn secure_mesh_pairwise_revoked_session_fail_closed_for_seal_and_open() {
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let sealed = alice_session
        .seal_message("msg-before-revoke", b"before revoke")
        .unwrap();
    bob_session.revoke();
    let open_error = bob_session.open_message(&sealed).unwrap_err();
    assert!(open_error.to_string().contains("revoked"));
    let seal_error = bob_session
        .seal_message("msg-after-revoke", b"should fail")
        .unwrap_err();
    assert!(seal_error.to_string().contains("revoked"));
}
