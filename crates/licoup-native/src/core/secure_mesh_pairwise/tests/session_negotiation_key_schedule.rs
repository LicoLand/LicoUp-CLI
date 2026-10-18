use super::test_support::*;

#[test]
fn initial_key_schedule_is_deterministic_and_transcript_bound() {
    let first = derive_initial_keys(
        &[0x33; 32],
        "session-vector-a",
        "desktop:alice",
        "mobile:bob",
    )
    .unwrap();
    let repeated = derive_initial_keys(
        &[0x33; 32],
        "session-vector-a",
        "desktop:alice",
        "mobile:bob",
    )
    .unwrap();
    let changed = derive_initial_keys(
        &[0x33; 32],
        "session-vector-b",
        "desktop:alice",
        "mobile:bob",
    )
    .unwrap();
    assert_eq!(first.root_key, repeated.root_key);
    assert_eq!(first.initiator_chain_key, repeated.initiator_chain_key);
    assert_eq!(first.responder_chain_key, repeated.responder_chain_key);
    assert_ne!(first.root_key, changed.root_key);
    assert_ne!(first.initiator_header_key, first.responder_header_key);
    assert_ne!(
        first.initiator_next_header_key,
        first.responder_next_header_key
    );
}

#[test]
fn classical_secret_collection_binds_both_endpoints_and_optional_one_time_key() {
    let dh1 = [1; PUBLIC_KEY_LEN];
    let dh2 = [2; PUBLIC_KEY_LEN];
    let dh3 = [3; PUBLIC_KEY_LEN];
    let dh4 = [4; PUBLIC_KEY_LEN];
    let complete =
        collect_pqxdh_classical_secret("desktop:alice", "mobile:bob", &dh1, &dh2, &dh3, Some(&dh4))
            .unwrap();
    let without_one_time =
        collect_pqxdh_classical_secret("desktop:alice", "mobile:bob", &dh1, &dh2, &dh3, None)
            .unwrap();
    let swapped =
        collect_pqxdh_classical_secret("mobile:bob", "desktop:alice", &dh1, &dh2, &dh3, Some(&dh4))
            .unwrap();
    assert_ne!(&*complete, &*without_one_time);
    assert_ne!(&*complete, &*swapped);
}
