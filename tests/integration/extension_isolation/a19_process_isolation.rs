//! A19 at `component-integration`: replaceable carriers and plugin fault domains,
//! with real subprocesses.
//!
//! Every case here starts a real program under the production carrier. Faults
//! are real: a process that never answers its handshake, a process that dies
//! mid-call, a process that floods its stdout, a process whose descendant holds
//! the stdout pipe, and a process that refuses to exit. The assertions are
//! about observed process state and the host's own records, never about a
//! carrier double.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use licoup_extension_contracts::deployment::InstanceLifecycle;
use licoup_native::platform::extension_host::isolation::{
    IsolationMode, IsolationRecord, ReleaseScope, ResolvedProgram, ResourceLimits, StaticPrograms,
};
use licoup_native::platform::extension_host::{
    CancelDisposition, CatalogJournal, ExtensionHost, InvocationOutcome, RuntimeCatalogJournal,
    SessionOwner,
};
use serde_json::{Value, json};

use crate::support::{
    Sandbox, activate, carrier, contract_range, failure_of, host_of, payload_of, pid_alive, policy,
    programs, python_runtime, read_text, settle, shell_program, wait_for,
};

/// The payload of a settled call.
fn finished_payload(outcome: InvocationOutcome) -> Value {
    payload_of(outcome)
}

