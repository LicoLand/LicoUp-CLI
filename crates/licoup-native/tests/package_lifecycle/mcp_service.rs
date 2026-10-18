#![cfg(unix)]
//! MCP-PACKAGE-LIFECYCLE: the optional service process and its caller
//! registrations, bound to the installed and enabled package generation.
//!
//! What is real here: a real managed package store on disk, real archives built
//! from real manifests, the production selector over that store, the production
//! process owner (it really spawns a program, really sends it `service <verb>`,
//! and really reads the JSON it answers with), the real lease the owner writes,
//! and the real digest both the launcher and the program measure.
//!
//! What is synthetic, stated plainly: the packaged executable is a small shell
//! program that stands in for `lico-subagent-mcp`. It models exactly the
//! contract the binding depends on — `service start|stop|status|serve`, one
//! served process at a time, and a refusal to serve bytes whose digest is not
//! the one the launcher approved — and nothing else. The fixture makes that
//! entry executable after installing it, the way the release stage would ship a
//! native executable. The real payload's own consent gate, its HTTP endpoint
//! and its connector are covered by `cargo test -p licoup-public-gateway`.
//!
//! What is deliberately not here: no network, no Agent, no installed client, and
//! no write to the real data home. Every root is a disposable directory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use licoup_native::platform::extension_packages::{GenerationSelection, PackageStore, uninstall};
use licoup_native::platform::mcp_service_process::{
    MCP_PACKAGE_ID, McpServiceBinding, PackageConsent, ProcessState, package_store_root,
};
use licoup_native::platform::package_registration_release::PackageRegistrationOwners;

use super::{
    archive, covering_client_versions, manifest_json_declaring, root as fixture_root, trust_for,
};

/// A synthetic service program: the verbs the binding sends, one live process at
/// a time, and the consent check the packaged payload performs.
const SERVICE: &str = r#"#!/bin/sh
root="${LICOUP_HOME:?}"
state="$root/client-state/subagent-mcp"
marker="$state/synthetic-service.pid"
mkdir -p "$state"

self_digest() {
  if command -v sha256sum >/dev/null 2>&1; then
    printf 'sha256:%s' "$(sha256sum "$0" | cut -d' ' -f1)"
  else
    printf 'sha256:%s' "$(shasum -a 256 "$0" | cut -d' ' -f1)"
  fi
}

live_pid() {
  [ -f "$marker" ] || return 1
  pid="$(cat "$marker" 2>/dev/null)"
  [ -n "$pid" ] || return 1
  if ps -p "$pid" >/dev/null 2>&1; then
    printf '%s' "$pid"
    return 0
  fi
  rm -f "$marker"
  return 1
}

case "$2" in
  serve)
    if [ "${LICOUP_MCP_APPROVED_DIGEST:-}" != "$(self_digest)" ]; then
      echo '{"service":"subagents","state":"refused"}'
      exit 1
    fi
    printf '%s' "$$" > "$marker"
    # One process, and a signal reaches it: exec keeps the published pid and
    # lets the stop this contract performs actually end the process.
    exec sleep 300
    ;;
  status)
    if live_pid >/dev/null; then
      echo '{"service":"subagents","state":"running"}'
    else
      echo '{"service":"subagents","state":"stopped"}'
    fi
    ;;
  start)
    if live_pid >/dev/null; then
      echo '{"service":"subagents","state":"running"}'
      exit 0
    fi
    nohup "$0" service serve >/dev/null 2>&1 &
    i=0
    while [ "$i" -lt 200 ]; do
      if live_pid >/dev/null; then
        echo '{"service":"subagents","state":"running"}'
        exit 0
      fi
      i=$((i + 1))
      sleep 0.05
    done
    echo '{"service":"subagents","state":"stopped"}'
    exit 1
    ;;
  stop)
    if pid="$(live_pid)"; then
      kill "$pid" 2>/dev/null || true
      rm -f "$marker"
    fi
    echo '{"service":"subagents","state":"stopped"}'
    ;;
  reload)
    "$0" service stop >/dev/null
    exec "$0" service start
    ;;
  *)
    echo '{"service":"subagents","state":"invalid"}'
    exit 1
    ;;
esac
"#;

/// A service program whose `start` never comes up: the injected failure an
/// activation has to roll back from.
const FAILING_SERVICE: &str = r#"#!/bin/sh
case "$2" in
  status) echo '{"service":"subagents","state":"stopped"}' ;;
  stop) echo '{"service":"subagents","state":"stopped"}' ;;
  *) echo '{"service":"subagents","state":"unavailable"}'; exit 1 ;;
esac
"#;

