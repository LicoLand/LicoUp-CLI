use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::process::Child;
use std::sync::{Mutex, OnceLock};

use super::super::model::LocalAssemblyRecord;
use super::process::ProcessLiveness;

struct SupervisedProcess {
    child: Child,
    pid: u32,
    runtime_instance_id: String,
    process_identity: String,
}

fn processes() -> &'static Mutex<BTreeMap<String, SupervisedProcess>> {
    static PROCESSES: OnceLock<Mutex<BTreeMap<String, SupervisedProcess>>> = OnceLock::new();
    PROCESSES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) fn register(
    record: &LocalAssemblyRecord,
    mut child: Child,
    process_identity: String,
) -> Result<()> {
    let runtime_instance_id = record
        .runtime_instance_id
        .clone()
        .ok_or_else(|| anyhow!("collaboration_local_server_runtime_instance_missing"))?;
    let pid = child.id();
    let mut values = processes()
        .lock()
        .map_err(|_| anyhow!("collaboration_local_server_supervisor_unavailable"))?;
    if values.contains_key(&record.deployment_id) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow!("collaboration_local_server_supervisor_conflict"));
    }
    values.insert(
        record.deployment_id.clone(),
        SupervisedProcess {
            child,
            pid,
            runtime_instance_id,
            process_identity,
        },
    );
    Ok(())
}

pub(super) fn owns(record: &LocalAssemblyRecord) -> bool {
    processes()
        .lock()
        .ok()
        .and_then(|values| {
            values
                .get(&record.deployment_id)
                .map(|process| binding_matches(process, record))
        })
        .unwrap_or(false)
}

pub(super) fn liveness(record: &LocalAssemblyRecord) -> Option<ProcessLiveness> {
    let mut values = processes().lock().ok()?;
    let process = values.get_mut(&record.deployment_id)?;
    if !binding_matches(process, record) {
        return Some(ProcessLiveness::Unavailable);
    }
    match process.child.try_wait() {
        Ok(None) => Some(ProcessLiveness::Alive),
        Ok(Some(_)) => {
            values.remove(&record.deployment_id);
            Some(ProcessLiveness::Dead)
        }
        Err(_) => Some(ProcessLiveness::Unavailable),
    }
}

pub(super) fn terminate_and_reap(record: &LocalAssemblyRecord) -> Option<Result<()>> {
    let mut values = match processes().lock() {
        Ok(values) => values,
        Err(_) => {
            return Some(Err(anyhow!(
                "collaboration_local_server_supervisor_unavailable"
            )));
        }
    };
    let process = values.get_mut(&record.deployment_id)?;
    if !binding_matches(process, record) {
        return Some(Err(anyhow!(
            "collaboration_local_server_supervisor_binding_mismatch"
        )));
    }
    let result = (|| -> Result<()> {
        if process
            .child
            .try_wait()
            .map_err(|_| anyhow!("collaboration_local_server_process_state_unavailable"))?
            .is_none()
        {
            process
                .child
                .kill()
                .map_err(|_| anyhow!("collaboration_local_server_stop_failed"))?;
        }
        process
            .child
            .wait()
            .map_err(|_| anyhow!("collaboration_local_server_reap_failed"))?;
        Ok(())
    })();
    if result.is_ok() {
        values.remove(&record.deployment_id);
    }
    Some(result)
}

fn binding_matches(process: &SupervisedProcess, record: &LocalAssemblyRecord) -> bool {
    record.runtime_pid == Some(process.pid)
        && record.runtime_instance_id.as_deref() == Some(process.runtime_instance_id.as_str())
        && record.runtime_process_identity.as_deref() == Some(process.process_identity.as_str())
}

#[cfg(test)]
mod tests {
    use super::{liveness, register, terminate_and_reap};
    use crate::domain::collaboration_plugin::assembly::model::LocalServerLifecycle;
    use crate::domain::collaboration_plugin::assembly::runtime::ProcessLiveness;
    use crate::domain::collaboration_plugin::assembly::tests::synthetic_record;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;
    use uuid::Uuid;

    #[cfg(unix)]
    #[test]
    fn exited_child_is_reaped_and_removed_from_supervisor() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let mut record = synthetic_record(
            &std::env::temp_dir().join(Uuid::new_v4().to_string()),
            LocalServerLifecycle::Running,
        );
        record.runtime_pid = Some(child.id());
        record.runtime_process_identity = Some("test-supervisor-exit".to_owned());
        register(&record, child, "test-supervisor-exit".to_owned()).unwrap();

        let mut observed = ProcessLiveness::Alive;
        for _ in 0..50 {
            observed = liveness(&record).unwrap_or(ProcessLiveness::Dead);
            if observed == ProcessLiveness::Dead {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(observed, ProcessLiveness::Dead);
        assert!(liveness(&record).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn owned_child_is_terminated_and_reaped_through_held_handle() {
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let mut record = synthetic_record(
            &std::env::temp_dir().join(Uuid::new_v4().to_string()),
            LocalServerLifecycle::Running,
        );
        record.runtime_pid = Some(child.id());
        record.runtime_process_identity = Some("test-supervisor-stop".to_owned());
        register(&record, child, "test-supervisor-stop".to_owned()).unwrap();

        terminate_and_reap(&record).unwrap().unwrap();
        assert!(liveness(&record).is_none());
    }
}