#[test]
fn real_subprocess_carriers_replace_each_other_without_changing_the_consumer() {
    let sandbox = Sandbox::new("replace");
    let alpha = carrier(
        programs(
            "acme.alpha/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "respond",
                "alpha",
                &sandbox.instance_root("alpha"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let beta = carrier(
        programs(
            "acme.beta/ext",
            "1.0.0",
            shell_program(&sandbox, "respond", "beta", &sandbox.instance_root("beta")),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let alpha_host = host_of(alpha.clone());
    let beta_host = host_of(beta.clone());
    activate(&alpha_host, "acme.alpha/ext", &["acme.alpha/render"]).expect("activate alpha");
    activate(&beta_host, "acme.beta/ext", &["acme.beta/render"]).expect("activate beta");

    // The consumer path is identical; only the program behind the carrier differs.
    let alpha_call = alpha_host
        .begin("acme.alpha/render", &json!({"input": "draw"}))
        .expect("admit on alpha");
    let beta_call = beta_host
        .begin("acme.beta/render", &json!({"input": "draw"}))
        .expect("admit on beta");

    let alpha_outcome = settle(&alpha_host, &alpha_call.binding, Duration::from_secs(5));
    let beta_outcome = settle(&beta_host, &beta_call.binding, Duration::from_secs(5));
    assert_eq!(finished_payload(alpha_outcome)["marker"], json!("alpha"));
    assert_eq!(finished_payload(beta_outcome)["marker"], json!("beta"));

    // Both answers came from real processes with observed exits.
    for (carrier, host) in [(&alpha, &alpha_host), (&beta, &beta_host)] {
        let facts = carrier.facts("instance-1").expect("the instance was real");
        assert!(facts.pid > 0);
        assert!(
            carrier
                .ledger()
                .grants()
                .expect("grants")
                .contains_key("instance-1")
        );
        assert!(host.catalog().entry("instance-1").is_some());
    }
}

#[test]
fn a_hung_extension_faults_within_its_window_and_the_host_keeps_serving() {
    let sandbox = Sandbox::new("hung");
    let mut programs = StaticPrograms::new();
    programs.insert(
        "acme.healthy/ext",
        "1.0.0",
        shell_program(
            &sandbox,
            "respond",
            "healthy",
            &sandbox.instance_root("healthy"),
        ),
    );
    programs.insert(
        "acme.hung/ext",
        "1.0.0",
        shell_program(&sandbox, "hang", "", &sandbox.instance_root("hung")),
    );
    let carrier = carrier(
        programs,
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 600,
            ..Default::default()
        }),
    );
    let host = host_of(carrier.clone());
    let healthy = activate(&host, "acme.healthy/ext", &["acme.healthy/run"]).expect("healthy");

    let started = Instant::now();
    let failure = activate(&host, "acme.hung/ext", &["acme.hung/run"])
        .expect_err("a handshake that never answers faults");
    assert_eq!(failure.code, "extension_carrier_unresponsive");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the fault is bounded, not an open-ended wait"
    );

    // The failed preparation was never committed, and its process is gone.
    assert!(host.instance_report("instance-2").is_none());
    let facts = carrier.facts("instance-2").expect("real process facts");
    assert!(!pid_alive(facts.pid));
    assert!(facts.exit().is_some(), "the teardown observed the exit");
    assert!(facts.released());

    // The other instance keeps working through the same host.
    let call = host
        .begin("acme.healthy/run", &json!({"input": "still here"}))
        .expect("the healthy capability still admits");
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("healthy")
    );
    assert_eq!(
        host.instance_report(&healthy.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Active
    );
}

#[test]
fn a_crashing_extension_is_isolated_and_its_call_is_never_replayed() {
    let sandbox = Sandbox::new("crash");
    let mut programs = StaticPrograms::new();
    programs.insert(
        "acme.crash/ext",
        "1.0.0",
        shell_program(&sandbox, "crash", "", &sandbox.instance_root("crash")),
    );
    programs.insert(
        "acme.healthy/ext",
        "1.0.0",
        shell_program(
            &sandbox,
            "respond",
            "healthy",
            &sandbox.instance_root("healthy"),
        ),
    );
    let carrier = carrier(
        programs,
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(&host, "acme.crash/ext", &["acme.crash/run"]).expect("activate");
    activate(&host, "acme.healthy/ext", &["acme.healthy/run"]).expect("healthy");

    let call = host
        .begin("acme.crash/run", &json!({"input": "go"}))
        .expect("admit");
    let failure = failure_of(&host, &call.binding, Duration::from_secs(8));
    assert_eq!(failure.code, "extension_carrier_crashed");

    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(report.state, InstanceLifecycle::Failed);
    assert_eq!(report.in_flight, 0);
    assert_eq!(report.unknown, 1, "the call is unknown, not completed");

    // The work was dispatched exactly once; the host never re-sent it.
    assert_eq!(
        carrier
            .facts(&receipt.instance_id)
            .expect("facts")
            .dispatched
            .load(Ordering::SeqCst),
        1
    );
    let failure = host
        .begin("acme.crash/run", &json!({}))
        .expect_err("a failed instance serves nothing new");
    assert_eq!(failure.presentation_args.get("reason"), Some("not-active"));

    // The healthy sibling is unaffected.
    let call = host
        .begin("acme.healthy/run", &json!({"input": "ok"}))
        .expect("healthy admission");
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("healthy")
    );
}

#[test]
fn an_output_flood_is_bounded_and_the_process_group_is_reclaimed() {
    let sandbox = Sandbox::new("flood");
    let limits = ResourceLimits {
        call_wall_ms: 1_500,
        max_frame_bytes: 16 * 1024,
        max_stdout_bytes: 128 * 1024,
        ..Default::default()
    };
    let carrier = carrier(
        programs(
            "acme.flood/ext",
            "1.0.0",
            shell_program(&sandbox, "flood", "", &sandbox.instance_root("flood")),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(limits),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(&host, "acme.flood/ext", &["acme.flood/run"]).expect("activate");
    let call = host
        .begin("acme.flood/run", &json!({"input": "flood"}))
        .expect("admit");

    let failure = failure_of(&host, &call.binding, Duration::from_secs(8));
    assert_eq!(failure.code, "extension_carrier_over_budget");

    let facts = carrier.facts(&receipt.instance_id).expect("facts");
    assert!(facts.frames_rejected.load(Ordering::SeqCst) >= 1);
    assert!(
        facts.bytes_stdout() <= limits.max_stdout_bytes + 8 * 1024,
        "the reader bounded its buffer instead of following the flood"
    );
    assert!(!pid_alive(facts.pid), "the flooding tree was reclaimed");
    assert!(facts.exit().is_some());
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Quarantined
    );

    // The host still publishes a complete catalogue.
    assert!(host.catalog().epoch().get() >= 2);
    assert!(host.catalog().entry(&receipt.instance_id).is_some());
}

#[test]
fn a_pipe_holding_descendant_is_reclaimed_with_an_observed_root_exit() {
    let sandbox = Sandbox::new("descendant");
    let instance = sandbox.instance_root("tree");
    let carrier = carrier(
        programs(
            "acme.tree/ext",
            "1.0.0",
            shell_program(&sandbox, "descendant", "tree", &instance),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_shutdown_grace_ms(800),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(&host, "acme.tree/ext", &["acme.tree/run"]).expect("activate");
    let call = host
        .begin("acme.tree/run", &json!({"input": "spawn"}))
        .expect("admit");
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("tree")
    );

    let grandchild_pid: u32 = read_text(&instance.join("grandchild.pid"))
        .expect("the descendant recorded its pid")
        .trim()
        .parse()
        .expect("pid");
    assert!(
        pid_alive(grandchild_pid),
        "the descendant really is running and holds the pipe"
    );

    // Withdrawing admission drains the instance; the root exits on its own, and
    // the carrier still has to reclaim the descendant that held the pipe.
    assert_eq!(host.revoke("acme.tree/ext"), 1);
    assert!(
        wait_for(Duration::from_secs(4), || !pid_alive(grandchild_pid)),
        "the process group was reclaimed"
    );

    // A trusted local release covers the supervised process and its group, not
    // the whole instance: the owner stays unverified instead of being cleared on
    // a root wait alone.
    let facts = carrier.facts(&receipt.instance_id).expect("facts");
    assert_eq!(facts.release_scope(), Some(ReleaseScope::ProcessGroup));
    assert!(
        !host.unverified_session_owners().is_empty(),
        "an unverified release keeps its owner visible"
    );
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Failed
    );

    // The record keeps the root's own observed exit; the root was not killed.
    let records = carrier.ledger().read().expect("record");
    let releases: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            IsolationRecord::Release {
                exit,
                forced,
                release_scope,
                ..
            } => Some((*exit, *forced, *release_scope)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "one release for one real process");
    assert!(releases[0].0.success);
    assert!(!releases[0].1, "the root exited on its own");
    assert_eq!(releases[0].2, ReleaseScope::ProcessGroup);
}

#[test]
fn cancel_is_a_request_and_in_flight_work_settles_on_its_original_generation() {
    let sandbox = Sandbox::new("cancel");
    let carrier = carrier(
        programs(
            "acme.delayed/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "delayed",
                "delayed",
                &sandbox.instance_root("delayed"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(&host, "acme.delayed/ext", &["acme.delayed/run"]).expect("activate");
    let call = host
        .begin("acme.delayed/run", &json!({"input": "long"}))
        .expect("admit");

    // Cancellation is a request: the extension acknowledges it, and the call is
    // still in flight until its terminal event.
    let disposition = match host.cancel(&call.binding) {
        Ok(disposition) => disposition,
        Err(failure) => panic!(
            "cancel failed: {} at {} (wire fault: {:?})",
            failure.code,
            failure.stage,
            carrier
                .facts(&receipt.instance_id)
                .and_then(|facts| facts.fault())
        ),
    };
    assert_eq!(disposition, CancelDisposition::Acknowledged);
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .in_flight,
        1
    );
    let failure = host
        .result(&call.binding)
        .expect_err("a cancellation alone settles nothing");
    assert_eq!(failure.code, "extension_result_not_ready");

    // Revocation withdraws new admission; it does not retract the admitted call.
    assert_eq!(host.revoke("acme.delayed/ext"), 1);
    let failure = host
        .begin("acme.delayed/run", &json!({}))
        .expect_err("revoked admission");
    assert_eq!(failure.presentation_args.get("reason"), Some("withdrawn"));

    let outcome = settle(&host, &call.binding, Duration::from_secs(5));
    assert_eq!(finished_payload(outcome)["marker"], json!("delayed"));
    // The drained trusted local instance keeps an unverified owner: its process
    // group was reclaimed, but descendants are not provable.
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Failed
    );
    assert!(!host.unverified_session_owners().is_empty());
    assert_eq!(
        carrier
            .facts(&receipt.instance_id)
            .expect("facts")
            .dispatched
            .load(Ordering::SeqCst),
        1,
        "the revoked instance's work was settled where it was admitted"
    );

    // An extension with no cancel at all says so, visibly, and the work is
    // still decided by its terminal event.
    let uncancellable = crate::support::carrier(
        programs(
            "acme.uncancellable/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "no-cancel-method",
                "uncancellable",
                &sandbox.instance_root("uncancellable"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(uncancellable.clone());
    activate(&host, "acme.uncancellable/ext", &["acme.uncancellable/run"]).expect("activate");
    let call = host
        .begin("acme.uncancellable/run", &json!({"input": "long"}))
        .expect("admit");
    assert_eq!(
        host.cancel(&call.binding).expect("cancel request"),
        CancelDisposition::Unsupported
    );
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(6)))["marker"],
        json!("uncancellable")
    );
}

#[test]
fn a_release_is_recorded_only_after_the_exit_was_observed() {
    let sandbox = Sandbox::new("noexit");
    let carrier = carrier(
        programs(
            "acme.noexit/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "noexit",
                "stubborn",
                &sandbox.instance_root("noexit"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_shutdown_grace_ms(400),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(&host, "acme.noexit/ext", &["acme.noexit/run"]).expect("activate");
    let call = host
        .begin("acme.noexit/run", &json!({"input": "work"}))
        .expect("admit");
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("stubborn")
    );

    // The runtime refuses to exit; the carrier has to tear the group down, and
    // only the observed exit lets the release be recorded.
    assert_eq!(host.revoke("acme.noexit/ext"), 1);
    let facts = carrier.facts(&receipt.instance_id).expect("facts");
    assert!(facts.released());
    assert!(facts.release_was_forced());
    let exit = facts.exit().expect("the exit was observed");
    assert!(
        exit.signal.is_some(),
        "teardown observed the signal that ended the process"
    );
    assert!(!pid_alive(facts.pid));
    // A forced group teardown is still a trusted local scope: the owner is not
    // verified from the root's status alone.
    assert_eq!(facts.release_scope(), Some(ReleaseScope::ProcessGroup));
    assert!(!host.unverified_session_owners().is_empty());
    assert_eq!(
        host.catalog()
            .entry(&receipt.instance_id)
            .expect("entry")
            .session_owner,
        SessionOwner::StoppedUnverified
    );
}

#[test]
fn a_real_journal_tracks_real_process_lifecycles_across_a_restart() {
    let sandbox = Sandbox::new("journal");
    let package = "acme.journal/ext";

    let journal = RuntimeCatalogJournal::open(sandbox.root()).expect("catalogue record");
    let first = carrier(
        programs(
            package,
            "1.0.0",
            shell_program(
                &sandbox,
                "respond",
                "first",
                &sandbox.instance_root("first"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = ExtensionHost::with_journal(first.clone(), contract_range(), Arc::new(journal))
        .expect("journal host");
    assert!(host.identity_is_durable());
    let receipt = activate(&host, package, &["acme.journal/run"]).expect("activate");
    let call = host
        .begin("acme.journal/run", &json!({"input": "go"}))
        .expect("admit");
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("first")
    );
    host.revoke(package);
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Failed
    );
    // The owner of a trusted local release stays unverified, and the durable
    // pointer is kept as an unconfirmed predecessor: a root wait plus a
    // reclaimed group is not proof that the instance left nothing behind.
    assert!(!host.unverified_session_owners().is_empty());
    drop(host);

    // A restarted host neither adopts nor drops the unconfirmed predecessor.
    let journal = RuntimeCatalogJournal::open(sandbox.root()).expect("reopen");
    let watermark = journal.watermark().expect("watermark");
    assert_eq!(
        watermark.active.len(),
        1,
        "an unverified release keeps its active pointer"
    );
    assert_eq!(watermark.generations.get(package), Some(&1));

    let second = carrier(
        programs(
            package,
            "1.0.0",
            shell_program(
                &sandbox,
                "respond",
                "second",
                &sandbox.instance_root("second"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = ExtensionHost::with_journal(second.clone(), contract_range(), Arc::new(journal))
        .expect("journal host");
    let predecessors = host.pending_predecessors();
    assert_eq!(
        predecessors.len(),
        1,
        "the pointer survives as a predecessor"
    );
    let predecessor_id = predecessors[0].instance_id.clone();

    // The same package and permission scope stay blocked until the predecessor
    // is explicitly confirmed.
    let failure = activate(&host, package, &["acme.journal/run"])
        .expect_err("an unconfirmed predecessor blocks its scope");
    assert_eq!(failure.code, "extension_predecessor_unreconciled");
    host.confirm_session_owner_stopped(&predecessor_id)
        .expect("explicit confirmation");
    assert!(host.pending_predecessors().is_empty());

    let receipt =
        activate(&host, package, &["acme.journal/run"]).expect("activate after confirmation");
    assert_eq!(
        receipt.generation, 3,
        "the blocked attempt consumed a label too: the watermark never hands one back"
    );
    let call = host
        .begin("acme.journal/run", &json!({"input": "again"}))
        .expect("admit");
    assert_eq!(
        finished_payload(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("second")
    );
}

#[test]
fn the_host_environment_is_not_inherited_by_the_extension() {
    let sandbox = Sandbox::new("env");
    let instance = sandbox.instance_root("env");
    let program = shell_program(&sandbox, "env", "", &instance)
        .with_env("LICOUP_TEST_MARKER", "declared-by-the-program");
    let carrier = carrier(
        programs("acme.env/ext", "1.0.0", program),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    activate(&host, "acme.env/ext", &["acme.env/run"]).expect("activate");
    let call = host
        .begin("acme.env/run", &json!({"input": "env"}))
        .expect("admit");
    settle(&host, &call.binding, Duration::from_secs(5));

    let environment =
        read_text(&instance.join("env.txt")).expect("the fixture wrote its environment");
    let marker = environment
        .lines()
        .find(|line| line.starts_with("LICOUP_TEST_MARKER="))
        .expect("a program's own declaration reaches it");
    assert!(
        marker.contains("declared-by-the-program"),
        "the declared value is carried: {marker}"
    );
    let home = environment
        .lines()
        .find(|line| line.starts_with("HOME="))
        .expect("a home inside the instance root");
    assert!(
        home.contains("instances/env"),
        "home is the instance root: {home}"
    );
    let tmpdir = environment
        .lines()
        .find(|line| line.starts_with("TMPDIR="))
        .expect("a temporary directory is provided");
    assert!(
        tmpdir.contains("instances/env"),
        "tmpdir is the instance root"
    );
    assert!(
        !environment.contains("CARGO_MANIFEST_DIR"),
        "the host's own environment is not inherited"
    );
}

#[test]
fn the_published_reference_sdk_sample_is_served_over_a_real_pipe() {
    let python = python_runtime();
    let sample = crate::support::reference_sample();
    assert!(
        sample.is_file(),
        "UNVERIFIABLE: required reference SDK sample is absent"
    );
    let sandbox = Sandbox::new("sdk-sample");
    let instance = sandbox.instance_root("sample");
    let mut program = ResolvedProgram::new(python.executable.clone())
        .with_args(vec!["-B".to_owned(), sample.display().to_string()])
        .with_read_root(sample.parent().expect("sample directory"))
        .with_read_root(crate::support::reference_sdk_dir())
        .with_read_root(&python.runtime_root)
        .with_write_root(&instance)
        .with_env("PYTHONDONTWRITEBYTECODE", "1");
    if let Some(app) = &python.framework_app {
        program = program.with_exec_path(app);
    }
    let carrier = carrier(
        programs("dev.example.agent.sdk.minimal", "1.0.0", program),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 15_000,
            ..Default::default()
        }),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(
        &host,
        "dev.example.agent.sdk.minimal",
        &["dev.example.agent/stream"],
    )
    .expect("the published sample activates");
    let call = host
        .begin(
            "dev.example.agent/stream",
            &json!({"input": "第一行 plain text\nsecond line"}),
        )
        .expect("admit");
    let outcome = settle(&host, &call.binding, Duration::from_secs(15));
    assert_eq!(
        payload_of(outcome),
        json!({"outcome": "succeeded"}),
        "the sample's own terminal body is the settled payload"
    );
    let facts = carrier.facts(&receipt.instance_id).expect("facts");
    assert_eq!(facts.terminal_events.load(Ordering::SeqCst), 1);
    assert!(facts.bytes_stdout() > 0, "the wire really carried frames");
}

#[test]
fn a_program_with_no_registered_source_is_refused_before_anything_starts() {
    let sandbox = Sandbox::new("nosource");
    let carrier = carrier(
        programs(
            "acme.known/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "respond",
                "known",
                &sandbox.instance_root("known"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    let failure = activate(&host, "acme.unknown/ext", &["acme.unknown/run"])
        .expect_err("an unknown package has no program");
    assert_eq!(failure.code, "extension_program_unavailable");
    assert!(
        carrier.facts("instance-1").is_none(),
        "no process was started"
    );
    assert!(
        !carrier
            .ledger()
            .grants()
            .expect("grants")
            .contains_key("instance-1"),
        "nothing was granted"
    );
}
