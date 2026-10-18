use super::test_support::*;

fn received_payload(
    receipt_id: &str,
    binding_digest: &str,
    payload_value: &str,
) -> SecureMeshPairwiseReceivedPayload {
    SecureMeshPairwiseReceivedPayload {
        receipt_id: receipt_id.to_string(),
        binding_digest: binding_digest.to_string(),
        mailbox_id: "mailbox_received_payload_fixture".to_string(),
        payload_json: serde_json::to_string(&json!({
            "ok": true,
            "result": payload_value,
            "bodyRedacted": true
        }))
        .unwrap(),
        received_at: "2026-06-26T00:06:00Z".to_string(),
    }
}

#[test]
fn received_payload_commits_atomically_with_ratchet_and_survives_reopen_until_ack() {
    let store_path = durable_store_path("received-payload-reopen");
    let _ = std::fs::remove_file(&store_path);
    let (alice_session, _) = pairwise_sessions();
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let secret_store_trait: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let mut store = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store_trait),
        "received-payload-reopen",
    );
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:06:00Z")
        .unwrap();
    let authorization = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "received payload atomic commit",
            12,
        ))
        .unwrap();
    let payload = received_payload(
        "receipt_received_payload_reopen",
        "binding_received_payload_reopen",
        "received-payload-reopen-canary",
    );

    let committed = store
        .commit_session_with_authorized_session_and_received_payload(
            &initial,
            &alice_session,
            &payload,
            "2026-06-26T00:06:01Z",
            &authorization,
        )
        .unwrap();
    assert_eq!(committed.state_version, 2);
    assert_eq!(
        store
            .read_received_payload_with_authorized_session(
                &alice_session.session_id,
                &alice_session.local_endpoint_id,
                &payload.binding_digest,
                &authorization,
            )
            .unwrap(),
        Some(payload.clone())
    );

    drop(store);
    let mut reopened = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store_trait),
        "received-payload-reopen",
    );
    let reopen_authorization = reopened
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "received payload reopen and acknowledgement",
            8,
        ))
        .unwrap();
    assert_eq!(
        reopened
            .read_received_payload_with_authorized_session(
                &alice_session.session_id,
                &alice_session.local_endpoint_id,
                &payload.binding_digest,
                &reopen_authorization,
            )
            .unwrap(),
        Some(payload.clone())
    );
    assert!(
        reopened
            .delete_received_payload_with_authorized_session(
                &alice_session.session_id,
                &alice_session.local_endpoint_id,
                &payload.receipt_id,
                &reopen_authorization,
            )
            .unwrap()
    );
    assert!(
        reopened
            .read_received_payload_with_authorized_session(
                &alice_session.session_id,
                &alice_session.local_endpoint_id,
                &payload.binding_digest,
                &reopen_authorization,
            )
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn stale_ratchet_cas_cannot_publish_a_received_payload_or_displace_the_winner() {
    let store_path = durable_store_path("received-payload-stale-cas");
    let _ = std::fs::remove_file(&store_path);
    let (alice_session, _) = pairwise_sessions();
    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "received-payload-stale-cas",
    );
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:07:00Z")
        .unwrap();
    let authorization = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "received payload stale CAS",
            20,
        ))
        .unwrap();
    let winner = received_payload(
        "receipt_received_payload_winner",
        "binding_received_payload_winner",
        "winner",
    );
    store
        .commit_session_with_authorized_session_and_received_payload(
            &initial,
            &alice_session,
            &winner,
            "2026-06-26T00:07:01Z",
            &authorization,
        )
        .unwrap();
    let rejected = received_payload(
        "receipt_received_payload_rejected",
        "binding_received_payload_rejected",
        "rejected",
    );

    let stale_error = store
        .commit_session_with_authorized_session_and_received_payload(
            &initial,
            &alice_session,
            &rejected,
            "2026-06-26T00:07:02Z",
            &authorization,
        )
        .unwrap_err();

    assert!(stale_error.to_string().contains("compare-and-swap failed"));
    assert_eq!(
        store
            .read_received_payload_with_authorized_session(
                &alice_session.session_id,
                &alice_session.local_endpoint_id,
                &winner.binding_digest,
                &authorization,
            )
            .unwrap(),
        Some(winner)
    );
    assert!(
        store
            .read_received_payload_with_authorized_session(
                &alice_session.session_id,
                &alice_session.local_endpoint_id,
                &rejected.binding_digest,
                &authorization,
            )
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_file(&store_path);
}
