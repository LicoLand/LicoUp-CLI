//! Direct boundary checks of the process boundary itself.
//!
//! Three questions, each answered with real files or real processes rather than
//! with prose:
//!
//! 1. **Who owns the environment.** A restricted program may declare its own
//!    variables, but not the host-owned ones (home, temporary directory, working
//!    directory, path, loader and interpreter control). Declaring one is refused
//!    before a process exists — never ignored, never overridden silently. A
//!    trusted local program is the user's own software and may declare them; that
//!    difference is asserted in both directions.
//! 2. **Group, not tree.** What is reclaimed is the supervised process group. A
//!    descendant that calls `setsid` leaves the group and is *not* reclaimed:
//!    the capability report says so, the release record says the instance's
//!    stdout was still held, and the probe's own bounded exit keeps the test
//!    clean. The probe also shows whether the sandbox still confines such a
//!    descendant (it does) — confinement and reclamation are different promises.
//! 3. **The record survives a crash.** A genuinely unterminated tail is
//!    terminated before the next append, so no later record can be concatenated
//!    onto a damaged one; a damaged complete line still fails closed instead of
//!    being skipped. A grant that cannot be written stops the instance with the
//!    pid and the *observed* exit preserved — a kill request is never an exit.

use std::path::Path;
use std::time::Duration;

use licoup_native::platform::extension_host::ExtensionCarrier;
use licoup_native::platform::extension_host::isolation::{
    EnforcedLimits, IsolationLedger, IsolationMode, IsolationRecord, LimitScope, NetworkGrant,
    ObservedExit, PlatformConfinement, ReleaseScope, ResolvedProgram, ResourceDeclaration,
    ResourceLimits, RevocationReason, Support,
};
use serde_json::json;

use crate::support::{
    Sandbox, activate, carrier, carrier_spec, host_of, payload_of, pid_alive, policy, programs,
    python_program, python_runtime, read_text, reference_sample, reference_sdk_dir, settle,
    shell_program, wait_for,
};

/// A declaration for the record tests, without any process behind it.
fn declaration(instance_id: &str) -> ResourceDeclaration {
    ResourceDeclaration {
        package_id: "acme.ledger/ext".to_owned(),
        package_version: "1.0.0".to_owned(),
        instance_id: instance_id.to_owned(),
        generation: 1,
        mode: IsolationMode::TrustedLocal,
        confinement: Support::unavailable("test declaration"),
        read_roots: vec!["/tmp/synthetic-fixture-root".to_owned()],
        write_root: "/tmp/synthetic-fixture-root/instances/one".to_owned(),
        executable: "/bin/bash".to_owned(),
        exec_paths: vec!["/bin/bash".to_owned()],
        network: NetworkGrant::Denied,
        limits: ResourceLimits::default(),
        enforced: EnforcedLimits {
            call_wall_ms: 2_000,
            max_frame_bytes: 64 * 1024,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 64 * 1024,
            cpu_seconds: None,
            file_bytes: None,
            open_files: None,
            address_space_bytes: None,
            core_dumps_disabled: true,
        },
        limit_scope: LimitScope::Process,
        declared_at_unix_ms: 0,
    }
}

fn descendant_outcome(instance: &Path) -> String {
    let path = instance.join("descendant.outcome");
    // Wait for the content, not just the file: the child creates and writes in
    // one step and a reader can catch the empty window.
    assert!(
        wait_for(Duration::from_secs(8), || {
            read_text(&path).is_some_and(|text| !text.trim().is_empty())
        }),
        "the descendant probe reported its outcome"
    );
    read_text(&path).expect("descendant outcome")
}

fn root_outcome(instance: &Path) -> String {
    let path = instance.join("root.outcome");
    assert!(
        wait_for(Duration::from_secs(8), || {
            read_text(&path).is_some_and(|text| !text.trim().is_empty())
        }),
        "the probe reported its own outcome"
    );
    read_text(&path).expect("root outcome")
}

fn descendant_pid(outcome: &str) -> u32 {
    outcome
        .split_whitespace()
        .next()
        .expect("pid")
        .parse()
        .expect("pid")
}

