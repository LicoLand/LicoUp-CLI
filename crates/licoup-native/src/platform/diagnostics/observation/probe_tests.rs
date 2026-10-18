//! Synchronization tests keep private lock control out of the public port.

use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::platform::diagnostics::observation::backend::DisabledObservationTelemetryBackend;

fn record() -> ObservationSegmentRecord {
    ObservationSegmentRecord::new(ObservationPhase::QueueWait, CorrelationIds::default(), 0, 1)
}

#[test]
fn contention_drops_instead_of_waiting_for_the_buffer_lock() {
    let probe = ObservationProbe::bounded(
        ObservationProbeConfig::bounded(1),
        Arc::new(|| 0),
        Arc::new(DisabledObservationTelemetryBackend),
    );
    let inner = probe.inner.as_ref().expect("enabled");
    let guard = inner.lock();
    let (tx, rx) = mpsc::channel();
    let worker_probe = probe.clone();
    let worker = std::thread::spawn(move || {
        tx.send(worker_probe.submit(record()))
            .expect("report submission");
    });
    // Release before asserting, so a blocking regression fails rather than
    // stranding the worker or hanging the test process.
    let outcome = rx.recv_timeout(Duration::from_secs(2));
    drop(guard);
    worker.join().expect("producer");
    assert_eq!(
        outcome.expect("submission must finish while the buffer lock is held"),
        ObservationSubmitOutcome::Dropped(ObservationDropReason::BufferContended)
    );
    assert_eq!(probe.pending(), 0);
    assert_eq!(probe.drop_counts().buffer_contended, 1);
    assert_eq!(probe.drop_counts().total(), 1);
    assert_eq!(probe.submit(record()), ObservationSubmitOutcome::Queued);
}

struct PausedBackend {
    started: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl ObservationTelemetryBackend for PausedBackend {
    fn emit(&self, _record: &ObservationSegmentRecord) {
        self.started.send(()).expect("backend started");
        self.release
            .lock()
            .expect("release lock")
            .recv()
            .expect("release backend");
    }
}

#[test]
fn stalled_backend_and_full_buffer_do_not_stall_the_producer() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let probe = ObservationProbe::bounded(
        ObservationProbeConfig::bounded(1),
        Arc::new(|| 0),
        Arc::new(PausedBackend {
            started: started_tx,
            release: Mutex::new(release_rx),
        }),
    );
    assert_eq!(probe.submit(record()), ObservationSubmitOutcome::Queued);
    let drain_probe = probe.clone();
    let drain = std::thread::spawn(move || drain_probe.drain(1));
    let backend_started = started_rx.recv_timeout(Duration::from_secs(2));
    let (finished_tx, finished_rx) = mpsc::channel();
    let producer_probe = probe.clone();
    let producer = std::thread::spawn(move || {
        let first = producer_probe.submit(record());
        let second = producer_probe.submit(record());
        finished_tx
            .send((first, second))
            .expect("producer finished");
    });
    let outcomes = finished_rx.recv_timeout(Duration::from_secs(2));
    // Release and join even on failure. The timeout is a deadlock oracle, not a
    // performance target; synchronization, not sleeps, holds the backend open.
    release_tx.send(()).expect("release backend");
    producer.join().expect("producer");
    assert_eq!(drain.join().expect("drain"), 1);
    backend_started.expect("backend started");
    assert_eq!(
        outcomes.expect("producer must finish before backend release"),
        (
            ObservationSubmitOutcome::Queued,
            ObservationSubmitOutcome::Dropped(ObservationDropReason::BufferFull)
        )
    );
    assert_eq!(probe.pending(), 1);
    assert_eq!(probe.drop_counts().buffer_full, 1);
    assert_eq!(probe.drop_counts().total(), 1);
}
