//! Durable catalogue identity and session-owner verification.
//!
//! Two boundaries are exercised here, and they are deliberately separate:
//!
//! - **The durable record.** C09 §6 requires the generation reference and the
//!   active pointer to survive a restart. A restarted host must not hand out a
//!   generation or epoch a previous run used, an activation whose pointer could
//!   not be recorded must not commit, and a stop whose record failed must stay
//!   visible instead of vanishing. The fixture journal used here keeps those
//!   facts in memory; a production host wires the package store's journal, and
//!   `identity_is_durable()` is how the harness tells the two apart.
//! - **The session owner.** `Stopped` is a catalogue state. An instance whose
//!   session nobody observed is *not* a writer that stopped: its owner is
//!   recorded unverified, the carrier is never called on its behalf, and H1's
//!   rule applies — reconcile first, and only treat a stop as cleanup evidence
//!   once the owner confirms.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use licoup_application::RecoveryAction;
use licoup_extension_contracts::deployment::InstanceLifecycle;
use licoup_native::platform::extension_host::{
    CatalogJournal, ExtensionHost, JournalDurability, MemoryCatalogJournal, SessionOwner,
    StopReason,
};
use serde_json::json;

use crate::support::{
    ControlledCarrier, activate, contract_range, host as build_host, host_with_journal,
    stage_request,
};

#[test]
fn a_restarted_host_does_not_reuse_generations_or_epochs() {
    let journal = Arc::new(MemoryCatalogJournal::new());
    let first_carrier = ControlledCarrier::new("run-one");
    let first = host_with_journal(first_carrier.clone(), Arc::clone(&journal));
    assert!(
        !first.identity_is_durable(),
        "the in-memory fixture is never reported as durable identity, whatever it is wired into"
    );
    let first_receipt =
        activate(&first, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    assert_eq!(first_receipt.generation, 1);
    assert_eq!(first_receipt.registry_epoch.get(), 1);
    let old_binding = first
        .begin("acme.durable/run", &json!({"prompt": "old run"}))
        .expect("admit");
    let _ = first.result(&old_binding.binding).expect("settle");
    // A recorded stop clears the durable active pointer.
    assert_eq!(first.revoke("acme.durable/ext"), 1);
    assert!(journal.active().is_empty());
    assert!(first.journal_anomalies().is_empty());
    drop(first);

    // A restart over the same durable record.
    let watermark = journal.watermark().expect("watermark");
    let second_carrier = ControlledCarrier::new("run-two");
    let second = host_with_journal(second_carrier.clone(), Arc::clone(&journal));
    assert!(
        second.catalog().is_empty(),
        "a restart does not adopt the previous run's instances"
    );
    assert_eq!(
        second.catalog().epoch().get(),
        watermark.epoch,
        "the epoch starts where the record left it, not at zero"
    );
    let second_receipt =
        activate(&second, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    assert_eq!(
        second_receipt.generation,
        watermark
            .generations
            .get("acme.durable/ext")
            .copied()
            .unwrap_or(0)
            + 1,
        "the generation watermark is seeded, not reset"
    );
    assert_eq!(
        second_receipt.registry_epoch.get(),
        watermark.epoch + 1,
        "a new activation continues from the durable epoch, including the epochs stops published"
    );

    // The previous run's handle is refused by identity, and the new carrier
    // never sees it.
    let dispatches_before = second_carrier.dispatches();
    let failure = second
        .observe(&old_binding.binding)
        .expect_err("handles do not survive a restart");
    assert_eq!(failure.code, "extension_invocation_foreign_host");
    assert_eq!(second_carrier.dispatches(), dispatches_before);

    // The durable record names the new run's pointer, not the old one.
    let active = second.durable_active_pointers();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].instance_id, second_receipt.instance_id);
    assert_eq!(active[0].generation, 2);
    assert_eq!(
        active[0].registry_epoch,
        second_receipt.registry_epoch.get()
    );
}

#[test]
fn an_allocation_that_cannot_be_recorded_never_starts_a_carrier() {
    let journal = Arc::new(MemoryCatalogJournal::new());
    journal.fail_next_allocation();
    let carrier = ControlledCarrier::new("allocation-failure");
    let host = host_with_journal(carrier.clone(), Arc::clone(&journal));
    let staged = host
        .stage(stage_request(
            "acme.durable/ext",
            "1.0.0",
            &["acme.durable/run"],
            &[("agent-execution", &["acme.durable/run"])],
        ))
        .expect("stage");
    let failure = host
        .prepare(staged)
        .expect_err("an unrecorded allocation must not consume a generation");
    assert_eq!(failure.code, "extension_catalog_journal_failed");
    // The carrier was never started, so there is no session to release.
    assert!(carrier.calls().is_empty());
}

#[test]
fn an_activation_that_cannot_be_recorded_never_commits() {
    let journal = Arc::new(MemoryCatalogJournal::new());
    journal.fail_next_activation();
    let carrier = ControlledCarrier::new("journal-failure");
    let host = host_with_journal(carrier.clone(), Arc::clone(&journal));
    let staged = host
        .stage(stage_request(
            "acme.durable/ext",
            "1.0.0",
            &["acme.durable/run"],
            &[("agent-execution", &["acme.durable/run"])],
        ))
        .expect("stage");
    let prepared = host.prepare(staged).expect("prepare");
    let failure = host
        .activate(prepared)
        .expect_err("an unrecorded activation must not commit");
    assert_eq!(failure.code, "extension_catalog_journal_failed");
    assert!(host.catalog().is_empty());
    assert_eq!(host.catalog().epoch().get(), 0);
    assert_eq!(carrier.dropped_sessions.load(Ordering::SeqCst), 1);
    assert!(journal.active().is_empty());
}

