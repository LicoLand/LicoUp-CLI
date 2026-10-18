//! The device activation ledger's own contracts.
//!
//! These cases drive the store directly, over a real file or an in-memory
//! connection, and read back what it really wrote. They prove the four rules the
//! record exists for: a destination that is already active is never replaced in,
//! an activation never names its own source as the destination, the epoch only
//! ever advances through an epoch the caller actually read, and a revocation
//! records a cleanup as an intent rather than as a performed erasure.

use super::{
    AuthorityPosition, DeviceActivationLookup, DeviceActivationRefusal, DeviceActivationRequest,
    DeviceActivationState, DeviceCleanupIntent, MAX_SECURE_MESH_DEVICE_ACTIVATIONS,
    SecureMeshDeviceActivationStore, activation_authority_digest, device_activation_ledger_path,
};
use serde_json::json;

const SUBJECT: &str = "subject-a";

fn request(
    endpoint: &str,
    source: Option<&str>,
    state: DeviceActivationState,
    accepted_epoch: u64,
    superseded_epoch: Option<u64>,
    expected_epoch: Option<u64>,
) -> DeviceActivationRequest {
    let authority_state = json!({
        "recordType": "userAuthorityState",
        "authorityEpoch": accepted_epoch,
        "synthetic": true
    });
    DeviceActivationRequest {
        subject_identity_ref: SUBJECT.to_owned(),
        endpoint_identity_ref: endpoint.to_owned(),
        source_endpoint_identity_ref: source.map(str::to_owned),
        state,
        position: AuthorityPosition {
            accepted_epoch,
            superseded_epoch,
        },
        authority_state_digest: activation_authority_digest(&authority_state)
            .expect("synthetic authority digest"),
        authority_state,
        cleanup: DeviceCleanupIntent::none(),
        expected_epoch,
    }
}

fn activate(
    endpoint: &str,
    source: Option<&str>,
    epoch: u64,
    expected: Option<u64>,
) -> DeviceActivationRequest {
    request(
        endpoint,
        source,
        DeviceActivationState::Active,
        epoch,
        expected,
        expected,
    )
}

fn store() -> SecureMeshDeviceActivationStore {
    SecureMeshDeviceActivationStore::open_in_memory().expect("in-memory ledger opens")
}

#[test]
fn an_empty_ledger_admits_nothing_and_says_so() {
    let mut store = store();
    assert_eq!(
        store.lookup(SUBJECT, "device-a").expect("lookup runs"),
        DeviceActivationLookup::EmptyLedger
    );
    assert_eq!(store.current_epoch(SUBJECT).expect("epoch reads"), None);
    // A device identity is authorized by a record, never by the absence of one.
    assert!(!DeviceActivationLookup::EmptyLedger.admits_new_sessions());
    assert!(store.records(SUBJECT).expect("records read").is_empty());

    let written = store
        .apply(&activate("device-a", None, 1, None))
        .expect("the first activation writes");
    assert!(written.admits_new_sessions());
    assert_eq!(written.position.accepted_epoch, 1);
    assert_eq!(written.position.superseded_epoch, None);
    assert_eq!(written.state_version, 1);
    assert_eq!(store.current_epoch(SUBJECT).expect("epoch reads"), Some(1));
    assert_eq!(
        store.lookup(SUBJECT, "device-a").expect("lookup runs"),
        DeviceActivationLookup::Found(Box::new(written))
    );
    // Now the ledger is no longer empty, so an unknown destination is reported
    // as absent rather than as an empty ledger.
    assert_eq!(
        store.lookup(SUBJECT, "device-b").expect("lookup runs"),
        DeviceActivationLookup::Absent
    );
}

#[test]
fn an_already_active_destination_is_never_replaced_in() {
    let mut store = store();
    store
        .apply(&activate("device-a", None, 1, None))
        .expect("the first activation writes");

    // A second activation of the same destination, even at a later epoch with
    // different content, is refused: replacing an active destination is not a
    // replacement, it is a rotation with no authority for it here.
    let refusal = store
        .apply(&activate("device-a", Some("device-old"), 2, Some(1)))
        .expect_err("an active destination cannot be activated again");
    assert_eq!(refusal, DeviceActivationRefusal::DestinationAlreadyActive);
    assert!(refusal.wrote_nothing());
    assert_eq!(refusal.code(), "destination_already_active");

    // Nothing moved: the epoch, the content and the version all still stand.
    assert_eq!(store.current_epoch(SUBJECT).expect("epoch reads"), Some(1));
    let record = store
        .lookup(SUBJECT, "device-a")
        .expect("lookup runs")
        .record()
        .cloned()
        .expect("the record stands");
    assert_eq!(record.position.accepted_epoch, 1);
    assert_eq!(record.source_endpoint_identity_ref, None);
    assert_eq!(record.state_version, 1);
}

