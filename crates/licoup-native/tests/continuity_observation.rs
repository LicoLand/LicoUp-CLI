use licoup_conversation::{ConversationStore, Principal, PrincipalKind};
use licoup_native::domain::assistant_continuity::{
    ObservationFact, ObservationFactKind, ObservationIndex, ObservationQuery,
    ObservationRecordOutcome, ObservationScope, ObservationState, ObservationStore,
};

fn scope(configuration_id: &str) -> ObservationScope {
    ObservationScope::new("responsibility:notes", "notes", configuration_id)
}

fn fact(
    id: &str,
    logical_key: &str,
    configuration_id: &str,
    revision: i64,
    observed_at: i64,
    kind: ObservationFactKind,
) -> ObservationFact {
    ObservationFact {
        fact_id: id.into(),
        conversation_id: "conversation:observation".into(),
        run_id: format!("run:{id}"),
        logical_key: logical_key.into(),
        scope: scope(configuration_id),
        membership_id: "membership:assistant".into(),
        runtime_version: "runtime:test-v1".into(),
        designation_epoch: 1,
        graph_visit: Some("visit:1".into()),
        revision,
        observed_at,
        kind,
        outcome: "observed".into(),
        supersedes: None,
        expires_at: None,
    }
}

fn store_with_conversation() -> (ConversationStore, String) {
    let store = ConversationStore::open_in_memory().unwrap();
    let conversation = store
        .create_conversation(
            "Observation test",
            Principal {
                id: "human:local".into(),
                kind: PrincipalKind::Human,
                display_name: "You".into(),
                agent_id: None,
                created_at_unix_ms: 1,
            },
        )
        .unwrap();
    (store, conversation.id)
}

#[test]
fn index_keeps_duplicates_stale_corrections_and_expiry_independently_testable() {
    let first = fact(
        "fact:first",
        "matter:notes",
        "config:v1",
        1,
        10,
        ObservationFactKind::Result,
    );
    let stale = fact(
        "fact:stale",
        "matter:notes",
        "config:v0",
        0,
        20,
        ObservationFactKind::Result,
    );
    let mut correction = fact(
        "fact:correction",
        "matter:notes",
        "config:v2",
        2,
        30,
        ObservationFactKind::Correction,
    );
    correction.supersedes = Some(first.fact_id.clone());
    let mut late_correction = fact(
        "fact:late-correction",
        "matter:notes",
        "config:v0",
        0,
        60,
        ObservationFactKind::Correction,
    );
    late_correction.supersedes = Some(correction.fact_id.clone());
    let mut expired = fact(
        "fact:expired",
        "matter:other",
        "config:v1",
        1,
        40,
        ObservationFactKind::Wait,
    );
    expired.expires_at = Some(50);

    let mut index = ObservationIndex::new();
    assert_eq!(
        index.insert(first.clone()).unwrap(),
        ObservationRecordOutcome::Inserted
    );
    assert_eq!(
        index.insert(first.clone()).unwrap(),
        ObservationRecordOutcome::Duplicate
    );
    assert_eq!(
        index.insert(stale.clone()).unwrap(),
        ObservationRecordOutcome::Inserted
    );
    assert_eq!(
        index.insert(correction.clone()).unwrap(),
        ObservationRecordOutcome::Inserted
    );
    assert_eq!(
        index.insert(late_correction.clone()).unwrap(),
        ObservationRecordOutcome::Inserted
    );
    assert_eq!(
        index.insert(expired.clone()).unwrap(),
        ObservationRecordOutcome::Inserted
    );

    assert!(index.is_stale(&stale.fact_id));
    assert!(index.is_stale(&late_correction.fact_id));
    assert_eq!(
        index.state_at(&first.fact_id, 45),
        Some(ObservationState::Superseded)
    );
    assert_eq!(
        index.state_at(&correction.fact_id, 45),
        Some(ObservationState::Current)
    );
    assert_eq!(
        index.state_at(&expired.fact_id, 50),
        Some(ObservationState::Expired)
    );
    assert_eq!(index.current_for(&scope("config:v2"), 45).len(), 1);
    assert_eq!(index.expired_at(&scope("config:v1"), 50).len(), 1);
}