#[test]
fn two_hosts_over_one_memory_journal_have_one_writer() {
    let journal = Arc::new(MemoryCatalogJournal::new());
    let first = host_with_journal(ControlledCarrier::new("first"), journal.clone());
    // The writer slot is a journal-level fact, not a file-level one: a second
    // host over the same fixture journal is refused at construction too.
    match ExtensionHost::with_journal(
        ControlledCarrier::new("second"),
        contract_range(),
        journal.clone(),
    ) {
        Ok(_) => panic!("two hosts cannot share one journal's writer slot"),
        Err(failure) => assert_eq!(failure.code, "runtime_catalog_writer_busy"),
    }
    drop(first);
    // The slot frees with its host, and the record carries on.
    let third = host_with_journal(ControlledCarrier::new("third"), journal.clone());
    assert!(third.catalog().entries().next().is_none());
}

#[test]
fn a_memory_fixture_is_never_reported_as_durable() {
    let local = build_host(ControlledCarrier::new("process-local"));
    assert!(!local.identity_is_durable());
    assert!(local.durable_active_pointers().is_empty());

    let journal = Arc::new(MemoryCatalogJournal::new());
    assert_eq!(journal.durability(), JournalDurability::ProcessLocal);
    let fixture = host_with_journal(ControlledCarrier::new("fixture"), journal);
    // A journal is present, and the identity is still not durable: presence is
    // not the capability.
    assert!(!fixture.identity_is_durable());
}

#[test]
fn an_unobserved_session_is_not_a_stopped_process() {
    let carrier = ControlledCarrier::new("owner");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.owner/ext", &["acme.owner/run"]).expect("activate");
    let admitted = host
        .begin("acme.owner/run", &json!({"prompt": "in flight"}))
        .expect("admit");
    assert_eq!(
        host.catalog()
            .entry(&receipt.instance_id)
            .expect("entry")
            .session_owner,
        SessionOwner::Held
    );

    let missing = host.reconcile_after_restart(&[]);
    assert_eq!(missing, vec![receipt.instance_id.clone()]);
    let entry = host
        .catalog()
        .entry(&receipt.instance_id)
        .cloned()
        .expect("entry");
    assert_eq!(entry.state, InstanceLifecycle::Stopped);
    assert_eq!(entry.session_owner, SessionOwner::StoppedUnverified);
    // No session owner confirmed the process is gone, so nothing was released
    // on its behalf and the catalogue stop is not cleanup evidence.
    assert_eq!(carrier.dropped_sessions.load(Ordering::SeqCst), 0);
    assert_eq!(
        host.unverified_session_owners(),
        vec![receipt.instance_id.clone()]
    );
    let failure = host
        .finish_drain(&receipt.instance_id)
        .expect_err("an unverified stop is not a clean stop");
    assert_eq!(failure.code, "extension_session_owner_unverified");
    assert_eq!(failure.recovery, RecoveryAction::ReconcileBeforeRetry);

    // The original owner confirms; the held session is released as that
    // evidence, and only then is the stop a verified one.
    host.confirm_session_owner_stopped(&receipt.instance_id)
        .expect("owner confirmation");
    assert_eq!(carrier.dropped_sessions.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.catalog()
            .entry(&receipt.instance_id)
            .expect("entry")
            .session_owner,
        SessionOwner::StoppedVerified
    );
    assert!(host.unverified_session_owners().is_empty());
    host.finish_drain(&receipt.instance_id)
        .expect("a confirmed stop is clean");
    // The in-flight call was recorded unknown at reconcile, never replayed.
    let failure = host
        .result(&admitted.binding)
        .expect_err("settled work is not settled again");
    assert_eq!(failure.code, "extension_invocation_settled");
}

#[test]
fn a_stop_whose_durable_record_fails_stays_visible() {
    let journal = Arc::new(MemoryCatalogJournal::new());
    let carrier = ControlledCarrier::new("stop-record");
    let host = host_with_journal(carrier.clone(), Arc::clone(&journal));
    let receipt = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");

    journal.fail_next_stop();
    let failure = host
        .finish_drain(&receipt.instance_id)
        .expect_err("the durable record failed");
    assert_eq!(failure.code, "extension_catalog_journal_failed");
    // The local stop happened; the durable pointer still names the instance and
    // that fact is reported, not hidden.
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Stopped
    );
    assert_eq!(
        host.catalog()
            .entry(&receipt.instance_id)
            .expect("entry")
            .session_owner,
        SessionOwner::StoppedVerified
    );
    assert_eq!(journal.active().len(), 1, "the stale pointer is visible");
    assert_eq!(host.journal_anomalies().len(), 1);
    assert!(host.journal_anomalies()[0].contains(&receipt.instance_id));
}

#[test]
fn a_recorded_stop_clears_the_pointer_and_names_its_reason() {
    let journal = Arc::new(MemoryCatalogJournal::new());
    let host = host_with_journal(ControlledCarrier::new("revoke"), Arc::clone(&journal));
    let receipt = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    assert_eq!(host.revoke("acme.durable/ext"), 1);

    assert!(journal.active().is_empty());
    let stops = journal.stops();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].reason, StopReason::Revoked);
    assert_eq!(stops[0].pointer.instance_id, receipt.instance_id);
    assert_eq!(stops[0].pointer.package_id, "acme.durable/ext");
    assert_eq!(stops[0].pointer.generation, 1);
    assert!(host.journal_anomalies().is_empty());
}