fn fixture(tag: &str) -> (PathBuf, PathBuf, PackageStore) {
    let root = fixture_root(tag);
    let data_home = root.join("data-home");
    std::fs::create_dir_all(&data_home).expect("data home");
    let store_root = package_store_root(&data_home);
    let store = PackageStore::open(&store_root).expect("store");
    (root, data_home, store)
}

fn binding(data_home: &Path, store: &PackageStore) -> McpServiceBinding {
    McpServiceBinding::open(data_home, store.root())
}

/// Install one generation of the service package and make its entry executable.
fn install(store: &PackageStore, version: &str, script: &str) -> String {
    let bytes = archive(&[
        (
            "manifest.json",
            manifest_json_declaring(
                MCP_PACKAGE_ID,
                version,
                "bin/lico-subagent-mcp",
                &[],
                None,
                &[],
                &covering_client_versions(),
            )
            .into_bytes(),
        ),
        ("bin/lico-subagent-mcp", script.as_bytes().to_vec()),
    ]);
    store
        .install_local_import(MCP_PACKAGE_ID, version, trust_for(&bytes), &bytes)
        .expect("install the service generation");
    let entry = entry_path(store, version);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755))
            .expect("the release stage ships an executable");
    }
    store
        .installed_version(MCP_PACKAGE_ID, version)
        .expect("record")
        .expect("installed")
        .digest
}

fn entry_path(store: &PackageStore, version: &str) -> PathBuf {
    store
        .installed_path(MCP_PACKAGE_ID, version)
        .join("bin/lico-subagent-mcp")
}

/// Switch one installed version off, through the record its owner writes.
fn switch_off(store: &PackageStore, version: &str) {
    let directory = store.root().join("preferences").join(MCP_PACKAGE_ID);
    std::fs::create_dir_all(&directory).expect("preference directory");
    std::fs::write(
        directory.join(format!("{version}.json")),
        serde_json::json!({"enabled": false, "updatedAtUnixMs": 1}).to_string(),
    )
    .expect("preference");
}

fn marker(data_home: &Path) -> PathBuf {
    data_home
        .join("client-state")
        .join("subagent-mcp")
        .join("synthetic-service.pid")
}

/// The one process the synthetic service says is live, if any.
fn live_pid(data_home: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(marker(data_home)).ok()?;
    let pid: u32 = text.trim().parse().ok()?;
    if alive(pid) { Some(pid) } else { None }
}

fn alive(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn wait_for_exit(data_home: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while live_pid(data_home).is_some() && Instant::now() < deadline {
        sleep(Duration::from_millis(20));
    }
    assert!(
        live_pid(data_home).is_none(),
        "the synthetic service process is still live"
    );
}

/// A stop is a signal: the process ends shortly after it is sent.
fn wait_until_dead(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) && Instant::now() < deadline {
        sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "the process {pid} is still live");
}

#[test]
fn an_enabled_generation_starts_exactly_one_service_process() {
    let (root, data_home, store) = fixture("mcp-enabled");
    install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);

    let selection = binding.selection().expect("selection");
    assert_eq!(selection.state(), "selected");
    let generation = selection.generation().expect("a generation is selected");
    assert_eq!(generation.entry(), "bin/lico-subagent-mcp");
    assert!(generation.approved_digest().starts_with("sha256:"));
    assert!(generation.payload_digest().starts_with("sha256:"));

    let started = binding.start().expect("start");
    assert_eq!(started["state"], "running");
    assert_eq!(started["action"], "started");
    let first = live_pid(&data_home).expect("one live process");

    // The callers this generation registered are the adapter registry's own
    // caller set, and every one of them is admitted while it serves.
    let callers = binding.admitted_callers();
    assert!(!callers.is_empty(), "the mesh admits at least one caller");
    for caller in &callers {
        let grant = binding
            .caller_admission(caller)
            .unwrap_or_else(|failure| panic!("{caller} must be admitted: {failure:?}"));
        assert_eq!(grant.caller(), caller);
    }
    assert_eq!(
        binding
            .caller_admission("not-a-mesh-caller")
            .expect_err("an unregistered caller is refused")
            .code,
        "mcp_caller_unregistered"
    );

    // Asking again does not start a second process: exactly one is live and it
    // is the same one.
    let again = binding.start().expect("start again");
    assert_eq!(again["action"], "already-running");
    assert_eq!(again["state"], "running");
    assert_eq!(live_pid(&data_home), Some(first));

    let status = binding.status().expect("status");
    assert_eq!(status["state"], "running");
    assert_eq!(status["package"]["selection"], "selected");
    assert_eq!(
        status["package"]["activeGenerationId"],
        generation.generation_id()
    );

    binding.stop().expect("stop");
    wait_for_exit(&data_home);
    assert!(binding.lease().expect("lease").is_none());
    cleanup(&root, &data_home);
}

