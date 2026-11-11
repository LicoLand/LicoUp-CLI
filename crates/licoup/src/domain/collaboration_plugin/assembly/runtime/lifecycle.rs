use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::super::apply::record_projection;
use super::super::model::{LocalAssemblyRecord, LocalServerLifecycle};
use super::super::store::{
    AssemblyOperationLock, find_record, read_records, replace_record, write_records,
};
use super::identity::{RuntimeIdentity, runtime_identity};
use super::process::ProcessLiveness;
use crate::platform::client_state::ClientStateStore;

const RUNTIME_READY_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::domain::collaboration_plugin::assembly) trait RuntimeControl:
    Send + Sync
{
    fn spawn(
        &self,
        store: &ClientStateStore,
        record: &LocalAssemblyRecord,
    ) -> Result<super::runner::SpawnedRuntime>;
    fn terminate(&self, record: &LocalAssemblyRecord) -> Result<()>;
    fn liveness(&self, record: &LocalAssemblyRecord) -> ProcessLiveness;
    fn has_supervised_handle(&self, record: &LocalAssemblyRecord) -> bool;
    fn identity(&self, store: &ClientStateStore, record: &LocalAssemblyRecord) -> RuntimeIdentity;
}

pub(super) struct SystemRuntimeControl;

impl RuntimeControl for SystemRuntimeControl {
    fn spawn(
        &self,
        store: &ClientStateStore,
        record: &LocalAssemblyRecord,
    ) -> Result<super::runner::SpawnedRuntime> {
        super::runner::spawn(store, record)
    }

    fn terminate(&self, record: &LocalAssemblyRecord) -> Result<()> {
        if let Some(result) = super::supervisor::terminate_and_reap(record) {
            return result;
        }
        super::shutdown::request(record)
    }

    fn liveness(&self, record: &LocalAssemblyRecord) -> ProcessLiveness {
        super::supervisor::liveness(record).unwrap_or_else(|| {
            record
                .runtime_pid
                .map_or(ProcessLiveness::Dead, super::process::liveness)
        })
    }

    fn has_supervised_handle(&self, record: &LocalAssemblyRecord) -> bool {
        super::supervisor::owns(record)
    }

    fn identity(&self, store: &ClientStateStore, record: &LocalAssemblyRecord) -> RuntimeIdentity {
        if super::supervisor::owns(record) {
            super::identity::runtime_identity_after_process_binding(store, record)
        } else {
            runtime_identity(store, record)
        }
    }
}

pub(in crate::domain::collaboration_plugin::assembly) fn status_with(
    store: &ClientStateStore,
    runtime: &dyn RuntimeControl,
) -> Result<Value> {
    let _operation_lock = AssemblyOperationLock::acquire(store)?;
    let mut records = read_records(store)?;
    let mut changed = false;
    for record in &mut records {
        if record.runtime_pid.is_none() {
            continue;
        }
        match runtime.liveness(record) {
            ProcessLiveness::Dead => {
                clear_runtime_fields(record);
                changed = true;
                continue;
            }
            ProcessLiveness::Unavailable => {
                if record.lifecycle != LocalServerLifecycle::Quarantined {
                    quarantine_runtime(record);
                    changed = true;
                }
                continue;
            }
            ProcessLiveness::Alive => {}
        }
        match runtime.identity(store, record) {
            RuntimeIdentity::Owned if record.lifecycle == LocalServerLifecycle::Starting => {
                record.lifecycle = LocalServerLifecycle::Running;
                changed = true;
            }
            RuntimeIdentity::Mismatched => {
                if record.lifecycle != LocalServerLifecycle::Quarantined {
                    quarantine_runtime(record);
                    changed = true;
                }
            }
            RuntimeIdentity::Unavailable => {
                if record.lifecycle != LocalServerLifecycle::Quarantined {
                    quarantine_runtime(record);
                    changed = true;
                }
            }
            RuntimeIdentity::Owned => {}
        }
    }
    if changed {
        write_records(store, &records)?;
    }
    Ok(json!({
        "ok": true,
        "status": "loaded",
        "assemblyApplyPendingCount": super::super::transaction::pending_count(store)?,
        "cleanupPendingCount": super::super::cleanup::count(store)?,
        "servers": records.iter().map(record_projection).collect::<Vec<_>>()
    }))
}

