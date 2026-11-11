use anyhow::{Result, anyhow, ensure};
use fs2::FileExt;
use serde_json::{Value, json};
use std::fs::File;
use std::io::ErrorKind;
use std::thread;
use std::time::{Duration, Instant};

use super::model::LocalAssemblyRecord;
use crate::platform::client_state::ClientStateStore;

const ASSEMBLY_COLLECTION: &str = "local-server-assemblies";
const MAX_ASSEMBLIES: usize = 8;
const OPERATION_LOCK_FILE: &str = ".local-server-assembly-operation.lock";
const OPERATION_LOCK_WAIT: Duration = Duration::from_millis(750);
const OPERATION_LOCK_POLL: Duration = Duration::from_millis(20);

pub(super) struct AssemblyOperationLock {
    file: File,
}

impl AssemblyOperationLock {
    pub(super) fn acquire(store: &ClientStateStore) -> Result<Self> {
        let file = licoup_foundation::platform::file_security::open_private_lock_file(
            &store.root().join(OPERATION_LOCK_FILE),
        )?;
        let deadline = Instant::now() + OPERATION_LOCK_WAIT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(OPERATION_LOCK_POLL);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return Err(anyhow!("collaboration_local_server_operation_busy"));
                }
                Err(_) => {
                    return Err(anyhow!("collaboration_local_server_operation_lock_failed"));
                }
            }
        }
    }
}

impl Drop for AssemblyOperationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) fn read_records(store: &ClientStateStore) -> Result<Vec<LocalAssemblyRecord>> {
    let collection = store.read_collection(ASSEMBLY_COLLECTION)?;
    let values = collection
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ensure!(
        values.len() <= MAX_ASSEMBLIES,
        "collaboration_local_server_limit_exceeded"
    );
    let mut records = values
        .into_iter()
        .map(|value| {
            let record: LocalAssemblyRecord = serde_json::from_value(value)
                .map_err(|_| anyhow!("collaboration_local_server_state_invalid"))?;
            record.validate()?;
            Ok(record)
        })
        .collect::<Result<Vec<_>>>()?;
    records.sort_by(|left, right| left.deployment_id.cmp(&right.deployment_id));
    ensure!(
        records
            .windows(2)
            .all(|pair| pair[0].deployment_id != pair[1].deployment_id),
        "collaboration_local_server_state_duplicate"
    );
    Ok(records)
}

pub(super) fn write_records(
    store: &ClientStateStore,
    records: &[LocalAssemblyRecord],
) -> Result<()> {
    ensure!(
        records.len() <= MAX_ASSEMBLIES,
        "collaboration_local_server_limit_exceeded"
    );
    for record in records {
        record.validate()?;
    }
    let values = records
        .iter()
        .map(serde_json::to_value)
        .collect::<serde_json::Result<Vec<_>>>()?;
    store
        .write_collection(ASSEMBLY_COLLECTION, json!({"items": values}))
        .map(|_| ())
}

pub(super) fn insert_record(store: &ClientStateStore, record: LocalAssemblyRecord) -> Result<()> {
    let mut records = read_records(store)?;
    ensure!(
        records.len() < MAX_ASSEMBLIES
            && records
                .iter()
                .all(|existing| existing.deployment_id != record.deployment_id
                    && existing.destination != record.destination
                    && existing.port != record.port),
        "collaboration_local_server_conflict"
    );
    records.push(record);
    records.sort_by(|left, right| left.deployment_id.cmp(&right.deployment_id));
    write_records(store, &records)
}

pub(super) fn replace_record(store: &ClientStateStore, record: LocalAssemblyRecord) -> Result<()> {
    let mut records = read_records(store)?;
    let target = records
        .iter_mut()
        .find(|existing| existing.deployment_id == record.deployment_id)
        .ok_or_else(|| anyhow!("collaboration_local_server_not_found"))?;
    *target = record;
    write_records(store, &records)
}

pub(super) fn remove_record(
    store: &ClientStateStore,
    deployment_id: &str,
) -> Result<LocalAssemblyRecord> {
    let mut records = read_records(store)?;
    let index = records
        .iter()
        .position(|record| record.deployment_id == deployment_id)
        .ok_or_else(|| anyhow!("collaboration_local_server_not_found"))?;
    let record = records.remove(index);
    write_records(store, &records)?;
    Ok(record)
}

pub(super) fn find_record(
    store: &ClientStateStore,
    deployment_id: &str,
) -> Result<LocalAssemblyRecord> {
    read_records(store)?
        .into_iter()
        .find(|record| record.deployment_id == deployment_id)
        .ok_or_else(|| anyhow!("collaboration_local_server_not_found"))
}
