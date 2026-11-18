//! The observation catalogue contract: every catalogued source has a
//! production consumer, and every consumer this crate ships is catalogued.
//!
//! The expectations are written here, from the consumers this crate ships, and
//! never read from the catalogue itself. That is what makes the check
//! falsifiable in both directions:
//!
//! * Each probe drives a *production* consumer with a synthetic observation and
//!   shows the observation arriving in the consumer's own output. A consumer
//!   that stopped reading its port fails its probe.
//! * [`probes`] is the independent list of sources. The catalogue must carry
//!   exactly those ids, so a source dropped from `SOURCES` leaves a probe with
//!   no catalogue entry and fails rather than silently losing its consumer.
//! * Each entry's `consumer` names a production module that mentions the entry's
//!   port type outside its test module, so an entry cannot name a consumer that
//!   never reads the port.
//!
//! What is synthetic, stated plainly: the rollout file, the run id and the node
//! observation are fixtures. Nothing scans a real Codex process, no run is
//! executed, and no ledger is read.

use super::*;
use crate::domain::conversation::history::{CodexRolloutPort, CodexRuntimeObservation};
use crate::domain::workflow_runtime::evolution::{
    CallbackEvolutionEnricher, DefaultEvolutionObservationPort, NodeObservationSummary,
};
use licoup_workflow::{PendingCallback, RunSnapshot, TransitionEvent};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The sources this contract drives, one probe each.
///
/// The list is deliberately independent of [`SOURCES`]: it is written from the
/// consumers this crate ships. Adding a consumer means adding its probe and its
/// catalogue entry together; dropping an entry breaks the pairing.
fn probes() -> Vec<(&'static str, fn())> {
    vec![
        (SOURCE_CODEX_ROLLOUTS_OPEN, probe_codex_rollout_observation),
        (
            SOURCE_WORKFLOW_EVOLUTION_NODES,
            probe_workflow_evolution_observations,
        ),
    ]
}

fn synthetic_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "licoup-observation-catalogue-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).expect("a synthetic data root");
    root
}

// ---------------------------------------------------------------------------
// Probe 1: the Codex rollout observation reaches conversation history
// ---------------------------------------------------------------------------

