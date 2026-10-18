//! Contract C07 tests for the observation port.
//!
//! The capturing backend below is test-only and bounded: the port ships no
//! in-memory history store, so the crate keeps no second database to keep honest.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::backend::{
    DisabledObservationTelemetryBackend, LogObservationTelemetryBackend,
    ObservationTelemetryBackend,
};
use super::correlation::{CorrelationField, CorrelationIds};
use super::privacy::{PrivacyBudget, PrivacyViolation, PrivacyViolationKind};
use super::probe::{
    ObservationDropCounts, ObservationDropReason, ObservationProbe, ObservationProbeConfig,
    ObservationSubmitOutcome, SamplingBudget,
};
use super::segment::{
    ObservationLinkRelation, ObservationPhase, ObservationSegmentKind, ObservationSegmentRecord,
};

/// Bounded, test-only backend.
#[derive(Default)]
struct CapturingBackend {
    emitted: AtomicUsize,
    records: Mutex<Vec<ObservationSegmentRecord>>,
    /// A probe this backend submits to while draining, to prove drain runs
    /// outside the probe lock.
    reentrant: Mutex<Option<ObservationProbe>>,
}

impl CapturingBackend {
    fn emitted(&self) -> usize {
        self.emitted.load(Ordering::SeqCst)
    }

    fn take(&self) -> Vec<ObservationSegmentRecord> {
        std::mem::take(&mut *self.records.lock().expect("records"))
    }

    fn install_reentrant(&self, probe: ObservationProbe) {
        *self.reentrant.lock().expect("reentrant") = Some(probe);
    }
}

impl ObservationTelemetryBackend for CapturingBackend {
    fn emit(&self, record: &ObservationSegmentRecord) {
        self.emitted.fetch_add(1, Ordering::SeqCst);
        self.records.lock().expect("records").push(record.clone());
        let reentrant = self
            .reentrant
            .lock()
            .expect("reentrant")
            .as_ref()
            .map(ObservationProbe::clone);
        if let Some(probe) = reentrant {
            let segment = probe.begin(
                ObservationPhase::QueueWait,
                ids(&[(CorrelationField::RunId, "run-reentrant")]),
            );
            if let Some(segment) = segment {
                assert_eq!(probe.complete(segment), ObservationSubmitOutcome::Queued);
            }
        }
    }
}

struct Harness {
    backend: Arc<CapturingBackend>,
    probe: ObservationProbe,
    now: Arc<AtomicU64>,
    clock_reads: Arc<AtomicUsize>,
}

fn harness(config: ObservationProbeConfig) -> Harness {
    let backend = Arc::new(CapturingBackend::default());
    let now = Arc::new(AtomicU64::new(0));
    let clock_reads = Arc::new(AtomicUsize::new(0));
    let clock_now = Arc::clone(&now);
    let clock_reads_writer = Arc::clone(&clock_reads);
    let probe = ObservationProbe::bounded(
        config,
        Arc::new(move || {
            clock_reads_writer.fetch_add(1, Ordering::SeqCst);
            clock_now.load(Ordering::SeqCst)
        }),
        Arc::clone(&backend) as Arc<dyn ObservationTelemetryBackend>,
    );
    Harness {
        backend,
        probe,
        now,
        clock_reads,
    }
}

fn ids(pairs: &[(CorrelationField, &str)]) -> CorrelationIds {
    pairs
        .iter()
        .fold(CorrelationIds::default(), |ids, (field, value)| {
            ids.with(*field, *value)
        })
}

#[test]
fn disabled_probe_opens_nothing_and_counts_nothing() {
    let probe = ObservationProbe::disabled();

    assert!(
        probe
            .begin(ObservationPhase::AdmissionWait, CorrelationIds::default())
            .is_none(),
        "a switched-off probe reads no clock and opens no segment"
    );
    assert!(!probe.is_enabled());
    assert_eq!(probe.pending(), 0);
    assert_eq!(probe.drop_counts(), ObservationDropCounts::default());
    assert_eq!(probe.drain(8), 0);
    assert_eq!(
        probe.submit(ObservationSegmentRecord::new(
            ObservationPhase::Build,
            CorrelationIds::default(),
            0,
            1,
        )),
        ObservationSubmitOutcome::Disabled,
        "a switched-off probe refuses records without inventing a drop"
    );
    assert_eq!(probe.drop_counts(), ObservationDropCounts::default());
    assert_eq!(probe.pending(), 0);
    assert_eq!(probe.drain(8), 0);
}