#[test]
fn an_activation_never_names_its_own_source_as_the_destination() {
    let mut store = store();
    let refusal = store
        .apply(&activate("device-a", Some("device-a"), 1, None))
        .expect_err("new endpoint keys never derive from the imported source identity");
    assert_eq!(refusal, DeviceActivationRefusal::DestinationIsSource);
    assert_eq!(refusal.code(), "destination_is_source");
    assert_eq!(
        store.lookup(SUBJECT, "device-a").expect("lookup runs"),
        DeviceActivationLookup::EmptyLedger,
        "a refused activation writes nothing at all"
    );
}

#[test]
fn the_epoch_only_advances_and_a_fork_is_refused() {
    let mut store = store();
    store
        .apply(&activate("device-new", Some("device-old"), 1, None))
        .expect("activation writes");
    store
        .apply(&activate("device-newer", Some("device-old"), 2, Some(1)))
        .expect("the successor writes");

    // An older epoch is stale, whatever the content.
    assert_eq!(
        store
            .apply(&activate("device-newest", Some("device-old"), 1, Some(2)))
            .expect_err("an older epoch is stale"),
        DeviceActivationRefusal::StaleEpoch
    );

    // The epoch the caller expected is no longer the one the ledger stands at.
    assert_eq!(
        store
            .apply(&activate("device-newest", Some("device-old"), 3, Some(0)))
            .expect_err("a write through a stale expected epoch is refused"),
        DeviceActivationRefusal::ConcurrentWrite
    );

    // The same epoch with different content is a fork, not a second activation.
    assert_eq!(
        store
            .apply(&activate("device-newest", Some("device-old"), 2, Some(2)))
            .expect_err("a same-epoch fork is refused"),
        DeviceActivationRefusal::EpochFork
    );

    assert_eq!(store.current_epoch(SUBJECT).expect("epoch reads"), Some(2));
    assert_eq!(store.records(SUBJECT).expect("records read").len(), 2);
}

#[test]
fn a_revocation_records_a_cleanup_as_an_intent_and_keeps_the_state() {
    let mut store = store();
    store
        .apply(&activate("device-new", Some("device-old"), 1, None))
        .expect("activation writes");

    let mut revoke = request(
        "device-old",
        None,
        DeviceActivationState::Revoked,
        2,
        Some(1),
        Some(1),
    );
    revoke.cleanup = DeviceCleanupIntent::requested();
    let written = store.apply(&revoke).expect("the revocation writes");

    assert!(written.is_revoked());
    assert!(!written.admits_new_sessions());
    assert!(written.cleanup.requested);
    assert_eq!(written.cleanup.completed_at, None);
    assert!(
        written.cleanup.is_outstanding(),
        "a revocation records the cleanup as owed, not as done"
    );

    // The revocation is separate from the activation it superseded: the
    // destination still stands, and the subject's epoch advanced once.
    let activation = store
        .lookup(SUBJECT, "device-new")
        .expect("lookup runs")
        .record()
        .cloned()
        .expect("the activation stands");
    assert!(activation.admits_new_sessions());
    assert_eq!(activation.position.accepted_epoch, 1);
    assert_eq!(store.current_epoch(SUBJECT).expect("epoch reads"), Some(2));

    // The projection reports the intent without reporting the signed material.
    let projected = written.to_json();
    assert_eq!(projected["state"], json!("revoked"));
    assert_eq!(projected["cleanup"]["requested"], json!(true));
    assert_eq!(projected["cleanup"]["outstanding"], json!(true));
    assert!(
        projected.get("authorityState").is_none(),
        "a projection never re-exports an authority record"
    );
}

