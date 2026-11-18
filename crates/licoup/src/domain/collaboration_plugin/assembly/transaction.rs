use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::model::LocalAssemblyRecord;
use crate::platform::client_state::ClientStateStore;

const COLLECTION: &str = "local-server-assembly-transaction";
const SCHEMA: &str = "licoup.local-server-assembly-transaction.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ApplyPhase {
    Prepared,
    ArtifactWritten,
    ProjectionWritten,
    AuthorityCommitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct PendingAssemblyApply {
    schema_version: String,
    pub(super) phase: ApplyPhase,
    pub(super) record: LocalAssemblyRecord,
    rollback_destination: String,
}

impl PendingAssemblyApply {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA,
            "collaboration_local_server_transaction_schema_invalid"
        );
        self.record.validate()?;
        let original = Path::new(&self.record.destination);
        let rollback = Path::new(&self.rollback_destination);
        ensure!(
            rollback.is_absolute()
                && rollback.parent() == original.parent()
                && rollback.file_name().and_then(|value| value.to_str())
                    == Some(rollback_name(&self.record.deployment_id).as_str()),
            "collaboration_local_server_transaction_rollback_invalid"
        );
        Ok(())
    }
}

pub(super) fn begin(store: &ClientStateStore, record: &LocalAssemblyRecord) -> Result<()> {
    ensure!(
        read(store)?.is_none(),
        "collaboration_local_server_transaction_pending"
    );
    let rollback = rollback_path(record)?;
    ensure!(
        !path_entry_exists(&rollback)?,
        "collaboration_local_server_transaction_rollback_conflict"
    );
    write(
        store,
        Some(&PendingAssemblyApply {
            schema_version: SCHEMA.to_owned(),
            phase: ApplyPhase::Prepared,
            record: record.clone(),
            rollback_destination: path_text(&rollback)?,
        }),
    )
}

pub(super) fn advance(store: &ClientStateStore, phase: ApplyPhase) -> Result<()> {
    let mut pending =
        read(store)?.ok_or_else(|| anyhow!("collaboration_local_server_transaction_missing"))?;
    ensure!(
        valid_transition(pending.phase, phase),
        "collaboration_local_server_transaction_phase_invalid"
    );
    pending.phase = phase;
    write(store, Some(&pending))
}

pub(super) fn read(store: &ClientStateStore) -> Result<Option<PendingAssemblyApply>> {
    let collection = store.read_collection(COLLECTION)?;
    let items = collection
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ensure!(
        items.len() <= 1,
        "collaboration_local_server_transaction_state_invalid"
    );
    let Some(value) = items.into_iter().next() else {
        return Ok(None);
    };
    let pending: PendingAssemblyApply = serde_json::from_value(value)
        .map_err(|_| anyhow!("collaboration_local_server_transaction_state_invalid"))?;
    pending.validate()?;
    Ok(Some(pending))
}

pub(super) fn clear(store: &ClientStateStore) -> Result<()> {
    write(store, None)
}

pub(super) fn pending_count(store: &ClientStateStore) -> Result<usize> {
    Ok(usize::from(read(store)?.is_some()))
}

pub(super) fn recover(store: &ClientStateStore) -> Result<()> {
    let Some(pending) = read(store)? else {
        return Ok(());
    };
    let record = &pending.record;
    let (_, authority) = crate::domain::collaboration_plugin::lifecycle::verified_authority(
        store,
        "Recover a prepared local-server assembly transaction",
    )?;
    if authority.authority.ensure_assembly(record).is_ok() {
        return recover_committed(store, &pending, |record| {
            super::runtime::verify_assembly_source_and_artifact(store, record).map(|_| ())
        });
    }
    ensure!(
        !authority.authority.contains_assembly(&record.deployment_id),
        "collaboration_authority_assembly_binding_mismatch"
    );
    let destination = Path::new(&record.destination);
    let rollback = Path::new(&pending.rollback_destination);
    let destination_exists = path_entry_exists(destination)?;
    let rollback_exists = path_entry_exists(rollback)?;
    ensure!(
        !(destination_exists && rollback_exists),
        "collaboration_local_server_transaction_state_invalid"
    );
    if destination_exists {
        super::runtime::verify_assembly_artifact(record)?;
        crate::domain::collaboration_plugin::workflow::commit_directory_no_replace(
            destination,
            rollback,
        )
        .map_err(|_| anyhow!("collaboration_local_server_transaction_rollback_failed"))?;
    } else if rollback_exists {
        let mut quarantined = record.clone();
        quarantined.destination = pending.rollback_destination.clone();
        quarantined.destination_digest_sha256 = super::snapshot::destination_digest(rollback)?;
        quarantined.validate()?;
        super::runtime::verify_assembly_artifact(&quarantined)?;
    }
    if super::store::read_records(store)?
        .iter()
        .any(|value| value.deployment_id == record.deployment_id)
    {
        super::store::remove_record(store, &record.deployment_id)?;
    }
    if path_entry_exists(rollback)? {
        std::fs::remove_dir_all(rollback)
            .map_err(|_| anyhow!("collaboration_local_server_transaction_cleanup_pending"))?;
    }
    clear(store)
}