#[test]
fn enabled_probe_reads_its_clock_once_per_segment_edge() {
    let timed = harness(ObservationProbeConfig::bounded(4));
    timed.now.store(50, Ordering::SeqCst);
    let opened = timed
        .probe
        .begin(
            ObservationPhase::Build,
            ids(&[(CorrelationField::RequestId, "req-clock")]),
        )
        .expect("enabled probe opens a segment");
    assert_eq!(timed.clock_reads.load(Ordering::SeqCst), 1);
    let record = opened.finish();
    assert_eq!(timed.clock_reads.load(Ordering::SeqCst), 2);
    assert_eq!(record.started_micros, 50);
    assert_eq!(record.duration_micros, 0);
}

#[test]
fn disabled_completion_does_not_read_an_enabled_segments_clock() {
    let timed = harness(ObservationProbeConfig::bounded(1));
    let segment = timed
        .probe
        .begin(ObservationPhase::Build, CorrelationIds::default())
        .expect("segment");
    assert_eq!(timed.clock_reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        ObservationProbe::disabled().complete(segment),
        ObservationSubmitOutcome::Disabled
    );
    assert_eq!(timed.clock_reads.load(Ordering::SeqCst), 1);
    assert_eq!(timed.probe.pending(), 0);
    assert_eq!(timed.backend.emitted(), 0);
}

#[test]
fn bounded_buffer_counts_drops_and_keeps_business_moving() {
    let buffer = harness(ObservationProbeConfig::bounded(2));
    for index in 0..3u64 {
        buffer.now.store(index, Ordering::SeqCst);
        let segment = buffer
            .probe
            .begin(
                ObservationPhase::Build,
                ids(&[(CorrelationField::RequestId, "req-1")]),
            )
            .expect("enabled probe opens a segment");
        let expected = if index < 2 {
            ObservationSubmitOutcome::Queued
        } else {
            ObservationSubmitOutcome::Dropped(ObservationDropReason::BufferFull)
        };
        assert_eq!(buffer.probe.complete(segment), expected, "record {index}");
    }
    let drops = buffer.probe.drop_counts();
    assert_eq!(drops.buffer_full, 1);
    assert_eq!(drops.total(), 1);
    assert_eq!(buffer.probe.pending(), 2);
    assert_eq!(buffer.probe.drain(8), 2);
    assert_eq!(buffer.probe.pending(), 0);
    assert_eq!(buffer.backend.emitted(), 2);
}

#[test]
fn sampling_budget_resets_per_window_and_counts_the_refusal() {
    let sampled = harness(ObservationProbeConfig {
        buffer_capacity: 8,
        sampling: SamplingBudget::per_window(1, 1_000),
        privacy: PrivacyBudget::STANDARD,
    });
    let run = ids(&[(CorrelationField::RunId, "run-sampled")]);
    let record = |duration| {
        ObservationSegmentRecord::new(ObservationPhase::QueueWait, run.clone(), 0, duration)
    };

    assert_eq!(
        sampled.probe.submit(record(10)),
        ObservationSubmitOutcome::Queued
    );
    assert_eq!(
        sampled.probe.submit(record(11)),
        ObservationSubmitOutcome::Dropped(ObservationDropReason::SamplingBudgetExhausted)
    );
    assert_eq!(sampled.probe.drop_counts().sampling_budget_exhausted, 1);

    sampled.now.store(999, Ordering::SeqCst);
    assert_eq!(
        sampled.probe.submit(record(12)),
        ObservationSubmitOutcome::Dropped(ObservationDropReason::SamplingBudgetExhausted),
        "the window has not elapsed yet"
    );

    sampled.now.store(1_000, Ordering::SeqCst);
    assert_eq!(
        sampled.probe.submit(record(13)),
        ObservationSubmitOutcome::Queued,
        "the next window samples again"
    );
    assert_eq!(sampled.probe.drain(8), 2);
}