#[test]
fn a_switched_off_generation_starts_no_process_and_refuses_its_callers() {
    let (root, data_home, store) = fixture("mcp-disabled");
    install(&store, "0.14.0", SERVICE);
    switch_off(&store, "0.14.0");
    let binding = binding(&data_home, &store);

    assert_eq!(
        binding.selection().expect("selection"),
        GenerationSelection::Disabled {
            installed_versions: vec!["0.14.0".to_owned()],
        }
    );

    let failure = binding
        .start()
        .expect_err("a switched-off package starts nothing");
    assert_eq!(failure.to_string(), "mcp_package_disabled");
    assert!(
        live_pid(&data_home).is_none(),
        "no process was started and none pretends to be"
    );
    assert!(binding.lease().expect("lease").is_none());

    assert_eq!(
        binding
            .caller_admission("codex")
            .expect_err("a switched-off package serves no caller")
            .code,
        "mcp_package_disabled"
    );

    let status = binding.status().expect("status");
    assert_eq!(status["state"], "stopped");
    assert_eq!(status["package"]["selection"], "disabled");
    assert!(status["callers"].as_array().expect("callers").is_empty());

    // Switching it back on is all it takes: the same installed bytes start.
    let mut preference = store.root().join("preferences").join(MCP_PACKAGE_ID);
    preference.push("0.14.0.json");
    std::fs::write(
        &preference,
        serde_json::json!({"enabled": true}).to_string(),
    )
    .expect("preference");
    binding
        .start()
        .expect("start after the switch is turned on");
    assert!(live_pid(&data_home).is_some());
    binding.stop().expect("stop");
    wait_for_exit(&data_home);
    cleanup(&root, &data_home);
}

#[test]
fn a_failed_activation_restores_the_previous_generation_and_its_callers() {
    let (root, data_home, store) = fixture("mcp-rollback");
    let first_digest = install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);
    binding.start().expect("the first generation serves");
    let first_generation = binding
        .lease()
        .expect("lease")
        .expect("a lease")
        .generation_id;
    let caller = binding.admitted_callers().remove(0);

    // A second generation is installed, and it cannot come up.
    install(&store, "0.15.0", FAILING_SERVICE);
    let second = store
        .installed_version(MCP_PACKAGE_ID, "0.15.0")
        .expect("record")
        .expect("installed");
    let consent = PackageConsent::new(second.digest.clone()).expect("consent");
    let failure = binding
        .activate(&consent)
        .expect_err("the second generation cannot serve");
    assert_eq!(failure.to_string(), "mcp_activation_failed");

    // The previous generation is still the one this client owns, its process is
    // serving again, and the caller it registered is still admitted.
    let lease = binding.lease().expect("lease").expect("the lease survived");
    assert_eq!(lease.generation_id, first_generation);
    assert_eq!(
        lease.approved_digest, first_digest,
        "the approval still names the installed content"
    );
    assert_eq!(
        lease.payload_digest,
        payload_digest_of(&store, "0.14.0"),
        "the payload is still the measured executable"
    );
    assert_eq!(
        binding.probe(&entry_path(&store, "0.14.0")),
        ProcessState::Running
    );
    assert!(
        live_pid(&data_home).is_some(),
        "exactly one process is live"
    );
    binding
        .caller_admission(&caller)
        .expect("the previous registration is intact");

    // The failure is attributable after the fact, and it names both generations.
    let record = binding
        .failure_record()
        .expect("failure record")
        .expect("a failed activation left a record");
    assert_eq!(record["code"], "mcp_activation_failed");
    assert_eq!(record["previousGenerationId"], first_generation);
    assert_eq!(record["attemptedGenerationId"], second_generation(&store));
    assert_eq!(record["restoredProcess"], "running");

    // A later successful start clears the failure record.
    store
        .remove_installed(MCP_PACKAGE_ID, "0.15.0")
        .expect("remove the generation that could not come up");
    binding.start().expect("the first generation still starts");
    assert!(binding.failure_record().expect("record").is_none());

    binding.stop().expect("stop");
    wait_for_exit(&data_home);
    cleanup(&root, &data_home);
}

