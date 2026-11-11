use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

use super::apply::record_projection;
use super::model::{
    AssemblyPayloadFile, LocalAssemblyRecord, LocalServerLifecycle, assembled_runner_relative_path,
};
use super::runtime::{
    ProcessLiveness, RuntimeControl, RuntimeIdentity, SpawnedRuntime, status_with, stop_with,
};
use super::store::{find_record, write_records};
use crate::domain::collaboration_plugin::manifest::{
    SERVER_CAPABILITIES_CONTRACT, SERVER_HEALTH_CONTRACT, SERVER_RUNNER_CONTRACT,
    current_server_runner_target, expected_server_runner_path,
};
use crate::platform::client_state::ClientStateStore;

struct Fixture {
    root: PathBuf,
    store: ClientStateStore,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lico-local-server-runtime-{name}-{}",
            Uuid::new_v4()
        ));
        licoup_foundation::platform::file_security::ensure_private_dir(&root).unwrap();
        let store = ClientStateStore::new(root.join("state")).unwrap();
        Self { root, store }
    }

    fn running_record(&self) -> LocalAssemblyRecord {
        synthetic_record(&self.root.join("assembled"), LocalServerLifecycle::Running)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct FakeRuntime {
    liveness: Mutex<ProcessLiveness>,
    identity: RuntimeIdentity,
    supervised: bool,
    terminate_count: AtomicUsize,
}

impl FakeRuntime {
    fn new(liveness: ProcessLiveness, identity: RuntimeIdentity, supervised: bool) -> Self {
        Self {
            liveness: Mutex::new(liveness),
            identity,
            supervised,
            terminate_count: AtomicUsize::new(0),
        }
    }
}

impl RuntimeControl for FakeRuntime {
    fn spawn(
        &self,
        _store: &ClientStateStore,
        _record: &LocalAssemblyRecord,
    ) -> anyhow::Result<SpawnedRuntime> {
        unreachable!("these stop/status tests never spawn")
    }

    fn terminate(&self, _record: &LocalAssemblyRecord) -> anyhow::Result<()> {
        self.terminate_count.fetch_add(1, Ordering::SeqCst);
        *self.liveness.lock().unwrap() = ProcessLiveness::Dead;
        Ok(())
    }

    fn liveness(&self, _record: &LocalAssemblyRecord) -> ProcessLiveness {
        *self.liveness.lock().unwrap()
    }

    fn has_supervised_handle(&self, _record: &LocalAssemblyRecord) -> bool {
        self.supervised
    }

    fn identity(
        &self,
        _store: &ClientStateStore,
        _record: &LocalAssemblyRecord,
    ) -> RuntimeIdentity {
        self.identity
    }
}

#[test]
fn supervised_stop_does_not_depend_on_mutable_package_or_assembly_files() {
    let fixture = Fixture::new("supervised-stop");
    let record = fixture.running_record();
    assert!(!Path::new(&record.destination).exists());
    write_records(&fixture.store, std::slice::from_ref(&record)).unwrap();
    let runtime = FakeRuntime::new(ProcessLiveness::Alive, RuntimeIdentity::Unavailable, true);

    let result = stop_with(&fixture.store, &record.deployment_id, &runtime).unwrap();

    assert_eq!(result["status"], "deployment-stopped");
    assert_eq!(runtime.terminate_count.load(Ordering::SeqCst), 1);
    let stored = find_record(&fixture.store, &record.deployment_id).unwrap();
    assert_eq!(stored.lifecycle, LocalServerLifecycle::Stopped);
    assert!(stored.runtime_pid.is_none());
    assert!(stored.execution_started);
}

#[test]
fn unavailable_liveness_is_quarantined_without_losing_process_identity() {
    let fixture = Fixture::new("unavailable");
    let record = fixture.running_record();
    let identity = record.runtime_process_identity.clone();
    write_records(&fixture.store, std::slice::from_ref(&record)).unwrap();
    let runtime = FakeRuntime::new(
        ProcessLiveness::Unavailable,
        RuntimeIdentity::Unavailable,
        false,
    );

    assert!(stop_with(&fixture.store, &record.deployment_id, &runtime).is_err());

    let stored = find_record(&fixture.store, &record.deployment_id).unwrap();
    assert_eq!(stored.lifecycle, LocalServerLifecycle::Quarantined);
    assert_eq!(stored.runtime_pid, record.runtime_pid);
    assert_eq!(stored.runtime_process_identity, identity);
    assert_eq!(runtime.terminate_count.load(Ordering::SeqCst), 0);
}

#[test]
fn mismatched_identity_is_quarantined_and_never_terminated() {
    let fixture = Fixture::new("mismatch");
    let record = fixture.running_record();
    write_records(&fixture.store, std::slice::from_ref(&record)).unwrap();
    let runtime = FakeRuntime::new(ProcessLiveness::Alive, RuntimeIdentity::Mismatched, false);

    assert!(stop_with(&fixture.store, &record.deployment_id, &runtime).is_err());

    let stored = find_record(&fixture.store, &record.deployment_id).unwrap();
    assert_eq!(stored.lifecycle, LocalServerLifecycle::Quarantined);
    assert_eq!(
        stored.runtime_process_identity,
        record.runtime_process_identity
    );
    assert_eq!(runtime.terminate_count.load(Ordering::SeqCst), 0);
}

#[test]
fn dead_process_is_reaped_from_projection_without_erasing_execution_history() {
    let fixture = Fixture::new("dead");
    let record = fixture.running_record();
    write_records(&fixture.store, std::slice::from_ref(&record)).unwrap();
    let runtime = FakeRuntime::new(ProcessLiveness::Dead, RuntimeIdentity::Unavailable, false);

    let status = status_with(&fixture.store, &runtime).unwrap();

    assert_eq!(
        status["servers"][0]["status"],
        "assembled-awaiting-deployment"
    );
    assert_eq!(status["servers"][0]["pluginCodeExecuted"], true);
    let stored = find_record(&fixture.store, &record.deployment_id).unwrap();
    assert!(stored.execution_started);
    assert!(stored.runtime_pid.is_none());
}

#[test]
fn runtime_projection_reports_selected_server_execution_truthfully() {
    let fixture = Fixture::new("truth");
    let record = fixture.running_record();
    let projection = record_projection(&record);
    assert_eq!(projection["pluginCodeExecuted"], true);
    assert_eq!(projection["runnerCodeExecuting"], true);
    assert_eq!(projection["selectedServerCodeExecuting"], true);
    assert_eq!(
        projection["runtimeCapability"],
        json!(super::runtime::SANDBOX_CAPABILITY)
    );
}

#[test]
fn uninstall_cleanup_target_is_persisted_before_any_rename() {
    let fixture = Fixture::new("cleanup-ledger");
    let mut record = fixture.running_record();
    record.lifecycle = LocalServerLifecycle::Stopped;
    record.runtime_pid = None;
    record.runtime_instance_id = None;
    record.runtime_process_identity = None;
    let operation_id = Uuid::new_v4().to_string();

    let pending =
        super::cleanup::PendingAssemblyCleanup::prepare(&fixture.store, &record, &operation_id)
            .unwrap();

    assert!(!Path::new(&pending.quarantine).exists());
    assert_eq!(
        super::cleanup::find(&fixture.store, &record.deployment_id)
            .unwrap()
            .unwrap(),
        pending
    );
    super::cleanup::remove(&fixture.store, &record.deployment_id).unwrap();
    assert!(
        super::cleanup::find(&fixture.store, &record.deployment_id)
            .unwrap()
            .is_none()
    );
}

pub(super) fn synthetic_record(
    destination: &Path,
    lifecycle: LocalServerLifecycle,
) -> LocalAssemblyRecord {
    let (platform, architecture) = current_server_runner_target().unwrap();
    let payload = vec![AssemblyPayloadFile {
        selection_id: "server-core".to_owned(),
        source_relative_path: "payload/server-core/server.json".to_owned(),
        destination_relative_path: "server-core/server.json".to_owned(),
        digest_sha256: "b".repeat(64),
        bytes: 16,
    }];
    let inventory_digest = super::payload_inventory::digest(&payload).unwrap();
    let running = lifecycle != LocalServerLifecycle::Stopped;
    let destination = destination.to_str().unwrap().to_owned();
    let record = LocalAssemblyRecord {
        schema_version: super::ASSEMBLY_STATE_SCHEMA.to_owned(),
        deployment_id: Uuid::new_v4().to_string(),
        plugin_id: "licomesh-collaboration".to_owned(),
        source_url: "https://github.com/example/collaboration-plugin.git".to_owned(),
        server_version: "1.0.0".to_owned(),
        package_digest_sha256: "a".repeat(64),
        selected_component_ids: vec!["server-core".to_owned()],
        destination: destination.clone(),
        assembly_adapter_id: super::ASSEMBLY_ADAPTER_ID.to_owned(),
        bind_host: "127.0.0.1".to_owned(),
        port: 32_121,
        manifest_digest_sha256: "c".repeat(64),
        destination_digest_sha256: super::snapshot::destination_digest(Path::new(&destination))
            .unwrap(),
        sealed_snapshot_digest_sha256: "d".repeat(64),
        sealed_snapshot_bytes: 64,
        runtime_generation: 1,
        execution_started: true,
        lifecycle,
        runtime_pid: running.then_some(42_424),
        runtime_instance_id: running.then(|| Uuid::new_v4().to_string()),
        runtime_process_identity: running.then(|| "test:42424:identity".to_owned()),
        runner_platform: platform.to_owned(),
        runner_architecture: architecture.to_owned(),
        runner_source_relative_path: expected_server_runner_path(platform, architecture)
            .to_string_lossy()
            .replace('\\', "/"),
        runner_destination_relative_path: assembled_runner_relative_path(platform),
        runner_digest_sha256: "e".repeat(64),
        runner_contract_version: SERVER_RUNNER_CONTRACT.to_owned(),
        health_contract_version: SERVER_HEALTH_CONTRACT.to_owned(),
        capabilities_contract_version: SERVER_CAPABILITIES_CONTRACT.to_owned(),
        signed_package_inventory_digest_sha256: "f".repeat(64),
        source_commit_oid: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        runner_trust_key_id: "official-test-key".to_owned(),
        runner_trust_fingerprint_sha256: "1".repeat(64),
        selected_payload_files: payload,
        selected_payload_inventory_digest_sha256: inventory_digest,
    };
    record.validate().unwrap();
    record
}