#[test]
fn privacy_budget_refuses_content_secrets_and_private_paths() {
    let private = harness(ObservationProbeConfig::bounded(8));
    // Home-anchored and drive-qualified paths are assembled from parts: this
    // file proves they are refused, and must not itself carry a
    // machine-specific path literal.
    let absolute_home = |tail: &str| ["", "Users", "private-owner", tail].join("/");
    let home_anchored = ["~", "private", "effect"].join("/");
    let drive_qualified = ["C:", "private", "run"].join("\\");
    assert!(home_anchored.starts_with('~'));
    assert!(drive_qualified.starts_with("C:") && drive_qualified.contains('\\'));
    let cases = [
        (
            CorrelationField::ConversationId,
            "line one\nline two".to_owned(),
            PrivacyViolationKind::Unprintable,
        ),
        (
            CorrelationField::RequestId,
            absolute_home("secrets.txt"),
            PrivacyViolationKind::PrivatePath,
        ),
        (
            CorrelationField::EffectId,
            home_anchored,
            PrivacyViolationKind::PrivatePath,
        ),
        (
            CorrelationField::RunId,
            drive_qualified,
            PrivacyViolationKind::PrivatePath,
        ),
        (
            CorrelationField::AttemptToken,
            concat!(
                "attempt-token-",
                "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            )
            .to_owned(),
            PrivacyViolationKind::Oversized,
        ),
    ];
    for (field, value, kind) in cases {
        private.now.fetch_add(1, Ordering::SeqCst);
        let outcome = private
            .probe
            .begin(
                ObservationPhase::PrepareCpu,
                ids(&[(field, value.as_str())]),
            )
            .map(|segment| private.probe.complete(segment));
        assert_eq!(
            outcome,
            Some(ObservationSubmitOutcome::Dropped(
                ObservationDropReason::PrivacyBudgetExceeded
            )),
            "{field:?} must be refused"
        );
        assert_eq!(
            private.probe.drop_counts().last_privacy_violation,
            Some(PrivacyViolation::Id { field, kind })
        );
    }
    assert_eq!(private.probe.drop_counts().privacy_budget_exceeded, 5);
    assert_eq!(private.probe.pending(), 0);
    assert_eq!(private.probe.drain(8), 0);
    assert_eq!(
        private.backend.emitted(),
        0,
        "a refused value never reaches a backend"
    );
    assert_eq!(
        private
            .probe
            .begin(
                ObservationPhase::PrepareCpu,
                ids(&[
                    (CorrelationField::SourcePosition, "crates/x.rs:12"),
                    (CorrelationField::PrepareId, "prepare-7"),
                ]),
            )
            .map(|segment| private.probe.complete(segment)),
        Some(ObservationSubmitOutcome::Queued),
        "repository-relative positions and ordinary ids stay acceptable"
    );
}

#[test]
fn privacy_budget_bounds_span_links() {
    let links = harness(ObservationProbeConfig {
        buffer_capacity: 4,
        sampling: SamplingBudget::UNLIMITED,
        privacy: PrivacyBudget {
            max_id_bytes: 128,
            max_links: 2,
        },
    });
    let record = ObservationSegmentRecord::new(
        ObservationPhase::DatabaseTransaction,
        ids(&[(CorrelationField::RunId, "run-links")]),
        0,
        4,
    )
    .with_link(
        ids(&[(CorrelationField::NodeVisit, "node-a/1")]),
        ObservationLinkRelation::Predecessor,
    )
    .with_link(
        ids(&[(CorrelationField::NodeVisit, "node-b/1")]),
        ObservationLinkRelation::Predecessor,
    );
    assert_eq!(
        links.probe.submit(record.clone()),
        ObservationSubmitOutcome::Queued
    );
    assert_eq!(
        links.probe.submit(record.with_link(
            ids(&[(CorrelationField::NodeVisit, "node-c/1")]),
            ObservationLinkRelation::Predecessor,
        )),
        ObservationSubmitOutcome::Dropped(ObservationDropReason::PrivacyBudgetExceeded)
    );
    assert_eq!(
        links.probe.drop_counts().last_privacy_violation,
        Some(PrivacyViolation::TooManyLinks)
    );
}

#[test]
fn parallel_predecessors_and_queue_producers_use_links_not_a_call_stack() {
    let join = harness(ObservationProbeConfig::bounded(4));
    join.now.store(100, Ordering::SeqCst);
    let mut segment = join
        .probe
        .begin(
            ObservationPhase::DatabaseTransaction,
            ids(&[
                (CorrelationField::RunId, "run-join"),
                (CorrelationField::NodeVisit, "join/1"),
            ]),
        )
        .expect("enabled probe opens a segment");
    assert_eq!(segment.phase(), ObservationPhase::DatabaseTransaction);
    assert_eq!(
        segment.correlation().get(CorrelationField::RunId),
        Some("run-join")
    );
    segment.add_link(
        ids(&[(CorrelationField::NodeVisit, "left/1")]),
        ObservationLinkRelation::Predecessor,
    );
    segment.add_link(
        ids(&[(CorrelationField::NodeVisit, "right/1")]),
        ObservationLinkRelation::Predecessor,
    );
    segment.add_link(
        ids(&[(CorrelationField::EffectId, "effect-1")]),
        ObservationLinkRelation::QueueProducer,
    );
    join.now.store(140, Ordering::SeqCst);
    assert_eq!(
        join.probe.complete(segment),
        ObservationSubmitOutcome::Queued
    );

    assert_eq!(join.probe.drain(4), 1);
    let records = join.backend.take();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!((record.started_micros, record.duration_micros), (100, 40));
    assert_eq!(
        record
            .links
            .iter()
            .map(|link| link.relation)
            .collect::<Vec<_>>(),
        vec![
            ObservationLinkRelation::Predecessor,
            ObservationLinkRelation::Predecessor,
            ObservationLinkRelation::QueueProducer,
        ],
        "links state dependency direction without inventing a synchronous stack"
    );
    assert_ne!(
        record.links[0].correlation.get(CorrelationField::NodeVisit),
        record.links[1].correlation.get(CorrelationField::NodeVisit),
        "each parallel predecessor keeps its own identity"
    );
    assert_eq!(
        record.correlation.get(CorrelationField::NodeVisit),
        Some("join/1"),
        "the join keeps its own context"
    );
}

#[test]
fn drain_runs_the_backend_outside_the_probe_lock() {
    let reentrant = harness(ObservationProbeConfig::bounded(4));
    reentrant.backend.install_reentrant(reentrant.probe.clone());
    reentrant.now.store(1, Ordering::SeqCst);
    let segment = reentrant
        .probe
        .begin(
            ObservationPhase::AdapterFirstEvent,
            ids(&[(CorrelationField::RunId, "run-drain")]),
        )
        .expect("segment");
    assert_eq!(
        reentrant.probe.complete(segment),
        ObservationSubmitOutcome::Queued
    );
    assert_eq!(
        reentrant.probe.drain(1),
        1,
        "a backend that submits re-entrantly must not deadlock the drain"
    );
    assert_eq!(reentrant.probe.pending(), 1);
    assert_eq!(reentrant.probe.drain(1), 1);
    assert_eq!(reentrant.backend.emitted(), 2);
}

#[test]
fn probe_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ObservationProbe>();
    let shared = harness(ObservationProbeConfig::bounded(64));
    let probe = shared.probe.clone();
    std::thread::scope(|scope| {
        for index in 0..4u64 {
            let probe = probe.clone();
            scope.spawn(move || {
                let record = ObservationSegmentRecord::new(
                    ObservationPhase::PrepareQueue,
                    ids(&[(CorrelationField::RunId, "run-parallel")]),
                    index,
                    index + 1,
                );
                assert!(matches!(
                    probe.submit(record),
                    ObservationSubmitOutcome::Queued
                        | ObservationSubmitOutcome::Dropped(ObservationDropReason::BufferContended)
                ));
            });
        }
    });
    assert_eq!(
        probe.pending() as u64 + probe.drop_counts().buffer_contended,
        4
    );
    assert_eq!(probe.drop_counts().buffer_full, 0);
    assert_eq!(probe.drop_counts().privacy_budget_exceeded, 0);
    assert_eq!(probe.drop_counts().sampling_budget_exhausted, 0);
}

