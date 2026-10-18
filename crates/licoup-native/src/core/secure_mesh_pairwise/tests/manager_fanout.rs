use super::test_support::*;

#[test]
fn secure_mesh_sesame_multi_device_fanout_uses_independent_pairwise_envelopes_and_ack_purge() {
    let canary = "multi-device-fanout-canary-secret";
    let (mut sender_to_desktop, mut desktop_receiver) =
        pairwise_sessions_between("desktop_sidecar:alice-pc-a", "desktop_sidecar:bob-pc-b");
    let (mut sender_to_mobile, mut mobile_receiver) =
        pairwise_sessions_between("desktop_sidecar:alice-pc-a", "mobile:bob-mobile-c");
    let mut manager = SecureMeshSesameSessionManager::new(2);
    manager
        .activate_session(
            "bob",
            "desktop_sidecar:bob-pc-b",
            sender_to_desktop.session_id.clone(),
        )
        .unwrap();
    manager
        .activate_session(
            "bob",
            "mobile:bob-mobile-c",
            sender_to_mobile.session_id.clone(),
        )
        .unwrap();
    assert_eq!(
        manager.fanout_targets_for_user("bob"),
        vec![
            (
                "desktop_sidecar:bob-pc-b".to_string(),
                sender_to_desktop.session_id.clone()
            ),
            (
                "mobile:bob-mobile-c".to_string(),
                sender_to_mobile.session_id.clone()
            )
        ]
    );

    let mut relay = OpaquePairwiseRelay::default();
    let desktop_context = payload_context_with_mailbox(
        &sender_to_desktop,
        "msg-fanout-1",
        "mbx-fanout-a13f",
        "desktop_sidecar:alice-pc-a",
        "desktop_sidecar:bob-pc-b",
    );
    let mobile_context = payload_context_with_mailbox(
        &sender_to_mobile,
        "msg-fanout-2",
        "mbx-fanout-b94c",
        "desktop_sidecar:alice-pc-a",
        "mobile:bob-mobile-c",
    );
    let desktop_plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        serde_json::to_vec(&json!({
            "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "commandKind": "agent.message.send",
            "targetEndpointId": "desktop_sidecar:bob-pc-b",
            "body": {"message": canary}
        }))
        .unwrap(),
    )
    .with_content_type("application/json");
    let mobile_plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        serde_json::to_vec(&json!({
            "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "commandKind": "agent.message.send",
            "targetEndpointId": "mobile:bob-mobile-c",
            "body": {"message": canary}
        }))
        .unwrap(),
    )
    .with_content_type("application/json");

    let desktop_envelope = sender_to_desktop
        .seal_payload_envelope(&desktop_context, &desktop_plaintext)
        .unwrap();
    let mobile_envelope = sender_to_mobile
        .seal_payload_envelope(&mobile_context, &mobile_plaintext)
        .unwrap();
    assert_ne!(desktop_envelope.ciphertext(), mobile_envelope.ciphertext());
    for forbidden in [
        canary,
        "desktop_sidecar:alice-pc-a",
        "desktop_sidecar:bob-pc-b",
        "mobile:bob-mobile-c",
        "agent.message.send",
    ] {
        assert!(!desktop_envelope.envelope_id().contains(forbidden));
        assert!(!desktop_envelope.mailbox_id().contains(forbidden));
        assert!(!desktop_envelope.ciphertext().contains(forbidden));
        assert!(!mobile_envelope.envelope_id().contains(forbidden));
        assert!(!mobile_envelope.mailbox_id().contains(forbidden));
        assert!(!mobile_envelope.ciphertext().contains(forbidden));
    }

    relay.send(desktop_envelope, canary);
    relay.send(mobile_envelope, canary);
    assert_eq!(relay.queue_len(), 2);

    let desktop_synced = relay.sync("mbx-fanout-a13f");
    let mobile_synced = relay.sync("mbx-fanout-b94c");
    assert_eq!(desktop_synced.len(), 1);
    assert_eq!(mobile_synced.len(), 1);
    let wrong_recipient = mobile_receiver
        .open_payload_envelope(&desktop_synced[0], SecureMeshPayloadKind::Command)
        .unwrap_err();
    assert!(
        wrong_recipient
            .to_string()
            .contains("header authentication failed")
    );

    let opened_desktop = desktop_receiver
        .open_payload_envelope(&desktop_synced[0], SecureMeshPayloadKind::Command)
        .unwrap();
    let opened_mobile = mobile_receiver
        .open_payload_envelope(&mobile_synced[0], SecureMeshPayloadKind::Command)
        .unwrap();
    let desktop_value: Value = serde_json::from_slice(&opened_desktop.body).unwrap();
    let mobile_value: Value = serde_json::from_slice(&opened_mobile.body).unwrap();
    assert_eq!(
        desktop_value["targetEndpointId"],
        "desktop_sidecar:bob-pc-b"
    );
    assert_eq!(mobile_value["targetEndpointId"], "mobile:bob-mobile-c");
    assert_eq!(desktop_value["body"]["message"], canary);
    assert_eq!(mobile_value["body"]["message"], canary);

    assert!(!relay.ack("msg-fanout-1"));
    assert!(relay.sync("mbx-fanout-a13f").is_empty());
    assert_eq!(relay.queue_len(), 1);
    assert!(!relay.ack("msg-fanout-2"));
    assert!(relay.ack("msg-fanout-2"));
    assert_eq!(relay.queue_len(), 0);
}

#[test]
fn secure_mesh_sesame_session_manager_tracks_devices_convergence_and_revoke() {
    let mut manager = SecureMeshSesameSessionManager::new(2);
    manager
        .activate_session("alice", "desktop_gui:alice", "session-b")
        .unwrap();
    manager
        .activate_session("alice", "mobile:alice", "session-mobile")
        .unwrap();
    assert_eq!(
        manager.active_sessions_for_user("alice"),
        vec!["session-b".to_string(), "session-mobile".to_string()]
    );

    let chosen = manager
        .converge_session_collision("alice", "desktop_gui:alice", "session-a")
        .unwrap();
    assert_eq!(chosen, "session-a");
    manager
        .activate_session("alice", "desktop_gui:alice", "session-c")
        .unwrap();
    manager
        .activate_session("alice", "desktop_gui:alice", "session-d")
        .unwrap();
    let desktop = manager.device_record("alice", "desktop_gui:alice").unwrap();
    assert_eq!(desktop.active_session_id.as_deref(), Some("session-d"));
    assert_eq!(
        desktop.inactive_session_ids,
        vec!["session-a".to_string(), "session-c".to_string()]
    );

    let fanout = manager.fanout_targets_for_user("alice");
    assert_eq!(fanout.len(), 2);
    manager.revoke_device("alice", "mobile:alice").unwrap();
    let fanout_after_revoke = manager.fanout_targets_for_user("alice");
    assert_eq!(
        fanout_after_revoke,
        vec![("desktop_gui:alice".to_string(), "session-d".to_string())]
    );
    let mobile = manager.device_record("alice", "mobile:alice").unwrap();
    assert!(mobile.revoked);
    assert!(mobile.stale);
    assert!(mobile.inactive_session_ids.is_empty());
}