#[test]
fn durable_observation_is_idempotent_indexed_and_available_after_store_rebind() {
    let (store, conversation_id) = store_with_conversation();
    let durable = ObservationStore::new(store.clone());
    let mut first = fact(
        "fact:first",
        "matter:notes",
        "config:v1",
        1,
        10,
        ObservationFactKind::Result,
    );
    first.conversation_id = conversation_id.clone();

    assert_eq!(
        durable.record(&first).unwrap(),
        ObservationRecordOutcome::Inserted
    );
    assert_eq!(
        durable.record(&first).unwrap(),
        ObservationRecordOutcome::Duplicate
    );

    let mut conflict = first.clone();
    conflict.outcome = "changed".into();
    let failure = durable.record(&conflict).unwrap_err();
    assert_eq!(
        failure.code,
        licoup_conversation::continuity::ContinuityFailureCode::IdempotencyConflict
    );

    let mut correction = first.clone();
    correction.fact_id = "fact:correction".into();
    correction.run_id = "run:correction".into();
    correction.scope = scope("config:v2");
    correction.revision = 2;
    correction.observed_at = 20;
    correction.kind = ObservationFactKind::Correction;
    correction.supersedes = Some(first.fact_id.clone());
    assert_eq!(
        durable.record(&correction).unwrap(),
        ObservationRecordOutcome::Inserted
    );

    let query = ObservationQuery {
        scope: Some(scope("config:v1")),
        after_fact_id: None,
        limit: 1,
    };
    let first_page = durable.page(&conversation_id, &query).unwrap();
    assert_eq!(first_page.observations.len(), 1);
    assert_eq!(first_page.observations[0].fact_id, first.fact_id);
    assert!(first_page.next_fact_id.is_none());

    let rebound = ObservationStore::new(store.clone());
    let index = rebound.index(&conversation_id).unwrap();
    assert_eq!(index.len(), 2);
    assert_eq!(
        rebound
            .load(&conversation_id, &correction.fact_id)
            .unwrap()
            .unwrap(),
        correction
    );
    assert_eq!(
        index.state_at(&first.fact_id, 20),
        Some(ObservationState::Superseded)
    );
}

#[test]
fn durable_paging_walks_scoped_and_unscoped_pages_without_repeats() {
    let (store, conversation_id) = store_with_conversation();
    let durable = ObservationStore::new(store.clone());
    let mut alpha = fact(
        "fact:alpha",
        "matter:notes",
        "config:v1",
        1,
        10,
        ObservationFactKind::Run,
    );
    alpha.conversation_id = conversation_id.clone();
    let mut beta = fact(
        "fact:beta",
        "matter:notes",
        "config:v1",
        2,
        20,
        ObservationFactKind::Result,
    );
    beta.conversation_id = conversation_id.clone();
    let mut gamma = fact(
        "fact:gamma",
        "matter:notes",
        "config:v2",
        1,
        30,
        ObservationFactKind::Run,
    );
    gamma.conversation_id = conversation_id.clone();
    for fact in [&alpha, &beta, &gamma] {
        assert_eq!(
            durable.record(fact).unwrap(),
            ObservationRecordOutcome::Inserted
        );
    }

    let unscoped = ObservationQuery {
        scope: None,
        after_fact_id: None,
        limit: 2,
    };
    let first_page = durable.page(&conversation_id, &unscoped).unwrap();
    assert_eq!(first_page.observations.len(), 2);
    let next = first_page
        .next_fact_id
        .clone()
        .expect("second unscoped page must exist");
    let second_page = durable
        .page(
            &conversation_id,
            &ObservationQuery {
                after_fact_id: Some(next),
                ..unscoped.clone()
            },
        )
        .unwrap();
    assert_eq!(second_page.observations.len(), 1);
    assert!(second_page.next_fact_id.is_none());
    let walked: Vec<String> = first_page
        .observations
        .iter()
        .chain(second_page.observations.iter())
        .map(|fact| fact.fact_id.clone())
        .collect();
    assert_eq!(
        walked,
        vec![
            "fact:alpha".to_string(),
            "fact:beta".to_string(),
            "fact:gamma".to_string()
        ]
    );

    let scoped = ObservationQuery {
        scope: Some(scope("config:v1")),
        after_fact_id: None,
        limit: 1,
    };
    let scoped_first = durable.page(&conversation_id, &scoped).unwrap();
    assert_eq!(scoped_first.observations.len(), 1);
    assert_eq!(scoped_first.observations[0].fact_id, alpha.fact_id);
    let scoped_next = scoped_first
        .next_fact_id
        .clone()
        .expect("second scoped page must exist");
    let scoped_second = durable
        .page(
            &conversation_id,
            &ObservationQuery {
                after_fact_id: Some(scoped_next),
                ..scoped.clone()
            },
        )
        .unwrap();
    assert_eq!(scoped_second.observations.len(), 1);
    assert_eq!(scoped_second.observations[0].fact_id, beta.fact_id);
    assert!(scoped_second.next_fact_id.is_none());
}

