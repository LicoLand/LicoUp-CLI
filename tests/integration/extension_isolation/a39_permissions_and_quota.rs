//! A39 at `component-integration`: extension permissions, sources, quotas and
//! the honest boundary of what this host can enforce.
//!
//! The cases here use real operating-system controls: a real seatbelt profile
//! that denies a write outside the instance root, a real listener that a
//! confined probe cannot reach, real `RLIMIT_CPU`/`RLIMIT_FSIZE` ceilings, and
//! the record that keeps the granted envelope. Where this host has no control,
//! the case asserts the refusal instead of a fabricated pass.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use licoup_native::platform::extension_host::ExtensionCarrier;
use licoup_native::platform::extension_host::isolation::{
    IsolationMode, IsolationRecord, LimitScope, NetworkGrant, PlatformConfinement, ReleaseScope,
    ResolvedProgram, ResourceLimits, RevocationReason, StaticPrograms,
};
use serde_json::json;

use crate::support::{
    Sandbox, activate, carrier, carrier_result, carrier_spec, failure_of, host_of, payload_of,
    pid_alive, policy, programs, python_program, python_runtime, read_text, settle, shell_program,
    wait_for,
};

#[test]
fn the_capability_report_matches_what_this_host_can_enforce() {
    let confinement = PlatformConfinement::detect();
    #[cfg(unix)]
    {
        assert!(confinement.process_group.is_enforced());
        #[cfg(target_os = "macos")]
        assert!(
            confinement.single_process.is_enforced(),
            "a restricted instance is forced to stay one process"
        );
        assert!(confinement.cpu_seconds.is_enforced());
        assert!(confinement.file_bytes.is_enforced());
        assert!(confinement.open_files.is_enforced());
        assert!(confinement.core_dumps.is_enforced());
    }
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            confinement.filesystem_scopes.mechanism(),
            Some("macos-seatbelt")
        );
        assert_eq!(confinement.network_deny.mechanism(), Some("macos-seatbelt"));
        assert!(confinement.supports_restricted());
        assert!(
            !confinement.address_space.is_enforced(),
            "darwin ignores RLIMIT_AS, so no memory ceiling is claimed"
        );
        assert!(confinement.address_space.reason().is_some());
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert!(
            !confinement.supports_restricted(),
            "this build implements filesystem confinement for macOS only"
        );
        let sandbox = Sandbox::new("unavailable");
        let refused = carrier_result(
            programs(
                "acme.unavailable/ext",
                "1.0.0",
                shell_program(&sandbox, "respond", "x", &sandbox.instance_root("x")),
            ),
            policy(IsolationMode::Restricted, sandbox.root()),
        )
        .err()
        .expect("a restricted request is refused rather than downgraded");
        assert_eq!(refused.code, "extension_isolation_unavailable");
        assert_eq!(refused.presentation_args.get("mode"), Some("restricted"));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_restricted_extension_cannot_leave_its_declared_roots() {
    let python = python_runtime();
    let sandbox = Sandbox::new("escape");
    let instance = sandbox.instance_root("escape");
    let outside_write = sandbox.outside_path("escape-probe.txt");
    let outside_read = sandbox.outside_file("synthetic-secret.txt", "synthetic-outside-content\n");
    let program = python_program(
        &python,
        vec![
            "-B".to_owned(),
            sandbox.root_probe_script().display().to_string(),
            outside_write.display().to_string(),
            outside_read.display().to_string(),
        ],
        &sandbox,
        &instance,
    );
    // The probe is not a protocol agent: it runs, reports through its own file
    // and exits. The carrier is driven directly, as the confinement unit.
    let carrier = carrier(
        programs("acme.escape/ext", "1.0.0", program),
        policy(IsolationMode::Restricted, sandbox.root()),
    );
    let session = carrier
        .start(&carrier_spec("acme.escape/ext", "1.0.0", "escape", 1))
        .expect("start");
    assert!(
        wait_for(Duration::from_secs(10), || {
            read_text(&instance.join("probe-result.txt"))
                .is_some_and(|marker| !marker.trim().is_empty())
        }),
        "the probe reported its result"
    );
    let marker = read_text(&instance.join("probe-result.txt")).expect("probe result");
    assert!(
        marker.contains("write-denied"),
        "a write outside the declared root is denied: {marker}"
    );
    assert!(
        marker.contains("read-denied"),
        "a read outside the declared roots is denied: {marker}"
    );
    assert!(
        marker.contains("inside-wrote"),
        "the instance's own root stays writable: {marker}"
    );
    assert!(
        !outside_write.exists(),
        "nothing was created outside the root"
    );
    assert_eq!(
        read_text(&outside_read).as_deref(),
        Some("synthetic-outside-content\n"),
        "the outside file was not modified"
    );

    let facts = carrier.facts("escape").expect("facts");
    carrier
        .shutdown(&session)
        .expect("a single-process instance is released completely");
    assert_eq!(
        facts.confinement.mechanism(),
        Some("macos-seatbelt"),
        "the run reports the mechanism that was applied"
    );
    assert!(
        facts.exit().expect("the observed exit").success,
        "the probe exited cleanly"
    );
    assert_eq!(facts.release_scope(), Some(ReleaseScope::Instance));
    assert!(!facts.pipe_held_after_release());
    let grant = carrier
        .ledger()
        .grants()
        .expect("grants")
        .get("escape")
        .cloned()
        .expect("a grant");
    assert_eq!(
        grant.limit_scope,
        LimitScope::Instance,
        "one enforced process makes the per-process limits instance limits"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn network_is_denied_by_default_and_a_loopback_grant_is_scoped() {
    let python = python_runtime();
    let sandbox = Sandbox::new("net");
    let granted_listener = TcpListener::bind("127.0.0.1:0").expect("first listener");
    let granted_port = granted_listener.local_addr().expect("addr").port();
    let other_listener = TcpListener::bind("127.0.0.1:0").expect("second listener");
    let other_port = other_listener.local_addr().expect("addr").port();

    let probe = |port: u16, instance: &Path| -> ResolvedProgram {
        let mut program = ResolvedProgram::new(python.executable.clone())
            .with_args(vec![
                "-B".to_owned(),
                sandbox.net_probe().display().to_string(),
                port.to_string(),
            ])
            .with_read_root(sandbox.fixture_dir())
            .with_read_root(&python.runtime_root)
            .with_read_root("/usr/lib")
            .with_read_root("/System")
            .with_write_root(instance)
            .with_env("PYTHONDONTWRITEBYTECODE", "1");
        if let Some(app) = &python.framework_app {
            program = program.with_exec_path(app);
        }
        program
    };

    let run = |name: &str, port: u16, network: NetworkGrant| -> String {
        let instance = sandbox.instance_root(name);
        let carrier = carrier(
            programs("acme.net/ext", "1.0.0", probe(port, &instance)),
            policy(IsolationMode::Restricted, sandbox.root()).with_network(network),
        );
        let session = carrier
            .start(&carrier_spec("acme.net/ext", "1.0.0", name, 1))
            .expect("start");
        assert!(
            wait_for(Duration::from_secs(10), || {
                read_text(&instance.join("net-result.txt"))
                    .is_some_and(|result| !result.trim().is_empty())
            }),
            "the probe reported its result"
        );
        let result = read_text(&instance.join("net-result.txt")).expect("probe result");
        carrier.shutdown(&session).expect("observed release");
        result
    };

    // A real listener on this port exists; without a grant it stays unreachable.
    let denied = run("net-denied", granted_port, NetworkGrant::Denied);
    assert!(
        denied.starts_with("denied"),
        "a listening port is unreachable without a grant: {denied}"
    );

    // The grant reaches exactly the declared port...
    let granted = run(
        "net-granted",
        granted_port,
        NetworkGrant::Loopback { port: granted_port },
    );
    assert_eq!(granted, "connected", "the declared loopback grant works");

    // ...and nothing else.
    let scoped = run(
        "net-scoped",
        other_port,
        NetworkGrant::Loopback { port: granted_port },
    );
    assert!(
        scoped.starts_with("denied"),
        "a loopback grant is scoped to its port: {scoped}"
    );
}

#[cfg(unix)]
#[test]
fn cpu_and_file_ceilings_are_enforced_and_reported() {
    let sandbox = Sandbox::new("limits");

    let spin = carrier(
        programs(
            "acme.spin/ext",
            "1.0.0",
            shell_program(&sandbox, "spin", "", &sandbox.instance_root("spin")),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(ResourceLimits {
            // One second of CPU can take much longer than a second of wall time
            // on a loaded host; the wall bound stays well clear of it so the
            // fault class, not the clock, proves which ceiling applied.
            call_wall_ms: 30_000,
            cpu_seconds: Some(1),
            ..Default::default()
        }),
    );
    let host = host_of(spin.clone());
    let receipt = activate(&host, "acme.spin/ext", &["acme.spin/run"]).expect("activate");
    let started = std::time::Instant::now();
    let failure = match host.begin("acme.spin/run", &json!({"input": "spin"})) {
        Err(failure) => failure,
        Ok(call) => failure_of(&host, &call.binding, Duration::from_secs(40)),
    };
    let elapsed = started.elapsed();
    assert_eq!(
        failure.code, "extension_carrier_crashed",
        "the instance died with its wire, not on the wall clock: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(700),
        "the CPU ceiling applied before the process died: {elapsed:?}"
    );
    let facts = spin.facts(&receipt.instance_id).expect("facts");
    assert_eq!(facts.enforced.cpu_seconds, Some(1));
    assert!(
        facts.exit().expect("observed exit").signal.is_some(),
        "the operating system reported the signal that ended it"
    );
    assert!(!pid_alive(facts.pid));

    let bomb_root = sandbox.instance_root("bomb");
    let bomb = carrier(
        programs(
            "acme.bomb/ext",
            "1.0.0",
            shell_program(&sandbox, "writebomb", "writebomb", &bomb_root),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(ResourceLimits {
            call_wall_ms: 8_000,
            file_bytes: Some(64 * 1024),
            ..Default::default()
        }),
    );
    let host = host_of(bomb.clone());
    let receipt = activate(&host, "acme.bomb/ext", &["acme.bomb/run"]).expect("activate");
    let call = host
        .begin("acme.bomb/run", &json!({"input": "write"}))
        .expect("admit");
    assert_eq!(
        payload_of(settle(&host, &call.binding, Duration::from_secs(10)))["marker"],
        json!("writebomb")
    );
    let size = std::fs::metadata(bomb_root.join("bomb"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    assert!(
        size <= 64 * 1024,
        "the file ceiling bounded the write instead of the disk: {size}"
    );
    assert_eq!(
        bomb.facts(&receipt.instance_id)
            .expect("facts")
            .enforced
            .file_bytes,
        Some(64 * 1024)
    );
    // The ceilings are per process, and this is a trusted local instance, so the
    // grant says so instead of claiming an instance-wide quota.
    let grant = bomb
        .ledger()
        .grants()
        .expect("grants")
        .get(&receipt.instance_id)
        .cloned()
        .expect("a grant");
    assert_eq!(grant.limit_scope, LimitScope::Process);
}

#[test]
fn a_memory_ceiling_is_claimed_only_where_the_platform_enforces_it() {
    let sandbox = Sandbox::new("memory");
    let requested = ResourceLimits {
        address_space_bytes: Some(256 * 1024 * 1024),
        ..Default::default()
    };
    let result = carrier_result(
        programs(
            "acme.mem/ext",
            "1.0.0",
            shell_program(&sandbox, "respond", "mem", &sandbox.instance_root("mem")),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_limits(requested),
    );
    #[cfg(target_os = "macos")]
    {
        let failure = result
            .err()
            .expect("darwin ignores RLIMIT_AS, so the request is refused");
        assert_eq!(failure.code, "extension_isolation_limit_unsupported");
        assert_eq!(failure.field.as_deref(), Some("addressSpaceBytes"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = result.expect("this host reports the ceiling it can enforce");
    }
}

#[test]
fn the_grant_and_its_revocation_are_recorded_without_retracting_effects() {
    let sandbox = Sandbox::new("record");
    let carrier = carrier(
        programs(
            "acme.record/ext",
            "1.0.0",
            shell_program(
                &sandbox,
                "delayed",
                "recorded",
                &sandbox.instance_root("record"),
            ),
        ),
        policy(IsolationMode::TrustedLocal, sandbox.root()).with_network(NetworkGrant::Denied),
    );
    let host = host_of(carrier.clone());
    let receipt = activate(&host, "acme.record/ext", &["acme.record/run"]).expect("activate");

    // The granted envelope is recorded before the handshake.
    let declaration = carrier
        .ledger()
        .grants()
        .expect("grants")
        .get(&receipt.instance_id)
        .cloned()
        .expect("a grant for the instance");
    assert_eq!(declaration.generation, receipt.generation);
    assert_eq!(declaration.mode, IsolationMode::TrustedLocal);
    assert_eq!(declaration.network, NetworkGrant::Denied);
    assert!(
        declaration.confinement.reason().is_some(),
        "trusted local never claims OS confinement"
    );
    assert_eq!(declaration.enforced.cpu_seconds, None);
    assert_eq!(declaration.enforced.file_bytes, None);
    assert!(PathBuf::from(&declaration.write_root).ends_with(Path::new("instances/record")));
    assert!(declaration.read_root_count() >= 1);

    let call = host
        .begin("acme.record/run", &json!({"input": "long"}))
        .expect("admit");
    carrier
        .revoke(
            "acme.record/ext",
            Some(&receipt.instance_id),
            RevocationReason::FaultedTransport,
        )
        .expect("revocation record");
    host.revoke("acme.record/ext");

    // Withdrawn for new admission; the admitted call still settles where it was
    // admitted, and the effect already under way is not reported as undone.
    let failure = host
        .begin("acme.record/run", &json!({}))
        .expect_err("withdrawn admission");
    assert_eq!(failure.presentation_args.get("reason"), Some("withdrawn"));
    assert_eq!(
        payload_of(settle(&host, &call.binding, Duration::from_secs(5)))["marker"],
        json!("recorded")
    );
    assert_eq!(
        carrier
            .facts(&receipt.instance_id)
            .expect("facts")
            .dispatched
            .load(Ordering::SeqCst),
        1
    );
    // The trusted local release covers the supervised process and its group, so
    // its owner stays unverified; the record ordering below is unaffected.
    assert!(!host.unverified_session_owners().is_empty());

    // The record is ordered and carries no effect claim.
    let records = carrier.ledger().read().expect("records");
    let kinds: Vec<&str> = records
        .iter()
        .map(|record| match record {
            IsolationRecord::Grant { .. } => "grant",
            IsolationRecord::Revocation { .. } => "revocation",
            IsolationRecord::Release { .. } => "release",
        })
        .collect();
    assert_eq!(kinds, vec!["grant", "revocation", "release"]);
    let reason = records
        .iter()
        .find_map(|record| match record {
            IsolationRecord::Revocation { reason, .. } => Some(*reason),
            _ => None,
        })
        .expect("revocation reason");
    assert_eq!(reason, RevocationReason::FaultedTransport);
    let release = records
        .iter()
        .find_map(|record| match record {
            IsolationRecord::Release { exit, forced, .. } => Some((*exit, *forced)),
            _ => None,
        })
        .expect("release");
    assert!(release.0.success);
}

#[test]
fn an_out_of_bounds_declaration_is_refused_before_any_process_starts() {
    let sandbox = Sandbox::new("bounds");
    let mut programs = StaticPrograms::new();
    let base = |instance: PathBuf| {
        ResolvedProgram::new("/bin/sh")
            .with_args([
                sandbox.agent_script().display().to_string(),
                "respond".to_owned(),
                "rogue".to_owned(),
            ])
            .with_read_root(sandbox.fixture_dir())
            .with_write_root(instance)
    };
    programs.insert(
        "acme.write-out",
        "1.0.0",
        base(sandbox.outside_path("rogue-write-root")),
    );
    programs.insert(
        "acme.read-root",
        "1.0.0",
        base(sandbox.instance_root("read-root")).with_read_root("/"),
    );
    programs.insert(
        "acme.read-ancestor",
        "1.0.0",
        base(sandbox.instance_root("read-ancestor"))
            .with_read_root(sandbox.root().parent().expect("temp parent").to_path_buf()),
    );
    programs.insert(
        "acme.relative",
        "1.0.0",
        base(PathBuf::from("relative/write-root")),
    );
    let inside = sandbox.instance_root("exec-inside");
    let nested = inside.join("entry.sh");
    std::fs::write(&nested, "#!/bin/sh\nexit 0\n").expect("nested entry");
    programs.insert(
        "acme.exec-inside",
        "1.0.0",
        ResolvedProgram::new(&nested)
            .with_read_root(sandbox.fixture_dir())
            .with_write_root(inside),
    );

    let carrier = carrier(
        programs,
        policy(IsolationMode::TrustedLocal, sandbox.root()),
    );
    let host = host_of(carrier.clone());
    let expectations = [
        ("acme.write-out", "extension_isolation_root_out_of_bounds"),
        ("acme.read-root", "extension_isolation_root_out_of_bounds"),
        (
            "acme.read-ancestor",
            "extension_isolation_root_out_of_bounds",
        ),
        ("acme.exec-inside", "extension_isolation_root_out_of_bounds"),
        ("acme.relative", "extension_isolation_path_invalid"),
    ];
    for (package, expected) in expectations {
        let failure = activate(&host, package, &["acme.rogue/run"])
            .expect_err("an out-of-bounds declaration is refused");
        assert_eq!(failure.code, expected, "for {package}");
    }
    assert!(
        !sandbox.outside_path("rogue-write-root").exists(),
        "a refused write root was not created"
    );
    for instance in [
        "instance-1",
        "instance-2",
        "instance-3",
        "instance-4",
        "instance-5",
    ] {
        assert!(
            carrier.facts(instance).is_none(),
            "no process was started for {instance}"
        );
    }
}