pub(in crate::domain::collaboration_plugin::assembly) fn start_with(
    store: &ClientStateStore,
    deployment_id: &str,
    runtime: &dyn RuntimeControl,
) -> Result<Value> {
    let _operation_lock = AssemblyOperationLock::acquire(store)?;
    super::super::transaction::recover(store)?;
    let mut record = find_record(store, deployment_id)?;
    ensure!(
        record.lifecycle == LocalServerLifecycle::Stopped
            && record.runtime_pid.is_none()
            && record.runtime_instance_id.is_none(),
        "collaboration_local_server_not_stopped"
    );
    super::runner::verify_assembly(store, &record)?;
    super::super::apply::ensure_port_available(record.port)?;
    record.lifecycle = LocalServerLifecycle::Starting;
    record.runtime_instance_id = Some(Uuid::new_v4().to_string());
    let spawned = runtime.spawn(store, &record)?;
    let pid = spawned.pid;
    record.runtime_pid = Some(pid);
    record.runtime_process_identity = Some(spawned.process_identity);
    record.execution_started = true;
    if let Err(error) = replace_record(store, record.clone()) {
        let _ = runtime.terminate(&record);
        return Err(error);
    }
    let deadline = Instant::now() + RUNTIME_READY_TIMEOUT;
    while Instant::now() < deadline {
        match runtime.liveness(&record) {
            ProcessLiveness::Dead => break,
            ProcessLiveness::Unavailable => {
                quarantine_runtime(&mut record);
                replace_record(store, record)?;
                return Err(anyhow!(
                    "collaboration_local_server_runtime_liveness_unavailable"
                ));
            }
            ProcessLiveness::Alive => {}
        }
        match runtime.identity(store, &record) {
            RuntimeIdentity::Owned => {
                record.lifecycle = LocalServerLifecycle::Running;
                replace_record(store, record.clone())?;
                return Ok(json!({
                    "ok": true,
                    "status": "deployment-started",
                    "server": record_projection(&record)
                }));
            }
            RuntimeIdentity::Mismatched => break,
            RuntimeIdentity::Unavailable => thread::sleep(Duration::from_millis(40)),
        }
    }
    if runtime.liveness(&record) == ProcessLiveness::Alive {
        runtime
            .terminate(&record)
            .map_err(|_| anyhow!("collaboration_local_server_start_cleanup_failed"))?;
        wait_until_stopped(runtime, &record)?;
    }
    clear_runtime_fields(&mut record);
    replace_record(store, record)?;
    Err(anyhow!("collaboration_local_server_readiness_failed"))
}

pub(in crate::domain::collaboration_plugin::assembly) fn stop_with(
    store: &ClientStateStore,
    deployment_id: &str,
    runtime: &dyn RuntimeControl,
) -> Result<Value> {
    let _operation_lock = AssemblyOperationLock::acquire(store)?;
    let mut record = find_record(store, deployment_id)?;
    let Some(_) = record.runtime_pid else {
        ensure!(
            record.lifecycle == LocalServerLifecycle::Stopped
                && record.runtime_instance_id.is_none(),
            "collaboration_local_server_process_state_invalid"
        );
        return Ok(stopped_projection(record, true, false));
    };
    match runtime.liveness(&record) {
        ProcessLiveness::Dead => {
            clear_runtime_fields(&mut record);
            replace_record(store, record.clone())?;
            return Ok(stopped_projection(record, true, true));
        }
        ProcessLiveness::Unavailable => {
            quarantine_runtime(&mut record);
            replace_record(store, record)?;
            return Err(anyhow!(
                "collaboration_local_server_runtime_liveness_unavailable"
            ));
        }
        ProcessLiveness::Alive => {}
    }
    if !runtime.has_supervised_handle(&record) {
        match runtime.identity(store, &record) {
            RuntimeIdentity::Owned => {}
            RuntimeIdentity::Mismatched => {
                quarantine_runtime(&mut record);
                replace_record(store, record)?;
                return Err(anyhow!(
                    "collaboration_local_server_runtime_identity_mismatch"
                ));
            }
            RuntimeIdentity::Unavailable => {
                quarantine_runtime(&mut record);
                replace_record(store, record)?;
                return Err(anyhow!(
                    "collaboration_local_server_runtime_identity_unavailable"
                ));
            }
        }
    }
    let previous_lifecycle = record.lifecycle;
    record.lifecycle = LocalServerLifecycle::Stopping;
    replace_record(store, record.clone())?;
    if let Err(error) = runtime.terminate(&record) {
        record.lifecycle = previous_lifecycle;
        replace_record(store, record)?;
        return Err(error);
    }
    if let Err(error) = wait_until_stopped(runtime, &record) {
        record.lifecycle = previous_lifecycle;
        replace_record(store, record)?;
        return Err(error);
    }
    clear_runtime_fields(&mut record);
    replace_record(store, record.clone())?;
    Ok(stopped_projection(record, false, false))
}

fn wait_until_stopped(runtime: &dyn RuntimeControl, record: &LocalAssemblyRecord) -> Result<()> {
    let deadline = Instant::now() + RUNTIME_READY_TIMEOUT;
    loop {
        match runtime.liveness(record) {
            ProcessLiveness::Dead => return Ok(()),
            ProcessLiveness::Alive if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(40));
            }
            ProcessLiveness::Alive => break,
            ProcessLiveness::Unavailable => {
                return Err(anyhow!(
                    "collaboration_local_server_runtime_liveness_unavailable"
                ));
            }
        }
    }
    Err(anyhow!("collaboration_local_server_stop_failed"))
}

fn clear_runtime_fields(record: &mut LocalAssemblyRecord) {
    record.lifecycle = LocalServerLifecycle::Stopped;
    record.runtime_pid = None;
    record.runtime_instance_id = None;
    record.runtime_process_identity = None;
}

fn quarantine_runtime(record: &mut LocalAssemblyRecord) {
    record.lifecycle = LocalServerLifecycle::Quarantined;
}

fn stopped_projection(
    record: LocalAssemblyRecord,
    idempotent: bool,
    stale_state_cleared: bool,
) -> Value {
    json!({
        "ok": true,
        "status": "deployment-stopped",
        "server": record_projection(&record),
        "idempotent": idempotent,
        "staleStateCleared": stale_state_cleared
    })
}