fn recover_committed(
    store: &ClientStateStore,
    pending: &PendingAssemblyApply,
    verify: impl FnOnce(&LocalAssemblyRecord) -> Result<()>,
) -> Result<()> {
    let record = &pending.record;
    let existing = super::store::read_records(store)?
        .into_iter()
        .find(|value| value.deployment_id == record.deployment_id);
    ensure!(
        existing.as_ref().is_none_or(|value| value == record),
        "collaboration_local_server_transaction_projection_mismatch"
    );
    ensure!(
        !path_entry_exists(Path::new(&pending.rollback_destination))?,
        "collaboration_local_server_transaction_state_invalid"
    );
    verify(record)?;
    if existing.is_none() {
        super::store::insert_record(store, record.clone())?;
    }
    clear(store)
}

fn write(store: &ClientStateStore, pending: Option<&PendingAssemblyApply>) -> Result<()> {
    if let Some(pending) = pending {
        pending.validate()?;
    }
    let items = match pending {
        Some(value) => vec![serde_json::to_value(value)?],
        None => Vec::new(),
    };
    store
        .write_collection(COLLECTION, json!({"items": items}))
        .map(|_| ())
}

fn valid_transition(previous: ApplyPhase, next: ApplyPhase) -> bool {
    matches!(
        (previous, next),
        (ApplyPhase::Prepared, ApplyPhase::ArtifactWritten)
            | (ApplyPhase::ArtifactWritten, ApplyPhase::ProjectionWritten)
            | (
                ApplyPhase::ProjectionWritten,
                ApplyPhase::AuthorityCommitted
            )
    )
}

fn rollback_path(record: &LocalAssemblyRecord) -> Result<PathBuf> {
    let destination = Path::new(&record.destination);
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("collaboration_local_server_destination_parent_missing"))?;
    Ok(parent.join(rollback_name(&record.deployment_id)))
}

fn rollback_name(deployment_id: &str) -> String {
    format!(".licoup-local-server-apply-rollback-{deployment_id}")
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("collaboration_local_server_destination_encoding_invalid"))
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(anyhow!(
            "collaboration_local_server_transaction_path_state_unavailable"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplyPhase, advance, begin, read, recover, recover_committed};
    use crate::domain::collaboration_plugin::assembly::model::LocalServerLifecycle;
    use crate::domain::collaboration_plugin::assembly::store::find_record;
    use crate::domain::collaboration_plugin::assembly::tests::synthetic_record;
    use crate::platform::client_state::ClientStateStore;
    use uuid::Uuid;

    fn fixture(name: &str) -> (std::path::PathBuf, ClientStateStore) {
        let root = std::env::temp_dir().join(format!(
            "licoup-assembly-transaction-{name}-{}",
            Uuid::new_v4()
        ));
        let store = ClientStateStore::new(root.join("state")).unwrap();
        (root, store)
    }

    #[test]
    fn unavailable_authority_recovery_keeps_prepared_journal_intact() {
        let (root, store) = fixture("authority-unavailable");
        let record = synthetic_record(&root.join("assembly"), LocalServerLifecycle::Stopped);
        begin(&store, &record).unwrap();
        advance(&store, ApplyPhase::ArtifactWritten).unwrap();

        assert!(recover(&store).is_err());
        let pending = read(&store).unwrap().unwrap();
        assert_eq!(pending.phase, ApplyPhase::ArtifactWritten);
        assert_eq!(pending.record, record);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn committed_authority_recovers_missing_ordinary_projection_before_clear() {
        let (root, store) = fixture("projection-recovery");
        let record = synthetic_record(&root.join("assembly"), LocalServerLifecycle::Stopped);
        begin(&store, &record).unwrap();
        advance(&store, ApplyPhase::ArtifactWritten).unwrap();
        advance(&store, ApplyPhase::ProjectionWritten).unwrap();
        let pending = read(&store).unwrap().unwrap();

        recover_committed(&store, &pending, |_| Ok(())).unwrap();

        assert_eq!(find_record(&store, &record.deployment_id).unwrap(), record);
        assert!(read(&store).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
