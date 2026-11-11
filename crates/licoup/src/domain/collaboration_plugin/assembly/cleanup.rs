use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::model::LocalAssemblyRecord;
use crate::platform::client_state::ClientStateStore;

const COLLECTION: &str = "local-server-assembly-cleanup";
const SCHEMA: &str = "licoup.local-server-assembly-cleanup.v1";
const MAX_PENDING: usize = 8;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct PendingAssemblyCleanup {
    schema_version: String,
    operation_id: String,
    pub(super) deployment_id: String,
    destination_digest_sha256: String,
    pub(super) original_destination: String,
    pub(super) quarantine: String,
}

impl PendingAssemblyCleanup {
    pub(super) fn prepare(
        store: &ClientStateStore,
        record: &LocalAssemblyRecord,
        operation_id: &str,
    ) -> Result<Self> {
        let original = Path::new(&record.destination);
        let parent = original
            .parent()
            .ok_or_else(|| anyhow!("collaboration_local_server_destination_parent_missing"))?;
        let quarantine = parent.join(format!(".licoup-local-server-uninstall-{operation_id}"));
        let pending = Self {
            schema_version: SCHEMA.to_owned(),
            operation_id: operation_id.to_owned(),
            deployment_id: record.deployment_id.clone(),
            destination_digest_sha256: record.destination_digest_sha256.clone(),
            original_destination: record.destination.clone(),
            quarantine: path_text(&quarantine)?,
        };
        pending.validate()?;
        let mut values = read(store)?;
        ensure!(
            values.len() < MAX_PENDING
                && values.iter().all(|value| {
                    value.deployment_id != pending.deployment_id
                        && value.quarantine != pending.quarantine
                }),
            "collaboration_local_server_cleanup_conflict"
        );
        values.push(pending.clone());
        values.sort_by(|left, right| left.deployment_id.cmp(&right.deployment_id));
        write(store, &values)?;
        Ok(pending)
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA
                && uuid::Uuid::parse_str(&self.operation_id)
                    .is_ok_and(|value| value.to_string() == self.operation_id)
                && uuid::Uuid::parse_str(&self.deployment_id)
                    .is_ok_and(|value| value.to_string() == self.deployment_id),
            "collaboration_local_server_cleanup_record_invalid"
        );
        let original = Path::new(&self.original_destination);
        let quarantine = Path::new(&self.quarantine);
        let expected_name = format!(".licoup-local-server-uninstall-{}", self.operation_id);
        ensure!(
            original.is_absolute()
                && quarantine.is_absolute()
                && original.parent() == quarantine.parent()
                && quarantine.file_name().and_then(|value| value.to_str())
                    == Some(expected_name.as_str())
                && super::snapshot::destination_digest(original)? == self.destination_digest_sha256,
            "collaboration_local_server_cleanup_record_invalid"
        );
        Ok(())
    }
}

pub(super) fn find(
    store: &ClientStateStore,
    deployment_id: &str,
) -> Result<Option<PendingAssemblyCleanup>> {
    Ok(read(store)?
        .into_iter()
        .find(|pending| pending.deployment_id == deployment_id))
}

pub(super) fn remove(store: &ClientStateStore, deployment_id: &str) -> Result<()> {
    let mut values = read(store)?;
    values.retain(|pending| pending.deployment_id != deployment_id);
    write(store, &values)
}

pub(super) fn count(store: &ClientStateStore) -> Result<usize> {
    Ok(read(store)?.len())
}

fn read(store: &ClientStateStore) -> Result<Vec<PendingAssemblyCleanup>> {
    let collection = store.read_collection(COLLECTION)?;
    let items = collection
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ensure!(
        items.len() <= MAX_PENDING,
        "collaboration_local_server_cleanup_limit_exceeded"
    );
    let values = items
        .into_iter()
        .map(|value| {
            let value: PendingAssemblyCleanup = serde_json::from_value(value)
                .map_err(|_| anyhow!("collaboration_local_server_cleanup_record_invalid"))?;
            value.validate()?;
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        values
            .windows(2)
            .all(|pair| pair[0].deployment_id < pair[1].deployment_id),
        "collaboration_local_server_cleanup_record_invalid"
    );
    Ok(values)
}

fn write(store: &ClientStateStore, values: &[PendingAssemblyCleanup]) -> Result<()> {
    ensure!(
        values.len() <= MAX_PENDING,
        "collaboration_local_server_cleanup_limit_exceeded"
    );
    for value in values {
        value.validate()?;
    }
    store
        .write_collection(COLLECTION, json!({"items": values}))
        .map(|_| ())
}

fn path_text(path: &PathBuf) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("collaboration_local_server_destination_encoding_invalid"))
}