#[test]
fn a_crashed_service_is_reconciled_to_exactly_one_live_process() {
    let (root, data_home, store) = fixture("mcp-crash");
    install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);
    binding.start().expect("start");
    let crashed = live_pid(&data_home).expect("a live process");
    Command::new("kill")
        .args(["-9", &crashed.to_string()])
        .status()
        .expect("kill the synthetic service");

    let reconciled = binding.reconcile().expect("reconcile");
    assert_eq!(reconciled["action"], "crashed-process-settled");
    assert_eq!(reconciled["state"], "stopped");
    assert_eq!(
        reconciled["callers"].as_array().expect("callers").len(),
        binding.admitted_callers().len(),
        "the registrations the crashed process held are named, not forgotten"
    );
    assert!(binding.lease().expect("lease").is_none());

    // The next start brings up exactly one process, and it is a new one.
    let restarted = binding.start().expect("restart");
    assert_eq!(restarted["state"], "running");
    let pid = live_pid(&data_home).expect("one live process");
    assert_ne!(pid, crashed);
    binding.start().expect("idempotent");
    assert_eq!(live_pid(&data_home), Some(pid), "still exactly one");

    binding.stop().expect("stop");
    wait_for_exit(&data_home);
    cleanup(&root, &data_home);
}

#[test]
fn uninstall_stops_the_process_and_withdraws_every_caller() {
    let (root, data_home, store) = fixture("mcp-uninstall");
    install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);
    binding.start().expect("start");
    let pid = live_pid(&data_home).expect("a live process");
    let callers = binding.admitted_callers();

    // The MCP owner runs before the store reclaims the bytes: the program that
    // holds the service's writer lease is the only thing that can prove drain.
    let retired = binding.retire().expect("retire");
    assert_eq!(retired["action"], "retired");
    assert_eq!(retired["state"], "stopped");
    assert_eq!(
        retired["callers"].as_array().expect("callers").len(),
        callers.len()
    );
    wait_until_dead(pid);
    wait_for_exit(&data_home);
    assert!(binding.lease().expect("lease").is_none());
    for caller in &callers {
        assert_eq!(
            binding
                .caller_admission(caller)
                .expect_err("a retired generation serves nobody")
                .code,
            "mcp_caller_unregistered"
        );
    }

    // Only then does the store reclaim the installed version, through the
    // uninstall transaction that owns the bytes.
    let installed = store
        .installed_version(MCP_PACKAGE_ID, "0.14.0")
        .expect("record")
        .expect("installed");
    let catalogue = licoup_extension_contracts::deployment::LocalCatalogue::new();
    let mut registry = licoup_native::platform::extension_packages::InstanceRegistry::new();
    let plan = uninstall::preview(&store, &catalogue, &installed, &registry).expect("preview");
    uninstall::UninstallTransaction::begin(
        &mut registry,
        plan,
        uninstall::DependentsDecision::SelectedOnly,
    )
    .expect("begin")
    .drain(
        &mut registry,
        licoup_native::platform::extension_packages::RemainingWork::Wait,
    )
    .expect("drain")
    .collect(&store, &registry, &PackageRegistrationOwners::default())
    .expect("collect");
    assert!(
        !store.installed_path(MCP_PACKAGE_ID, "0.14.0").exists(),
        "the installed bytes are gone"
    );
    assert_eq!(binding.selection().expect("selection").state(), "absent");

    cleanup(&root, &data_home);
}

#[test]
fn an_explicit_stop_reaches_a_service_this_client_did_not_start() {
    let (root, data_home, store) = fixture("mcp-unleased");
    install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);
    binding.start().expect("start");
    let pid = live_pid(&data_home).expect("a live process");

    // A client that lost its own record still owns the stop it was asked for:
    // the installed generation is asked, and the answer names what it did.
    std::fs::remove_file(
        data_home
            .join("client-state")
            .join("subagent-mcp")
            .join("service-generation.json"),
    )
    .expect("drop the lease");
    let stopped = binding.stop().expect("stop");
    assert_eq!(stopped["action"], "stopped-unleased-generation");
    assert_eq!(stopped["state"], "stopped");
    wait_until_dead(pid);
    wait_for_exit(&data_home);
    assert!(binding.lease().expect("lease").is_none());

    cleanup(&root, &data_home);
}