/// The descendant program: forked child in `mode`, bounded hold, optional write
/// outside the roots.
fn descendant_program(
    sandbox: &Sandbox,
    python: &crate::support::PythonRuntime,
    instance: &Path,
    mode: &str,
    hold: &str,
    outside: Option<&Path>,
) -> ResolvedProgram {
    let mut args = vec![
        "-B".to_owned(),
        sandbox.group_escape_script().display().to_string(),
        mode.to_owned(),
        hold.to_owned(),
    ];
    if let Some(outside) = outside {
        args.push(outside.display().to_string());
    }
    python_program(python, args, sandbox, instance)
}

#[cfg(target_os = "macos")]
#[test]
fn a_restricted_program_may_not_redeclare_host_owned_environment() {
    let sandbox = Sandbox::new("env-owner");
    let reserved = [
        "HOME",
        "TMPDIR",
        "PWD",
        "PATH",
        "DYLD_INSERT_LIBRARIES",
        "LD_PRELOAD",
        "PYTHONPATH",
        "NODE_OPTIONS",
    ];
    for key in reserved {
        let package = format!(
            "acme.reserved.{}",
            key.to_ascii_lowercase().replace('_', "-")
        );
        let instance = sandbox.instance_root(&format!("reserved-{}", key.to_ascii_lowercase()));
        let program =
            shell_program(&sandbox, "respond", "reserved", &instance).with_env(key, "/outside");
        let carrier = carrier(
            programs(&package, "1.0.0", program),
            policy(IsolationMode::Restricted, sandbox.root()),
        );
        let host = host_of(carrier.clone());
        let failure = activate(&host, &package, &["acme.reserved/run"])
            .expect_err("a host-owned variable is refused before anything starts");
        assert_eq!(
            failure.code, "extension_isolation_env_reserved",
            "for {key}"
        );
        assert_eq!(failure.presentation_args.get("variable"), Some(key));
        assert_eq!(failure.presentation_args.get("mode"), Some("restricted"));
        assert!(
            carrier.facts("instance-1").is_none(),
            "no process was started for {key}"
        );
    }

    // A declaration the host does not own still reaches the program.
    let program = shell_program(
        &sandbox,
        "respond",
        "benign",
        &sandbox.instance_root("benign"),
    )
    .with_env("LICOUP_TEST_MARKER", "declared-by-the-program");
    let carrier = carrier(
        programs("acme.benign/ext", "1.0.0", program),
        policy(IsolationMode::Restricted, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    let receipt =
        activate(&host, "acme.benign/ext", &["acme.benign/run"]).expect("a benign declaration");
    let call = host
        .begin("acme.benign/run", &json!({"input": "go"}))
        .expect("admit");
    assert_eq!(
        payload_of(settle(&host, &call.binding, Duration::from_secs(8)))["marker"],
        json!("benign")
    );
    assert!(carrier.facts(&receipt.instance_id).is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn a_restricted_probe_sees_host_owned_environment_pinned() {
    let sandbox = Sandbox::new("env-pinned");
    let instance = sandbox.instance_root("envpinned");
    let program = shell_program(&sandbox, "env", "", &instance)
        .with_env("LICOUP_TEST_MARKER", "declared-by-the-program");
    let carrier = carrier(
        programs("acme.envpinned/ext", "1.0.0", program),
        policy(IsolationMode::Restricted, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    activate(&host, "acme.envpinned/ext", &["acme.envpinned/run"]).expect("activate");
    let call = host
        .begin("acme.envpinned/run", &json!({"input": "env"}))
        .expect("admit");
    settle(&host, &call.binding, Duration::from_secs(8));

    let environment =
        read_text(&instance.join("env.txt")).expect("the fixture wrote its environment");
    for key in ["HOME", "TMPDIR", "PWD"] {
        let line = environment
            .lines()
            .find(|line| line.starts_with(&format!("{key}=")))
            .unwrap_or_else(|| panic!("{key} is present"));
        assert!(
            line.contains("instances/envpinned"),
            "{key} is pinned to the instance root: {line}"
        );
    }
    let marker = environment
        .lines()
        .find(|line| line.starts_with("LICOUP_TEST_MARKER="))
        .expect("the declared marker is present");
    assert!(marker.contains("declared-by-the-program"));
    let path = environment
        .lines()
        .find(|line| line.starts_with("PATH="))
        .expect("the host provides a path");
    assert!(
        path.len() > "PATH=".len() + 1,
        "the host path is not empty: {path}"
    );
}

#[test]
fn a_trusted_local_program_may_declare_its_own_environment() {
    let sandbox = Sandbox::new("env-trusted");
    let instance = sandbox.instance_root("trusted-env");
    let home = instance.join("home");
    std::fs::create_dir_all(&home).expect("declared home");
    let program =
        shell_program(&sandbox, "env", "", &instance).with_env("HOME", home.display().to_string());
    let carrier = carrier(
        programs("acme.trusted.env/ext", "1.0.0", program),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    activate(&host, "acme.trusted.env/ext", &["acme.trusted.env/run"]).expect("activate");
    let call = host
        .begin("acme.trusted.env/run", &json!({"input": "env"}))
        .expect("admit");
    settle(&host, &call.binding, Duration::from_secs(8));

    let environment = read_text(&instance.join("env.txt")).expect("the fixture wrote its");
    let home = environment
        .lines()
        .find(|line| line.starts_with("HOME="))
        .expect("HOME is present");
    assert!(
        home.contains("trusted-env/home"),
        "a trusted local program keeps its own declaration: {home}"
    );
}

#[test]
fn an_escaped_descendant_is_reported_and_never_claimed_as_reclaimed() {
    let python = python_runtime();
    let sandbox = Sandbox::new("descendant-escape");
    let instance = sandbox.instance_root("escape");
    let program = descendant_program(&sandbox, &python, &instance, "escape", "6", None);
    let carrier = carrier(
        programs("acme.escape.desc/ext", "1.0.0", program),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 800,
            ..Default::default()
        }),
    );
    let session = carrier
        .start(&carrier_spec(
            "acme.escape.desc/ext",
            "1.0.0",
            "descendant-escape",
            1,
        ))
        .expect("start");
    let outcome = descendant_outcome(&instance);
    eprintln!("descendant outcome: {}", outcome.trim_end());
    assert!(
        outcome.contains("setsid-ok"),
        "the descendant really left the group: {outcome}"
    );
    let descendant = descendant_pid(&outcome);
    let facts = carrier.facts("descendant-escape").expect("facts");

    // The supervised process is released with an observed exit, and that is all
    // the carrier can prove: a trusted local program's descendants are outside
    // the group, so the release is scoped and the owner must stay unverified.
    let failure = carrier
        .shutdown(&session)
        .expect_err("a trusted local release is scoped, not complete");
    assert_eq!(failure.code, "extension_isolation_descendants_unverified");
    assert_eq!(
        failure.presentation_args.get("scope"),
        Some("process-group")
    );
    assert_eq!(
        failure.presentation_args.get("stdoutPipeHeld"),
        Some("true")
    );
    assert!(facts.released(), "the observed release is recorded");
    let exit = facts.exit().expect("the exit was observed");
    assert!(
        !facts.release_was_forced(),
        "the root exited on its own: {exit:?}"
    );
    assert!(!facts.release_unconfirmed());
    assert_eq!(facts.release_scope(), Some(ReleaseScope::ProcessGroup));
    assert!(facts.pipe_held_after_release());
    assert!(
        pid_alive(descendant),
        "the descendant was never signalled by the group kill"
    );

    // A retry keeps the scoped verdict: the evidence must not become a verified
    // release just because the same wait runs again.
    let again = carrier
        .shutdown(&session)
        .expect_err("a retry stays scoped");
    assert_eq!(again.code, "extension_isolation_descendants_unverified");

    let records = carrier.ledger().read().expect("records");
    let releases: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            IsolationRecord::Release {
                stdout_pipe_held,
                release_scope,
                ..
            } => Some((*stdout_pipe_held, *release_scope)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "one release, recorded once");
    assert!(releases[0].0, "the record names the unreclaimed writer");
    assert_eq!(releases[0].1, ReleaseScope::ProcessGroup);

    // The probe ends itself inside its bounded window, so the test leaves
    // nothing behind. This is the probe's fallback, not host reclamation.
    assert!(
        wait_for(Duration::from_secs(12), || !pid_alive(descendant)),
        "the probe's own fallback ended it"
    );
}

#[test]
fn a_grouped_descendant_is_reclaimed_and_still_scoped() {
    let python = python_runtime();
    let sandbox = Sandbox::new("descendant-group");
    let instance = sandbox.instance_root("grouped");
    let program = descendant_program(&sandbox, &python, &instance, "grouped", "6", None);
    let carrier = carrier(
        programs("acme.grouped.desc/ext", "1.0.0", program),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 800,
            ..Default::default()
        }),
    );
    let session = carrier
        .start(&carrier_spec(
            "acme.grouped.desc/ext",
            "1.0.0",
            "descendant-grouped",
            1,
        ))
        .expect("start");
    let outcome = descendant_outcome(&instance);
    assert!(
        outcome.contains(" grouped"),
        "the descendant stayed in its group: {outcome}"
    );
    let descendant = descendant_pid(&outcome);

    let facts = carrier.facts("descendant-grouped").expect("facts");
    let failure = carrier
        .shutdown(&session)
        .expect_err("the group scope is still not the whole instance");
    assert_eq!(failure.code, "extension_isolation_descendants_unverified");
    assert!(facts.released());
    assert!(
        !facts.pipe_held_after_release(),
        "the group was signalled and the pipe closed"
    );
    assert!(
        !pid_alive(descendant),
        "a descendant that stays in the group is reclaimed"
    );
    // A closed pipe is residue evidence only: the owner still stays unverified,
    // because a descendant that closed stdio could have survived unnoticed.
    assert_eq!(facts.release_scope(), Some(ReleaseScope::ProcessGroup));
}

#[cfg(target_os = "macos")]
#[test]
fn a_restricted_instance_cannot_create_descendants() {
    let python = python_runtime();
    let sandbox = Sandbox::new("descendant-confined");
    let instance = sandbox.instance_root("escape-confined");
    let outside = sandbox.outside_path("escaped-write.txt");
    let program = descendant_program(&sandbox, &python, &instance, "escape", "2", Some(&outside));
    let carrier = carrier(
        programs("acme.confined.desc/ext", "1.0.0", program),
        policy(IsolationMode::Restricted, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 800,
            ..Default::default()
        }),
    );
    let session = carrier
        .start(&carrier_spec(
            "acme.confined.desc/ext",
            "1.0.0",
            "descendant-confined",
            1,
        ))
        .expect("start");
    let root = root_outcome(&instance);
    eprintln!("restricted root outcome: {}", root.trim_end());
    assert!(
        root.contains("fork-denied"),
        "the confined process cannot create a descendant: {root}"
    );
    assert!(
        root.contains("write-denied"),
        "and it is still confined: {root}"
    );
    assert!(
        !instance.join("descendant.outcome").exists(),
        "no descendant exists to escape the group"
    );
    assert!(!outside.exists(), "nothing was written outside the roots");

    let facts = carrier.facts("descendant-confined").expect("facts");
    carrier
        .shutdown(&session)
        .expect("a single-process instance is released completely");
    assert_eq!(facts.release_scope(), Some(ReleaseScope::Instance));
    assert!(!facts.pipe_held_after_release());
    let records = carrier.ledger().read().expect("records");
    let scope = records
        .iter()
        .find_map(|record| match record {
            IsolationRecord::Release { release_scope, .. } => Some(*release_scope),
            _ => None,
        })
        .expect("a release record");
    assert_eq!(scope, ReleaseScope::Instance);
}

#[test]
fn a_restricted_program_that_needs_descendants_is_refused() {
    let sandbox = Sandbox::new("descendants-needed");
    let program = shell_program(
        &sandbox,
        "respond",
        "needs-children",
        &sandbox.instance_root("needs-children"),
    )
    .with_requires_descendants();

    let restricted = carrier(
        programs("acme.needs.children/ext", "1.0.0", program.clone()),
        policy(IsolationMode::Restricted, sandbox.root()),
    );
    let restricted_host = host_of(restricted.clone());
    let failure = activate(
        &restricted_host,
        "acme.needs.children/ext",
        &["acme.needs.children/run"],
    )
    .expect_err("a restricted instance is one process");
    assert_eq!(failure.code, "extension_isolation_descendants_unsupported");
    assert!(
        restricted.facts("instance-1").is_none(),
        "nothing was created"
    );

    // The same declaration is honest in trusted local mode, where descendants
    // are allowed and the release is scoped accordingly.
    let trusted = carrier(
        programs("acme.needs.children/ext", "1.0.0", program),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(trusted.clone());
    activate(
        &host,
        "acme.needs.children/ext",
        &["acme.needs.children/run"],
    )
    .expect("trusted local allows descendants");
    assert!(trusted.facts("instance-1").is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn the_reference_runtime_runs_confined_without_descendants() {
    let python = python_runtime();
    let sample = reference_sample();
    assert!(
        sample.is_file(),
        "UNVERIFIABLE: required reference SDK sample is absent"
    );
    let sandbox = Sandbox::new("sdk-confined");
    let instance = sandbox.instance_root("sdk-sample");
    let program = python_program(
        &python,
        vec!["-B".to_owned(), sample.display().to_string()],
        &sandbox,
        &instance,
    )
    .with_read_root(sample.parent().expect("sample directory"))
    .with_read_root(reference_sdk_dir());
    let carrier = carrier(
        programs("dev.example.agent.sdk.confined", "1.0.0", program),
        policy(IsolationMode::Restricted, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 15_000,
            ..Default::default()
        }),
    );
    let journal =
        licoup_native::platform::extension_host::RuntimeCatalogJournal::open(sandbox.root())
            .expect("catalogue record");
    let host = licoup_native::platform::extension_host::ExtensionHost::with_journal(
        carrier.clone(),
        crate::support::contract_range(),
        std::sync::Arc::new(journal),
    )
    .expect("journal host");
    let receipt = activate(
        &host,
        "dev.example.agent.sdk.confined",
        &["dev.example.agent/stream"],
    )
    .expect("the published sample activates confined");
    let call = host
        .begin(
            "dev.example.agent/stream",
            &json!({"input": "confined reference run"}),
        )
        .expect("admit");
    assert_eq!(
        payload_of(settle(&host, &call.binding, Duration::from_secs(15)))["outcome"],
        json!("succeeded")
    );

    // A restricted instance is one process, so its drain is a complete release:
    // the owner is verified and the record says the limits cover the instance.
    assert_eq!(host.revoke("dev.example.agent.sdk.confined"), 1);
    let facts = carrier.facts(&receipt.instance_id).expect("facts");
    assert_eq!(facts.release_scope(), Some(ReleaseScope::Instance));
    assert!(!facts.pipe_held_after_release());
    assert!(host.unverified_session_owners().is_empty());
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Stopped
    );
    let grant = carrier
        .ledger()
        .grants()
        .expect("grants")
        .get(&receipt.instance_id)
        .cloned()
        .expect("a grant");
    assert_eq!(grant.limit_scope, LimitScope::Instance);

    // A verified release clears the durable pointer; the trusted local scope
    // keeps it as an unconfirmed predecessor (the journal test covers that side).
    drop(host);
    let journal =
        licoup_native::platform::extension_host::RuntimeCatalogJournal::open(sandbox.root())
            .expect("reopen");
    use licoup_native::platform::extension_host::CatalogJournal;
    let watermark = journal.watermark().expect("watermark");
    assert!(
        watermark.active.is_empty(),
        "a single-process instance leaves no unconfirmed predecessor"
    );
    assert_eq!(
        watermark.generations.get("dev.example.agent.sdk.confined"),
        Some(&1)
    );
}

#[test]
fn the_report_names_the_process_group_boundary() {
    let confinement = PlatformConfinement::detect();
    #[cfg(unix)]
    assert!(confinement.process_group.is_enforced());
    #[cfg(target_os = "macos")]
    assert!(
        confinement.single_process.is_enforced(),
        "the restricted mode's completeness rests on this mechanism"
    );
    #[cfg(not(target_os = "macos"))]
    assert!(
        !confinement.supports_restricted(),
        "without single-process enforcement a restricted run cannot be honoured"
    );
    assert!(
        !confinement.group_escape.is_enforced(),
        "no mechanism reclaims a descendant that left the group"
    );
    let reason = confinement
        .group_escape
        .reason()
        .expect("the boundary is reported, not hidden");
    assert!(
        reason.contains("setsid") || reason.contains("group"),
        "the reason names the boundary: {reason}"
    );
}

#[test]
fn a_half_written_record_never_swallows_the_next_one() {
    let sandbox = Sandbox::new("ledger-tail");
    let ledger = IsolationLedger::open(sandbox.root()).expect("ledger");
    let path = ledger.path().to_path_buf();
    // A crash left half a record: bytes, no newline.
    let half = "{\"record\":\"grant\",\"declaration\":{\"packageId\":\"acme.ledger";
    std::fs::write(&path, half).expect("crash residue");

    let declaration = declaration("instance-tail");
    ledger
        .record_grant(&declaration)
        .expect("grant after a damaged tail");
    ledger
        .record_revocation(
            "acme.ledger/ext",
            Some("instance-tail"),
            Some(1),
            RevocationReason::UserWithdrawn,
        )
        .expect("revocation");
    ledger
        .record_release(
            &declaration,
            ObservedExit::success(),
            false,
            false,
            ReleaseScope::ProcessGroup,
        )
        .expect("release");

    // Reopening repairs nothing further and the damaged line fails closed
    // instead of being skipped.
    let reopened = IsolationLedger::open(sandbox.root()).expect("reopen");
    let failure = reopened
        .read()
        .expect_err("a damaged complete line fails closed");
    assert_eq!(failure.code, "extension_isolation_record_corrupt");

    // Every later record is on its own line and intact: the damaged tail was
    // terminated, never merged with the next record.
    let raw = read_text(&path).expect("raw record");
    assert!(
        raw.starts_with(&format!("{half}\n")),
        "the damaged line is preserved"
    );
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 4, "one damaged line and three records");
    assert!(
        serde_json::from_str::<IsolationRecord>(lines[0]).is_err(),
        "the damaged line is not a record"
    );
    let records: Vec<IsolationRecord> = lines[1..]
        .iter()
        .map(|line| serde_json::from_str(line).expect("each later record is intact"))
        .collect();
    assert!(matches!(records[0], IsolationRecord::Grant { .. }));
    assert!(matches!(records[1], IsolationRecord::Revocation { .. }));
    assert!(matches!(records[2], IsolationRecord::Release { .. }));
}

#[test]
fn a_corrupt_complete_record_fails_closed_without_skipping() {
    use std::io::Write;

    let sandbox = Sandbox::new("ledger-corrupt");
    let ledger = IsolationLedger::open(sandbox.root()).expect("ledger");
    let path = ledger.path().to_path_buf();
    let declaration = declaration("instance-corrupt");
    ledger.record_grant(&declaration).expect("grant");
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append the damaged line");
        file.write_all(b"{\"record\":\"grant\",\"declaration\":{\n")
            .expect("damaged line");
    }
    ledger
        .record_release(
            &declaration,
            ObservedExit::success(),
            false,
            false,
            ReleaseScope::ProcessGroup,
        )
        .expect("release after the damaged line");

    let failure = ledger.read().expect_err("the damaged line is not skipped");
    assert_eq!(failure.code, "extension_isolation_record_corrupt");
    assert!(
        ledger.grants().is_err(),
        "no partial map is served from a damaged record"
    );
    // Reading never mutates the record.
    let before = read_text(&path).expect("raw");
    let _ = ledger.read();
    assert_eq!(read_text(&path).expect("raw"), before);
}

#[test]
fn a_grant_record_failure_preserves_real_process_evidence() {
    let sandbox = Sandbox::new("ledger-failure");
    // Force the append to fail: the record path is a directory.
    std::fs::create_dir_all(sandbox.root().join("isolation").join("grants.jsonl"))
        .expect("blocking directory");
    let carrier = carrier(
        programs(
            "acme.blocked/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "respond",
                "blocked",
                &sandbox.instance_root("blocked"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    let failure = activate(&host, "acme.blocked/ext", &["acme.blocked/run"])
        .expect_err("the grant cannot be recorded");

    // The record failure is the primary refusal and names what failed underneath.
    assert_eq!(failure.code, "extension_isolation_record_unavailable");
    assert_eq!(
        failure.presentation_args.get("storageFailure"),
        Some("package_journal_unavailable")
    );
    // The process evidence travels with it: pid and the observed exit.
    let facts = carrier.facts("instance-1").expect("the pid is preserved");
    let pid_text = facts.pid.to_string();
    assert_eq!(
        failure.presentation_args.get("processPid"),
        Some(pid_text.as_str())
    );
    assert!(
        failure
            .presentation_args
            .get("processExitObserved")
            .is_some(),
        "the refusal names the exit it observed"
    );
    let exit = facts.exit().expect("the exit was really observed");
    assert!(
        exit.signal.is_some() || exit.success,
        "the observed exit is real: {exit:?}"
    );
    assert!(!pid_alive(facts.pid));
    assert!(!facts.released(), "no release was recorded");
    assert!(
        !facts.release_unconfirmed(),
        "the teardown observed the exit"
    );
}
