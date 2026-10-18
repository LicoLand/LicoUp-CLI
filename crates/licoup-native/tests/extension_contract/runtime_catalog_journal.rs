//! The real runtime catalogue record: files under a managed root, reopened by a
//! later run, with failures injected through the filesystem.
//!
//! These tests use synthetic temporary roots — nothing here touches a real
//! install root or a user process. What they prove is that the record the host
//! writes is the record the next host reads: epochs and consumed generations
//! continue instead of resetting, an active pointer a previous run left behind
//! stays visible and blocks its scope until its owner is confirmed, and a write
//! that fails prevents the operation that depended on it instead of pretending.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use licoup_application::RecoveryAction;
use licoup_native::platform::extension_host::{
    CatalogJournal, ExtensionHost, JournalDurability, RuntimeCatalogJournal, SessionOwner,
};
use serde_json::json;

use crate::support::{
    ControlledCarrier, activate, contract_range, host_with_catalog_journal, stage_request,
};

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "licoup-x1-runtime-catalog-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("temp root");
    root
}

fn set_record_read_only(path: &std::path::Path, read_only: bool) {
    let mut permissions = fs::metadata(path).expect("record metadata").permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(read_only);
    fs::set_permissions(path, permissions).expect("record permissions");
}

#[test]
fn a_real_record_survives_reopen_and_keeps_an_unconfirmed_owner_visible() {
    let root = temp_root("reopen");
    {
        let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
        assert_eq!(
            journal.durability(),
            JournalDurability::Durable { root: root.clone() }
        );
        let first = host_with_catalog_journal(ControlledCarrier::new("run-one"), journal.clone());
        assert!(first.identity_is_durable());
        let receipt = activate(&first, "acme.durable/ext", &["acme.durable/run"]).expect("v1");
        assert_eq!(receipt.generation, 1);
        assert_eq!(receipt.registry_epoch.get(), 1);
        // The run ends without stopping the instance: this is a crash.
    }

    // A later run opens the same managed root.
    let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("reopen"));
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(watermark.epoch, 1);
    assert_eq!(watermark.generations.get("acme.durable/ext"), Some(&1));
    assert_eq!(watermark.active.len(), 1);

    let carrier = ControlledCarrier::new("run-two");
    let second = host_with_catalog_journal(carrier.clone(), journal.clone());
    assert!(second.identity_is_durable());
    assert_eq!(
        second.catalog().epoch().get(),
        1,
        "the epoch starts where the record left it, not at zero"
    );
    // The recorded pointer is kept, not dropped, and it is visible to every
    // surface through the catalogue document.
    let pending = second.pending_predecessors();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].instance_id, "instance-1");
    assert_eq!(second.catalog().document().predecessors.len(), 1);
    assert_eq!(
        second.unverified_session_owners(),
        vec!["instance-1".to_owned()]
    );

    // New work for that scope is refused while the owner is unconfirmed. The
    // blocked attempt was still prepared, and a consumed generation is never
    // handed out again.
    let staged = second
        .stage(stage_request(
            "acme.durable/ext",
            "2.0.0",
            &["acme.durable/run"],
            &[("agent-execution", &["acme.durable/run"])],
        ))
        .expect("stage");
    let prepared = second.prepare(staged).expect("prepare");
    assert_eq!(prepared.generation(), 2);
    let blocked = second
        .activate(prepared)
        .expect_err("an unconfirmed owner blocks the scope");
    assert_eq!(blocked.code, "extension_predecessor_unreconciled");
    assert_eq!(blocked.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(!second.catalog().serves("acme.durable/run"));
    assert_eq!(carrier.dispatches(), 0);
    assert_eq!(
        second.catalog().epoch().get(),
        1,
        "a refused activation publishes no epoch"
    );
    assert_eq!(
        journal
            .watermark()
            .expect("watermark")
            .generations
            .get("acme.durable/ext"),
        Some(&2),
        "the blocked preparation consumed generation 2"
    );

    // The owner confirms, and that clearing is durable.
    second
        .confirm_session_owner_stopped("instance-1")
        .expect("predecessor confirmed");
    assert_eq!(
        second.catalog().epoch().get(),
        2,
        "resolving a predecessor is a catalogue change of its own epoch"
    );
    assert!(second.pending_predecessors().is_empty());
    assert!(journal.watermark().expect("watermark").active.is_empty());
    assert_eq!(
        second
            .catalog()
            .document()
            .extensions
            .iter()
            .filter(|entry| entry.session_owner == SessionOwner::StoppedUnverified)
            .count(),
        0
    );

    // The scope is activatable again, with the next generation and a free id.
    let receipt = activate(&second, "acme.durable/ext", &["acme.durable/run"]).expect("v2");
    assert_eq!(
        receipt.generation, 3,
        "the consumed generation is not reused"
    );
    assert_eq!(receipt.registry_epoch.get(), 3);
    assert_ne!(receipt.instance_id, "instance-1");
    assert!(second.catalog().serves("acme.durable/run"));
    let admitted = second
        .begin("acme.durable/run", &json!({"prompt": "new run"}))
        .expect("admit on the new generation");
    assert_eq!(admitted.binding.generation(), 3);
    assert_eq!(admitted.binding.instance_id(), receipt.instance_id);

    // The durable record now names the new owner only.
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(watermark.active.len(), 1);
    assert_eq!(watermark.active[0].instance_id, receipt.instance_id);
    assert_eq!(watermark.active[0].generation, 3);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn an_unverified_owner_survives_a_second_restart_until_confirmed() {
    let root = temp_root("reconcile-chain");
    let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
    let first = host_with_catalog_journal(ControlledCarrier::new("first"), journal.clone());
    let receipt = activate(&first, "acme.durable/ext", &["acme.durable/run"]).expect("activate");

    let missing = first.reconcile_after_restart(&[]);
    assert_eq!(missing, vec![receipt.instance_id.clone()]);
    assert_eq!(
        first
            .instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Stopped
    );
    assert_eq!(
        first.unverified_session_owners(),
        vec![receipt.instance_id.clone()]
    );
    // Reconcile is a catalogue state, not a release: the record keeps naming
    // the owner whose process nobody confirmed stopped.
    assert_eq!(
        journal.watermark().expect("watermark").active.len(),
        1,
        "reconcile does not clear the durable pointer"
    );
    drop(first);

    // A second restart inherits the same unverified owner and still refuses the
    // scope, so no unknown owner is silently replaced.
    let second = host_with_catalog_journal(ControlledCarrier::new("second"), journal.clone());
    assert_eq!(second.pending_predecessors().len(), 1);
    let failure = activate(&second, "acme.durable/ext", &["acme.durable/run"])
        .expect_err("the owner is still unverified");
    assert_eq!(failure.code, "extension_predecessor_unreconciled");

    // Only the owner's confirmation clears the slot.
    second
        .confirm_session_owner_stopped(&receipt.instance_id)
        .expect("confirmed");
    assert!(journal.watermark().expect("watermark").active.is_empty());
    let next = activate(&second, "acme.durable/ext", &["acme.durable/run"]).expect("admitted");
    assert!(next.generation > receipt.generation);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_prepare_only_crash_burns_the_generation() {
    let root = temp_root("prepare-crash");
    {
        let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
        let host = host_with_catalog_journal(ControlledCarrier::new("crash"), journal);
        let staged = host
            .stage(stage_request(
                "acme.durable/ext",
                "1.0.0",
                &["acme.durable/run"],
                &[("agent-execution", &["acme.durable/run"])],
            ))
            .expect("stage");
        let prepared = host.prepare(staged).expect("prepare");
        assert_eq!(prepared.generation(), 1);
        // Never activated: the run dies between preparation and commit.
    }

    let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("reopen"));
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(
        watermark.generations.get("acme.durable/ext"),
        Some(&1),
        "a consumed generation is recorded before the carrier starts"
    );
    assert!(
        watermark.active.is_empty(),
        "nothing was published, so nothing is active"
    );

    let host = host_with_catalog_journal(ControlledCarrier::new("next"), journal);
    let receipt = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    assert_eq!(
        receipt.generation, 2,
        "a burned generation is not handed to real work"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_real_write_failure_does_not_commit_and_stays_visible() {
    let root = temp_root("write-failure");
    let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
    let carrier = ControlledCarrier::new("write-failure");
    let host = host_with_catalog_journal(carrier.clone(), journal.clone());
    let baseline = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("baseline");
    let record = journal.path().to_path_buf();

    // An allocation that cannot be recorded must not start a carrier.
    set_record_read_only(&record, true);
    let staged = host
        .stage(stage_request(
            "acme.durable/ext",
            "2.0.0",
            &["acme.durable/run"],
            &[("agent-execution", &["acme.durable/run"])],
        ))
        .expect("stage");
    let failure = host
        .prepare(staged)
        .expect_err("an unrecorded allocation must not start a carrier");
    assert_eq!(failure.code, "runtime_catalog_unavailable");
    assert_eq!(
        carrier.dropped_sessions.load(Ordering::SeqCst),
        0,
        "no session was started"
    );

    // An activation that cannot be recorded must not publish.
    set_record_read_only(&record, false);
    let staged = host
        .stage(stage_request(
            "acme.durable/ext",
            "2.0.0",
            &["acme.durable/run"],
            &[("agent-execution", &["acme.durable/run"])],
        ))
        .expect("stage");
    let prepared = host.prepare(staged).expect("prepare");
    let prepared_instance = prepared.instance_id().to_owned();
    let epoch_before = host.catalog().epoch();
    set_record_read_only(&record, true);
    let failure = host
        .activate(prepared)
        .expect_err("an unrecorded activation must not commit");
    assert_eq!(failure.code, "runtime_catalog_unavailable");
    assert!(host.catalog().entry(&prepared_instance).is_none());
    assert_eq!(host.catalog().epoch(), epoch_before);
    assert_eq!(carrier.dropped_sessions.load(Ordering::SeqCst), 1);
    assert!(host.journal_anomalies().is_empty());

    // A stop that cannot be recorded is visible instead of hidden.
    let failure = host
        .finish_drain(&baseline.instance_id)
        .expect_err("an unrecorded stop is visible");
    assert_eq!(failure.code, "runtime_catalog_unavailable");
    assert_eq!(host.journal_anomalies().len(), 1);
    assert_eq!(
        host.instance_report(&baseline.instance_id)
            .expect("report")
            .state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Stopped
    );
    // The durable pointer is still the baseline's: the stop's clearing was not
    // written, and the record says so.
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(watermark.active.len(), 1);
    assert_eq!(watermark.active[0].instance_id, baseline.instance_id);

    set_record_read_only(&record, false);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_stop_reason_is_recorded_and_reads_back() {
    let root = temp_root("stop-reason");
    let (published, record, instance_id) = {
        let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
        let host = host_with_catalog_journal(ControlledCarrier::new("stop"), journal.clone());
        let receipt = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
        assert_eq!(host.revoke("acme.durable/ext"), 1);
        assert!(host.journal_anomalies().is_empty());
        (
            host.catalog().epoch().get(),
            journal.path().to_path_buf(),
            receipt.instance_id,
        )
    };

    // The writer lease is released with its holder, so a later run can read.
    let reopened = RuntimeCatalogJournal::open(&root).expect("reopen");
    let watermark = reopened.watermark().expect("watermark");
    assert!(watermark.active.is_empty());
    assert!(
        watermark.epoch >= published,
        "the durable epoch never falls below what the run published ({watermark_epoch} vs {published})",
        watermark_epoch = watermark.epoch
    );
    assert_eq!(watermark.generations.get("acme.durable/ext"), Some(&1));
    // The stop line is real and names the instance; the fold removes it from
    // the active set without losing the generation it consumed.
    let lines = fs::read_to_string(&record).expect("record");
    assert!(lines.contains("\"kind\":\"stop\""));
    assert!(lines.contains(&instance_id));
    assert!(lines.contains("revoked"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn one_root_has_one_writer_and_a_second_open_is_refused() {
    let root = temp_root("single-writer");
    let first_journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("first writer"));
    let busy = RuntimeCatalogJournal::open(&root).expect_err("one root has one writer");
    assert_eq!(busy.code, "runtime_catalog_writer_busy");

    let first_carrier = ControlledCarrier::new("first");
    let host = host_with_catalog_journal(first_carrier.clone(), first_journal.clone());
    let receipt = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    assert_eq!(receipt.generation, 1);
    // Exactly one writer published: the refused opener cannot be a second host
    // publishing generation one behind the first one's back.
    let record = root.join("journal").join("runtime-catalog.jsonl");
    let lines = fs::read_to_string(&record).expect("record");
    assert_eq!(lines.matches("\"kind\":\"activation\"").count(), 1);
    let watermark = first_journal.watermark().expect("watermark");
    assert_eq!(watermark.active.len(), 1, "one writer, one owner");
    assert_eq!(watermark.active[0].generation, 1);
    // A clean handover: the instance is stopped before the lease is released,
    // so the next writer inherits no unconfirmed owner.
    assert_eq!(host.revoke("acme.durable/ext"), 1);
    assert_eq!(first_carrier.dropped_sessions.load(Ordering::SeqCst), 1);
    // Both handles must go: the lease lives until the last holder drops it.
    drop(host);
    drop(first_journal);

    let reopened = RuntimeCatalogJournal::open(&root).expect("released writer");
    let watermark = reopened.watermark().expect("watermark");
    assert_eq!(watermark.generations.get("acme.durable/ext"), Some(&1));
    let host = host_with_catalog_journal(ControlledCarrier::new("second"), Arc::new(reopened));
    let next = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    assert_eq!(
        next.generation, 2,
        "the next writer continues, it does not restart"
    );
    let lines = fs::read_to_string(&record).expect("record");
    assert_eq!(lines.matches("\"kind\":\"activation\"").count(), 2);
    drop(host);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn two_hosts_sharing_one_journal_cannot_both_become_catalog_writers() {
    let root = temp_root("shared-writer");
    let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));

    // Both hosts are constructed before anything is prepared: at this point the
    // record is empty and both would read watermark zero.
    let first_carrier = ControlledCarrier::new("first");
    let first = host_with_catalog_journal(first_carrier.clone(), journal.clone());
    let second_carrier = ControlledCarrier::new("second");
    match ExtensionHost::with_journal(second_carrier.clone(), contract_range(), journal.clone()) {
        Err(failure) => {
            assert_eq!(failure.code, "runtime_catalog_writer_busy");
            assert!(
                second_carrier.calls().is_empty(),
                "a refused host never starts a carrier"
            );
            // The first host is the writer and publishes normally.
            let first_receipt =
                activate(&first, "acme.durable/ext", &["acme.durable/run"]).expect("first");
            assert_eq!(first_receipt.generation, 1);
            // It leaves without stopping its instance.
            drop(first);
            // A later host claims the record and inherits the unconfirmed owner
            // instead of silently replacing it.
            let third_carrier = ControlledCarrier::new("third");
            let third = host_with_catalog_journal(third_carrier.clone(), journal.clone());
            assert_eq!(
                third.catalog().epoch().get(),
                1,
                "the record's epoch is read, not restarted"
            );
            assert_eq!(third.pending_predecessors().len(), 1);
            assert_eq!(
                third.unverified_session_owners(),
                vec![first_receipt.instance_id.clone()]
            );
            let blocked = activate(&third, "acme.durable/ext", &["acme.durable/run"])
                .expect_err("the unconfirmed owner is not replaced");
            assert_eq!(blocked.code, "extension_predecessor_unreconciled");
            third
                .confirm_session_owner_stopped(&first_receipt.instance_id)
                .expect("the owner confirms");
            let next = activate(&third, "acme.durable/ext", &["acme.durable/run"])
                .expect("activate after confirmation");
            assert!(next.generation > first_receipt.generation);
            fs::remove_dir_all(&root).ok();
        }
        Ok(second) => {
            // Unreachable once the guard holds. The pre-fix split: both hosts
            // were constructed at watermark zero and each publishes its own
            // generation one, with its own carrier started.
            let first_receipt =
                activate(&first, "acme.durable/ext", &["acme.durable/run"]).expect("first");
            let second_receipt =
                activate(&second, "acme.durable/ext", &["acme.durable/run"]).expect("second");
            assert!(second.catalog().serves("acme.durable/run"));
            panic!(
                "two hosts over one journal: generation {} and generation {} both routable; \
                 second carrier calls {}",
                first_receipt.generation,
                second_receipt.generation,
                second_carrier.calls().len()
            );
        }
    }
}

#[test]
fn a_corrupt_complete_line_is_fail_closed() {
    let root = temp_root("corrupt");
    let record = {
        let journal = RuntimeCatalogJournal::open(&root).expect("open");
        let host = host_with_catalog_journal(ControlledCarrier::new("corrupt"), Arc::new(journal));
        let _ = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
        root.join("journal").join("runtime-catalog.jsonl")
    };
    {
        // A complete line that is not this record's shape, followed by a valid
        // line so it is provably not a truncated tail.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&record)
            .expect("append");
        file.write_all(b"{\"kind\":\"future-version\",\"atUnixMs\":1}\n")
            .expect("write corrupt line");
        file.write_all(
            b"{\"kind\":\"allocation\",\"atUnixMs\":2,\"packageId\":\"acme.durable/ext\",\"generation\":9}\n",
        )
        .expect("write valid line");
    }
    let journal = RuntimeCatalogJournal::open(&root).expect("open");
    let failure = journal
        .watermark()
        .expect_err("a complete invalid line is not a truncated tail");
    assert_eq!(failure.code, "runtime_catalog_corrupt");
    // The host refuses to come up over it instead of inheriting a forgotten
    // active pointer.
    match ExtensionHost::with_journal(
        ControlledCarrier::new("reader"),
        contract_range(),
        Arc::new(journal),
    ) {
        Ok(_) => panic!("a corrupt record must fail closed"),
        Err(failure) => assert_eq!(failure.code, "runtime_catalog_corrupt"),
    }
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_truncated_tail_is_dropped_before_the_next_append() {
    let root = temp_root("truncated-tail");
    let receipt = {
        let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
        let host = host_with_catalog_journal(ControlledCarrier::new("tail"), journal.clone());
        let receipt = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
        // A half-written record: incomplete JSON, no terminator.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(journal.path())
            .expect("append");
        file.write_all(b"{\"kind\":\"activation\",\"atUnixMs\":1,\"pointer\":{\"packageId\":\"acme.durable/ext\",\"generation\":")
            .expect("write half line");
        // The next append must repair the tail first, not merge into it.
        let staged = host
            .stage(stage_request(
                "acme.durable/ext",
                "1.0.0",
                &["acme.durable/run"],
                &[("agent-execution", &["acme.durable/run"])],
            ))
            .expect("stage");
        let prepared = host.prepare(staged).expect("prepare");
        assert_eq!(prepared.generation(), 2);
        receipt
    };

    let journal = RuntimeCatalogJournal::open(&root).expect("reopen");
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(
        watermark.generations.get("acme.durable/ext"),
        Some(&2),
        "the allocation written after the repair is a whole line"
    );
    assert_eq!(watermark.active.len(), 1);
    assert_eq!(watermark.active[0].instance_id, receipt.instance_id);
    let content = fs::read_to_string(journal.path()).expect("record");
    assert!(content.ends_with('\n'));
    for line in content.lines() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "every complete line parses: {line}"
        );
    }
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_complete_fact_without_its_terminator_is_kept() {
    let root = temp_root("unterminated-fact");
    {
        let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
        let host = host_with_catalog_journal(ControlledCarrier::new("fact"), journal.clone());
        let _ = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
        // A complete record whose newline never made it out.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(journal.path())
            .expect("append");
        file.write_all(
            b"{\"kind\":\"allocation\",\"atUnixMs\":2,\"packageId\":\"acme.other/ext\",\"generation\":7}",
        )
        .expect("write unterminated fact");
    }
    // The fact is a fact: the reader keeps it.
    {
        let journal = RuntimeCatalogJournal::open(&root).expect("open");
        let watermark = journal.watermark().expect("watermark");
        assert_eq!(watermark.generations.get("acme.other/ext"), Some(&7));
    }
    // The next append terminates it and writes a fresh line.
    {
        let journal = Arc::new(RuntimeCatalogJournal::open(&root).expect("open"));
        let host = host_with_catalog_journal(ControlledCarrier::new("next"), journal.clone());
        let staged = host
            .stage(stage_request(
                "acme.durable/ext",
                "1.0.0",
                &["acme.durable/run"],
                &[("agent-execution", &["acme.durable/run"])],
            ))
            .expect("stage");
        let _prepared = host.prepare(staged).expect("prepare");
    }
    let journal = RuntimeCatalogJournal::open(&root).expect("reopen");
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(
        watermark.generations.get("acme.other/ext"),
        Some(&7),
        "a complete fact is not dropped as a tail"
    );
    assert_eq!(watermark.generations.get("acme.durable/ext"), Some(&2));
    let content = fs::read_to_string(journal.path()).expect("record");
    for line in content.lines() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "every complete line parses: {line}"
        );
    }
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_stale_stop_does_not_clear_a_newer_pointer() {
    let root = temp_root("stale-stop");
    {
        let journal = RuntimeCatalogJournal::open(&root).expect("open");
        let host = host_with_catalog_journal(ControlledCarrier::new("stale"), Arc::new(journal));
        let _ = activate(&host, "acme.durable/ext", &["acme.durable/run"]).expect("activate");
    }
    let record = root.join("journal").join("runtime-catalog.jsonl");
    {
        // A stop naming the same instance but a different generation is not
        // evidence about the recorded pointer.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&record)
            .expect("append");
        file.write_all(
            b"{\"kind\":\"stop\",\"atUnixMs\":9,\"reason\":\"predecessor-confirmed\",\"epoch\":9,\"pointer\":{\"packageId\":\"acme.durable/ext\",\"packageVersion\":\"1.0.0\",\"permissionScope\":[\"scope:local\"],\"instanceId\":\"instance-1\",\"generation\":2,\"registryEpoch\":1}}\n",
        )
        .expect("write stale stop");
    }
    {
        let journal = RuntimeCatalogJournal::open(&root).expect("open");
        let watermark = journal.watermark().expect("watermark");
        assert_eq!(
            watermark.active.len(),
            1,
            "a stop for another generation does not clear this pointer"
        );
    }
    {
        // The stop that matches the full pointer does clear it.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&record)
            .expect("append");
        file.write_all(
            b"{\"kind\":\"stop\",\"atUnixMs\":10,\"reason\":\"predecessor-confirmed\",\"epoch\":10,\"pointer\":{\"packageId\":\"acme.durable/ext\",\"packageVersion\":\"1.0.0\",\"permissionScope\":[\"scope:local\"],\"instanceId\":\"instance-1\",\"generation\":1,\"registryEpoch\":1}}\n",
        )
        .expect("write matching stop");
    }
    let journal = RuntimeCatalogJournal::open(&root).expect("open");
    assert!(journal.watermark().expect("watermark").active.is_empty());
    fs::remove_dir_all(&root).ok();
}