thread_local! {
    /// The rollout the probe's answer reports.
    ///
    /// `CodexRolloutPort::install` takes a bare `fn` pointer, so the probe hands
    /// its synthetic path through this cell instead of through a capture. The
    /// cell is thread-local because each test runs on its own thread, so two
    /// probes never read each other's answer.
    static PROBED_ROLLOUT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn probed_rollout_answer() -> BTreeSet<PathBuf> {
    PROBED_ROLLOUT
        .with(|cell| cell.borrow().clone())
        .into_iter()
        .collect()
}

/// A rollout whose newest lifecycle record says a task started.
fn synthetic_running_rollout(root: &Path) -> PathBuf {
    let rollout = root.join("rollout-2026-08-13-synthetic-thread.jsonl");
    std::fs::write(
        &rollout,
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
    )
    .expect("a synthetic rollout transcript");
    rollout
}

fn probe_codex_rollout_observation() {
    let root = synthetic_root("codex-rollout");
    let rollout = synthetic_running_rollout(&root);
    let canonical = std::fs::canonicalize(&rollout).expect("the synthetic rollout resolves");
    PROBED_ROLLOUT.with(|cell| *cell.borrow_mut() = Some(canonical));

    let port = CodexRolloutPort::new();
    port.install(probed_rollout_answer)
        .expect("the probe installs its answer once");
    let observed = CodexRuntimeObservation::capture_from(&port);
    assert!(
        observed.is_running(&rollout),
        "the rollout the port reports is running: the consumer must read the port"
    );

    // The same transcript through a port nobody answered: process evidence is
    // what makes a rollout running, never the transcript alone.
    let unanswered = CodexRuntimeObservation::capture_from(&CodexRolloutPort::new());
    assert!(
        !unanswered.is_running(&rollout),
        "a rollout no port reports is not running"
    );

    std::fs::remove_dir_all(&root).expect("the synthetic data root is removed");
}

// ---------------------------------------------------------------------------
// Probe 2: the workflow node observation reaches the evolution enricher
// ---------------------------------------------------------------------------

fn probed_node_observation() -> NodeObservationSummary {
    NodeObservationSummary {
        node_id: "synthetic-node".to_owned(),
        generation: 3,
        lifecycle_state: "active".to_owned(),
        work_role: "worker".to_owned(),
        active_invocation_id: Some("synthetic-invocation".to_owned()),
        durable_cursor: 7,
    }
}

fn pending_callback() -> PendingCallback {
    PendingCallback {
        state_id: "synthetic-state".to_owned(),
        state_visit: 1,
        transition_id: "synthetic-transition".to_owned(),
        event: TransitionEvent::Complete,
        target: "synthetic-target".to_owned(),
    }
}

fn probe_workflow_evolution_observations() {
    let root = synthetic_root("workflow-evolution");
    let observed = probed_node_observation();
    let port = Arc::new(DefaultEvolutionObservationPort::new());
    port.record_observation("synthetic-run", observed.clone());
    let enricher = CallbackEvolutionEnricher::new(&root).with_observation_port(port);

    let snapshot = RunSnapshot::empty("synthetic-run", "rev-1", "sem-1");
    let payload = enricher.enrich_callback_request(&snapshot, &pending_callback(), "synthetic");
    assert_eq!(
        payload.facts.observation.node_observations,
        vec![observed.clone()],
        "the node observation the port recorded is the one the consumer reports"
    );
    assert!(
        payload.facts.observation.observation_seam_active,
        "an admitted observation source reports an active seam"
    );

    let terminal = enricher.enrich_terminal_outcome(&snapshot);
    assert_eq!(
        terminal.facts.observation.node_observations,
        vec![observed],
        "the terminal path reads the same port"
    );

    // A run nobody observed contributes no node observation: the facts come
    // from the port, not from a constant.
    let unobserved = RunSnapshot::empty("synthetic-unobserved-run", "rev-1", "sem-1");
    let payload = enricher.enrich_callback_request(&unobserved, &pending_callback(), "synthetic");
    assert!(
        payload.facts.observation.node_observations.is_empty(),
        "a run no port answered reports no node observation"
    );

    std::fs::remove_dir_all(&root).expect("the synthetic data root is removed");
}

// ---------------------------------------------------------------------------
// The catalogue's own shape, and its pairing with the probes
// ---------------------------------------------------------------------------

#[test]
fn the_catalogue_carries_exactly_the_sources_this_contract_probes() {
    let probed = probes()
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    for id in &probed {
        assert!(
            admits(id),
            "the catalogue does not carry `{id}`, which a consumer this crate ships reads; \
             an observation source with no catalogue entry has no registered consumer"
        );
    }
    for source in sources() {
        assert!(
            probed.contains(&source.id),
            "the catalogue carries `{}` with no probe in this contract; a source with no \
             consumer this contract can drive is not an observation this host consumes",
            source.id
        );
    }
}

#[test]
fn every_catalogued_entry_names_a_consumer_that_reads_its_port() {
    let mut ids = sources().iter().map(|source| source.id).collect::<Vec<_>>();
    ids.sort_unstable();
    let unique = {
        let mut sorted = ids.clone();
        sorted.dedup();
        sorted
    };
    assert_eq!(ids, unique, "every catalogue id is unique: {ids:?}");

    for source in sources() {
        assert!(
            !source.observer.trim().is_empty() && !source.observes.trim().is_empty(),
            "`{}` states who observes and what is observed",
            source.id
        );
        let consumer = consumer_source(source);
        let production = production_region(&consumer);
        assert!(
            production.contains(source.port.port_type()),
            "`{}` names `{}` as its consumer, but that module reads no {} outside its tests",
            source.id,
            source.consumer,
            source.port.port_type()
        );
    }
}

/// The repository file the entry's consumer names: the path segments before the
/// type name are the module path under the crate's `src`.
fn consumer_source(source: &ObservationSource) -> String {
    let mut segments = source.consumer.split("::").collect::<Vec<_>>();
    let type_name = segments
        .pop()
        .expect("a consumer path names a type as its last segment");
    assert!(
        type_name.starts_with(char::is_uppercase),
        "`{}` names `{}`, whose last segment is not a type",
        source.id,
        source.consumer
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(segments.join("/"))
        .with_extension("rs");
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "`{}` names the consumer `{}`, which is not the production file {}: {error}",
            source.id,
            source.consumer,
            path.display()
        )
    })
}

/// The part of a module that production compiles: everything before its test
/// module. A consumer that reads its port only in a test does not consume it.
fn production_region(source: &str) -> &str {
    match source.find("#[cfg(test)]") {
        Some(offset) => &source[..offset],
        None => source,
    }
}

#[test]
fn every_probed_source_is_consumed_through_its_port() {
    for (id, probe) in probes() {
        assert!(
            admits(id),
            "`{id}` is not catalogued, so its consumer is not admitted"
        );
        probe();
    }
}