#[test]
fn a_record_survives_a_reopen_and_an_incompatible_generation_resets() {
    let path = std::env::temp_dir().join(format!(
        "lico-device-activation-{}-{}.sqlite3",
        std::process::id(),
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    let written = {
        let mut store = SecureMeshDeviceActivationStore::open(&path).expect("ledger opens");
        store
            .apply(&activate("device-a", None, 1, None))
            .expect("activation writes")
    };
    {
        let store = SecureMeshDeviceActivationStore::open(&path).expect("ledger reopens");
        let record = store
            .lookup(SUBJECT, "device-a")
            .expect("lookup runs")
            .record()
            .cloned()
            .expect("the record survived the reopen");
        assert_eq!(record, written);
        assert_eq!(record.predecessor_state(), &written.authority_state);
    }

    // A file another generation wrote is reset rather than migrated: the record
    // is a cache of a decision the authority can re-derive, so a reset loses no
    // authority and keeps one current format.
    {
        let connection = rusqlite::Connection::open(&path).expect("raw connection opens");
        connection
            .execute_batch("PRAGMA user_version = 99;")
            .expect("version writes");
    }
    {
        let store = SecureMeshDeviceActivationStore::open(&path).expect("ledger reopens");
        assert_eq!(
            store.lookup(SUBJECT, "device-a").expect("lookup runs"),
            DeviceActivationLookup::EmptyLedger,
            "an incompatible generation is reset, not read"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_ledger_path_sits_beside_the_other_secure_mesh_state() {
    let path = device_activation_ledger_path(std::path::Path::new("/tmp/root"));
    assert!(path.starts_with("/tmp/root"));
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("device-activation.sqlite3")
    );
}

#[test]
fn each_subject_keeps_its_own_ledger() {
    let mut store = store();
    store
        .apply(&activate("device-a", None, 1, None))
        .expect("subject-a writes");

    let mut other = activate("device-a", None, 1, None);
    other.subject_identity_ref = "subject-b".to_owned();
    store
        .apply(&other)
        .expect("subject-b writes its own ledger");

    // The same destination at the same epoch under another subject is another
    // ledger: it is found there and absent here.
    assert!(
        store
            .lookup("subject-b", "device-a")
            .expect("lookup runs")
            .is_present()
    );
    assert_eq!(store.records("subject-b").expect("records read").len(), 1);
    assert_eq!(store.records("subject-a").expect("records read").len(), 1);
}

#[test]
fn the_ledger_refuses_to_grow_past_its_bound() {
    let mut store = store();
    for index in 0..MAX_SECURE_MESH_DEVICE_ACTIVATIONS {
        let endpoint = format!("device-{index:04}");
        let epoch = index as u64 + 1;
        let expected = if index == 0 { None } else { Some(epoch - 1) };
        store
            .apply(&activate(&endpoint, None, epoch, expected))
            .unwrap_or_else(|refusal| panic!("activation {index} refused: {refusal}"));
    }
    assert_eq!(
        store.records(SUBJECT).expect("records read").len(),
        MAX_SECURE_MESH_DEVICE_ACTIVATIONS
    );
    let refusal = store
        .apply(&activate("device-overflow", None, 999, Some(256)))
        .expect_err("the ledger is at its bound");
    assert_eq!(refusal, DeviceActivationRefusal::BoundExceeded);
    assert_eq!(refusal.code(), "bound_exceeded");
}

#[test]
fn every_refusal_names_a_stable_code_and_writes_nothing() {
    for refusal in [
        DeviceActivationRefusal::DestinationAlreadyActive,
        DeviceActivationRefusal::DestinationIsSource,
        DeviceActivationRefusal::StaleEpoch,
        DeviceActivationRefusal::EpochFork,
        DeviceActivationRefusal::ConcurrentWrite,
        DeviceActivationRefusal::UnreadableRecord,
        DeviceActivationRefusal::BoundExceeded,
    ] {
        assert!(!refusal.code().is_empty(), "{refusal:?} names no code");
        assert!(!refusal.reason().is_empty(), "{refusal:?} names no reason");
        assert!(refusal.wrote_nothing());
        assert!(refusal.to_string().starts_with(refusal.code()));
    }
}

#[test]
fn a_record_that_is_not_readable_in_this_shape_is_refused() {
    let mut store = store();
    let mut empty_destination = activate("device-a", None, 1, None);
    empty_destination.endpoint_identity_ref = "   ".to_owned();
    assert_eq!(
        store
            .apply(&empty_destination)
            .expect_err("an empty destination is not readable"),
        DeviceActivationRefusal::UnreadableRecord
    );

    let mut bad_genesis = activate("device-a", None, 0, None);
    bad_genesis.position.superseded_epoch = Some(1);
    assert_eq!(
        store
            .apply(&bad_genesis)
            .expect_err("a genesis record supersedes nothing"),
        DeviceActivationRefusal::UnreadableRecord
    );
}