#[test]
fn correlation_ids_round_trip_the_contract_wire_shape() {
    let fixture = r#"{
      "userInteractionId": "interaction-1",
      "requestId": "request-1",
      "conversationId": "conversation-1",
      "runId": "run-1",
      "nodeVisit": "node-1/2",
      "effectId": "effect-1",
      "attemptToken": "attempt-1",
      "noticeId": "notice-1",
      "sourcePosition": "crates/x.rs:12",
      "prepareId": "prepare-1"
    }"#;
    let ids: CorrelationIds = serde_json::from_str(fixture).expect("wire shape");
    assert_eq!(CorrelationField::ALL.len(), 10);
    for field in CorrelationField::ALL {
        assert!(ids.get(field).is_some(), "{field:?} must survive the wire");
    }
    assert_eq!(
        serde_json::to_value(&ids).expect("serialize"),
        serde_json::from_str::<serde_json::Value>(fixture).expect("fixture")
    );
    assert_eq!(
        ids.iter().map(|(field, _)| field).collect::<Vec<_>>(),
        CorrelationField::ALL.to_vec()
    );

    let sparse = CorrelationIds::default().with(CorrelationField::RunId, "run-only");
    assert!(!sparse.is_empty());
    assert_eq!(
        serde_json::to_value(&sparse).expect("serialize"),
        serde_json::json!({ "runId": "run-only" }),
        "absent ids stay absent rather than becoming placeholders"
    );
    assert!(CorrelationIds::default().is_empty());
    assert_eq!(CorrelationIds::default().to_string(), "-");
}