#[test]
fn repeat_facts_are_distinct_from_duplicate_delivery() {
    let (store, conversation_id) = store_with_conversation();
    let durable = ObservationStore::new(store.clone());
    let mut run = fact(
        "fact:run",
        "matter:notes",
        "config:v1",
        1,
        10,
        ObservationFactKind::Run,
    );
    run.conversation_id = conversation_id.clone();
    let mut repeat = fact(
        "fact:repeat",
        "matter:notes",
        "config:v1",
        1,
        40,
        ObservationFactKind::Repeat,
    );
    repeat.conversation_id = conversation_id.clone();

    assert_eq!(
        durable.record(&run).unwrap(),
        ObservationRecordOutcome::Inserted
    );
    assert_eq!(
        durable.record(&run).unwrap(),
        ObservationRecordOutcome::Duplicate
    );
    assert_eq!(
        durable.record(&repeat).unwrap(),
        ObservationRecordOutcome::Inserted
    );

    assert!(
        durable
            .load(&conversation_id, "fact:missing")
            .unwrap()
            .is_none()
    );
    let index = durable.index(&conversation_id).unwrap();
    assert_eq!(index.len(), 2);
    let latest = index
        .latest_for("responsibility:notes", "notes", "matter:notes")
        .unwrap();
    assert_eq!(latest.fact_id, repeat.fact_id);
    assert_eq!(latest.kind, ObservationFactKind::Repeat);
    assert_eq!(index.current_for(&scope("config:v1"), 50).len(), 2);

    let mut new_epoch = fact(
        "fact:new-epoch",
        "matter:notes",
        "config:v3",
        1,
        50,
        ObservationFactKind::Run,
    );
    new_epoch.conversation_id = conversation_id.clone();
    new_epoch.designation_epoch = 2;
    assert_eq!(
        durable.record(&new_epoch).unwrap(),
        ObservationRecordOutcome::Inserted
    );

    let index = durable.index(&conversation_id).unwrap();
    assert!(index.is_stale(&run.fact_id));
    assert!(index.is_stale(&repeat.fact_id));
    assert!(!index.is_stale(&new_epoch.fact_id));
    assert_eq!(
        index.state_at(&new_epoch.fact_id, 60),
        Some(ObservationState::Current)
    );
    assert_eq!(index.current_for(&scope("config:v1"), 60).len(), 0);
}

#[test]
fn record_and_query_reject_out_of_scope_or_malformed_requests() {
    let (store, conversation_id) = store_with_conversation();
    let durable = ObservationStore::new(store.clone());
    let mut orphan = fact(
        "fact:orphan",
        "matter:notes",
        "config:v1",
        1,
        10,
        ObservationFactKind::Run,
    );
    orphan.conversation_id = "conversation:missing".into();
    let failure = durable.record(&orphan).unwrap_err();
    assert_eq!(
        failure.code,
        licoup_conversation::continuity::ContinuityFailureCode::ScopeDenied
    );

    let mut blank_id = fact(
        "fact:blank",
        "matter:notes",
        "config:v1",
        1,
        10,
        ObservationFactKind::Run,
    );
    blank_id.conversation_id = conversation_id.clone();
    blank_id.fact_id = "  ".into();
    let failure = durable.record(&blank_id).unwrap_err();
    assert_eq!(
        failure.code,
        licoup_conversation::continuity::ContinuityFailureCode::InvalidRequest
    );

    for limit in [0, 1_001] {
        let failure = durable
            .page(
                &conversation_id,
                &ObservationQuery {
                    scope: None,
                    after_fact_id: None,
                    limit,
                },
            )
            .unwrap_err();
        assert_eq!(
            failure.code,
            licoup_conversation::continuity::ContinuityFailureCode::InvalidRequest
        );
    }
    let failure = durable
        .page(
            &conversation_id,
            &ObservationQuery {
                scope: None,
                after_fact_id: Some(" ".into()),
                limit: 1,
            },
        )
        .unwrap_err();
    assert_eq!(
        failure.code,
        licoup_conversation::continuity::ContinuityFailureCode::InvalidRequest
    );
    let failure = durable
        .page(
            &conversation_id,
            &ObservationQuery {
                scope: Some(ObservationScope::new(" ", "notes", "config:v1")),
                after_fact_id: None,
                limit: 1,
            },
        )
        .unwrap_err();
    assert_eq!(
        failure.code,
        licoup_conversation::continuity::ContinuityFailureCode::InvalidRequest
    );
}