#[test]
fn consent_that_does_not_name_the_installed_bytes_starts_nothing() {
    let (root, data_home, store) = fixture("mcp-consent");
    install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);

    let consent = PackageConsent::new(
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .expect("a digest shape");
    let failure = binding
        .activate(&consent)
        .expect_err("a digest that is not the installed content is refused");
    assert_eq!(failure.to_string(), "mcp_package_consent_mismatch");
    assert!(live_pid(&data_home).is_none(), "nothing was started");
    assert!(binding.lease().expect("lease").is_none());
    assert_eq!(
        binding
            .failure_record()
            .expect("record")
            .expect("a refusal is attributable")["code"],
        "mcp_package_consent_mismatch"
    );

    // The consent that does name the installed content starts the generation.
    let digest = store
        .installed_version(MCP_PACKAGE_ID, "0.14.0")
        .expect("record")
        .expect("installed")
        .digest;
    binding
        .activate(&PackageConsent::new(digest).expect("consent"))
        .expect("the approved bytes start");
    assert!(live_pid(&data_home).is_some());
    assert!(binding.failure_record().expect("record").is_none());

    binding.stop().expect("stop");
    wait_for_exit(&data_home);
    cleanup(&root, &data_home);
}

#[test]
fn a_stale_generation_is_stopped_before_the_selected_one_starts() {
    let (root, data_home, store) = fixture("mcp-stale");
    install(&store, "0.14.0", SERVICE);
    let binding = binding(&data_home, &store);
    binding.start().expect("the first generation serves");
    let stale = live_pid(&data_home).expect("a live process");

    // A newer generation is installed while the older one is still serving.
    install(&store, "0.15.0", SERVICE);
    let selection = binding.selection().expect("selection");
    assert_eq!(
        selection.generation().expect("a generation").version(),
        "0.15.0",
        "the highest switched-on generation is selected"
    );

    let reconciled = binding.reconcile().expect("reconcile");
    assert_eq!(reconciled["action"], "stale-generation-stopped");
    wait_until_dead(stale);
    wait_for_exit(&data_home);
    assert!(binding.lease().expect("lease").is_none());

    let started = binding.start().expect("start the selected generation");
    assert_eq!(started["package"]["version"], "0.15.0");
    let pid = live_pid(&data_home).expect("one live process");
    assert_ne!(pid, stale);

    binding.stop().expect("stop");
    wait_for_exit(&data_home);
    cleanup(&root, &data_home);
}

#[test]
fn the_public_lifecycle_route_is_bound_to_the_installed_package() {
    let (root, data_home, store) = fixture("mcp-route");
    install(&store, "0.14.0", SERVICE);

    // The route resolves the data home the way the running client does, so a
    // hermetic root is selected for this thread and no real state is touched.
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(data_home.clone()));
    let outcome = (|| -> Vec<String> {
        let mut failures = Vec::new();
        // An executable that is not the selected generation is refused: the
        // process must be the bytes the package lifecycle published.
        let other = data_home.join("not-the-generation");
        std::fs::write(&other, b"#!/bin/sh\nexit 1\n").expect("other binary");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
        }
        if let Err(error) =
            licoup_native::platform::mcp_service_process::execute("start", Some(&other))
        {
            if error.to_string() != "mcp_binary_not_selected_generation" {
                failures.push(error.to_string());
            }
        } else {
            failures.push("an unrelated executable was accepted".to_owned());
        }
        if live_pid(&data_home).is_some() {
            failures.push("the refused claim started a process".to_owned());
        }

        // The real route starts the installed generation and stops it again.
        if let Err(error) = licoup_native::platform::mcp_service_process::execute("start", None) {
            failures.push(error.to_string());
        }
        if live_pid(&data_home).is_none() {
            failures.push("the installed generation did not start".to_owned());
        }
        if let Err(error) = licoup_native::platform::mcp_service_process::execute("stop", None) {
            failures.push(error.to_string());
        }
        failures
    })();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);

    assert!(outcome.is_empty(), "{outcome:?}");
    wait_for_exit(&data_home);
    cleanup(&root, &data_home);
}

/// Remove a disposable root, after proving no synthetic process is left.
fn cleanup(root: &Path, data_home: &Path) {
    if let Some(pid) = live_pid(data_home) {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        wait_for_exit(data_home);
    }
    let _ = std::fs::remove_dir_all(root);
}

fn payload_digest_of(store: &PackageStore, version: &str) -> String {
    let entry = entry_path(store, version);
    licoup_native::platform::extension_packages::content_digest(
        &std::fs::read(entry).expect("installed entry"),
    )
}

fn second_generation(store: &PackageStore) -> String {
    // The generation token is derived from the installed record, exactly as the
    // binding derives it, so the assertion names the same identity.
    let record = store
        .installed_version(MCP_PACKAGE_ID, "0.15.0")
        .expect("record")
        .expect("installed");
    let digest = record
        .digest
        .strip_prefix("sha256:")
        .unwrap_or(&record.digest);
    format!(
        "{}@{}#{}",
        MCP_PACKAGE_ID,
        record.version,
        digest.get(..12).unwrap_or(digest)
    )
}