#[test]
fn segment_records_round_trip_with_links() {
    let record = ObservationSegmentRecord::new(
        ObservationPhase::Raster,
        ids(&[(CorrelationField::ConversationId, "conversation-1")]),
        7,
        13,
    )
    .with_link(
        ids(&[(CorrelationField::RunId, "run-predecessor")]),
        ObservationLinkRelation::Predecessor,
    );
    let encoded = serde_json::to_value(&record).expect("serialize");
    assert_eq!(encoded["phase"], serde_json::json!("raster"));
    assert_eq!(encoded["correlation"]["conversationId"], "conversation-1");
    assert_eq!(encoded["startedMicros"], 7);
    assert_eq!(encoded["durationMicros"], 13);
    assert_eq!(encoded["links"][0]["relation"], "predecessor");
    assert_eq!(
        encoded["links"][0]["correlation"]["runId"],
        "run-predecessor"
    );
    let decoded: ObservationSegmentRecord = serde_json::from_value(encoded).expect("deserialize");
    assert_eq!(decoded, record);
    assert_eq!(decoded.kind(), ObservationSegmentKind::Work);
}

#[test]
fn every_contract_phase_maps_to_waiting_or_work() {
    let mut waits = 0;
    let mut works = 0;
    for phase in ObservationPhase::ALL {
        assert!(!phase.wire().is_empty());
        assert!(!phase.kind().wire().is_empty());
        match phase.kind() {
            ObservationSegmentKind::Wait => waits += 1,
            ObservationSegmentKind::Work => works += 1,
        }
    }
    assert_eq!(ObservationPhase::ALL.len(), 9);
    assert_eq!((waits, works), (5, 4));
    assert_eq!(
        ObservationPhase::AdmissionWait.kind(),
        ObservationSegmentKind::Wait
    );
    assert_eq!(
        ObservationPhase::AdapterFirstEvent.kind(),
        ObservationSegmentKind::Wait
    );
    assert_eq!(
        ObservationPhase::PrepareCpu.kind(),
        ObservationSegmentKind::Work
    );
    assert_eq!(
        ObservationPhase::InputDisplay.kind(),
        ObservationSegmentKind::Wait,
        "perceived display latency is waiting; inner work has its own segments"
    );
}

