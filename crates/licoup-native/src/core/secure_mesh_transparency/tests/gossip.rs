use super::super::persistence::require_fresh_gossip_observation_transaction;
use super::super::*;
use super::support::{leaf, state_path};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use rusqlite::{TransactionBehavior, params};

#[test]
fn gossip_json_codec_round_trips_without_leaf_lists() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    log.append_leaf(&leaf("device-codec", 1, "active")).unwrap();
    let gossip = SecureMeshKtGossipPayload::from_sth(log.sign_tree_head(100).unwrap(), None);

    let encoded = gossip.to_json_bytes().unwrap();
    assert!(!String::from_utf8_lossy(&encoded).contains("leafHashes"));
    assert_eq!(
        SecureMeshKtGossipPayload::from_json_bytes(&encoded).unwrap(),
        gossip,
    );
}

#[test]
fn gossip_same_size_split_view_is_persisted() {
    let path = state_path("gossip-split");
    let shared = SigningKey::generate(&mut OsRng);
    let bytes = shared.to_bytes();
    let mut first =
        SecureMeshKtLog::with_identity(SigningKey::from_bytes(&bytes), "gossip-log", "gossip-key");
    let mut split =
        SecureMeshKtLog::with_identity(SigningKey::from_bytes(&bytes), "gossip-log", "gossip-key");
    first.append_leaf(&leaf("device-a", 1, "active")).unwrap();
    split.append_leaf(&leaf("device-b", 1, "active")).unwrap();
    let pin = first.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    let first_gossip =
        SecureMeshKtGossipPayload::from_sth(first.sign_tree_head(100).unwrap(), None);

    let mut state = SecureMeshKtClientState::open(&path, pin.clone(), policy).unwrap();
    state.observe_peer_gossip_sth(&first_gossip, 100).unwrap();
    let split_gossip =
        SecureMeshKtGossipPayload::from_sth(split.sign_tree_head(100).unwrap(), None);
    let error = state
        .observe_peer_gossip_sth(&split_gossip, 100)
        .unwrap_err();
    assert!(error.to_string().contains("same-size split view"));
    drop(state);
    let restored = SecureMeshKtClientState::open(&path, pin, policy).unwrap();
    assert!(restored.equivocation_detected().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn gossip_observations_bind_distinct_issue_times_for_the_same_tree_view() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    log.append_leaf(&leaf("device-a", 1, "active")).unwrap();
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    let first_sth = log.sign_tree_head(100).unwrap();
    let second_sth = log.sign_tree_head(101).unwrap();
    let first_gossip = SecureMeshKtGossipPayload::from_sth(first_sth.clone(), None);
    let second_gossip = SecureMeshKtGossipPayload::from_sth(second_sth.clone(), None);
    let mut state = SecureMeshKtClientState::open_in_memory(pin.clone(), policy).unwrap();

    state.observe_peer_gossip_sth(&first_gossip, 100).unwrap();
    state.observe_peer_gossip_sth(&second_gossip, 101).unwrap();

    let observation_count: i64 = state
        .connection
        .query_row(
            "SELECT COUNT(*) FROM secure_mesh_kt_gossip_observations WHERE log_id = ?1",
            params![pin.log_id()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(observation_count, 2);
    let transaction = state
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    require_fresh_gossip_observation_transaction(&transaction, &pin, &first_sth, policy, 101)
        .unwrap();
    require_fresh_gossip_observation_transaction(&transaction, &pin, &second_sth, policy, 101)
        .unwrap();
    transaction.commit().unwrap();
}