#[test]
fn correlation_ids_render_for_the_existing_log_sink() {
    let rendered = ids(&[
        (CorrelationField::RunId, "run-1"),
        (CorrelationField::EffectId, "effect-1"),
    ]);
    assert_eq!(rendered.to_string(), "runId=run-1 effectId=effect-1");
}

#[test]
fn log_backend_reuses_the_existing_log_sink_line_shape() {
    let record = ObservationSegmentRecord::new(
        ObservationPhase::Build,
        ids(&[(CorrelationField::RunId, "run-log")]),
        4,
        3,
    )
    .with_link(
        ids(&[(CorrelationField::RunId, "run-predecessor")]),
        ObservationLinkRelation::Predecessor,
    );
    let line = LogObservationTelemetryBackend::render(&record);
    for expected in [
        "phase=build",
        "kind=work",
        "started_us=4",
        "duration_us=3",
        "links=1",
        "runId=run-log",
    ] {
        assert!(line.contains(expected), "{expected} missing from {line}");
    }
    let (_, links) = line.split_once(" span_links=").expect("causal links");
    let links: serde_json::Value = serde_json::from_str(links).expect("structured links");
    assert_eq!(links[0]["correlation"]["runId"], "run-predecessor");
    assert_eq!(links[0]["relation"], "predecessor");

    // The disabled backend and the log backend both absorb a record without
    // opening a sink of their own.
    DisabledObservationTelemetryBackend.emit(&record);
    LogObservationTelemetryBackend.emit(&record);
}

#[test]
fn linked_contexts_have_the_same_privacy_budget_as_the_segment() {
    let private = harness(ObservationProbeConfig::bounded(1));
    for field in CorrelationField::ALL {
        for value in [
            "line one\nline two".to_owned(),
            ["~", "private", "key"].join("/"),
            "x".repeat(129),
        ] {
            let record = ObservationSegmentRecord::new(
                ObservationPhase::QueueWait,
                ids(&[(CorrelationField::RunId, "run-safe")]),
                0,
                1,
            )
            .with_link(
                ids(&[(field, &value)]),
                ObservationLinkRelation::QueueProducer,
            );
            assert_eq!(
                private.probe.submit(record),
                ObservationSubmitOutcome::Dropped(ObservationDropReason::PrivacyBudgetExceeded),
                "linked {field:?} must be validated before buffering"
            );
        }
    }
    assert_eq!(private.probe.drop_counts().privacy_budget_exceeded, 30);
    assert_eq!(private.probe.pending(), 0);
    assert_eq!(private.probe.drain(1), 0);
    assert_eq!(private.backend.emitted(), 0);
}

#[test]
fn direct_log_backend_calls_cannot_bypass_privacy_validation() {
    let private_id = ["~", "private", "synthetic-key"].join("/");
    let record = ObservationSegmentRecord::new(
        ObservationPhase::QueueWait,
        ids(&[(CorrelationField::RequestId, &private_id)]),
        0,
        1,
    );
    assert!(!LogObservationTelemetryBackend::render(&record).contains(&private_id));
    let linked =
        ObservationSegmentRecord::new(ObservationPhase::QueueWait, CorrelationIds::default(), 0, 1)
            .with_link(record.correlation, ObservationLinkRelation::QueueProducer);
    assert!(!LogObservationTelemetryBackend::render(&linked).contains(&private_id));
}

#[test]
#[should_panic(expected = "observation buffer capacity must be positive")]
fn zero_capacity_is_a_configuration_error() {
    let source: Arc<dyn ObservationTelemetryBackend> =
        Arc::new(DisabledObservationTelemetryBackend);
    ObservationProbe::bounded(ObservationProbeConfig::bounded(0), Arc::new(|| 0), source);
}

#[test]
#[should_panic(expected = "observation sampling window must be positive")]
fn zero_sampling_window_is_a_configuration_error() {
    let source: Arc<dyn ObservationTelemetryBackend> =
        Arc::new(DisabledObservationTelemetryBackend);
    ObservationProbe::bounded(
        ObservationProbeConfig {
            buffer_capacity: 1,
            sampling: SamplingBudget::per_window(1, 0),
            privacy: PrivacyBudget::STANDARD,
        },
        Arc::new(|| 0),
        source,
    );
}
